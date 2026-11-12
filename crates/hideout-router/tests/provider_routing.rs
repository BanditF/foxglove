// SPDX-License-Identifier: MIT
//! Provider method routing end to end (contract §4.5, §6.3, §8.3, §8.9, #1118): a service
//! module's `gpu.request` reaches the module that provides `gpu.*` (shaped as Arete's
//! `arete hub`, an adopted module), only with `gpu.request:<class>`; a provider that goes ends
//! the pending call with error 2; and an adopted module provides only what its manifest
//! declares and no root-signed catalog assigns to another module. Window-free.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

/// `arete.hub` as the hub knows it: `arete` is read as `vulpes` (§4.3).
const ARETE: &str = "vulpes.hub";
const CALLER: &str = "example.service";
const VULPES: &str = "foxglove.vulpes";

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

#[test]
fn a_service_modules_gpu_request_reaches_the_provider_and_the_result_returns() {
    let now = Instant::now();
    let (mut r, a, c) = rig(&["gpu.request:chat", "gpu.status"], now);
    let out = call(
        &mut r,
        c,
        5,
        "gpu.request",
        json!({"class":"chat","why":"t"}),
        now,
    );
    let fwd = sends(&out, a).pop().expect("forwarded to the provider");
    assert_eq!(fwd["method"], "gpu.request");
    assert_eq!(fwd["params"]["class"], "chat");
    assert!(sends(&out, c).is_empty());
    let ans = json!({"jsonrpc":"2.0","id":fwd["id"],"result":{"tenant":"chat"}});
    let out = r.receive(a, &ans.to_string(), now);
    assert_eq!(sends(&out, c)[0]["id"], 5);
    assert_eq!(sends(&out, c)[0]["result"]["tenant"], "chat");
    // A provider's own error (3 while reserved for gaming) reaches the caller unchanged.
    let out = call(&mut r, c, 6, "gpu.request", json!({"class":"chat"}), now);
    let fwd = sends(&out, a).pop().unwrap();
    let busy = json!({"jsonrpc":"2.0","id":fwd["id"],"error":{"code":3,"message":"busy"}});
    assert_eq!(code(&sends(&r.receive(a, &busy.to_string(), now), c)[0]), 3);
    // gpu.status needs its own grant and goes to the same provider.
    let out = call(&mut r, c, 7, "gpu.status", json!({}), now);
    assert_eq!(sends(&out, a).len(), 1);
}

#[test]
fn without_the_class_permission_the_caller_gets_5_and_the_provider_sees_nothing() {
    let now = Instant::now();
    // No permission at all; only gpu.status; the wrong class; an unknown class (= any).
    for (grants, class) in [
        (&[][..], "chat"),
        (&["gpu.status"][..], "chat"),
        (&["gpu.request:image"][..], "chat"),
        (&["gpu.request:chat"][..], "image"),
        (&["gpu.request:chat"][..], "any"),
        (&["gpu.request:chat"][..], "bogus"),
    ] {
        let (mut r, a, c) = rig(grants, now);
        let out = call(&mut r, c, 5, "gpu.request", json!({"class":class}), now);
        assert!(
            sends(&out, a).is_empty(),
            "{grants:?} {class}: reached the provider"
        );
        assert_eq!(code(&sends(&out, c)[0]), 5, "{grants:?} {class}");
    }
    // Only the granted class passes; `any` covers every class.
    let (mut r, a, c) = rig(&["gpu.request:chat"], now);
    let out = call(&mut r, c, 1, "gpu.request", json!({"class":"chat"}), now);
    assert_eq!(sends(&out, a).len(), 1);
    let (mut r, a, c) = rig(&["gpu.request:any"], now);
    for (i, class) in ["chat", "image", "any"].iter().enumerate() {
        let out = call(
            &mut r,
            c,
            i as u64 + 1,
            "gpu.request",
            json!({"class":class}),
            now,
        );
        assert_eq!(sends(&out, a).len(), 1, "{class}");
    }
    // Another provider method of the area keeps its own check: ai.ask needs ai.ask:<agent>.
    let (mut r, a, c) = rig(&["gpu.status"], now);
    let out = call(
        &mut r,
        c,
        9,
        "ai.ask",
        json!({"agent":"x","prompt":"hi"}),
        now,
    );
    assert!(sends(&out, a).is_empty());
    assert_eq!(code(&sends(&out, c)[0]), 5);
}

#[test]
fn a_provider_that_goes_ends_the_pending_call_with_error_2_and_later_calls_wait() {
    let now = Instant::now();
    let (mut r, a, c) = rig(&["gpu.request:chat"], now);
    let out = call(&mut r, c, 5, "gpu.request", json!({"class":"chat"}), now);
    assert_eq!(sends(&out, a).len(), 1);
    let out = r.disconnect(a);
    let gone = sends(&out, c);
    assert_eq!(gone.len(), 1, "the pending call ends at once: {out:?}");
    assert_eq!(gone[0]["id"], 5);
    assert_eq!(code(&gone[0]), 2);
    // Down and not activated by call: error 2 with "Waiting for", never -32601 or a hang.
    let out = call(&mut r, c, 6, "gpu.request", json!({"class":"chat"}), now);
    let e = sends(&out, c).pop().expect("answered at once");
    assert_eq!(code(&e), 2);
    assert!(e["error"]["data"]["reason"]
        .as_str()
        .unwrap()
        .contains("Waiting for"));
    // A provider that stays silent: the caller's call fails by the timeout (error 4).
    let (mut r, _a, c) = rig(&["gpu.request:chat"], now);
    call(&mut r, c, 5, "gpu.request", json!({"class":"chat"}), now);
    let late = sends(&r.tick(now + Settings::default().call_timeout), c);
    assert_eq!(late.len(), 1);
    assert_eq!(code(&late[0]), 4);
}

