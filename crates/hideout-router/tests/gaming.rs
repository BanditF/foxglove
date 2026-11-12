// SPDX-License-Identifier: MIT
//! The gaming pair (contract §8.9, #1121): `gpu.reserved` (from Vulpes) and `keyboard.game_mode`
//! (the GAME layer) are shared facts and neither acts on the other; a reservation changes only
//! through `action.invoke` / `ui.event` from the hub with a user source, and no module can send
//! either. A bound key starts a cold module (§4.6). Window-free.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

/// `arete.hub` as the hub knows it: `arete` is read as `vulpes` (§4.3).
const ARETE: &str = "vulpes.hub";
const CALLER: &str = "example.service";

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

fn manifest(id: &str, provides: &str, extra: &str) -> Module {
    let text = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"adopted\"\nfamily = \"arete\"\nhub = \"1.0\"\nrun = [\"C:\\\\x\\\\x.exe\", \"hub\"]\nprovides = [{provides}]\n{extra}"
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

/// Registers `methods` and `facts` as `c` and says ready; returns the `rejected` list.
fn register(r: &mut Router, c: ConnId, methods: &[&str], facts: &[&str], now: Instant) -> Value {
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":methods,"facts":facts,"channels":[],"apis":{}}});
    let out = r.receive(c, &reg.to_string(), now);
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    sends(&out, c)[0]["result"]["rejected"].clone()
}

fn call(r: &mut Router, c: ConnId, id: u64, method: &str, params: Value, now: Instant) -> Vec<Out> {
    let m = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    r.receive(c, &m.to_string(), now)
}

fn code(v: &Value) -> i64 {
    v["error"]["code"].as_i64().expect("an error")
}

const GPU: &[&str] = &["provide:gpu.*", "provide:vulpes.*"];
const ARETE_PROVIDES: &str = r#""method:gpu.*", "method:vulpes.*", "facts:gpu.*""#;

/// The hub with `arete hub` registered as an adopted provider and a caller with `grants`.
fn rig(grants: &[&str], now: Instant) -> (Router, ConnId, ConnId) {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(ARETE, GPU));
    // The manifest says `arete.hub`; the reader writes `vulpes.hub` (§4.3), the id used here.
    r.apply_manifest(&manifest("arete.hub", ARETE_PROVIDES, ""));
    r.add_module(entry(CALLER, Holder::FirstParty, grants, Some(&[])));
    let a = module(&mut r, ARETE, now);
    let rejected = register(&mut r, a, &["gpu.*", "vulpes.*"], &["gpu.*"], now);
    assert_eq!(
        rejected,
        json!([]),
        "an adopted provider registers what it declares"
    );
    let c = module(&mut r, CALLER, now);
    (r, a, c)
}

fn fact_set(r: &mut Router, c: ConnId, id: u64, key: &str, value: Value, now: Instant) -> Vec<Out> {
    call(
        r,
        c,
        id,
        "facts.set",
        json!({"key": key, "value": value}),
        now,
    )
}

/// Everything the hub would send to `conn` or start, as a flat count.
fn effects(out: &[Out]) -> usize {
    out.iter()
        .filter(|o| matches!(o, Out::Send(..) | Out::Activate(_)))
        .count()
}

