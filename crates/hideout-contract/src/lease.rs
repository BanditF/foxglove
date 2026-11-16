// SPDX-License-Identifier: MIT
//! Leases a provider hands out through the hub (§8.5): the id rule (v1.14) and `lease.closed`.
//!
//! A lease id is a JSON string of 1 to [`MAX_ID_BYTES`] bytes with no control characters. The
//! hub records the caller whose call's result carried it as the lease's holder, and forwards a
//! routed call naming a lease only for that holder ([`error_id::LEASE_UNKNOWN`] otherwise).

use serde::{Deserialize, Serialize};

/// The notification that tells a provider a lease it handed out ended (H → M, §8.8, v1.3).
pub const LEASE_CLOSED: &str = "lease.closed";

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
}
