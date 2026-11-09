//! Hideout's own files (spec §2.3) and its preferences, `hideout.toml` (§7.3), written only by
//! the hub. `HIDEOUT_CONFIG_DIR` (tests, the Test lab) or `HIDEOUT_HOME` replaces the folder.

use std::path::{Path, PathBuf};

use hub_shell::placement::Saved;
use serde::{Deserialize, Serialize};

/// The hub's folder: `HIDEOUT_CONFIG_DIR`, else `HIDEOUT_HOME`, else `%LOCALAPPDATA%\Hideout`.
/// `None` when none is known (no LOCALAPPDATA): the hub then runs without saving.
pub fn dir() -> Option<PathBuf> {
    dir_from(|name| std::env::var_os(name))
}

/// The folder given by an override, for the single-instance names: `None` for the user's own.
pub fn override_dir() -> Option<std::ffi::OsString> {
    ["HIDEOUT_CONFIG_DIR", "HIDEOUT_HOME"]
        .into_iter()
        .find_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()))
}

fn dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    set("HIDEOUT_CONFIG_DIR")
        .or_else(|| set("HIDEOUT_HOME"))
        .or_else(|| set("LOCALAPPDATA").map(|d| d.join("Hideout")))
}

/// Which theme the hub uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

/// `hideout.toml`: the hub's own preferences.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemeChoice,
    /// Where the window was last (display and place in its points).
    pub window: Saved,
}

const FILE: &str = "hideout.toml";

impl Prefs {
    /// Reads `hideout.toml` from `dir`; a missing or unreadable file is the defaults.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes `hideout.toml` into `dir`, making the folder when needed.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(FILE), text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn the_config_dir_override_wins_then_home_then_local_app_data() {
        let all = env(&[
            ("HIDEOUT_CONFIG_DIR", r"C:\lab\cfg"),
            ("HIDEOUT_HOME", r"C:\lab\home"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ]);
        assert_eq!(dir_from(all), Some(PathBuf::from(r"C:\lab\cfg")));
        let home = env(&[
            ("HIDEOUT_CONFIG_DIR", ""),
            ("HIDEOUT_HOME", r"C:\lab\home"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ]);
        assert_eq!(dir_from(home), Some(PathBuf::from(r"C:\lab\home")));
        let user = env(&[("LOCALAPPDATA", r"C:\Users\u\AppData\Local")]);
        assert_eq!(
            dir_from(user),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local").join("Hideout"))
        );
        assert_eq!(dir_from(env(&[])), None);
    }

    #[test]
    fn prefs_round_trip_and_a_missing_file_is_the_defaults() {
        let dir = std::env::temp_dir().join(format!("hideout-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Prefs::load(&dir), Prefs::default());
        let prefs = Prefs {
            theme: ThemeChoice::Light,
            window: Saved {
                monitor: r"\\.\DISPLAY2".into(),
                place: Some([10.0, 20.0, 880.0, 600.0]),
                rect: None,
            },
        };
        prefs.save(&dir).unwrap();
        assert_eq!(Prefs::load(&dir), prefs);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
