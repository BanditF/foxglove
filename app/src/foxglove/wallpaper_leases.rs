//! Who holds Wallpaper Engine paused (#1120, H4.6): the lease table behind the local API's
//! `wallpaper-engine.pause` / `.release` / `.status`. Foxglove's sync worker is the only sender
//! of Wallpaper Engine's command line; this table only decides what the worker sends, and the
//! worker runs it between pushes, so a pause and an `applyProperties` never overlap.
//!
//! - The first lease sends `pause`; further ones only join. A lease ends by `release` or with
//!   its connection; `play` goes out when the last one ends.
//! - `play` never goes out over the user's own pause (Wallpaper Engine was already paused when
//!   the first lease came: a paired wallpaper had reported `Paused`), or with a full-screen
//!   program in front (Wallpaper Engine's own `playbackfullscreen` brings it back then).
//! - If the pause command fails, no lease is taken.

use super::wallpaper_sync::Apply;

/// Leases one connection may hold at once.
pub const PER_CONNECTION: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Lease {
    id: u64,
    conn: u64,
    reason: String,
}

/// Why a call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// Wallpaper Engine isn't installed.
    NotInstalled,
    /// Wallpaper Engine isn't running: its command line would start it.
    NotRunning,
    /// The command failed or timed out.
    Command(String),
    /// Not a lease of this connection.
    Unknown,
    /// This connection holds [`PER_CONNECTION`] leases already.
    TooMany,
}

/// What ending leases did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ended {
    /// `play` went out.
    pub played: bool,
    /// Why it didn't: `held` (other leases remain), `user-paused` or `fullscreen`.
    pub why: Option<&'static str>,
    /// The last lease ended, whatever was sent.
    pub last: bool,
}

#[derive(Default)]
pub struct Leases {
    next: u64,
    live: Vec<Lease>,
    /// Foxglove sent the `pause` that holds Wallpaper Engine now (it wasn't paused before).
    we_paused: bool,
    pub paused: u64,
    pub played: u64,
    pub dropped: u64,
}

impl Leases {
    /// Takes a lease. The first one pauses, unless the user had (`user_paused`). Answers the
    /// lease and whether Wallpaper Engine is paused on its account (`false`: the user's pause,
    /// which this lease will not play over).
    pub fn pause(
        &mut self,
        apply: &mut impl Apply,
        conn: u64,
        reason: &str,
        user_paused: bool,
    ) -> Result<(u64, bool), Fail> {
        if self.live.iter().filter(|l| l.conn == conn).count() >= PER_CONNECTION {
            return Err(Fail::TooMany);
        }
        if !apply.ready() {
            return Err(Fail::NotRunning);
        }
        if self.live.is_empty() && !user_paused {
            apply.control("pause").map_err(Fail::Command)?;
            self.we_paused = true;
            self.paused += 1;
        }
        self.next += 1;
        let id = self.next;
        self.live.push(Lease {
            id,
            conn,
            reason: reason.to_string(),
        });
        Ok((id, self.we_paused))
    }

    /// Ends one lease of `conn`.
    pub fn release(
        &mut self,
        apply: &mut impl Apply,
        conn: u64,
        id: u64,
        full: bool,
    ) -> Result<Ended, Fail> {
        if !self.live.iter().any(|l| l.id == id && l.conn == conn) {
            return Err(Fail::Unknown);
        }
        self.live.retain(|l| l.id != id);
        Ok(self.after(apply, full))
    }

    /// Ends every lease of a connection that closed.
    pub fn drop_conn(&mut self, apply: &mut impl Apply, conn: u64, full: bool) -> Option<Ended> {
        let before = self.live.len();
        self.live.retain(|l| l.conn != conn);
        (self.live.len() != before).then(|| self.after(apply, full))
    }

    /// Quitting: every lease ends.
    pub fn end_all(&mut self, apply: &mut impl Apply, full: bool) -> Option<Ended> {
        let had = !self.live.is_empty();
        self.live.clear();
        had.then(|| self.after(apply, full))
    }

    fn after(&mut self, apply: &mut impl Apply, full: bool) -> Ended {
        if !self.live.is_empty() {
            return Ended {
                why: Some("held"),
                ..Ended::default()
            };
        }
        let mine = std::mem::take(&mut self.we_paused);
        let last = |played, why| Ended {
            played,
            why,
            last: true,
        };
        if !mine {
            self.dropped += 1;
            return last(false, Some("user-paused"));
        }
        if full {
            self.dropped += 1;
            return last(false, Some("fullscreen"));
        }
        // A play that can't go out (Wallpaper Engine quit meanwhile, or no answer) leaves
        // nothing to hold: the user's next play or Wallpaper Engine's restart does it.
        match apply.ready().then(|| apply.control("play")) {
            Some(Ok(())) => {
                self.played += 1;
                last(true, None)
            }
            Some(Err(e)) => {
                eprintln!("Foxglove: wallpaper sync: play failed: {e}");
                last(false, None)
            }
            None => last(false, None),
        }
    }

    pub fn count(&self) -> u32 {
        self.live.len() as u32
    }

    /// Wallpaper Engine is paused by a lease Foxglove sent.
    pub fn paused_by_lease(&self) -> bool {
        self.we_paused
    }

    /// The live leases' reasons, for the `wallpaper_engine.paused_by` fact.
    pub fn reasons(&self) -> String {
        let mut r: Vec<&str> = self.live.iter().map(|l| l.reason.as_str()).collect();
        r.dedup();
        r.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        running: bool,
        calls: Vec<String>,
        fail: bool,
    }

