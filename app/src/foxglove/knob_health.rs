//! Health of the knob (#719), as plain data for the shared "Keyboard health" view (#705):
//! whether Foxglove holds the knob (and master volume, #855), what it points at now, and how the
//! last level change went.
//! The knob service (`knob.rs`) notes each event as it happens; [`rows`] only reads. No polling.
//! Registered with `health::register` as "knob"; the keyboard's own side is its self-report's
//! knob row (block version 2).

use std::sync::Mutex;

pub use super::health::{HealthRow, Level};

/// The subsystem these rows belong to in the health view.
pub const SUBSYSTEM: &str = "knob";

/// The last level Foxglove set for an app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastLevel {
    /// `app` is at `percent` now.
    Set { app: String, percent: u8 },
    /// `app` was muted (or unmuted) by a double click.
    Muted { app: String, muted: bool },
    /// Setting `app`'s level (or its mute) failed for `why`.
    Failed { app: String, why: String },
}

/// The knob's health now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnobHealth {
    /// The connected keyboard has the knob (HELLO bit 31); `None` with no keyboard.
    pub supported: Option<bool>,
    /// Foxglove holds the knob (D7 14 01) and the keyboard hasn't dropped it.
    pub held: bool,
    /// The keyboard's firmware lets Foxglove hold master volume (word 2's bit 35, #855); `None`
    /// with no keyboard (or one without the knob).
    pub master_volume_supported: Option<bool>,
    /// Foxglove holds master volume (D7 16 01): master's turns show its card, not Windows'.
    pub master_volume: bool,
    /// The app the knob points at; `None`: master (Windows' own volume keys).
    pub target: Option<String>,
    pub last_level: Option<LastLevel>,
    /// Knob events that reached Foxglove, and the ones the sequence numbers say were lost.
    pub events: u32,
    pub lost: u32,
    /// Why the knob last went back to master on its own (the app's sound ended, ...).
    pub last_back: Option<String>,
    /// Popups shown (`knob_popup.rs`), and the last one's words.
    pub popups: u32,
    pub last_popup: Option<String>,
}

static HEALTH: Mutex<KnobHealth> = Mutex::new(KnobHealth {
    supported: None,
    held: false,
    master_volume_supported: None,
    master_volume: false,
    target: None,
    last_level: None,
    events: 0,
    lost: 0,
    last_back: None,
    popups: 0,
    last_popup: None,
});

/// Changes the knob's health (the knob service, as events happen).
pub fn note(f: impl FnOnce(&mut KnobHealth)) {
    f(&mut HEALTH.lock().unwrap_or_else(|e| e.into_inner()));
}

