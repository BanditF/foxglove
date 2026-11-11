// SPDX-License-Identifier: MIT
//! Activation by a fact and per-module caps (#983, contract §4.6 and §7.7, v1.10), through the
//! real driver on a lab hub. The lab module's exe is a copy of the system's `ping.exe`: a real
//! child with no window (the supervisor starts it with CREATE_NO_WINDOW) that never connects.
//! It is not started before its fact, started once after it (a second event while it starts
//! does nothing), and its Job Object holds its manifest's `[limits]` clamped to the hub's
//! maximum, read back from Windows. Turn off ends it.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hideout_contract::module::ResourceLimits;
use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::policy::{Cmd, Ev, Policy};
use hideout_supervisor::win::{Caps, Job, Process};
use serde_json::{json, Value};

const ID: &str = "example.lab.pinger";

/// A lab hub folder with one lab module that waits for `fact:lab.go` and asks for 64 MiB and
/// 80 % CPU (over the hub's 50 %).
fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-activation-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    let dir = home.join(LAB_MODULES_DIR).join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    let ping = PathBuf::from(std::env::var("SystemRoot").unwrap_or(r"C:\Windows".into()))
        .join(r"System32\PING.EXE");
    std::fs::copy(&ping, dir.join("pinger.exe")).unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Lab pinger\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.2\"\nvisibility = \"private\"\n\
             run = [\"pinger.exe\", \"-n\", \"60\", \"127.0.0.1\"]\n\
             [activation]\non = [\"fact:lab.go\"]\n\
             [limits]\nmemory_mb = 64\ncpu_percent = 80\n"
        ),
    )
    .unwrap();
    // Lab mode: HIDEOUT_LAB=1 with HIDEOUT_CONFIG_DIR on this folder (this test binary only).
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

fn hub(home: &std::path::Path) -> (Hub, mpsc::Receiver<Out>) {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(home).unwrap());
    let _ = router.watch_facts(vec![String::new()]);
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    (hub, rx)
}

fn state(hub: &Hub) -> Value {
    hub.read(|r| r.fact(&format!("module.{ID}.state")).cloned())
        .unwrap_or(Value::Null)
}

/// Hands the router's events to the supervisor (as the hub's events thread does) until
/// `module.<id>.state` is `want`, counting the `Activate`s for the module. Start event: now;
/// stop event: that fact on the channel; 10 s is the fallback only.
fn until_state(hub: &Hub, rx: &mpsc::Receiver<Out>, sup: &Supervisor, want: &str) -> usize {
    let key = format!("module.{ID}.state");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut activations = 0;
    while state(hub) != json!(want) {
        let left = deadline.saturating_duration_since(Instant::now());
        let o = rx
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("state {:?}, want {want}", state(hub)));
        if matches!(&o, Out::Activate(id) if id == ID) {
            activations += 1;
        }
        sup.on_router(&o);
        if matches!(&o, Out::Fact { key: k, value } if *k == key && *value == json!(want)) {
            break;
        }
    }
    activations
}

