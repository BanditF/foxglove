//! Health of the keymap (F6: #582, #583), as plain data for the shared "Keyboard health" view
//! (`health.read`, the app's health page, `foxglove-lab health`; #705): whether the keymap was
//! read from the keyboard, whether the keyboard holds unsaved changes, how the last save went
//! and what happened to the last change. The keymap reader and the editor note each event as it
//! happens (a read, a write, a save, the board's unsaved notification); [`rows`] only reads. No
//! polling.
//!
//! The row shape matches `art_keys_health::HealthRow` and `lamp_health::HealthRow` (#704): the
//! health view keeps one shared type when it lands (#709/#710).

// The shared health view (#705) picks these up; until it lands only the tests read them.
#![allow(dead_code)]

#[cfg(not(test))]
use std::sync::Mutex;

/// The subsystem these rows belong to in the health view.
pub const SUBSYSTEM: &str = "keymap";

/// How the keymap was last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    /// Read from the keyboard: this many layers and keys.
    Board { layers: usize, keys: usize },
    /// The keyboard couldn't be asked, so the overlay shows the copy saved at the last read.
    Cached(String),
    /// The keyboard couldn't be asked and there's no saved copy.
    Failed(String),
}

/// What happened to the last change the editor made (a key, or a layer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The keyboard took it.
    Written,
    /// Foxglove's checks refused it, so it was never sent.
    Refused(String),
    /// The keyboard refused it and kept what it had.
    Rejected(String),
    /// The link failed during the write; the keymap was read back to show what the keyboard has.
    LinkFailed(String),
}

/// The keymap's health now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeymapHealth {
    /// `None` until the first read since Foxglove started.
    pub read: Option<Read>,
    /// The keyboard reports unsaved changes (Studio boards).
    pub unsaved: bool,
    /// `None` before the first save; `Err` holds the reason it failed.
    pub last_save: Option<Result<(), String>>,
    pub last_change: Option<Change>,
}

impl KeymapHealth {
    pub fn read(&mut self, read: Read) {
        self.read = Some(read);
    }

    pub fn unsaved(&mut self, unsaved: bool) {
        self.unsaved = unsaved;
    }

    pub fn saved(&mut self, result: Result<(), String>) {
        if result.is_ok() {
            self.unsaved = false;
        }
        self.last_save = Some(result);
    }

    pub fn changed(&mut self, change: Change) {
        if change == Change::Written {
            self.unsaved = true;
        }
        self.last_change = Some(change);
    }
}

/// A row's state in the health view (as `art_keys_health::Level`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// One view-model row for the health view (as `art_keys_health::HealthRow`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRow {
    pub subsystem: &'static str,
    pub item: String,
    pub level: Level,
    pub detail: String,
}

#[cfg(not(test))]
static HEALTH: Mutex<KeymapHealth> = Mutex::new(KeymapHealth {
    read: None,
    unsaved: false,
    last_save: None,
    last_change: None,
});

// Tests run benches side by side; each test thread keeps its own health, so a test reads only
// what its own editor noted.
#[cfg(test)]
thread_local! {
    static HEALTH: std::cell::RefCell<KeymapHealth> = std::cell::RefCell::default();
}

/// Notes an event (the reader and the editor call this as each happens).
pub fn note(f: impl FnOnce(&mut KeymapHealth)) {
    #[cfg(not(test))]
    f(&mut HEALTH.lock().unwrap_or_else(|e| e.into_inner()));
    #[cfg(test)]
    HEALTH.with(|h| f(&mut h.borrow_mut()));
}

/// The keymap's health now.
pub fn now() -> KeymapHealth {
    #[cfg(not(test))]
    return HEALTH.lock().unwrap_or_else(|e| e.into_inner()).clone();
    #[cfg(test)]
    HEALTH.with(|h| h.borrow().clone())
}

