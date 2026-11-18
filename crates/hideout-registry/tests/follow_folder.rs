// SPDX-License-Identifier: MIT
//! The follow-folder grant ("Keep my OK after updates in this folder", design note option A,
//! slice 3): a new hash of the same file in the same local folder is kept and recorded; any
//! other change voids as before; only the dialog can set it. Window-free.

use std::path::{Path, PathBuf};

use hideout_registry::keyboard::VoidReason;
use hideout_registry::permission::on_local_fixed_drive;
use hideout_registry::{Failure, Registry};

const NOW: &str = "2026-11-17T12:00:00Z";
const LATER: &str = "2026-11-17T20:40:00Z";
const ID: &str = "foxglove";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-follow-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(run: &[&str]) -> String {
    let run: Vec<String> = run
        .iter()
        .map(|a| format!("\"{}\"", a.replace('\\', "/")))
        .collect();
    format!(
        "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
         family = \"foxglove\"\nhub = \"1.0\"\nrun = [{}]\napi = {{ foxglove = \"1.0\" }}\n\
         permissions = [\"hub.notify\", \"ui.window\"]\n",
        run.join(", ")
    )
}

struct Fx {
    home: PathBuf,
    install: PathBuf,
    exe: PathBuf,
    folder: PathBuf,
    reg: Registry,
}

impl Fx {
    /// Points the manifest at `run` and re-adopts it, as `--adopt` does; returns a fresh
    /// registry read from disk.
    fn readopt(&mut self, run: &[&str]) {
        std::fs::write(self.folder.join("module.toml"), manifest(run)).unwrap();
        let mut other = Registry::open(&self.home, true).unwrap();
        other.register_adopted(&self.folder, NOW).unwrap();
        self.reg = Registry::open(&self.home, true).unwrap();
    }

    fn bound(&self) -> hideout_registry::inventory::Bound {
        self.reg.inventory.modules[ID].bound.clone().unwrap()
    }

    fn granted(&self) -> Vec<String> {
        self.reg.inventory.modules[ID].granted.clone()
    }
}

fn fx(name: &str) -> Fx {
    let install = temp(&format!("{name}-install"));
    let exe = install.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(
        folder.join("module.toml"),
        manifest(&[&exe.display().to_string()]),
    )
    .unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    Fx {
        home,
        install,
        exe,
        folder,
        reg,
    }
}

fn allow(f: &mut Fx, by: &str, follow: bool) -> Result<(), Failure> {
    let list = f.reg.permission_list(ID)?;
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    f.reg
        .approve_listed(ID, list.code(), &all, &[], NOW, by, follow)?;
    f.reg.set_enabled(ID, true)?;
    Ok(())
}

fn id_of<T: std::fmt::Debug>(r: Result<T, Failure>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => f.id().map_or_else(|| f.to_string(), String::from),
    }
}

/// The change voids exactly as without a follow: refused at launch, granted back to pending.
fn assert_voids(f: &mut Fx, why: VoidReason) {
    assert_eq!(f.reg.binding_void(ID), Some(why));
    assert_eq!(
        f.reg.pre_launch(ID).unwrap_err().id(),
        Some("trust.mismatch")
    );
    assert!(!f.reg.keep_followed(ID, LATER).unwrap());
    assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), Some(why));
    assert!(f.granted().is_empty());
    assert!(f.bound().void);
    assert!(f.bound().attested.is_empty(), "{:?}", f.bound().attested);
}

#[test]
fn the_dialog_sets_a_follow_for_the_programs_folder_and_file_name() {
    let mut f = fx("set");
    allow(&mut f, "dialog", true).unwrap();
    let b = f.bound();
    let follow = b.follow.expect("follow set");
    assert_eq!(follow.by, "dialog");
    assert_eq!(follow.at, NOW);
    assert!(follow.file_name.eq_ignore_ascii_case("foxglove.exe"));
    let canon = hideout_registry::keyboard::canonical_path(&f.install).unwrap();
    assert!(hideout_registry::keyboard::same_path(
        &follow.folder,
        &canon
    ));
    // It survives a reopen.
    let again = Registry::open(&f.home, true).unwrap();
    assert_eq!(again.inventory.modules[ID], f.reg.inventory.modules[ID]);
}

