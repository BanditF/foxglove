//! Re-evaluating the default output device when an endpoint arrives or the PC resumes.
//!
//! Windows doesn't always send "default device changed" when an output endpoint comes back (a
//! dongle replug), or sends it before the endpoint can report its volume support, so the software
//! master volume (#1064) never started and the keyboard's plain volume keys did nothing audible.
//! This is the decision logic, free of Windows: the notifications become [`Event`]s, and
//! [`Follow`] says when to look at the default device again. Looks are event-gated and bounded:
//! an arrival (or a resume) starts a short, backed-off series of looks that ends when the device
//! is held or after [`BACKOFF`] runs out; nothing runs between episodes.
use std::time::{Duration, Instant};

/// The waits before each look after the first one (which is at once): the hard cap on a series.
pub const BACKOFF: [Duration; 5] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// What Windows told us, by endpoint id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// An endpoint was added.
    Added(String),
    /// An endpoint's state changed; `active` when it is now usable.
    State { id: String, active: bool },
    /// An endpoint was removed.
    Removed(String),
    /// The PC resumed from standby.
    Resumed,
}

/// What the sessions' thread does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// Look at the default device again: if it is not the one followed, switch to it; if it is,
    /// only evaluate again (no release, so no off/on lines for an unchanged device). `force`
    /// switches in any case (after standby the old objects can be stale).
    Look {
        force: bool,
    },
}

#[derive(Debug)]
struct Pending {
    /// The endpoint expected to become the default (`None`: any, after a resume).
    want: Option<String>,
    /// Looks made after the first one.
    retries: usize,
    due: Instant,
    /// The first look is not made yet.
    fresh: bool,
}

