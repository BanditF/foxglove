// SPDX-License-Identifier: MIT
//! The legacy keyboard grant (#1155-B): the record in `inventory.json`, the decline in
//! `hideout.toml`, void and revoke. Window-free; the "install" is a fake exe in a temp folder.

use std::path::{Path, PathBuf};

use hideout_registry::keyboard::{
    canonical_path, GrantState, KeyboardGrant, KeyboardState, VoidReason,
};
use hideout_registry::Registry;

const NOW: &str = "2026-11-12T10:00:00Z";
const LATER: &str = "2026-11-12T11:00:00Z";
const ID: &str = "foxglove";
const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-keyboard-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(version: &str, exe: &Path) -> String {
    let run = exe.display().to_string().replace('\\', "/");
    format!(
        "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"{version}\"\nkind = \"adopted\"\n\
         family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}']\napi = {{ foxglove = \"1.0\" }}\n"
    )
}

fn setup(name: &str) -> (PathBuf, PathBuf, Registry) {
    let home = temp(&format!("{name}-home"));
    let install = temp(&format!("{name}-install"));
    let exe = install.join("foxglove.exe");
    std::fs::write(&exe, b"fake").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(folder.join("module.toml"), manifest("0.1.0", &exe)).unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    (home, exe, reg)
}

fn on_disk(home: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(home.join("inventory.json")).unwrap()).unwrap()
}

#[test]
fn nothing_is_granted_by_adopting_or_reopening() {
    let (home, _, reg) = setup("never");
    assert_eq!(reg.keyboard_state(ID), KeyboardState::NotAsked);
    assert!(on_disk(&home)["modules"][ID].get("keyboard").is_none());
    let again = Registry::open(&home, false).unwrap();
    assert_eq!(again.keyboard_state(ID), KeyboardState::NotAsked);
    assert_eq!(again.keyboard_state("other"), KeyboardState::NotApplicable);
}

#[test]
fn allow_records_path_and_hash_and_round_trips() {
    let (home, exe, mut reg) = setup("allow");
    let path = canonical_path(&exe).unwrap();
    let g = reg.allow_keyboard(ID, &path, H1, NOW).unwrap();
    assert_eq!(g.state, GrantState::On);
    let json = &on_disk(&home)["modules"][ID]["keyboard"];
    assert_eq!(json["state"], "on");
    assert_eq!(json["sha256"], H1);
    assert_eq!(json["path"], path.as_str());
    let again = Registry::open(&home, false).unwrap();
    assert_eq!(
        again.keyboard_state(ID),
        KeyboardState::Granted { at: NOW.into() }
    );
    assert!(again.inventory.modules[ID]
        .keyboard
        .as_ref()
        .unwrap()
        .allows(&path, H1));
}

#[test]
fn allow_for_another_path_or_a_bad_hash_is_refused() {
    let (_, _, mut reg) = setup("refuse");
    assert!(reg
        .allow_keyboard(ID, r"C:\elsewhere\foxglove.exe", H1, NOW)
        .is_err());
    let path = reg.keyboard_exe(ID).unwrap();
    assert!(reg.allow_keyboard(ID, &path, "abc", NOW).is_err());
    assert_eq!(reg.keyboard_state(ID), KeyboardState::NotAsked);
    // Not the legacy module: no grant at all.
    assert!(reg.allow_keyboard("vulpes", &path, H1, NOW).is_err());
}

#[test]
fn void_never_revives_and_only_allow_turns_it_on() {
    let (home, _, mut reg) = setup("void");
    let path = reg.keyboard_exe(ID).unwrap();
    // Void from nothing does nothing (it can never turn a grant on).
    reg.void_keyboard(ID, VoidReason::Changed, NOW).unwrap();
    assert_eq!(reg.keyboard_state(ID), KeyboardState::NotAsked);
    reg.allow_keyboard(ID, &path, H1, NOW).unwrap();
    reg.void_keyboard(ID, VoidReason::Changed, LATER).unwrap();
    assert_eq!(
        reg.keyboard_state(ID),
        KeyboardState::Changed {
            at: LATER.into(),
            reason: VoidReason::Changed
        }
    );
    // The old file put back (same path, same hash) is not enough.
    let g = reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert!(!g.allows(&path, H1));
    assert!(!Registry::open(&home, false).unwrap().inventory.modules[ID]
        .keyboard
        .as_ref()
        .unwrap()
        .allows(&path, H1));
    // Allow again records the new hash, and only that one.
    reg.allow_keyboard(ID, &path, H2, LATER).unwrap();
    let g = reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert!(g.allows(&path, H2) && !g.allows(&path, H1));
}

