//! Master volume on a device that ignores Windows volume (#1064, UX plan
//! `docs/knob-exclude-ux.md`, "Master volume on a device that ignores Windows volume"). The
//! engine is `foxglove_panel::master` (run on the audio sessions' thread); this is Foxglove's
//! side of it:
//!
//! - the shipped data list (`resources/volume-quirks.toml`): devices on by default; data, not
//!   code, and no device names in code;
//! - [`StoreHost`]: the per-device choice and master's level in the settings (`[master_volume]`),
//!   and the write-ahead journal beside them (`master-volume-journal.json`);
//! - the Knob card's MASTER VOLUME part (Keyboard page), the master card's slider row and the
//!   tray row's tooltip words, exactly as the plan has them.
//!
//! Polling: none. The part reads the device's facts the sessions' thread published; its first
//! frame of a visit asks for them once more (the exclusive-control setting may have changed).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use egui::RichText;
use foxglove_panel::master::{self, AppVolume, Device, Host, Journal};
use serde::{Deserialize, Serialize};

use super::theme;

/// The settings section.
pub const SECTION: &str = "master_volume";

/// The per-device choices and levels (`[master_volume]`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub devices: Vec<DeviceChoice>,
}

/// One device: the user's choice (none: the default) and master's level as last remembered.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceChoice {
    /// The endpoint id.
    pub endpoint: String,
    /// Windows' name for it, for reading the file.
    pub name: String,
    pub on: Option<bool>,
    pub level: Option<f32>,
    pub muted: bool,
}

/// One entry of the shipped data list.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Quirk {
    pub ids: Vec<String>,
    pub name: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Deserialize)]
struct QuirkFile {
    #[serde(default)]
    device: Vec<Quirk>,
}

/// Parses a data list; a broken one lists nothing (and says so).
pub fn parse_quirks(text: &str) -> Vec<Quirk> {
    match toml::from_str::<QuirkFile>(text) {
        Ok(f) => f.device,
        Err(e) => {
            eprintln!("Foxglove: master volume: the shipped device list can't be read: {e}");
            Vec::new()
        }
    }
}

/// The shipped data list's id prefixes.
pub fn prefixes() -> &'static [String] {
    static PREFIXES: OnceLock<Vec<String>> = OnceLock::new();
    PREFIXES.get_or_init(|| {
        parse_quirks(include_str!("../../resources/volume-quirks.toml"))
            .into_iter()
            .flat_map(|q| q.ids)
            .collect()
    })
}

/// The journal's file, beside the settings.
fn journal_path() -> Option<PathBuf> {
    Some(
        super::store::path()?
            .parent()?
            .join("master-volume-journal.json"),
    )
}

/// The journal and choices in Foxglove's config folder.
pub struct StoreHost;

impl Host for StoreHost {
    fn journal(&self) -> Result<Option<Journal>, String> {
        let Some(path) = journal_path() else {
            return Ok(None);
        };
        // Only a missing file is no journal: any other read error keeps it, and the engine
        // writes nothing over it until a later read works.
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        match serde_json::from_str(&text) {
            Ok(j) => Ok(Some(j)),
            Err(e) => {
                // Kept for a person to read: the next write would replace it.
                match set_aside(&path) {
                    Some(aside) => {
                        eprintln!(
                            "Foxglove: master volume: the journal can't be read ({e}); kept as {}",
                            aside.display()
                        );
                        Ok(None)
                    }
                    // Not moved: it stays where it is, and so isn't written over either.
                    None => Err(format!("{e}, and it couldn't be moved aside")),
                }
            }
        }
    }

