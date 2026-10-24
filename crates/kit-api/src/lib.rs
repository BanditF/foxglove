//! Foxglove's local API, which Kits (plugins) and outside tools speak. See docs/local-api.md.
//!
//! - [`rpc`]: JSON-RPC 2.0 messages, one per line
//! - [`methods`]: every method and event with its params and result
//! - [`permission`]: what a client may touch, and how patterns match
//! - [`manifest`]: a Kit's `kit.toml`
//!
//! The types here are the contract: the checked-in `schema/v1.json` is generated from them, and
//! a test fails when they change without the schema (and, for breaking changes, the version).

pub mod manifest;
pub mod methods;
pub mod permission;
pub mod rpc;

/// The API version this crate describes. Minor versions only add; a major version breaks.
pub const API_VERSION: Version = Version { major: 1, minor: 1 };

/// An API version as `major.minor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let (major, minor) = text.split_once('.')?;
        Some(Version { major: major.parse().ok()?, minor: minor.parse().ok()? })
    }

    /// Whether a client built for `self` can talk to an app speaking `app`: same major, and the
    /// app at least as new (minor versions only add).
    pub fn served_by(self, app: Version) -> bool {
        self.major == app.major && self.minor <= app.minor
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The JSON Schema of every message, as checked in at `schema/v1.json`.
pub fn schema() -> serde_json::Value {
    methods::schema()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("1.0").served_by(v("1.3")));
        assert!(!v("1.4").served_by(v("1.3")));
        assert!(!v("2.0").served_by(v("1.9")));
        assert_eq!(Version::parse("1"), None);
        assert_eq!(API_VERSION.to_string(), "1.1");
    }

    /// `schema/v1.json` must match the types. After an intended change, regenerate it with
    /// `UPDATE_SCHEMA=1 cargo test -p foxglove-kit-api` and bump the version as the change needs.
    #[test]
    fn schema_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/v1.json");
        let generated = serde_json::to_string_pretty(&schema()).unwrap() + "\n";
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        let checked_in = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(
            checked_in == generated,
            "schema/v1.json is out of date: run UPDATE_SCHEMA=1 cargo test -p foxglove-kit-api"
        );
    }
}
