// SPDX-License-Identifier: MIT
//! #1465, for records written before it: a folder step that carried everything to changed bytes
//! gets the rule once when the registry opens. The held items wait for the user's OK again, the
//! keyboard record is void, the rest stays, and a second open changes nothing.
//! Window-free; temp folders only.

use std::path::PathBuf;

use hideout_registry::inventory::Inventory;
use hideout_registry::keyboard::{GrantState, KeyboardGrant, VoidReason};
use hideout_registry::permission::{held_on_change, ItemState, FOLLOW_ATTESTED_BY};
use hideout_registry::Registry;

const NOW: &str = "2026-11-17T12:00:00Z";
const LATER: &str = "2026-11-17T20:40:00Z";
const ID: &str = "foxglove";
const ALL: [&str; 3] = ["hub.notify", "net", "ui.window"];

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-keep-reask-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Fx {
    home: PathBuf,
    exe: PathBuf,
    reg: Registry,
}

/// An adopted app that asks for two ordinary items and a held one, allowed with the folder
/// choice on and the keyboard on.
fn fx(name: &str) -> Fx {
    let install = temp(&format!("{name}-install"));
    let exe = install.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    let run = exe.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = [\"{run}\"]\n\
             api = {{ foxglove = \"1.0\" }}\npermissions = [{}]\n",
            ALL.iter()
                .map(|a| format!("\"{a}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
    .unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    let list = reg.permission_list(ID).unwrap();
    let all: Vec<String> = ALL.iter().map(|a| a.to_string()).collect();
    reg.approve_listed(ID, list.code(), &all, &[], NOW, "dialog", true)
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    let (path, hash) = reg.current_program(ID).unwrap();
    reg.allow_keyboard(ID, &path, &hash, NOW).unwrap();
    Fx { home, exe, reg }
}

/// The app file changes and the folder step is taken; then the file on disk is rewritten to
/// what a hub from before the rule left: everything granted, nothing waiting, the keyboard `on`
/// for the new bytes, and no mark on the step.
fn old_format(name: &str) -> Fx {
    let mut f = fx(name);
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    let (path, new) = f.reg.current_program(ID).unwrap();
    Inventory::update(&f.home.join("inventory.json"), |inv| {
        let m = inv.modules.get_mut(ID).unwrap();
        m.granted = ALL.iter().map(|a| a.to_string()).collect();
        m.pending_permissions.clear();
        m.last_choice.clear();
        m.keyboard = Some(KeyboardGrant {
            state: GrantState::On,
            path: path.clone(),
            sha256: new.to_ascii_lowercase(),
            at: LATER.into(),
            reason: None,
        });
        let step = m.bound.as_mut().unwrap().attested.last_mut().unwrap();
        assert!(step.by == FOLLOW_ATTESTED_BY && step.held_back);
        step.held_back = false;
    })
    .unwrap();
    // An old file has no such field at all.
    let text = std::fs::read_to_string(f.home.join("inventory.json")).unwrap();
    assert!(!text.contains("held_back"));
    f
}

fn on_disk(f: &Fx) -> String {
    std::fs::read_to_string(f.home.join("inventory.json")).unwrap()
}

#[test]
fn an_old_folder_step_asks_again_for_the_held_items_once() {
    let f = old_format("old");
    let (path, new) = f.reg.current_program(ID).unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    let m = &reg.inventory.modules[ID];
    // The held item waits, shown as allowed before; the others stay granted.
    assert_eq!(m.granted, ["hub.notify", "ui.window"]);
    assert_eq!(m.pending_permissions, ["net"]);
    assert_eq!(m.last_choice, ["net"]);
    // The keyboard record is void for the changed file.
    let k = m.keyboard.clone().unwrap();
    assert_eq!(k.state, GrantState::Void);
    assert_eq!(k.reason, Some(VoidReason::Changed));
    assert!(k.sha256.is_empty() && !k.allows(&path, &new));
    assert!(reg.attested_keyboard_moves().is_empty());
    // The binding itself is as it was: still on, still for the new bytes, nothing removed.
    let b = m.bound.as_ref().unwrap();
    assert!(!b.void && b.follow.is_some() && b.exe_sha256.eq_ignore_ascii_case(&new));
    assert_eq!(b.attested.len(), 1);
    assert!(b.attested[0].held_back);
    assert_eq!(reg.binding_void(ID), None);
    assert!(reg.pre_launch(ID).is_ok());
    // The row state is the existing "changed since you allowed it" one: not void, the newest
    // kept step is the folder's, and a held item is asked and waiting.
    let v = reg.permission_view(ID).unwrap();
    assert!(v.void.is_none());
    assert!(v.bound.as_ref().is_some_and(|b| !b.void
        && b.follow.is_some()
        && b.attested.last().unwrap().by == FOLLOW_ATTESTED_BY));
    assert!(v.listing.items.iter().any(|i| i.state == ItemState::Asked
        && v.pending.contains(&i.raw)
        && held_on_change(&i.raw)));

    // It is saved, and it does not run twice: a second open leaves the file as it is, also
    // after the user allowed the item and the keyboard again.
    let saved = on_disk(&f);
    let mut again = Registry::open(&f.home, true).unwrap();
    assert_eq!(again.inventory.modules[ID], reg.inventory.modules[ID]);
    assert_eq!(on_disk(&f), saved);
    assert!(again.reask_old_folder_steps(LATER).unwrap().is_empty());
    let list = again.permission_list(ID).unwrap();
    again
        .approve_listed(ID, list.code(), &["net".into()], &[], LATER, "dialog", true)
        .unwrap();
    again.allow_keyboard(ID, &path, &new, LATER).unwrap();
    let allowed = on_disk(&f);
    let third = Registry::open(&f.home, true).unwrap();
    assert_eq!(on_disk(&f), allowed);
    let m = &third.inventory.modules[ID];
    let mut g = m.granted.clone();
    g.sort();
    assert_eq!(g, ALL);
    assert!(m.pending_permissions.is_empty());
    assert!(m.keyboard.as_ref().unwrap().allows(&path, &new));
}

#[test]
fn a_folder_step_made_under_the_rule_is_not_touched() {
    let mut f = fx("new");
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    let saved = on_disk(&f);
    assert!(saved.contains("held_back"));
    let reg = Registry::open(&f.home, true).unwrap();
    assert_eq!(on_disk(&f), saved);
    assert_eq!(reg.inventory.modules[ID], f.reg.inventory.modules[ID]);
}

#[test]
fn a_vouched_update_is_not_touched() {
    let mut f = fx("signed");
    let (path, _) = f.reg.current_program(ID).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    assert!(!f.reg.keep_followed(ID, LATER).unwrap());
    let (_, new) = f.reg.current_program(ID).unwrap();
    let saved = on_disk(&f);
    assert!(!saved.contains("held_back"));
    let reg = Registry::open(&f.home, true).unwrap();
    assert_eq!(on_disk(&f), saved);
    let m = &reg.inventory.modules[ID];
    let mut g = m.granted.clone();
    g.sort();
    assert_eq!(g, ALL);
    assert!(m.pending_permissions.is_empty() && m.last_choice.is_empty());
    assert!(m.keyboard.as_ref().unwrap().allows(&path, &new));
    assert_eq!(reg.attested_keyboard_moves().len(), 1);
}

#[test]
fn an_update_vouched_after_an_old_folder_step_is_not_touched() {
    // The newest step decides: the updater vouched for the bytes the grant now names.
    let mut f = old_format("old-then-signed");
    std::fs::write(&f.exe, b"build 3").unwrap();
    // Read the old file without the open-time pass having a say: the step is the updater's
    // before any open sees the folder one as newest.
    Inventory::update(&f.home.join("inventory.json"), |inv| {
        let b = inv.modules.get_mut(ID).unwrap().bound.as_mut().unwrap();
        let mut step = b.attested.last().unwrap().clone();
        step.by = "updater".into();
        b.attested.push(step);
    })
    .unwrap();
    let saved = on_disk(&f);
    f.reg = Registry::open(&f.home, true).unwrap();
    assert_eq!(on_disk(&f), saved);
    let mut g = f.reg.inventory.modules[ID].granted.clone();
    g.sort();
    assert_eq!(g, ALL);
}

#[test]
fn an_old_step_on_a_grant_with_nothing_held_only_gets_the_mark() {
    let f = old_format("nothing-held");
    Inventory::update(&f.home.join("inventory.json"), |inv| {
        let m = inv.modules.get_mut(ID).unwrap();
        m.granted.retain(|g| !held_on_change(g));
        m.keyboard = None;
    })
    .unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    let m = &reg.inventory.modules[ID];
    assert_eq!(m.granted, ["hub.notify", "ui.window"]);
    assert!(m.pending_permissions.is_empty() && m.last_choice.is_empty());
    assert!(m.keyboard.is_none());
    assert!(m.bound.as_ref().unwrap().attested[0].held_back);
}