    fn write_journal(&mut self, journal: &Journal) -> Result<(), String> {
        let path = journal_path().ok_or("no settings folder")?;
        if journal.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        }
        let text = serde_json::to_string(journal).map_err(|e| e.to_string())?;
        // Written whole and flushed to the disk, then moved over the old one: a crash or a power
        // cut leaves one or the other, never an empty file.
        let part = path.with_extension("json.part");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
            f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }
        std::fs::rename(&part, &path).map_err(|e| e.to_string())
    }

    fn choice(&self, endpoint: &str) -> Option<bool> {
        choice_of(endpoint)
    }

    fn listed(&self, ids: &[String]) -> bool {
        master::listed(ids, prefixes())
    }

    fn saved(&self, endpoint: &str) -> Option<(f32, bool)> {
        let s = super::store::try_load::<Settings>(SECTION)?;
        let d = s.devices.into_iter().find(|d| d.endpoint == endpoint)?;
        Some((d.level?, d.muted))
    }

    fn remember(&mut self, endpoint: &str, name: &str, level: f32, muted: bool) {
        let r = super::store::update::<Settings>(SECTION, |s| {
            let d = entry(s, endpoint, name);
            d.level = Some(level);
            d.muted = muted;
        });
        if let Err(e) = r {
            eprintln!("Foxglove: master volume: the level can't be saved: {e}");
        }
    }
}

/// Moves an unreadable journal aside (`master-volume-journal.unreadable-<seconds>.json`).
fn set_aside(path: &std::path::Path) -> Option<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let aside = path.with_file_name(format!("master-volume-journal.unreadable-{secs}.json"));
    std::fs::rename(path, &aside).ok().map(|()| aside)
}

fn entry<'a>(s: &'a mut Settings, endpoint: &str, name: &str) -> &'a mut DeviceChoice {
    let i = match s.devices.iter().position(|d| d.endpoint == endpoint) {
        Some(i) => i,
        None => {
            s.devices.push(DeviceChoice {
                endpoint: endpoint.into(),
                ..DeviceChoice::default()
            });
            s.devices.len() - 1
        }
    };
    let d = &mut s.devices[i];
    if !name.is_empty() {
        d.name = name.into();
    }
    d
}

/// The user's choice for a device (`None`: the default).
pub fn choice_of(endpoint: &str) -> Option<bool> {
    super::store::try_load::<Settings>(SECTION)?
        .devices
        .into_iter()
        .find(|d| d.endpoint == endpoint)?
        .on
}

/// Stores the user's choice and tells the sessions' thread.
pub fn choose(endpoint: &str, name: &str, on: bool) {
    let r = super::store::update::<Settings>(SECTION, |s| entry(s, endpoint, name).on = Some(on));
    match r {
        Ok(_) => {
            eprintln!(
                "Foxglove: master volume: {name}: Foxglove sets its volume: {}",
                if on { "on" } else { "off" }
            );
            foxglove_panel::sessions::recheck();
        }
        Err(e) => eprintln!("Foxglove: master volume: the choice can't be saved: {e}"),
    }
}

// The words (UX plan, #1064 §1, §3, §4).

pub const CAPTION: &str = "MASTER VOLUME";
pub const TOGGLE: &str = "Foxglove sets this device's volume";
pub const LISTED_LINE: &str = "On by default: this device often ignores Windows volume.";
pub const OTHER_LINE: &str = "Turn on if the volume number moves but the sound stays the same.";
pub const ON_LINES: [&str; 4] = [
    "Windows' slider goes back near the top for this device; the sound follows the knob, the slider and the volume keys.",
    "Foxglove turns each app down instead. Volume mixer shows those lower levels; the knob shows each app's own.",
    "Apps that take exclusive control of this device play at full volume.",
    "If an app is too quiet, hold the knob on master twice, or use Reset app levels in the tray.",
];
pub const NOT_ELIGIBLE: &str = "Windows sets this device's volume itself.";
pub const AFTER_OFF: &str = "Windows sets this device's volume again.";
/// On, but not started: the journal can't be read (#1070). It replaces the while-on lines.
pub const PAUSED: &str = "Master volume is paused: Foxglove couldn't read its saved levels. \
                          It tries again when you change the volume.";
