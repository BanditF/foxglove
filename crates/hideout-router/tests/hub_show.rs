//! `hub.show {id}` and the tool's read of `module.foxglove.state` (contract section 8.8, v1.19,
//! H5.5): the hub brings a module's page to the front for a verified same-user peer only, and a
//! foreign, unverified, contained or bridged peer is refused. Mock-only: no pipe, no window.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::Holder;
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const M: &str = "foxglove";

fn entry(id: &str) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder: Holder::FirstParty,
        granted: vec![],
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    }
}

fn router() -> Router {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(M));
    r
}

fn sends(out: &[Out], to: ConnId) -> Vec<Value> {
    out.iter()
        .filter_map(|o| match o {
            Out::Send(c, v) if *c == to => Some(v.clone()),
            _ => None,
        })
        .collect()
}

fn shows(out: &[Out]) -> Vec<String> {
    out.iter()
        .filter_map(|o| match o {
            Out::Show { module } => Some(module.clone()),
            _ => None,
        })
        .collect()
}

fn call(r: &mut Router, c: ConnId, id: i64, method: &str, params: Value) -> Vec<Out> {
    let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    r.receive(c, &msg.to_string(), Instant::now())
}

/// A tool on the pipe whose process the transport named (`pid`), asking `permissions`.
fn tool(r: &mut Router, pid: Option<u32>, container: Option<&str>, perms: &[&str]) -> ConnId {
    let c = r.connect(Origin::Pipe);
    if let Some(pid) = pid {
        r.set_peer_pid(c, pid);
    }
    if let Some(sid) = container {
        r.set_peer_container(c, sid);
    }
    let out = call(
        r,
        c,
        0,
        "initialize",
        json!({"client":{"name":"foxglove-app","version":"1"},"hub":"1.4","api":{},
               "permissions": perms}),
    );
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    c
}

fn error_code(out: &[Out], c: ConnId) -> i64 {
    sends(out, c)[0]["error"]["code"]
        .as_i64()
        .expect("an error")
}

#[test]
fn the_hub_says_it_serves_1_4() {
    let mut r = router();
    let c = r.connect(Origin::Pipe);
    let out = call(
        &mut r,
        c,
        0,
        "initialize",
        json!({"client":{"name":"t","version":"1"},"api":{}}),
    );
    assert_eq!(sends(&out, c)[0]["result"]["hub"], "1.4");
}

#[test]
fn a_verified_same_user_peer_brings_a_modules_page_forward() {
    let mut r = router();
    let c = tool(&mut r, Some(4242), None, &[]);
    let out = call(&mut r, c, 1, "hub.show", json!({"id": M}));
    assert_eq!(sends(&out, c)[0]["result"], json!({}));
    assert_eq!(shows(&out), [M]);
}

#[test]
fn a_peer_whose_process_is_unknown_is_refused() {
    let mut r = router();
    let c = tool(&mut r, None, None, &[]);
    let out = call(&mut r, c, 1, "hub.show", json!({"id": M}));
    assert_eq!(error_code(&out, c), 5, "{out:?}");
    assert!(shows(&out).is_empty());
}

#[test]
fn a_contained_peer_is_refused() {
    let mut r = router();
    let c = tool(&mut r, Some(77), Some("S-1-15-2-1-2-3"), &[]);
    let out = call(&mut r, c, 1, "hub.show", json!({"id": M}));
    assert_eq!(error_code(&out, c), 5, "{out:?}");
    assert!(shows(&out).is_empty());
}

#[test]
fn a_bridged_plugin_and_an_uninitialized_connection_are_refused() {
    let mut r = router();
    let b = r.connect(Origin::Bridged(M.into()));
    r.set_peer_pid(b, 5);
    let out = call(
        &mut r,
        b,
        0,
        "initialize",
        json!({"client":{"name":"x","version":"1"},"api":"1.3"}),
    );
    assert!(sends(&out, b)[0].get("result").is_some(), "{out:?}");
    let out = call(&mut r, b, 1, "hub.show", json!({"id": M}));
    assert_eq!(error_code(&out, b), 5, "{out:?}");
    assert!(shows(&out).is_empty());

    let n = r.connect(Origin::Pipe);
    r.set_peer_pid(n, 9);
    let out = call(&mut r, n, 1, "hub.show", json!({"id": M}));
    assert_eq!(error_code(&out, n), 5, "{out:?}");
    assert!(shows(&out).is_empty());
}

#[test]
fn an_unknown_id_and_bad_params_show_nothing() {
    let mut r = router();
    let c = tool(&mut r, Some(4242), None, &[]);
    for params in [
        json!({"id": "nope"}),
        json!({}),
        json!({"id": 3}),
        json!({"id": M, "extra": true}),
    ] {
        let out = call(&mut r, c, 9, "hub.show", params);
        assert_eq!(error_code(&out, c), -32602, "{out:?}");
        assert!(shows(&out).is_empty());
    }
}

#[test]
fn a_tool_reads_the_one_state_fact_it_was_granted_and_no_other() {
    let mut r = router();
    r.publish("module.foxglove.state", json!("running"));
    r.publish("module.other.state", json!("running"));
    let c = tool(&mut r, Some(1), None, &["facts.read:module.foxglove.state"]);
    let out = call(
        &mut r,
        c,
        1,
        "facts.get",
        json!({"key": "module.foxglove.state"}),
    );
    assert_eq!(sends(&out, c)[0]["result"]["value"], "running");
    let out = call(
        &mut r,
        c,
        2,
        "facts.get",
        json!({"key": "module.other.state"}),
    );
    assert_eq!(error_code(&out, c), 5, "{out:?}");
    // Not granted at all: refused, and hub.show still needs no grant.
    let d = tool(&mut r, Some(2), None, &[]);
    let out = call(
        &mut r,
        d,
        1,
        "facts.get",
        json!({"key": "module.foxglove.state"}),
    );
    assert_eq!(error_code(&out, d), 5, "{out:?}");
}
