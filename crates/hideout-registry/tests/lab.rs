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

#[test]
fn lab_withdrawn_json_withdraws_a_lab_module_in_lab_mode_only() {
    // #1026: the lab's stand-in for a catalog revocation.
    let home = home("withdrawn");
    lab_folder(&home, ID);
    std::fs::write(
        home.join(hideout_registry::lab::LAB_WITHDRAWN_FILE),
        format!(r#"{{"{ID}": {{"version": "1.0.0", "reason": "Please update."}}}}"#),
    )
    .unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    assert_eq!(reg.revocation_of(ID), None, "not a lab hub: no lab module");
    reg.load_lab();
    let r = reg.revocation_of(ID).expect("withdrawn in the lab");
    assert_eq!(
        (r.version.as_str(), r.reason.as_str()),
        ("1.0.0", "Please update.")
    );
    assert!(
        reg.lab_report
            .iter()
            .any(|l| l.contains("reads as withdrawn")),
        "{:?}",
        reg.lab_report
    );
    // Another version named: not this one.
    std::fs::write(
        home.join(hideout_registry::lab::LAB_WITHDRAWN_FILE),
        format!(r#"{{"{ID}": {{"version": "0.9.0"}}}}"#),
    )
    .unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    reg.load_lab();
    assert_eq!(reg.revocation_of(ID), None);
    assert_eq!(
        reg.go_back_version(ID),
        None,
        "a lab module never rolls back"
    );
    let _ = std::fs::remove_dir_all(home);
}

/// A manifest for a newer hub, with a key this hub doesn't know (#1096).
const NEWER_TOML: &str = r#"
id = "org.foxglove.example-lab"
name = "Example lab plugin"
version = "2.0.0"
kind = "service"
family = "foxglove"
hub = "1.9"
run = ["example.exe"]
permissions = ["devices.read"]
a_key_from_hub_1_9 = true
"#;

/// #1096: a lab module whose manifest asks for a newer hub is listed from its head only (id,
/// name, version), with nothing granted, and never passes the launch check: the supervisor
/// reads it as needs-hub (§7.8). Not dropped at load.
#[test]
fn a_newer_hub_lab_module_is_listed_from_its_head_and_never_launches() {
    let home = home("needs-hub");
    let dir = lab_folder(&home, ID);
    std::fs::write(dir.join("module.toml"), NEWER_TOML).unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    reg.load_lab();
    assert_eq!(reg.lab_report.len(), 1, "{:?}", reg.lab_report);
    assert!(
        reg.lab_report[0].contains("needs hub 1.9") && reg.lab_report[0].contains("not started"),
        "{:?}",
        reg.lab_report
    );
    let m = &reg.inventory.modules[ID];
    assert_eq!((m.source.as_str(), m.version.as_str()), (LAB, "2.0.0"));
    assert!(m.granted.is_empty() && m.pending_permissions.is_empty());
    assert!(reg.is_lab(ID) && reg.hub.enabled(ID));
    assert!(reg.manifest(ID).is_none());
    let head = reg.manifest_head(ID).expect("its head");
    assert_eq!(
        (head.name.as_str(), head.version.as_str()),
        ("Example lab plugin", "2.0.0")
    );
    assert_eq!(reg.module_name(ID), "Example lab plugin");
    // What the supervisor reads: manifest.hub, at hub start and at a launch.
    let e = reg.manifest_checked(ID).unwrap_err();
    assert_eq!(e.id(), Some("manifest.hub"), "{e}");
    let e = reg.pre_launch(ID).unwrap_err();
    assert_eq!(e.id(), Some("manifest.hub"), "{e}");

    // An older major isn't "needs a newer hub": still not loaded, as before.
    std::fs::write(
        dir.join("module.toml"),
        NEWER_TOML.replace("\"1.9\"", "\"0.9\""),
    )
    .unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    reg.load_lab();
    assert!(!reg.inventory.modules.contains_key(ID));
    assert!(
        reg.lab_report[0].contains("not loaded"),
        "{:?}",
        reg.lab_report
    );
}

/// #1096, the normal path: an installed (sideloaded or catalog) version whose manifest asks for
/// a newer hub (the hub went back a version) stays in the inventory and is named from its head;
/// installing one is still refused at step 6 (`manifest.hub`, §7.4), keeping what runs.
#[test]
fn an_installed_newer_hub_manifest_is_named_from_its_head_and_its_install_refused() {
    let home = home("needs-hub-installed");
    let src = home.join("src").join("kiln");
    std::fs::create_dir_all(&src).unwrap();
    let served = NEWER_TOML
        .replace(ID, "org.example.kiln")
        .replace("\"foxglove\"", "\"third-party\"")
        .replace("\"1.9\"", "\"1.0\"")
        .replace("a_key_from_hub_1_9 = true\n", "");
    std::fs::write(src.join("module.toml"), &served).unwrap();
    std::fs::write(src.join("example.exe"), b"stand-in").unwrap();
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&src, NOW).unwrap();
    // The hub went back a version: the installed manifest now asks for a newer one.
    let installed = reg.module_dir("org.example.kiln", "2.0.0");
    std::fs::write(
        installed.join("module.toml"),
        served.replace("\"1.0\"", "\"1.9\"") + "a_key_from_hub_1_9 = true\n",
    )
    .unwrap();
    let reg = Registry::open(&home, true).unwrap();
    assert!(reg.inventory.modules.contains_key("org.example.kiln"));
    assert_eq!(reg.module_name("org.example.kiln"), "Example lab plugin");
    assert_eq!(
        reg.manifest_checked("org.example.kiln").unwrap_err().id(),
        Some("manifest.hub")
    );
    // Installing a newer-hub version is refused before anything changes.
    let newer = home.join("src").join("kiln-2");
    std::fs::create_dir_all(&newer).unwrap();
    std::fs::write(
        newer.join("module.toml"),
        served
            .replace("\"1.0\"", "\"1.9\"")
            .replace("2.0.0", "2.1.0"),
    )
    .unwrap();
    let mut reg = Registry::open(&home, true).unwrap();
    let e = reg.install(&newer, NOW).unwrap_err();
    assert_eq!(e.id(), Some("manifest.hub"), "{e}");
    assert_eq!(reg.inventory.modules["org.example.kiln"].version, "2.0.0");
}
