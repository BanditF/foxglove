// SPDX-License-Identifier: MIT
//! An app adopted into a RUNNING hub, then the user's Allow (#1375): the grant takes effect in
//! the router at once (a `facts.subscribe` on the granted pattern works, no restart), survives
//! the inventory watch's narrow pass, and a grant only added to `inventory.json` stays
//! ignored. Both orders: Allow after the watch learnt the module, and Allow before it did.
//! Window-free; no module process is started (the module is never turned on).
#![cfg(windows)]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Origin, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use serde_json::{json, Value};

const NOW: &str = "2026-11-20T00:00:00Z";
const READ: &str = "facts.read:sensor.*";

fn running_hub(name: &str) -> (PathBuf, PathBuf, Hub, common::Reaped) {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base =
        std::env::temp_dir().join(format!("hideout-late-grant-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("sensors");
    std::fs::create_dir_all(&folder).unwrap();
    let home = base.join("home");
    // The hub starts first, with nothing installed.
    drop(Registry::open(&home, true).unwrap());
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(&home).unwrap());
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let sup = common::start(&home, hub.clone());
    (home, folder, hub, sup)
}

fn adopt(home: &Path, folder: &Path, id: &str) {
    let exe = folder.join("sensors.exe");
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::fs::copy(Path::new(&windir).join("System32").join("PING.EXE"), &exe).unwrap();
    let run = exe
        .display()
        .to_string()
        .replace(std::path::MAIN_SEPARATOR, "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"Sensors\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"arete\"\nhub = \"1.0\"\nrun = [\"{run}\", \"-n\", \"60\", \"127.0.0.1\"]\n\
             api = {{ arete = \"1.0\" }}\npermissions = [\"{READ}\"]\n"
        ),
    )
    .unwrap();
    let mut reg = Registry::open_from_env(home).unwrap();
    reg.register_adopted(folder, NOW).unwrap();
}

/// Connects as `id` and subscribes to a sensor fact: whether the router allowed it.
fn may_read_sensors(hub: &Hub, id: &str, version: &str) -> bool {
    let mut ok = false;
    hub.act(|r| {
        let now = Instant::now();
        r.set_token(id, "tok");
        let c = r.connect(Origin::Bridged(id.to_string()));
        let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "id":id,"version":version,"instance":"i","token":"tok","hub":"1.5",
            "api":{"arete":"1.0"}}});
        r.receive(c, &init.to_string(), now);
        let sub = json!({"jsonrpc":"2.0","id":7,"method":"facts.subscribe",
            "params":{"pattern":"sensor.gpu.*"}});
        let out = r.receive(c, &sub.to_string(), now);
        ok = out.iter().any(|o| match o {
            hideout_router::Out::Send(to, v) if *to == c => {
                v.get("id") == Some(&Value::from(7)) && v.get("result").is_some()
            }
            _ => false,
        });
        r.disconnect(c);
        Vec::new()
    });
    ok
}

fn file_grants(home: &Path) -> HashMap<String, Vec<String>> {
    Registry::open_from_env(home)
        .unwrap()
        .inventory
        .modules
        .iter()
        .map(|(k, m)| (k.clone(), m.granted.clone()))
        .collect()
}

fn allow(sup: &Supervisor, home: &Path, id: &str) {
    let list = Registry::open_from_env(home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    let out = sup
        .grant_listed(id, list.code(), &[READ.to_string()], &[], "dialog")
        .unwrap();
    assert_eq!(out.granted, [READ.to_string()]);
}

fn check_survives_the_file(hub: &Hub, home: &Path, id: &str) {
    // The watch's narrow pass with what the file says now: the grant stays.
    let files = file_grants(home);
    hub.act(|r| r.narrow_module_grants(&files));
    assert_eq!(hub.read(|r| r.module_grants(id)), [READ.to_string()]);
    assert!(
        may_read_sensors(hub, id, "1.0.0"),
        "lost after the file pass"
    );
    // A grant only added to the file stays ignored.
    let mut forged = files;
    forged
        .get_mut(id)
        .unwrap()
        .push("facts.read:secret.*".into());
    hub.act(|r| r.narrow_module_grants(&forged));
    assert_eq!(hub.read(|r| r.module_grants(id)), [READ.to_string()]);
}

#[test]
fn an_allow_after_the_hub_learnt_the_late_app_takes_effect_without_a_restart() {
    let (home, folder, hub, sup) = running_hub("learnt");
    let id = "example.lab.sensors-learnt";
    assert!(!hub.read(|r| r.knows_module(id)));
    adopt(&home, &folder, id);
    assert!(sup.learn(id).is_some());
    assert!(hub.read(|r| r.knows_module(id)));
    assert!(
        !may_read_sensors(&hub, id, "1.0.0"),
        "allowed before the Allow"
    );
    allow(&sup, &home, id);
    assert!(
        may_read_sensors(&hub, id, "1.0.0"),
        "denied after the Allow"
    );
    check_survives_the_file(&hub, &home, id);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn an_allow_that_beats_the_watchs_older_read_is_not_lost() {
    let (home, folder, hub, sup) = running_hub("early");
    let id = "example.lab.sensors-early";
    adopt(&home, &folder, id);
    // The watch read the registry when the app joined (no grant yet) ...
    let stale = Registry::open_from_env(&home).unwrap();
    let files = file_grants(&home);
    hub.act(|r| r.narrow_module_grants(&files));
    assert!(!hub.read(|r| r.knows_module(id)));
    // ... the user's Allow landed before the router learnt the app ...
    allow(&sup, &home, id);
    // ... and the router learns it from that older read, as the watch does.
    assert!(sup.learn_from(&stale, id).is_some());
    assert!(hub.read(|r| r.knows_module(id)));
    assert!(
        may_read_sensors(&hub, id, "1.0.0"),
        "denied after the Allow"
    );
    check_survives_the_file(&hub, &home, id);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
