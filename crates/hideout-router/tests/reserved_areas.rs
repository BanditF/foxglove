// SPDX-License-Identifier: MIT
//! Every area of Foxglove's v1 method table is reserved (#1156 M1, contract section 6.3): no
//! third-party or other adopted module provides it, as methods or as facts, with the one
//! temporary exception of the legacy adopted module `foxglove` for the 11 methods it serves
//! today (#1143 P6; #1155 replaces the exception with an explicit grant). Plus the per-caller
//! in-flight cap on live routes (#1156 M2). Window-free.

use std::collections::BTreeMap;
use std::time::Instant;

use foxglove_plugin_api::methods::METHODS;
use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::checks::{
    covers, FIRST_PARTY_AREAS, HUB_OWNED, LEGACY_FOXGLOVE_ID, LEGACY_FOXGLOVE_METHODS,
};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const FOX: &str = "foxglove";
const CALLER: &str = "example.tool";

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

fn adopted(id: &str, granted: &[&str]) -> ModuleEntry {
    let mut e = entry(id, Holder::ThirdParty(Source::Sideload), granted, None);
    e.registered_version = Some("0.1.0".into());
    e
}

fn manifest(id: &str, provides: &[&str]) -> Module {
    let list: Vec<String> = provides.iter().map(|p| format!("\"{p}\"")).collect();
    let text = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"adopted\"\nfamily = \"foxglove\"\nhub = \"1.0\"\nrun = [\"C:\\\\x\\\\x.exe\"]\nprovides = [{}]\n",
        list.join(", ")
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

fn call(r: &mut Router, c: ConnId, id: u64, method: &str, now: Instant) -> Vec<Out> {
    let m = json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}});
    r.receive(c, &m.to_string(), now)
}

/// Calls `method` as `c`, returns the calls `to` received, and answers each so no slot stays
/// in flight.
fn served_by(r: &mut Router, c: ConnId, to: ConnId, method: &str, now: Instant) -> Vec<Value> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1000);
    let out = call(r, c, NEXT.fetch_add(1, Ordering::SeqCst), method, now);
    let got = sends(&out, to);
    for g in &got {
        let ans = json!({"jsonrpc":"2.0","id":g["id"],"result":{}});
        r.receive(to, &ans.to_string(), now);
    }
    got
}

fn code(v: &Value) -> i64 {
    v["error"]["code"].as_i64().expect("an error")
}

fn provide_grants() -> Vec<String> {
    LEGACY_FOXGLOVE_METHODS
        .iter()
        .map(|m| format!("provide:{m}"))
        .collect()
}

fn method_decls() -> Vec<String> {
    LEGACY_FOXGLOVE_METHODS
        .iter()
        .map(|m| format!("method:{m}"))
        .collect()
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// The hub with the legacy adopted module `foxglove` registered and a calling module.
fn rig(now: Instant) -> (Router, ConnId, ConnId) {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(FOX, &strs(&provide_grants())));
    r.apply_manifest(&manifest(FOX, &strs(&method_decls())));
    r.add_module(entry(
        CALLER,
        Holder::FirstParty,
        &["devices.read", "app.windows", "lighting.write"],
        Some(&[]),
    ));
    let f = module(&mut r, FOX, now);
    let rejected = register(&mut r, f, LEGACY_FOXGLOVE_METHODS, &[], now);
    assert_eq!(
        rejected,
        json!([]),
        "the legacy module keeps its 11 methods"
    );
    let c = module(&mut r, CALLER, now);
    (r, f, c)
}

#[test]
fn every_method_of_the_v1_table_is_in_a_reserved_area() {
    let reserved: Vec<&str> = HUB_OWNED
        .iter()
        .chain(FIRST_PARTY_AREAS.iter())
        .copied()
        .collect();
    for m in METHODS {
        assert!(
            reserved.iter().any(|a| covers(a, m.name)),
            "{} is in no reserved area (HUB_OWNED / FIRST_PARTY_AREAS): add its area",
            m.name
        );
    }
}

#[test]
fn the_legacy_list_is_only_v1_methods_with_a_permission() {
    for m in LEGACY_FOXGLOVE_METHODS {
        let row = foxglove_plugin_api::methods::find(m).expect("a v1 method");
        assert!(row.permission.is_some(), "{m}");
        assert!(!m.contains('*'));
    }
    assert_eq!(LEGACY_FOXGLOVE_ID, "foxglove");
}

