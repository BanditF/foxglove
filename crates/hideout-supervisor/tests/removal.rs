// SPDX-License-Identifier: MIT
//! A module removed while the hub runs (#1054.2), through the real driver and router: once
//! its folder is gone, the supervisor and the router forget it, its `module.<id>.*` facts go,
//! and a Start again for it starts nothing. Windowless: the module's exe doesn't exist, so no
//! process is ever started.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use serde_json::Value;

const ID: &str = "example.lab.removed";

/// A lab hub folder with one lab module.
fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-removal-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    let dir = home.join(LAB_MODULES_DIR).join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Lab gone\"\nversion = \"0.5.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"missing.exe\"]\n"
        ),
    )
    .unwrap();
    // Lab mode: HIDEOUT_LAB=1 with HIDEOUT_CONFIG_DIR on this folder (this test binary only).
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

fn facts_of(hub: &Hub) -> Vec<String> {
    let prefix = format!("module.{ID}.");
    ["state", "reason", "restart", "recovery", "revoked", "check"]
        .iter()
        .map(|k| format!("{prefix}{k}"))
        .filter(|k| hub.read(|r| r.fact(k).is_some()))
        .collect()
}

#[test]
fn a_removed_module_is_forgotten_by_the_supervisor_and_the_router() {
    let home = lab_home();
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(&home).unwrap());
    let _ = router.watch_facts(vec![String::new()]);
    let (events, rx) = mpsc::channel::<Out>();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    let sup = Supervisor::start(&home, hub.clone());
    assert_eq!(sup.pid(ID), None);
    assert!(sup.knows(ID), "the supervisor didn't know the lab module");
    assert!(hub.read(|r| r.knows_module(ID)));
    assert!(!facts_of(&hub).is_empty(), "no state fact to forget");

    // Removed (the lab's stand-in for `hideout-core remove`), then the watch's report.
    std::fs::remove_dir_all(home.join(LAB_MODULES_DIR).join(ID)).unwrap();
    while rx.try_recv().is_ok() {}
    assert!(sup.forget(ID));
    hub.act(|r| r.forget_module(ID));
    assert!(!sup.knows(ID));
    assert!(!hub.read(|r| r.knows_module(ID)));
    assert!(facts_of(&hub).is_empty(), "left: {:?}", facts_of(&hub));
    let told: Vec<_> = rx
        .try_iter()
        .filter_map(|o| match o {
            Out::Fact { key, value: Value::Null } => Some(key),
            _ => None,
        })
        .collect();
    assert!(
        told.contains(&format!("module.{ID}.state")),
        "the fact watch wasn't told: {told:?}"
    );
    // Nothing of it starts again, and a second forget finds nothing.
    sup.start_again(ID);
    assert_eq!(sup.pid(ID), None);
    assert!(!sup.knows(ID));
    assert!(!sup.forget(ID));
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
