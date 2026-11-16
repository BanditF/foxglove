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
//! - Ids are strings the hub's lease rules accept (contract section 8.5, v1.14): a counter, so
//!   no id is handed out twice in this process, and 64 random bits, so none can be guessed.
//! - Through the hub (#1120 slice C) every routed caller is [`ROUTED`]: the router keeps the
//!   callers apart (only a lease's holder may name it) and caps each one's leases
//!   (`MAX_LEASES_PER_CALLER`); `lease.closed` ends one lease ([`Leases::hub_closed`]), and the
//!   hub link dropping ends them all ([`Leases::drop_conn`] of [`ROUTED`]).

use std::collections::VecDeque;

use super::wallpaper_sync::Apply;

/// Leases one connection may hold at once.
pub const PER_CONNECTION: usize = 16;

/// The connection number of every call the hub routes (`hub_link::ROUTED_CONN`).
pub const ROUTED: u64 = super::hub_link::ROUTED_CONN;

/// Leases all routed callers together may hold. The router caps each caller at 16
/// (`hideout_router::MAX_LEASES_PER_CALLER`), which this module can't see; this is only the
/// module's own bound on its table, room for four callers at their cap.
pub const ROUTED_TOTAL: usize = 64;

/// Leases the hub ended (`lease.closed`) kept with what ending them did, so a `release` of the
/// same lease that was already on its way (the hub forwards the release, then closes) still
/// answers what happened. The oldest go past this.
const KEPT_ENDS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Lease {
    id: String,
    conn: u64,
    reason: String,
}