#[test]
fn the_legacy_module_keeps_serving_its_11_methods() {
    let now = Instant::now();
    let (mut r, f, c) = rig(now);
    for m in LEGACY_FOXGLOVE_METHODS {
        let routed = served_by(&mut r, c, f, m, now);
        assert_eq!(routed.len(), 1, "{m}");
        assert_eq!(routed[0]["method"], *m);
    }
}

#[test]
fn the_legacy_module_gets_nothing_beyond_the_11() {
    // Wildcards and names off the list are refused whatever the grant covers.
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(
        FOX,
        &[
            "provide:overlay.*",
            "provide:lights.set",
            "provide:health.*",
        ],
    ));
    r.apply_manifest(&manifest(
        FOX,
        &[
            "method:overlay.*",
            "method:lights.set",
            "method:health.*",
            "method:health.read",
        ],
    ));
    let f = module(&mut r, FOX, now);
    let rej = register(
        &mut r,
        f,
        &["overlay.*", "lights.set", "health.*", "health.read"],
        &[],
        now,
    );
    // Only the exact name health.read (declared, covered by the grant `health.*`) is accepted.
    assert_eq!(rej.as_array().unwrap().len(), 3, "{rej:?}");
}

#[test]
fn the_id_alone_grants_nothing() {
    // A catalog entry (third party) with the id `foxglove`, the grants and its own listing.
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    let g = provide_grants();
    r.add_module(entry(
        FOX,
        Holder::ThirdParty(Source::Catalog),
        &strs(&g),
        Some(LEGACY_FOXGLOVE_METHODS),
    ));
    let f = module(&mut r, FOX, now);
    let rej = register(&mut r, f, LEGACY_FOXGLOVE_METHODS, &[], now);
    assert_eq!(rej.as_array().unwrap().len(), 11, "{rej:?}");
    // An adopted module with another id gets nothing either.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted("foxglove.twin", &strs(&g)));
    r.apply_manifest(&manifest("foxglove.twin", &strs(&method_decls())));
    let t = module(&mut r, "foxglove.twin", now);
    let rej = register(&mut r, t, LEGACY_FOXGLOVE_METHODS, &[], now);
    assert_eq!(rej.as_array().unwrap().len(), 11, "{rej:?}");
    // The adopted module `foxglove` without the user's provide: grant gets nothing.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(FOX, &[]));
    r.apply_manifest(&manifest(FOX, &strs(&method_decls())));
    let f = module(&mut r, FOX, now);
    let rej = register(&mut r, f, LEGACY_FOXGLOVE_METHODS, &[], now);
    assert_eq!(rej.as_array().unwrap().len(), 11, "{rej:?}");
}

#[test]
fn a_third_party_catalog_module_granted_overlay_is_refused_and_foxglove_keeps_serving() {
    let now = Instant::now();
    let (mut r, f, c) = rig(now);
    r.add_module(entry(
        "acme.shiny",
        Holder::ThirdParty(Source::Catalog),
        &["provide:overlay.*", "provide:tray.*", "provide:health.*"],
        Some(&["overlay.*", "tray.*", "health.*"]),
    ));
    let t = module(&mut r, "acme.shiny", now);
    let rej = register(
        &mut r,
        t,
        &["overlay.*", "tray.*", "health.read", "lights.page"],
        &[],
        now,
    );
    let rej = rej.as_array().unwrap();
    for item in ["overlay.*", "tray.*", "health.read"] {
        assert!(rej.iter().any(|x| x["item"] == item), "{item}: {rej:?}");
    }
    // Its facts in reserved areas are refused too.
    let rej = register(&mut r, t, &[], &["overlay.*", "health.x"], now);
    assert_eq!(rej.as_array().unwrap().len(), 2, "{rej:?}");
    // Nothing of foxglove's routes moved.
    for m in [
        "overlay.restore",
        "tray.panel",
        "health.read",
        "scenes.page",
    ] {
        assert_eq!(served_by(&mut r, c, f, m, now).len(), 1, "{m}");
        assert!(served_by(&mut r, c, t, m, now).is_empty(), "{m}");
    }
}

