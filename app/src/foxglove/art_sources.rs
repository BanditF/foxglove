//! The art library's own settings (#59), in the store's `[art_library]`:
//! - **source folders:** Flipper-style frame folders (e.g. `art-src\bandit\out`) the library lists
//!   and converts on demand (`art_encode`);
//! - **stand-ins:** a saved slot that stands in for a built-in, so a hotlist, layer or preset
//!   entry naming that built-in shows the slot instead. It's app-side only; the firmware keeps
//!   its built-ins, and undoing it is one click.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandIn {
    /// The built-in's manifest name.
    pub builtin: String,
    pub slot: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stand_ins: Vec<StandIn>,
}

static CACHE: Mutex<Option<LibrarySettings>> = Mutex::new(None);

pub fn load() -> LibrarySettings {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .get_or_insert_with(|| super::store::load_or_migrate("art_library", || None))
        .clone()
}

pub fn save(settings: LibrarySettings) {
    if let Err(e) = super::store::save("art_library", &settings) {
        eprintln!("Foxglove: can't save the art library settings: {e}");
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings);
}

/// The slot standing in for this built-in, if any.
pub fn stand_in(builtin: &str) -> Option<u8> {
    load()
        .stand_ins
        .iter()
        .find(|s| s.builtin == builtin)
        .map(|s| s.slot)
}

/// Makes `slot` stand in for `builtin` (`None` undoes it).
pub fn set_stand_in(builtin: &str, slot: Option<u8>) {
    let mut s = load();
    s.stand_ins.retain(|i| i.builtin != builtin);
    if let Some(slot) = slot {
        s.stand_ins.push(StandIn {
            builtin: builtin.to_owned(),
            slot,
        });
    }
    save(s);
}

pub fn add_source(folder: PathBuf) {
    let mut s = load();
    if !s.sources.contains(&folder) {
        s.sources.push(folder);
        save(s);
    }
}

pub fn remove_source(folder: &PathBuf) {
    let mut s = load();
    s.sources.retain(|f| f != folder);
    save(s);
}

/// The set folders under a source: the folder itself if it is one, else its sub-folders that are.
pub fn sets_in(source: &std::path::Path) -> Vec<PathBuf> {
    if super::art_encode::is_set(source) {
        return vec![source.to_path_buf()];
    }
    let mut sets: Vec<PathBuf> = std::fs::read_dir(source)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| super::art_encode::is_set(p))
        .collect();
    sets.sort();
    sets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let s = LibrarySettings {
            sources: vec![PathBuf::from("art-src")],
            stand_ins: vec![StandIn {
                builtin: "Bandit_Gem".into(),
                slot: 2,
            }],
        };
        assert_eq!(
            toml::from_str::<LibrarySettings>(&toml::to_string(&s).unwrap()).unwrap(),
            s
        );
        assert_eq!(toml::to_string(&LibrarySettings::default()).unwrap(), "");
    }

    #[test]
    fn finds_set_folders() {
        let dir = std::env::temp_dir().join(format!("foxglove-sources-{}", std::process::id()));
        let set = dir.join("Bandit_x");
        std::fs::create_dir_all(&set).unwrap();
        std::fs::write(set.join("meta.txt"), "Width: 68\n").unwrap();
        std::fs::create_dir_all(dir.join("not-a-set")).unwrap();
        assert_eq!(sets_in(&dir), vec![set.clone()]);
        assert_eq!(sets_in(&set), vec![set.clone()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
