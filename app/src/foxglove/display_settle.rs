//! Settles a flapping display before anything reacts to it.
//!
//! A monitor whose link drops and returns every few seconds (a sleeping 4K, a bad cable; 2,444
//! `displays changed` lines on the user's PC from 2026-11-15) sent one `WM_DISPLAYCHANGE` per
//! drop and per return, and each one moved the overlay, re-applied every device's state (a fresh
//! black frame on the keyboard lights) and reopened Screen glow's capture.
//!
//! Now each handler hears the changes through a [`Debounce`]: it waits [`SETTLE`] after the
//! LAST change, then looks at the final displays once. It acts only when those differ from what
//! it last acted on, so a display that left and came back inside the window changes nothing.
//! [`Settle`] is the pure part (the clock is passed in, so tests need no sleeping); [`Debounce`]
//! is its thread. The thread sleeps on a condvar until a change arrives, then until the settle
//! deadline: events first, the timeout is the settle itself, no polling.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long the displays must stay as they are before a change is acted on.
pub const SETTLE: Duration = Duration::from_millis(2500);

/// The settle rule over a topology `T` (whatever the handler cares about: the monitors).
#[derive(Debug)]
pub struct Settle<T> {
    /// What was last acted on.
    acted: Option<T>,
    /// The latest topology heard and when it settles.
    pending: Option<(T, Instant)>,
}

impl<T> Default for Settle<T> {
    fn default() -> Self {
        Settle {
            acted: None,
            pending: None,
        }
    }
}

impl<T: PartialEq + Clone> Settle<T> {
    /// Takes `topology` as already acted on (the displays at start).
    pub fn seed(&mut self, topology: T) {
        self.acted = Some(topology);
    }

    /// A display change at `now` that left the displays as `topology`: the settle starts over.
    pub fn event(&mut self, now: Instant, topology: T) {
        self.pending = Some((topology, now + SETTLE));
    }

    /// When the pending change settles, if one waits.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|(_, d)| *d)
    }

    /// The settled topology to act on at `now`: only once the settle passed, and only when it
    /// differs from the last acted on.
    pub fn due(&mut self, now: Instant) -> Option<T> {
        if self.pending.as_ref().is_none_or(|(_, d)| now < *d) {
            return None;
        }
        let (topology, _) = self.pending.take()?;
        if self.acted.as_ref() == Some(&topology) {
            return None;
        }
        self.acted = Some(topology.clone());
        Some(topology)
    }
}

struct Shared<T> {
    state: Mutex<Settle<T>>,
    wake: Condvar,
}

/// A [`Settle`] with its own thread: `act` runs there, once per settled change.
pub struct Debounce<T> {
    shared: Arc<Shared<T>>,
}

