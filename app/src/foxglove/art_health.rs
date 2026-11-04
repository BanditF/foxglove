//! Health of the art playlist (#522) and of art across firmware updates (#521), as plain data
//! for the shared "Keyboard health" view (`health.read`, the app's health page,
//! `foxglove-lab health`; #705): how long the playlist is, which places it has and what they
//! hold, how its last swap went; and after an update, "art restored N/M". The art service notes
//! each event as it happens (a step, a stage that landed or failed, a stage skipped and why, a
//! restore's end); [`rows`] only reads. No polling.
//!
//! The rows are the Keyboard health view's shared type (`foxglove_link::health::HealthRow`, #705);
//! [`rows`] is the `health::Provider` registered with `health::register` as "art_playlist".

// Some of the notes are read only by the tests so far.
#![allow(dead_code)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The subsystem these rows belong to in the health view.
pub const SUBSYSTEM: &str = "screens.art_playlist";
/// The restore's rows (#521).
pub const RESTORE: &str = "screens.art_restore";

/// The last time a connect found kept places lost (#521) and what Foxglove put back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreHealth {
    /// Pieces lost (M); 0 when no connect lost any since Foxglove started.
    pub lost: usize,
    /// Put back (N of them), by name.
    pub restored: Vec<String>,
    /// Not put back, with the reason.
    pub failed: Vec<(String, String)>,
}

static RESTORED: Mutex<Option<RestoreHealth>> = Mutex::new(None);

/// Notes a restore's end (the art service calls this).
pub fn note_restore(r: RestoreHealth) {
    *RESTORED.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

pub fn restore_now() -> RestoreHealth {
    RESTORED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// The restore's row: "art restored N/M"; Warn when some weren't, Fail when none were.
pub fn restore_rows(r: &RestoreHealth) -> Vec<HealthRow> {
    let (level, detail) = if r.lost == 0 {
        (
            Level::Ok,
            "no kept pieces lost since Foxglove started".to_string(),
        )
    } else {
        let n = r.restored.len();
        let mut d = format!("art restored {n}/{}", r.lost);
        if !r.restored.is_empty() {
            d.push_str(&format!(": {}", r.restored.join(", ")));
        }
        if !r.failed.is_empty() {
            let why: Vec<String> = r.failed.iter().map(|(n, e)| format!("{n} ({e})")).collect();
            d.push_str(&format!("; not restored: {}", why.join(", ")));
        }
        let level = match n {
            0 => Level::Fail,
            n if n < r.lost => Level::Warn,
            _ => Level::Ok,
        };
        (level, d)
    };
    vec![HealthRow {
        subsystem: RESTORE.into(),
        item: "Art after a firmware update · Restored".into(),
        level,
        detail,
    }]
}

// The shared row type of the Keyboard health view (#705).
pub use super::health::{HealthRow, Level};

/// One swap: a piece put into a lent place ahead of its step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Swap {
    pub name: String,
    pub slot: u8,
    /// How long it took, or why it failed.
    pub result: Result<Duration, String>,
    pub at: Instant,
}

/// The playlist's health now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlaylistHealth {
    /// Rotation plays the playlist.
    pub on: bool,
    /// Entries, and how many of them are files from this PC (the ones that need a place).
    pub length: usize,
    pub files: usize,
    /// Lent places, and what each holds now (the keyboard's report): `None` = free.
    pub places: Vec<(u8, Option<String>)>,
    /// The keyboard can keep a piece without showing it (HELLO bit 30 and a right that takes
    /// STORE); `None` with no keyboard.
    pub can_store: Option<bool>,
    pub last_swap: Option<Swap>,
    pub swaps_ok: u32,
    pub swaps_failed: u32,
    /// Steps shown from a place or the keyboard's own art (instant), and steps that sent their
    /// piece from this PC (about 1 s, not kept).
    pub steps_instant: u32,
    pub steps_sent: u32,
    /// Why the last piece wasn't staged ahead (wear, no places, firmware without STORE).
    pub last_skip: Option<String>,
}

impl PlaylistHealth {
    /// The playlist's settings and the places as the keyboard reports them.
    pub fn configured(
        &mut self,
        on: bool,
        length: usize,
        files: usize,
        places: Vec<(u8, Option<String>)>,
    ) {
        self.on = on;
        self.length = length;
        self.files = files;
        self.places = places;
    }

    pub fn connected(&mut self, can_store: Option<bool>) {
        self.can_store = can_store;
    }