/// A new lease id: `we-<counter>-<64 random bits>`, at most 36 bytes, no control characters.
fn new_id(n: u64) -> String {
    let mut bytes = [0u8; 8];
    // Without randomness the counter alone keeps ids apart; the hub doesn't rely on guessing.
    let _ = getrandom::fill(&mut bytes);
    format!("we-{n:x}-{:016x}", u64::from_le_bytes(bytes))
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
    /// This connection holds [`PER_CONNECTION`] leases already ([`ROUTED_TOTAL`] for the hub's).
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
    /// Leases the hub ended, with what that did (see [`KEPT_ENDS`]).
    ended: VecDeque<(u64, String, Ended)>,
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
    ) -> Result<(String, bool), Fail> {
        let cap = if conn == ROUTED {
            ROUTED_TOTAL
        } else {
            PER_CONNECTION
        };
        if self.live.iter().filter(|l| l.conn == conn).count() >= cap {
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
        let id = new_id(self.next);
        self.live.push(Lease {
            id: id.clone(),
            conn,
            reason: reason.to_string(),
        });
        Ok((id, self.we_paused))
    }

    /// Ends one lease of `conn`. A lease the hub already ended answers what ending it did.
    pub fn release(
        &mut self,
        apply: &mut impl Apply,
        conn: u64,
        id: &str,
        full: bool,
    ) -> Result<Ended, Fail> {
        if !self.live.iter().any(|l| l.id == id && l.conn == conn) {
            let kept = self
                .ended
                .iter()
                .position(|(c, i, _)| *c == conn && i == id);
            return match kept.and_then(|k| self.ended.remove(k)) {
                Some((_, _, e)) => Ok(e),
                None => Err(Fail::Unknown),
            };
        }
        self.live.retain(|l| l.id != id);
        Ok(self.after(apply, full))
    }

    /// The hub's `lease.closed` (any reason: unknown ones mean ended too) for one lease of
    /// `conn`: it ends as a release would, the last one plays. `None` for an id that isn't live
    /// (released already, or never this connection's).
    pub fn hub_closed(
        &mut self,
        apply: &mut impl Apply,
        conn: u64,
        id: &str,
        full: bool,
    ) -> Option<Ended> {
        if !self.live.iter().any(|l| l.id == id && l.conn == conn) {
            return None;
        }
        self.live.retain(|l| l.id != id);
        let e = self.after(apply, full);
        if self.ended.len() >= KEPT_ENDS {
            self.ended.pop_front();
        }
        self.ended.push_back((conn, id.to_string(), e));
        Some(e)
    }

    /// Ends every lease of a connection that closed.
    pub fn drop_conn(&mut self, apply: &mut impl Apply, conn: u64, full: bool) -> Option<Ended> {
        self.ended.retain(|(c, _, _)| *c != conn);
        let before = self.live.len();
        self.live.retain(|l| l.conn != conn);
        (self.live.len() != before).then(|| self.after(apply, full))
    }

    /// Quitting: every lease ends.
    pub fn end_all(&mut self, apply: &mut impl Apply, full: bool) -> Option<Ended> {
        let had = !self.live.is_empty();
        self.live.clear();
        self.ended.clear();
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

        let e = l.release(&mut &mut f, 1, &a, false).unwrap();
        assert_eq!((e.played, e.why), (false, Some("held")));
        assert_eq!(f.calls, ["pause"], "the first release does not play");
        let e = l.release(&mut &mut f, 2, &b, false).unwrap();
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
        let e = l.release(&mut &mut f, 1, &id, false).unwrap();
        assert_eq!((e.played, e.why), (false, Some("user-paused")));
        assert!(f.calls.is_empty());
        assert_eq!(l.dropped, 1);
    }

    #[test]
    fn a_full_screen_program_in_front_defers_the_play_to_wallpaper_engine() {
        let (mut f, mut l) = (up(), Leases::default());
        let (id, _) = l.pause(&mut &mut f, 1, "a", false).unwrap();
        let e = l.release(&mut &mut f, 1, &id, true).unwrap();
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
        assert_eq!(l.release(&mut &mut f, 2, &id, false), Err(Fail::Unknown));
        assert_eq!(
            l.release(&mut &mut f, 1, "we-1-0", false),
            Err(Fail::Unknown)
        );
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

    #[test]
    fn ids_are_hub_lease_ids_and_never_repeat() {
        let (mut f, mut l) = (up(), Leases::default());
        let mut seen = std::collections::HashSet::new();
        for conn in 1..=3 {
            for _ in 0..PER_CONNECTION {
                let (id, _) = l.pause(&mut &mut f, conn, "a", false).unwrap();
                assert!(hideout_contract::lease::valid_id(&id), "{id}");
                assert!(id.starts_with("we-") && id.len() <= 36, "{id}");
                assert!(seen.insert(id), "an id handed out twice");
            }
        }
    }

    #[test]
    fn the_hubs_close_ends_one_lease_and_the_last_plays() {
        let (mut f, mut l) = (up(), Leases::default());
        let (a, _) = l.pause(&mut &mut f, ROUTED, "a", false).unwrap();
        let (b, _) = l.pause(&mut &mut f, ROUTED, "b", false).unwrap();
        let e = l.hub_closed(&mut &mut f, ROUTED, &a, false).unwrap();
        assert_eq!((e.played, e.why), (false, Some("held")));
        assert_eq!(l.reasons(), "b", "only that lease ended");
        // Again, or an id it never handed out, or another connection's: nothing.
        assert_eq!(l.hub_closed(&mut &mut f, ROUTED, &a, false), None);
        assert_eq!(l.hub_closed(&mut &mut f, ROUTED, "we-9-0", false), None);
        assert_eq!(l.hub_closed(&mut &mut f, 7, &b, false), None);
        let e = l.hub_closed(&mut &mut f, ROUTED, &b, false).unwrap();
        assert!(e.played && e.last);
        assert_eq!(f.calls, ["pause", "play"]);
    }

    #[test]
    fn a_release_that_was_on_its_way_when_the_hub_closed_answers_what_happened() {
        // The hub forwards a release, then sends lease.closed {released}: the close may run
        // first. The release still answers the play, once.
        let (mut f, mut l) = (up(), Leases::default());
        let (id, _) = l.pause(&mut &mut f, ROUTED, "a", false).unwrap();
        l.hub_closed(&mut &mut f, ROUTED, &id, false).unwrap();
        let e = l.release(&mut &mut f, ROUTED, &id, false).unwrap();
        assert!(e.played);
        assert_eq!(f.calls, ["pause", "play"], "one play");
        assert_eq!(
            l.release(&mut &mut f, ROUTED, &id, false),
            Err(Fail::Unknown)
        );
        // Another connection never gets an answer for it.
        let (id, _) = l.pause(&mut &mut f, ROUTED, "a", false).unwrap();
        l.hub_closed(&mut &mut f, ROUTED, &id, false).unwrap();
        assert_eq!(l.release(&mut &mut f, 3, &id, false), Err(Fail::Unknown));
        // The link dropping forgets what was kept.
        l.drop_conn(&mut &mut f, ROUTED, false);
        assert_eq!(
            l.release(&mut &mut f, ROUTED, &id, false),
            Err(Fail::Unknown)
        );
    }

    #[test]
    fn routed_callers_share_a_larger_backstop() {
        let (mut f, mut l) = (up(), Leases::default());
        for _ in 0..ROUTED_TOTAL {
            l.pause(&mut &mut f, ROUTED, "a", false).unwrap();
        }
        assert_eq!(l.pause(&mut &mut f, ROUTED, "a", false), Err(Fail::TooMany));
        // The router's per-caller cap is below it: four callers at their cap fill it.
        assert_eq!(ROUTED_TOTAL, 4 * hideout_contract::lease::MAX_PER_CALLER);
        assert_eq!(ROUTED, 0, "plugin-host connections count from 1");
    }
}
