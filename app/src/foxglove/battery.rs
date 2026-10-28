//! Foxglove (#372, #373): the keyboard's battery outside Settings. Both follow the battery
//! events the link already sends (`DeviceEvent::Battery`, from HELLO and its refreshes); nothing
//! polls.
//!
//! - **Heads-up:** one Windows notification when a half drops below 15%, and one more below 5%.
//!   Each fires once per crossing; a half has to charge above 20% before it can fire again. On by
//!   default, with a switch on the Keyboard page.
//! - **Tray tooltip:** `Right 61% · Left —` under the status line, `—` for a half that isn't
//!   reporting (the left needs firmware with LEFT_BATTERY). It reads the same keyboard facts as
//!   the tray panel (`keyboard.<part>.battery`) and is refreshed on each change of them.
//!
//! The right half's level comes from HELLO (`right_battery`); the left's own, from firmware with
//! the LEFT_BATTERY bit, rides on CONN_STATE (byte 8) and shows up here the same way.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use foxglove_devices::facts::{Facts, Value};
use foxglove_devices::DeviceEvent;
use serde::{Deserialize, Serialize};

/// Below these a heads-up shows, highest first.
pub const THRESHOLDS: [u8; 2] = [15, 5];
/// Above this a half's heads-ups can fire again.
pub const RESET_ABOVE: u8 = 20;

/// The heads-ups already shown per half, until it charges past [`RESET_ABOVE`].
#[derive(Debug, Default)]
pub struct Alerts {
    fired: BTreeMap<String, Vec<u8>>,
}

impl Alerts {
    /// A new level for `part`: the threshold to tell the user about, if this crossed one. A drop
    /// past both at once tells only the lower one. Only real readings count: 0 is what ZMK
    /// reports before its first battery sample (right after a boot, a flash or a reconnect), and
    /// anything over 100 isn't a level, so neither can raise a notice or re-arm one.
    pub fn level(&mut self, part: &str, percent: u8) -> Option<u8> {
        if !(1..=100).contains(&percent) {
            return None;
        }
        let fired = self.fired.entry(part.to_string()).or_default();
        if percent > RESET_ABOVE {
            fired.clear();
            return None;
        }
        let crossed: Vec<u8> = THRESHOLDS
            .into_iter()
            .filter(|t| percent < *t && !fired.contains(t))
            .collect();
        fired.extend(&crossed);
        crossed.into_iter().min()
    }
}

/// The notification's title and text, for the part the link calls `part` ("right": the
/// Corne's "Right half", a one-piece board's "Keyboard", #559).
pub fn notice(part: &str, threshold: u8, percent: u8) -> (String, String) {
    notice_for(&super::parts::Parts::current(), part, threshold, percent)
}

fn notice_for(
    parts: &super::parts::Parts,
    part: &str,
    threshold: u8,
    percent: u8,
) -> (String, String) {
    let half = parts.name_of(part);
    if threshold <= THRESHOLDS[THRESHOLDS.len() - 1] {
        (
            format!("{half} battery very low"),
            format!("{percent}% left. It will switch off soon: plug it in to charge."),
        )
    } else {
        (
            format!("{half} battery low"),
            format!("{percent}% left. Charge it soon so it doesn't stop typing."),
        )
    }
}

fn capitalised(part: &str) -> String {
    let mut c = part.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The battery line for the tray tooltip: the board's parts, the last first ("Right 61% · Left
/// 80%", or "Keyboard 80%" for a one-piece board; `—` for one not reporting), then any other
/// part that reported.
fn tooltip_line_for(levels: &BTreeMap<String, u8>, board: &super::parts::Parts) -> String {
    use super::parts::Parts;
    let level = |part: &str| levels.get(part).map_or("—".into(), |p| format!("{p}%"));
    let board = board
        .clone()
        .seen(if levels.contains_key("right") { 2 } else { 1 });
    let linked: Vec<(usize, &str)> = (0..board.len())
        .filter_map(|i| Some((i, Parts::link_id(i)?)))
        .collect();
    let mut parts: Vec<String> = linked
        .iter()
        .rev()
        .map(|(i, id)| format!("{} {}", board.short(*i), level(id)))
        .collect();
    for (part, p) in levels
        .iter()
        .filter(|(k, _)| !linked.iter().any(|(_, id)| id == k))
    {
        parts.push(format!("{} {p}%", capitalised(part)));
    }
    parts.join(" · ")
}

/// The whole tooltip, inside Windows' 127 characters. A long status line is cut, never the
/// battery line under it: a status that once carried the art relay's whole report pushed the
/// battery past the limit, so the tooltip never showed it.
pub fn tooltip(status: &str, levels: &BTreeMap<String, u8>) -> String {
    tooltip_for(status, levels, &super::parts::Parts::current())
}

fn tooltip_for(status: &str, levels: &BTreeMap<String, u8>, board: &super::parts::Parts) -> String {
    let battery = tooltip_line_for(levels, board);
    // "Foxglove\n" and the "\n" before the battery line.
    let room = 127usize.saturating_sub(10 + battery.chars().count());
    let status = if status.chars().count() <= room {
        status.to_string()
    } else {
        let mut cut: String = status.chars().take(room.saturating_sub(1)).collect();
        cut.push('…');
        cut
    };
    format!("Foxglove\n{status}\n{battery}")
}

/// The `[battery]` section of the settings store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatterySettings {
    pub low_battery_notice: bool,
    /// The same heads-up for Bluetooth devices and controllers (#504).
    pub device_notice: bool,
}