    /// A step: from a place or the keyboard's own art (`instant`), or sent from this PC.
    pub fn stepped(&mut self, instant: bool) {
        if instant {
            self.steps_instant = self.steps_instant.saturating_add(1);
        } else {
            self.steps_sent = self.steps_sent.saturating_add(1);
        }
    }

    /// A stage ended: landed in its place, or failed with the reason.
    pub fn swapped(&mut self, swap: Swap) {
        match swap.result {
            Ok(_) => self.swaps_ok = self.swaps_ok.saturating_add(1),
            Err(_) => self.swaps_failed = self.swaps_failed.saturating_add(1),
        }
        if swap.result.is_ok() {
            self.last_skip = None;
        }
        self.last_swap = Some(swap);
    }

    /// The next piece won't be staged ahead, and why.
    pub fn skipped(&mut self, why: String) {
        self.last_skip = Some(why);
    }
}

static HEALTH: Mutex<Option<PlaylistHealth>> = Mutex::new(None);

/// Notes an event (the art service calls this as each happens).
pub fn note(f: impl FnOnce(&mut PlaylistHealth)) {
    f(HEALTH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default));
}

/// The playlist's health now.
pub fn now() -> PlaylistHealth {
    HEALTH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// The rows for the health view now (a `health::Provider`): the playlist's, then the restore's.
pub fn rows() -> Vec<HealthRow> {
    let mut out = rows_of(&now(), Instant::now());
    out.extend(restore_rows(&restore_now()));
    out
}

fn ago(at: Instant, now: Instant) -> String {
    let s = now.saturating_duration_since(at).as_secs();
    match s {
        0..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

/// The health view's rows: Length, Places, Last swap, Steps.
pub fn rows_of(h: &PlaylistHealth, now: Instant) -> Vec<HealthRow> {
    let row = |what: &str, level, detail: String| HealthRow {
        subsystem: SUBSYSTEM.into(),
        item: format!("Art playlist · {what}"),
        level,
        detail,
    };
    if !h.on {
        return vec![row(
            "Length",
            Level::Ok,
            "off: Rotation steps the keyboard's own art".into(),
        )];
    }
    let length = if h.length == 0 {
        row(
            "Length",
            Level::Warn,
            "on, but empty: nothing to step through".into(),
        )
    } else {
        row(
            "Length",
            Level::Ok,
            format!("{} pieces, {} of them from this PC", h.length, h.files),
        )
    };
    let held = h
        .places
        .iter()
        .map(|(n, name)| match name {
            Some(name) => format!("place {} holds {name}", n + 1),
            None => format!("place {} free", n + 1),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let places = match (h.places.len(), h.can_store) {
        (_, Some(false)) if h.files > 0 => row(
            "Places",
            Level::Warn,
            "the keyboard's firmware can't keep a piece without showing it: each step sends its piece (update the keyboard)".into(),
        ),
        (0, _) if h.files > 0 => row(
            "Places",
            Level::Warn,
            "no places lent: each step sends its piece from this PC".into(),
        ),
        (0, _) => row("Places", Level::Ok, "none needed: no pieces from this PC".into()),
        (n, _) => row(
            "Places",
            Level::Ok,
            format!("{n} lent: {held}"),
        ),
    };
    let last = match &h.last_swap {
        None => match &h.last_skip {
            Some(why) if h.files > 0 => row("Last swap", Level::Warn, format!("none yet: {why}")),
            _ => row("Last swap", Level::Ok, "none yet".into()),
        },
        Some(s) => match &s.result {
            Ok(took) => {
                let detail = format!(
                    "OK: {} into place {} in {:.1} s, {}",
                    s.name,
                    s.slot + 1,
                    took.as_secs_f32(),
                    ago(s.at, now)
                );
                match &h.last_skip {
                    Some(why) => row(
                        "Last swap",
                        Level::Warn,
                        format!("{detail}; the next isn't staged: {why}"),
                    ),
                    None => row("Last swap", Level::Ok, detail),
                }
            }
            Err(e) => row(
                "Last swap",
                Level::Fail,
                format!(
                    "failed: {} into place {}, {}: {e}",
                    s.name,
                    s.slot + 1,
                    ago(s.at, now)
                ),
            ),
        },
    };
    let steps = row(
        "Steps",
        if h.steps_sent > 0 && h.files > 0 && h.can_store == Some(true) && !h.places.is_empty() {
            Level::Warn
        } else {
            Level::Ok
        },
        format!(
            "{} instant, {} sent from this PC; swaps {} OK, {} failed",
            h.steps_instant, h.steps_sent, h.swaps_ok, h.swaps_failed
        ),
    );
    vec![length, places, last, steps]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(h: &PlaylistHealth, now: Instant) -> Vec<Level> {
        rows_of(h, now).into_iter().map(|r| r.level).collect()
    }

    /// "art restored N/M" at each level: nothing lost, all, some, none.
    #[test]
    fn the_restore_row_says_n_of_m() {
        let row = |r: &RestoreHealth| restore_rows(r).remove(0);
        let none = row(&RestoreHealth::default());
        assert_eq!((none.level, none.subsystem.as_str()), (Level::Ok, RESTORE));
        let all = RestoreHealth {
            lost: 2,
            restored: vec!["fury".into(), "radar".into()],
            failed: vec![],
        };
        assert_eq!(row(&all).detail, "art restored 2/2: fury, radar");
        assert_eq!(row(&all).level, Level::Ok);
        let some = RestoreHealth {
            lost: 3,
            failed: vec![("gem".into(), "this PC has no copy of it".into())],
            ..all.clone()
        };
        assert_eq!(row(&some).level, Level::Warn);
        assert_eq!(
            row(&some).detail,
            "art restored 2/3: fury, radar; not restored: gem (this PC has no copy of it)"
        );
        let zero = RestoreHealth {
            lost: 1,
            restored: vec![],
            failed: vec![("gem".into(), "Bluetooth write failed".into())],
        };
        assert_eq!(row(&zero).level, Level::Fail);
        note_restore(some.clone());
        assert!(rows()
            .iter()
            .any(|r| r.detail.starts_with("art restored 2/3")));
    }

    /// The signal fires: each event the art service notes shows in the rows.
    #[test]
    fn each_playlist_event_shows_in_the_rows() {
        let t0 = Instant::now();
        let mut h = PlaylistHealth::default();
        let r = rows_of(&h, t0);
        assert_eq!(r.len(), 1);
        assert!(r[0].detail.starts_with("off"), "{}", r[0].detail);
        assert_eq!(r[0].subsystem, SUBSYSTEM);

        h.configured(true, 0, 0, vec![]);
        assert_eq!(levels(&h, t0)[0], Level::Warn, "on but empty");

        h.configured(true, 5, 3, vec![(6, Some("radar".into())), (7, None)]);
        h.connected(Some(true));
        let r = rows_of(&h, t0);
        assert_eq!(r[0].detail, "5 pieces, 3 of them from this PC");
        assert_eq!(r[1].detail, "2 lent: place 7 holds radar, place 8 free");
        assert_eq!(r[2].detail, "none yet");

        h.stepped(true);
        h.swapped(Swap {
            name: "orbit".into(),
            slot: 7,
            result: Ok(Duration::from_millis(1600)),
            at: t0,
        });
        let r = rows_of(&h, t0 + Duration::from_secs(90));
        assert_eq!(r[2].level, Level::Ok);
        assert_eq!(r[2].detail, "OK: orbit into place 8 in 1.6 s, 1 min ago");
        assert_eq!(
            r[3].detail,
            "1 instant, 0 sent from this PC; swaps 1 OK, 0 failed"
        );

        h.swapped(Swap {
            name: "uplink".into(),
            slot: 6,
            result: Err("Bluetooth write failed".into()),
            at: t0,
        });
        let r = rows_of(&h, t0);
        assert_eq!(r[2].level, Level::Fail);
        assert!(
            r[2].detail.contains("uplink into place 7")
                && r[2].detail.contains("Bluetooth write failed")
        );

        h.swapped(Swap {
            name: "uplink".into(),
            slot: 6,
            result: Ok(Duration::from_secs(2)),
            at: t0,
        });
        h.skipped("each lent place was written in the last 60 min (flash wear)".into());
        h.stepped(false);
        let r = rows_of(&h, t0);
        assert_eq!(r[2].level, Level::Warn);
        assert!(r[2].detail.contains("flash wear"));
        assert_eq!(
            r[3].level,
            Level::Warn,
            "a step sent although places are lent"
        );

        h.connected(Some(false));
        assert_eq!(levels(&h, t0)[1], Level::Warn, "firmware without STORE");
        h.configured(true, 2, 0, vec![]);
        assert_eq!(levels(&h, t0)[1], Level::Ok, "built-ins only need no place");
    }
}