/// After turning off while master was muted: the apps stay muted with the device (#1070).
pub const AFTER_OFF_MUTED: &str =
    "Windows sets this device's volume again. Apps stay muted until you unmute the device.";
pub const OPEN_SOUND: &str = "Open Sound settings";
/// The master card's third row, on the first outside change of a run (§1).
pub const SLIDER_ROW: &str = "Windows' slider goes back near the top.";

// "Reset app levels" (UX plan ux/reset-levels, decision A): the knob's, the popup's and the tray's
// words.

/// The master card's third row after the first hold (3 s).
pub const RESET_ASK: &str = "Hold again to reset app levels";
pub const RESET_DONE: &str = "App levels reset";
pub const RESET_BUTTON: &str = "Reset app levels";
pub const RESET_HOVER: &str = "Every app back to 100 %, then turned down by master.";
pub const MASTER_TITLE: &str = "Master volume";
pub const UNDONE: &str = "App levels put back.";

/// The tray's master line: "<m> % · Foxglove sets this device's volume".
pub fn tray_master_line(master: u8) -> String {
    format!("{master} % · Foxglove sets this device's volume")
}

/// The popup after a reset: its title and second line.
pub fn done_card(r: &foxglove_panel::master::Reset) -> (String, String) {
    if r.failed > 0 {
        (
            format!("{RESET_DONE}, except {}", r.failed),
            if r.muted {
                "Master volume muted".to_string()
            } else {
                format!("All others follow master · {} %", r.master)
            },
        )
    } else {
        (
            RESET_DONE.to_string(),
            if r.muted {
                "Master volume muted".to_string()
            } else {
                format!("All apps follow master · {} %", r.master)
            },
        )
    }
}

/// The tray's line after a reset (with Undo).
pub fn tray_done(r: &foxglove_panel::master::Reset) -> String {
    if r.failed > 0 {
        format!(
            "App levels reset, except {} app{}. All others follow master.",
            r.failed,
            if r.failed == 1 { "" } else { "s" }
        )
    } else {
        format!("App levels reset. All apps follow master ({} %).", r.master)
    }
}

/// Open Sound settings' tooltip: where the switch is in the panel it opens.
pub const OPEN_SOUND_HOVER: &str =
    "Opens Windows' Sound panel. The setting is on this device's Properties > Advanced tab.";

/// The toggle's second line.
pub fn second_line(listed: bool) -> &'static str {
    if listed {
        LISTED_LINE
    } else {
        OTHER_LINE
    }
}

/// The toggle's accessible description: the row title, then the second line.
pub fn description(name: &str, listed: bool) -> String {
    format!("{name}. {}", second_line(listed))
}

/// The tray Knob row's tooltip for an app on a device Foxglove sets the volume of (§4).
pub fn tray_hover(name: &str, volume: &AppVolume) -> String {
    match volume {
        AppVolume::Scaled {
            own,
            applied,
            master,
        } => format!("{name} {own} % · Volume mixer shows {applied} % with master at {master} %."),
        AppVolume::SelfManaged => {
            format!("{name} sets its own volume, so master doesn't change it.")
        }
    }
}

// The Knob card's MASTER VOLUME part.

/// The page asked for the device's facts on this visit.
static ASKED: AtomicBool = AtomicBool::new(false);
/// The endpoint just turned off on this visit ("Windows sets this device's volume again.").
/// With whether master was muted then (its apps stay muted).
static TURNED_OFF: Mutex<Option<(String, bool)>> = Mutex::new(None);

/// What the part draws, for a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// Windows sets the volume in software: one line, no toggle.
    NotEligible { name: String },
    /// The toggle, its second line, and while on the three lines (with Open Sound settings when
    /// Windows allows exclusive control); after turning off, the line saying so.
    Toggle {
        name: String,
        on: bool,
        listed: bool,
        open_sound: bool,
        after_off: bool,
        /// Turned off while master was muted: [`AFTER_OFF_MUTED`] instead of [`AFTER_OFF`].
        kept_muted: bool,
        /// On, but the journal can't be read: [`PAUSED`] instead of the while-on lines.
        paused: bool,
    },
}

