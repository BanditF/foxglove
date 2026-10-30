//! The keyboard's underglow, saved once at the start of a suite run and put back at its end
//! (#618 follow-up). Cases such as right-relink-keeps-the-lefts-look set a look and may fail or be
//! interrupted before they end on the user's; every gate run used to leave 19/100/20 behind and
//! On-call put the user's look back by hand. One suite-level save/restore replaces per-case fixes.
//!
//! The restore runs from [`Saved`]'s `Drop`, so a failed case, a `?` out of the run loop, Ctrl-C
//! or a panic all end in it. It is bounded: three INVOKEs (colour, effect, on), each with the
//! link's own timeout, and a failure is logged, never raised.
use anyhow::Result;
use foxglove_link::{
    protocol::{InvokeStatus, RgbHalf},
    settings, Transport,
};

// `&rgb_ug` commands (ZMK's dt-bindings/zmk/rgb.h).
const RGB_ON: u32 = 1;
const RGB_EFFECT_SET: u32 = 13;
const RGB_COLOR_HSB: u32 = 14;

/// The left half's underglow as read at the start. The left is the one the user (and sync) sets;
/// the right follows it over the relay.
pub struct Saved<'a, T: Transport> {
    transport: &'a T,
    rgb_ug: u16,
    look: RgbHalf,
    done: bool,
}

impl<'a, T: Transport> Saved<'a, T> {
    /// The suite run's guard: none when the keymap has no `&rgb_ug`, or with `keep` (`run
    /// --keep-lights`). A script that changes the look on the keyboard's side on purpose (#677:
    /// `scripts/lab/lights_page.py` runs one RGB_EFF per suite) keeps it: the restore at the end
    /// of each run put the old effect straight back, so RGB_EFF looked like it did nothing. Such a
    /// script puts the user's lights back itself.
    pub fn for_run(transport: &'a T, rgb_ug: Option<u16>, keep: bool) -> Option<Self> {
        if keep {
            println!("lights kept: --keep-lights, the run leaves the underglow as its cases do");
            return None;
        }
        rgb_ug.and_then(|id| Self::save(transport, id))
    }

    /// Reads RGB_STATE (D7 83) once and logs one line. `None` (and a line saying why) when the
    /// read fails: the run goes on, with nothing to put back.
    pub fn save(transport: &'a T, rgb_ug: u16) -> Option<Self> {
        match settings::rgb_state(transport) {
            Ok(s) => {
                println!("lights saved: {}", describe(&s.left));
                Some(Self {
                    transport,
                    rgb_ug,
                    look: s.left,
                    done: false,
                })
            }
            Err(e) => {
                println!("lights NOT saved (RGB_STATE: {e:#}); the run will not restore them");
                None
            }
        }
    }

