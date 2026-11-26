// SPDX-License-Identifier: MIT
//! #1451: a turn-off that changes how an app is contained applies to the app that runs now,
//! through the real driver and router with no process ever started (the app's program is not
//! there, so every start of it fails before anything is set up). Windowless.
//!
//! What must hold: an app never keeps running as it was allowed before. It is restarted, and
//! one that can't be started again is stopped. A turn-off that leaves the containment as it
//! was restarts nothing; with nothing running, nothing restarts. With no supervisor to ask,
//! nothing claims the app is not running. A turn-off that can't be recorded stops the app.
#![cfg(windows)]
// It turns on developer mode from the environment, which only a build with `lab-switches` reads.
#![cfg(feature = "lab-switches")]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::{revoke_items_in, Applies, Supervisor};
use hideout_supervisor::policy::Ev;
use hideout_supervisor::words;
use serde_json::Value;

const NOW: &str = "2026-11-25T12:00:00Z";

fn temp(name: &str) -> PathBuf {
    // Not %TEMP%: a dev folder must not be writable by other accounts.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("hideout-turn-off-applies")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An app allowed any network, a keyboard and notifications, in a hub that contains apps. Its
/// program (the second path) is removed before a supervisor starts: nothing of it can ever start.
fn fixture(id: &str) -> (PathBuf, PathBuf) {
    let out = temp("out");
    let exe = out.join("app.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let exe_s = exe
        .display()
        .to_string()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let folder = temp("manifest");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"Fox\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = [\"{exe_s}\"]\n\
             api = {{ foxglove = \"1.0\" }}\n\
             permissions = [\"hub.notify\", \"net\", \"device.hid:keyboard\"]\n"
        ),
    )
    .unwrap();
    let home = temp("home");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    std::env::set_var("HIDEOUT_DEV_MODE", "1");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.add_dev_folder(&out.display().to_string()).unwrap();
    let list = reg.permission_list(id).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    reg.approve_listed(id, list.code(), &all, &[], NOW, "dialog", false)
        .unwrap();
    reg.set_enabled(id, true).unwrap();
    let toml = home.join("hideout.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap_or_default();
    text.push_str("\n[supervisor]\nappcontainer = true\n");
    std::fs::write(&toml, text).unwrap();
    (home, exe)
}

fn hub(home: &Path) -> Hub {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(home).unwrap());
    let _ = router.watch_facts(vec![String::new()]);
    let (events, rx) = mpsc::channel();
    std::mem::forget(rx);
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

fn fact(hub: &Hub, id: &str, name: &str) -> Value {
    hub.read(|r| r.fact(&format!("module.{id}.{name}")).cloned())
        .unwrap_or(Value::Null)
}

fn off(home: &Path, hub: &Hub, id: &str, item: &str) -> Applies {
    revoke_items_in(home, hub, id, Some(&[item.to_string()]))
        .unwrap()
        .applies
}

/// The policy holds instance `n` of the app as running (no process behind it).
fn runs(sup: &Supervisor, id: &str, n: u64) {
    sup.feed(Ev::Started {
        id: id.into(),
        instance: n,
    });
    sup.feed(Ev::Ready { id: id.into() });
}

/// One test: the hub folder comes from the environment, which is the process's.
#[test]
fn a_turn_off_that_changes_containment_reaches_the_app_that_runs() {
    let id = format!("turn-off-{}", std::process::id());
    let id = id.as_str();
    let (home, exe) = fixture(id);
    let h = hub(&home);

    // No supervisor on this folder yet: nothing can tell whether the app runs, and nothing
    // says it does not.
    let a = off(&home, &h, id, "net");
    assert!(a.unreached() && !a.restarts(), "{a:?}");
    assert_eq!(a.pipe_line(), Some(words::PIPE_UNREACHED));
    // Allowed again, as an Allow would.
    let mut reg = Registry::open_from_env(&home).unwrap();
    let list = reg.permission_list(id).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    reg.approve_listed(id, list.code(), &all, &[], NOW, "dialog", false)
        .unwrap();
    drop(reg);
    std::fs::remove_file(&exe).unwrap();

    let sup = Supervisor::start(&home, h.clone());
    sup.feed(Ev::Known {
        id: id.into(),
        name: "Fox".into(),
        enabled: true,
        at_enable: true,
        previous: None,
    });
    assert_ne!(fact(&h, id, "state"), Value::String("running".into()));

    // Nothing of it runs: the turn-off applies at its next start, and nothing is started.
    let before = fact(&h, id, "state");
    let a = off(&home, &h, id, "net");
    assert!(matches!(a, Applies::NotRunning), "{a:?}");
    assert_eq!(a.pipe_line(), Some(words::PIPE_NOT_RUNNING));
    assert_eq!(fact(&h, id, "state"), before);

    // It runs, and what is turned off leaves its containment as it was: nothing restarts.
    runs(&sup, id, 1);
    assert_eq!(fact(&h, id, "state"), Value::String("running".into()));
    let a = off(&home, &h, id, "hub.notify");
    assert!(matches!(a, Applies::Unchanged), "{a:?}");
    assert_eq!(a.pipe_line(), None);
    assert_eq!(fact(&h, id, "state"), Value::String("running".into()));
    assert_eq!(fact(&h, id, "restarted"), Value::Null);

    // It runs, and the turn-off changes its containment: it is ended and started again. Its
    // program is gone, so it can't be: it is stopped, never left as running, and the caller
    // hears so.
    let a = off(&home, &h, id, "device.hid:keyboard");
    assert!(a.restarts(), "{a:?}");
    assert_eq!(a.pipe_line(), Some(words::PIPE_STOPPED));
    let state = fact(&h, id, "state");
    assert!(
        state != Value::String("running".into()) && state != Value::String("restarting".into()),
        "left as {state}"
    );
    assert_eq!(fact(&h, id, "restarted"), Value::Null);
    // Checklist 1: the restarted line is not there before the app runs again.
    runs(&sup, id, 2);
    assert_eq!(
        fact(&h, id, "restarted"),
        Value::Null,
        "a start by hand after the stop is not a restart to apply"
    );

    // It runs, and a turn-off can't be recorded (the hub's records don't read): it is stopped
    // and not started again, and the caller hears both.
    assert_eq!(fact(&h, id, "state"), Value::String("running".into()));
    std::fs::write(home.join("inventory.json"), b"not json").unwrap();
    let e = revoke_items_in(&home, &h, id, Some(&["hub.notify".to_string()])).unwrap_err();
    assert!(e.stopped, "{e:?}");
    assert!(!e.why.is_empty());
    assert_eq!(
        fact(&h, id, "state"),
        Value::String("install-failed".into())
    );
    assert_eq!(fact(&h, id, "failed"), Value::String("unsaved".into()));
    assert_eq!(
        fact(&h, id, "reason"),
        Value::String(words::STOPPED_UNSAVED.into())
    );
    assert_eq!(fact(&h, id, "restarted"), Value::Null);
    // Nothing runs now: a second failure has nothing to stop and says so.
    let e = revoke_items_in(&home, &h, id, Some(&["hub.notify".to_string()])).unwrap_err();
    assert!(!e.stopped, "{e:?}");
}
