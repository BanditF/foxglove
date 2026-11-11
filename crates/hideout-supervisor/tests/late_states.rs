// SPDX-License-Identifier: MIT
//! Late modules (#1076) and the states of #983: a module that joins the inventory after the
//! hub started is read like one found at the start. Its manifest needing a newer hub makes it
//! `needs-hub`, never started; one that `requires` a module that is down starts and, once
//! ready, reads `waiting` for that module (§4.4, §7.8). Lab modules are copies of `cmd.exe`;
//! the one that runs says `initialize` and `module.ready` on its pipe as lab_pipe.rs's does.
//! Windowless; every job is ended at the end.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::win::Job;

const OLD_HUB: &str = "example.lab.newer-hub";
const NEEDS: &str = "example.lab.needs";

/// Says `initialize` and `module.ready` on `%HIDEOUT_PIPE%`, then holds it open up to 30 s.
const SCRIPT: &str = "@echo off\r\n\
(echo {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"id\":\"example.lab.needs\",\
\"version\":\"0.1.0\",\"instance\":\"%HIDEOUT_INSTANCE%\",\"token\":\"%HIDEOUT_TOKEN%\",\"hub\":\"1.0\",\
\"api\":{}}}& echo {\"jsonrpc\":\"2.0\",\"method\":\"module.ready\",\"params\":{}}& \
ping -n 30 127.0.0.1 >nul) > \"%HIDEOUT_PIPE%\"\r\n";

fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-late-states-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(LAB_MODULES_DIR)).unwrap();
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

fn lab_module(home: &Path, id: &str, hub: &str, extra: &str, run: &str) {
    let dir = home.join(LAB_MODULES_DIR).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::fs::copy(
        Path::new(&windir).join("System32").join("cmd.exe"),
        dir.join("cmd.exe"),
    )
    .unwrap();
    std::fs::write(dir.join("module.cmd"), SCRIPT).unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"{hub}\"\nvisibility = \"private\"\n{extra}\
             run = {run}\n"
        ),
    )
    .unwrap();
}

fn fact(hub: &Hub, key: &str) -> Option<String> {
    hub.read(|r| r.fact(key).and_then(|v| v.as_str().map(String::from)))
}

#[test]
fn late_modules_read_needs_hub_and_waiting() {
    let home = lab_home();
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(&home).unwrap());
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    hub.serve_pipe(&hub.router_pipe()).unwrap();
    let sup = Supervisor::start(&home, hub.clone());

    // Installed while the hub served its `hub`, then the hub went back a version (here: its
    // installed manifest asks for a newer one). Joins: known, needs-hub, not started.
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let src = home.parent().unwrap().join("src");
    lab_module(
        &src,
        OLD_HUB,
        "1.0",
        "",
        "[\"cmd.exe\", \"/d\", \"/c\", \"exit 0\"]",
    );
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(
        &src.join(LAB_MODULES_DIR).join(OLD_HUB),
        "2026-11-10T00:00:00Z",
    )
    .unwrap();
    reg.set_enabled(OLD_HUB, true).unwrap();
    let toml = reg.module_dir(OLD_HUB, "0.1.0").join("module.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, text.replace("hub = \"1.0\"", "hub = \"9.9\"")).unwrap();
    assert!(matches!(sup.learn(OLD_HUB), Some(None)), "not learnt");
    let old_state = fact(&hub, &format!("module.{OLD_HUB}.state"));
    let old_pid = sup.pid(OLD_HUB);

    // Joins requiring it: it starts, and once ready it waits for the root.
    lab_module(
        &home,
        NEEDS,
        "1.0",
        &format!("requires = [\"{OLD_HUB}\"]\n"),
        "[\"cmd.exe\", \"/d\", \"/c\", '.\\module.cmd']",
    );
    let learnt = sup.learn(NEEDS);
    let started = sup.pid(NEEDS).is_some();
    // Stop event: module.ready forwarded to the supervisor; 20 s only as the fallback.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(o) => {
                sup.on_router(&o);
                if matches!(&o, Out::Ready(m) if m == NEEDS) {
                    ready = true;
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let state = fact(&hub, &format!("module.{NEEDS}.state"));
    let waiting = fact(&hub, &format!("module.{NEEDS}.waiting"));
    let log = std::fs::read_to_string(home.join("logs").join(format!("{NEEDS}.log")));
    for id in [OLD_HUB, NEEDS] {
        if let Some(job) = Instances::load(&home)
            .modules
            .get(id)
            .and_then(|r| Job::open(&r.job).ok())
        {
            job.kill(1);
        }
    }
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());

    assert_eq!(old_state.as_deref(), Some("needs-hub"));
    assert!(
        old_pid.is_none(),
        "a module needing a newer hub was started"
    );
    assert!(
        learnt.is_some_and(|m| m.is_some()),
        "the second wasn't learnt"
    );
    assert!(started, "the second wasn't started");
    assert!(ready, "no module.ready from it; its log: {log:?}");
    assert_eq!(state.as_deref(), Some("waiting"));
    assert_eq!(
        waiting.as_deref(),
        Some(OLD_HUB),
        "Waiting names the wrong root"
    );
}