#[test]
fn a_down_provider_with_activation_on_call_is_started_and_the_call_waits() {
    let now = Instant::now();
    let (mut r, a, c) = rig(&["gpu.request:chat"], now);
    r.apply_manifest(&manifest(
        ARETE,
        ARETE_PROVIDES,
        "[activation]\non = [\"call\"]\n",
    ));
    r.disconnect(a);
    let out = call(&mut r, c, 6, "gpu.request", json!({"class":"chat"}), now);
    assert!(
        out.iter()
            .any(|o| matches!(o, Out::Activate(id) if id == ARETE)),
        "{out:?}"
    );
    assert!(sends(&out, c).is_empty(), "queued, not refused");
}

/// The first-party areas (checks.rs) but `gpu.*`, which an adopted module may take.
const CLOSED: &[&str] = &[
    "lighting.*",
    "screens.*",
    "art.*",
    "ai.*",
    "image.*",
    "devices.*",
    "behaviors.*",
    "profiles.*",
    "focus.*",
    "modules.*",
];
const ON_CALL: &str = "[activation]\non = [\"call\"]\n";

#[test]
fn an_adopted_module_provides_only_what_its_manifest_declares() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(
        ARETE,
        &["provide:gpu.*", "provide:art.*", "provide:lighting.set"],
    ));
    r.apply_manifest(&manifest(ARETE, r#""method:gpu.*""#, ""));
    let a = module(&mut r, ARETE, now);
    // art.* is granted but not declared; `*` is neither.
    let rejected = register(
        &mut r,
        a,
        &["gpu.*", "art.*", "lighting.set", "*"],
        &[],
        now,
    );
    assert_eq!(rejected.as_array().unwrap().len(), 3, "{rejected}");
}

#[test]
fn an_adopted_module_never_provides_a_first_party_area_declared_and_granted() {
    let now = Instant::now();
    // Declared in the manifest AND granted, with no root catalog and with one that lists none
    // of them: every closed area is still refused (§6.3).
    for root in [false, true] {
        for area in CLOSED {
            let grant = format!("provide:{area}");
            let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
            r.add_module(adopted(ARETE, &[&grant, "provide:gpu.*"]));
            r.apply_manifest(&manifest(
                ARETE,
                &format!(r#""method:{area}", "method:gpu.*""#),
                "",
            ));
            if root {
                r.set_root_areas(vec![("foxglove.other".into(), "other.*".into())]);
            }
            let a = module(&mut r, ARETE, now);
            let rejected = register(&mut r, a, &[area, "gpu.*"], &[], now);
            assert_eq!(
                rejected,
                json!([{"item": area, "error": "permission.provider"}]),
                "{area} root={root}"
            );
        }
    }
}

#[test]
fn a_refused_adopted_module_is_never_queued_a_call_nor_started() {
    let now = Instant::now();
    // Declares art.* with activation on call: ungranted, then granted but closed. A caller
    // calls art.x: nothing is started, nothing queued (-32601).
    for grants in [&[][..], &["provide:art.*"][..]] {
        let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
        let mut e = adopted(ARETE, grants);
        e.activate_on_call = true;
        r.add_module(e);
        r.apply_manifest(&manifest(ARETE, r#""method:art.*""#, ON_CALL));
        r.add_module(entry(CALLER, Holder::FirstParty, &[], Some(&[])));
        let c = module(&mut r, CALLER, now);
        let out = call(&mut r, c, 5, "art.x", json!({}), now);
        assert!(
            !out.iter().any(|o| matches!(o, Out::Activate(_))),
            "{out:?}"
        );
        assert_eq!(code(&sends(&out, c)[0]), -32601);
    }
}

#[test]
fn a_queued_call_is_dropped_when_the_started_module_has_no_accepted_route() {
    let now = Instant::now();
    // May provide gpu.* (queued and started), but the module registers nothing: on
    // module.ready the queued call ends with error 2 and the module never sees it.
    let (mut r, a, c) = rig(&["gpu.request:chat"], now);
    r.apply_manifest(&manifest("arete.hub", ARETE_PROVIDES, ON_CALL));
    r.disconnect(a);
    let out = call(&mut r, c, 6, "gpu.request", json!({"class":"chat"}), now);
    assert!(out.iter().any(|o| matches!(o, Out::Activate(_))), "{out:?}");
    let a2 = module(&mut r, ARETE, now);
    let out = r.receive(
        a2,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    assert!(
        sends(&out, a2).is_empty(),
        "the provider saw the call: {out:?}"
    );
    assert_eq!(code(&sends(&out, c)[0]), 2);
}

#[test]
fn a_root_catalog_area_is_not_inherited_by_an_adopted_module_with_that_id() {
    let now = Instant::now();
    // The root catalog lists gpu.* for vulpes.hub, which is not installed; the adopted module
    // carrying that id (through `arete.hub`) gets nothing from it (§6.5).
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(ARETE, GPU));
    r.apply_manifest(&manifest("arete.hub", ARETE_PROVIDES, ""));
    r.set_root_areas(vec![(ARETE.into(), "gpu.*".into())]);
    let a = module(&mut r, ARETE, now);
    let rejected = register(&mut r, a, &["gpu.*", "vulpes.*"], &[], now);
    assert_eq!(
        rejected,
        json!([{"item":"gpu.*","error":"permission.provider"}])
    );
}

#[test]
fn a_later_catalog_assignment_drops_the_adopted_route() {
    let now = Instant::now();
    let (mut r, a, c) = rig(&["gpu.request:chat"], now);
    let out = call(&mut r, c, 1, "gpu.request", json!({"class":"chat"}), now);
    assert_eq!(sends(&out, a).len(), 1, "routed before the catalog");
    // A catalog now gives gpu.* to Vulpes: the route goes, vulpes.* stays.
    r.set_root_areas(vec![(VULPES.into(), "gpu.*".into())]);
    let out = call(&mut r, c, 2, "gpu.request", json!({"class":"chat"}), now);
    assert!(sends(&out, a).is_empty(), "{out:?}");
    assert_eq!(code(&sends(&out, c)[0]), -32601);
    let out = call(&mut r, c, 3, "vulpes.x", json!({}), now);
    assert_eq!(sends(&out, a).len(), 1);
}

#[test]
fn a_second_adopted_module_cannot_take_an_overlapping_area() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(ARETE, GPU));
    r.apply_manifest(&manifest(ARETE, ARETE_PROVIDES, ""));
    r.add_module(adopted("other.adopted", &["provide:gpu.*"]));
    r.apply_manifest(&manifest("other.adopted", r#""method:gpu.*""#, ""));
    let a = module(&mut r, ARETE, now);
    assert_eq!(register(&mut r, a, &["gpu.*"], &[], now), json!([]));
    let b = module(&mut r, "other.adopted", now);
    let rejected = register(&mut r, b, &["gpu.*"], &[], now);
    assert_eq!(
        rejected,
        json!([{"item":"gpu.*","error":"permission.provider"}])
    );
}

#[test]
fn a_caller_cannot_queue_without_limit_for_a_provider_that_is_down() {
    let now = Instant::now();
    let (mut r, a, c) = rig(&["gpu.request:chat"], now);
    r.apply_manifest(&manifest("arete.hub", ARETE_PROVIDES, ON_CALL));
    r.disconnect(a);
    let mut last = vec![];
    for i in 0..=64 {
        last = call(
            &mut r,
            c,
            100 + i,
            "gpu.request",
            json!({"class":"chat"}),
            now,
        );
    }
    assert_eq!(code(&sends(&last, c)[0]), 3);
}

#[test]
fn an_adopted_module_never_provides_an_area_another_module_holds() {
    let now = Instant::now();
    // Vulpes, installed from the catalog, holds gpu.*: the adopted module is refused it, even
    // though it declares and was granted it, and Vulpes down does not hand the area over.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(
        VULPES,
        Holder::FirstParty,
        &["provide:gpu.*"],
        Some(&["gpu.*"]),
    ));
    r.add_module(adopted(ARETE, GPU));
    r.apply_manifest(&manifest(ARETE, r#""method:gpu.*", "method:vulpes.*""#, ""));
    r.add_module(entry(
        CALLER,
        Holder::FirstParty,
        &["gpu.request:chat"],
        Some(&[]),
    ));
    let a = module(&mut r, ARETE, now);
    let rejected = register(&mut r, a, &["gpu.*", "vulpes.*"], &[], now);
    assert_eq!(
        rejected,
        json!([{"item":"gpu.*","error":"permission.provider"}])
    );
    let c = module(&mut r, CALLER, now);
    let out = call(&mut r, c, 5, "gpu.request", json!({"class":"chat"}), now);
    assert!(sends(&out, a).is_empty(), "{out:?}");
    assert_eq!(
        code(&sends(&out, c)[0]),
        2,
        "Vulpes is down: waiting for it"
    );
}

#[test]
fn a_sideloaded_service_module_still_cannot_take_gpu() {
    let now = Instant::now();
    // Not adopted: a sideloaded third-party module keeps §6.3's rule (first-party areas).
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(
        "example.side",
        Holder::ThirdParty(Source::Sideload),
        &["provide:gpu.*"],
        None,
    ));
    let s = module(&mut r, "example.side", now);
    let rejected = register(&mut r, s, &["gpu.*"], &[], now);
    assert_eq!(
        rejected,
        json!([{"item":"gpu.*","error":"permission.provider"}])
    );
}
