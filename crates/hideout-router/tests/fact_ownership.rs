// SPDX-License-Identifier: MIT
//! Who may own a fact pattern (contract §4.5 `facts:`, §6.3, §8.3, §8.8, #1150): a module
//! registers only facts its manifest provides, an adopted module never a hub-owned, first-party
//! or catalog-assigned area nor one a rival adopted module also qualifies for, and `facts.set`
//! needs `facts.write:` or the declared `facts:` provide. Window-free.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const FOX: &str = "foxglove";

fn entry(id: &str, holder: Holder, granted: &[&str], catalog: Option<&[&str]>) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder,
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: catalog.map(|c| c.iter().map(|s| s.to_string()).collect()),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    }
}

/// An adopted module as `register_adopted` records it: no catalog entry, a registered version.
fn adopted(id: &str, granted: &[&str]) -> ModuleEntry {
    let mut e = entry(id, Holder::ThirdParty(Source::Sideload), granted, None);
    e.registered_version = Some("0.1.0".into());
    e
}

fn manifest(id: &str, provides: &str) -> Module {
    let family = if id == FOX { "foxglove" } else { "arete" };
    let text = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"adopted\"\nfamily = \"{family}\"\nhub = \"1.0\"\nrun = [\"C:\\\\x\\\\x.exe\", \"hub\"]\nprovides = [{provides}]\n"
    );
    Module::parse(&text, &Context::default()).expect("manifest")
}

fn sends(out: &[Out], to: ConnId) -> Vec<Value> {
    out.iter()
        .filter_map(|o| match o {
            Out::Send(c, v) if *c == to => Some(v.clone()),
            _ => None,
        })
        .collect()
}

fn module(r: &mut Router, id: &str, now: Instant) -> ConnId {
    r.set_token(id, &format!("tok-{id}"));
    let c = r.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":id,"version":"0.1.0","instance":"i","token":format!("tok-{id}"),"hub":"1.3",
        "api":{"foxglove":"1.13"}}});
    let out = r.receive(c, &init.to_string(), now);
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    c
}

/// Registers `facts` as `c` and says ready; returns the `rejected` list.
fn register(r: &mut Router, c: ConnId, facts: &[&str], now: Instant) -> Value {
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":[],"facts":facts,"channels":[],"apis":{}}});
    let out = r.receive(c, &reg.to_string(), now);
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    sends(&out, c)[0]["result"]["rejected"].clone()
}

fn rig_one(id: &str, provides: &str, now: Instant) -> (Router, ConnId) {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(id, &[]));
    if !provides.is_empty() {
        r.apply_manifest(&manifest(id, provides));
    }
    let c = module(&mut r, id, now);
    (r, c)
}

fn set(r: &mut Router, c: ConnId, key: &str, now: Instant) -> Value {
    let m = json!({"jsonrpc":"2.0","id":9,"method":"facts.set","params":{"key":key,"value":true}});
    sends(&r.receive(c, &m.to_string(), now), c).remove(0)
}

fn code(v: &Value) -> i64 {
    v["error"]["code"].as_i64().expect("an error")
}

fn rej(item: &str) -> Value {
    json!({"item": item, "error": "permission.provider"})
}

#[test]
fn a_module_without_the_provide_cannot_register_a_squatted_area() {
    let now = Instant::now();
    for provides in ["", r#""method:art.*""#, r#""facts:weather.*""#] {
        let (mut r, c) = rig_one("example.squat", provides, now);
        let rejected = register(
            &mut r,
            c,
            &["gpu.*", "keyboard.*", "keyboard.game_mode"],
            now,
        );
        assert_eq!(
            rejected.as_array().unwrap().len(),
            3,
            "{provides}: {rejected}"
        );
        assert_eq!(code(&set(&mut r, c, "keyboard.game_mode", now)), 5);
    }
}

#[test]
fn the_foxglove_module_owns_keyboard_and_writes_only_it() {
    let now = Instant::now();
    let (mut r, c) = rig_one(FOX, r#""facts:keyboard.*""#, now);
    assert_eq!(register(&mut r, c, &["keyboard.*"], now), json!([]));
    assert!(set(&mut r, c, "keyboard.game_mode", now)["result"].is_object());
    assert_eq!(r.fact("keyboard.game_mode"), Some(&json!(true)));
    for key in [
        "gpu.reserved",
        "device.x.layers",
        "apps.running.x",
        "module.foxglove.state",
    ] {
        assert_eq!(code(&set(&mut r, c, key, now)), 5, "{key}");
    }
    // Registering more than it declares is refused.
    assert_eq!(
        register(&mut r, c, &["keyboard.*", "gpu.*"], now),
        json!([rej("gpu.*")])
    );
}

#[test]
fn registering_alone_writes_nothing_without_a_declared_provide_or_a_grant() {
    let now = Instant::now();
    // A module the router has no manifest for (a legacy one): the owner has no `facts:` provide.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(
        "example.weather",
        Holder::ThirdParty(Source::Sideload),
        &[],
        None,
    ));
    let c = module(&mut r, "example.weather", now);
    assert_eq!(register(&mut r, c, &["weather.*"], now), json!([]));
    assert_eq!(code(&set(&mut r, c, "weather.temp", now)), 5);
    // The `facts.write:` grant alone writes (v1), owner or not.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(
        "example.weather",
        Holder::ThirdParty(Source::Sideload),
        &["facts.write:weather.*"],
        None,
    ));
    let c = module(&mut r, "example.weather", now);
    assert!(set(&mut r, c, "weather.temp", now)["result"].is_object());
}

