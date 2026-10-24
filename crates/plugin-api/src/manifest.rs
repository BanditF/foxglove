//! A Plugin's `plugin.toml`: who it is, which API it was built for, how to start it, what it may do.
//!
//! ```toml
//! id = "com.example.weather"
//! name = "Weather"
//! version = "1.2.0"
//! api = "1.0"
//! run = ["weather.exe"]
//! permissions = ["facts.write:weather.*", "net"]
//! provides = ["facts"]
//! description = "Puts the temperature on the left screen."
//! ```

use serde::{Deserialize, Serialize};

use crate::permission::Permission;
use crate::Version;

/// A plugin's manifest file, at the top of its folder.
pub const FILE: &str = "plugin.toml";
/// The manifest's name before #87, still read.
pub const LEGACY_FILES: &[&str] = &["kit.toml"];

/// The manifest text in `dir`: `plugin.toml`, or `kit.toml` from before the rename.
pub fn read_in(dir: &std::path::Path) -> std::io::Result<String> {
    let mut last = std::io::Error::from(std::io::ErrorKind::NotFound);
    for name in std::iter::once(FILE).chain(LEGACY_FILES.iter().copied()) {
        match std::fs::read_to_string(dir.join(name)) {
            Ok(text) => return Ok(text),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Reverse-DNS, unique: also the plugin's folder name.
    pub id: String,
    pub name: String,
    /// The plugin's own semver.
    pub version: String,
    /// The API version it was built for (`major.minor`).
    pub api: String,
    /// The program and its arguments, relative to the plugin's folder.
    pub run: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub provides: Vec<Provides>,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provides {
    Facts,
    PanelLayout,
    LightingEffect,
    Action,
    Backend,
}

impl Manifest {
    /// Parses and checks a manifest against the API this app speaks.
    pub fn parse(text: &str, app: Version) -> Result<Manifest, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        let id_ok = m.id.split('.').count() >= 2
            && m.id
                .split('.')
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        if !id_ok {
            return Err(format!("id {:?} must be reverse-DNS, like com.example.plugin", m.id));
        }
        let api = Version::parse(&m.api).ok_or_else(|| format!("api {:?} must be major.minor", m.api))?;
        if !api.served_by(app) {
            return Err(format!("built for API {api}; this Foxglove speaks {app}"));
        }
        let program = m.run.first().ok_or("run must name the program")?;
        let escapes = std::path::Path::new(program)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
        if escapes {
            return Err(format!("run {program:?} must stay inside the plugin's folder"));
        }
        Ok(m)
    }

    /// Permissions in `self` that `previous` didn't have: an update needs the user's OK for these.
    pub fn added_permissions(&self, previous: &Manifest) -> Vec<Permission> {
        self.permissions.iter().filter(|p| !previous.permissions.contains(p)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: Version = Version { major: 1, minor: 0 };

    #[test]
    fn reads_plugin_toml_or_the_old_kit_toml() {
        let dir = std::env::temp_dir().join(format!("plugin-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_in(&dir).is_err());
        std::fs::write(dir.join("kit.toml"), "old").unwrap();
        assert_eq!(read_in(&dir).unwrap(), "old");
        std::fs::write(dir.join("plugin.toml"), "new").unwrap();
        assert_eq!(read_in(&dir).unwrap(), "new");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn plugin(extra: &str) -> String {
        format!(
            "id = \"com.example.weather\"\nname = \"Weather\"\nversion = \"1.0.0\"\napi = \"1.0\"\nrun = [\"weather.exe\"]\n{extra}"
        )
    }

    #[test]
    fn parses_and_checks() {
        let m = Manifest::parse(&plugin("permissions = [\"facts.write:weather.*\", \"net\"]\nprovides = [\"facts\"]"), APP).unwrap();
        assert_eq!(m.permissions, vec![Permission::FactsWrite("weather.*".into()), Permission::Net]);
        assert_eq!(m.provides, vec![Provides::Facts]);
        assert!(Manifest::parse(&plugin("permissions = [\"root\"]"), APP).is_err());
        assert!(Manifest::parse(&plugin("surprise = 1"), APP).is_err());
        assert!(Manifest::parse(&plugin("").replace("1.0\"\nrun", "2.0\"\nrun"), APP).is_err());
        assert!(Manifest::parse(&plugin("").replace("weather.exe", "../evil.exe"), APP).is_err());
        assert!(Manifest::parse(&plugin("").replace("com.example.weather", "weather"), APP).is_err());
    }

    #[test]
    fn update_lists_added_permissions() {
        let old = Manifest::parse(&plugin("permissions = [\"net\"]"), APP).unwrap();
        let new = Manifest::parse(&plugin("permissions = [\"net\", \"lighting.write\"]"), APP).unwrap();
        assert_eq!(new.added_permissions(&old), vec![Permission::LightingWrite]);
    }
}
