//! Typing art (#886, docs/art-cycling.md): the right screen shows a chosen piece while the user
//! types, from the "typing or not" bit alone.
//!
//! - **Privacy:** Foxglove counts activity only. The signal is the shared Raw Input listener
//!   (`follow_typing`: `RIDEV_INPUTSINK` on a message-only window, any keyboard), which reads
//!   each event's header alone (`RID_HEADER`), so the `RAWKEYBOARD` fields (make code, flags,
//!   virtual key, message) never reach Foxglove. What comes here is an [`Activity`]: the time a
//!   key moved, and nothing else. No key code, scan code or VK is stored, logged or sent; a test
//!   holds the type to that.
//! - **Events only:** each event goes to the art service at most once per [`NOTE_EVERY`] (a
//!   timestamp check, no timer); `art_triggers` does the hysteresis with one-shot deadlines.
//! - **Health:** how many times the typing art came on and went off (counts only), as rows of the
//!   shared health view ("typing_art"), and a log line per switch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use super::health::{HealthRow, Level};

/// The subsystem these rows belong to in the health view.
pub const SUBSYSTEM: &str = "screens.typing_art";

/// Activity goes to the art service at most this often: plenty for second-scale hysteresis.
pub const NOTE_EVERY: Duration = Duration::from_millis(250);

/// One moment of typing activity. PRIVACY (#886): it holds the time a key moved and nothing
/// else; never add the key, its scan code or virtual key, or the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Activity(Instant);

impl Activity {
    /// Activity at `at`.
    pub fn at_time(at: Instant) -> Self {
        Activity(at)
    }

    /// When the key moved.
    pub fn at(self) -> Instant {
        self.0
    }
}

/// A typing art is set, so the listener should run and activity should go to the art service.
static WANTED: AtomicBool = AtomicBool::new(false);
/// When activity last went to the art service.
static LAST_SENT: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether a typing art is set (the listener runs for it).
pub fn wanted() -> bool {
    WANTED.load(Ordering::Relaxed)
}

/// The art service read its hotlist: starts the shared listener when a typing art is set, and
/// stops it when nothing else wants it.
pub fn set_wanted(on: bool) {
    if WANTED.swap(on, Ordering::Relaxed) != on {
        super::follow_typing::refresh();
    }
}

/// Whether activity at `now` goes on, after the last sent at `last`.
fn passes(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|l| now.saturating_duration_since(l) >= NOTE_EVERY)
}

/// A key moved at `now` (from the listener thread). Goes to the art service, thinned to one per
/// [`NOTE_EVERY`], while a typing art is set.
pub fn activity(now: Instant) {
    if !wanted() {
        return;
    }
    {
        let mut last = LAST_SENT.lock().unwrap_or_else(|e| e.into_inner());
        if !passes(*last, now) {
            return;
        }
        *last = Some(now);
    }
    if let Some(h) = super::art_service::handle() {
        h.typed(Activity::at_time(now));
    }
}

/// The typing art's health: switches on and off, counts only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypingArtHealth {
    pub on: u32,
    pub off: u32,
}

static HEALTH: Mutex<TypingArtHealth> = Mutex::new(TypingArtHealth { on: 0, off: 0 });

/// The typing art came on (`true`) or went off (`art_triggers`).
pub fn note_switch(on: bool) {
    let h = {
        let mut h = HEALTH.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            h.on = h.on.saturating_add(1);
        } else {
            h.off = h.off.saturating_add(1);
        }
        *h
    };
    eprintln!(
        "Foxglove: typing art {} (on {} times, off {} times)",
        if on { "on" } else { "off" },
        h.on,
        h.off
    );
}

/// The typing art's health now.
pub fn now() -> TypingArtHealth {
    *HEALTH.lock().unwrap_or_else(|e| e.into_inner())
}

/// The health view's rows now (registered with `health::register`).
pub fn rows_now() -> Vec<HealthRow> {
    rows(&now(), wanted())
}

/// The health view's row: Switches.
pub fn rows(h: &TypingArtHealth, set: bool) -> Vec<HealthRow> {
    let detail = if set {
        format!("on {} times, off {} times", h.on, h.off)
    } else {
        "off (no typing art set)".into()
    };
    vec![HealthRow::new(
        SUBSYSTEM,
        "Typing art · Switches".to_string(),
        Level::Ok,
        detail,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_carries_a_time_and_no_key_data() {
        // PRIVACY (#886): an Activity is exactly one Instant. A key, scan code, virtual key or
        // device added to it would grow it and fail here.
        assert_eq!(size_of::<Activity>(), size_of::<Instant>());
        let t = Instant::now();
        assert_eq!(Activity::at_time(t).at(), t);
        // The listener hands the typing art only the time: no path, no key.
        let src = include_str!("follow_typing.rs");
        assert!(src.contains("super::typing_art::activity(now);"));
        assert!(
            !src.contains("RAWKEYBOARD"),
            "the key's fields are never read"
        );
    }

    #[test]
    fn activity_is_thinned_to_one_per_note_every() {
        let t0 = Instant::now();
        assert!(passes(None, t0));
        assert!(!passes(Some(t0), t0 + NOTE_EVERY / 2));
        assert!(passes(Some(t0), t0 + NOTE_EVERY));
    }

    #[test]
    fn health_counts_switches_only() {
        let before = now();
        note_switch(true);
        note_switch(false);
        let after = now();
        assert!(after.on > before.on && after.off > before.off);
        let rows = rows(&TypingArtHealth { on: 3, off: 2 }, true);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].detail.contains("on 3 times, off 2 times"));
    }
}