#[test]
fn a_new_build_in_the_same_folder_is_kept_and_recorded() {
    let mut f = fx("kept");
    allow(&mut f, "dialog", true).unwrap();
    let old = f.bound().exe_sha256;
    std::fs::write(&f.exe, b"build 2").unwrap();
    // Read-only checks already see it as kept.
    assert_eq!(f.reg.binding_void(ID), None);
    assert!(f.reg.pre_launch(ID).is_ok());
    // The turn-on check records it and voids nothing.
    assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), None);
    assert_eq!(f.granted(), ["hub.notify", "ui.window"]);
    let b = f.bound();
    assert!(!b.void);
    assert_ne!(b.exe_sha256, old);
    assert_eq!(b.exe_sha256.len(), 64);
    assert_eq!(b.attested.len(), 1);
    let a = &b.attested[0];
    assert_eq!((a.by.as_str(), a.at.as_str()), ("folder", LATER));
    assert_eq!(
        (a.from_sha256.as_str(), a.to_sha256.as_str()),
        (old.as_str(), b.exe_sha256.as_str())
    );
    assert!(hideout_registry::keyboard::same_path(
        &a.from_path,
        &a.to_path
    ));
    // Recorded on disk; a second check finds nothing new to record.
    let mut again = Registry::open(&f.home, true).unwrap();
    assert_eq!(again.inventory.modules[ID], f.reg.inventory.modules[ID]);
    assert!(!again.keep_followed(ID, LATER).unwrap());
    // A third build adds a second line.
    std::fs::write(&f.exe, b"build 3").unwrap();
    assert!(again.keep_followed(ID, LATER).unwrap());
    assert_eq!(
        again.inventory.modules[ID]
            .bound
            .as_ref()
            .unwrap()
            .attested
            .len(),
        2
    );
    assert!(again.pre_launch(ID).is_ok());
}

#[test]
fn a_kept_build_moves_an_on_keyboard_record_with_it() {
    let mut f = fx("keyboard");
    allow(&mut f, "dialog", true).unwrap();
    let (path, old) = f.reg.current_program(ID).unwrap();
    f.reg.allow_keyboard(ID, &path, &old, NOW).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    let (_, new) = f.reg.current_program(ID).unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    let k = f.reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert!(k.allows(&path, &new));
    assert!(!k.allows(&path, &old));
}

#[test]
fn without_follow_a_new_build_voids_as_before() {
    let mut f = fx("off");
    allow(&mut f, "dialog", false).unwrap();
    assert!(f.bound().follow.is_none());
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert_voids(&mut f, VoidReason::Changed);
}

#[test]
fn another_folder_voids() {
    let mut f = fx("folder");
    allow(&mut f, "dialog", true).unwrap();
    let other = temp("folder-other");
    let exe2 = other.join("foxglove.exe");
    std::fs::write(&exe2, b"build 2").unwrap();
    f.readopt(&[&exe2.display().to_string()]);
    assert_voids(&mut f, VoidReason::Moved);
}

#[test]
fn another_file_name_in_the_same_folder_voids() {
    let mut f = fx("name");
    allow(&mut f, "dialog", true).unwrap();
    let exe2 = f.install.join("other.exe");
    std::fs::write(&exe2, b"build 2").unwrap();
    f.readopt(&[&exe2.display().to_string()]);
    assert_voids(&mut f, VoidReason::Moved);
}

#[test]
fn changed_program_arguments_void_even_when_none_were_recorded() {
    let mut f = fx("args");
    allow(&mut f, "dialog", true).unwrap();
    assert!(f.bound().run_args.is_empty());
    let exe = f.exe.display().to_string();
    std::fs::write(&f.exe, b"build 2").unwrap();
    f.readopt(&[&exe, "--evil"]);
    assert_voids(&mut f, VoidReason::Changed);
}

#[test]
fn changed_recorded_program_arguments_void() {
    let mut f = fx("args2");
    let exe = f.exe.display().to_string();
    f.readopt(&[&exe, "--quiet"]);
    allow(&mut f, "dialog", true).unwrap();
    assert_eq!(f.bound().run_args, ["--quiet"]);
    std::fs::write(&f.exe, b"build 2").unwrap();
    f.readopt(&[&exe, "--evil"]);
    assert_voids(&mut f, VoidReason::Changed);
}

