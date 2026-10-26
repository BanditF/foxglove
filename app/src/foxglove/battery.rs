//! Foxglove (#372, #373): the keyboard's battery outside Settings. Both follow the battery
//! events the link already sends (`DeviceEvent::Battery`, from HELLO and its refreshes); nothing
//! polls.
//!
//! - **Heads-up:** one Windows notification when a half drops below 15%, and one more below 5%.
//!   Each fires once per crossing; a half has to charge above 20% before it can fire again. On by
//!   default, with a switch on the Keyboard page.
//! - **Tray tooltip:** `Right 61%` (and the left once it's reported) under the status line,
//!   `Right —` while the right half isn't reporting.
//!
//! The right half's level comes from HELLO (`right_battery`); the left's own, from firmware with
//! the LEFT_BATTERY bit, rides on CONN_STATE (byte 8) and shows up here the same way.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

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
    /// past both at once tells only the lower one.
    pub fn level(&mut self, part: &str, percent: u8) -> Option<u8> {
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

/// The notification's title and text.
pub fn notice(part: &str, threshold: u8, percent: u8) -> (String, String) {
    let half = capitalised(part);
    if threshold <= THRESHOLDS[THRESHOLDS.len() - 1] {
        (
            format!("{half} half battery very low"),
            format!("{percent}% left. It will switch off soon: plug it in to charge."),
        )
    } else {
        (
            format!("{half} half battery low"),
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

/// The battery line for the tray tooltip: the right always (`—` when not reporting), the left
/// and any other part only once reported.
pub fn tooltip_line(levels: &BTreeMap<String, u8>) -> String {
    let mut parts = vec![format!(
        "Right {}",
        levels.get("right").map_or("—".into(), |p| format!("{p}%"))
    )];
    for (part, p) in levels.iter().filter(|(k, _)| k.as_str() != "right") {
        parts.insert(
            if part == "left" { 0 } else { parts.len() },
            format!("{} {p}%", capitalised(part)),
        );
    }
    parts.join(" · ")
}

/// The whole tooltip, inside Windows' 127 characters.
pub fn tooltip(status: &str, levels: &BTreeMap<String, u8>) -> String {
    let text = format!("Foxglove\n{status}\n{}", tooltip_line(levels));
    if text.chars().count() <= 127 {
        return text;
    }
    let mut cut: String = text.chars().take(126).collect();
    cut.push('…');
    cut
}

/// The `[battery]` section of the settings store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatterySettings {
    pub low_battery_notice: bool,
}

impl Default for BatterySettings {
    fn default() -> Self {
        Self {
            low_battery_notice: true,
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
    super::store::save(
        "battery",
        &BatterySettings {
            low_battery_notice: on,
        },
    )
}

/// The latest level per half, for the tooltip; cleared when the keyboard goes.
static LEVELS: Mutex<BTreeMap<String, u8>> = Mutex::new(BTreeMap::new());
static WAKE: OnceLock<crate::ui_wake::UiWake> = OnceLock::new();

/// The UI's wake, so a new level reaches the tray tooltip at once.
pub fn set_wake(wake: crate::ui_wake::UiWake) {
    let _ = WAKE.set(wake);
}

pub fn levels() -> BTreeMap<String, u8> {
    LEVELS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The backend of the keyboard with Foxglove's host link.
const HOST_KEYBOARD: &str = "zmk";

/// Follows the battery events. Blocks on the device bus; ends with the process.
pub fn start() {
    let events = super::devices::bus().subscribe(false);
    super::services::spawn_named("battery", move || {
        let mut alerts = Alerts::default();
        for e in events {
            if e.device.backend.0 != HOST_KEYBOARD {
                continue;
            }
            let changed = match &e.event {
                DeviceEvent::Battery { part, percent } => {
                    if let Some(threshold) = alerts.level(part, *percent) {
                        if notices_on() {
                            let (title, text) = notice(part, threshold, *percent);
                            imp::show(&title, &text);
                        }
                    }
                    LEVELS
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(part.clone(), *percent)
                        != Some(*percent)
                }
                DeviceEvent::Disconnected { .. } => {
                    let mut levels = LEVELS.lock().unwrap_or_else(|e| e.into_inner());
                    let had = !levels.is_empty();
                    levels.clear();
                    had
                }
                _ => false,
            };
            if changed {
                if let Some(w) = WAKE.get() {
                    w.request_repaint();
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
        assert_eq!(notice("right", 15, 14).0, "Right half battery low");
        assert!(notice("right", 5, 4).0.contains("very low"));
        assert!(notice("left", 5, 4).1.starts_with("4% left."));
    }

    #[test]
    fn the_tooltip_follows_the_levels() {
        let mut levels = BTreeMap::new();
        assert_eq!(
            tooltip("Disconnected", &levels),
            "Foxglove\nDisconnected\nRight —"
        );
        levels.insert("right".to_string(), 61);
        assert_eq!(tooltip_line(&levels), "Right 61%");
        levels.insert("left".to_string(), 80);
        assert_eq!(tooltip_line(&levels), "Left 80% · Right 61%");
        let long = "x".repeat(200);
        let t = tooltip(&long, &levels);
        assert_eq!(t.chars().count(), 127);
        assert!(t.ends_with('…'));
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
    }
}
