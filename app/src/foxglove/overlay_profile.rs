//! Profiles for the overlay's look and behaviour (docs/profiles.md, the Planner's default:
//! profiles may override it). KeyPeek's `settings.ini` stays the base and keeps its format;
//! a profile stores only the INI values it changes, in the store's profiled `[overlay]`.
//!
//! - Effective overlay settings = `settings.ini` with the active profile's values on top.
//! - Saving while a profile is active writes the differences into that profile and leaves
//!   `settings.ini` as it is; with no profile, `settings.ini` as before.
//! - The display (`monitor`) belongs to the machine and is never overridden.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// Keys a profile never overrides.
const MACHINE: &[&str] = &["settings.monitor"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Overrides {
    /// `section.key` -> the INI value.
    values: BTreeMap<String, String>,
}

fn temp_ini() -> std::path::PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "foxglove-overlay-{}-{}.ini",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A Settings value as `section.key` -> INI value (through KeyPeek's own writer).
fn pairs(s: &Settings) -> BTreeMap<String, String> {
    let path = temp_ini();
    let mut out = BTreeMap::new();
    if s.save_to_file(&path).is_ok() {
        if let Ok(ini) = ini::Ini::load_from_file(&path) {
            for (section, props) in ini.iter() {
                for (k, v) in props.iter() {
                    out.insert(format!("{}.{k}", section.unwrap_or("")), v.to_string());
                }
            }
        }
    }
    let _ = std::fs::remove_file(path);
    out
}

/// Settings from `section.key` pairs (through KeyPeek's own reader).
fn from_pairs(values: &BTreeMap<String, String>) -> Option<Settings> {
    let mut ini = ini::Ini::new();
    for (key, v) in values {
        let (section, k) = key.split_once('.')?;
        ini.with_section(Some(section)).set(k, v.as_str());
    }
    let path = temp_ini();
    ini.write_to_file(&path).ok()?;
    let s = Settings::load_from_file(&path);
    let _ = std::fs::remove_file(path);
    s
}

/// The base settings with the active profile's overlay values on top.
pub fn effective(base: Settings) -> Settings {
    let over: Overrides = super::store::try_load("overlay").unwrap_or_default();
    if over.values.is_empty() {
        return base;
    }
    let mut values = pairs(&base);
    for (k, v) in over.values {
        if !MACHINE.contains(&k.as_str()) {
            values.insert(k, v);
        }
    }
    from_pairs(&values).unwrap_or(base)
}

/// Saves the overlay settings: into the active profile as differences from `settings.ini`,
/// or into `settings.ini` when no profile is active.
pub fn save(active: &Settings) -> std::io::Result<()> {
    if super::store::cached_profiles().1.is_none() {
        return active.save();
    }
    // The display belongs to the machine, not the profile: it goes to settings.ini (where
    // `effective` reads it) even with a profile active, so a chosen or dragged-to display stays.
    let mut base_settings = Settings::load().unwrap_or_default();
    if base_settings.monitor != active.monitor {
        base_settings.monitor = active.monitor.clone();
        base_settings.save()?;
    }
    let base = pairs(&base_settings);
    let values = pairs(active)
        .into_iter()
        .filter(|(k, v)| !MACHINE.contains(&k.as_str()) && base.get(k) != Some(v))
        .collect();
    super::store::save("overlay", &Overrides { values })
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
static WATCH: OnceLock<()> = OnceLock::new();

/// Call every frame: true once after each profile switch (the overlay then reloads its
/// settings). The first call starts listening and asks for a repaint on each switch.
pub fn switched(ctx: &egui::Context, seen: &mut u64) -> bool {
    WATCH.get_or_init(|| {
        let changes = super::devices::bus().facts().subscribe("profile.active");
        let ctx = ctx.clone();
        super::services::spawn_named("overlay-profile", move || {
            for _ in changes {
                GENERATION.fetch_add(1, Ordering::Relaxed);
                ctx.request_repaint();
            }
        });
    });
    let now = GENERATION.load(Ordering::Relaxed);
    let changed = now != *seen;
    *seen = now;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_pairs() {
        let s = Settings {
            size: 77,
            timeout: 1234,
            ..Settings::default()
        };
        let p = pairs(&s);
        assert_eq!(p.get("settings.size").map(String::as_str), Some("77"));
        assert!(from_pairs(&p) == Some(s));
    }

    #[test]
    fn a_profile_carries_the_overlay_opacity() {
        let s = Settings {
            opacity: 45,
            ..Settings::default()
        };
        let p = pairs(&s);
        assert_eq!(p.get("settings.opacity").map(String::as_str), Some("45"));
        assert!(from_pairs(&p).is_some_and(|back| back.opacity == 45));
        // Zero is allowed (hidden); out of range in a hand-edited file is clamped.
        let mut low = p.clone();
        low.insert("settings.opacity".into(), "0".into());
        assert_eq!(from_pairs(&low).map(|s| s.opacity), Some(0));
        low.insert("settings.opacity".into(), "250".into());
        assert_eq!(from_pairs(&low).map(|s| s.opacity), Some(100));
        // Missing (an older settings.ini): the look it always had.
        let mut old = p;
        old.remove("settings.opacity");
        assert_eq!(from_pairs(&old).map(|s| s.opacity), Some(100));
    }

    #[test]
    fn a_fresh_config_starts_the_overlay_at_the_detent() {
        let fresh = Settings::default();
        assert_eq!(fresh.opacity, 20);
        // The key labels follow it.
        assert!(fresh.label_opacity.is_none());
        // Saved, it keeps 20 rather than reading back as an older file.
        assert!(from_pairs(&pairs(&fresh)).is_some_and(|back| back.opacity == 20));
    }

    #[test]
    fn a_profile_carries_the_label_opacity_or_following() {
        let own = Settings {
            label_opacity: Some(80),
            ..Settings::default()
        };
        let p = pairs(&own);
        assert_eq!(
            p.get("settings.label_opacity").map(String::as_str),
            Some("80")
        );
        assert!(from_pairs(&p).is_some_and(|back| back.label_opacity == Some(80)));
        // Following is written out too, so a profile that follows overrides a base that doesn't.
        let follows = pairs(&Settings::default());
        assert_eq!(
            follows.get("settings.label_opacity").map(String::as_str),
            Some("follow")
        );
        let mut values = p;
        values.insert("settings.label_opacity".into(), "follow".into());
        assert!(from_pairs(&values).is_some_and(|back| back.label_opacity.is_none()));
        // Missing (an older settings.ini): follows.
        values.remove("settings.label_opacity");
        assert!(from_pairs(&values).is_some_and(|back| back.label_opacity.is_none()));
    }

    #[test]
    fn machine_keys_are_never_overridden() {
        assert!(MACHINE.contains(&"settings.monitor"));
    }
}