/// The health view's rows: Read, Unsaved changes, Last save, Last change.
pub fn rows(h: &KeymapHealth) -> Vec<HealthRow> {
    let row = |what: &str, level, detail: String| HealthRow {
        subsystem: SUBSYSTEM,
        item: format!("Keymap · {what}"),
        level,
        detail,
    };
    let read = match &h.read {
        None => row("Read", Level::Ok, "not read since Foxglove started".into()),
        Some(Read::Board { layers, keys }) => row(
            "Read",
            Level::Ok,
            format!("{layers} layers, {keys} keys read from the keyboard"),
        ),
        Some(Read::Cached(why)) => row(
            "Read",
            Level::Warn,
            format!("showing the copy saved at the last read: {why}"),
        ),
        Some(Read::Failed(why)) => row(
            "Read",
            Level::Fail,
            format!("couldn't read the keymap: {why}"),
        ),
    };
    let unsaved = if h.unsaved {
        row(
            "Unsaved changes",
            Level::Warn,
            "yes: on the keyboard until it restarts".into(),
        )
    } else {
        row("Unsaved changes", Level::Ok, "none".into())
    };
    let save = match &h.last_save {
        None => row("Last save", Level::Ok, "none since Foxglove started".into()),
        Some(Ok(())) => row("Last save", Level::Ok, "saved".into()),
        Some(Err(why)) => row("Last save", Level::Fail, format!("not saved: {why}")),
    };
    let change = match &h.last_change {
        None => row(
            "Last change",
            Level::Ok,
            "none since Foxglove started".into(),
        ),
        Some(Change::Written) => row("Last change", Level::Ok, "written to the keyboard".into()),
        Some(Change::Refused(why)) => row(
            "Last change",
            Level::Warn,
            format!("refused before writing: {why}"),
        ),
        Some(Change::Rejected(why)) => row(
            "Last change",
            Level::Fail,
            format!("the keyboard refused it and kept what it had: {why}"),
        ),
        Some(Change::LinkFailed(why)) => row(
            "Last change",
            Level::Fail,
            format!("the link failed during the write ({why}); the keymap was read back"),
        ),
    };
    vec![read, unsaved, save, change]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(h: &KeymapHealth, i: usize) -> Level {
        rows(h)[i].level
    }

    /// The signal fires: each event the reader and the editor note shows in the rows.
    #[test]
    fn each_event_shows_in_the_rows() {
        let mut h = KeymapHealth::default();
        assert!(rows(&h)
            .iter()
            .all(|r| r.level == Level::Ok && r.subsystem == SUBSYSTEM));
        assert_eq!(rows(&h).len(), 4);

        h.read(Read::Board {
            layers: 4,
            keys: 42,
        });
        assert_eq!(
            rows(&h)[0].detail,
            "4 layers, 42 keys read from the keyboard"
        );
        h.read(Read::Cached("the keyboard is locked".into()));
        assert_eq!(level(&h, 0), Level::Warn);
        assert!(rows(&h)[0].detail.contains("locked"));
        h.read(Read::Failed("no reply over USB".into()));
        assert_eq!(level(&h, 0), Level::Fail);

        h.changed(Change::Written);
        assert!(h.unsaved, "a write leaves the keyboard unsaved");
        assert_eq!(level(&h, 1), Level::Warn);
        assert_eq!(rows(&h)[3].detail, "written to the keyboard");

        h.saved(Err("the keyboard has no room left to save".into()));
        assert_eq!(level(&h, 2), Level::Fail);
        assert!(rows(&h)[2].detail.contains("no room left"));
        assert!(h.unsaved, "a failed save keeps the changes unsaved");
        h.saved(Ok(()));
        assert_eq!(level(&h, 2), Level::Ok);
        assert_eq!(level(&h, 1), Level::Ok, "saved, nothing unsaved");

        h.changed(Change::Refused("layer 9 doesn't exist".into()));
        assert_eq!(level(&h, 3), Level::Warn);
        h.changed(Change::Rejected("invalid parameters".into()));
        assert_eq!(level(&h, 3), Level::Fail);
        h.changed(Change::LinkFailed("no reply".into()));
        assert!(rows(&h)[3].detail.contains("read back"));

        h.unsaved(true);
        assert_eq!(level(&h, 1), Level::Warn);
        h.unsaved(false);
        assert_eq!(level(&h, 1), Level::Ok);
    }

    /// The global note reaches `now()`.
    #[test]
    fn notes_reach_the_shared_state() {
        note(|h| {
            h.read(Read::Board {
                layers: 3,
                keys: 61,
            })
        });
        assert!(matches!(now().read, Some(Read::Board { .. })));
    }
}