#[test]
fn a_fact_starts_the_module_once_in_a_job_with_its_clamped_caps() {
    let home = lab_home();
    let (h, rx) = hub(&home);
    let sup = Supervisor::start(&home, h.clone());
    h.act(|r| r.activation_due(None));
    let early: Vec<Out> = rx.try_iter().collect();
    for o in &early {
        sup.on_router(o);
    }
    assert!(
        !early.iter().any(|o| matches!(o, Out::Activate(_))),
        "activated before its fact: {early:?}"
    );
    assert_eq!(sup.pid(ID), None, "started before its fact");

    // The fact: one Activate, one launch.
    h.act(|r| r.publish("lab.go", json!(true)));
    assert_eq!(until_state(&h, &rx, &sup, "starting"), 1);
    let pid = sup.pid(ID).expect("started by its fact");

    // Another event while it starts (it never connects): the router asks again, the
    // supervisor starts nothing new.
    h.act(|r| r.publish("lab.go", json!(false)));
    h.act(|r| r.publish("lab.go", json!(true)));
    let again: Vec<Out> = rx.try_iter().collect();
    assert!(again
        .iter()
        .any(|o| matches!(o, Out::Activate(id) if id == ID)));
    for o in &again {
        sup.on_router(o);
    }
    assert_eq!(sup.pid(ID), Some(pid), "a second instance started");
    assert_eq!(Instances::load(&home).modules.len(), 1);

    // The Job Object, read back from Windows: 64 MiB as asked, CPU clamped to the hub's 50 %.
    let rec = Instances::load(&home).modules[ID].clone();
    assert_eq!(rec.pid, pid);
    let job = Job::open(&rec.job).expect("the module's job");
    assert_eq!(
        job.caps().unwrap(),
        Caps {
            memory: 64 << 20,
            cpu_rate: 5000
        }
    );
    assert!(job.holds(&Process::open(pid).unwrap()).unwrap());

    sup.turn_off(ID);
    until_state(&h, &rx, &sup, "off");
    drop(job);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn caps_are_the_manifest_limits_clamped_to_the_hub_maximum() {
    let max = Caps::HUB_MAX;
    assert_eq!(Caps::for_module(None, max), max);
    assert_eq!(Caps::for_module(Some(&ResourceLimits::default()), max), max);
    let l = |memory_mb, cpu_percent| ResourceLimits {
        memory_mb,
        cpu_percent,
    };
    assert_eq!(
        Caps::for_module(Some(&l(Some(256), Some(10))), max),
        Caps {
            memory: 256 << 20,
            cpu_rate: 1000
        }
    );
    assert_eq!(
        Caps::for_module(Some(&l(Some(1 << 20), Some(100))), max),
        max,
        "over the maximum is clamped"
    );
    assert_eq!(
        Caps::for_module(Some(&l(None, Some(25))), max),
        Caps {
            memory: max.memory,
            cpu_rate: 2500
        }
    );
}

#[test]
fn caps_hold_on_a_real_child_in_its_job() {
    let caps = Caps::for_module(
        Some(&ResourceLimits {
            memory_mb: Some(32),
            cpu_percent: Some(20),
        }),
        Caps::HUB_MAX,
    );
    let name = format!(r"Local\hideout-job-test-caps-{}", std::process::id());
    let job = Job::create(&name, caps).unwrap();
    assert_eq!(job.caps().unwrap(), caps);
    let ping = PathBuf::from(std::env::var("SystemRoot").unwrap_or(r"C:\Windows".into()))
        .join(r"System32\PING.EXE");
    let mut cmd = std::process::Command::new(ping);
    cmd.args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = job.spawn(cmd).unwrap();
    assert!(job.holds(&Process::open(child.id()).unwrap()).unwrap());
    job.kill(1);
    let _ = child.wait();
}

/// The policy side of "started once": every Activate after the first, while the module starts
/// or runs, is ignored; after its own clean exit the next one starts it again.
#[test]
fn activations_launch_once_until_it_stops_by_itself() {
    let t = Instant::now();
    let mut p = Policy::new();
    let launches = |cmds: &[Cmd]| {
        cmds.iter()
            .filter(|c| matches!(c, Cmd::Launch { id } if id == ID))
            .count()
    };
    let cmds = p.handle(
        Ev::Known {
            id: ID.into(),
            name: "Lab pinger".into(),
            enabled: true,
            at_enable: false,
            previous: None,
        },
        t,
    );
    assert_eq!(launches(&cmds), 0, "started at enable without `enable`");
    let activate = || Ev::Activate { id: ID.into() };
    assert_eq!(launches(&p.handle(activate(), t)), 1);
    let _ = p.handle(
        Ev::Started {
            id: ID.into(),
            instance: 1,
        },
        t,
    );
    assert_eq!(launches(&p.handle(activate(), t)), 0);
    let _ = p.handle(Ev::Ready { id: ID.into() }, t);
    assert_eq!(launches(&p.handle(activate(), t)), 0);
    let _ = p.handle(
        Ev::Exited {
            id: ID.into(),
            instance: 1,
            code: 0,
        },
        t,
    );
    assert_eq!(launches(&p.handle(activate(), t)), 1);
}
