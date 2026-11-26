// SPDX-License-Identifier: MIT
//! An OK for one run is for one counted start (#1452): when the start can't be written down,
//! the launch is refused and waits for the OK again, instead of running on an OK that would
//! stay unused. With a real windowless process (a copy of `ping.exe` under a script runner's
//! file name, registered as an adopted app).
#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::inventory::RunState;
use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::set_launch_hook;
use hideout_supervisor::win::Process;

const NOW: &str = "2026-11-25T00:00:00Z";

fn adopted_home(name: &str) -> (PathBuf, String) {
    let id = format!("example.lab.onerun{name}-{}", std::process::id());
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-onerun-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let install = base.join("install");
    let folder = base.join("manifest");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    // The file name is a script runner's, so its OK is for one run.
    let exe = install.join("python.exe");
    std::fs::copy(Path::new(&windir).join("System32").join("PING.EXE"), &exe).unwrap();
    let run = exe.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"Runner\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"arete\"\nhub = \"1.0\"\nrun = [\"{run}\", \"-n\", \"60\", \"127.0.0.1\"]\n\
             api = {{ arete = \"1.0\" }}\npermissions = [\"hub.notify\", \"ui.window\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    (home, id)
}

fn hub() -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    Hub::new(router, events)
}

fn state(home: &Path, id: &str) -> Option<RunState> {
    Registry::open_from_env(home).unwrap().inventory.modules[id]
        .one_run
        .as_ref()
        .map(|r| r.state)
}

fn allow_all(sup: &common::Reaped, home: &Path, id: &str) {
    // An OK that went void first changes the list; the second try is for the list as it is.
    let mut last = String::new();
    for _ in 0..2 {
        let list = Registry::open_from_env(home)
            .unwrap()
            .permission_list(id)
            .unwrap();
        let all: Vec<String> = list.grantable().map(|i| i.raw.clone()).collect();
        match sup.grant_listed(id, list.code(), &all, &[], "dialog") {
            Ok(_) => return,
            Err(e) => last = e,
        }
    }
    panic!("the Allow was not taken: {last}");
}

fn end(sup: &common::Reaped, id: &str) {
    let running = sup.pid(id).map(|pid| Process::open(pid).unwrap());
    sup.turn_off(id);
    if let Some(p) = running {
        p.kill(1);
        let _ = p.wait();
    }
}

#[test]
fn a_start_that_cannot_be_counted_does_not_run_and_asks_again() {
    let (home, id) = adopted_home("count");
    let id = id.as_str();
    let hub = hub();
    let sup = common::start(&home, hub.clone());
    let fact = |k: &str| hub.read(|r| r.fact(&format!("module.{id}.{k}")).cloned());
    assert!(Registry::open_from_env(&home)
        .unwrap()
        .one_run(id)
        .is_some());

    allow_all(&sup, &home, id);
    assert_eq!(state(&home, id), Some(RunState::Allowed));
    sup.turn_on(id).expect("turn_on after the Allow");
    assert!(sup.pid(id).is_some(), "the counted start runs");
    assert_eq!(state(&home, id), Some(RunState::Started));
    end(&sup, id);

    // The OK is used. Now the count of the next start can't be written (a folder is where the
    // inventory's new copy goes, from just before the start is counted), so the file goes on
    // saying `started`: that must not read as a start that was counted.
    let blocker = home.join("inventory.json.tmp");
    let block = blocker.clone();
    set_launch_hook(Some(Box::new(move |stage| {
        if stage == "before-count" {
            std::fs::create_dir_all(&block).unwrap();
        }
    })));
    let turned_on = sup.turn_on(id);
    // However often the hub tries, nothing runs.
    for _ in 0..3 {
        sup.feed(hideout_supervisor::policy::Ev::Enable { id: id.into() });
        assert!(sup.pid(id).is_none(), "a second start on one OK");
    }
    set_launch_hook(None);
    assert!(blocker.is_dir(), "the launch never reached the count");
    assert!(sup.pid(id).is_none(), "a second start on one OK");
    assert_eq!(state(&home, id), Some(RunState::Started));
    // Refused as waiting for the OK, not as a crash.
    assert_eq!(
        fact("failed"),
        Some(serde_json::json!("ok")),
        "{turned_on:?}"
    );
    std::fs::remove_dir_all(&blocker).unwrap();

    // With the inventory writable again the OK is still used up: the next start ends it.
    let again = sup.turn_on(id);
    assert!(sup.pid(id).is_none(), "a second start on one OK: {again:?}");
    assert_eq!(state(&home, id), Some(RunState::Ended));

    // A new Allow is a new start.
    allow_all(&sup, &home, id);
    sup.turn_on(id).expect("turn_on after the second Allow");
    assert!(sup.pid(id).is_some(), "the start after a new Allow runs");
    end(&sup, id);

    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