#[test]
fn a_junction_that_points_elsewhere_voids() {
    let mut f = fx("junction");
    allow(&mut f, "dialog", true).unwrap();
    // The install folder is swapped for a junction to another folder holding a new build:
    // the manifest's path is unchanged, the canonical path is not.
    let elsewhere = temp("junction-elsewhere");
    std::fs::write(elsewhere.join("foxglove.exe"), b"build 2").unwrap();
    let parked = f.install.with_extension("old");
    std::fs::rename(&f.install, &parked).unwrap();
    let ok = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&f.install)
        .arg(&elsewhere)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "mklink /J failed");
    assert!(f.exe.exists());
    assert_voids(&mut f, VoidReason::Moved);
    let _ = std::fs::remove_dir(&f.install);
}

/// `\\localhost\C$\...`: the same file over the network, when the admin share is reachable.
fn unc_of(p: &Path) -> Option<String> {
    let s = hideout_registry::keyboard::canonical_path(p).ok()?;
    let (drive, rest) = s.split_once(":\\")?;
    let unc = format!(r"\\localhost\{drive}$\{rest}");
    std::fs::metadata(&unc).ok().map(|_| unc)
}

#[test]
fn network_paths_are_never_local() {
    for p in [
        r"\\server\share\foxglove.exe",
        r"\\?\UNC\server\share\foxglove.exe",
        r"//server/share/foxglove.exe",
        r"\\.\C:\foxglove.exe",
        r"\\localhost\C$\x\foxglove.exe",
        "foxglove.exe",
        "",
    ] {
        assert!(!on_local_fixed_drive(p), "{p}");
    }
    let here = hideout_registry::keyboard::canonical_path(&std::env::temp_dir()).unwrap();
    assert!(on_local_fixed_drive(&format!(r"{here}\x.exe")), "{here}");
}

#[test]
fn a_program_on_a_unc_path_cannot_follow_and_a_forged_follow_voids() {
    let mut f = fx("unc");
    let Some(unc) = unc_of(&f.exe) else {
        eprintln!("skipped: \\\\localhost admin share not reachable");
        return;
    };
    f.readopt(&[&unc]);
    let before = std::fs::read(f.home.join("inventory.json")).unwrap();
    let r = allow(&mut f, "dialog", true);
    assert!(matches!(r, Err(Failure::Io(_))), "{r:?}");
    assert_eq!(
        std::fs::read(f.home.join("inventory.json")).unwrap(),
        before
    );
    // Allowed without follow, then a follow written into the file by hand: still void.
    allow(&mut f, "dialog", false).unwrap();
    forge_follow(&f, "dialog");
    f.reg = Registry::open(&f.home, true).unwrap();
    assert!(f.bound().follow.is_some());
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert_voids(&mut f, VoidReason::Changed);
}

