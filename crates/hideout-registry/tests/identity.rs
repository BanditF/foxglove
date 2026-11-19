// SPDX-License-Identifier: MIT
//! The dormant `Identity` record (trust model S1): old inventory files load unchanged, the new
//! field round-trips, and fields or classes from a newer version never break loading.

use hideout_registry::inventory::{Identity, IdentityClass, Inventory};

/// An inventory as written before `identity` existed, byte for byte.
const OLD: &str = r#"{
  "format": "hideout-inventory-v1",
  "modules": {
    "foxglove": {
      "version": "0.1.0",
      "kind": "adopted",
      "family": "foxglove",
      "visibility": "public",
      "source": "adopted",
      "channel": "",
      "installed": "2026-11-01T10:00:00Z",
      "files": {},
      "footprint": [],
      "granted": ["hub.notify"],
      "pending_permissions": [],
      "bound": {
        "manifest_sha256": "aa",
        "exe_path": "C:\\Foxglove\\foxglove.exe",
        "exe_sha256": "bb",
        "list_hash": "cc",
        "at": "2026-11-01T10:00:00Z",
        "by": "dialog"
      }
    }
  },
  "seen": {}
}"#;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-identity-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("inventory.json")
}

#[test]
fn old_file_loads_with_no_identity_and_saves_without_one() {
    let path = temp("old");
    std::fs::write(&path, OLD).unwrap();
    let inv = Inventory::load(&path).unwrap();
    let m = &inv.modules["foxglove"];
    assert_eq!(m.identity, None);
    assert_eq!(m.granted, vec!["hub.notify"]);
    inv.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("identity"), "a None identity is not written");
    assert_eq!(Inventory::load(&path).unwrap(), inv);
}

#[test]
fn identity_round_trips_for_every_class() {
    let classes = [
        IdentityClass::Adopted,
        IdentityClass::DevFolder {
            folder: Some(r"C:\dev\mod".into()),
        },
        IdentityClass::Sideload {
            folder: Some(r"C:\src\mod".into()),
        },
        IdentityClass::Catalog {
            anchor: Some("root".into()),
            key_fpr: Some("ab12".into()),
            family: Some("foxglove".into()),
        },
        IdentityClass::InstalledSigned {
            key_fpr: Some("cd34".into()),
            folder: Some(r"C:\Foxglove".into()),
        },
    ];
    let path = temp("rt");
    for class in classes {
        let mut inv: Inventory = serde_json::from_str(OLD).unwrap();
        inv.modules.get_mut("foxglove").unwrap().identity = Some(Identity {
            class,
            kind: None,
            visibility: None,
            established_at: Some("2026-11-19T09:00:00Z".into()),
            established_by: Some("dialog".into()),
            sensitive_confirmed: vec!["keyboard.write".into()],
        });
        inv.save(&path).unwrap();
        assert_eq!(Inventory::load(&path).unwrap(), inv);
    }
}

#[test]
fn unknown_fields_and_classes_do_not_break_loading() {
    let with_future = OLD
        .replace("\"seen\": {}", "\"seen\": {}, \"future_top\": 1")
        .replace(
            "\"bound\": {",
            "\"identity\": {\"class\": \"catalog\", \"anchor\": \"a\", \"key_fpr\": \"k\", \
         \"family\": \"vulpes\", \"added_later\": [1, 2]}, \"future_field\": true, \"bound\": {",
        );
    let inv: Inventory = serde_json::from_str(&with_future).unwrap();
    let id = inv.modules["foxglove"].identity.clone().unwrap();
    assert_eq!(
        id.class,
        IdentityClass::Catalog {
            anchor: Some("a".into()),
            key_fpr: Some("k".into()),
            family: Some("vulpes".into()),
        }
    );

    let unknown_class = OLD.replace(
        "\"bound\": {",
        "\"identity\": {\"class\": \"quantum-signed\", \"x\": 1}, \"bound\": {",
    );
    let inv: Inventory = serde_json::from_str(&unknown_class).unwrap();
    assert_eq!(
        inv.modules["foxglove"].identity.as_ref().unwrap().class,
        IdentityClass::Unknown
    );
}
