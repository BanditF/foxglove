//! The Hideout module contract (docs/hideout-contract.md): types, parsing and validation only.
//! No hub and no transport live here.
//!
//! - [`module`]: the `module.toml` manifest (§4) and reading a module folder (§4.10 precedence)
//! - [`legacy`]: today's `plugin.toml`/`kit.toml` (§4.10), as `foxglove-plugin-api` always read it
//! - [`permission`]: Foxglove API v1 permissions, kept by the contract (§6.1)
//! - [`capability`]: the full §6.1 grammar and the never-list (§6.6)
//! - [`panel`]: the panel schema, `ui.register` trees and their caps (§5)
//! - [`frame`]: direct-channel stream frames (§5.4, §8.4)
//! - [`messages`]: registration and channel handshake messages (§8.8)
//! - [`config`]: `hideout.toml` sources and trust anchors (§13.4, §13.6)
//! - [`error`]: the §15 error ids
//!
//! `foxglove-plugin-api` re-exports this crate, so plugins built against it are unchanged.

pub mod capability;
pub mod config;
pub mod error;
pub mod frame;
pub mod legacy;
pub mod messages;
pub mod module;
pub mod panel;
pub mod permission;

pub use error::{Error, ErrorId};

/// The contract version this crate reads (`hub` in a manifest, §8.7).
pub const HUB_VERSION: Version = Version { major: 1, minor: 0 };

/// Crash-restart delays in seconds (§7.7, spec v1.1): the last step repeats.
pub const BACKOFF_STEPS_S: &[u64] = &[1, 2, 4, 8, 16, 32, 60];

/// The scope a legacy `foxglove-plugin-root` signs for (§12.3, spec v1.1).
pub const LEGACY_ROOT_SCOPE: &[&str] = &["foxglove.*", "org.foxglove.*"];

/// The family a user-anchored catalog has when it names none (§13.6, spec v1.1).
pub const DEFAULT_USER_CATALOG_FAMILY: &str = "user";

/// A version as `major.minor` (API versions, and the contract's `hub`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let (major, minor) = text.split_once('.')?;
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(major) || !digits(minor) {
            return None;
        }
        Some(Version {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
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

/// Whether `id` matches a scope `pattern`: `a.*` covers the children of `a` (`a.b`, `a.b.c`),
/// anything else only itself.
pub fn scope_covers(pattern: &str, id: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => {
            id.len() > prefix.len() + 1
                && id.starts_with(prefix)
                && id.as_bytes()[prefix.len()] == b'.'
        }
        None => pattern == id,
    }
}

/// Whether two scope patterns can both cover one id.
pub fn scopes_overlap(a: &str, b: &str) -> bool {
    let base = |p: &str| p.strip_suffix(".*").map(str::to_string);
    match (base(a), base(b)) {
        (None, None) => a == b,
        (Some(_), None) => scope_covers(a, b),
        (None, Some(_)) => scope_covers(b, a),
        (Some(x), Some(y)) => {
            x == y || x.starts_with(&format!("{y}.")) || y.starts_with(&format!("{x}."))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes() {
        assert!(scope_covers("foxglove.*", "foxglove.art"));
        assert!(!scope_covers("foxglove.*", "foxglove"));
        assert!(!scope_covers("foxglove.*", "foxglovex.art"));
        assert!(scopes_overlap("hideout.*", "hideout.example-lights"));
        assert!(scopes_overlap("foxglove.*", "foxglove.a.*"));
        assert!(!scopes_overlap("example.private.*", "foxglove.*"));
        assert!(!scopes_overlap("vulpes", "vulpes.*"));
    }

    #[test]
    fn versions() {
        assert_eq!(Version::parse("1.0"), Some(HUB_VERSION));
        assert_eq!(Version::parse("1.0.0"), None);
        assert_eq!(Version::parse("+1.0"), None);
        assert_eq!(Version::parse("1"), None);
    }
}
