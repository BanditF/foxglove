// SPDX-License-Identifier: MIT
//! A module that left the inventory (#1054): the router drops its entry, token and
//! `module.<id>.*` facts, telling the hub's fact watch; a module still connected is kept.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::Holder;
use hideout_router::{ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const ART: &str = "foxglove.example-art";
const BEAT: &str = "foxglove.example-beat";

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
    }
}

fn initialize(r: &mut Router) -> (u64, Value) {
    let c = r.connect(Origin::Pipe);
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"id": ART, "version": "0.4.0", "instance": "i", "token": "tok",
                   "hub": "1.0", "api": {"foxglove": "1.13"}}});
    let out = r.receive(c, &msg.to_string(), Instant::now());
    let answer = out
        .iter()
        .find_map(|o| match o {
            Out::Send(_, v) => Some(v.clone()),
            _ => None,
        })
        .unwrap();
    (c, answer)
}

#[test]
fn a_removed_module_is_forgotten_once_it_is_gone() {
    let mut r = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    r.add_module(entry(ART));
    r.add_module(entry(BEAT));
    r.set_token(ART, "tok");
    let _ = r.watch_facts(vec!["module.".into()]);
    let _ = r.publish(&format!("module.{ART}.state"), json!("running"));
    let _ = r.publish(&format!("module.{BEAT}.state"), json!("running"));

    // Still connected: kept, facts and all.
    let (c, answer) = initialize(&mut r);
    assert!(answer.get("result").is_some(), "{answer}");
    assert!(r.forget_module(ART).is_empty());
    assert!(r.knows_module(ART));

    let _ = r.disconnect(c);
    let out = r.forget_module(ART);
    let key = format!("module.{ART}.state");
    assert!(
        out.contains(&Out::Fact {
            key: key.clone(),
            value: Value::Null
        }),
        "the hub's fact watch wasn't told: {out:?}"
    );
    assert!(r.fact(&key).is_none());
    assert!(!r.knows_module(ART));
    // Its old launch token no longer starts a session.
    let (_, answer) = initialize(&mut r);
    assert!(answer.get("error").is_some(), "{answer}");
    // The other module is untouched; forgetting twice is a no-op.
    assert!(r.knows_module(BEAT));
    assert_eq!(
        r.fact(&format!("module.{BEAT}.state")),
        Some(&json!("running"))
    );
    assert!(r.forget_module(ART).is_empty());
}
