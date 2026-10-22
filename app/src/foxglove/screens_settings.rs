//! Settings for the keyboard's screens, in `screens.ini` next to `settings.ini`. A file of its
//! own because `Settings::save_to_file` (upstream) rewrites `settings.ini` from scratch.
//!
//! On first run they're imported from the standalone hostpanel's `config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ini::Ini;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq)]
pub struct ScreensSettings {
    /// Draw the left screen's host panel (only on keyboards with Foxglove's host link).
    pub drive_left: bool,
    /// Panel redraws per second, 1-10 (the media title scrolls at this rate).
    pub fps: u32,
    /// Lower-case exe name -> hint shown on the panel while that program is in front.
    pub games: BTreeMap<String, String>,
}

impl Default for ScreensSettings {
    fn default() -> Self {
        Self {
            drive_left: true,
            fps: 4,
            games: BTreeMap::new(),
        }
    }
}

impl ScreensSettings {
    pub fn path() -> Option<PathBuf> {
        crate::settings::Settings::config_file_path().map(|p| p.with_file_name("screens.ini"))
    }

    /// The saved settings; on first run the ones imported from hostpanel, else the defaults.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        if let Some(s) = Self::load_from_file(&path) {
            return s;
        }
        let imported = hostpanel_config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| Self::from_hostpanel(&text));
        let s = imported.unwrap_or_default();
        let _ = s.save_to_file(&path);
        s
    }

    pub fn load_from_file(path: impl AsRef<Path>) -> Option<Self> {
        let conf = Ini::load_from_file(path).ok()?;
        let mut s = Self::default();
        if let Some(section) = conf.section(Some("screens")) {
            if let Some(v) = section.get("drive_left") {
                s.drive_left = v.parse().unwrap_or(s.drive_left);
            }
            if let Some(v) = section.get("fps") {
                s.fps = v.parse::<u32>().unwrap_or(s.fps).clamp(1, 10);
            }
        }
        if let Some(section) = conf.section(Some("games")) {
            s.games = section
                .iter()
                .map(|(exe, hint)| (exe.to_lowercase(), hint.to_string()))
                .collect();
        }
        Some(s)
    }

    pub fn save_to_file(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        if let Some(dir) = path.as_ref().parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut conf = Ini::new();
        conf.with_section(Some("screens"))
            .set("drive_left", self.drive_left.to_string())
            .set("fps", self.fps.to_string());
        let mut games = conf.with_section(Some("games"));
        for (exe, hint) in &self.games {
            games.set(exe.as_str(), hint.as_str());
        }
        conf.write_to_file(path)
    }

    /// The panel settings from hostpanel's `config.toml` (fps and game hints).
    fn from_hostpanel(text: &str) -> Option<Self> {
        #[derive(Deserialize)]
        struct HostpanelConfig {
            fps: Option<u32>,
            #[serde(default)]
            games: BTreeMap<String, String>,
        }
        let c: HostpanelConfig = toml::from_str(text).ok()?;
        let defaults = Self::default();
        Some(Self {
            drive_left: true,
            fps: c.fps.unwrap_or(defaults.fps).clamp(1, 10),
            games: c
                .games
                .into_iter()
                .map(|(exe, hint)| (exe.to_lowercase(), hint))
                .collect(),
        })
    }
}

/// `%APPDATA%\hostpanel\config.toml`.
fn hostpanel_config_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("hostpanel").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_hostpanel_config() {
        let s = ScreensSettings::from_hostpanel(
            "# hostpanel settings\nfps = 6\n\n[games]\n\"Valorant.exe\" = \"GAME: WASD\"\n",
        )
        .unwrap();
        assert_eq!(s.fps, 6);
        assert!(s.drive_left);
        assert_eq!(
            s.games.get("valorant.exe").map(String::as_str),
            Some("GAME: WASD")
        );

        // hostpanel's commented default has neither.
        let s = ScreensSettings::from_hostpanel("# product = \"Eyelash\"\n").unwrap();
        assert_eq!(s, ScreensSettings::default());
    }

    #[test]
    fn round_trips_through_the_file() {
        let path =
            std::env::temp_dir().join(format!("foxglove-screens-{}.ini", std::process::id()));
        let mut s = ScreensSettings {
            drive_left: false,
            fps: 7,
            ..Default::default()
        };
        s.games.insert("factorio.exe".into(), "L2 = BUILD".into());
        s.save_to_file(&path).unwrap();
        assert_eq!(ScreensSettings::load_from_file(&path), Some(s));
        let _ = std::fs::remove_file(path);
    }
}
