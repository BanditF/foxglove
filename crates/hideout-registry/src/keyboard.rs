// SPDX-License-Identifier: MIT
//! The user's grant for the adopted legacy module `foxglove` (#1155-B, contract §6.3, §6.4).
//!
//! One record per install in `inventory.json` under `modules.foxglove` (written only through
//! this crate, under `inventory.lock`): what the user allowed, the canonical full path of
//! `run[0]` and the SHA-256 of that file at the moment of **Allow**, and the time. The router
//! routes the reserved areas in [`GRANT_AREAS`] to the module only while the record is `on` and
//! the connecting instance's verified image has exactly that path and hash. A changed path or
//! hash makes the record `void`, and a void record is never revived by putting the old file back:
//! only a new Allow records it again. A decline is remembered in `hideout.toml` (a hub
//! preference, not a grant). Nothing here starts a process or reads a file but the module's own
//! program path.
//!
//! The record is never given silently: no code path other than [`Registry::allow_keyboard`]
//! (the user's Allow) writes state `on`, and [`crate::Registry::register_adopted`] keeps an
//! existing record as it is.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The only module id the grant applies to (§6.3: the legacy adopted module).
pub const LEGACY_MODULE: &str = "foxglove";

/// The reserved areas the grant lets `foxglove` provide (the plan: keyboard, lights, art and
/// screens, actions). `FIRST_PARTY_AREAS` itself does not change for adopted modules.
pub const GRANT_AREAS: &[&str] = &[
    "devices.*",
    "lighting.*",
    "art.*",
    "screens.*",
    "behaviors.*",
    "profiles.*",
    "focus.*",
];

/// Where the grant stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantState {
    /// Allowed by the user at `at`, for `path` and `sha256`.
    On,
    /// The user turned it off at `at`. Nothing is routed; the path and hash are dropped.
    Off,
    /// The program file's path or hash changed since Allow: void until the user allows again.
    Void,
    /// A state this hub does not know (a newer or edited file): never a grant.
    #[serde(other)]
    Unknown,
}

/// Why a grant went void.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoidReason {
    /// The same path holds a different file (swapped or updated).
    Changed,
    /// The module now runs from another path (moved, or another copy was adopted).
    Moved,
    #[serde(other)]
    Other,
}

/// The record in `inventory.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardGrant {
    pub state: GrantState,
    /// Canonical full path of `run[0]` at Allow (verbatim prefix stripped); empty unless `on`
    /// or `void`.
    #[serde(default)]
    pub path: String,
    /// SHA-256 of that file at Allow, lowercase hex; empty once the record is not `on`.
    #[serde(default)]
    pub sha256: String,
    /// When the state last changed (RFC 3339, UTC).
    #[serde(default)]
    pub at: String,
    /// Why it is void.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<VoidReason>,
}

impl KeyboardGrant {
    /// Whether this record lets a module whose verified image is `path` and `sha256` provide the
    /// areas: state `on`, a non-empty absolute path and a 64-digit hash, both equal to the
    /// verified ones (paths compared by [`same_path`]).
    pub fn allows(&self, path: &str, sha256: &str) -> bool {
        self.state == GrantState::On
            && is_hash(&self.sha256)
            && is_hash(sha256)
            && self.sha256.eq_ignore_ascii_case(sha256)
            && is_absolute(&self.path)
            && !is_unc(&self.path)
            && same_path(&self.path, path)
    }
}

/// A network path (`\\host\share`, or the verbatim `\\?\UNC\` form): never carries the grant,
/// because a remote file is not under this machine's write lock.
pub fn is_unc(p: &str) -> bool {
    let p = p.replace('/', "\\");
    let up = p.to_ascii_uppercase();
    up.starts_with("\\\\?\\UNC\\")
        || (p.starts_with("\\\\") && !p.starts_with("\\\\?\\") && !p.starts_with("\\\\.\\"))
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_absolute(p: &str) -> bool {
    Path::new(p).is_absolute() && !p.is_empty()
}

/// Reads the record leniently: a record that does not parse is no grant (fail closed), never an
/// unreadable inventory.
pub fn lenient<'de, D>(d: D) -> Result<Option<KeyboardGrant>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

/// The canonical full path of `path` without the `\\?\` prefix: symlinks, junctions and 8.3
/// short names resolved and the real case, as the file system names the file now.
pub fn canonical_path(path: &Path) -> io::Result<String> {
    Ok(strip_verbatim(
        &std::fs::canonicalize(path)?.to_string_lossy(),
    ))
}

/// `\\?\C:\x` becomes `C:\x`, `\\?\UNC\h\s` becomes `\\h\s`.
pub fn strip_verbatim(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

/// Whether two canonical paths name the same file as Windows sees names: either slash, case
/// folded. The hash is what binds the content; this only decides "same place".
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| strip_verbatim(p).replace('/', "\\").to_lowercase();
    !a.is_empty() && norm(a) == norm(b)
}

/// What the Foxglove page and the Permissions row show (plan §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyboardState {
    /// Not the legacy module, or not adopted: no row, no grant.
    NotApplicable,
    /// Adopted, never asked.
    NotAsked,
    /// The user answered Not now: not asked again by itself.
    Declined,
    Granted {
        at: String,
    },
    /// The user turned it off.
    Revoked {
        at: String,
    },
    /// Path or hash changed: void, asks again.
    Changed {
        at: String,
        reason: VoidReason,
    },
}

