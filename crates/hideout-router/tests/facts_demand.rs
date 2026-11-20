// SPDX-License-Identifier: MIT
//! `facts.demand` (contract §8.8, v1.20): the hub tells the module that owns a fact pattern
//! when subscribers to it appear (0 to some) and when the last one leaves (some to 0), and
//! once after the owner's register result when someone already listens. A count only, to the
//! owner only; the owner's own subscriptions and other modules' patterns don't count.
//! Window-free.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const OWNER: &str = "example.sensors";
const OTHER: &str = "example.other";

fn adopted(id: &str, granted: &[&str]) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder: Holder::ThirdParty(Source::Sideload),
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: None,
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: Some("0.1.0".into()),
    }
}

fn manifest(id: &str, provides: &str) -> Module {
    let text = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"adopted\"\nfamily = \"arete\"\nhub = \"1.5\"\nrun = [\"C:\\\\x\\\\x.exe\", \"hub\"]\nprovides = [{provides}]\n"
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

fn demands(out: &[Out], to: ConnId) -> Vec<Value> {
    sends(out, to)
        .into_iter()
        .filter(|v| v["method"] == "facts.demand")
        .map(|v| v["params"].clone())
        .collect()
}

fn connect(r: &mut Router, id: &str, now: Instant) -> ConnId {
    r.set_token(id, &format!("tok-{id}"));
    let c = r.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":id,"version":"0.1.0","instance":"i","token":format!("tok-{id}"),"hub":"1.5",
        "api":{"foxglove":"1.13"}}});
    let out = r.receive(c, &init.to_string(), now);
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    c
}

fn register(r: &mut Router, c: ConnId, facts: &[&str], now: Instant) -> Vec<Out> {
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":[],"facts":facts,"channels":[],"apis":{}}});
    let out = r.receive(c, &reg.to_string(), now);
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    out
}

fn subscribe(r: &mut Router, c: ConnId, pattern: &str, now: Instant) -> (u64, Vec<Out>) {
    let m = json!({"jsonrpc":"2.0","id":7,"method":"facts.subscribe","params":{"pattern":pattern}});
    let out = r.receive(c, &m.to_string(), now);
    let n = sends(&out, c)[0]["result"]["subscription"]
        .as_u64()
        .expect("subscription");
    (n, out)
}

fn unsubscribe(r: &mut Router, c: ConnId, n: u64, now: Instant) -> Vec<Out> {
    let m =
        json!({"jsonrpc":"2.0","id":8,"method":"facts.unsubscribe","params":{"subscription":n}});
    r.receive(c, &m.to_string(), now)
}

/// A router with the owner registered and two modules that may subscribe to `sensor.*`.
fn rig(now: Instant) -> (Router, ConnId, ConnId, ConnId) {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(OWNER, &["facts.read:sensor.*"]));
    r.apply_manifest(&manifest(OWNER, r#""facts:sensor.*""#));
    r.add_module(adopted(
        OTHER,
        &["facts.read:sensor.*", "facts.read:other.*"],
    ));
    let owner = connect(&mut r, OWNER, now);
    let a = connect(&mut r, OTHER, now);
    let b = {
        r.add_module(adopted("example.third", &["facts.read:sensor.*"]));
        connect(&mut r, "example.third", now)
    };
    register(&mut r, owner, &["sensor.*"], now);
    (r, owner, a, b)
}

#[test]
fn edges_only_to_the_owner_with_a_count() {
    let now = Instant::now();
    let (mut r, owner, a, b) = rig(now);
    // Nobody listens yet: the register result carried no demand.
    let (n1, out) = subscribe(&mut r, a, "sensor.gpu.*", now);
    assert_eq!(
        demands(&out, owner),
        [json!({"pattern":"sensor.*","subscribers":1})]
    );
    // The subscriber's own answer comes first, and it gets no demand.
    assert!(sends(&out, a)[0]["result"].is_object());
    assert!(demands(&out, a).is_empty());
    // A second subscriber is not an edge.
    let (n2, out) = subscribe(&mut r, b, "sensor.*", now);
    assert!(demands(&out, owner).is_empty());
    // One of two leaving is not an edge either.
    assert!(demands(&unsubscribe(&mut r, a, n1, now), owner).is_empty());
    // The last one leaving is.
    let out = unsubscribe(&mut r, b, n2, now);
    assert_eq!(
        demands(&out, owner),
        [json!({"pattern":"sensor.*","subscribers":0})]
    );
    // Only the owner ever hears it.
    assert!(demands(&out, a).is_empty() && demands(&out, b).is_empty());
}

#[test]
fn an_unrelated_pattern_is_not_demand_and_the_owners_own_does_not_count() {
    let now = Instant::now();
    let (mut r, owner, a, _b) = rig(now);
    let (_, out) = subscribe(&mut r, a, "other.*", now);
    assert!(demands(&out, owner).is_empty());
    let (_, out) = subscribe(&mut r, owner, "sensor.*", now);
    assert!(demands(&out, owner).is_empty());
}

#[test]
fn a_closed_subscriber_counts_as_leaving() {
    let now = Instant::now();
    let (mut r, owner, a, _b) = rig(now);
    subscribe(&mut r, a, "sensor.cpu.*", now);
    let out = r.disconnect(a);
    assert_eq!(
        demands(&out, owner),
        [json!({"pattern":"sensor.*","subscribers":0})]
    );
}

#[test]
fn a_registration_while_someone_listens_tells_the_owner_once_after_its_result() {
    let now = Instant::now();
    let (mut r, owner, a, _b) = rig(now);
    subscribe(&mut r, a, "sensor.*", now);
    // The owner restarts: its connection closes, a new one registers.
    r.disconnect(owner);
    let owner2 = connect(&mut r, OWNER, now);
    let out = register(&mut r, owner2, &["sensor.*"], now);
    let msgs = sends(&out, owner2);
    assert!(
        msgs[0]["result"]["rejected"].is_array(),
        "result first: {msgs:?}"
    );
    assert_eq!(msgs[1]["method"], "facts.demand");
    assert_eq!(
        msgs[1]["params"],
        json!({"pattern":"sensor.*","subscribers":1})
    );
    assert_eq!(msgs.len(), 2);
    // A later subscriber is not an edge.
    let (_, out) = subscribe(&mut r, a, "sensor.*", now);
    assert!(demands(&out, owner2).is_empty());
}

/// An overlap swap: old and new instance both hold the module id. The new one's register is
/// told, the old one is not, and the old one leaving changes nothing for the new one.
#[test]
fn an_overlap_swap_tells_the_new_instance_and_not_the_old() {
    let now = Instant::now();
    let (mut r, old, a, _b) = rig(now);
    let (n, _) = subscribe(&mut r, a, "sensor.*", now);
    let new = connect(&mut r, OWNER, now);
    let out = register(&mut r, new, &["sensor.*"], now);
    assert_eq!(
        demands(&out, new),
        [json!({"pattern":"sensor.*","subscribers":1})]
    );
    assert!(demands(&out, old).is_empty(), "{out:?}");
    // The old instance holds a subscription of its own (game mode): not demand, on either.
    let (_, out) = subscribe(&mut r, old, "sensor.*", now);
    assert!(demands(&out, new).is_empty() && demands(&out, old).is_empty());
    // It exits: the new instance hears nothing wrong.
    let out = r.disconnect(old);
    assert!(demands(&out, new).is_empty(), "{out:?}");
    // The last real subscriber leaves: the new instance is told 0.
    let out = unsubscribe(&mut r, a, n, now);
    assert_eq!(
        demands(&out, new),
        [json!({"pattern":"sensor.*","subscribers":0})]
    );
}
