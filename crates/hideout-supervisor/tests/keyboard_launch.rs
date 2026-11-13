// SPDX-License-Identifier: MIT
//! The keyboard grant at launch (#1155-B), with a real windowless process (a copy of `ping.exe`
//! registered as the adopted module `foxglove`; never the real Foxglove). Every process a test
//! starts is ended by its handle when the test ends.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::keyboard::{canonical_path, GrantState, KeyboardState, VoidReason};
use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::win::Process;

const NOW: &str = "2026-11-12T10:00:00Z";
const ID: &str = "foxglove";

fn hub() -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    Hub::new(router, events)
}

/// A hub folder with the adopted `foxglove` (a ping copy, enabled) and its exe path.
fn home_with(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("hideout-kbl-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let install = base.join("install");
    std::fs::create_dir_all(&install).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let exe = install.join("foxglove.exe");
    std::fs::copy(Path::new(&windir).join("System32").join("PING.EXE"), &exe).unwrap();
    let folder = base.join("manifest");
    std::fs::create_dir_all(&folder).unwrap();
    let run = exe.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"0.1.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}', '-n', '60', '127.0.0.1']\n\
             api = {{ foxglove = \"1.0\" }}\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.set_enabled(ID, true).unwrap();
    (home, exe)
}

fn run_once(home: &Path) -> Vec<u32> {
    let sup = Supervisor::start(home, hub());
    let pid = sup.pid(ID);
    if let Some(p) = pid {
        let p = Process::open(p).unwrap();
        p.kill(1);
        let _ = p.wait();
    }
    drop(sup);
    pid.into_iter().collect()
}

fn state(home: &Path) -> KeyboardState {
    Registry::open(home, false).unwrap().keyboard_state(ID)
}

#[test]
fn a_grant_that_matches_the_program_survives_the_launch() {
    let (home, exe) = home_with("ok");
    let mut reg = Registry::open(&home, false).unwrap();
    let sha = hideout_supervisor::sha256_file(&exe).unwrap();
    reg.allow_keyboard(ID, &canonical_path(&exe).unwrap(), &sha, NOW)
        .unwrap();
    assert_eq!(run_once(&home).len(), 1);
    assert_eq!(state(&home), KeyboardState::Granted { at: NOW.into() });
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn nothing_is_granted_by_a_start() {
    let (home, _) = home_with("none");
    assert_eq!(run_once(&home).len(), 1);
    assert_eq!(state(&home), KeyboardState::NotAsked);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_changed_program_voids_the_grant_and_putting_it_back_does_not_revive_it() {
    let (home, exe) = home_with("hash");
    let mut reg = Registry::open(&home, false).unwrap();
    let old = hideout_supervisor::sha256_file(&exe).unwrap();
    let path = canonical_path(&exe).unwrap();
    // The grant was given for a program with another hash (the file was replaced since).
    reg.allow_keyboard(ID, &path, &"a".repeat(64), NOW).unwrap();
    assert_eq!(run_once(&home).len(), 1);
    assert!(matches!(
        state(&home),
        KeyboardState::Changed {
            reason: VoidReason::Changed,
            ..
        }
    ));
    // Another start with the program as it was: still void.
    let mut reg = Registry::open(&home, false).unwrap();
    let g = reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert_eq!(g.state, GrantState::Void);
    assert!(!g.allows(&path, &old) && !g.allows(&path, &"a".repeat(64)));
    assert_eq!(run_once(&home).len(), 1);
    assert!(matches!(state(&home), KeyboardState::Changed { .. }));
    // Only the user's own Allow turns it on again, recording the program as it is now.
    reg.allow_keyboard(ID, &path, &old, NOW).unwrap();
    assert_eq!(run_once(&home).len(), 1);
    assert_eq!(state(&home), KeyboardState::Granted { at: NOW.into() });
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_record_for_another_path_is_void_even_with_the_right_hash() {
    let (home, exe) = home_with("moved");
    let sha = hideout_supervisor::sha256_file(&exe).unwrap();
    // Another user process edits inventory.json: a record that names a different path.
    let mut reg = Registry::open(&home, false).unwrap();
    reg.allow_keyboard(ID, &canonical_path(&exe).unwrap(), &sha, NOW)
        .unwrap();
    drop(reg);
    let file = home.join("inventory.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    v["modules"][ID]["keyboard"]["path"] = serde_json::json!(r"C:\Users\x\evil\foxglove.exe");
    std::fs::write(&file, v.to_string()).unwrap();
    assert_eq!(run_once(&home).len(), 1);
    assert!(matches!(
        state(&home),
        KeyboardState::Changed {
            reason: VoidReason::Moved,
            ..
        }
    ));
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn the_running_program_cannot_be_overwritten_renamed_or_deleted() {
    let (home, exe) = home_with("pinned");
    let sup = Supervisor::start(&home, hub());
    let pid = sup.pid(ID).expect("not started");
    assert!(std::fs::write(&exe, b"x").is_err(), "overwritten");
    assert!(
        std::fs::rename(&exe, exe.with_extension("old")).is_err(),
        "renamed"
    );
    assert!(std::fs::remove_file(&exe).is_err(), "deleted");
    let p = Process::open(pid).unwrap();
    p.kill(1);
    let _ = p.wait();
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