#[test]
fn a_hub_owned_or_first_party_fact_area_is_refused_to_an_adopted_module() {
    let now = Instant::now();
    for area in [
        "apps.*",
        "module.*",
        "hub.*",
        "mic.*",
        "device.*",
        "lighting.*",
        "devices.*",
    ] {
        let provides = format!(r#""facts:{area}""#);
        let (mut r, c) = rig_one("example.adopted", &provides, now);
        assert_eq!(
            register(&mut r, c, &[area], now),
            json!([rej(area)]),
            "{area}"
        );
    }
    // `*` has no manifest spelling and is never registered.
    let (mut r, c) = rig_one("example.adopted", r#""facts:art.*""#, now);
    assert_eq!(register(&mut r, c, &["*"], now), json!([rej("*")]));
    // gpu.* stays the adopted GPU broker's, like the methods (§8.9).
    let (mut r, c) = rig_one("example.gpu", r#""facts:gpu.*""#, now);
    assert_eq!(register(&mut r, c, &["gpu.*"], now), json!([]));
}

#[test]
fn two_adopted_candidates_for_a_fact_area_are_both_refused() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    for id in [FOX, "example.rival"] {
        r.add_module(adopted(id, &[]));
        r.apply_manifest(&manifest(id, r#""facts:keyboard.*""#));
    }
    // No start order or id decides it.
    for id in ["example.rival", FOX] {
        let c = module(&mut r, id, now);
        assert_eq!(
            register(&mut r, c, &["keyboard.*"], now),
            json!([rej("keyboard.*")]),
            "{id}"
        );
        assert_eq!(code(&set(&mut r, c, "keyboard.game_mode", now)), 5);
    }
}

#[test]
fn a_catalog_assigned_fact_area_is_refused_to_adopted_modules_whoever_registers_first() {
    let now = Instant::now();
    // A root-signed catalog assigns keyboard.* to another module, installed or not.
    let (mut r, c) = rig_one(FOX, r#""facts:keyboard.*""#, now);
    r.set_root_areas(vec![("foxglove.keys".into(), "keyboard.*".into())]);
    assert_eq!(
        register(&mut r, c, &["keyboard.*"], now),
        json!([rej("keyboard.*")])
    );
    // The installed catalog module takes it although an adopted one registered first.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(FOX, &[]));
    r.apply_manifest(&manifest(FOX, r#""facts:keyboard.*""#));
    r.add_module(entry(
        "foxglove.keys",
        Holder::FirstParty,
        &[],
        Some(&["keyboard.*"]),
    ));
    let f = module(&mut r, FOX, now);
    assert_eq!(
        register(&mut r, f, &["keyboard.*"], now),
        json!([rej("keyboard.*")])
    );
    let k = module(&mut r, "foxglove.keys", now);
    assert_eq!(register(&mut r, k, &["keyboard.*"], now), json!([]));
    // A third-party module with a catalog entry elsewhere cannot take it either.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(
        "foxglove.keys",
        Holder::FirstParty,
        &[],
        Some(&["keyboard.*"]),
    ));
    r.add_module(entry(
        "example.t",
        Holder::ThirdParty(Source::Sideload),
        &[],
        Some(&["art.*"]),
    ));
    let t = module(&mut r, "example.t", now);
    assert_eq!(
        register(&mut r, t, &["keyboard.*"], now),
        json!([rej("keyboard.*")])
    );
}

#[test]
fn a_registry_reload_revokes_a_fact_route_that_is_no_longer_allowed() {
    let now = Instant::now();
    let (mut r, c) = rig_one(FOX, r#""facts:keyboard.*""#, now);
    assert_eq!(register(&mut r, c, &["keyboard.*"], now), json!([]));
    assert!(set(&mut r, c, "keyboard.game_mode", now)["result"].is_object());
    // A catalog now assigns the area to another module: the route goes at once.
    r.set_root_areas(vec![("foxglove.keys".into(), "keyboard.*".into())]);
    assert_eq!(code(&set(&mut r, c, "keyboard.game_mode", now)), 5);
    // A rival adopted module arriving drops it too.
    let (mut r, c) = rig_one(FOX, r#""facts:keyboard.*""#, now);
    assert_eq!(register(&mut r, c, &["keyboard.*"], now), json!([]));
    r.add_module(adopted("example.rival", &[]));
    r.apply_manifest(&manifest("example.rival", r#""facts:keyboard.*""#));
    r.set_root_areas(vec![]);
    assert_eq!(code(&set(&mut r, c, "keyboard.game_mode", now)), 5);
}