/// The part's view of `d`, with the user's choice `choice` as stored now.
/// `turned_off`: turned off on this visit, and whether master was muted then.
pub fn view(d: &Device, choice: Option<bool>, turned_off: Option<bool>) -> View {
    let name = d.name.clone();
    if !d.eligible {
        return View::NotEligible { name };
    }
    let on = master::turned_on(d.eligible, d.listed, choice);
    View::Toggle {
        name,
        on,
        listed: d.listed,
        // Windows' default is to allow it: unknown counts as allowed.
        open_sound: on && !d.paused && d.exclusive != Some(false),
        after_off: !on && turned_off.is_some(),
        kept_muted: !on && turned_off == Some(true),
        paused: on && d.paused,
    }
}

/// The lines a view draws, in order (the strings test reads them; `part` draws the same).
#[cfg(test)]
pub fn lines(v: &View) -> Vec<&'static str> {
    match v {
        View::NotEligible { .. } => vec![NOT_ELIGIBLE],
        View::Toggle {
            on,
            listed,
            after_off,
            kept_muted,
            paused,
            ..
        } => {
            let mut out = vec![TOGGLE, second_line(*listed)];
            if *paused {
                out.push(PAUSED);
            } else if *on {
                out.extend(ON_LINES);
            }
            if *after_off {
                out.push(if *kept_muted {
                    AFTER_OFF_MUTED
                } else {
                    AFTER_OFF
                });
            }
            out
        }
    }
}

/// Draws the part under the hidden-apps list (inside the Knob card).
pub fn part(ui: &mut egui::Ui) {
    if !ASKED.swap(true, Ordering::Relaxed) {
        foxglove_panel::sessions::recheck();
    }
    let Some(d) = master::device() else {
        return;
    };
    let p = theme::palette(ui);
    let turned_off = TURNED_OFF
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(e, _)| *e == d.endpoint)
        .map(|(_, muted)| *muted);
    let v = view(&d, choice_of(&d.endpoint), turned_off);
    ui.add_space(10.0);
    ui.label(theme::caps_label(ui, CAPTION));
    ui.add_space(2.0);
    ui.label(RichText::new(&d.name).color(p.text_strong));
    match &v {
        View::NotEligible { .. } => {
            ui.label(RichText::new(NOT_ELIGIBLE).color(p.muted));
        }
        View::Toggle {
            name,
            on,
            listed,
            open_sound,
            after_off,
            kept_muted,
            paused,
        } => {
            let mut checked = *on;
            if toggle(ui, &mut checked, name, *listed).changed() {
                choose(&d.endpoint, name, checked);
                // Master's mute as it stops: its apps stay muted until the device is unmuted.
                let muted = master::handled_master().is_some_and(|(_, m)| m);
                *TURNED_OFF.lock().unwrap_or_else(|e| e.into_inner()) =
                    (!checked).then(|| (d.endpoint.clone(), muted));
            }
            ui.label(RichText::new(second_line(*listed)).small().color(p.muted));
            if *paused {
                ui.label(RichText::new(PAUSED).color(p.text));
            } else if *on {
                ui.label(RichText::new(ON_LINES[0]).color(p.text));
                ui.label(RichText::new(ON_LINES[1]).color(p.text));
                ui.label(RichText::new(ON_LINES[3]).color(p.text));
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(ON_LINES[2]).color(p.text));
                    if *open_sound
                        && ui
                            .button(OPEN_SOUND)
                            .on_hover_text(OPEN_SOUND_HOVER)
                            .clicked()
                    {
                        open_sound_settings();
                    }
                });
            }
            if *after_off {
                let words = if *kept_muted {
                    AFTER_OFF_MUTED
                } else {
                    AFTER_OFF
                };
                ui.label(RichText::new(words).color(p.text));
            }
        }
    }
}