#[test]
fn game_mode_alone_never_calls_the_reservation_or_touches_gpu_reserved() {
    let now = Instant::now();
    let (mut r, a, _c) = rig(&["gpu.status"], now);
    // Vulpes says the GPU is free.
    fact_set(&mut r, a, 10, "gpu.reserved", json!(false), now);
    assert_eq!(r.fact("gpu.reserved"), Some(&json!(false)));
    // The GAME layer comes and goes (the hub's cache is where the link's fact lands).
    for on in [true, false, true] {
        let out = r.publish("keyboard.game_mode", json!(on));
        assert!(
            !out.iter().any(|o| matches!(o, Out::Activate(_))),
            "game mode started something: {out:?}"
        );
        // Nothing goes to Vulpes: no `action.invoke`, `ui.event` or any other call.
        let to_vulpes = sends(&out, a);
        assert!(
            to_vulpes
                .iter()
                .all(|m| m["method"] != "action.invoke" && m["method"] != "ui.event"),
            "{to_vulpes:?}"
        );
        assert_eq!(r.fact("gpu.reserved"), Some(&json!(false)));
        assert_eq!(r.fact("keyboard.game_mode"), Some(&json!(on)));
    }
    // And the other way: a reservation does not set game mode.
    fact_set(&mut r, a, 11, "gpu.reserved", json!(true), now);
    assert_eq!(r.fact("keyboard.game_mode"), Some(&json!(true)));
    r.publish("keyboard.game_mode", json!(false));
    assert_eq!(
        r.fact("gpu.reserved"),
        Some(&json!(true)),
        "leaving game mode must not release a reservation"
    );
}

#[test]
fn a_cold_vulpes_is_not_started_by_game_mode() {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(ARETE, GPU));
    r.apply_manifest(&manifest(
        "arete.hub",
        ARETE_PROVIDES,
        "[activation]
on = [\"open\", \"call\"]
",
    ));
    assert_eq!(effects(&r.publish("keyboard.game_mode", json!(true))), 0);
    assert_eq!(effects(&r.publish("keyboard.game_mode", json!(false))), 0);
}

#[test]
fn no_module_can_send_the_reservation_action_or_a_button_event() {
    let now = Instant::now();
    // The caller holds every GPU permission there is.
    let (mut r, a, c) = rig(&["gpu.status", "gpu.request:any"], now);
    for (id, method, params) in [
        (
            20,
            "action.invoke",
            json!({"action": "free-gpu", "source": "key"}),
        ),
        (
            21,
            "action.invoke",
            json!({"action": "free-gpu", "source": "tray"}),
        ),
        (
            22,
            "action.invoke",
            json!({"action": "release-gpu-reservation", "source": "panel"}),
        ),
        (
            23,
            "ui.event",
            json!({"panel": "page", "node": "free", "kind": "click"}),
        ),
    ] {
        let out = call(&mut r, c, id, method, params, now);
        let reply = &sends(&out, c)[0];
        assert_eq!(code(reply), -32601, "{method}: {reply}");
        assert!(
            sends(&out, a).is_empty(),
            "{method} reached Vulpes: {out:?}"
        );
    }
    // As a notification too (what a module may send as an event): dropped, not forwarded.
    let note = json!({"jsonrpc":"2.0","method":"action.invoke","params":{"action":"free-gpu","source":"key"}});
    let out = r.receive(c, &note.to_string(), now);
    assert!(sends(&out, a).is_empty(), "{out:?}");
}

#[test]
fn only_vulpes_sets_gpu_reserved() {
    let now = Instant::now();
    let (mut r, a, c) = rig(&["gpu.status"], now);
    let out = fact_set(&mut r, c, 30, "gpu.reserved", json!(true), now);
    assert!(sends(&out, c)[0].get("error").is_some(), "{out:?}");
    assert_eq!(r.fact("gpu.reserved"), None);
    fact_set(&mut r, a, 31, "gpu.reserved", json!(true), now);
    assert_eq!(r.fact("gpu.reserved"), Some(&json!(true)));
}

#[test]
fn a_bound_key_starts_a_cold_module_and_leaves_a_running_one() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(ARETE, GPU));
    r.apply_manifest(&manifest(
        "arete.hub",
        ARETE_PROVIDES,
        "[activation]
on = [\"open\", \"call\"]
",
    ));
    // Cold (on demand, no key): the key binding starts it, once per call.
    assert_eq!(r.key_bound(ARETE), vec![Out::Activate(ARETE.into())]);
    // Connected: nothing to start.
    let a = module(&mut r, ARETE, now);
    register(&mut r, a, &["gpu.*", "vulpes.*"], &["gpu.*"], now);
    assert!(r.key_bound(ARETE).is_empty());
}
