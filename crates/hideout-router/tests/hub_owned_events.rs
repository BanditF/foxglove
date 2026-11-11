//! A module can't send hub-owned names (§8.3, v1.12, #1107 review): a provider's addressed
//! event (`to: <lease>`) reaches its lease holder, but a forged `net.data`, `net.closed`,
//! `net.accepted` or `facts.changed` is dropped, addressed or broadcast.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::Holder;
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const A: &str = "foxglove.example-art";
const B: &str = "foxglove.example-scenes";

fn entry(id: &str, granted: &[&str], provide: &[&str]) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder: Holder::FirstParty,
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: Some(provide.iter().map(|s| s.to_string()).collect()),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    }
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

#[test]
fn a_module_cannot_send_hub_owned_events_to_a_lease_holder() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(A, &["provide:example.art.*"], &["example.art.*"]));
    r.add_module(entry(
        B,
        &["net.connect:127.0.0.1:4455", "audio.inputs.read"],
        &[],
    ));
    r.serve(&hideout_router::host_methods());
    let a = module(&mut r, A, now);
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":["example.art.*"],"facts":[],"channels":[],"apis":{}}});
    r.receive(a, &reg.to_string(), now);
    r.receive(
        a,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    let b = module(&mut r, B, now);

    // B takes a lease from A.
    let call = json!({"jsonrpc":"2.0","id":7,"method":"example.art.lease","params":{}});
    let out = r.receive(b, &call.to_string(), now);
    let fwd = sends(&out, a).pop().expect("forwarded to A");
    let answer = json!({"jsonrpc":"2.0","id":fwd["id"],"result":{"lease":"L1"}});
    let out = r.receive(a, &answer.to_string(), now);
    assert_eq!(sends(&out, b)[0]["result"]["lease"], "L1");

    // A's own addressed event reaches B.
    let ok = json!({"jsonrpc":"2.0","method":"example.art.changed","params":{"to":"L1","x":1}});
    assert_eq!(sends(&r.receive(a, &ok.to_string(), now), b).len(), 1);

    // Forged hub-owned names reach nobody, addressed or not.
    for (m, p) in [
        ("net.data", json!({"to":"L1","stream":1,"data":"aGk="})),
        ("net.closed", json!({"to":"L1","stream":1,"reason":"x"})),
        (
            "net.accepted",
            json!({"to":"L1","listener":1,"stream":2,"from":"127.0.0.1:1"}),
        ),
        (
            "facts.changed",
            json!({"to":"L1","subscription":1,"key":"mic.state","value":"unmuted"}),
        ),
        ("net.data", json!({"stream":1,"data":"aGk="})),
        (
            "facts.changed",
            json!({"subscription":1,"key":"mic.state","value":"unmuted"}),
        ),
        // The app watch's events (§8.12, v1.13, #1108).
        (
            "app.window.appeared",
            json!({"to":"L1","exe":"obs64.exe","open":1}),
        ),
        ("app.window.closed", json!({"exe":"obs64.exe","open":0})),
    ] {
        let msg = json!({"jsonrpc":"2.0","method":m,"params":p});
        let out = r.receive(a, &msg.to_string(), now);
        assert!(sends(&out, b).is_empty(), "{m} {p} reached B: {out:?}");
    }
}