/// The toggle: its name is [`TOGGLE`], and its accessible description (what a screen reader
/// reads after the name) is the row title plus the second line, as is its tooltip.
fn toggle(ui: &mut egui::Ui, checked: &mut bool, name: &str, listed: bool) -> egui::Response {
    let words = description(name, listed);
    let r = ui.checkbox(checked, TOGGLE);
    ui.ctx()
        .accesskit_node_builder(r.id, |node| node.set_description(words.clone()));
    r.on_hover_text(words)
}

/// Windows' Sound panel (Playback tab), where the device's Advanced tab has the exclusive-control
/// setting. Foxglove never changes it itself.
fn open_sound_settings() {
    #[cfg(windows)]
    {
        let r = std::process::Command::new("control.exe")
            .arg("mmsys.cpl")
            .spawn();
        if let Err(e) = r {
            eprintln!("Foxglove: master volume: Sound settings didn't open: {e}");
        }
    }
}

/// The page changed: the after-off line goes, and the next visit reads the facts again.
pub fn page_left() {
    ASKED.store(false, Ordering::Relaxed);
    *TURNED_OFF.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(eligible: bool, listed: bool, exclusive: Option<bool>) -> Device {
        Device {
            endpoint: "{0.0.0.00000000}.{x}".into(),
            name: "Headphones (Xbox Controller)".into(),
            eligible,
            listed,
            exclusive,
            ..Device::default()
        }
    }

    #[test]
    fn the_shipped_list_is_data_and_lists_the_dongle() {
        assert!(!prefixes().is_empty());
        assert!(master::listed(
            &[r"USB\VID_0ECB&PID_2148&IGA_00\7&2f1c".to_string()],
            prefixes()
        ));
        assert!(!master::listed(
            &[r"HDAUDIO\FUNC_01&VEN_10EC".to_string()],
            prefixes()
        ));
        assert!(parse_quirks("[[device]\nids = [").is_empty());
    }

    #[test]
    fn the_setting_eligible_listed_is_on_unlisted_off_and_not_eligible_has_no_toggle() {
        let v = view(&device(true, true, Some(true)), None, None);
        assert!(matches!(
            v,
            View::Toggle {
                on: true,
                open_sound: true,
                ..
            }
        ));
        assert_eq!(lines(&v)[1], LISTED_LINE);
        assert_eq!(&lines(&v)[2..], &ON_LINES);
        let v = view(&device(true, false, None), None, None);
        assert!(matches!(v, View::Toggle { on: false, .. }));
        assert_eq!(lines(&v), [TOGGLE, OTHER_LINE]);
        // The user's choice wins over the list.
        let v = view(&device(true, true, Some(false)), Some(false), Some(false));
        assert!(matches!(
            v,
            View::Toggle {
                on: false,
                after_off: true,
                ..
            }
        ));
        assert_eq!(lines(&v).last(), Some(&AFTER_OFF));
        // Turned off while master was muted: the apps stay muted, and the line says so.
        let v = view(&device(true, true, Some(false)), Some(false), Some(true));
        assert_eq!(lines(&v).last(), Some(&AFTER_OFF_MUTED));
        // Exclusive control off in Windows: no button.
        let v = view(&device(true, true, Some(false)), None, None);
        assert!(matches!(
            v,
            View::Toggle {
                open_sound: false,
                ..
            }
        ));
        // On, but the journal can't be read: one plain line instead of the while-on lines.
        let mut d = device(true, true, Some(true));
        d.paused = true;
        let v = view(&d, None, None);
        assert!(matches!(
            v,
            View::Toggle {
                on: true,
                paused: true,
                open_sound: false,
                ..
            }
        ));
        assert_eq!(lines(&v), [TOGGLE, LISTED_LINE, PAUSED]);
        // Windows sets the volume in software: one line, no toggle, even listed and chosen.
        let v = view(&device(false, true, None), Some(true), None);
        assert_eq!(lines(&v), [NOT_ELIGIBLE]);
    }

    #[test]
    fn the_words_are_the_plans() {
        assert_eq!(
            description("Headphones (Xbox Controller)", true),
            "Headphones (Xbox Controller). On by default: this device often ignores Windows volume."
        );
        assert_eq!(
            tray_hover(
                "Escape from Tarkov",
                &AppVolume::Scaled {
                    own: 84,
                    applied: 29,
                    master: 35
                }
            ),
            "Escape from Tarkov 84 % · Volume mixer shows 29 % with master at 35 %."
        );
        assert_eq!(
            tray_hover("Game", &AppVolume::SelfManaged),
            "Game sets its own volume, so master doesn't change it."
        );
        // No session ids, "session", "mirror" or counts in a drawn string.
        let all = [
            CAPTION,
            TOGGLE,
            LISTED_LINE,
            OTHER_LINE,
            NOT_ELIGIBLE,
            AFTER_OFF,
            AFTER_OFF_MUTED,
            PAUSED,
            OPEN_SOUND,
            SLIDER_ROW,
            OPEN_SOUND_HOVER,
        ]
        .into_iter()
        .chain(ON_LINES);
        // UX review #1069: "near the top", no number (the hold depends on the device's steps).
        assert_eq!(SLIDER_ROW, "Windows' slider goes back near the top.");
        assert_eq!(
            ON_LINES[0],
            "Windows' slider goes back near the top for this device; the sound follows the \
             knob, the slider and the volume keys."
        );
        for s in all {
            let l = s.to_lowercase();
            for word in ["session", "mirror", "endpoint", "journal", "{"] {
                assert!(!l.contains(word), "{s}");
            }
        }
    }

    #[test]
    fn the_toggle_has_the_row_title_and_second_line_as_its_accessible_description() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut on = true;
            toggle(ui, &mut on, "Headphones (Xbox Controller)", true);
        });
        // Nothing paints here: the font atlas's upload is dropped on purpose.
        out.textures_delta.clear();
        let update = out
            .platform_output
            .accesskit_update
            .expect("an accessibility tree");
        let node = update
            .nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(TOGGLE))
            .expect("the toggle's node, named by its label");
        assert_eq!(
            node.description(),
            Some(concat!(
                "Headphones (Xbox Controller). ",
                "On by default: this device often ignores Windows volume."
            ))
        );
    }

    #[test]
    fn the_choice_and_level_survive_a_restart() {
        super::super::store::test_config_dir();
        let ep = "{0.0.0.00000000}.{choice-1064}";
        choose(ep, "Headphones", false);
        assert_eq!(choice_of(ep), Some(false));
        let mut h = StoreHost;
        h.remember(ep, "Headphones", 0.35, false);
        assert_eq!(h.saved(ep), Some((0.35, false)));
        assert_eq!(choice_of(ep), Some(false), "remembering keeps the choice");
        // The journal round-trips, and an empty one is no file.
        let mut j = Journal::default();
        j.apps.insert(
            "sid".into(),
            master::Record {
                own: 0.84,
                applied: 0.15,
                by_master: false,
                day: 1,
            },
        );
        h.write_journal(&j).unwrap();
        assert_eq!(h.journal(), Ok(Some(j)));
        h.write_journal(&Journal::default()).unwrap();
        assert_eq!(h.journal(), Ok(None));
        assert!(!journal_path().unwrap().exists());
        // A journal that can't be read (here: a folder in its place) is not "no journal".
        let path = journal_path().unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(h.journal().is_err());
        std::fs::remove_dir(&path).unwrap();
        // An unparsable journal is set aside, never overwritten.
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(h.journal(), Ok(None));
        assert!(!path.exists());
        let dir = path.parent().unwrap();
        let kept: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("master-volume-journal.unreadable-")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            std::fs::read_to_string(kept[0].path()).unwrap(),
            "{ not json"
        );
    }
}
