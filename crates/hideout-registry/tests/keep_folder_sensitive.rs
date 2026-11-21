// SPDX-License-Identifier: MIT
//! #1465: "Keep my OK after updates in this folder" never carries Sensitive items or the
//! keyboard to an app file whose bytes changed, unless the updater or installer vouched for it.
//! Window-free; temp folders only.

use std::path::PathBuf;
use std::sync::Arc;

use hideout_registry::dev_folder::DevProbes;
use hideout_registry::drive::SystemDrives;
use hideout_registry::keyboard::{GrantState, VoidReason};
use hideout_registry::sync_root::{SyncAnswer, SyncProbe};
use hideout_registry::Registry;

const NOW: &str = "2026-11-17T12:00:00Z";
const LATER: &str = "2026-11-17T20:40:00Z";
const ID: &str = "foxglove";
const ALL: [&str; 3] = ["hub.notify", "net", "ui.window"];

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-keep-sensitive-tests")
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

/// An adopted app that asks for two ordinary items and a Sensitive one, allowed with the
/// folder choice on and the keyboard on.
fn fx(name: &str) -> Fx {
    fx_asking(name, &ALL)
}

fn fx_asking(name: &str, asks: &[&str]) -> Fx {
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
            asks.iter()
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
    let mut all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    all.sort();
    let mut want: Vec<String> = asks.iter().map(|a| a.to_string()).collect();
    want.sort();
    assert_eq!(all, want, "the app asks for these");
    reg.approve_listed(ID, list.code(), &all, &[], NOW, "dialog", true)
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    let (path, hash) = reg.current_program(ID).unwrap();
    reg.allow_keyboard(ID, &path, &hash, NOW).unwrap();
    Fx { home, exe, reg }
}

fn granted(f: &Fx) -> Vec<String> {
    let mut g = f.reg.inventory.modules[ID].granted.clone();
    g.sort();
    g
}

#[test]
fn changed_bytes_hold_back_sensitive_items_and_the_keyboard() {
    let mut f = fx("changed");
    let (path, old) = f.reg.current_program(ID).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    let (_, new) = f.reg.current_program(ID).unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    // What is not Sensitive carried; the app is not stopped.
    assert_eq!(granted(&f), ["hub.notify", "ui.window"]);
    assert_eq!(f.reg.binding_void(ID), None);
    assert!(f.reg.pre_launch(ID).is_ok());
    let m = &f.reg.inventory.modules[ID];
    assert!(m
        .bound
        .as_ref()
        .unwrap()
        .exe_sha256
        .eq_ignore_ascii_case(&new));
    // Sensitive waits for the OK, shown as allowed before; nothing else moved.
    assert_eq!(m.pending_permissions, ["net"]);
    assert_eq!(m.last_choice, ["net"]);
    let v = f.reg.permission_view(ID).unwrap();
    assert!(v.void.is_none());
    assert!(v.listing.grantable().any(|i| i.raw == "net"));
    // The keyboard is not carried to either file; the router is not told to follow.
    let k = m.keyboard.clone().unwrap();
    assert_eq!(k.state, GrantState::Void);
    assert_eq!(k.reason, Some(VoidReason::Changed));
    assert!(!k.allows(&path, &old) && !k.allows(&path, &new));
    assert!(f.reg.attested_keyboard_moves().is_empty());
    // It survives a reopen, and a second build changes nothing more.
    let mut again = Registry::open(&f.home, true).unwrap();
    assert_eq!(again.inventory.modules[ID], f.reg.inventory.modules[ID]);
    std::fs::write(&f.exe, b"build 3").unwrap();
    assert!(again.keep_followed(ID, LATER).unwrap());
    assert_eq!(again.inventory.modules[ID].pending_permissions, ["net"]);
}

#[test]
fn a_signed_update_keeps_everything_with_no_prompt() {
    let mut f = fx("signed");
    let (path, _) = f.reg.current_program(ID).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    // The updater vouches first, as it does before the next launch check.
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    assert!(!f.reg.keep_followed(ID, LATER).unwrap());
    assert_eq!(granted(&f), ALL);
    let m = &f.reg.inventory.modules[ID];
    assert!(m.pending_permissions.is_empty() && m.last_choice.is_empty());
    let (_, new) = f.reg.current_program(ID).unwrap();
    assert!(m.keyboard.as_ref().unwrap().allows(&path, &new));
    assert_eq!(f.reg.attested_keyboard_moves().len(), 1);
}

