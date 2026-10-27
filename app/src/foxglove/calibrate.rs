//! Colour calibration's settings: every device's calibration in the store's `[calibration]`
//! section (versioned), loaded into the device layer's registry at startup so every driver
//! applies it (`foxglove_devices::calibration`).
//!
//! Calibration belongs to the device, never to a colour or a profile: it isn't in
//! `store::PROFILED`, and changing a palette or a profile leaves it alone.
//!
//! The calibrator (`calibrate_ui`) writes through here: [`show`] puts a colour on a device
//! without saving it as the user's (the PC lights' worker coalesces a burst; the keyboard has
//! a one-slot writer here), [`restore`] puts back what a device showed before, and [`commit`]
//! saves a calibration. Every write follows a slider move or a click; nothing waits on a timer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

use foxglove_devices::calibration::{self, Calibration, Calibrations, DeviceCalibration, Hsb};

use super::pc_lights::{self, Msg};

/// The store section.
pub const SECTION: &str = "calibration";

/// The saved section couldn't be read: saving would overwrite it, so saving is refused.
static UNREADABLE: AtomicBool = AtomicBool::new(false);

/// A device the calibrator can calibrate or compare with.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Its key in the calibration registry.
    pub key: String,
    pub name: String,
    pub kind: Kind,
    /// The model's default calibration, from the catalog.
    pub default: Option<Calibration>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// The keyboard with Foxglove's firmware: one colour (hue, saturation, brightness).
    Keyboard,
    /// A PC light Foxglove drives, with its zones (id, name).
    Lights { zones: Vec<(String, String)> },
}

/// What a device showed before the calibrator took it over, to put back.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Before {
    #[default]
    Unknown,
    Keyboard(Hsb),
    Lights(Vec<Option<[u8; 3]>>),
}

/// An sRGB colour as the keyboard's hue, saturation and brightness.
pub fn hsb(c: [u8; 3]) -> Hsb {
    let (h, s, v) = calibration::rgb_to_hsv(c.map(|v| f32::from(v) / 255.0));
    (
        (h.round() as u16) % 360,
        (s * 100.0).round() as u8,
        (v * 100.0).round() as u8,
    )
}

/// What a device shows now, from what Foxglove last set (the palette's colours).
pub fn capture(t: &Target) -> Before {
    match &t.kind {
        Kind::Keyboard => super::keyboard_settings::cached_rgb_state()
            .map_or(Before::Unknown, |s| {
                Before::Keyboard((s.left.hue, s.left.sat, s.left.brightness))
            }),
        Kind::Lights { .. } => pc_lights::snapshot()
            .devices
            .iter()
            .find(|r| r.key == t.key)
            .map_or(Before::Unknown, |r| {
                Before::Lights(r.zones.iter().map(|(_, c)| *c).collect())
            }),
    }
}

/// Shows one colour on every zone of a device, through its calibration as the registry holds
/// it now, without saving the colour.
pub fn show(t: &Target, color: [u8; 3]) {
    match &t.kind {
        Kind::Keyboard => keyboard(hsb(color)),
        Kind::Lights { zones } => pc_lights::send(Msg::Show {
            key: t.key.clone(),
            colors: (0..zones.len()).map(|i| (i, color)).collect(),
        }),
    }
}

/// Puts back what a device showed before; zones Foxglove never set end white, as after
/// Identify.
pub fn restore(t: &Target, before: &Before) {
    match (&t.kind, before) {
        (Kind::Keyboard, Before::Keyboard(c)) => {
            keyboard(*c);
        }
        (Kind::Lights { zones }, Before::Lights(colors)) => pc_lights::send(Msg::Show {
            key: t.key.clone(),
            colors: (0..zones.len())
                .map(|i| (i, colors.get(i).copied().flatten().unwrap_or([255; 3])))
                .collect(),
        }),
        (Kind::Lights { .. }, Before::Unknown) => pc_lights::send(Msg::Recalibrate(t.key.clone())),
        // The keyboard's colour wasn't known: white rather than a calibration step.
        (Kind::Keyboard, _) => keyboard((0, 0, 100)),
        _ => {}
    }
}

/// The colour the keyboard's calibrator writer sends next; `None` once it's taken.
static KEYBOARD_NEXT: Mutex<Option<Hsb>> = Mutex::new(None);
static KEYBOARD_WAKE: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();