/// Writes a follow into `inventory.json` as a same-user process could.
fn forge_follow(f: &Fx, by: &str) {
    let path = f.home.join("inventory.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let b = &mut v["modules"][ID]["bound"];
    let (folder, file) = {
        let p = PathBuf::from(b["exe_path"].as_str().unwrap());
        (
            p.parent().unwrap().display().to_string(),
            p.file_name().unwrap().to_string_lossy().into_owned(),
        )
    };
    b["follow"] = serde_json::json!({"folder": folder, "file_name": file, "at": NOW, "by": by});
    std::fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

#[test]
fn only_the_dialog_can_set_a_follow() {
    let mut f = fx("caller");
    let before = std::fs::read(f.home.join("inventory.json")).unwrap();
    for by in ["cli", "router", "updater", "folder", ""] {
        let r = allow(&mut f, by, true);
        assert_eq!(id_of(r), "permission.caller", "{by}");
        assert_eq!(
            std::fs::read(f.home.join("inventory.json")).unwrap(),
            before,
            "{by}"
        );
    }
    assert!(f.granted().is_empty());
    // A follow that names another writer is no follow.
    allow(&mut f, "cli", false).unwrap();
    forge_follow(&f, "cli");
    f.reg = Registry::open(&f.home, true).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert_voids(&mut f, VoidReason::Changed);
}

#[test]
fn an_allow_without_follow_turns_it_off() {
    let mut f = fx("again");
    allow(&mut f, "dialog", true).unwrap();
    assert!(f.bound().follow.is_some());
    allow(&mut f, "cli", false).unwrap();
    assert!(f.bound().follow.is_none());
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert_voids(&mut f, VoidReason::Changed);
}

#[test]
fn a_void_binding_is_not_revived_by_its_follow() {
    let mut f = fx("revive");
    allow(&mut f, "dialog", true).unwrap();
    let exe = f.exe.display().to_string();
    let exe2 = f.install.join("other.exe");
    std::fs::write(&exe2, b"build 2").unwrap();
    f.readopt(&[&exe2.display().to_string()]);
    assert_voids(&mut f, VoidReason::Moved);
    // Back to the followed file with a new build: still void, only a new Allow helps.
    std::fs::write(&f.exe, b"build 3").unwrap();
    f.readopt(&[&exe]);
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
    assert!(!f.reg.keep_followed(ID, LATER).unwrap());
    assert!(f.reg.pre_launch(ID).is_err());
    assert!(f.granted().is_empty());
}

#[test]
fn stop_keeping_clears_only_the_follow() {
    let mut f = fx("stop");
    allow(&mut f, "dialog", true).unwrap();
    let before = f.bound();
    let granted = f.granted();
    assert!(f.reg.stop_keeping(ID).unwrap());
    let after = f.bound();
    assert!(after.follow.is_none());
    // The hash, path, list, history, void flag and grants are as they were.
    let mut expect = before.clone();
    expect.follow = None;
    assert_eq!(after, expect);
    assert_eq!(f.granted(), granted);
    // On disk too, and asking again changes nothing.
    let mut again = Registry::open(&f.home, true).unwrap();
    assert!(again.inventory.modules[ID]
        .bound
        .as_ref()
        .unwrap()
        .follow
        .is_none());
    assert!(!again.stop_keeping(ID).unwrap());
    // The next new build voids as it does without a follow.
    std::fs::write(&f.exe, b"build 2").unwrap();
    f.reg = Registry::open(&f.home, true).unwrap();
    assert_voids(&mut f, VoidReason::Changed);
}

#[test]
fn stop_keeping_does_not_record_a_build_that_landed_unchecked() {
    let mut f = fx("stop-landed");
    allow(&mut f, "dialog", true).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert!(f.reg.stop_keeping(ID).unwrap());
    assert!(f.bound().attested.is_empty());
    assert_voids(&mut f, VoidReason::Changed);
}

#[test]
fn stop_keeping_on_an_unknown_module_fails_and_without_a_follow_does_nothing() {
    let mut f = fx("stop-none");
    assert!(f.reg.stop_keeping("nope").is_err());
    allow(&mut f, "dialog", false).unwrap();
    assert!(!f.reg.stop_keeping(ID).unwrap());
}

#[test]
fn keeping_an_existing_allow_with_no_new_items_leaves_the_grants() {
    // The Modules page's "Turn on…": the same call with nothing chosen and nothing declined.
    let mut f = fx("turn-on");
    allow(&mut f, "dialog", false).unwrap();
    let granted = f.granted();
    assert!(!granted.is_empty());
    let list = f.reg.permission_list(ID).unwrap();
    f.reg
        .approve_listed(ID, list.code(), &[], &[], LATER, "dialog", true)
        .unwrap();
    assert_eq!(f.granted(), granted);
    assert!(f.bound().follow.is_some());
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert_eq!(f.reg.binding_void(ID), None);
}

// ---- #1267: the router may follow a recorded keyboard-record move ----

/// Allows by `by` (follow on or off), turns the keyboard grant on, then puts a new build in place.
/// Returns the old hash.
fn keyboard_on_then_rebuild(f: &mut Fx, by: &str, follow: bool) -> String {
    allow(f, by, follow).unwrap();
    let (path, old) = f.reg.current_program(ID).unwrap();
    f.reg.allow_keyboard(ID, &path, &old, NOW).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    old
}

#[test]
fn a_followed_build_yields_a_move_from_the_old_hash() {
    let mut f = fx("mv-follow");
    let old = keyboard_on_then_rebuild(&mut f, "dialog", true);
    assert!(
        f.reg.attested_keyboard_moves().is_empty(),
        "nothing recorded yet"
    );
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    let mv = f.reg.attested_keyboard_moves();
    assert_eq!(mv.len(), 1);
    assert_eq!(mv[0].id, ID);
    assert_eq!(mv[0].ancestors.len(), 1);
    assert_eq!(mv[0].ancestors[0].1, old);
    assert_eq!(
        mv[0].to,
        f.reg.inventory.modules[ID].keyboard.clone().unwrap()
    );
    // A second kept build keeps the whole chain, oldest first.
    std::fs::write(&f.exe, b"build 3").unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    let mv = f.reg.attested_keyboard_moves();
    assert_eq!(mv[0].ancestors.len(), 2);
    assert_eq!(mv[0].ancestors[0].1, old);
}

#[test]
fn an_attested_update_yields_a_move_without_a_follow() {
    let mut f = fx("mv-attest");
    let old = keyboard_on_then_rebuild(&mut f, "dialog", false);
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    let mv = f.reg.attested_keyboard_moves();
    assert_eq!(mv.len(), 1);
    assert_eq!(mv[0].ancestors[0].1, old);
}

#[test]
fn no_move_for_an_unattested_change() {
    let mut f = fx("mv-unattested");
    keyboard_on_then_rebuild(&mut f, "dialog", false);
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn no_move_when_the_binding_was_not_set_by_the_dialog() {
    let mut f = fx("mv-cli");
    keyboard_on_then_rebuild(&mut f, "cli", false);
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn no_move_without_an_on_keyboard_record() {
    let mut f = fx("mv-nokb");
    allow(&mut f, "dialog", true).unwrap();
    std::fs::write(&f.exe, b"build 2").unwrap();
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    assert!(f.reg.attested_keyboard_moves().is_empty());
    // A record that is off gives nothing either.
    let (path, hash) = f.reg.current_program(ID).unwrap();
    f.reg.allow_keyboard(ID, &path, &hash, NOW).unwrap();
    f.reg.revoke_keyboard(ID, NOW).unwrap();
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn no_move_for_a_void_or_moved_binding() {
    let mut f = fx("mv-void");
    keyboard_on_then_rebuild(&mut f, "dialog", false);
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    assert_eq!(f.reg.attested_keyboard_moves().len(), 1);
    // The program moves to another path: void, never a move.
    let other = temp("mv-void-other").join("foxglove.exe");
    std::fs::write(&other, b"build 9").unwrap();
    f.readopt(&[&other.display().to_string()]);
    assert_eq!(
        f.reg.void_if_changed(ID, LATER).unwrap(),
        Some(VoidReason::Moved)
    );
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn no_move_when_the_program_arguments_changed() {
    let mut f = fx("mv-args");
    keyboard_on_then_rebuild(&mut f, "dialog", false);
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    let exe = f.exe.display().to_string();
    f.readopt(&[&exe, "--extra"]);
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn no_move_when_the_chain_is_broken_or_changes_the_path() {
    let mut f = fx("mv-chain");
    keyboard_on_then_rebuild(&mut f, "dialog", false);
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    // A step whose path differs from the bound file ends the chain: no move.
    let mut moved = Registry::open(&f.home, true).unwrap();
    moved
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap()
        .attested[0]
        .from_path = r"C:\Elsewhere\foxglove.exe".into();
    assert!(moved.attested_keyboard_moves().is_empty());
    // A step whose new hash is not the bound hash is no link.
    let mut broken = Registry::open(&f.home, true).unwrap();
    broken
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap()
        .attested[0]
        .to_sha256 = "0".repeat(64);
    assert!(broken.attested_keyboard_moves().is_empty());
    // An unknown voucher is no attestation.
    let mut odd = Registry::open(&f.home, true).unwrap();
    odd.inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap()
        .attested[0]
        .by = "module".into();
    assert!(odd.attested_keyboard_moves().is_empty());
}

#[test]
fn a_folder_step_needs_the_follow_still_set_by_the_dialog() {
    let mut f = fx("mv-forged");
    keyboard_on_then_rebuild(&mut f, "dialog", true);
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    assert_eq!(f.reg.attested_keyboard_moves().len(), 1);
    // The follow is cleared or set by anyone but the dialog: the folder step no longer counts.
    let mut cleared = Registry::open(&f.home, true).unwrap();
    cleared
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap()
        .follow = None;
    assert!(cleared.attested_keyboard_moves().is_empty());
    let mut forged = Registry::open(&f.home, true).unwrap();
    forged
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap()
        .follow
        .as_mut()
        .unwrap()
        .by = "cli".into();
    assert!(forged.attested_keyboard_moves().is_empty());
}

// ---- #1267 security review: replay, loops, forks, stop_keeping ----

const T1: &str = "2026-11-17T13:00:00Z";
const T2: &str = "2026-11-17T14:00:00Z";
const T3: &str = "2026-11-17T15:00:00Z";

fn allow_at(f: &mut Fx, by: &str, follow: bool, at: &str) {
    let list = f.reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    f.reg
        .approve_listed(ID, list.code(), &all, &[], at, by, follow)
        .unwrap();
    f.reg.set_enabled(ID, true).unwrap();
}

fn keyboard_now(f: &mut Fx, at: &str) {
    let (path, h) = f.reg.current_program(ID).unwrap();
    f.reg.allow_keyboard(ID, &path, &h, at).unwrap();
}

#[test]
fn a_cli_attestation_carried_over_a_dialog_re_allow_is_not_replayed() {
    let mut f = fx("mv-replay-cli");
    allow_at(&mut f, "cli", false, NOW);
    std::fs::write(&f.exe, b"build 2").unwrap();
    f.reg.attest_update(ID, "1.1.0", "updater", T1).unwrap();
    // The user then allows from the dialog at the attested hash, and turns the keyboard on.
    allow_at(&mut f, "dialog", false, T2);
    keyboard_now(&mut f, T2);
    assert_eq!(f.bound().attested.len(), 1, "the old step is carried over");
    assert!(f.reg.attested_keyboard_moves().is_empty());
    // A step made under the dialog binding counts, and only it.
    std::fs::write(&f.exe, b"build 3").unwrap();
    f.reg.attest_update(ID, "1.2.0", "updater", T3).unwrap();
    let mv = f.reg.attested_keyboard_moves();
    assert_eq!(mv.len(), 1);
    assert_eq!(mv[0].ancestors.len(), 1);
}

#[test]
fn a_void_then_re_allow_at_an_old_attested_hash_has_no_move() {
    let mut f = fx("mv-replay-void");
    allow_at(&mut f, "dialog", false, NOW);
    keyboard_now(&mut f, NOW);
    std::fs::write(&f.exe, b"build 2").unwrap();
    f.reg.attest_update(ID, "1.1.0", "updater", T1).unwrap();
    // An unattested swap voids; the user allows again at that file.
    std::fs::write(&f.exe, b"build 3").unwrap();
    assert_eq!(
        f.reg.void_if_changed(ID, T2).unwrap(),
        Some(VoidReason::Changed)
    );
    allow_at(&mut f, "dialog", false, T3);
    keyboard_now(&mut f, T3);
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn loops_and_forks_in_the_history_end_the_chain() {
    let mut f = fx("mv-loop");
    keyboard_on_then_rebuild(&mut f, "dialog", false);
    f.reg.attest_update(ID, "1.1.0", "updater", LATER).unwrap();
    let mut reg = Registry::open(&f.home, true).unwrap();
    let b = reg
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap();
    let step = b.attested[0].clone();
    assert_eq!(reg.attested_keyboard_moves().len(), 1);
    // Loop: a step back to the hash the chain ends on (H0 -> H1 -> H0 shape).
    let mut looped = step.clone();
    looped.from_sha256 = step.to_sha256.clone();
    looped.to_sha256 = step.from_sha256.clone();
    let b = reg
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap();
    b.attested.insert(0, looped);
    let mv = reg.attested_keyboard_moves();
    assert_eq!(mv[0].ancestors.len(), 1, "the walk stops at the loop");
    // Fork: two steps with the same `from`.
    let mut reg = Registry::open(&f.home, true).unwrap();
    let b = reg
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .bound
        .as_mut()
        .unwrap();
    let mut fork = step.clone();
    fork.to_sha256 = "1".repeat(64);
    b.attested.insert(0, fork);
    let mv = reg.attested_keyboard_moves();
    assert_eq!(mv[0].ancestors.len(), 1, "the walk stops at the fork");
}

#[test]
fn stop_keeping_through_the_api_ends_the_folder_steps() {
    let mut f = fx("mv-stop");
    keyboard_on_then_rebuild(&mut f, "dialog", true);
    assert!(f.reg.keep_followed(ID, LATER).unwrap());
    assert_eq!(f.reg.attested_keyboard_moves().len(), 1);
    assert!(f.reg.stop_keeping(ID).unwrap());
    assert!(f.reg.attested_keyboard_moves().is_empty());
}
