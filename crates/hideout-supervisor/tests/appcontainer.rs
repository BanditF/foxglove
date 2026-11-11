// SPDX-License-Identifier: MIT
//! AppContainer isolation for modules (contract §7.7, #983), end to end and window-free: a lab
//! hub with `[supervisor] appcontainer = true` starts a real module (`hideout-ac-probe`, which
//! connects with the SDK's `connect_env`) and the test checks that
//! - it says `module.ready` on the router pipe (the pipe admits its container),
//! - its token says `TokenIsAppContainer`, with the capability its manifest's `device.hid:`
//!   permission maps to,
//! - it can't read a file of the user's outside its folder.
//!
//! The same hub without the setting starts it outside a container. The module's job and its
//! AppContainer profile are removed at the end.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::sandbox;
use hideout_supervisor::win::{Caps, Job, Process};

const NOW: &str = "2026-11-10T00:00:00Z";

/// A lab hub folder with the probe installed (developer mode) and enabled; `contain` sets the
/// hub's AppContainer setting. Returns the hub folder and the file outside the module's folder.
fn lab_home(id: &str, contain: bool) -> (PathBuf, PathBuf) {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-ac-{}-{contain}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("src");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_hideout-ac-probe"),
        folder.join("probe.exe"),
    )
    .unwrap();
    let outside = base.join("outside.txt");
    std::fs::write(&outside, "the user's").unwrap();
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"AppContainer probe\"\nversion = \"0.1.0\"\n\
             kind = \"service\"\nfamily = \"third-party\"\nhub = \"1.0\"\n\
             visibility = \"private\"\npermissions = [\"device.hid:keyboard\"]\n\
             run = [\"probe.exe\", \"{id}\", '{}']\n",
            outside.display()
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&folder, NOW).unwrap();
    reg.set_enabled(id, true).unwrap();
    if contain {
        let toml = home.join("hideout.toml");
        let mut text = std::fs::read_to_string(&toml).unwrap_or_default();
        text.push_str("\n[supervisor]\nappcontainer = true\n");
        std::fs::write(&toml, text).unwrap();
    }
    (home, outside)
}

const ID_ON: &str = "example.lab.ac-on";
const ID_OFF: &str = "example.lab.ac-off";

/// Waits for `module.ready` from `id` (the stop event); 30 s only as the fallback.
fn ready(rx: &Receiver<Out>, id: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(Out::Ready(m)) if m == id => return true,
            Ok(Out::Refused(m)) if m == id => panic!("the hub refused its own module"),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    false
}

struct Run {
    home: PathBuf,
    hub: Hub,
    sup: Supervisor,
    rx: Receiver<Out>,
}

fn start(id: &str, contain: bool) -> Run {
    let (home, _) = lab_home(id, contain);
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open(&home, true).unwrap());
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    hub.serve_pipe(&hub.router_pipe()).unwrap();
    let sup = Supervisor::start(&home, hub.clone());
    Run { home, hub, sup, rx }
}

fn finish(r: Run, id: &str) -> String {
    let log =
        std::fs::read_to_string(r.home.join("logs").join(format!("{id}.log"))).unwrap_or_default();
    if let Some(rec) = Instances::load(&r.home).modules.get(id) {
        if let Ok(job) = Job::open(&rec.job) {
            job.kill(1);
        }
    }
    drop(r.sup);
    sandbox::delete_profile(id);
    let _ = std::fs::remove_dir_all(r.home.parent().unwrap());
    log
}

#[test]
fn a_contained_module_connects_and_is_in_its_container() {
    let r = start(ID_ON, true);
    let pid = r.sup.pid(ID_ON).expect("the module was not started");
    let p = Process::open(pid).unwrap();
    let inside = sandbox::is_app_container(&p);
    let caps = sandbox::capabilities(&p);
    let own = sandbox::container_sid(&p);
    let hid = sandbox::capability_sid_string("humaninterfacedevice").unwrap();
    let guests = r.hub.pipe_guests();
    let up = ready(&r.rx, ID_ON);
    // The probe writes its lines before it is ready; read them once it is.
    let log = finish(r, ID_ON);
    assert!(
        up,
        "no module.ready from the contained module; its log: {log:?}"
    );
    assert!(inside.unwrap(), "not in an AppContainer");
    assert_eq!(caps.unwrap(), vec![hid], "its token's capabilities");
    assert!(
        guests.len() == 1 && guests[0].starts_with("S-1-15-2-"),
        "the router pipe's guests: {guests:?}"
    );
    // What a restarted hub reads to let a survivor back in: the same SID.
    assert_eq!(own.unwrap(), Some(guests[0].clone()));
    assert!(
        log.contains("probe: outside file: refused"),
        "it read a file outside its folder: {log:?}"
    );
}

#[test]
fn without_the_setting_it_runs_outside() {
    let r = start(ID_OFF, false);
    let pid = r.sup.pid(ID_OFF).expect("the module was not started");
    let inside = sandbox::is_app_container(&Process::open(pid).unwrap());
    let up = ready(&r.rx, ID_OFF);
    let guests = r.hub.pipe_guests();
    let log = finish(r, ID_OFF);
    assert!(up, "no module.ready; its log: {log:?}");
    assert!(!inside.unwrap(), "in an AppContainer without the setting");
    assert!(guests.is_empty());
    assert!(log.contains("probe: outside file: read"), "{log:?}");
}

/// The router pipe admits a container's SID only once the hub allows it, on the instance that
/// was already waiting and on every one after.
#[test]
fn the_router_pipe_admits_a_container_only_once_allowed() {
    const ID: &str = "example.lab.ac-pipe";
    let base = std::env::temp_dir().join(format!("hideout-ac-pipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let exe = base.join("probe.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_hideout-ac-probe"), &exe).unwrap();
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&format!(".actest{}", std::process::id()));
    let pipe = hub.router_pipe();
    hub.serve_pipe(&pipe).unwrap();
    let c = sandbox::Container::for_module(ID, &[]).unwrap();
    c.grant(&base, false).unwrap();
    let env = [
        ("HIDEOUT_PIPE", pipe.clone()),
        ("HIDEOUT_TOKEN", "t".to_string()),
        ("HIDEOUT_INSTANCE", "1".to_string()),
    ];
    let job = Job::create(
        &Job::name_for("test", ID, std::process::id() as u64),
        Caps::default(),
    )
    .unwrap();
    let run = || {
        let (p, _) = c
            .spawn(
                &job,
                &exe,
                &[ID.into(), "-".into(), "--exit".into()],
                &base,
                &env,
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(base.join("probe.log"))
                    .ok(),
            )
            .unwrap()
            .resume()
            .unwrap();
        assert!(sandbox::is_app_container(&p).unwrap());
        p.wait().unwrap()
    };
    let before = run();
    hub.allow_pipe_client(&c.sid_string).unwrap();
    hub.allow_pipe_client(&c.sid_string).unwrap(); // twice: one guest
    let after = [run(), run()];
    let guests = hub.pipe_guests();
    let log = std::fs::read_to_string(base.join("probe.log")).unwrap_or_default();
    job.kill(1);
    drop(c);
    sandbox::delete_profile(ID);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(
        before, 3,
        "the container opened the pipe before it was allowed"
    );
    assert_eq!(
        after,
        [0, 0],
        "the container couldn't open the pipe once allowed: {log}"
    );
    assert_eq!(guests.len(), 1);
}