#[test]
fn a_third_party_catalog_entry_is_never_queued_a_reserved_call() {
    // The entry lists overlay.* and activates on call: with nobody serving, nothing starts.
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    let mut e = entry(
        "acme.shiny",
        Holder::ThirdParty(Source::Catalog),
        &["provide:overlay.*"],
        Some(&["overlay.*"]),
    );
    e.activate_on_call = true;
    r.add_module(e);
    r.add_module(entry(
        CALLER,
        Holder::FirstParty,
        &["devices.read"],
        Some(&[]),
    ));
    let c = module(&mut r, CALLER, now);
    let out = call(&mut r, c, 1, "overlay.window", now);
    assert!(
        !out.iter().any(|o| matches!(o, Out::Activate(_))),
        "{out:?}"
    );
    assert_eq!(code(&sends(&out, c)[0]), -32601);
}

#[test]
fn a_rival_adopted_module_is_refused_and_does_not_block_foxglove() {
    let now = Instant::now();
    let (mut r, f, c) = rig(now);
    r.add_module(adopted(
        "rival.mod",
        &["provide:tray.*", "provide:overlay.*"],
    ));
    r.apply_manifest(&manifest(
        "rival.mod",
        &["method:tray.*", "method:overlay.*", "facts:tray.*"],
    ));
    let v = module(&mut r, "rival.mod", now);
    let rej = register(&mut r, v, &["tray.*", "overlay.*"], &["tray.*"], now);
    assert_eq!(rej.as_array().unwrap().len(), 3, "{rej:?}");
    for m in ["tray.panel", "tray.set_panel", "overlay.window"] {
        assert_eq!(served_by(&mut r, c, f, m, now).len(), 1, "{m}");
        assert!(served_by(&mut r, c, v, m, now).is_empty(), "{m}");
    }
}

#[test]
fn a_reload_revokes_the_legacy_routes_when_the_grant_or_the_area_goes() {
    let now = Instant::now();
    // A first-party catalog now assigns overlay.*: foxglove's overlay routes go, the rest stay.
    let (mut r, f, c) = rig(now);
    r.set_root_areas(vec![("foxglove.overlay".into(), "overlay.*".into())]);
    let out = call(&mut r, c, 1, "overlay.window", now);
    assert!(sends(&out, f).is_empty(), "{out:?}");
    assert_eq!(code(&sends(&out, c)[0]), -32601);
    let out = call(&mut r, c, 2, "tray.panel", now);
    assert_eq!(sends(&out, f).len(), 1);
    // The user takes the provide: grants back (registry reload): every route goes.
    let (mut r, f, c) = rig(now);
    r.add_module(adopted(FOX, &[]));
    r.set_root_areas(vec![]);
    for m in LEGACY_FOXGLOVE_METHODS {
        let out = call(&mut r, c, 3, m, now);
        assert!(sends(&out, f).is_empty(), "{m}: {out:?}");
    }
}

#[test]
fn one_caller_gets_busy_beyond_its_in_flight_cap_and_others_are_unaffected() {
    use hideout_router::MAX_INFLIGHT_PER_CALLER as CAP;
    let now = Instant::now();
    let (mut r, f, c) = rig(now);
    r.add_module(entry(
        "example.other",
        Holder::FirstParty,
        &["app.windows", "devices.read"],
        Some(&[]),
    ));
    let o = module(&mut r, "example.other", now);
    let mut forwarded = Vec::new();
    for i in 0..CAP {
        let out = call(&mut r, c, 100 + i as u64, "overlay.window", now);
        forwarded.extend(sends(&out, f));
    }
    assert_eq!(forwarded.len(), CAP);
    let out = call(&mut r, c, 200, "overlay.window", now);
    assert!(sends(&out, f).is_empty(), "nothing forwarded past the cap");
    assert_eq!(code(&sends(&out, c)[0]), 3, "BUSY, contract section 8.1");
    // Another caller still gets through.
    let out = call(&mut r, o, 1, "overlay.window", now);
    assert_eq!(sends(&out, f).len(), 1);
    // An answer frees a slot for the capped caller.
    let first = &forwarded[0];
    let ans = json!({"jsonrpc":"2.0","id":first["id"],"result":{}});
    r.receive(f, &ans.to_string(), now);
    let out = call(&mut r, c, 201, "overlay.window", now);
    assert_eq!(sends(&out, f).len(), 1, "{out:?}");
}
