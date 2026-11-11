// SPDX-License-Identifier: MIT
//! Turning off a lab module (#1076): the Modules page's Turn off reaches the supervisor, which
//! saved it through a registry opened without lab mode, so the lab module read as not
//! installed and `hideout.toml` was never written; after a restart it came back on. Now it is
//! saved, only its own key is written (the other lab modules' "on unless the file says
//! otherwise" stays in memory), and a new supervisor on the same folder keeps it off. Turn on
//! saves it on again. Windowless: the modules' exe doesn't exist, so no process starts.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;

const ART: &str = "example.lab.art";
const BEAT: &str = "example.lab.beat";

/// A lab hub folder with two lab modules, in lab mode (this test binary only).
fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-labtoff-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    for (id, name) in [(ART, "Art"), (BEAT, "Beat")] {
        let dir = home.join(LAB_MODULES_DIR).join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("module.toml"),
            format!(
                "id = \"{id}\"\nname = \"{name}\"\nversion = \"0.5.0\"\nkind = \"service\"\n\
                 family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
                 run = [\"missing.exe\"]\n"
            ),
        )
        .unwrap();
    }
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

fn hub_on(home: &std::path::Path) -> Hub {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(home).unwrap());
    let _ = router.watch_facts(vec![String::new()]);
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

fn state(hub: &Hub, id: &str) -> Option<String> {
    hub.read(|r| {
        r.fact(&format!("module.{id}.state"))
            .and_then(|v| v.as_str())
            .map(String::from)
    })
}

#[test]
fn turning_off_a_lab_module_is_saved_and_holds_after_a_restart() {
    let home = lab_home();
    let toml = home.join("hideout.toml");
    let hub = hub_on(&home);
    let sup = Supervisor::start(&home, hub.clone());
    assert!(sup.knows(ART) && sup.knows(BEAT));
    assert!(!toml.exists(), "a lab hub's start wrote hideout.toml");

    sup.turn_off(ART);
    let text = std::fs::read_to_string(&toml).expect("Turn off didn't write hideout.toml");
    let saved = hideout_registry::hub_config::HubConfig::parse(&text).unwrap();
    assert_eq!(saved.enabled_set(ART), Some(false), "{text}");
    assert_eq!(
        saved.enabled_set(BEAT),
        None,
        "Beat's in-memory default was saved as a choice: {text}"
    );
    assert_eq!(state(&hub, ART).as_deref(), Some("off"));
    drop(sup);

    // The restart: a new registry, router and supervisor on the same folder.
    let reg = Registry::open_from_env(&home).unwrap();
    assert!(!reg.hub.enabled(ART) && reg.hub.enabled(BEAT));
    let hub = hub_on(&home);
    let sup = Supervisor::start(&home, hub.clone());
    assert_eq!(state(&hub, ART).as_deref(), Some("off"), "Art came back on");

    // Turn on saves it on again (it then tries to start; its exe is missing).
    sup.turn_on(ART).unwrap();
    let text = std::fs::read_to_string(&toml).unwrap();
    let saved = hideout_registry::hub_config::HubConfig::parse(&text).unwrap();
    assert_eq!(saved.enabled_set(ART), Some(true), "{text}");
    assert_eq!(saved.enabled_set(BEAT), None, "{text}");
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
