// SPDX-License-Identifier: MIT
//! #1096: the lab hub's update verbs drive a running lab module into Update ready, Update
//! waiting for you and Updating… through the policy's update step and a real `module.drain`
//! call over the module's pipe; Not now and the swap's restart follow as usual. Outside lab mode,
//! or for a module that isn't a lab module, they are refused. The module is a copy of `cmd.exe`
//! that says `initialize` and `module.ready` and answers nothing (as the first-party plugins have
//! no drain handler); the router's call timeout is short, so its drain "answer" (an error) comes
//! quickly. Windowless; every job is ended at the end.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::lab::LabUpdate;
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::win::Job;

const ID: &str = "example.lab.swap";
const STOP: &str = "--test-stop--";
const WAIT: Duration = Duration::from_secs(20);

/// Says `initialize` and `module.ready` on `%HIDEOUT_PIPE%`, then holds it open up to 30 s.
const SCRIPT: &str = "@echo off\r\n\
(echo {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"id\":\"example.lab.swap\",\
\"version\":\"0.1.0\",\"instance\":\"%HIDEOUT_INSTANCE%\",\"token\":\"%HIDEOUT_TOKEN%\",\"hub\":\"1.0\",\
\"api\":{}}}& echo {\"jsonrpc\":\"2.0\",\"method\":\"module.ready\",\"params\":{}}& \
ping -n 30 127.0.0.1 >nul) > \"%HIDEOUT_PIPE%\"\r\n";

fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-lab-verbs-{}", std::process::id()))
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
            "id = \"{ID}\"\nname = \"Swap\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"cmd.exe\", \"/d\", \"/c\", '.\\module.cmd']\n"
        ),
    )
    .unwrap();
}

fn fact(hub: &Hub, key: &str) -> Option<String> {
    hub.read(|r| r.fact(key).and_then(|v| v.as_str().map(String::from)))
}

#[test]
fn each_update_verb_reaches_its_state_and_is_refused_outside_lab() {
    let home = lab_home();
    lab_module(&home);
    let settings = Settings {
        call_timeout: Duration::from_millis(300),
        ..Settings::default()
    };
    let mut router = Router::new(settings, Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(&home).unwrap());
    let (events, rx) = mpsc::channel();
    let stop = events.clone();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    hub.serve_pipe(&hub.router_pipe()).unwrap();
    let sup = Supervisor::start(&home, hub.clone());
    // The hub's events thread: every router event to the supervisor, until the stop event.
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

    let not_lab = sup.lab_update("example.not-here", LabUpdate::Updating, WAIT);
    let running = sup.lab_wait_for(ID, "running", WAIT);
    let first_pid = sup.pid(ID);
    let state = |h: &Hub| fact(h, &format!("module.{ID}.state"));
    let bad_permission = sup.lab_update(
        ID,
        LabUpdate::Waiting {
            adds: vec!["not a permission".into()],
        },
        WAIT,
    );
    let no_permission = sup.lab_update(ID, LabUpdate::Waiting { adds: vec![] }, WAIT);

    // Update ready: a busy drain answer.
    let ready = sup.lab_update(
        ID,
        LabUpdate::Ready {
            reason: "a flash running".into(),
        },
        WAIT,
    );
    let ready_fact = state(&hub);
    let ready_reason = fact(&hub, &format!("module.{ID}.reason"));
    sup.not_now(ID);
    let after_not_now = sup.lab_wait_for(ID, "running", WAIT);

    // Update waiting for you: an update that adds a permission.
    let waiting = sup.lab_update(
        ID,
        LabUpdate::Waiting {
            adds: vec!["devices.read".into()],
        },
        WAIT,
    );
    let waiting_fact = state(&hub);
    sup.not_now(ID);
    let after_waiting = sup.lab_wait_for(ID, "running", WAIT);

    // Updating…: a drain that answers done; the old instance exits and a new one starts.
    let updating = sup.lab_update(ID, LabUpdate::Updating, WAIT);
    let updating_fact = state(&hub);
    let restarted = sup.lab_wait_for(ID, "running", Duration::from_secs(30));
    let second_pid = sup.pid(ID);

    // Outside lab mode: refused, nothing staged.
    std::env::remove_var("HIDEOUT_LAB");
    let outside = sup.lab_update(ID, LabUpdate::Updating, WAIT);
    let outside_check = sup.lab_check(ID);
    let still = sup.lab_state(ID);

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

    assert!(
        not_lab
            .as_ref()
            .is_err_and(|e| e.contains("not a lab module")),
        "{not_lab:?}"
    );
    assert_eq!(running.as_deref(), Ok("running"), "its log: {log:?}");
    assert!(bad_permission.is_err() && no_permission.is_err());
    assert_eq!(ready.as_deref(), Ok("update-busy"));
    assert_eq!(ready_fact.as_deref(), Some("update-busy"));
    assert!(
        ready_reason
            .as_deref()
            .is_some_and(|r| r.contains("a flash running")),
        "{ready_reason:?}"
    );
    assert_eq!(after_not_now.as_deref(), Ok("running"));
    assert_eq!(waiting.as_deref(), Ok("update-waiting"));
    assert_eq!(waiting_fact.as_deref(), Some("update-waiting"));
    assert_eq!(after_waiting.as_deref(), Ok("running"));
    assert_eq!(updating.as_deref(), Ok("updating"));
    assert_eq!(updating_fact.as_deref(), Some("updating"));
    assert_eq!(restarted.as_deref(), Ok("running"), "its log: {log:?}");
    assert!(
        first_pid.is_some() && second_pid.is_some() && first_pid != second_pid,
        "the swap didn't start a new instance ({first_pid:?}, {second_pid:?})"
    );
    assert!(
        outside.as_ref().is_err_and(|e| e.contains("lab hub only")),
        "{outside:?}"
    );
    assert!(outside_check.is_err());
    assert_eq!(still, "running");
}