impl Default for BatterySettings {
    fn default() -> Self {
        Self {
            low_battery_notice: true,
            device_notice: true,
        }
    }
}

/// The switch, read from the store once and then kept here: 0 = not read yet, 1 = off, 2 = on.
static NOTICES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn notices_on() -> bool {
    use std::sync::atomic::Ordering;
    match NOTICES.load(Ordering::Relaxed) {
        0 => {
            let on = super::store::try_load::<BatterySettings>("battery")
                .unwrap_or_default()
                .low_battery_notice;
            NOTICES.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        v => v == 2,
    }
}

pub fn set_notices(on: bool) -> std::io::Result<()> {
    NOTICES.store(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
    save_with(|s| s.low_battery_notice = on)
}

/// Foxglove (#504): the switch for the other devices' heads-ups, cached like [`notices_on`].
static DEVICE_NOTICES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn device_notices_on() -> bool {
    use std::sync::atomic::Ordering;
    match DEVICE_NOTICES.load(Ordering::Relaxed) {
        0 => {
            let on = super::store::try_load::<BatterySettings>("battery")
                .unwrap_or_default()
                .device_notice;
            DEVICE_NOTICES.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        v => v == 2,
    }
}

pub fn set_device_notices(on: bool) -> std::io::Result<()> {
    DEVICE_NOTICES.store(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
    save_with(|s| s.device_notice = on)
}

/// Changes one switch and keeps the other as stored.
fn save_with(change: impl FnOnce(&mut BatterySettings)) -> std::io::Result<()> {
    let mut s = super::store::try_load::<BatterySettings>("battery").unwrap_or_default();
    change(&mut s);
    super::store::save("battery", &s)
}

/// Shows a battery notification (the halves' and the other devices').
pub fn show_notice(title: &str, text: &str) {
    imp::show(title, text);
}

/// A frame for the tray and the pages, after a battery list changed.
pub fn wake() {
    if let Some(w) = WAKE.get() {
        w.request_repaint();
    }
}

static WAKE: OnceLock<crate::ui_wake::UiWake> = OnceLock::new();

/// The UI's wake, so a new level reaches the tray tooltip at once.
pub fn set_wake(wake: crate::ui_wake::UiWake) {
    let _ = WAKE.set(wake);
}

/// The latest level per half, from the keyboard facts the tray panel shows too; gone when the
/// keyboard goes.
pub fn levels() -> BTreeMap<String, u8> {
    levels_in(super::devices::bus().facts())
}

/// The `keyboard.<part>.battery` facts as levels per part.
fn levels_in(facts: &Facts) -> BTreeMap<String, u8> {
    facts
        .list("keyboard.*")
        .into_iter()
        .filter_map(|(key, value)| {
            let part = key.strip_prefix("keyboard.")?.strip_suffix(".battery")?;
            match value {
                Value::Int(p) if !part.contains('.') => {
                    Some((part.to_string(), p.clamp(0, 100) as u8))
                }
                _ => None,
            }
        })
        .collect()
}

/// True for a fact the tooltip shows.
fn is_level(key: &str) -> bool {
    key.starts_with("keyboard.") && key.ends_with(".battery")
}

/// The backend of the keyboard with Foxglove's host link.
const HOST_KEYBOARD: &str = "zmk";

/// Follows the battery events (heads-ups) and the battery facts (the tooltip). Blocks on the
/// device bus; ends with the process.
pub fn start() {
    // The tooltip: a repaint on each battery fact that changes (set from the first reading
    // after a connect, cleared on a disconnect), so the tray reads it at once.
    // Supervised (#587): the subscriptions made here go to the first attempt (nothing is
    // missed before it starts); a restart subscribes again.
    let changes =
        super::services::First::new(super::devices::bus().facts().subscribe("keyboard.*"));
    let _ = super::services::spawn_supervised("battery-tooltip", move || {
        let changes = changes.take_or(|| super::devices::bus().facts().subscribe("keyboard.*"));
        for change in changes {
            if is_level(&change.key) {
                if let Some(w) = WAKE.get() {
                    w.request_repaint();
                }
            }
        }
    });
    let events = super::services::First::new(super::devices::bus().subscribe(false));
    let _ = super::services::spawn_supervised("battery", move || {
        let events = events.take_or(|| super::devices::bus().subscribe(false));
        let mut alerts = Alerts::default();
        for e in events {
            if e.device.backend.0 != HOST_KEYBOARD {
                continue;
            }
            if let DeviceEvent::Battery { part, percent } = &e.event {
                if let Some(threshold) = alerts.level(part, *percent) {
                    if notices_on() {
                        let (title, text) = notice(part, threshold, *percent);
                        imp::show(&title, &text);
                    }
                }
            }
        }
    });
}

/// The XML-escaped text for a toast.
#[cfg_attr(not(windows), allow(dead_code))]
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg_attr(not(windows), allow(dead_code))]
fn toast_xml(title: &str, text: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        escape(title),
        escape(text)
    )
}

#[cfg(windows)]
mod imp {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    /// The app id the toasts carry; registered under HKCU so an unpackaged app may show them.
    const APP_ID: &str = "BanditF.Foxglove";

    /// `HKCU\Software\Classes\AppUserModelId\BanditF.Foxglove`: the name and icon Windows shows.
    fn register() -> std::io::Result<()> {
        let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
        key.set_value("DisplayName", &"Foxglove")?;
        if let Some(dir) =
            super::super::store::path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            let icon = dir.join("notification-icon.png");
            if !icon.exists() {
                std::fs::create_dir_all(&dir)?;
                std::fs::write(&icon, include_bytes!("../../resources/icon.png"))?;
            }
            key.set_value("IconUri", &icon.to_string_lossy().to_string())?;
        }
        Ok(())
    }

    pub fn show(title: &str, text: &str) {
        if let Err(e) = register() {
            eprintln!("Foxglove: can't register for notifications: {e}");
        }
        let shown = (|| -> windows::core::Result<()> {
            let xml = XmlDocument::new()?;
            xml.LoadXml(&HSTRING::from(super::toast_xml(title, text)))?;
            let toast = ToastNotification::CreateToastNotification(&xml)?;
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?
                .Show(&toast)
        })();
        match shown {
            Ok(()) => eprintln!("Foxglove: battery notice: {title}"),
            Err(e) => eprintln!("Foxglove: can't show the battery notice: {e}"),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn show(title: &str, _text: &str) {
        eprintln!("Foxglove: battery notice: {title}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corne() -> crate::foxglove::parts::Parts {
        crate::foxglove::fixtures::users_desk::board().parts()
    }

    /// The notices a sequence of levels gives.
    fn run(levels: &[u8]) -> Vec<(u8, u8)> {
        let mut a = Alerts::default();
        levels
            .iter()
            .filter_map(|&p| a.level("right", p).map(|t| (p, t)))
            .collect()
    }

    #[test]
    fn one_notice_per_threshold_crossing() {
        // Draining slowly, with repeats (a reconnect resends the level): 15% once, then 5% once.
        assert_eq!(
            run(&[40, 20, 16, 15, 14, 14, 13, 10, 10, 6, 5, 4, 4, 3, 1]),
            vec![(14, 15), (4, 5)]
        );
    }

    #[test]
    fn charging_past_twenty_rearms_and_hovering_doesnt() {
        // Charging to 18 and draining again doesn't repeat; past 20 it can fire again.
        assert_eq!(run(&[14, 18, 14, 19, 12]), vec![(14, 15)]);
        assert_eq!(run(&[14, 21, 14]), vec![(14, 15), (14, 15)]);
        assert_eq!(run(&[4, 20, 4]), vec![(4, 5)], "20 is not above 20");
    }

    #[test]
    fn a_first_reading_below_both_tells_once() {
        assert_eq!(run(&[3, 3, 2]), vec![(3, 5)]);
        // After that, only a charge past 20 starts over.
        assert_eq!(run(&[3, 12, 3, 30, 12, 4]), vec![(3, 5), (12, 15), (4, 5)]);
    }

    /// After a flash the left reported 0 (no reading yet) and then its real 100: the app said
    /// "Left half battery very low". Unknown readings tell nothing and change nothing.
    #[test]
    fn a_reading_before_the_first_sample_is_not_low() {
        assert_eq!(run(&[0, 100, 0, 100]), vec![]);
        assert_eq!(run(&[0, 0, 255]), vec![]);
        // A real low level after an unknown one still tells, once.
        assert_eq!(run(&[0, 14, 0, 14, 13]), vec![(14, 15)]);
        // An unknown reading doesn't re-arm a notice that fired: only a charge past 20 does.
        assert_eq!(run(&[14, 0, 14]), vec![(14, 15)]);
    }

    #[test]
    fn halves_are_separate() {
        let mut a = Alerts::default();
        assert_eq!(a.level("right", 14), Some(15));
        assert_eq!(a.level("left", 14), Some(15));
        assert_eq!(a.level("right", 13), None);
        assert_eq!(a.level("left", 50), None);
        assert_eq!(a.level("left", 14), Some(15));
    }

    #[test]
    fn notices_read_plainly() {
        let corne = crate::foxglove::fixtures::users_desk::board().parts();
        assert_eq!(
            notice_for(&corne, "right", 15, 14).0,
            "Right half battery low"
        );
        assert!(notice_for(&corne, "right", 5, 4).0.contains("very low"));
        assert!(notice_for(&corne, "left", 5, 4).1.starts_with("4% left."));
        // #559: a one-piece board's battery is the keyboard's.
        let plank = crate::foxglove::fixtures::second_desk::board().parts();
        assert_eq!(notice_for(&plank, "left", 15, 14).0, "Keyboard battery low");
        let mut levels = BTreeMap::new();
        levels.insert("left".to_string(), 80);
        assert_eq!(tooltip_line_for(&levels, &plank), "Keyboard 80%");
    }

    #[test]
    fn the_tooltip_follows_the_levels() {
        let mut levels = BTreeMap::new();
        assert_eq!(
            tooltip_for("Disconnected", &levels, &corne()),
            "Foxglove\nDisconnected\nRight — · Left —"
        );
        levels.insert("right".to_string(), 61);
        assert_eq!(tooltip_line_for(&levels, &corne()), "Right 61% · Left —");
        levels.insert("left".to_string(), 80);
        assert_eq!(tooltip_line_for(&levels, &corne()), "Right 61% · Left 80%");
        let long = "x".repeat(200);
        let t = tooltip_for(&long, &levels, &corne());
        assert_eq!(t.chars().count(), 127);
        assert!(
            t.ends_with("\nRight 61% · Left 80%"),
            "the battery is never cut: {t}"
        );
        assert!(t.contains('…'));
        // The status the tray once used, with the art relay's report in it, still shows the
        // battery.
        let old = "Connected Eyelash_Corne / right art: left: idle, 0/0 bytes pushed, error: none; right: idle, set 3, 12 built-in, slot 8192 bytes";
        assert!(tooltip_for(old, &levels, &corne()).ends_with("Right 61% · Left 80%"));
        // Windows counts UTF-16 units: every character here is one.
        assert!(tooltip_for(&long, &levels, &corne()).encode_utf16().count() <= 127);
    }

    /// The tooltip reads the keyboard facts the tray panel shows: the first reading after a
    /// connect (HELLO's right level, here over BLE) shows at once, a new reading replaces it, and
    /// a disconnect clears it.
    #[test]
    fn the_tooltip_reads_the_panels_battery_facts() {
        let facts = Facts::default();
        let changes = facts.subscribe("keyboard.*");
        assert_eq!(
            tooltip_line_for(&levels_in(&facts), &corne()),
            "Right — · Left —"
        );
        facts.set("keyboard.connected", Value::Bool(true));
        facts.set("keyboard.link", Value::Text("ble".into()));
        facts.set("keyboard.right.battery", Value::Int(91));
        assert_eq!(
            tooltip_line_for(&levels_in(&facts), &corne()),
            "Right 91% · Left —"
        );
        facts.set("keyboard.right.battery", Value::Int(90));
        facts.set("keyboard.left.battery", Value::Int(77));
        assert_eq!(
            tooltip_line_for(&levels_in(&facts), &corne()),
            "Right 90% · Left 77%"
        );
        facts.remove_prefix("keyboard.", &["keyboard.connected"]);
        assert_eq!(
            tooltip_line_for(&levels_in(&facts), &corne()),
            "Right — · Left —"
        );
        // Each battery change wakes the tray; the other keyboard facts don't.
        let woken = changes.try_iter().filter(|c| is_level(&c.key)).count();
        assert_eq!(woken, 5, "3 readings and 2 cleared halves");
    }

    #[test]
    fn toast_text_is_escaped() {
        let xml = toast_xml("A & B", "<5%>");
        assert!(xml.contains("<text>A &amp; B</text><text>&lt;5%&gt;</text>"));
    }

    #[test]
    fn the_notice_is_on_by_default() {
        crate::foxglove::store::test_config_dir();
        assert!(BatterySettings::default().low_battery_notice);
        let parsed: BatterySettings = toml::from_str("").unwrap();
        assert!(
            parsed.low_battery_notice,
            "a store without the section keeps it on"
        );
        // #504: a store from before the devices' switch keeps the halves' and turns that on.
        let old: BatterySettings = toml::from_str("low_battery_notice = false").unwrap();
        assert!(!old.low_battery_notice && old.device_notice);
    }
}