    /// Puts the saved look back (once) and logs one line. Colour and effect always; the lights
    /// are switched on if they were on, and never switched off: a half dark from idle at the
    /// save would otherwise stay off until the next RGB_ON.
    pub fn restore(&mut self) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        let l = self.look;
        let hsb = (u32::from(l.hue) << 16) | (u32::from(l.sat) << 8) | u32::from(l.brightness);
        let mut sent = vec![
            (RGB_COLOR_HSB, hsb, "colour"),
            (RGB_EFFECT_SET, u32::from(l.effect), "effect"),
        ];
        if l.on {
            sent.push((RGB_ON, 0, "on"));
        }
        let failed: Vec<String> = sent
            .iter()
            .filter_map(|&(p1, p2, what)| match self.send(p1, p2) {
                Ok(InvokeStatus::Ok) => None,
                Ok(s) => Some(format!("{what}: {s:?}")),
                Err(e) => Some(format!("{what}: {e:#}")),
            })
            .collect();
        if failed.is_empty() {
            println!("lights restored: {}", describe(&l));
        } else {
            println!(
                "lights NOT fully restored ({}); wanted {}",
                failed.join(", "),
                describe(&l)
            );
        }
    }

    // Busy: the invoke before's release hasn't run yet (it goes to the right through ZMK's
    // split queue): asked again every 50 ms for at most 1.5 s, as the runner's invoke step does.
    fn send(&self, param1: u32, param2: u32) -> Result<InvokeStatus> {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        loop {
            let s = settings::invoke(self.transport, self.rgb_ug, param1, param2, 0)?;
            if s != InvokeStatus::Busy || std::time::Instant::now() >= end {
                return Ok(s);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

impl<T: Transport> Drop for Saved<'_, T> {
    fn drop(&mut self) {
        self.restore();
    }
}

fn describe(h: &RgbHalf) -> String {
    let effect = match h.effect {
        0 => "Solid",
        1 => "Breathe",
        2 => "Spectrum",
        3 => "Swirl",
        _ => "effect ?",
    };
    format!(
        "{} {effect} {}/{}/{}",
        if h.on { "on" } else { "off" },
        h.hue,
        h.sat,
        h.brightness
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol::{settings as cmd, REPORT};
    use std::{cell::RefCell, collections::VecDeque, time::Duration};

    /// Answers RGB_STATE from `state` and INVOKE with `invoke_status` (255: the link is gone),
    /// recording every request.
    struct Fake {
        state: RefCell<VecDeque<[u8; REPORT]>>,
        invoke_status: u8,
        requests: RefCell<Vec<Vec<u8>>>,
    }
    impl Transport for Fake {
        fn send(&self, _: &[u8]) -> Result<()> {
            unreachable!()
        }
        fn request(&self, r: &[u8], p: u8, c: u8, _: Duration) -> Result<[u8; REPORT]> {
            self.requests.borrow_mut().push(r.to_vec());
            if c == cmd::RGB_STATE {
                return self
                    .state
                    .borrow_mut()
                    .pop_front()
                    .ok_or_else(|| anyhow::anyhow!("scripted timeout"));
            }
            if self.invoke_status == 255 {
                anyhow::bail!("link gone");
            }
            let mut out = [0; REPORT];
            out[..2].copy_from_slice(&[p, c | 0x80]);
            out[2] = self.invoke_status;
            Ok(out)
        }
    }

    /// RGB_STATE with the left on/off at 23/100/42 Swirl (the user's look), no right half yet.
    fn user_look(on: bool) -> [u8; REPORT] {
        let mut r = [0; REPORT];
        r[..9].copy_from_slice(&[0xd7, 0x83, on as u8, 23, 0, 100, 42, 3, 2]);
        r[9] = 0xff;
        r
    }
    fn fake(state: Vec<[u8; REPORT]>, invoke_status: u8) -> Fake {
        Fake {
            state: RefCell::new(state.into()),
            invoke_status,
            requests: RefCell::default(),
        }
    }
    /// The (param1, param2) of each INVOKE sent, in order.
    fn invokes(f: &Fake) -> Vec<(u32, u32)> {
        f.requests
            .borrow()
            .iter()
            .filter(|r| r[1] == cmd::INVOKE)
            .map(|r| {
                (
                    u32::from_le_bytes(r[4..8].try_into().unwrap()),
                    u32::from_le_bytes(r[8..12].try_into().unwrap()),
                )
            })
            .collect()
    }
    const USER_HSB: u32 = (23 << 16) | (100 << 8) | 42;

    #[test]
    fn restores_colour_effect_and_on_when_dropped() {
        let f = fake(vec![user_look(true)], 0);
        drop(Saved::save(&f, 7));
        assert_eq!(
            invokes(&f),
            [(RGB_COLOR_HSB, USER_HSB), (RGB_EFFECT_SET, 3), (RGB_ON, 0)]
        );
        // One read at the save, three writes at the restore.
        assert_eq!(f.requests.borrow().len(), 4);
    }

    #[test]
    fn restore_runs_when_the_run_fails_out_of_a_scope() {
        let f = fake(vec![user_look(true)], 0);
        let run = || -> Result<()> {
            let _saved = Saved::save(&f, 7);
            anyhow::bail!("a case blew up");
        };
        assert!(run().is_err());
        assert_eq!(invokes(&f).len(), 3);
    }

    #[test]
    fn restore_runs_after_a_panic() {
        let f = fake(vec![user_look(true)], 0);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _saved = Saved::save(&f, 7);
            panic!("abort");
        }));
        assert!(r.is_err());
        assert_eq!(invokes(&f).len(), 3);
    }

    #[test]
    fn restores_once_even_if_called_then_dropped() {
        let f = fake(vec![user_look(true)], 0);
        let mut saved = Saved::save(&f, 7).unwrap();
        saved.restore();
        drop(saved);
        assert_eq!(invokes(&f).len(), 3);
    }

    #[test]
    fn lights_dark_at_the_save_are_never_switched_off() {
        let f = fake(vec![user_look(false)], 0);
        drop(Saved::save(&f, 7));
        assert_eq!(
            invokes(&f),
            [(RGB_COLOR_HSB, USER_HSB), (RGB_EFFECT_SET, 3)]
        );
    }

    #[test]
    fn a_failed_restore_is_logged_not_raised_and_tries_every_command() {
        // Refused by the keyboard: all three commands are still sent.
        let f = fake(vec![user_look(true)], 6);
        drop(Saved::save(&f, 7));
        assert_eq!(invokes(&f).len(), 3);
        // The link gone: each command fails, nothing panics.
        let f = fake(vec![user_look(true)], 255);
        drop(Saved::save(&f, 7));
    }

    /// #677: lights_page.py runs `&rgb_ug RGB_EFF` as a one-case suite. The run's restore sent
    /// the saved effect (Swirl) right after the case's RGB_EFF, so the keyboard never stayed on
    /// Solid. A kept run reads nothing and sends nothing: the case's effect stays.
    #[test]
    fn a_kept_run_leaves_a_keyboard_side_effect_change_alone() {
        // RGB_EFF from Swirl, as the case does, then the run ends.
        let f = fake(vec![user_look(true)], 0);
        drop(Saved::for_run(&f, Some(7), false));
        assert!(
            invokes(&f).contains(&(RGB_EFFECT_SET, 3)),
            "the default run puts Swirl back over the case's RGB_EFF"
        );
        let f = fake(vec![user_look(true)], 0);
        assert!(Saved::for_run(&f, Some(7), true).is_none());
        assert!(f.requests.borrow().is_empty(), "nothing read or sent");
        // A keymap without &rgb_ug has nothing to save either way.
        let f = fake(vec![user_look(true)], 0);
        assert!(Saved::for_run(&f, None, false).is_none());
        assert!(f.requests.borrow().is_empty());
    }

    #[test]
    fn no_saved_state_means_nothing_to_restore() {
        let f = fake(vec![], 0);
        assert!(Saved::save(&f, 7).is_none());
        assert!(invokes(&f).is_empty());
    }
}
