//! The PC state for idle waits (#618 follow-up after #651). Since #651's firmware a keyboard on
//! USB stays lit while the PC is awake: a lease from the app, or 10 minutes after the last key
//! without one. A case that taps a key and then waits up to 45 s for the left to idle could never
//! pass plugged in (the soak's idle -> wake, resilience's right-follows-left-activity,
//! rgb-state-follows-the-leds and right-redraw-after-wake). An `expect_rgb` with `pc_away = true`
//! first tells the keyboard the PC is away (D7 11 away, as `foxglove-lab pc-state --set away`),
//! which idles it at once; the next key ends that, as on a real PC.
//!
//! What the keyboard had is read once before the first away of a case and put back when the case
//! ends, whatever the case did: the runner's cleanup runs after a failed or interrupted (Ctrl-C)
//! case, and [`Saved`]'s `Drop` covers a `?` out of the run or a panic. The restore is one D7 11
//! with the link's own timeout; a failure is logged, never raised.
//!
//! Put back as:
//!  - awake with a lease: awake with what was left of it (the app renews it as before);
//!  - away: away;
//!  - none (no lease): awake for 1 s without the wake flag, which ends the away and lapses back
//!    to none. The keyboard takes no "none" request, and the next key would end the away anyway.
use std::time::{Duration, Instant};

use anyhow::Result;
use foxglove_link::{
    protocol::{self, PcState, PcStateKind},
    settings, Transport,
};

/// The PC state read before the first away.
pub struct Saved<'a, T: Transport> {
    transport: &'a T,
    state: PcStateKind,
    lease_s: u16,
    at: Instant,
    done: bool,
}

/// One line for the report: the state and the lease.
pub fn describe(s: &PcState) -> String {
    match s.state {
        PcStateKind::Awake => format!("awake, lease {} s", s.lease_s),
        PcStateKind::Away => "away".into(),
        PcStateKind::None => "none (no lease)".into(),
        PcStateKind::Unknown(v) => format!("unknown ({v})"),
    }
}

impl<'a, T: Transport> Saved<'a, T> {
    /// Reads the PC state, then sends away. Returns the saved state and what happened.
    pub fn away(transport: &'a T) -> Result<(Self, String)> {
        let before = settings::pc_state(transport, &protocol::pc_read())?;
        anyhow::ensure!(
            before.status == 0,
            "PC_STATE read: status {}",
            before.status
        );
        let saved = Self {
            transport,
            state: before.state,
            lease_s: before.lease_s,
            at: Instant::now(),
            done: false,
        };
        let after = settings::pc_state(transport, &protocol::pc_away())?;
        anyhow::ensure!(after.status == 0, "PC_STATE away: status {}", after.status);
        let line = format!(
            "PC state away (was {}; put back when the case ends)",
            describe(&before)
        );
        println!("{line}");
        Ok((saved, line))
    }

    /// Another away within the same case (the saved state stays the first one).
    pub fn again(&self) -> Result<String> {
        let after = settings::pc_state(self.transport, &protocol::pc_away())?;
        anyhow::ensure!(after.status == 0, "PC_STATE away: status {}", after.status);
        Ok("PC state away (again)".into())
    }

    /// The request that puts the saved state back now.
    fn request(&self) -> Option<Vec<u8>> {
        match self.state {
            PcStateKind::Awake => {
                let left =
                    Duration::from_secs(self.lease_s.into()).saturating_sub(self.at.elapsed());
                Some(protocol::pc_awake(left.as_secs().max(1) as u16, false))
            }
            PcStateKind::Away => Some(protocol::pc_away()),
            PcStateKind::None => Some(protocol::pc_awake(1, false)),
            PcStateKind::Unknown(_) => None,
        }
    }

    /// Puts the saved state back (once) and logs one line.
    pub fn restore(&mut self) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        let Some(r) = self.request() else {
            println!("PC state NOT restored: the saved state was unknown");
            return;
        };
        match settings::pc_state(self.transport, &r) {
            Ok(s) if s.status == 0 => println!("PC state restored: {}", describe(&s)),
            Ok(s) => println!("PC state NOT restored: status {}", s.status),
            Err(e) => println!("PC state NOT restored: {e:#}"),
        }
    }
}

impl<T: Transport> Drop for Saved<'_, T> {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol::{settings as cmd, REPORT};
    use std::cell::RefCell;

    /// Answers every PC_STATE with `state` and `lease_s`, recording each request.
    struct Fake {
        state: u8,
        lease_s: u16,
        requests: RefCell<Vec<Vec<u8>>>,
    }
    impl Transport for Fake {
        fn send(&self, _: &[u8]) -> Result<()> {
            unreachable!()
        }
        fn request(&self, r: &[u8], p: u8, c: u8, _: Duration) -> Result<[u8; REPORT]> {
            assert_eq!(c, cmd::PC_STATE);
            self.requests.borrow_mut().push(r.to_vec());
            let mut out = [0; REPORT];
            out[..2].copy_from_slice(&[p, c | 0x80]);
            out[4] = self.state;
            out[6..8].copy_from_slice(&self.lease_s.to_le_bytes());
            Ok(out)
        }
    }
    fn fake(state: u8, lease_s: u16) -> Fake {
        Fake {
            state,
            lease_s,
            requests: RefCell::default(),
        }
    }
    fn sent(f: &Fake) -> Vec<Vec<u8>> {
        f.requests.borrow().clone()
    }

    #[test]
    fn a_lease_comes_back_with_what_was_left_of_it() {
        let f = fake(1, 600);
        let (saved, line) = Saved::away(&f).unwrap();
        assert!(line.contains("was awake, lease 600 s"), "{line}");
        drop(saved);
        let r = sent(&f);
        assert_eq!(r[0], protocol::pc_read());
        assert_eq!(r[1], protocol::pc_away());
        // Awake, no wake flag, the lease left (600 s less the few ms of the test).
        assert_eq!(&r[2][..4], &[0xD7, 0x11, 1, 0]);
        let lease = u16::from_le_bytes([r[2][4], r[2][5]]);
        assert!((599..=600).contains(&lease), "{lease}");
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn no_lease_comes_back_as_a_one_second_lease_that_lapses() {
        let f = fake(0, 0);
        drop(Saved::away(&f).unwrap().0);
        assert_eq!(sent(&f)[2], protocol::pc_awake(1, false));
    }

    #[test]
    fn away_comes_back_as_away_and_restores_once() {
        let f = fake(2, 0);
        let (mut saved, _) = Saved::away(&f).unwrap();
        saved.again().unwrap();
        saved.restore();
        drop(saved); // the Drop after an explicit restore sends nothing more
        let r = sent(&f);
        assert_eq!(
            r[1..],
            [
                protocol::pc_away(),
                protocol::pc_away(),
                protocol::pc_away()
            ]
        );
    }

    #[test]
    fn the_restore_runs_when_a_run_fails_out_of_a_scope() {
        let f = fake(1, 300);
        let run = || -> Result<()> {
            let _saved = Saved::away(&f)?;
            anyhow::bail!("a case blew up");
        };
        assert!(run().is_err());
        assert_eq!(sent(&f).len(), 3);
    }
}
