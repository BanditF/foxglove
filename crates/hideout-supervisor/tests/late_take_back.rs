// SPDX-License-Identifier: MIT
//! An app adopted into a RUNNING hub, and what takes its Allow back or keeps it: Remove and
//! adopt again (nothing held), a hub restart (the Allow is read back from the registry, a
//! grant only written to the file is not), and a hand-over the hub does not live to finish
//! (the row stays on, nothing is held after the restart). Window-free: the app is a copy of
//! `ping.exe` that is never started (it is off, or its program file is gone before a start).
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

/// A hub on `home` as a hub start makes it: the router reads the registry, then the
/// supervisor starts.
fn hub_on(home: &Path) -> (Hub, common::Reaped) {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(home).unwrap());
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let sup = common::start(home, hub.clone());
    (hub, sup)
}

/// A running hub with nothing installed, and the folder an app will be adopted from.
fn running_hub(name: &str) -> (PathBuf, PathBuf, Hub, common::Reaped) {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base =
        std::env::temp_dir().join(format!("hideout-take-back-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("sensors");
    std::fs::create_dir_all(&folder).unwrap();
    let home = base.join("home");
    drop(Registry::open(&home, true).unwrap());
    let (hub, sup) = hub_on(&home);
    (home, folder, hub, sup)
}

fn exe(folder: &Path) -> PathBuf {
    folder.join("sensors.exe")
}

fn adopt(home: &Path, folder: &Path, id: &str) {
    let exe = exe(folder);
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
fn may_read_sensors(hub: &Hub, id: &str) -> bool {
    let mut ok = false;
    hub.act(|r| {
        let now = Instant::now();
        r.set_token(id, "tok");
        let c = r.connect(Origin::Bridged(id.to_string()));
        let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "id":id,"version":"1.0.0","instance":"i","token":"tok","hub":"1.5",
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

fn enabled(home: &Path, id: &str) -> bool {
    Registry::open_from_env(home).unwrap().hub.enabled(id)
}

fn end(sup: common::Reaped, home: &Path) {
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// Remove, then the same app adopted again while the hub still runs: it holds nothing, in
/// the file and in the router, whether or not the inventory watch's forget ran in between.
#[test]
fn a_removed_late_app_adopted_again_holds_nothing() {
    for forget in [true, false] {
        let name = if forget { "readd-f" } else { "readd-n" };
        let (home, folder, hub, sup) = running_hub(name);
        let id = format!("example.lab.sensors-{name}");
        let id = id.as_str();
        adopt(&home, &folder, id);
        assert!(sup.learn(id).is_some());
        allow(&sup, &home, id);
        assert!(may_read_sensors(&hub, id), "denied after the Allow");

        sup.remove(id).unwrap();
        assert!(!file_grants(&home).contains_key(id));
        // The inventory watch: narrows to the file it read, and forgets what left it.
        let files = file_grants(&home);
        hub.act(|r| r.narrow_module_grants(&files));
        assert!(hub.read(|r| r.module_grants(id)).is_empty());
        if forget {
            hub.act(|r| r.forget_module(id));
            assert!(!hub.read(|r| r.knows_module(id)));
        }
        assert!(!may_read_sensors(&hub, id), "a removed app still reads");

        adopt(&home, &folder, id);
        assert!(sup.learn(id).is_some());
        assert!(file_grants(&home)[id].is_empty(), "the file kept a grant");
        assert!(
            hub.read(|r| r.module_grants(id)).is_empty(),
            "the Allow came back (forget = {forget})"
        );
        assert!(
            !may_read_sensors(&hub, id),
            "allowed with no new Allow (forget = {forget})"
        );
        // The list is asked afresh, and a new Allow works as the first did.
        allow(&sup, &home, id);
        assert!(may_read_sensors(&hub, id), "denied after the new Allow");
        end(sup, &home);
    }
}

/// A Remove that lands while the Allow still waits for the router to learn the app: the next
/// app of that id and build holds nothing.
#[test]
fn a_remove_before_the_hub_learnt_the_late_app_drops_its_kept_allow() {
    let (home, folder, hub, sup) = running_hub("early-rm");
    let id = "example.lab.sensors-early-rm";
    adopt(&home, &folder, id);
    allow(&sup, &home, id);
    assert!(!hub.read(|r| r.knows_module(id)));
    sup.remove(id).unwrap();
    // The watch's pass on the file without the app.
    let files = file_grants(&home);
    hub.act(|r| r.narrow_module_grants(&files));

    adopt(&home, &folder, id);
    assert!(sup.learn(id).is_some());
    assert!(hub.read(|r| r.module_grants(id)).is_empty());
    assert!(!may_read_sensors(&hub, id), "allowed with no new Allow");
    end(sup, &home);
}

/// A hub restart: the user's Allow for the late app is read back from the registry, and so
/// is a turn-off. The app is off the whole time, so nothing is started.
#[test]
fn an_allow_for_a_late_app_holds_after_a_hub_restart_and_a_turn_off_does_too() {
    let (home, folder, hub, sup) = running_hub("restart");
    let id = "example.lab.sensors-restart";
    adopt(&home, &folder, id);
    assert!(sup.learn(id).is_some());
    allow(&sup, &home, id);
    assert!(may_read_sensors(&hub, id));
    drop(sup);
    drop(hub);

    let (hub, sup) = hub_on(&home);
    assert!(sup.knows(id));
    assert_eq!(hub.read(|r| r.module_grants(id)), [READ.to_string()]);
    assert!(
        may_read_sensors(&hub, id),
        "the Allow was lost in a restart"
    );
    assert_eq!(sup.pid(id), None, "an app that is off was started");

    // Turned off in the permission list: gone now, and still gone after the next restart.
    let left = sup.revoke_items(id, None).unwrap();
    assert!(left.is_empty(), "still allowed: {left:?}");
    assert!(file_grants(&home)[id].is_empty());
    assert!(!may_read_sensors(&hub, id), "reads after the turn-off");
    drop(sup);
    drop(hub);
    let (hub, sup) = hub_on(&home);
    assert!(hub.read(|r| r.module_grants(id)).is_empty());
    assert!(!may_read_sensors(&hub, id), "the turn-off was lost");
    end(sup, &home);
}

/// A hand-over needs the user's OK first: with the permission list still waiting nothing is
/// saved and nothing is held.
#[test]
fn a_hand_over_for_an_app_not_allowed_yet_saves_and_holds_nothing() {
    let (home, folder, _hub, sup) = running_hub("unasked");
    let id = "example.lab.sensors-unasked";
    adopt(&home, &folder, id);
    assert!(sup.begin_hand_over(id).is_err());
    assert!(!sup.hand_over_held(id));
    assert!(!enabled(&home, id));
    // An end for it changes nothing.
    sup.end_hand_over(id, false);
    sup.end_hand_over(id, true);
    assert!(!enabled(&home, id));
    end(sup, &home);
}

/// The hub exits in the middle of a hand-over: the row was saved as on when it began, so the
/// next hub knows the app as on and holds nothing. (The program file is gone before that
/// start, so nothing runs here.)
#[test]
fn a_hub_that_exits_mid_hand_over_leaves_the_app_on_and_unheld() {
    let (home, folder, hub, sup) = running_hub("mid");
    let id = "example.lab.sensors-mid";
    adopt(&home, &folder, id);
    allow(&sup, &home, id);
    sup.begin_hand_over(id).expect("saved as on");
    assert!(sup.hand_over_held(id) && enabled(&home, id));
    assert_eq!(sup.pid(id), None);
    drop(sup);
    drop(hub);

    std::fs::remove_file(exe(&folder)).unwrap();
    let (_hub, sup) = hub_on(&home);
    assert!(sup.knows(id));
    assert!(enabled(&home, id), "the row went off with the old hub");
    assert!(!sup.hand_over_held(id), "the hold outlived the hub");
    // A late end from nowhere (nothing is held) never turns the row off.
    sup.end_hand_over(id, false);
    assert!(enabled(&home, id));
    assert_eq!(sup.pid(id), None);
    end(sup, &home);
}

/// The ends of a hand-over: the copy went (hold dropped, row stays on); a row saved on by a
/// plain save is not a hand-over, so an end for it turns nothing off.
#[test]
fn an_end_only_takes_back_a_hand_over_that_began() {
    let (home, folder, _hub, sup) = running_hub("ends");
    let id = "example.lab.sensors-ends";
    adopt(&home, &folder, id);
    allow(&sup, &home, id);

    sup.save_on(id).unwrap();
    assert!(enabled(&home, id) && !sup.hand_over_held(id));
    sup.end_hand_over(id, false);
    assert!(enabled(&home, id), "a plain save was taken back");

    sup.begin_hand_over(id).unwrap();
    sup.end_hand_over(id, true);
    assert!(!sup.hand_over_held(id));
    assert!(enabled(&home, id), "the row went off though the copy left");
    // The hold is gone: a second end is nothing, whatever it says.
    sup.end_hand_over(id, false);
    assert!(enabled(&home, id));

    // And the one that takes it back.
    sup.begin_hand_over(id).unwrap();
    sup.end_hand_over(id, false);
    assert!(!sup.hand_over_held(id) && !enabled(&home, id));
    assert_eq!(sup.pid(id), None, "something was started");
    end(sup, &home);
}