use crate::inventory::ADOPTED;
use crate::{adopted_exe, Failure, Registry};

impl Registry {
    fn legacy(&self, id: &str) -> bool {
        id == LEGACY_MODULE
            && self
                .inventory
                .modules
                .get(id)
                .is_some_and(|m| m.source == ADOPTED)
    }

    /// The canonical path of the adopted legacy module's `run[0]` now (what Allow records).
    pub fn keyboard_exe(&self, id: &str) -> Result<String, Failure> {
        if !self.legacy(id) {
            return Err(Failure::NotInstalled(id.into()));
        }
        let exe = adopted_exe(&self.manifest_checked(id)?)?;
        Ok(canonical_path(&exe)?)
    }

    /// Where the grant stands (plan §4): the record in `inventory.json`, else the decline marker
    /// in `hideout.toml`, else never asked.
    pub fn keyboard_state(&self, id: &str) -> KeyboardState {
        if !self.legacy(id) {
            return KeyboardState::NotApplicable;
        }
        let none = || {
            if self.hub.keyboard_declined(id) {
                KeyboardState::Declined
            } else {
                KeyboardState::NotAsked
            }
        };
        match self.inventory.modules[id].keyboard.as_ref() {
            None => none(),
            Some(g) => match g.state {
                GrantState::On => KeyboardState::Granted { at: g.at.clone() },
                GrantState::Off => KeyboardState::Revoked { at: g.at.clone() },
                GrantState::Void => KeyboardState::Changed {
                    at: g.at.clone(),
                    reason: g.reason.unwrap_or(VoidReason::Other),
                },
                GrantState::Unknown => none(),
            },
        }
    }

    /// Whether the hub should ask now: turned on, never asked or asks again, and not declined.
    pub fn keyboard_needs_ok(&self, id: &str) -> bool {
        self.hub.enabled(id)
            && !self.hub.keyboard_declined(id)
            && matches!(
                self.keyboard_state(id),
                KeyboardState::NotAsked | KeyboardState::Changed { .. }
            )
    }

