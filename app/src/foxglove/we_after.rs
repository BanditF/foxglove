//! The wallpaper section's after-events (#1142, UX plan `review/ux-plan-we-pause-holder.md` 3-4):
//! the lines that say what just happened once nothing holds the wallpapers paused any more.
//!
//! - Resumed: the user's Resume ended every app's pause ("Resumed. Arete can pause them again.").
//! - Stopped: the last app holding the pause quit or was stopped ("Arete stopped, so its pause
//!   ended."). An app that released its pause itself says nothing, and a hub reconnect says
//!   nothing: the tracker is made new with each connection, so it cannot tell who was there.
//!
//! A line lasts until the next change of what the section shows, or [`SHOW`]; one shot, never a
//! refresh loop. [`Tracker`] is pure (the clock is given), so the fake-clock tests need no thread;
//! the link arms one thread per new line that wakes the panels once at its end.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::hub_panels::After;

/// How long an after-line shows at most.
pub const SHOW: Duration = Duration::from_secs(10);
/// A stop hint older than this belongs to some other app's stop, not this transition.
const HINT_LIVES: Duration = Duration::from_secs(5);

/// When an app's connection closed with its pause live (told by the worker and by the hub's
/// `lease.closed {reason: "disconnect"}`): the next "nothing holds it" is that app stopping.
static HINT: Mutex<Option<Instant>> = Mutex::new(None);

/// An app holding a pause stopped (its connection closed).
pub fn holder_stopped(now: Instant) {
    *HINT.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
}

/// Takes the stop hint when it is fresh at `now`; an old one is dropped.
pub fn take_stop(now: Instant) -> bool {
    let at = HINT.lock().unwrap_or_else(|e| e.into_inner()).take();
    at.is_some_and(|t| now.saturating_duration_since(t) <= HINT_LIVES)
}

/// Drops a stop hint (a holder stopped, but others still hold the pause: not the last).
pub fn clear_stop() {
    HINT.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// What the section showed when its line began; a change ends the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    held: bool,
    running: bool,
    user_paused: bool,
}

/// One connection's after-line.
#[derive(Debug, Default)]
pub struct Tracker {
    /// The names of the last held state, and whether it was held.
    prev: Vec<String>,
    was_held: bool,
    /// Resume was pressed at `.1` with these names: the line starts when the hold is gone.
    pending: Option<(Vec<String>, Instant)>,
    note: Option<(After, Instant, Seen)>,
    /// The end of a line that began since [`Tracker::take_deadline`].
    fresh: Option<Instant>,
}

impl Tracker {
    /// The user pressed Resume while `names` held the wallpapers.
    pub fn resume_pressed(&mut self, names: &[String], now: Instant) {
        self.pending = Some((names.to_vec(), now));
    }

    /// Fewer apps hold it than at the last update, and some still do: whoever stopped was not the
    /// last, so a stop hint belongs to no "stopped" line.
    pub fn shrank(&self, names: &[String], held: bool) -> bool {
        held && self.was_held && names.len() < self.prev.len()
    }

    /// The section's facts now; the line to show, if any.
    pub fn update(
        &mut self,
        names: &[String],
        held: bool,
        running: bool,
        user_paused: bool,
        stop_hint: bool,
        now: Instant,
    ) -> Option<After> {
        let seen = Seen {
            held,
            running,
            user_paused,
        };
        if let Some((_, at, was)) = &self.note {
            if now.saturating_duration_since(*at) >= SHOW || *was != seen {
                self.note = None;
            }
        }
        if let Some((_, at)) = &self.pending {
            if now.saturating_duration_since(*at) >= SHOW {
                self.pending = None;
            }
        }
        if held {
            self.note = None;
            self.prev = names.to_vec();
        } else {
            let started = if let Some((names, _)) = self.pending.take() {
                Some(After::Resumed(names))
            } else if self.was_held && stop_hint && running {
                Some(After::Stopped(self.prev.first().cloned()))
            } else {
                None
            };
            if let Some(a) = started {
                self.note = Some((a, now, seen));
                self.fresh = Some(now + SHOW);
            }
        }
        self.was_held = held;
        self.note.as_ref().map(|(a, _, _)| a.clone())
    }

    /// When the line that began since the last call ends (one wake-up to arm), once.
    pub fn take_deadline(&mut self) -> Option<Instant> {
        self.fresh.take()
    }

    /// Whether a line shows.
    #[cfg(test)]
    pub fn showing(&self) -> bool {
        self.note.is_some()
    }
}

#[cfg(test)]
mod tests;
