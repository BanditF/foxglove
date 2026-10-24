//! Curated underglow presets (#265, docs/lighting-presets.md): a colour plus an effect that match
//! a Bandit wallpaper family, and matching right-screen art (#273).
//!
//! Applying one sets the lights on the keyboard and, while "Change the right screen with the
//! preset" is on, hands the art to the art service (`preset_art`). That's layer art's priority
//! rule: the user's pinned art comes back when the preset is cleared. The choice is saved in the
//! store's `[lighting]` (accent.rs), so a profile can pick its own preset; a profile switch
//! re-applies it. Event-driven: profile switches and keyboard arrivals, never a timer.

use std::sync::{Mutex, OnceLock};

use foxglove_devices::{Device as _, DeviceEvent, Effect};
use serde::Deserialize;

use super::accent::LightingSettings;
use super::art_cycle::Item;

const PRESETS_JSON: &str = include_str!("../../resources/lighting-presets.json");

/// The bundled right-screen sets the presets point at (`art.file`).
const ART_FILES: &[(&str, &[u8])] = &[
    ("fury.bin", include_bytes!("../../resources/lighting/art/fury.bin")),
    ("constellations.bin", include_bytes!("../../resources/lighting/art/constellations.bin")),
    ("hearth.bin", include_bytes!("../../resources/lighting/art/hearth.bin")),
    ("moon-garden.bin", include_bytes!("../../resources/lighting/art/moon-garden.bin")),
];

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Hsb {
    pub hue: u16,
    pub saturation: u8,
    pub brightness: u8,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Art {
    pub builtin: Option<String>,
    pub file: Option<String>,
    pub fallback: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub color: Hsb,
    pub effect: String,
    /// The colour on screen, `#rrggbb`.
    pub swatch: String,
    #[serde(default)]
    pub art: Art,
}

#[derive(Deserialize)]
struct File {
    format: String,
    version: u32,
    effects: Vec<String>,
    presets: Vec<Preset>,
}

fn parse(text: &str) -> Option<(Vec<String>, Vec<Preset>)> {
    let f: File = serde_json::from_str(text).ok()?;
    (f.format == "foxglove-lighting-presets" && f.version == 1).then_some((f.effects, f.presets))
}

fn data() -> &'static (Vec<String>, Vec<Preset>) {
    static DATA: OnceLock<(Vec<String>, Vec<Preset>)> = OnceLock::new();
    DATA.get_or_init(|| parse(PRESETS_JSON).unwrap_or_default())
}

pub fn presets() -> &'static [Preset] {
    &data().1
}

pub fn find(id: &str) -> Option<&'static Preset> {
    presets().iter().find(|p| p.id == id)
}

/// The keyboard's effect number for a preset's effect name ("0" solid … "3" swirl).
fn effect_id(name: &str) -> String {
    data().0.iter().position(|e| e == name).unwrap_or(0).to_string()
}

/// The swatch as an egui colour.
pub fn swatch(p: &Preset) -> egui::Color32 {
    let hex = p.swatch.trim_start_matches('#');
    let byte = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(0);
    egui::Color32::from_rgb(byte(0), byte(2), byte(4))
}

/// The preset's right-screen art as a hotlist item: a built-in by name, or its bundled set
/// copied into the art library (once), falling back to `fallback` if the copy fails.
pub fn art_item(p: &Preset) -> Option<Item> {
    if let Some(name) = &p.art.builtin {
        return Some(Item::BuiltIn(name.clone()));
    }
    let file = p.art.file.as_deref()?;
    let copied = (|| {
        let (_, bytes) = ART_FILES.iter().find(|(f, _)| *f == file)?;
        let folder = super::art_library::folder()?;
        std::fs::create_dir_all(&folder).ok()?;
        let path = folder.join(file);
        if std::fs::read(&path).ok().as_deref() != Some(*bytes) {
            std::fs::write(&path, bytes).ok()?;
            super::art_library::refresh();
        }
        Some(Item::File(path))
    })();
    copied.or_else(|| p.art.fallback.clone().map(Item::BuiltIn))
}

/// The preset last put on the keyboard this session, so a repeat event doesn't re-send it.
static APPLIED: Mutex<Option<String>> = Mutex::new(None);

