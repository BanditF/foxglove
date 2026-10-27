//! Pause Foxglove (#404): while paused, Foxglove stops driving the keyboard. No lighting,
//! screen or art writes go out: the left panel goes back to the firmware, the right art stays
//! as it is, the Windows-accent colour waits, the app's own lights and art controls refuse, and
//! plugin lighting leases give the lights back so the keyboard shows its own effects. The layer
//! overlay hides and the tray icon shows the paused glyph. Reads (status, lights, battery) go on.
//!
//! Resuming brings it back: leases show again, the left panel is drawn again, and art that was
//! skipped while paused shows. Pause lives in memory only, so a restart starts unpaused.
//!
//! Pausing and resuming is the event: [`set`] wakes each service and the UI once; nothing here
//! repeats or waits.

use std::sync::atomic::{AtomicBool, Ordering};

static PAUSED: AtomicBool = AtomicBool::new(false);

/// What a write refused while paused tells the user (or the plugin).
pub const REFUSED: &str = "Foxglove is paused: resume it to change the keyboard";

pub fn paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// Pauses or resumes Foxglove; nothing happens when it already is so.
pub fn set(paused: bool) {
    if !change(&PAUSED, paused) {
        return;
    }
    eprintln!("Foxglove: {}", if paused { "paused" } else { "resumed" });
    // The left panel's service hands the panel back, or takes it again.
    super::screens::pause_changed();
    // Plugin leases give the lights back, or show again (on their own thread).
    super::api::pause_leases();
    // A running scene stops drawing, or draws again (#485).
    super::scene_run::pause_changed(paused);
    // The live wallpaper feed goes quiet, or live again (#499).
    super::live_feed::pause_changed();
    if !paused {
        super::accent::wake();
        if let Some(h) = super::art_service::handle() {
            h.resumed();
        }
    }
    // The tray icon, the tray panel and the layer overlay.
    super::tray_panel::wake();
}

/// Sets `flag`; true when that changed it.
fn change(flag: &AtomicBool, paused: bool) -> bool {
    flag.swap(paused, Ordering::SeqCst) != paused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pausing_twice_is_one_pause() {
        // A flag of its own: the app's would pause the other tests running alongside.
        let flag = AtomicBool::new(false);
        assert!(change(&flag, true));
        assert!(!change(&flag, true), "already paused: nothing to wake");
        assert!(change(&flag, false));
        assert!(!change(&flag, false));
        // A restart starts unpaused.
        assert!(!paused());
    }
}
