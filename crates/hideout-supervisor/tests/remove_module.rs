// SPDX-License-Identifier: MIT
//! "Remove from Hideout" through the real driver (product slice 6): a module that runs is
//! stopped first and then removed from the registry (its folder under `modules\` goes, nothing
//! else); one that doesn't run is removed at once. Windowless: a copy of `ping.exe` installed as
//! a developer-mode module; every process the test starts is ended by its handle.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::win::Process;

const NOW: &str = "2026-11-19T00:00:00Z";

fn home_with(id: &str, enabled: bool) -> PathBuf {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-rm-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("src");
    std::fs::create_dir_all(&folder).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::fs::copy(
        Path::new(&windir).join("System32").join("PING.EXE"),
        folder.join("ping.exe"),
    )
    .unwrap();
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"Gone\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"ping.exe\", \"-n\", \"60\", \"127.0.0.1\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&folder, NOW).unwrap();
    reg.set_enabled(id, enabled).unwrap();
    home
}

fn hub() -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    Hub::new(router, events)
}

fn installed(home: &Path, id: &str) -> bool {
    Registry::open(home, true)
        .unwrap()
        .inventory
        .modules
        .contains_key(id)
}

#[test]
fn a_running_module_is_stopped_first_and_then_removed() {
    let id = format!("example.lab.rm-run-{}", std::process::id());
    let home = home_with(&id, true);
    let sup = Supervisor::start(&home, hub());
    let pid = sup.pid(&id).expect("the module was not started");
    let proc = Process::open(pid).unwrap();
    assert!(installed(&home, &id));

    sup.remove(&id).unwrap();
    // Stopped (the handle's wait is the event), then removed when its instance ends.
    let _ = proc.wait();
    assert!(!proc.running(), "the module was not stopped");
    let limit = Instant::now() + Duration::from_secs(15);
    while installed(&home, &id) && Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!installed(&home, &id), "still in the inventory");
    assert!(!home.join("modules").join(&id).exists());
    assert!(!sup.knows(&id));
    // It never comes back: nothing restarts it.
    assert_eq!(sup.pid(&id), None);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_module_that_is_off_is_removed_at_once_and_a_second_remove_finds_nothing() {
    let id = format!("example.lab.rm-off-{}", std::process::id());
    let home = home_with(&id, false);
    let sup = Supervisor::start(&home, hub());
    assert_eq!(sup.pid(&id), None);
    sup.remove(&id).unwrap();
    assert!(!installed(&home, &id));
    assert!(!home.join("modules").join(&id).exists());
    assert!(sup.remove(&id).is_err());
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
