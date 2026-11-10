// SPDX-License-Identifier: MIT
//! `hideout.toml`, the hub's own preferences (contract §7.3). The shell owns `theme` and
//! `[window]`; every other key (modules, sources, trust, privacy, safe mode, written by later
//! parts of the hub) is kept as it was when the file is saved.

use crate::placement::SavedRect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follow Windows' app mode (light or dark), live.
    #[default]
    System,
    Light,
    Dark,
}

/// The main window's placement, one entry per monitor (§2.5).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPrefs {
    /// The monitor the window was last on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_monitor: Option<String>,
    /// Keyed by monitor (see `placement::Monitor::key`).
    pub monitors: BTreeMap<String, SavedRect>,
}

/// The tray panel's tile layout (Edit tiles, the ··· menu), tiles named `<module>/<panel>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayPrefs {
    /// The user's tile order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Tiles the user hid.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    /// Tiles the user showed although their app wasn't seen yet.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

impl TrayPrefs {
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.hidden.is_empty() && self.shown.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemeChoice,
    pub window: WindowPrefs,
    /// `[tray]`: the tray panel's tile layout.
    #[serde(skip_serializing_if = "TrayPrefs::is_empty")]
    pub tray: TrayPrefs,
    /// Keys the shell doesn't own, kept verbatim.
    #[serde(flatten)]
    pub rest: toml::Table,
}

impl Prefs {
    /// Parses `text`. A file that doesn't parse gives the defaults and the reason, and is not
    /// overwritten until the caller decides to (see `load`).
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Reads `path`: missing gives the defaults; unreadable or invalid gives the defaults and
    /// `Err` with the reason, so the hub can log it and keep the bad file.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::parse(&text) {
                Ok(p) => (p, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    /// Writes atomically: a temp file next to `path`, then a rename over it.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            hideout_acl::create_private_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_system_theme() {
        let p = Prefs::parse("").unwrap();
        assert_eq!(p.theme, ThemeChoice::System);
        assert!(p.window.monitors.is_empty());
    }

    #[test]
    fn unknown_keys_survive_a_save() {
        let text = "theme = \"dark\"\nautostart = true\n\n[modules.\"x.y\"]\nenabled = false\n\n[[sources]]\nid = \"main\"\n";
        let mut p = Prefs::parse(text).unwrap();
        assert_eq!(p.theme, ThemeChoice::Dark);
        p.window.last_monitor = Some("m".into());
        p.window.monitors.insert(
            "m".into(),
            SavedRect {
                x: 1,
                y: 2,
                w: 300,
                h: 200,
                scale: 1.5,
            },
        );
        let back = Prefs::parse(&p.to_toml()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.rest["autostart"].as_bool(), Some(true));
        assert!(back.rest["modules"]["x.y"]["enabled"].as_bool() == Some(false));
        assert_eq!(back.rest["sources"].as_array().map(|a| a.len()), Some(1));
    }

    #[test]
    fn the_tray_layout_round_trips_and_an_empty_one_writes_nothing() {
        let p = Prefs::default();
        assert!(!p.to_toml().contains("[tray]"));
        let mut p = Prefs::parse(
            "theme = \"dark\"
other = 1
",
        )
        .unwrap();
        p.tray.order = vec!["b.x/tile".into(), "a.x/tile".into()];
        p.tray.hidden = vec!["c.x/tile".into()];
        let text = p.to_toml();
        assert!(text.contains("[tray]"), "{text}");
        let back = Prefs::parse(&text).unwrap();
        assert_eq!(back.tray, p.tray);
        assert_eq!(back.rest["other"].as_integer(), Some(1));
    }

    #[test]
    fn a_bad_file_gives_defaults_and_the_reason() {
        let dir = std::env::temp_dir().join(format!("hideout-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hideout.toml");
        std::fs::write(&f, "theme = [").unwrap();
        let (p, err) = Prefs::load(&f);
        assert_eq!(p, Prefs::default());
        assert!(err.is_some());
        std::fs::remove_file(&f).unwrap();
        let (p, err) = Prefs::load(&f);
        assert_eq!((p, err), (Prefs::default(), None));
        let p = Prefs {
            theme: ThemeChoice::Light,
            ..Default::default()
        };
        p.save(&f).unwrap();
        assert_eq!(Prefs::load(&f).0.theme, ThemeChoice::Light);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
