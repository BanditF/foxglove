//! Screen-reader support (#1376): AccessKit's Windows adapter answers UI Automation for each of
//! the app's windows. It names its subclass with a window property, and a named property needs a
//! global atom: when another program has filled Windows' system-wide atom table, creating that
//! property fails (upstream panicked, commit 23b354fa turned the adapter off; the vendored
//! accesskit_windows now just leaves that window without a tree).
//!
//! At startup [`start`] takes the atom once and holds it for the whole run, so a table that fills
//! later (the usual leak) cannot break a window opened later: with the atom held, every later
//! property of that name only counts a reference.

use std::sync::atomic::{AtomicU8, Ordering};

const UNKNOWN: u8 = 0;
const ON: u8 = 1;
const ATOM_FULL: u8 = 2;
const NOT_AVAILABLE: u8 = 3;

static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Holds the adapter's atom for the process. Call once, early, on Windows; elsewhere it notes that
/// there is no adapter.
pub fn start() {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::System::DataExchange::GlobalAddAtomW;
        // The name AccessKit's Windows adapter uses for its window property.
        let atom = unsafe { GlobalAddAtomW(w!("AccessKitAdapter")) };
        if atom == 0 {
            eprintln!(
                "Foxglove: screen reader support off: GlobalAddAtomW failed (atom table full?)"
            );
            STATE.store(ATOM_FULL, Ordering::Relaxed);
        } else {
            STATE.store(ON, Ordering::Relaxed);
        }
    }
    #[cfg(not(windows))]
    STATE.store(NOT_AVAILABLE, Ordering::Relaxed);
}

/// The "Screen readers" line of the diagnostics report.
pub fn diagnostics() -> String {
    describe(STATE.load(Ordering::Relaxed))
}

fn describe(state: u8) -> String {
    match state {
        ON => "on".into(),
        ATOM_FULL => "off this session. Windows' atom table is full, so screen readers can't read \
             the app. Signing out or restarting the PC fixes it."
            .into(),
        NOT_AVAILABLE => "not available on this system".into(),
        _ => "not started".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_words_each_state() {
        assert_eq!(describe(ON), "on");
        assert!(describe(ATOM_FULL).starts_with("off this session"));
        assert!(describe(ATOM_FULL).contains("restarting the PC"));
        assert_eq!(describe(NOT_AVAILABLE), "not available on this system");
        assert_eq!(describe(UNKNOWN), "not started");
    }

    #[cfg(windows)]
    #[test]
    fn holding_the_atom_works_twice() {
        // Adding the same name again only counts a reference; no failure.
        start();
        start();
        assert_eq!(STATE.load(Ordering::Relaxed), ON);
    }
}