#[derive(Debug, Default)]
pub struct Follow {
    pending: Option<Pending>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Follow {
    /// An event. `current` is the endpoint the master volume follows now.
    pub fn event(&mut self, ev: Event, current: Option<&str>, now: Instant) -> Act {
        match ev {
            Event::Added(id) | Event::State { id, active: true } => self.expect(Some(id), now),
            Event::Resumed => match self.expect(None, now) {
                Act::Look { .. } => Act::Look { force: true },
                Act::Nothing => Act::Nothing,
            },
            Event::Removed(id) | Event::State { id, active: false } => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.want.as_deref().is_some_and(|w| same(w, &id)))
                {
                    self.pending = None;
                }
                // The device in use went: look now, so master disengages cleanly (Windows also
                // names a new default, which looks again; that is harmless, nothing changes).
                if current.is_some_and(|c| same(c, &id)) {
                    Act::Look { force: true }
                } else {
                    Act::Nothing
                }
            }
        }
    }

    fn expect(&mut self, want: Option<String>, now: Instant) -> Act {
        // The same endpoint announced again (added, then active) while a series runs: one series.
        if let Some(p) = &self.pending {
            let dup = match (&p.want, &want) {
                (Some(a), Some(b)) => same(a, b),
                (None, None) => true,
                _ => false,
            };
            if dup {
                return Act::Nothing;
            }
        }
        self.pending = Some(Pending {
            want,
            retries: 0,
            due: now,
            fresh: true,
        });
        Act::Look { force: false }
    }

    /// A look was made: `default` is the default device now, `held` whether the master volume
    /// follows it and holds it. Ends the series when held, else schedules the next look, until
    /// the backoff runs out.
    pub fn looked(&mut self, default: Option<&str>, held: bool, now: Instant) {
        let Some(p) = self.pending.as_mut() else {
            return;
        };
        p.fresh = false;
        let arrived = match (&p.want, default) {
            (Some(w), Some(d)) => same(w, d),
            (Some(_), None) => false,
            (None, d) => d.is_some(),
        };
        if arrived && held {
            self.pending = None;
            return;
        }
        match BACKOFF.get(p.retries) {
            Some(wait) => {
                p.retries += 1;
                p.due = now + *wait;
            }
            None => self.pending = None,
        }
    }

    /// When the next look is due, if a series runs.
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|p| p.due)
    }

    /// The wait is over: look now (the caller then calls [`Follow::looked`]).
    pub fn due(&mut self, now: Instant) -> Act {
        match &self.pending {
            Some(p) if now >= p.due && !p.fresh => Act::Look { force: false },
            _ => Act::Nothing,
        }
    }

    /// Whether a series runs.
    pub fn active(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JBL: &str = "{0.0.0.00000000}.{jbl}";
    const SPK: &str = "{0.0.0.00000000}.{speakers}";

    /// A fake Windows plus the master volume: the default device, which endpoints can be held,
    /// and the one master follows. The log has the on/off lines, one per transition.
    struct Sim {
        follow: Follow,
        now: Instant,
        default: Option<&'static str>,
        /// Endpoints that hand their volume to the hardware (eligible for the master).
        eligible: Vec<&'static str>,
        /// The device master holds.
        held: Option<&'static str>,
        /// The device the worker has opened.
        opened: Option<&'static str>,
        log: Vec<String>,
        looks: usize,
    }

    impl Sim {
        fn new() -> Sim {
            Sim {
                follow: Follow::default(),
                now: Instant::now(),
                default: Some(SPK),
                eligible: vec![JBL],
                held: None,
                opened: Some(SPK),
                log: vec![],
                looks: 0,
            }
        }
        fn send(&mut self, ev: Event) {
            let current = self.opened;
            let act = self.follow.event(ev, current, self.now);
            self.run(act);
        }
        fn run(&mut self, act: Act) {
            let Act::Look { force } = act else { return };
            self.looks += 1;
            // The worker: switch if the default isn't what it follows (or forced), else only
            // evaluate again. Engage and disengage log on a transition only.
            if force || self.opened != self.default {
                if let Some(h) = self.held.take() {
                    self.log.push(format!("off {h}"));
                }
                self.opened = self.default;
            }
            if let Some(d) = self.opened {
                if self.eligible.contains(&d) && self.held != Some(d) {
                    self.held = Some(d);
                    self.log.push(format!("on {d}"));
                }
            }
            let held = self.held.is_some() && self.held == self.opened;
            self.follow.looked(self.default, held, self.now);
        }
        /// Time passes to the next due look, which is made.
        fn tick(&mut self) -> bool {
            let Some(at) = self.follow.next_due() else {
                return false;
            };
            self.now = at;
            let act = self.follow.due(self.now);
            self.run(act);
            true
        }
        fn drain(&mut self) {
            while self.tick() {}
        }
    }

    fn on(d: &str) -> String {
        format!("on {d}")
    }
    fn off(d: &str) -> String {
        format!("off {d}")
    }

    #[test]
    fn an_endpoint_that_arrives_as_the_default_engages_once() {
        let mut s = Sim::new();
        s.default = Some(JBL);
        s.send(Event::Added(JBL.into()));
        assert_eq!(s.log, [on(JBL)]);
        // Announced again as active: nothing more.
        s.send(Event::State {
            id: JBL.into(),
            active: true,
        });
        s.drain();
        assert_eq!(s.log.len(), 1);
        assert!(!s.follow.active());
    }

    #[test]
    fn an_endpoint_that_becomes_the_default_later_engages_when_it_does() {
        let mut s = Sim::new();
        s.send(Event::Added(JBL.into()));
        // Not the default yet: nothing engaged, a series runs.
        assert!(s.log.is_empty());
        assert!(s.follow.active());
        // The first retry still sees the speakers; the default then moves.
        assert!(s.tick());
        assert!(s.log.is_empty());
        s.default = Some(JBL);
        s.drain();
        assert_eq!(s.log, [on(JBL)]);
        assert!(!s.follow.active());
    }

    #[test]
    fn a_device_not_ready_at_first_is_held_on_a_retry() {
        // Default at once, but it can't report hardware volume support yet.
        let mut s = Sim::new();
        s.default = Some(JBL);
        s.eligible.clear();
        s.send(Event::Added(JBL.into()));
        assert!(s.log.is_empty());
        s.tick();
        s.eligible.push(JBL);
        s.drain();
        assert_eq!(s.log, [on(JBL)]);
    }

    #[test]
    fn retries_stop_at_the_cap() {
        let mut s = Sim::new();
        s.send(Event::Added(JBL.into()));
        s.drain();
        // One look at once, then one per backoff step, and no more.
        assert_eq!(s.looks, 1 + BACKOFF.len());
        assert!(s.log.is_empty());
        assert!(!s.follow.active());
        assert_eq!(s.follow.next_due(), None);
        // Time passing alone does nothing.
        assert_eq!(
            s.follow.due(s.now + Duration::from_secs(3600)),
            Act::Nothing
        );
    }

    #[test]
    fn repeated_events_make_one_series_and_no_double_engage() {
        let mut s = Sim::new();
        s.send(Event::Added(JBL.into()));
        let looks = s.looks;
        for _ in 0..5 {
            s.send(Event::Added(JBL.into()));
            s.send(Event::State {
                id: JBL.to_uppercase(),
                active: true,
            });
        }
        assert_eq!(s.looks, looks, "duplicates don't look again");
        s.default = Some(JBL);
        s.drain();
        assert_eq!(s.log, [on(JBL)]);
        // An event after it is held: a new series, but no new line.
        s.send(Event::State {
            id: JBL.into(),
            active: true,
        });
        s.drain();
        assert_eq!(s.log.len(), 1);
    }

    #[test]
    fn removal_disengages_cleanly_and_a_return_engages_again() {
        let mut s = Sim::new();
        s.default = Some(JBL);
        s.send(Event::Added(JBL.into()));
        assert_eq!(s.log.len(), 1);
        // Unplugged: Windows falls back to the speakers.
        s.default = Some(SPK);
        s.send(Event::Removed(JBL.into()));
        s.drain();
        assert_eq!(s.log, [on(JBL), off(JBL)]);
        assert_eq!(s.held, None);
        // A removal of an unrelated endpoint does nothing.
        let looks = s.looks;
        s.send(Event::Removed("other".into()));
        assert_eq!(s.looks, looks);
        // Replug.
        s.default = Some(JBL);
        s.send(Event::Added(JBL.into()));
        assert_eq!(s.log, [on(JBL), off(JBL), on(JBL)]);
    }

    #[test]
    fn a_removal_ends_the_series_waiting_for_that_endpoint() {
        let mut s = Sim::new();
        s.send(Event::Added(JBL.into()));
        s.send(Event::Removed(JBL.into()));
        assert!(!s.follow.active());
    }

    #[test]
    fn a_resume_looks_again_forced_and_is_bounded() {
        let mut s = Sim::new();
        s.default = Some(JBL);
        s.opened = Some(JBL);
        s.held = Some(JBL);
        s.send(Event::Resumed);
        // Forced switch: the rebuilt objects, one off and one on line.
        assert_eq!(s.log, [off(JBL), on(JBL)]);
        s.drain();
        assert_eq!(s.log.len(), 2);
        // No device at all after a resume: bounded.
        let mut s = Sim::new();
        s.default = None;
        s.send(Event::Resumed);
        s.drain();
        assert_eq!(s.looks, 1 + BACKOFF.len());
        assert!(s.log.is_empty());
    }
}