    /// The user's **Allow**: records `path` (canonical) and `sha256` of the exact file the caller
    /// pinned. Refused unless `path` is where `run[0]` points now. The only code that writes
    /// state `on`. Clears a decline.
    pub fn allow_keyboard(
        &mut self,
        id: &str,
        path: &str,
        sha256: &str,
        now: &str,
    ) -> Result<KeyboardGrant, Failure> {
        let here = self.keyboard_exe(id)?;
        if !same_path(&here, path) || !is_hash(sha256) || is_unc(&here) {
            return Err(Failure::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the program is not where the grant was asked for",
            )));
        }
        let grant = KeyboardGrant {
            state: GrantState::On,
            path: here,
            sha256: sha256.to_ascii_lowercase(),
            at: now.into(),
            reason: None,
        };
        self.write_keyboard(id, Some(grant.clone()))?;
        self.change_hub(|h| h.set_keyboard_declined(id, false))?;
        Ok(grant)
    }

    /// **Turn off**: the record becomes `off`, the path and hash are dropped.
    pub fn revoke_keyboard(&mut self, id: &str, now: &str) -> Result<(), Failure> {
        if !self.legacy(id) {
            return Err(Failure::NotInstalled(id.into()));
        }
        let g = KeyboardGrant {
            state: GrantState::Off,
            path: String::new(),
            sha256: String::new(),
            at: now.into(),
            reason: None,
        };
        self.write_keyboard(id, Some(g))
    }

    /// The program's path or hash changed: `on` becomes `void` (the path is kept, the hash cleared, for the
    /// record only; [`KeyboardGrant::allows`] refuses anything but `on`). Does nothing from any
    /// other state, so it can never turn a grant on.
    pub fn void_keyboard(
        &mut self,
        id: &str,
        reason: VoidReason,
        now: &str,
    ) -> Result<(), Failure> {
        if !self.legacy(id) {
            return Err(Failure::NotInstalled(id.into()));
        }
        let path = self.inventory_path();
        let mut changed = None;
        crate::inventory::Inventory::update(&path, |inv| {
            if let Some(m) = inv.modules.get_mut(id) {
                if let Some(g) = m.keyboard.as_mut().filter(|g| g.state == GrantState::On) {
                    g.state = GrantState::Void;
                    // What was granted is not kept: nothing can match it again.
                    g.sha256.clear();
                    g.reason = Some(reason);
                    g.at = now.into();
                    changed = Some(g.clone());
                }
            }
        })?;
        if let Some(g) = changed {
            if let Some(m) = self.inventory.modules.get_mut(id) {
                m.keyboard = Some(g);
            }
        }
        Ok(())
    }

    /// Saves **Not now**: a decline marker in `hideout.toml`. A record, if any, is untouched.
    pub fn decline_keyboard(&mut self, id: &str) -> Result<(), Failure> {
        if !self.legacy(id) {
            return Err(Failure::NotInstalled(id.into()));
        }
        self.change_hub(|h| h.set_keyboard_declined(id, true))
    }

    fn inventory_path(&self) -> std::path::PathBuf {
        self.home.join("inventory.json")
    }

    /// Replaces the record under `inventory.lock` (a read-modify-write of the file as it is now)
    /// and mirrors it in memory.
    fn write_keyboard(&mut self, id: &str, g: Option<KeyboardGrant>) -> Result<(), Failure> {
        let path = self.inventory_path();
        let inv = crate::inventory::Inventory::update(&path, |inv| {
            if let Some(m) = inv.modules.get_mut(id) {
                m.keyboard = g.clone();
            }
        })?;
        if !inv.modules.contains_key(id) {
            return Err(Failure::NotInstalled(id.into()));
        }
        if let Some(m) = self.inventory.modules.get_mut(id) {
            m.keyboard = g;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    fn on(path: &str) -> KeyboardGrant {
        KeyboardGrant {
            state: GrantState::On,
            path: path.into(),
            sha256: H.into(),
            at: "2026-11-12T10:00:00Z".into(),
            reason: None,
        }
    }

    #[test]
    fn allows_only_the_exact_path_and_hash() {
        let g = on(r"C:\Apps\Foxglove\foxglove.exe");
        assert!(g.allows(r"C:\Apps\Foxglove\foxglove.exe", H));
        // Slash and case do not matter for the place; the hash is exact.
        assert!(g.allows(r"c:/apps/foxglove/FOXGLOVE.EXE", &H.to_uppercase()));
        assert!(g.allows(r"\\?\C:\Apps\Foxglove\foxglove.exe", H));
        assert!(!g.allows(r"C:\Apps\Other\foxglove.exe", H));
        assert!(!g.allows(r"C:\Apps\Foxglove\foxglove.exe", &"0".repeat(64)));
        assert!(!g.allows(r"C:\Apps\Foxglove\foxglove.exe", "abc"));
        assert!(!g.allows("", H));
    }

    #[test]
    fn anything_but_on_with_a_full_record_allows_nothing() {
        for state in [GrantState::Off, GrantState::Void, GrantState::Unknown] {
            let g = KeyboardGrant {
                state,
                ..on(r"C:\a\foxglove.exe")
            };
            assert!(!g.allows(r"C:\a\foxglove.exe", H), "{state:?}");
        }
        // A relative path or a short hash in an `on` record (a hand edit) allows nothing.
        assert!(!on("foxglove.exe").allows("foxglove.exe", H));
        let mut short = on(r"C:\a\foxglove.exe");
        short.sha256 = "ab".into();
        assert!(!short.allows(r"C:\a\foxglove.exe", "ab"));
    }

    #[test]
    fn a_record_with_an_unknown_state_reads_and_grants_nothing() {
        let g: KeyboardGrant =
            serde_json::from_str(r#"{"state":"maybe","path":"C:\\a","sha256":"x"}"#).unwrap();
        assert_eq!(g.state, GrantState::Unknown);
    }

    #[test]
    fn verbatim_prefixes_come_off() {
        assert_eq!(strip_verbatim(r"\\?\C:\x\y.exe"), r"C:\x\y.exe");
        assert_eq!(strip_verbatim(r"\\?\UNC\h\s\y.exe"), r"\\h\s\y.exe");
        assert_eq!(strip_verbatim(r"C:\x"), r"C:\x");
    }
}
