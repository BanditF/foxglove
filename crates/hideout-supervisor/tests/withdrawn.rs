// SPDX-License-Identifier: MIT
//! A version its publisher withdrew (#1026), through the real driver: a lab hub whose lab
//! module `lab-withdrawn.json` names (the lab's stand-in for a catalog revocation) is never
//! started, reads `revoked` with the router's `module.<id>.revoked` fact, is notified once (the
//! notification made at start reaches the notifier set after it), and a second hub start shows
//! the state again without a second notification. Turn on is refused with `trust.revoked`;
//! Check for updates without a catalog fetch reads `failed`; once the file no longer names the
//! version, Check for updates clears it and the hub tries to start it. Windowless: the module's
//! exe doesn't exist, so no process is ever started.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hideout_registry::lab::{LAB_MODULES_DIR, LAB_WITHDRAWN_FILE};
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use serde_json::{json, Value};

const ID: &str = "example.lab.withdrawn";

/// A lab hub folder with one lab module and `lab-withdrawn.json` naming its version.
fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-withdrawn-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    let dir = home.join(LAB_MODULES_DIR).join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Lab art\"\nversion = \"0.5.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"missing.exe\"]\n"
        ),
    )
    .unwrap();
    withdraw(&home, Some("Please update.\u{202e} Thanks"));
    // Lab mode: HIDEOUT_LAB=1 with HIDEOUT_CONFIG_DIR on this folder (this test binary only).
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

fn withdraw(home: &std::path::Path, reason: Option<&str>) {
    let doc = match reason {
        Some(r) => json!({ ID: {"version": "0.5.0", "reason": r} }),
        None => json!({}),
    };
    std::fs::write(home.join(LAB_WITHDRAWN_FILE), doc.to_string()).unwrap();
}

/// A lab hub's router; every fact change comes out on the receiver (`Out::Fact`).
fn hub(home: &std::path::Path) -> (Hub, mpsc::Receiver<Out>) {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(home).unwrap());
    let _ = router.watch_facts(vec![String::new()]);
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    (hub, rx)
}

fn fact(hub: &Hub, k: &str) -> Value {
    hub.read(|r| r.fact(&format!("module.{ID}.{k}")).cloned())
        .unwrap_or(Value::Null)
}

type Notes = Arc<Mutex<Vec<String>>>;

fn notes(sup: &Supervisor) -> Notes {
    let got: Notes = Arc::default();
    let g = got.clone();
    sup.set_notifier(Box::new(move |_, title, _, _| {
        g.lock().unwrap().push(title.to_string())
    }));
    got
}

/// Waits for `k` to become `want`. Start event: now; stop event: the fact change on the hub's
/// event channel (`Out::Fact`); 10 s is the fallback only.
fn wait_fact(hub: &Hub, rx: &mpsc::Receiver<Out>, k: &str, want: &Value) -> bool {
    let key = format!("module.{ID}.{k}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fact(hub, k) == *want {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Out::Fact { key: got, value }) if got == key && value == *want => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

#[test]
fn a_withdrawn_lab_module_is_stopped_shown_and_notified_once() {
    let home = lab_home();

    // First start: never launched, revoked, the router's fact, one notification.
    let (h, rx) = hub(&home);
    let sup = Supervisor::start(&home, h.clone());
    assert_eq!(sup.pid(ID), None, "a withdrawn version was started");
    assert_eq!(fact(&h, "state"), json!("revoked"));
    assert_eq!(
        fact(&h, "revoked"),
        json!({"version": "0.5.0", "reason": "Please update.\u{202e} Thanks"})
    );
    let got = notes(&sup);
    assert_eq!(*got.lock().unwrap(), ["Lab art 0.5.0 was withdrawn"]);
    assert!(home.join("run").join("withdrawn-notified.json").exists());

    // Copy diagnostics: the state id, the version and the note escaped.
    let d = sup.diagnostics(ID, "0.0.0");
    assert!(d.contains("state: revoked"), "{d}");
    assert!(d.contains("withdrawn version: 0.5.0"), "{d}");
    assert!(d.contains("\\u{202e}") && !d.contains('\u{202e}'), "{d}");

    // Turn on is refused with trust.revoked, and nothing starts.
    let e = sup.turn_on(ID).unwrap_err();
    assert!(e.starts_with("trust.revoked"), "{e}");
    assert_eq!(sup.pid(ID), None);

    // Check for updates in a hub without a catalog fetch: couldn't check.
    sup.check_for_updates(ID);
    assert!(
        wait_fact(&h, &rx, "check", &json!("failed")),
        "{:?}",
        fact(&h, "check")
    );
    drop(sup);

    // Second start (a later sign-in): the state again, no second notification.
    let (h, rx) = hub(&home);
    let sup = Supervisor::start(&home, h.clone());
    let got = notes(&sup);
    assert_eq!(fact(&h, "state"), json!("revoked"));
    assert!(got.lock().unwrap().is_empty(), "notified again: {got:?}");

    // A check that reaches the source with nothing newer.
    sup.set_refresh(Arc::new(|_| Ok(())));
    sup.check_for_updates(ID);
    assert!(
        wait_fact(&h, &rx, "check", &json!("none")),
        "{:?}",
        fact(&h, "check")
    );

    // The file no longer names it (a fixed catalog): the check clears it, the notice goes,
    // and the hub tries to start it (its exe is missing: Couldn't start, not Withdrawn).
    withdraw(&home, None);
    sup.check_for_updates(ID);
    assert!(wait_fact(&h, &rx, "revoked", &Value::Null));
    assert!(
        wait_fact(&h, &rx, "state", &json!("install-failed")),
        "{:?}",
        fact(&h, "state")
    );
    assert_eq!(fact(&h, "check"), Value::Null);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