#[test]
fn unchanged_bytes_carry_everything() {
    let mut f = fx("same");
    let (path, hash) = f.reg.current_program(ID).unwrap();
    assert!(!f.reg.keep_followed(ID, LATER).unwrap());
    assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), None);
    assert_eq!(granted(&f), ALL);
    let m = &f.reg.inventory.modules[ID];
    assert!(m.pending_permissions.is_empty());
    assert!(m.keyboard.as_ref().unwrap().allows(&path, &hash));
}

#[test]
fn allowing_again_binds_the_sensitive_items_and_the_keyboard_to_the_new_bytes() {
    let mut f = fx("again");
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    let list = f.reg.permission_list(ID).unwrap();
    f.reg
        .approve_listed(ID, list.code(), &["net".into()], &[], LATER, "dialog", true)
        .unwrap();
    let (path, new) = f.reg.current_program(ID).unwrap();
    f.reg.allow_keyboard(ID, &path, &new, LATER).unwrap();
    assert_eq!(granted(&f), ALL);
    let m = &f.reg.inventory.modules[ID];
    let b = m.bound.as_ref().unwrap();
    assert!(b.exe_sha256.eq_ignore_ascii_case(&new) && !b.void);
    assert!(m.pending_permissions.is_empty() && m.last_choice.is_empty());
    assert!(m.keyboard.as_ref().unwrap().allows(&path, &new));
    // Still bound to the new bytes only: another change is held back the same way.
    std::fs::write(&f.exe, b"build 3").unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    assert_eq!(granted(&f), ["hub.notify", "ui.window"]);
}

#[test]
fn an_allow_made_before_the_carry_ran_still_holds_the_sensitive_items_back() {
    let mut f = fx("early-allow");
    std::fs::write(&f.exe, b"build 2").unwrap();
    // No keep_followed yet: an Allow must not bind the old Sensitive grants to the new bytes.
    let list = f.reg.permission_list(ID).unwrap();
    let r = f
        .reg
        .approve_listed(ID, list.code(), &[], &[], LATER, "dialog", true);
    assert!(
        r.is_err(),
        "the list changed under it, so nothing was bound"
    );
    assert_eq!(granted(&f), ["hub.notify", "ui.window"]);
}

#[derive(Clone, Copy)]
struct Fake(SyncAnswer);
impl SyncProbe for Fake {
    fn check(&self, _: &str) -> SyncAnswer {
        self.0
    }
}

#[test]
fn a_folder_that_syncs_or_cannot_be_read_keeps_nothing() {
    for answer in [SyncAnswer::Synced, SyncAnswer::Unknown] {
        let mut f = fx("sync");
        f.reg.set_dev_probes(DevProbes {
            drives: Arc::new(SystemDrives),
            sync: Arc::new(Fake(answer)),
        });
        std::fs::write(&f.exe, b"build 2").unwrap();
        assert!(!f.reg.keep_followed(ID, LATER).unwrap());
        assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
        assert_eq!(f.reg.permission_view(ID).unwrap().folder_sync, answer);
    }
}

#[test]
fn files_secrets_channel_opens_and_offers_wait_like_net() {
    let asks = [
        "hub.notify",
        "net",
        "files",
        "secrets",
        "channel.open:foxglove.example-link/art",
        "channel.offer:art",
    ];
    let mut f = fx_asking("held-set", &asks);
    let before = granted(&f);
    for a in asks {
        assert!(before.iter().any(|g| g == a), "{a} was allowed");
    }
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    assert_eq!(granted(&f), ["hub.notify"]);
    let mut waiting = f.reg.inventory.modules[ID].pending_permissions.clone();
    waiting.sort();
    assert_eq!(
        waiting,
        [
            "channel.offer:art",
            "channel.open:foxglove.example-link/art",
            "files",
            "net",
            "secrets"
        ]
    );
    assert!(hideout_registry::permission::held_on_change("secrets"));
    assert!(hideout_registry::permission::held_on_change(
        "channel.offer:art"
    ));
    assert!(hideout_registry::permission::held_on_change(
        "provide:a.read"
    ));
    assert!(!hideout_registry::permission::held_on_change("hub.notify"));
}

#[test]
fn the_pinned_check_does_not_excuse_new_bytes_that_keep_followed_never_saw() {
    let f = fx("pinned-swap");
    let (path, _) = f.reg.current_program(ID).unwrap();
    let other = foxglove_plugin_trust::sha256_hex(b"swapped after the keep step");
    // Follow is on and keep_followed has not run: the new bytes are Changed, not carried.
    assert!(f.reg.inventory.modules[ID]
        .bound
        .as_ref()
        .unwrap()
        .follow
        .is_some());
    assert_eq!(
        f.reg.binding_void_pinned(ID, &path, &other),
        Some(VoidReason::Changed)
    );
}
