// SPDX-License-Identifier: MIT
//! R3 race, through the real driver and router with no process ever started: a refused rebuild
//! stops nothing and pins nothing, and a new build pinned before a stop is let go (its file
//! writable again) when the launch is refused. Windowless.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::policy::Ev;
use serde_json::Value;

const ID: &str = "dev.unregistered";

/// One folder per test: the tests run side by side in one process, and a shared folder let one
/// test's clean-up delete the other's files (#1400).
fn home(test: &str) -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!(
            "hideout-prepared-pin-{}-{test}",
            std::process::id()
        ))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

fn hub(home: &std::path::Path) -> Hub {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(home).unwrap());
    let _ = router.watch_facts(vec![String::new()]);
    let (events, rx) = mpsc::channel();
    std::mem::forget(rx);
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

fn state(hub: &Hub) -> Value {
    hub.read(|r| r.fact(&format!("module.{ID}.state")).cloned())
        .unwrap_or(Value::Null)
}

fn running(sup: &Supervisor) {
    sup.feed(Ev::Known {
        id: ID.into(),
        name: "Dev".into(),
        enabled: true,
        at_enable: true,
        previous: None,
    });
    sup.feed(Ev::Started {
        id: ID.into(),
        instance: 1,
    });
    sup.feed(Ev::Ready { id: ID.into() });
}

fn writable(p: &std::path::Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(p).is_ok()
}

#[test]
fn a_refused_preflight_leaves_the_old_copy_running_and_nothing_pinned() {
    let home = home("refused");
    let h = hub(&home);
    let sup = Supervisor::start(&home, h.clone());
    running(&sup);
    assert_eq!(state(&h), Value::String("running".into()));
    // The app is not enrolled in any dev folder here: the check refuses before any stop.
    sup.rebuilt(ID, "abcd");
    assert_eq!(
        state(&h),
        Value::String("running".into()),
        "the old copy was stopped for a build that was never verified"
    );
    assert!(!sup.prepared_held(ID));
}

#[test]
fn a_launch_refused_after_the_stop_releases_the_pinned_build() {
    let home = home("released");
    let exe = home.join("new-build.exe");
    std::fs::write(&exe, b"build 2").unwrap();
    let h = hub(&home);
    let sup = Supervisor::start(&home, h);
    running(&sup);
    sup.hold_prepared(ID, &exe).unwrap();
    assert!(sup.prepared_held(ID));
    assert!(!writable(&exe), "pinned: a build can't overwrite it");
    // The launch that follows is refused (the app is not installed: the dev folder or trust
    // changed during the stop). The pin goes with it.
    sup.feed(Ev::Exited {
        id: ID.into(),
        instance: 1,
        code: 0,
    });
    sup.feed(Ev::StartAgain { id: ID.into() });
    assert!(
        !sup.prepared_held(ID),
        "the pin outlived the refused launch"
    );
    assert!(writable(&exe), "the file stayed locked");
}
