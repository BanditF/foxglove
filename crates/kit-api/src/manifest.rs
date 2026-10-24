//! A Kit's `kit.toml`: who it is, which API it was built for, how to start it, what it may do.
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Reverse-DNS, unique: also the Kit's folder name.
    pub id: String,
    pub name: String,
    /// The Kit's own semver.
    pub version: String,
    /// The API version it was built for (`major.minor`).
    pub api: String,
    /// The program and its arguments, relative to the Kit's folder.
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
            return Err(format!("id {:?} must be reverse-DNS, like com.example.kit", m.id));
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
            return Err(format!("run {program:?} must stay inside the Kit's folder"));
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

    fn kit(extra: &str) -> String {
        format!(
            "id = \"com.example.weather\"\nname = \"Weather\"\nversion = \"1.0.0\"\napi = \"1.0\"\nrun = [\"weather.exe\"]\n{extra}"
        )
    }

    #[test]
    fn parses_and_checks() {
        let m = Manifest::parse(&kit("permissions = [\"facts.write:weather.*\", \"net\"]\nprovides = [\"facts\"]"), APP).unwrap();
        assert_eq!(m.permissions, vec![Permission::FactsWrite("weather.*".into()), Permission::Net]);
        assert_eq!(m.provides, vec![Provides::Facts]);
        assert!(Manifest::parse(&kit("permissions = [\"root\"]"), APP).is_err());
        assert!(Manifest::parse(&kit("surprise = 1"), APP).is_err());
        assert!(Manifest::parse(&kit("").replace("1.0\"\nrun", "2.0\"\nrun"), APP).is_err());
        assert!(Manifest::parse(&kit("").replace("weather.exe", "../evil.exe"), APP).is_err());
        assert!(Manifest::parse(&kit("").replace("com.example.weather", "weather"), APP).is_err());
    }

    #[test]
    fn update_lists_added_permissions() {
        let old = Manifest::parse(&kit("permissions = [\"net\"]"), APP).unwrap();
        let new = Manifest::parse(&kit("permissions = [\"net\", \"lighting.write\"]"), APP).unwrap();
        assert_eq!(new.added_permissions(&old), vec![Permission::LightingWrite]);
    }
}
