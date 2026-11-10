// SPDX-License-Identifier: MIT
//! Lab mode (#1001, docs/hideout-lab.md): a first-party plugin folder under
//! `<config>\lab-modules\` loads as it ships, and nothing about trust changes.

use std::path::{Path, PathBuf};

use hideout_contract::capability::{Holder, Source};
use hideout_registry::inventory::LAB;
use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;

const ID: &str = "org.foxglove.example-lab";

/// A first-party legacy plugin as it ships (contract §4.10): `org.foxglove.*`, family `foxglove`,
/// bridged over stdio.
const MODULE_TOML: &str = r#"
id = "org.foxglove.example-lab"
name = "Example lab plugin"
version = "1.0.0"
kind = "service"
family = "foxglove"
hub = "1.0"
transport = "stdio"
api = { foxglove = "1.2" }
run = ["example.exe"]
permissions = ["devices.read", "lighting.write"]
description = "A first-party plugin for the lab tests."

[[ui.panels]]
id = "tile"
slot = "tile"
title = "Example"
icon = "timer"
"#;
const NOW: &str = "2026-11-10T12:00:00Z";

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-registry-lab-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The plugin's manifest and a stand-in exe, in `lab-modules\<folder>\`.
fn lab_folder(home: &Path, folder: &str) -> PathBuf {
    let dir = home.join(LAB_MODULES_DIR).join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("module.toml"), MODULE_TOML).unwrap();
    std::fs::write(dir.join("example.exe"), b"stand-in").unwrap();
    dir
}

#[test]
fn a_first_party_plugin_loads_as_it_ships_in_lab_mode_only() {
    let home = home("loads");
    let dir = lab_folder(&home, ID);

    // Not in lab mode: nothing from lab-modules.
    let reg = Registry::open(&home, true).unwrap();
    assert!(!reg.inventory.modules.contains_key(ID));

    let mut reg = Registry::open(&home, false).unwrap();
    reg.load_lab();
    assert_eq!(reg.lab_report.len(), 1, "{:?}", reg.lab_report);
    assert!(reg.lab_report[0].contains("loaded"), "{:?}", reg.lab_report);
    let m = &reg.inventory.modules[ID];
    assert_eq!((m.source.as_str(), m.anchor.as_deref()), (LAB, None));
    // Permissions as a sideload holds them, never as a first-party catalog module.
    assert_eq!(m.holder(), Holder::ThirdParty(Source::Sideload));
    assert!(
        m.granted.contains(&"lighting.write".to_string()),
        "{:?}",
        m.granted
    );
    assert!(reg.hub.enabled(ID));
    assert!(reg.is_lab(ID));

    let l = reg.pre_launch(ID).unwrap();
    assert_eq!(l.dir, dir);
    assert!(l.stdio, "a bridged stdio plugin");
    assert_eq!(l.run.as_deref(), Some(&["example.exe".to_string()][..]));

    // Trust is not touched: no root, no catalog, no `seen`.
    assert!(reg.trust.root.is_none() && reg.trust.catalogs.is_empty());
    assert!(reg.inventory.seen.is_empty());

    // Lab entries never reach inventory.json, even when the registry writes it.
    reg.inventory.save(&home.join("inventory.json")).unwrap();
    let on_disk = std::fs::read_to_string(home.join("inventory.json")).unwrap();
    assert!(!on_disk.contains(ID), "{on_disk}");
    // One written there by hand is dropped at open.
    let forged = on_disk.replace(
        "\"modules\": {}",
        &format!(
            "\"modules\": {{\"{ID}\": {{\"version\": \"1.0.0\", \"kind\": \"service\", \"family\": \"foxglove\", \"visibility\": \"public\", \"source\": \"lab\", \"installed\": \"\"}}}}"
        ),
    );
    assert_ne!(forged, on_disk);
    std::fs::write(home.join("inventory.json"), forged).unwrap();
    assert!(!Registry::open(&home, true)
        .unwrap()
        .inventory
        .modules
        .contains_key(ID));

    // Out of lab mode the entry can't launch.
    reg.lab = false;
    assert!(reg.pre_launch(ID).is_err());
}

#[test]
fn a_changed_file_stops_the_launch() {
    let home = home("changed");
    let dir = lab_folder(&home, ID);
    let mut reg = Registry::open(&home, false).unwrap();
    reg.load_lab();
    std::fs::write(dir.join("example.exe"), b"swapped").unwrap();
    let e = reg.pre_launch(ID).unwrap_err();
    assert_eq!(e.id(), Some("trust.files"), "{e}");
}

#[test]
fn a_folder_not_named_by_its_id_is_skipped_and_sideload_rules_stay() {
    let home = home("misnamed");
    let renamed = lab_folder(&home, "example");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.load_lab();
    assert!(!reg.inventory.modules.contains_key(ID));
    assert!(
        reg.lab_report[0].contains("must be named"),
        "{:?}",
        reg.lab_report
    );
    // Lab mode doesn't open developer-mode sideload to first-party ids (§12.6).
    let e = reg.install(&renamed, NOW).unwrap_err();
    assert_eq!(e.id(), Some("trust.sideload-scope"), "{e}");
}

#[test]
fn removing_a_lab_entry_leaves_its_folder() {
    let home = home("grants");
    lab_folder(&home, ID);
    let mut reg = Registry::open(&home, false).unwrap();
    reg.load_lab();
    assert!(!reg.inventory.modules[ID].granted.is_empty());
    // Removing the lab entry deletes nothing in lab-modules.
    reg.remove(ID).unwrap();
    assert!(home
        .join(LAB_MODULES_DIR)
        .join(ID)
        .join("module.toml")
        .exists());
}
