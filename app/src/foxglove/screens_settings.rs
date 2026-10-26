//! Settings for the keyboard's screens: the `[screens]` section of the settings store
//! (`foxglove.toml`). The first time it's missing, they're imported from the old `screens.ini`,
//! or else from the standalone hostpanel's `config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ini::Ini;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreensSettings {
    /// Draw the left screen's host panel (only on keyboards with Foxglove's host link).
    pub drive_left: bool,
    /// Panel redraws per second, 1-10 (the media title scrolls at this rate).
    pub fps: u32,
    /// Lower-case exe name -> hint shown on the panel while that program is in front.
    pub games: BTreeMap<String, String>,
    /// What the panel shows: the clock, media and hints, or the wallpaper's own animation.
    pub layout: Layout,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Info,
    /// The wallpaper's left frames (art/wallpapers.json): Wallpaper Engine's, or the Windows
    /// desktop wallpaper's without it (#446); Info when it has none.
    Wallpaper,
}

impl Default for ScreensSettings {
    fn default() -> Self {
        Self {
            drive_left: true,
            fps: 4,
            games: BTreeMap::new(),
            layout: Layout::Info,
        }
    }
}

impl ScreensSettings {
    /// The old `screens.ini`, read once when moving into the store.
    fn ini_path() -> Option<PathBuf> {
        crate::settings::Settings::config_file_path().map(|p| p.with_file_name("screens.ini"))
    }

    /// The file to watch for changes: the settings store.
    pub fn path() -> Option<PathBuf> {
        super::store::path()
    }

    fn normalized(mut self) -> Self {
        self.fps = self.fps.clamp(1, 10);
        self.games = self
            .games
            .into_iter()
            .map(|(exe, hint)| (exe.to_lowercase(), hint))
            .collect();
        self
    }

    /// The saved settings; on first run the ones imported from `screens.ini` or hostpanel.
    pub fn load() -> Self {
        super::store::load_or_migrate("screens", || {
            if let Some(ini) = Self::ini_path().filter(|p| p.exists()) {
                return Self::load_from_file(&ini).map(|s| (s, vec![ini]));
            }
            hostpanel_config_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|text| Self::from_hostpanel(&text))
                .map(|s| (s, vec![]))
        })
        .normalized()
    }

    /// The settings as saved now, or `None` while the store can't be read (keep the current ones).
    pub fn try_load() -> Option<Self> {
        super::store::try_load::<Self>("screens").map(Self::normalized)
    }

    pub fn save(&self) -> std::io::Result<()> {
        super::store::save("screens", self)
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
            ..defaults
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
    fn reads_the_old_ini_and_round_trips_through_toml() {
        let path =
            std::env::temp_dir().join(format!("foxglove-screens-{}.ini", std::process::id()));
        std::fs::write(
            &path,
            "[screens]
drive_left=false
fps=7
[games]
Factorio.exe=L2 = BUILD
",
        )
        .unwrap();
        let s = ScreensSettings::load_from_file(&path).unwrap();
        assert_eq!((s.drive_left, s.fps), (false, 7));
        assert_eq!(
            s.games.get("factorio.exe").map(String::as_str),
            Some("L2 = BUILD")
        );
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<ScreensSettings>(&text).unwrap(), s);
        let partial: ScreensSettings = toml::from_str("fps = 30").unwrap();
        assert_eq!(partial.normalized().fps, 10);
        let _ = std::fs::remove_file(path);
    }
}
