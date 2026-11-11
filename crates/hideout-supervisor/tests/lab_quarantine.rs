// SPDX-License-Identifier: MIT
//! #1104: a running module the panel host quarantines (the lab hub's `quarantine` verb, #1100)
//! stops and reads `quarantined`; Start again starts a new instance that reaches `running`.
//! The module is a copy of `cmd.exe` that says `initialize` and `module.ready` and asks for no
//! host service, so a start can only fail in the quarantine → Start again path itself (not a
//! missing `net.*`, #1102). Windowless; every job is ended at the end.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::win::Job;

const ID: &str = "example.lab.quarantine";
const STOP: &str = "--test-stop--";
const WAIT: Duration = Duration::from_secs(20);

/// Says `initialize` and `module.ready` on `%HIDEOUT_PIPE%`, then holds it open up to 30 s.
const SCRIPT: &str = "@echo off\r\n\
(echo {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"id\":\"example.lab.quarantine\",\
\"version\":\"0.1.0\",\"instance\":\"%HIDEOUT_INSTANCE%\",\"token\":\"%HIDEOUT_TOKEN%\",\"hub\":\"1.0\",\
\"api\":{}}}& echo {\"jsonrpc\":\"2.0\",\"method\":\"module.ready\",\"params\":{}}& \
ping -n 30 127.0.0.1 >nul) > \"%HIDEOUT_PIPE%\"\r\n";

fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-lab-quarantine-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(LAB_MODULES_DIR)).unwrap();
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

fn lab_module(home: &Path) {
    let dir = home.join(LAB_MODULES_DIR).join(ID);
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
            "id = \"{ID}\"\nname = \"Quarantine\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"cmd.exe\", \"/d\", \"/c\", '.\\module.cmd']\n"
        ),
    )
    .unwrap();
}

#[test]
fn start_again_after_a_quarantine_runs_a_new_instance() {
    let home = lab_home();
    lab_module(&home);
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(&home).unwrap());
    let (events, rx) = mpsc::channel();
    let stop = events.clone();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    hub.serve_pipe(&hub.router_pipe()).unwrap();
    let sup = Supervisor::start(&home, hub.clone());
    let forward = {
        let sup = sup.clone();
        std::thread::spawn(move || {
            while let Ok(o) = rx.recv() {
                if matches!(&o, Out::Ready(m) if m == STOP) {
                    break;
                }
                sup.on_router(&o);
            }
        })
    };

    let running = sup.lab_wait_for(ID, "running", WAIT);
    let first_pid = sup.pid(ID);
    // The panel host's quarantine reaches the supervisor through the hub's hand-off.
    sup.quarantine(ID);
    let quarantined = sup.lab_wait_for(ID, "quarantined", WAIT);
    // Its exit is no crash: it stays quarantined.
    std::thread::sleep(Duration::from_millis(500));
    let still = sup.lab_state(ID);
    // Start again on the card.
    sup.start_again(ID);
    let again = sup.lab_wait_for(ID, "running", Duration::from_secs(30));
    let second_pid = sup.pid(ID);

    if let Some(job) = Instances::load(&home)
        .modules
        .get(ID)
        .and_then(|r| Job::open(&r.job).ok())
    {
        job.kill(1);
    }
    let _ = stop.send(Out::Ready(STOP.into()));
    let _ = forward.join();
    let log = std::fs::read_to_string(home.join("logs").join(format!("{ID}.log")));
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());

    assert_eq!(running.as_deref(), Ok("running"), "its log: {log:?}");
    assert_eq!(
        quarantined.as_deref(),
        Ok("quarantined"),
        "its log: {log:?}"
    );
    assert_eq!(still, "quarantined", "its log: {log:?}");
    assert_eq!(again.as_deref(), Ok("running"), "its log: {log:?}");
    assert!(
        first_pid.is_some() && second_pid.is_some() && first_pid != second_pid,
        "Start again didn't start a new instance ({first_pid:?}, {second_pid:?}); log: {log:?}"
    );
}
