// SPDX-License-Identifier: MIT
//! A module started by a lab hub connects to that hub (#1010): the hub on a folder of its own
//! serves `\\.\pipe\hideout-<user><suffix>`, never the user's `\\.\pipe\hideout-<user>`, and the
//! supervisor names that pipe in `HIDEOUT_PIPE`. The module is a copy of `cmd.exe` running a
//! script that opens `%HIDEOUT_PIPE%`, as the SDKs' `connect_env` does, and says `initialize`
//! with its launch token and `module.ready`. Windowless; the module's job is ended at the end.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, router_pipe_name, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::win::Job;

const NOW: &str = "2026-11-10T00:00:00Z";
const ID: &str = "example.lab.pipe";

/// What the module says on `%HIDEOUT_PIPE%`; it then holds the pipe open for up to 30 s.
const SCRIPT: &str = "@echo off\r\n\
(echo {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"id\":\"example.lab.pipe\",\
\"version\":\"0.1.0\",\"instance\":\"%HIDEOUT_INSTANCE%\",\"token\":\"%HIDEOUT_TOKEN%\",\"hub\":\"1.0\",\
\"api\":{}}}& echo {\"jsonrpc\":\"2.0\",\"method\":\"module.ready\",\"params\":{}}& \
ping -n 30 127.0.0.1 >nul) > \"%HIDEOUT_PIPE%\"\r\n";

/// A lab hub folder with the module installed (developer mode) and enabled.
fn lab_home() -> PathBuf {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-labpipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("src");
    std::fs::create_dir_all(&folder).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::fs::copy(
        Path::new(&windir).join("System32").join("cmd.exe"),
        folder.join("cmd.exe"),
    )
    .unwrap();
    std::fs::write(folder.join("module.cmd"), SCRIPT).unwrap();
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Lab pipe\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"cmd.exe\", \"/d\", \"/c\", '.\\module.cmd']\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&folder, NOW).unwrap();
    reg.set_enabled(ID, true).unwrap();
    home
}

#[test]
fn a_module_started_by_a_lab_hub_connects_to_that_hub() {
    let home = lab_home();
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open(&home, true).unwrap());
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    let pipe = hub.router_pipe();
    assert_ne!(
        pipe,
        router_pipe_name(),
        "the lab hub took the user's pipe name"
    );
    hub.serve_pipe(&pipe).unwrap();

    let sup = Supervisor::start(&home, hub.clone());
    assert!(sup.pid(ID).is_some(), "the module was not started");
    let rec = Instances::load(&home).modules[ID].clone();
    // Stop event: `module.ready` from the module on the lab pipe; 20 s only as the fallback.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(Out::Ready(m)) if m == ID => {
                ready = true;
                break;
            }
            Ok(Out::Refused(m)) if m == ID => panic!("the lab hub refused its own module"),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let log = std::fs::read_to_string(home.join("logs").join(format!("{ID}.log")));
    // The job is the lab hub's own too: its name carries the folder suffix.
    assert!(rec.job.contains(&hub.folder_suffix()), "job {}", rec.job);
    if let Ok(job) = Job::open(&rec.job) {
        job.kill(1);
    }
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
    assert!(
        ready,
        "the module never said module.ready on {pipe}; its log: {log:?}"
    );
}
