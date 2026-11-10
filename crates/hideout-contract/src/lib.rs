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

/// The JSON Schema of the contract's wire types, as checked in at `schema/hideout-v1.json`
/// (the repository's `schema/` folder): module declarations, registration and channel messages,
/// the panel tree, and `hideout.toml`. Generated from the types, so it cannot drift from them.
pub fn schema() -> serde_json::Value {
    let mut types = serde_json::Map::new();
    macro_rules! add {
        ($($module:ident :: $name:ident),* $(,)?) => {$(
            let mut schema = serde_json::to_value(schemars::schema_for!($module::$name)).unwrap();
            if let Some(object) = schema.as_object_mut() {
                object.remove("$schema");
            }
            types.insert(format!("{}.{}", stringify!($module), stringify!($name)), schema);
        )*};
    }
    add!(
        legacy::Manifest,
        module::PanelDecl,
        module::WindowDecl,
        module::ModuleSetting,
        module::ModuleAction,
        module::McpTool,
        module::Pin,
        module::Identity,
        module::Edition,
        panel::Node,
        panel::Panel,
        panel::Register,
        panel::Rejected,
        messages::InitializeModule,
        messages::InitializeTool,
        messages::ModuleRegister,
        messages::RegisterResult,
        messages::ModuleReady,
        messages::ModuleDrain,
        messages::DrainResult,
        messages::UiPatch,
        messages::UiEvent,
        messages::UiAssetPut,
        messages::UiWindowOpen,
        messages::UiBadge,
        messages::UiVisibility,
        messages::ActionInvoke,
        messages::SettingsSet,
        messages::HubNotify,
        messages::ChannelOpen,
        messages::ChannelOpened,
        messages::ChannelCreate,
        messages::ModuleIdParams,
        config::Config,
    );
    serde_json::json!({
        "hub": HUB_VERSION.to_string(),
        "types": types,
    })
}

/// Bracket nesting a TOML file may have (inline arrays and tables), the same cap as a wire message.
pub const MAX_TOML_NESTING: usize = panel::MAX_MESSAGE_NESTING;

/// Refuses a TOML text nested deeper than [`MAX_TOML_NESTING`] before the (recursive) TOML parser
/// sees it. Counted without recursion; strings and comments are skipped.
pub(crate) fn check_toml_nesting(text: &str) -> Result<(), error::Error> {
    let b = text.as_bytes();
    let (mut i, mut depth) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            q @ (b'"' | b'\'') => {
                let multi = b[i..].starts_with(&[q, q, q]);
                i += if multi { 3 } else { 1 };
                while i < b.len() {
                    if q == b'"' && b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if multi && b[i..].starts_with(&[q, q, q]) {
                        i += 2;
                        break;
                    }
                    if !multi && (b[i] == q || b[i] == b'\n') {
                        break;
                    }
                    i += 1;
                }
            }
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_TOML_NESTING {
                    return Err(error::Error::new(
                        error::ErrorId::ManifestSyntax,
                        format!("file nests deeper than {MAX_TOML_NESTING}"),
                    ));
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `schema/hideout-v1.json` must match the types. After an intended change, regenerate it with
    /// `UPDATE_SCHEMA=1 cargo test -p hideout-contract` and bump the version as the change needs.
    #[test]
    fn schema_is_current() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/hideout-v1.json");
        let generated = serde_json::to_string_pretty(&schema()).unwrap() + "\n";
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &generated).unwrap();
        }
        let checked_in = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            checked_in == generated,
            "schema/hideout-v1.json is out of date: run UPDATE_SCHEMA=1 cargo test -p hideout-contract"
        );
    }

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
