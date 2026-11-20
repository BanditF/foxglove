// SPDX-License-Identifier: MIT
//! Leases a provider hands out through the hub (§8.5): the id rule (v1.14) and `lease.closed`.
//!
//! A lease id is a JSON string of 1 to [`MAX_ID_BYTES`] bytes with no control characters. The
//! hub records the caller whose call's result carried it as the lease's holder, and forwards a
//! routed call naming a lease only for that holder ([`error_id::LEASE_UNKNOWN`] otherwise).

use serde::{Deserialize, Serialize};

/// The notification that tells a provider a lease it handed out ended (H → M, §8.8, v1.3).
pub const LEASE_CLOSED: &str = "lease.closed";

/// (v1.17) A provider ends every live lease it handed out of one family (M → H, §8.8):
/// `{family}` → `{ended}`; each closes with `lease.closed {reason: "revoked"}`, as the hub's
/// own Resume does. The holders aren't told; a later call naming one answers `lease.unknown`.
pub const LEASE_END: &str = "lease.end";

/// (v1.21) A provider tells the hub the names of the callers it holds leases for outside the
/// hub, so the hub's `lease_holders` fact lists them with the hub's own (M -> H, §8.8):
/// `{family, names}` -> `{}`. The latest call replaces the module's earlier list for that
/// family; an empty list removes it; it ends with the module's connection.
pub const LEASE_HOLDERS: &str = "lease.holders";

/// The longest lease id, in bytes (v1.14).
pub const MAX_ID_BYTES: usize = 128;

/// Whether `id` is a lease id (§8.5, v1.14): 1 to [`MAX_ID_BYTES`] bytes, no control characters.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_BYTES && !id.chars().any(char::is_control)
}

/// `lease.closed` (§8.8): which lease ended and why. `reason` is one of [`reason`]; a provider
/// MUST treat a reason it doesn't know as ended (v1.14), so it is kept as a string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LeaseClosed {
    pub lease: String,
    pub reason: String,
}

/// `lease.end` (§8.8, v1.17): the family of the provider's own leases to end, as in its
/// `<family>.release`. Answered `{ended}`, the number that ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseEnd {
    pub family: String,
}

/// `lease.holders` (§8.8, v1.21): the family and the names of the provider's own callers (the
/// ones that don't come through the hub). The hub cleans every name like a holder name (hidden
/// characters dropped, at most [`HOLDER_NAME_MAX`] characters), keeps at most
/// [`HOLDERS_MAX_NAMES`] and lists each once. A name is the caller's own word, never checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseHolders {
    pub family: String,
    pub names: Vec<String>,
}

/// Why a lease ended (§8.5).
pub mod reason {
    /// The holder's connection ended (v1.3).
    pub const DISCONNECT: &str = "disconnect";
    /// The holder gave it back with the provider's `<family>.release` (v1.3).
    pub const RELEASED: &str = "released";
    /// The hub ended it: the holder's or the route's permission was withdrawn (v1.3).
    pub const REVOKED: &str = "revoked";
    /// (v1.14) The provider's connection ended while the lease was live; sent to the module's
    /// next connection at its `module.ready`.
    pub const RESTART: &str = "restart";
    /// Every reason this contract names.
    pub const ALL: &[&str] = &[DISCONNECT, RELEASED, REVOKED, RESTART];
}

/// Live leases one caller may hold at one provider (§8.5, v1.15, #1120). The caller is the
/// router's caller key (a module's id, a tool's name), so a reconnect buys no fresh room; a
/// result over it is not passed on ([`error_id::LEASE_LIMIT`]) and the provider hears
/// `lease.closed {reason: "revoked"}` for it.
pub const MAX_PER_CALLER: usize = 16;

/// (v1.17) The hub fact `module.<provider>.<HOLDERS_FACT>`: who holds live leases at that
/// provider, `{"<family>": ["<name>", ...]}` with the family of the handing-out method
/// ([`family`]) and each holder by the name people know it by (a module's display
/// name, a tool's `initialize` name; hidden characters dropped, at most [`HOLDER_NAME_MAX`]
/// characters), sorted, once each. Absent while nothing is held. Never lease ids or reasons.
pub const HOLDERS_FACT: &str = "lease_holders";

/// (v1.17) The longest holder name in [`HOLDERS_FACT`], in characters; a longer one is cut.
pub const HOLDER_NAME_MAX: usize = 32;

/// (v1.21) The most names one `lease.holders` call keeps; the rest are dropped.
pub const HOLDERS_MAX_NAMES: usize = 16;

/// (v1.21) The most families one module may list with `lease.holders`, and the longest family
/// name, in bytes.
pub const HOLDERS_MAX_FAMILIES: usize = 8;
pub const HOLDERS_FAMILY_MAX: usize = 64;

/// The fact key of [`HOLDERS_FACT`] for `provider`.
pub fn holders_key(provider: &str) -> String {
    format!("module.{provider}.{HOLDERS_FACT}")
}

/// The family of a lease-handing method, the part before its last `.`, as in the family's
/// `<family>.release` (`wallpaper-engine.pause` → `wallpaper-engine`).
pub fn family(method: &str) -> &str {
    method.rsplit_once('.').map_or(method, |(f, _)| f)
}

/// The `data.id`s of a lease refusal (§15, v1.14).
pub mod error_id {
    /// A routed call names a lease the calling connection doesn't hold at that provider
    /// (−32602; not forwarded).
    pub const LEASE_UNKNOWN: &str = "lease.unknown";
    /// A provider's result names a lease another connection holds (error 6).
    pub const LEASE_REUSED: &str = "lease.reused";
    /// (v1.15) A provider's result would give a caller more than
    /// [`super::MAX_PER_CALLER`] live leases at that provider (error 8).
    pub const LEASE_LIMIT: &str = "lease.limit";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_rule() {
        for ok in [
            "L1",
            "a",
            "we-0123456789abcdef",
            "é",
            &"x".repeat(MAX_ID_BYTES),
        ] {
            assert!(valid_id(ok), "{ok:?}");
        }
        for bad in [
            "",
            "a\nb",
            "\u{0}",
            "tab\there",
            &"x".repeat(MAX_ID_BYTES + 1),
        ] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn lease_closed_reads_any_reason() {
        let v = serde_json::json!({"lease": "L1", "reason": "later"});
        let c: LeaseClosed = serde_json::from_value(v).unwrap();
        assert_eq!(c.reason, "later");
        assert!(reason::ALL.contains(&reason::RESTART));
    }

    #[test]
    fn a_family_is_the_method_without_its_last_part() {
        assert_eq!(family("wallpaper-engine.pause"), "wallpaper-engine");
        assert_eq!(family("example.lessor.lease"), "example.lessor");
        assert_eq!(family("odd"), "odd");
        assert_eq!(holders_key("foxglove"), "module.foxglove.lease_holders");
    }
}