/// Picks a preset (or clears it with `None`), saves the choice in `[lighting]` and applies it.
/// Turns "Follow the Windows accent colour" off: the two would fight over the hue.
pub fn choose(id: Option<&str>) {
    let mut settings = LightingSettings::load();
    settings.preset = id.map(str::to_owned);
    if id.is_some() {
        settings.follow_accent = false;
        super::accent::turn_off();
    }
    settings.save();
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    apply_current();
}

/// Turns "Change the right screen with the preset" on or off, and updates the art now.
pub fn set_preset_art(on: bool) {
    let mut settings = LightingSettings::load();
    settings.preset_art = on;
    settings.save();
    send_art(&settings);
}

fn send_art(settings: &LightingSettings) {
    let art = settings
        .preset
        .as_deref()
        .and_then(find)
        .filter(|_| settings.preset_art)
        .and_then(art_item);
    if let Some(h) = super::art_service::handle() {
        h.preset_art(art);
    }
}

/// Applies the effective `[lighting]` preset to the keyboard (lights and art), off the UI thread.
pub fn apply_current() {
    let _ = std::thread::Builder::new().name("foxglove-preset".into()).spawn(|| {
        let settings = LightingSettings::load();
        send_art(&settings);
        let Some(preset) = settings.preset.as_deref().and_then(find) else {
            *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return;
        };
        {
            let applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
            if applied.as_deref() == Some(preset.id.as_str()) {
                return;
            }
        }
        let Some(d) = super::devices::device() else { return };
        let Some(lighting) = d.lighting() else { return };
        let c = preset.color;
        let result = lighting
            .set_color(c.hue, c.saturation, c.brightness.max(1))
            .and_then(|()| lighting.set_effect(Effect { id: effect_id(&preset.effect), params: vec![] }));
        match result {
            Ok(()) => {
                *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = Some(preset.id.clone());
                super::keyboard_settings::refresh_rgb_state();
            }
            Err(e) => eprintln!("Foxglove preset {}: {}", preset.id, super::devices::describe(&e)),
        }
    });
}

/// Re-applies on a profile switch (a profile can pick its own preset) and when a keyboard
/// arrives. Both are events; nothing runs in between.
pub fn start() {
    if super::repaint_trace::disabled("presets") {
        return;
    }
    let profile = super::devices::bus().facts().subscribe("profile.active");
    super::services::spawn_named("preset-profile", move || {
        for _ in profile {
            *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = None;
            apply_current();
        }
    });
    let events = super::devices::bus().subscribe(false);
    super::services::spawn_named("preset-arrivals", move || {
        for e in events {
            match e.event {
                DeviceEvent::Connected => apply_current(),
                // The keyboard forgets nothing we sent, but a new connection re-sends once.
                DeviceEvent::Disconnected { .. } => *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = None,
                _ => {}
            }
        }
    });
    apply_current();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_presets_parse_and_point_at_real_art() {
        let all = presets();
        assert_eq!(all.len(), 7);
        for p in all {
            assert!(data().0.contains(&p.effect), "{} has a known effect", p.id);
            match (&p.art.builtin, &p.art.file) {
                (Some(_), None) => {}
                (None, Some(f)) => {
                    assert!(ART_FILES.iter().any(|(name, _)| name == f), "{} ships {f}", p.id);
                    assert!(p.art.fallback.is_some(), "{} has a fallback built-in", p.id);
                }
                other => panic!("{}: art must be a built-in or a file, got {other:?}", p.id),
            }
        }
        assert_eq!(effect_id("breathe"), "1");
        assert_eq!(swatch(find("fury").unwrap()), egui::Color32::from_rgb(0xc8, 0x34, 0x2b));
    }

    #[test]
    fn settings_default_to_changing_the_art() {
        let s: LightingSettings = toml::from_str("follow_accent = true").unwrap();
        assert!(s.preset_art, "on by default, the user's pick for #273");
        assert_eq!(s.preset, None);
        let p: LightingSettings = toml::from_str("preset = \"fury\"\npreset_art = false").unwrap();
        assert_eq!((p.preset.as_deref(), p.preset_art), (Some("fury"), false));
    }
}