#[test]
fn revoke_is_at_once_and_drops_path_and_hash() {
    let (home, _, mut reg) = setup("revoke");
    let path = reg.keyboard_exe(ID).unwrap();
    reg.allow_keyboard(ID, &path, H1, NOW).unwrap();
    reg.revoke_keyboard(ID, LATER).unwrap();
    assert_eq!(
        reg.keyboard_state(ID),
        KeyboardState::Revoked { at: LATER.into() }
    );
    let json = &on_disk(&home)["modules"][ID]["keyboard"];
    assert_eq!(json["state"], "off");
    assert_eq!(json["sha256"], "");
    let g = reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert!(!g.allows(&path, H1));
}

#[test]
fn a_decline_is_remembered_in_hideout_toml_and_not_asked_again() {
    let (home, _, mut reg) = setup("decline");
    reg.set_enabled(ID, true).unwrap();
    assert!(reg.keyboard_needs_ok(ID));
    reg.decline_keyboard(ID).unwrap();
    assert!(!reg.keyboard_needs_ok(ID));
    assert_eq!(reg.keyboard_state(ID), KeyboardState::Declined);
    let toml = std::fs::read_to_string(home.join("hideout.toml")).unwrap();
    assert!(toml.contains("keyboard = \"declined\""), "{toml}");
    // Across a restart it is still declined, and nothing was granted.
    let again = Registry::open(&home, false).unwrap();
    assert_eq!(again.keyboard_state(ID), KeyboardState::Declined);
    assert!(!again.keyboard_needs_ok(ID));
    assert!(on_disk(&home)["modules"][ID].get("keyboard").is_none());
    // Allow clears the decline.
    let mut reg = again;
    let path = reg.keyboard_exe(ID).unwrap();
    reg.allow_keyboard(ID, &path, H1, NOW).unwrap();
    assert!(!reg.hub.keyboard_declined(ID));
}

#[test]
fn needs_ok_only_when_on_and_not_asked_or_changed() {
    let (_, _, mut reg) = setup("needs");
    assert!(!reg.keyboard_needs_ok(ID), "an off module is not asked");
    reg.set_enabled(ID, true).unwrap();
    assert!(reg.keyboard_needs_ok(ID));
    let path = reg.keyboard_exe(ID).unwrap();
    reg.allow_keyboard(ID, &path, H1, NOW).unwrap();
    assert!(!reg.keyboard_needs_ok(ID));
    reg.void_keyboard(ID, VoidReason::Changed, LATER).unwrap();
    assert!(reg.keyboard_needs_ok(ID), "a changed program asks again");
    reg.revoke_keyboard(ID, LATER).unwrap();
    assert!(
        !reg.keyboard_needs_ok(ID),
        "a turned-off grant is not asked by itself"
    );
}

#[test]
fn registering_again_keeps_the_record_and_never_adds_one() {
    let (home, exe, mut reg) = setup("again");
    // An upgrade with no record: still no record.
    let folder = temp("again-manifest2");
    std::fs::write(folder.join("module.toml"), manifest("0.2.0", &exe)).unwrap();
    reg.register_adopted(&folder, LATER).unwrap();
    assert_eq!(reg.keyboard_state(ID), KeyboardState::NotAsked);
    // With a record: carried over exactly as it was.
    let path = canonical_path(&exe).unwrap();
    reg.allow_keyboard(ID, &path, H1, NOW).unwrap();
    std::fs::write(folder.join("module.toml"), manifest("0.3.0", &exe)).unwrap();
    reg.register_adopted(&folder, LATER).unwrap();
    let g: KeyboardGrant = reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert_eq!((g.state, g.at.as_str()), (GrantState::On, NOW));
    assert_eq!(
        Registry::open(&home, false).unwrap().inventory.modules[ID].keyboard,
        Some(g)
    );
}

#[test]
fn a_hand_edited_unknown_or_broken_record_is_no_grant_and_the_inventory_still_loads() {
    let (home, _, reg) = setup("broken");
    drop(reg);
    let mut v = on_disk(&home);
    v["modules"][ID]["keyboard"] = serde_json::json!({"state": "maybe"});
    std::fs::write(home.join("inventory.json"), v.to_string()).unwrap();
    let reg = Registry::open(&home, false).unwrap();
    assert_eq!(reg.keyboard_state(ID), KeyboardState::NotAsked);
    let mut v = on_disk(&home);
    v["modules"][ID]["keyboard"] = serde_json::json!("garbage");
    std::fs::write(home.join("inventory.json"), v.to_string()).unwrap();
    let reg = Registry::open(&home, false).unwrap();
    assert_eq!(reg.keyboard_state(ID), KeyboardState::NotAsked);
}
