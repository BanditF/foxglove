// SPDX-License-Identifier: MIT
//! What is installed, read cheaply for the hub's change watch (contract §7.1: the hub notices
//! `inventory.json` changes by a file-change notification, never by polling; #1012.4): the ids
//! only, nothing hashed or checked. The hub compares them with the last ids it saw; an id that
//! is gone was removed.

use std::collections::BTreeSet;
use std::path::Path;

use crate::inventory::{Inventory, LAB};
use crate::lab::{lab_mode_from_env, LabMode};

/// The ids installed in `home` now: `inventory.json`'s modules (a lab entry written there by
/// hand left out, as [`crate::Registry::open`] does) and, in lab mode, the folder names under
/// `lab-modules\`. `None` when either can't be read just now, so a removal is never guessed
/// from a failed read. A missing `inventory.json` is nothing installed.
pub fn installed_ids(home: &Path) -> Option<BTreeSet<String>> {
    let inv = Inventory::load(&home.join("inventory.json")).ok()?;
    let mut ids: BTreeSet<String> = inv
        .modules
        .iter()
        .filter(|(_, m)| m.source != LAB)
        .map(|(id, _)| id.clone())
        .collect();
    if lab_mode_from_env(home) == LabMode::On {
        let base = LabMode::modules_dir(home);
        match std::fs::read_dir(&base) {
            Ok(rd) => {
                for e in rd {
                    let e = e.ok()?;
                    if e.file_type().ok()?.is_dir() {
                        ids.insert(e.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    Some(ids)
}

/// The ids of `before` that `now` no longer has, in order.
pub fn removed(before: &BTreeSet<String>, now: &BTreeSet<String>) -> Vec<String> {
    before.difference(now).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_ids_that_left_are_removed() {
        let before = set(&["a.art", "b.beat", "c.chat"]);
        assert_eq!(
            removed(&before, &set(&["a.art", "c.chat", "d.new"])),
            ["b.beat"]
        );
        assert!(removed(&before, &before).is_empty());
        assert_eq!(removed(&before, &set(&[])).len(), 3);
    }

    #[test]
    fn the_inventory_ids_are_read_without_a_registry() {
        let dir = std::env::temp_dir().join(format!("hideout-installed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No inventory yet: nothing installed.
        assert_eq!(installed_ids(&dir), Some(BTreeSet::new()));
        let text = r#"{"format": "hideout-inventory-v1", "modules": {
            "a.art": {"version": "1.0.0", "kind": "service", "family": "foxglove", "visibility": "public", "source": "s", "channel": "stable",
                      "installed": "2026-11-10T00:00:00Z", "files": {}},
            "x.lab": {"version": "1.0.0", "kind": "service", "family": "foxglove", "visibility": "public", "source": "lab", "channel": "lab",
                      "installed": "", "files": {}}}}"#;
        std::fs::write(dir.join("inventory.json"), text).unwrap();
        let ids = installed_ids(&dir);
        assert_eq!(
            ids,
            Some(set(&["a.art"])),
            "a lab entry on disk doesn't count"
        );
        // A half-written or foreign file reads as "can't tell", never as everything removed.
        std::fs::write(dir.join("inventory.json"), "{not json").unwrap();
        assert_eq!(installed_ids(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