    impl Apply for &mut Fake {
        fn ready(&mut self) -> bool {
            self.running
        }
        fn apply(&mut self, _: &str, _: &str) -> Result<(), String> {
            unreachable!("leases never push properties")
        }
        fn control(&mut self, command: &str) -> Result<(), String> {
            if self.fail {
                return Err("refused".into());
            }
            self.calls.push(command.into());
            Ok(())
        }
        fn interfaces(&mut self) -> Vec<(String, String)> {
            vec![]
        }
    }

    fn up() -> Fake {
        Fake {
            running: true,
            ..Fake::default()
        }
    }

    #[test]
    fn the_first_lease_pauses_once_and_the_last_release_plays_once() {
        let (mut f, mut l) = (up(), Leases::default());
        let (a, paused) = l.pause(&mut &mut f, 1, "model loaded", false).unwrap();
        assert!(paused);
        let (b, paused) = l.pause(&mut &mut f, 2, "render", false).unwrap();
        assert!(paused);
        assert_eq!(f.calls, ["pause"], "the second lease sends nothing");
        assert_eq!(l.count(), 2);
        assert_eq!(l.reasons(), "model loaded; render");

        let e = l.release(&mut &mut f, 1, a, false).unwrap();
        assert_eq!((e.played, e.why), (false, Some("held")));
        assert_eq!(f.calls, ["pause"], "the first release does not play");
        let e = l.release(&mut &mut f, 2, b, false).unwrap();
        assert_eq!((e.played, e.why, e.last), (true, None, true));
        assert_eq!(f.calls, ["pause", "play"]);
        assert_eq!((l.paused, l.played, l.dropped), (1, 1, 0));
    }

    #[test]
    fn a_connection_ending_ends_its_leases() {
        let (mut f, mut l) = (up(), Leases::default());
        l.pause(&mut &mut f, 7, "a", false).unwrap();
        l.pause(&mut &mut f, 7, "b", false).unwrap();
        let e = l.drop_conn(&mut &mut f, 7, false).unwrap();
        assert!(e.played);
        assert_eq!(f.calls, ["pause", "play"]);
        assert_eq!(l.drop_conn(&mut &mut f, 7, false), None, "nothing left");
    }

    #[test]
    fn the_users_own_pause_is_not_played_over() {
        let (mut f, mut l) = (up(), Leases::default());
        let (id, paused) = l.pause(&mut &mut f, 1, "a", true).unwrap();
        assert!(!paused, "already paused: this lease will not play");
        assert!(f.calls.is_empty(), "and sends no pause");
        let e = l.release(&mut &mut f, 1, id, false).unwrap();
        assert_eq!((e.played, e.why), (false, Some("user-paused")));
        assert!(f.calls.is_empty());
        assert_eq!(l.dropped, 1);
    }

    #[test]
    fn a_full_screen_program_in_front_defers_the_play_to_wallpaper_engine() {
        let (mut f, mut l) = (up(), Leases::default());
        let (id, _) = l.pause(&mut &mut f, 1, "a", false).unwrap();
        let e = l.release(&mut &mut f, 1, id, true).unwrap();
        assert_eq!((e.played, e.why), (false, Some("fullscreen")));
        assert_eq!(f.calls, ["pause"]);
        assert!(
            !l.paused_by_lease(),
            "the lease is over; the play is not ours"
        );
        assert_eq!(l.dropped, 1);
    }

    #[test]
    fn unknown_or_foreign_leases_are_refused_and_nothing_runs_when_it_is_not() {
        let (mut f, mut l) = (up(), Leases::default());
        let (id, _) = l.pause(&mut &mut f, 1, "a", false).unwrap();
        assert_eq!(l.release(&mut &mut f, 2, id, false), Err(Fail::Unknown));
        assert_eq!(l.release(&mut &mut f, 1, id + 9, false), Err(Fail::Unknown));
        assert_eq!(l.count(), 1);

        let (mut down, mut l) = (Fake::default(), Leases::default());
        assert_eq!(
            l.pause(&mut &mut down, 1, "a", false),
            Err(Fail::NotRunning)
        );
        assert!(down.calls.is_empty());
        let (mut bad, mut l) = (Fake { fail: true, ..up() }, Leases::default());
        assert_eq!(
            l.pause(&mut &mut bad, 1, "a", false),
            Err(Fail::Command("refused".into()))
        );
        assert_eq!(l.count(), 0, "a failed pause takes no lease");
    }

    #[test]
    fn quitting_plays_for_the_leases_that_are_left() {
        let (mut f, mut l) = (up(), Leases::default());
        assert_eq!(l.end_all(&mut &mut f, false), None);
        l.pause(&mut &mut f, 1, "a", false).unwrap();
        assert!(l.end_all(&mut &mut f, false).unwrap().played);
        assert_eq!(f.calls, ["pause", "play"]);
    }

    #[test]
    fn one_connection_cannot_hold_without_limit() {
        let (mut f, mut l) = (up(), Leases::default());
        for _ in 0..PER_CONNECTION {
            l.pause(&mut &mut f, 1, "a", false).unwrap();
        }
        assert_eq!(l.pause(&mut &mut f, 1, "a", false), Err(Fail::TooMany));
        l.pause(&mut &mut f, 2, "a", false).unwrap();
    }
}