/// The knob's health now.
pub fn now() -> KnobHealth {
    HEALTH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The health view's rows now (registered with `health::register`).
pub fn rows_now() -> Vec<HealthRow> {
    let mut out = rows(&now());
    out.push(master_row(
        foxglove_panel::master::device().as_ref(),
        foxglove_panel::master::handled_master(),
        foxglove_panel::master::restored(),
    ));
    if let Some(row) = apps_row(&foxglove_panel::master::last_report()) {
        out.push(row);
    }
    out
}

/// "Knob · Master volume apps": the last engage or disengage report's summary (every session's
/// own, applied and Windows level are in the log); a warning when a write failed.
pub fn apps_row(report: &[String]) -> Option<HealthRow> {
    let summary = report.first()?;
    let words = summary
        .strip_prefix("Foxglove: master volume: ")
        .unwrap_or(summary);
    let failed = report.iter().any(|l| l.contains("WRITE FAILED"));
    let level = if failed { Level::Warn } else { Level::Ok };
    Some(HealthRow::new(
        SUBSYSTEM,
        "Knob · Master volume apps",
        level,
        format!("{words} (per-app levels: the log)"),
    ))
}

/// "Knob · Master volume" (#1064): whether Foxglove sets the default device's volume, and how
/// many apps it put back from a crashed run's journal.
pub fn master_row(
    device: Option<&foxglove_panel::master::Device>,
    master: Option<(u8, bool)>,
    restored: u32,
) -> HealthRow {
    let mut words = match (device, master) {
        (None, _) => "no output device read yet".to_string(),
        (Some(d), Some((level, muted))) => format!(
            "{}: Foxglove sets its volume · master {level} %{}, held one step below the top",
            d.name,
            if muted { " (muted)" } else { "" }
        ),
        (Some(d), None) if !d.eligible => {
            format!("{}: Windows sets its volume itself", d.name)
        }
        (Some(d), None) if d.on => format!("{}: on, but not started (see the log)", d.name),
        (Some(d), None) => format!("{}: Windows' own volume (the setting is off)", d.name),
    };
    match restored {
        0 => {}
        1 => words.push_str("; 1 app restored from a crashed run's journal"),
        n => words.push_str(&format!("; {n} apps restored from a crashed run's journal")),
    }
    let level = match (device, master) {
        (Some(d), None) if d.on && d.eligible => Level::Warn,
        _ => Level::Ok,
    };
    HealthRow::new(SUBSYSTEM, "Knob · Master volume", level, words)
}

/// The health view's rows: Held, Target, Last level.
pub fn rows(h: &KnobHealth) -> Vec<HealthRow> {
    let row = |what: &str, level, detail: String| {
        HealthRow::new(SUBSYSTEM, format!("Knob · {what}"), level, detail)
    };
    let held = match (h.supported, h.held) {
        (None, _) => row("Held", Level::Ok, "no keyboard connected".into()),
        (Some(false), _) => row(
            "Held",
            Level::Ok,
            "this keyboard's firmware has no knob (HELLO bit 31): the knob is the volume keys"
                .into(),
        ),
        (Some(true), true) => row(
            "Held",
            Level::Ok,
            match (h.master_volume_supported, h.master_volume) {
                (_, true) => "yes: Foxglove holds the knob and master volume (its own card on \
                              master, no Windows flyout)"
                    .into(),
                (Some(false), false) => "yes: Foxglove holds the knob; master stays the volume \
                                         keys (this firmware has no master volume hold, word \
                                         2's bit 35)"
                    .into(),
                _ => "yes: Foxglove holds the knob".into(),
            },
        ),
        (Some(true), false) => row(
            "Held",
            Level::Ok,
            "no: the knob is the volume keys (Foxglove paused or the keyboard dropped it)".into(),
        ),
    };
    let mut target_words = match &h.target {
        None if h.master_volume => "master volume (set by Foxglove, its own card)".to_string(),
        None => "master volume (Windows' own volume keys)".to_string(),
        Some(app) => format!("{app} (its own volume)"),
    };
    if let Some(why) = &h.last_back {
        target_words.push_str(&format!("; last back to master: {why}"));
    }
    let target = row("Target", Level::Ok, target_words);
    let level = match &h.last_level {
        None => row(
            "Last level",
            Level::Ok,
            "none set since Foxglove started".into(),
        ),
        Some(LastLevel::Set { app, percent }) => {
            row("Last level", Level::Ok, format!("{app} set to {percent} %"))
        }
        Some(LastLevel::Muted { app, muted }) => row(
            "Last level",
            Level::Ok,
            format!("{app} {}", if *muted { "muted" } else { "unmuted" }),
        ),
        Some(LastLevel::Failed { app, why }) => row(
            "Last level",
            Level::Fail,
            format!("couldn't set {app}'s level: {why}"),
        ),
    };
    let popup = row(
        "Popup",
        Level::Ok,
        match &h.last_popup {
            None => "none shown since Foxglove started".into(),
            Some(last) => format!("shown {} times; last: {last}", h.popups),
        },
    );
    let mut out = vec![held, target, level, popup];
    let resets = foxglove_panel::master::resets();
    if resets > 0 {
        out.push(row(
            "App levels",
            Level::Ok,
            format!(
                "reset {resets} time{} since Foxglove started",
                if resets == 1 { "" } else { "s" }
            ),
        ));
    }
    if h.lost > 0 {
        out.push(row(
            "Events",
            Level::Warn,
            format!("{} received, {} lost on the way", h.events, h.lost),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(h: &KnobHealth) -> Vec<(String, Level)> {
        rows(h).into_iter().map(|r| (r.item, r.level)).collect()
    }

    #[test]
    fn a_held_knob_on_an_app_reads_as_such() {
        let h = KnobHealth {
            supported: Some(true),
            held: true,
            target: Some("Spotify".into()),
            last_level: Some(LastLevel::Set {
                app: "Spotify".into(),
                percent: 42,
            }),
            events: 9,
            ..KnobHealth::default()
        };
        let r = rows(&h);
        assert_eq!(r[0].detail, "yes: Foxglove holds the knob");
        assert_eq!(r[1].detail, "Spotify (its own volume)");
        assert_eq!(r[2].detail, "Spotify set to 42 %");
        assert!(r
            .iter()
            .all(|r| r.level == Level::Ok && r.subsystem == SUBSYSTEM));
    }

    #[test]
    fn a_failed_level_fails_and_lost_events_warn() {
        let h = KnobHealth {
            supported: Some(true),
            held: true,
            target: None,
            last_level: Some(LastLevel::Failed {
                app: "Discord".into(),
                why: "the app's sound ended".into(),
            }),
            events: 5,
            lost: 2,
            last_back: Some("Discord's sound ended".into()),
            popups: 3,
            last_popup: Some("Discord stopped · knob: master".into()),
            ..KnobHealth::default()
        };
        assert_eq!(
            levels(&h),
            [
                ("Knob · Held".to_string(), Level::Ok),
                ("Knob · Target".to_string(), Level::Ok),
                ("Knob · Last level".to_string(), Level::Fail),
                ("Knob · Popup".to_string(), Level::Ok),
                ("Knob · Events".to_string(), Level::Warn),
            ]
        );
        let r = rows(&h);
        assert_eq!(
            r[1].detail,
            "master volume (Windows' own volume keys); last back to master: Discord's sound ended"
        );
        assert_eq!(
            r[2].detail,
            "couldn't set Discord's level: the app's sound ended"
        );
    }

    #[test]
    fn master_volume_held_or_not_reads_as_such() {
        // #855: Foxglove holds master volume; then firmware without it.
        let mut h = KnobHealth {
            supported: Some(true),
            held: true,
            master_volume_supported: Some(true),
            master_volume: true,
            last_level: Some(LastLevel::Set {
                app: "master volume".into(),
                percent: 66,
            }),
            ..KnobHealth::default()
        };
        let r = rows(&h);
        assert_eq!(
            r[0].detail,
            "yes: Foxglove holds the knob and master volume (its own card on master, no Windows \
             flyout)"
        );
        assert_eq!(r[1].detail, "master volume (set by Foxglove, its own card)");
        assert_eq!(r[2].detail, "master volume set to 66 %");
        h.master_volume_supported = Some(false);
        h.master_volume = false;
        let r = rows(&h);
        assert_eq!(
            r[0].detail,
            "yes: Foxglove holds the knob; master stays the volume keys (this firmware has no \
             master volume hold, word 2's bit 35)"
        );
        assert_eq!(r[1].detail, "master volume (Windows' own volume keys)");
        assert!(r.iter().all(|r| r.level == Level::Ok));
    }

    #[test]
    fn no_keyboard_or_old_firmware_is_not_a_failure() {
        assert!(rows(&KnobHealth::default())
            .iter()
            .all(|r| r.level == Level::Ok));
        let old = KnobHealth {
            supported: Some(false),
            ..KnobHealth::default()
        };
        assert!(rows(&old).iter().all(|r| r.level == Level::Ok));
    }

    #[test]
    fn the_master_volume_row_says_who_sets_the_device() {
        use foxglove_panel::master::Device;
        let d = Device {
            name: "Headphones (Xbox Controller)".into(),
            eligible: true,
            listed: true,
            on: true,
            ..Device::default()
        };
        let r = master_row(Some(&d), Some((35, false)), 3);
        assert!(r.detail.contains("master 35 %"), "{}", r.detail);
        assert!(r.detail.contains("3 apps restored"), "{}", r.detail);
        assert!(
            r.detail.starts_with(
                "Headphones (Xbox Controller): Foxglove sets its volume · master 35 %"
            ),
            "{}",
            r.detail
        );
        let one = master_row(Some(&d), Some((35, false)), 1);
        assert!(one
            .detail
            .ends_with("; 1 app restored from a crashed run's journal"));
        let r = master_row(Some(&d), None, 0);
        assert_eq!(r.level, Level::Warn, "on but not started");
        let r = master_row(
            Some(&Device {
                eligible: false,
                ..d.clone()
            }),
            None,
            0,
        );
        assert_eq!(r.level, Level::Ok);
        assert!(r.detail.contains("Windows sets its volume itself"));
    }
}
