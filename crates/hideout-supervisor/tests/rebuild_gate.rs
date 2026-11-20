// SPDX-License-Identifier: MIT
//! R3: the gate in front of a rebuild restart. It must pass only for an app the window enrolled
//! under a dev folder, with developer mode on, when the carry would hold. Window-free.

use std::path::PathBuf;

use hideout_registry::Registry;
use hideout_supervisor::driver::rebuild_gate;

const NOW: &str = "2026-11-19T12:00:00Z";
const ID: &str = "foxglove";

fn temp(name: &str) -> PathBuf {
    // Not %TEMP%: a dev folder must not be writable by other accounts.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("hideout-rebuild-gate")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Fx {
    home: PathBuf,
    out: PathBuf,
    exe: PathBuf,
    reg: Registry,
}

fn manifest(exe: &std::path::Path, extra_arg: Option<&str>) -> String {
    let exe = exe
        .display()
        .to_string()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let args = extra_arg.map_or(String::new(), |a| format!(", \"{a}\""));
    format!(
        "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
         family = \"foxglove\"\nhub = \"1.0\"\nrun = [\"{exe}\"{args}]\n\
         api = {{ foxglove = \"1.0\" }}\npermissions = [\"hub.notify\"]\n"
    )
}

/// An app at `<out>\foxglove.exe` in a listed dev folder, allowed in the window (`by`).
fn fx(name: &str, by: &str) -> Fx {
    let out = temp(&format!("{name}-out"));
    let exe = out.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(folder.join("module.toml"), manifest(&exe, None)).unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.add_dev_folder(&out.display().to_string()).unwrap();
    let list = reg.permission_list(ID).unwrap();
    let chosen = vec!["hub.notify".to_string()];
    reg.approve_listed(ID, list.code(), &chosen, &[], NOW, by, false)
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    Fx {
        home,
        out,
        exe,
        reg,
    }
}

fn rebuild(f: &Fx, bytes: &[u8]) {
    std::fs::write(&f.exe, bytes).unwrap();
}

#[test]
fn an_enrolled_app_with_a_carried_build_passes() {
    let f = fx("pass", "dialog");
    rebuild(&f, b"build 2");
    assert_eq!(rebuild_gate(&f.reg, ID, Some("old"), "new"), Ok(()));
    // Not running (stopped after crashes): nothing to compare, still passes.
    assert_eq!(rebuild_gate(&f.reg, ID, None, "new"), Ok(()));
}

#[test]
fn the_build_already_running_is_not_restarted() {
    let f = fx("same", "dialog");
    assert!(rebuild_gate(&f.reg, ID, Some("ABCD"), "abcd").is_err());
}

#[test]
fn developer_mode_off_refuses_even_for_a_listed_enrolled_app() {
    let f = fx("devoff", "dialog");
    rebuild(&f, b"build 2");
    let shipped = Registry::open(&f.home, false).unwrap();
    assert_eq!(
        rebuild_gate(&shipped, ID, None, "new"),
        Err("developer mode is off")
    );
}

#[test]
fn a_cli_allow_never_enrols_so_it_never_restarts() {
    let f = fx("cli", "cli");
    rebuild(&f, b"build 2");
    assert!(rebuild_gate(&f.reg, ID, None, "new").is_err());
}

#[test]
fn a_removed_dev_folder_refuses() {
    let mut f = fx("removed", "dialog");
    rebuild(&f, b"build 2");
    let out = f.out.display().to_string();
    f.reg.remove_dev_folder(&out).unwrap();
    assert!(rebuild_gate(&f.reg, ID, None, "new").is_err());
}

#[test]
fn a_build_that_starts_differently_is_not_carried_and_does_not_restart() {
    let f = fx("args", "dialog");
    rebuild(&f, b"build 2");
    // The manifest now passes an extra argument: run[1..] changed.
    let dir = f.reg.module_dir(ID, "1.0.0");
    std::fs::write(dir.join("module.toml"), manifest(&f.exe, Some("--new"))).unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    assert!(rebuild_gate(&reg, ID, None, "new").is_err());
}

#[test]
fn an_app_moved_outside_the_folder_refuses() {
    let f = fx("moved", "dialog");
    let elsewhere = temp("moved-elsewhere");
    let exe2 = elsewhere.join("foxglove.exe");
    std::fs::write(&exe2, b"build 1").unwrap();
    let dir = f.reg.module_dir(ID, "1.0.0");
    std::fs::write(dir.join("module.toml"), manifest(&exe2, None)).unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    assert!(rebuild_gate(&reg, ID, None, "new").is_err());
}