/// Sets the keyboard's colour from the calibrator. One slot: while a write is on its way, a
/// newer colour replaces the waiting one, so sliders never queue up writes. The writer thread
/// blocks on its channel between colours.
fn keyboard(c: Hsb) {
    let idle = KEYBOARD_NEXT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(c)
        .is_none();
    if !idle {
        return;
    }
    let wake = KEYBOARD_WAKE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<()>();
        super::services::spawn_named("calibrate-keyboard", move || {
            for () in rx {
                // The slot's lock is let go before each write (a helper, not a `while let`
                // temporary, which would hold it through the write).
                while let Some(c) = next_keyboard() {
                    write_keyboard(c);
                }
            }
        });
        Mutex::new(tx)
    });
    if wake
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(())
        .is_err()
    {
        // The writer is gone (it panicked): let the next colour try again, not wait forever.
        next_keyboard();
        eprintln!("Foxglove: calibrate: the keyboard writer stopped");
    }
}

fn next_keyboard() -> Option<Hsb> {
    KEYBOARD_NEXT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

fn write_keyboard((hue, sat, brightness): Hsb) {
    // Tests never look for a keyboard: the user's own is plugged in.
    if cfg!(test) || super::pause::paused() {
        return;
    }
    let Some(d) = super::devices::device() else {
        return;
    };
    let Some(lighting) = foxglove_devices::Device::lighting(d.as_ref()) else {
        return;
    };
    if let Err(e) = lighting.set_color(hue, sat, brightness) {
        eprintln!("Foxglove: calibrate: {}", super::devices::describe(&e));
    }
    super::keyboard_settings::refresh_rgb_state();
}

fn rgb(c: [u8; 3]) -> foxglove_devices::Rgb {
    foxglove_devices::Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

/// A colour as Foxglove draws it for a zone of a device: through the device's screen side.
pub fn on_screen(key: &str, zone: Option<&str>, c: [u8; 3]) -> [u8; 3] {
    let out = calibration::global().on_screen(key, zone, rgb(c));
    [out.r, out.g, out.b]
}

/// [`on_screen`] as a colour to paint.
pub fn screen_color(key: &str, zone: Option<&str>, c: [u8; 3]) -> egui::Color32 {
    let [r, g, b] = on_screen(key, zone, c);
    egui::Color32::from_rgb(r, g, b)
}

/// The colour a picker drawn through [`on_screen`] stands for.
pub fn from_screen(key: &str, zone: Option<&str>, c: [u8; 3]) -> [u8; 3] {
    let out = calibration::global().from_screen(key, zone, rgb(c));
    [out.r, out.g, out.b]
}

/// Whether a device's matches cover a colour; `None` when it has none.
pub fn covered(key: &str, zone: Option<&str>, c: [u8; 3]) -> Option<bool> {
    calibration::global().covers(key, zone, rgb(c))
}

/// A colour Foxglove draws for the keyboard, through its screen side (as is while no keyboard
/// is connected).
pub fn keyboard_screen(c: egui::Color32) -> egui::Color32 {
    match super::devices::keyboard_key() {
        Some(key) => screen_color(&key, None, [c.r(), c.g(), c.b()]),
        None => c,
    }
}

/// Saves every calibration the registry holds, unless the saved section couldn't be read at
/// startup (saving would overwrite it).
pub fn save() -> std::io::Result<()> {
    if UNREADABLE.load(Ordering::Relaxed) {
        return Err(std::io::Error::other(
            "the saved calibration is from a newer Foxglove or damaged, so it isn't overwritten",
        ));
    }
    super::store::save(SECTION, &calibration::global().all())
}

/// Sets (or forgets) one device's calibration and saves it.
pub fn commit(key: &str, cal: Option<DeviceCalibration>) -> std::io::Result<()> {
    calibration::global().set_device(key, cal);
    save()
}

/// Reads the saved calibrations into the registry. Call once at startup, before anything
/// writes a colour. A section this Foxglove can't read (a newer version, an unknown field) is
/// logged and left as it is: the devices run uncalibrated rather than half calibrated.
pub fn load() {
    load_into(calibration::global());
}

fn load_into(registry: &calibration::Registry) {
    let Some(section) = super::store::try_load::<toml::Table>(SECTION) else {
        return;
    };
    match read(section) {
        Ok(all) => registry.replace(all),
        Err(e) => {
            UNREADABLE.store(true, Ordering::Relaxed);
            eprintln!("Foxglove: [{SECTION}]: {e}; colours go out uncalibrated");
        }
    }
}

/// The section's calibrations, refusing a version newer than this Foxglove reads.
fn read(section: toml::Table) -> Result<Calibrations, String> {
    let version = section
        .get("version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(i64::from(calibration::VERSION));
    if version > i64::from(calibration::VERSION) {
        return Err(format!(
            "version {version} is newer than this Foxglove reads ({})",
            calibration::VERSION
        ));
    }
    toml::Value::Table(section)
        .try_into::<Calibrations>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_devices::calibration::{Calibration, DeviceCalibration, Match};

    #[test]
    fn a_section_from_a_newer_foxglove_or_with_unknown_fields_is_not_read() {
        let parse = |text: &str| read(toml::from_str::<toml::Table>(text).unwrap());
        assert!(parse("version = 2\n").unwrap_err().contains("version 2"));
        let unknown = "version = 1\n[devices.\"1d50:615e\".device]\nsparkle = 1.0\n";
        assert!(parse(unknown).is_err());
        let ok = "version = 1\n[devices.\"1d50:615e\".device]\ngain = [1.0, 0.9, 0.8]\n";
        assert_eq!(
            parse(ok).unwrap().devices["1d50:615e"].device.gain,
            [1.0, 0.9, 0.8]
        );
        assert!(parse("").unwrap().devices.is_empty());
    }

    #[test]
    fn colours_become_the_keyboards_hue_saturation_and_brightness() {
        assert_eq!(hsb([255, 255, 255]), (0, 0, 100));
        assert_eq!(hsb([255, 0, 0]), (0, 100, 100));
        assert_eq!(hsb([0, 255, 0]), (120, 100, 100));
        assert_eq!(hsb([0, 0, 255]), (240, 100, 100));
        assert_eq!(hsb([128, 128, 128]), (0, 0, 50));
    }

    #[test]
    fn calibration_is_not_part_of_a_profile() {
        assert!(!super::super::store::PROFILED.contains(&SECTION));
    }

    #[test]
    fn saved_calibration_survives_a_palette_change() {
        let _dir = super::super::store::test_config_dir();
        let key = "calibrate-test:0001";
        let cal = DeviceCalibration {
            name: "Test strip".into(),
            device: Calibration {
                gain: [1.0, 0.8, 0.6],
                ..Default::default()
            },
            ..Default::default()
        };
        let matched = "calibrate-test:0002";
        let with_matches = DeviceCalibration {
            name: "Test fan".into(),
            matches: vec![
                Match {
                    color: [0, 0, 153],
                    light: [0, 0, 107],
                    screen: [0, 0, 161],
                    zone: None,
                },
                Match {
                    color: [153; 3],
                    light: [150, 147, 120],
                    screen: [153; 3],
                    zone: Some("2".into()),
                },
            ],
            ..Default::default()
        };
        let mut all = Calibrations::default();
        all.devices.insert(key.into(), cal.clone());
        all.devices.insert(matched.into(), with_matches.clone());
        super::super::store::save(SECTION, &all).unwrap();
        // A new palette: the lights' colours are saved in their own section.
        let mut colours = super::super::pc_lights::Settings::default();
        colours.devices.entry(key.into()).or_default().colors =
            [("1".to_string(), "#5fc48e".to_string())].into();
        super::super::store::save("pc_lighting", &colours).unwrap();
        // Read back from the file, as the next start does (into a registry of its own, so
        // the process's, which other tests use, isn't replaced).
        let registry = calibration::Registry::default();
        load_into(&registry);
        assert_eq!(registry.device(key), Some(cal));
        // Matches, with both sides, come back the same (one test writes this section: the
        // tests share one settings file).
        assert_eq!(registry.device(matched), Some(with_matches));
        let blue = foxglove_devices::Rgb { r: 0, g: 0, b: 153 };
        assert_eq!(registry.rgb(matched, None, None, blue).b, 107);
        assert_eq!(registry.on_screen(matched, None, blue).b, 161);
        let saved: Calibrations = super::super::store::try_load(SECTION).unwrap();
        assert_eq!(saved.version, calibration::VERSION);
    }
}
