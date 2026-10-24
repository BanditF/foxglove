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
    /// Things a key can trigger (#212); the user binds them to keys in the Plugins tab.
    #[serde(default)]
    pub actions: Vec<ActionDecl>,
    /// Secrets the user enters in the Plugins tab (#213); the plugin reads them with
    /// `secrets.get`. Needs the `secrets` permission.
    #[serde(default)]
    pub secrets: Vec<SecretDecl>,
    /// Options the user sets in the Plugins tab (#69, API 1.3); the plugin reads them with
    /// `settings.get` and hears of changes as `settings.changed`.
    #[serde(default)]
    pub settings: Vec<SettingDecl>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    Bool,
    Int,
    Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingDecl {
    /// What the plugin reads (`focus-minutes`).
    pub name: String,
    /// For the Plugins tab (`Focus minutes`).
    pub label: String,
    pub kind: SettingKind,
    /// The value until the user changes it; its type must match `kind`.
    pub default: serde_json::Value,
    /// For `int`: the range the Plugins tab allows.
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    /// For `text`: Foxglove fills it with a random value the first time (a pairing token the
    /// user copies into another app), and the user can still change it.
    #[serde(default)]
    pub random: bool,
}

impl SettingDecl {
    /// `value` as this setting takes it (an int clamped to its range), or `None` if it's the
    /// wrong type.
    pub fn accept(&self, value: &serde_json::Value) -> Option<serde_json::Value> {
        match (self.kind, value) {
            (SettingKind::Bool, serde_json::Value::Bool(_)) | (SettingKind::Text, serde_json::Value::String(_)) => Some(value.clone()),
            (SettingKind::Int, v) => {
                let n = v.as_i64()?;
                Some(n.clamp(self.min.unwrap_or(i64::MIN), self.max.unwrap_or(i64::MAX)).into())
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretDecl {
    /// What `secrets.get` asks for (`obs-password`).
    pub name: String,
    /// For the Plugins tab (`OBS WebSocket password`).
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDecl {
    /// Stable: bindings and `action.invoked` use it (`toggle-mute`).
    pub id: String,
    /// For the Plugins tab (`Toggle mic mute`).
    pub name: String,
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
        let mut ids = std::collections::HashSet::new();
        for a in &m.actions {
            let ok = !a.id.is_empty() && a.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !ok || !ids.insert(a.id.as_str()) || a.name.trim().is_empty() {
                return Err(format!("action {:?} needs a unique id (a-z, 0-9, -) and a name", a.id));
            }
        }
        let mut names = std::collections::HashSet::new();
        for s in &m.settings {
            let ok = !s.name.is_empty() && s.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            let random_ok = !s.random || s.kind == SettingKind::Text;
            if !ok || !random_ok || !names.insert(s.name.as_str()) || s.label.trim().is_empty() || s.accept(&s.default).is_none() {
                return Err(format!("setting {:?} needs a unique name (a-z, 0-9, -), a label and a default of its kind", s.name));
            }
        }
        if !m.secrets.is_empty() && !m.permissions.contains(&Permission::Secrets) {
            return Err("[[secrets]] needs the secrets permission".into());
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
    fn declared_secrets_need_the_permission() {
        let decl = "[[secrets]]
name = \"obs-password\"
label = \"OBS password\"
";
        assert!(Manifest::parse(&plugin(decl), APP).is_err());
        let with = plugin(&format!("permissions = [\"secrets\"]
{decl}"));
        assert_eq!(Manifest::parse(&with, APP).unwrap().secrets[0].name, "obs-password");
    }

    #[test]
    fn actions_need_unique_ids_and_names() {
        let ok = plugin("[[actions]]
id = \"toggle-mute\"
name = \"Toggle mute\"
");
        assert_eq!(Manifest::parse(&ok, APP).unwrap().actions[0].id, "toggle-mute");
        let twice = plugin("[[actions]]
id = \"a\"
name = \"A\"
[[actions]]
id = \"a\"
name = \"B\"
");
        assert!(Manifest::parse(&twice, APP).is_err());
        assert!(Manifest::parse(&plugin("[[actions]]
id = \"Bad Id\"
name = \"x\"
"), APP).is_err());
        assert!(Manifest::parse(&plugin("[[actions]]
id = \"a\"
name = \"x\"
key = \"F13\"
"), APP).is_err());
    }

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