#[test]
fn an_unknown_app_refuses() {
    let f = fx("unknown", "dialog");
    assert!(rebuild_gate(&f.reg, "nobody", None, "new").is_err());
}

// ---- R3 race: the new build is verified and pinned BEFORE anything stops -------------------

use hideout_supervisor::driver::rebuild_preflight;
use hideout_supervisor::sha256_file;

fn sha(f: &Fx) -> String {
    sha256_file(&f.exe).unwrap()
}

#[test]
fn a_settled_readable_build_passes_and_comes_back_pinned() {
    let f = fx("pf-pass", "dialog");
    rebuild(&f, b"build 2");
    let want = sha(&f);
    let pin = rebuild_preflight(&f.reg, ID, Some("old"), &want).expect("passes");
    assert_eq!(pin.sha256, want, "the pin is of the settled bytes");
}

#[test]
fn a_build_that_failed_and_left_no_program_stops_nothing() {
    let f = fx("pf-failed", "dialog");
    rebuild(&f, b"build 2");
    let want = sha(&f);
    std::fs::remove_file(&f.exe).unwrap();
    // Refused (by the gate, which no longer finds an enrolled program); nothing is pinned.
    assert!(rebuild_preflight(&f.reg, ID, Some("old"), &want).is_err());
}

#[test]
fn a_later_build_that_superseded_the_settled_one_refuses() {
    let f = fx("pf-superseded", "dialog");
    rebuild(&f, b"build 2");
    let settled = sha(&f);
    // A second build lands before the first one's check.
    rebuild(&f, b"build 3");
    let e = rebuild_preflight(&f.reg, ID, Some("old"), &settled).unwrap_err();
    assert!(e.contains("superseded"), "{e}");
    // The second build's own settle then passes.
    let again = sha(&f);
    assert!(rebuild_preflight(&f.reg, ID, Some("old"), &again).is_ok());
}

#[test]
fn a_dev_folder_removed_between_trigger_and_start_refuses() {
    let mut f = fx("pf-removed", "dialog");
    rebuild(&f, b"build 2");
    let want = sha(&f);
    assert!(rebuild_preflight(&f.reg, ID, Some("old"), &want).is_ok());
    let out = f.out.display().to_string();
    f.reg.remove_dev_folder(&out).unwrap();
    let e = rebuild_preflight(&f.reg, ID, Some("old"), &want).unwrap_err();
    assert!(e.contains("dev folder"), "{e}");
}

#[test]
fn the_program_moved_or_started_differently_between_trigger_and_start_refuses() {
    let f = fx("pf-changed", "dialog");
    rebuild(&f, b"build 2");
    let want = sha(&f);
    // Another program path in the manifest.
    let elsewhere = temp("pf-changed-elsewhere");
    let exe2 = elsewhere.join("foxglove.exe");
    std::fs::write(&exe2, b"build 2").unwrap();
    let dir = f.reg.module_dir(ID, "1.0.0");
    std::fs::write(dir.join("module.toml"), manifest(&exe2, None)).unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    assert!(rebuild_preflight(&reg, ID, Some("old"), &want).is_err());
    // Different arguments.
    std::fs::write(dir.join("module.toml"), manifest(&f.exe, Some("--new"))).unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    assert!(rebuild_preflight(&reg, ID, Some("old"), &want).is_err());
}

#[test]
fn the_build_already_running_and_developer_mode_off_refuse_before_any_file_is_read() {
    let f = fx("pf-gate", "dialog");
    rebuild(&f, b"build 2");
    let want = sha(&f);
    assert!(rebuild_preflight(&f.reg, ID, Some(&want), &want).is_err());
    let shipped = Registry::open(&f.home, false).unwrap();
    assert!(rebuild_preflight(&shipped, ID, None, &want).is_err());
}

#[test]
fn a_refusal_is_total_for_every_unreadable_state() {
    // Fail closed: an empty settled hash, an unknown app and a missing program all refuse.
    let f = fx("pf-closed", "dialog");
    rebuild(&f, b"build 2");
    assert!(rebuild_preflight(&f.reg, ID, None, "").is_err());
    assert!(rebuild_preflight(&f.reg, "nobody", None, &sha(&f)).is_err());
}
