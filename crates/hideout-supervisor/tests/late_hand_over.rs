// SPDX-License-Identifier: MIT
//! A hand-over that fails for a module that joins the inventory after the hub started: the app
//! is held, learnt as off and saved back as off; nothing starts beside the copy. The module is a
//! copy of `cmd.exe` that must never start. Windowless. (Its own file: it sets process env.)
#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};

const ID: &str = "example.lab.late";

fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-late-hand-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(LAB_MODULES_DIR)).unwrap();
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

/// A hand-over that failed (the copy stayed): while it ran the app was held, so learning it
/// started nothing, and ending it with `went = false` saves the row back as off.
#[test]
fn a_failed_hand_over_leaves_the_late_app_off_and_unstarted() {
    let home = lab_home();
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(&home).unwrap());
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    let sup = common::start(&home, hub.clone());

    let dir = home.join(LAB_MODULES_DIR).join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::fs::copy(
        Path::new(&windir).join("System32").join("cmd.exe"),
        dir.join("cmd.exe"),
    )
    .unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Late\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"cmd.exe\", \"/d\", \"/c\", \"ping -n 20 127.0.0.1 >nul\"]\n"
        ),
    )
    .unwrap();
    // The registry holds it (a read), the hub has not learnt it yet.
    assert!(Registry::open_from_env(&home)
        .unwrap()
        .inventory
        .modules
        .contains_key(ID));
    sup.begin_hand_over(ID).expect("saved as on");
    assert!(sup.hand_over_held(ID));
    assert!(Registry::open_from_env(&home).unwrap().hub.enabled(ID));
    // The watch reads the file (on) while the copy is being asked: nothing starts.
    assert!(sup.learn(ID).is_some());
    assert!(sup.knows(ID));
    assert!(sup.pid(ID).is_none(), "started beside the copy");
    // The copy stayed: off again, hold dropped, still nothing running.
    sup.end_hand_over(ID, false);
    assert!(!sup.hand_over_held(ID));
    assert!(!Registry::open_from_env(&home).unwrap().hub.enabled(ID));
    assert!(sup.pid(ID).is_none());
    // An end with nothing held changes nothing.
    sup.end_hand_over(ID, false);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
