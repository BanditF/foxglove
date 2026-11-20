// SPDX-License-Identifier: MIT
//! R3/R7: the running exe of an app enrolled in a dev folder can be renamed aside (so a rebuild
//! can land a new build at its name), but never written; an app that is not enrolled (installed,
//! signed, adopted elsewhere) keeps the strict pin. A windowless `ping.exe` copy stands in for
//! the app; never the real Foxglove.
#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::win::Process;

const NOW: &str = "2026-11-20T12:00:00Z";
const ID: &str = "foxglove";

fn hub(home: &Path) -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

/// The app at `<base>\out\foxglove.exe`; enrolled in a dev folder when `dev`.
fn fixture(tag: &str, dev: bool) -> (PathBuf, PathBuf) {
    // Not %TEMP%: a dev folder must not be writable by other accounts.
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hideout-devren-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let out = base.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let exe = out.join("foxglove.exe");
    std::fs::copy(Path::new(&windir).join("System32").join("PING.EXE"), &exe).unwrap();
    let folder = base.join("manifest");
    std::fs::create_dir_all(&folder).unwrap();
    let run = exe.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}', '-n', '60', '127.0.0.1']\n\
             api = {{ foxglove = \"1.0\" }}\npermissions = [\"hub.notify\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    if dev {
        reg.add_dev_folder(&out.display().to_string()).unwrap();
        let list = reg.permission_list(ID).unwrap();
        let chosen = vec!["hub.notify".to_string()];
        reg.approve_listed(ID, list.code(), &chosen, &[], NOW, "dialog", false)
            .unwrap();
    }
    reg.set_enabled(ID, true).unwrap();
    (home, exe)
}

fn stop(sup: common::Reaped, pid: u32) {
    let p = Process::open(pid).unwrap();
    p.kill(1);
    let _ = p.wait();
    drop(sup);
}

#[test]
fn a_running_dev_app_can_be_renamed_aside_but_not_written() {
    let (home, exe) = fixture("dev", true);
    // Developer mode is the hub's own environment switch; the other test in this file is not an
    // enrolled app, so it stays strict whichever way this variable reads while both run.
    std::env::set_var("HIDEOUT_DEV_MODE", "1");
    let sup = common::start(&home, hub(&home));
    let pid = sup.pid(ID).expect("not started");
    assert!(std::fs::write(&exe, b"x").is_err(), "overwritten");
    std::fs::rename(&exe, exe.with_extension("old")).expect("rename aside while running");
    std::fs::write(&exe, b"new build").expect("the new build lands at the name");
    stop(sup, pid);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_running_app_outside_a_dev_folder_keeps_the_strict_pin() {
    let (home, exe) = fixture("strict", false);
    let sup = common::start(&home, hub(&home));
    let pid = sup.pid(ID).expect("not started");
    assert!(std::fs::write(&exe, b"x").is_err(), "overwritten");
    assert!(
        std::fs::rename(&exe, exe.with_extension("old")).is_err(),
        "renamed"
    );
    assert!(std::fs::remove_file(&exe).is_err(), "deleted");
    stop(sup, pid);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