impl<T: PartialEq + Clone + Send + 'static> Debounce<T> {
    pub fn new(name: &str, act: impl Fn(T) + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(Settle::default()),
            wake: Condvar::new(),
        });
        let s = shared.clone();
        super::services::spawn_named(name, move || {
            let mut g = s.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let now = Instant::now();
                if let Some(t) = g.due(now) {
                    drop(g);
                    // A panic in the handler must not end the debounce.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| act(t)));
                    g = s.state.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                g = match g.deadline() {
                    Some(d) => {
                        s.wake
                            .wait_timeout(g, d.saturating_duration_since(now))
                            .unwrap_or_else(|e| e.into_inner())
                            .0
                    }
                    None => s.wake.wait(g).unwrap_or_else(|e| e.into_inner()),
                };
            }
        });
        Debounce { shared }
    }

    /// The displays at start count as acted on.
    pub fn seed(&self, topology: T) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .seed(topology);
    }

    /// A display change, with the displays as they are now.
    pub fn event(&self, topology: T) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .event(Instant::now(), topology);
        self.shared.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Topo = Vec<&'static str>;
    const STEP: Duration = Duration::from_millis(500);

    /// Replays `events` (offset, topology) on a simulated clock for `total`, asking `due` every
    /// half second of simulated time; returns the topologies acted on, with when.
    fn replay(
        s: &mut Settle<Topo>,
        events: &[(Duration, Topo)],
        total: Duration,
    ) -> Vec<(Duration, Topo)> {
        let t0 = Instant::now();
        let mut acted = vec![];
        let mut at = Duration::ZERO;
        while at <= total {
            for (when, topo) in events {
                if *when > at.saturating_sub(STEP) && *when <= at {
                    s.event(t0 + *when, topo.clone());
                }
            }
            if let Some(t) = s.due(t0 + at) {
                acted.push((at, t));
            }
            at += STEP;
        }
        acted
    }

    /// `count` flaps, one every `every`: the Samsung leaves for `gone`, then returns.
    fn flaps(count: u64, every: Duration, gone: Duration) -> Vec<(Duration, Topo)> {
        let mut v = vec![];
        for i in 0..count {
            let t = every * i as u32 + Duration::from_secs(1);
            v.push((t, vec!["main"]));
            v.push((t + gone, vec!["main", "samsung"]));
        }
        v
    }

    /// 20 flaps in 3 minutes (a 4K leaving 1.5 s every 9 s): nothing to repaint, re-apply or move.
    #[test]
    fn twenty_flaps_in_three_minutes_act_on_nothing_when_seeded() {
        let mut s = Settle::default();
        s.seed(vec!["main", "samsung"]);
        let ev = flaps(20, Duration::from_secs(9), Duration::from_millis(1500));
        let acted = replay(&mut s, &ev, Duration::from_secs(185));
        assert!(acted.is_empty(), "{acted:?}");
    }

    /// Without a seed the first settled change acts once; the rest of the flaps don't.
    #[test]
    fn twenty_flaps_act_at_most_once_unseeded() {
        let mut s = Settle::default();
        let ev = flaps(20, Duration::from_secs(9), Duration::from_millis(1500));
        let acted = replay(&mut s, &ev, Duration::from_secs(185));
        assert!(acted.len() <= 1, "{acted:?}");
    }

    /// The settle runs from the LAST change: a flap every 2 s never settles while it goes on.
    #[test]
    fn nothing_acts_while_changes_keep_coming() {
        let mut s = Settle::default();
        s.seed(vec!["main", "samsung"]);
        let ev: Vec<_> = (0..30u64)
            .map(|i| {
                let topo = if i % 2 == 0 {
                    vec!["main"]
                } else {
                    vec!["main", "samsung"]
                };
                (Duration::from_secs(1 + i * 2), topo)
            })
            .collect();
        // The last event (59 s) leaves the seeded topology, so nothing acts at all.
        let acted = replay(&mut s, &ev, Duration::from_secs(70));
        assert!(acted.is_empty(), "{acted:?}");
    }

    /// A real change after the flapping acts once, on the final topology, SETTLE after the last
    /// event; a second real change acts once more.
    #[test]
    fn a_real_change_after_settling_acts_once() {
        let mut s = Settle::default();
        s.seed(vec!["main", "samsung"]);
        let mut ev = flaps(20, Duration::from_secs(9), Duration::from_millis(1500));
        ev.push((Duration::from_secs(200), vec!["main"]));
        let acted = replay(&mut s, &ev, Duration::from_secs(215));
        assert_eq!(acted.len(), 1, "{acted:?}");
        assert_eq!(acted[0].1, vec!["main"]);
        assert_eq!(acted[0].0, Duration::from_millis(202_500));

        let mut s = Settle::default();
        s.seed(vec!["main", "samsung"]);
        ev.push((Duration::from_secs(230), vec!["main", "samsung"]));
        let acted = replay(&mut s, &ev, Duration::from_secs(240));
        assert_eq!(acted.len(), 2, "one per real change: {acted:?}");
    }
}
