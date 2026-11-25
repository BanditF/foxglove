// SPDX-License-Identifier: MIT
//! Provider lease ids (#1229, contract 1.14 §8.5): every routed caller reaches a provider over
//! the hub's one connection, so the router is what keeps callers' leases apart. A lease ends
//! with its holder's connection, a release, a revoke or the provider's restart, and the
//! provider hears `lease.closed` with its own id; only the holder may name a lease, so a
//! forged, guessed, ended or another caller's id never reaches the provider. Window-free.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::Holder;
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const P: &str = "example.lessor";
const Q: &str = "example.other-lessor";
const A: &str = "example.caller-a";
const B: &str = "example.caller-b";

fn entry(id: &str, provide: &[&str]) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder: Holder::FirstParty,
        granted: provide.iter().map(|p| format!("provide:{p}")).collect(),
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

fn closes(out: &[Out], to: ConnId) -> Vec<(String, String)> {
    sends(out, to)
        .into_iter()
        .filter(|m| m["method"] == "lease.closed")
        .map(|m| {
            (
                m["params"]["lease"].as_str().unwrap().to_string(),
                m["params"]["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn connect(r: &mut Router, id: &str, now: Instant) -> ConnId {
    r.set_token(id, &format!("tok-{id}"));
    let c = r.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":id,"version":"0.1.0","instance":"i","token":format!("tok-{id}"),"hub":"1.3",
        "api":{}}});
    let out = r.receive(c, &init.to_string(), now);
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    c
}

/// Registers `methods` and sends `module.ready`; returns what `module.ready` sent out.
fn ready(r: &mut Router, c: ConnId, methods: &[&str], now: Instant) -> Vec<Out> {
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":methods,"facts":[],"channels":[],"apis":{}}});
    let out = r.receive(c, &reg.to_string(), now);
    assert_eq!(
        sends(&out, c)[0]["result"]["rejected"],
        json!([]),
        "{out:?}"
    );
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    )
}

struct Rig {
    r: Router,
    now: Instant,
    p: ConnId,
    a: ConnId,
    b: ConnId,
    next: u64,
}

impl Rig {
    fn new() -> Rig {
        let now = Instant::now();
        let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
        r.add_module(entry(P, &["example.lessor.*"]));
        r.add_module(entry(Q, &["example.other.*"]));
        r.add_module(entry(A, &[]));
        r.add_module(entry(B, &[]));
        let p = connect(&mut r, P, now);
        ready(&mut r, p, &["example.lessor.*"], now);
        let a = connect(&mut r, A, now);
        let b = connect(&mut r, B, now);
        Rig {
            r,
            now,
            p,
            a,
            b,
            next: 100,
        }
    }

    /// `from` calls `method`; returns everything the call sent out.
    fn call(&mut self, from: ConnId, method: &str, params: Value) -> Vec<Out> {
        self.next += 1;
        let m = json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params});
        self.r.receive(from, &m.to_string(), self.now)
    }

    /// The provider `prov` answers the one call `out` forwarded to it with `result`.
    fn answer(&mut self, prov: ConnId, out: &[Out], result: Value) -> Vec<Out> {
        let fwd: Vec<Value> = sends(out, prov)
            .into_iter()
            .filter(|m| m.get("id").is_some())
            .collect();
        assert_eq!(fwd.len(), 1, "one call forwarded: {out:?}");
        let ans = json!({"jsonrpc":"2.0","id":fwd[0]["id"],"result":result});
        self.r.receive(prov, &ans.to_string(), self.now)
    }

    /// `from` takes a lease from P that P names `id`; returns the caller's answer.
    fn lease(&mut self, from: ConnId, id: Value) -> Value {
        let out = self.call(from, "example.lessor.lease", json!({}));
        let p = self.p;
        let out = self.answer(p, &out, json!({ "lease": id }));
        sends(&out, from)[0].clone()
    }

    /// `from` calls `method` naming `lease`: whether it reached P, and the refusal if not.
    fn names(&mut self, from: ConnId, method: &str, lease: &str) -> (bool, Option<Value>) {
        let out = self.call(from, method, json!({ "lease": lease }));
        let reached = sends(&out, self.p).iter().any(|m| m["method"] == method);
        if reached {
            let p = self.p;
            self.answer(p, &out, json!({}));
        }
        let back = sends(&out, from).first().cloned();
        (reached, back.filter(|v| v.get("error").is_some()))
    }
}

fn unknown(v: &Option<Value>) -> bool {
    v.as_ref().is_some_and(|v| {
        v["error"]["code"] == -32602 && v["error"]["data"]["id"] == "lease.unknown"
    })
}

#[test]
fn the_holders_disconnect_closes_its_lease_and_nobody_elses() {
    let mut t = Rig::new();
    assert_eq!(t.lease(t.a, json!("LA"))["result"]["lease"], "LA");
    assert_eq!(t.lease(t.b, json!("LB"))["result"]["lease"], "LB");
    let out = t.r.disconnect(t.a);
    assert_eq!(closes(&out, t.p), [("LA".into(), "disconnect".into())]);
    // B's lease is untouched: it still reaches the provider, and ends with B.
    assert_eq!(t.names(t.b, "example.lessor.update", "LB"), (true, None));
    let out = t.r.disconnect(t.b);
    assert_eq!(closes(&out, t.p), [("LB".into(), "disconnect".into())]);
}

#[test]
fn a_lease_handed_out_after_its_caller_left_is_closed_at_once() {
    let mut t = Rig::new();
    let out = t.call(t.a, "example.lessor.lease", json!({}));
    t.r.disconnect(t.a);
    let p = t.p;
    let out = t.answer(p, &out, json!({"lease": "LATE"}));
    assert_eq!(closes(&out, t.p), [("LATE".into(), "disconnect".into())]);
    // Nobody holds it: B can't name it.
    assert!(unknown(&t.names(t.b, "example.lessor.release", "LATE").1));
}

#[test]
fn a_release_closes_once_and_the_id_is_dead_after() {
    let mut t = Rig::new();
    t.lease(t.a, json!("LA"));
    let out = t.call(t.a, "example.lessor.release", json!({"lease": "LA"}));
    assert_eq!(closes(&out, t.p), [("LA".into(), "released".into())]);
    let p = t.p;
    t.answer(p, &out, json!({}));
    // Ended: naming it again is refused, and the disconnect sends nothing more.
    let (reached, err) = t.names(t.a, "example.lessor.release", "LA");
    assert!(!reached && unknown(&err), "{err:?}");
    assert!(closes(&t.r.disconnect(t.a), t.p).is_empty());
}

#[test]
fn a_revoked_holder_loses_its_leases_once() {
    let mut t = Rig::new();
    t.lease(t.a, json!("LA"));
    t.lease(t.b, json!("LB"));
    let out = t.r.revoke(A, "0.1.0", "placeholder reason");
    assert_eq!(closes(&out, t.p), [("LA".into(), "revoked".into())]);
    let (reached, err) = t.names(t.a, "example.lessor.update", "LA");
    assert!(!reached && unknown(&err));
    assert!(closes(&t.r.disconnect(t.a), t.p).is_empty(), "sent once");
    assert_eq!(t.names(t.b, "example.lessor.update", "LB"), (true, None));
}

#[test]
fn a_provider_restart_ends_its_leases_and_its_next_connection_hears_of_them_first() {
    let mut t = Rig::new();
    let mut lessor = entry(P, &["example.lessor.*"]);
    lessor.activate_on_call = true;
    t.r.add_module(lessor);
    t.lease(t.a, json!("LA"));
    t.lease(t.b, json!("LB"));
    // The provider's connection ends: nothing to send it, and the leases are over.
    let out = t.r.disconnect(t.p);
    assert!(sends(&out, t.p).is_empty());
    assert!(closes(&out, t.a).is_empty() && closes(&out, t.b).is_empty());
    // A holder naming its old lease is refused at once; it starts nothing.
    let out = t.call(t.a, "example.lessor.release", json!({"lease": "LA"}));
    assert!(unknown(&sends(&out, t.a).first().cloned()), "{out:?}");
    assert!(!out.iter().any(|o| matches!(o, Out::Activate(_))));
    // A plain call waits for the module to start again.
    let out = t.call(t.b, "example.lessor.status", json!({}));
    assert!(out.contains(&Out::Activate(P.into())));
    // The holders' disconnects say nothing to anybody: the leases already ended.
    assert!(closes(&t.r.disconnect(t.a), t.p).is_empty());
    // The next connection gets lease.closed {restart} for each, before the waiting call.
    let p2 = connect(&mut t.r, P, t.now);
    let out = ready(&mut t.r, p2, &["example.lessor.*"], t.now);
    let got = sends(&out, p2);
    let methods: Vec<&str> = got.iter().map(|m| m["method"].as_str().unwrap()).collect();
    assert_eq!(
        methods,
        ["lease.closed", "lease.closed", "example.lessor.status"]
    );
    assert_eq!(
        closes(&out, p2),
        [
            ("LA".into(), "restart".into()),
            ("LB".into(), "restart".into())
        ]
    );
    // Once only: the connection after that hears nothing.
    t.r.disconnect(p2);
    let p3 = connect(&mut t.r, P, t.now);
    assert!(closes(&ready(&mut t.r, p3, &["example.lessor.*"], t.now), p3).is_empty());
}

#[test]
fn a_forged_guessed_or_foreign_lease_never_reaches_the_provider() {
    let mut t = Rig::new();
    t.lease(t.a, json!("LA"));
    // B names A's lease: for a release, an update, anything.
    for m in ["example.lessor.release", "example.lessor.update"] {
        let (reached, err) = t.names(t.b, m, "LA");
        assert!(!reached && unknown(&err), "{m}: {err:?}");
    }
    // An id nobody was handed, and one shaped like a guess.
    for guess in ["nope", "LB", "", "LA "] {
        let (reached, err) = t.names(t.b, "example.lessor.release", guess);
        assert!(!reached && unknown(&err), "{guess:?}");
    }
    // A's lease at P is no lease at another provider.
    let q = connect(&mut t.r, Q, t.now);
    ready(&mut t.r, q, &["example.other.*"], t.now);
    let out = t.call(t.a, "example.other.release", json!({"lease": "LA"}));
    assert!(sends(&out, q).is_empty());
    assert!(unknown(&sends(&out, t.a).first().cloned()));
    // A still holds it, and its own release goes through.
    let out = t.call(t.a, "example.lessor.release", json!({"lease": "LA"}));
    assert_eq!(sends(&out, t.p).len(), 2, "the release, then lease.closed");
    assert_eq!(closes(&out, t.p), [("LA".into(), "released".into())]);
}

#[test]
fn an_id_handed_out_twice_is_never_made_another_callers() {
    let mut t = Rig::new();
    t.lease(t.a, json!("SAME"));
    // The provider hands the live id to B: B gets lease.reused, A keeps it.
    let back = t.lease(t.b, json!("SAME"));
    assert_eq!(back["error"]["code"], 6, "{back}");
    assert_eq!(back["error"]["data"]["id"], "lease.reused");
    let (reached, err) = t.names(t.b, "example.lessor.release", "SAME");
    assert!(!reached && unknown(&err));
    assert!(
        closes(&t.r.disconnect(t.b), t.p).is_empty(),
        "B held nothing"
    );
    assert_eq!(t.names(t.a, "example.lessor.update", "SAME"), (true, None));
    // The same holder getting its own id again is no change.
    assert_eq!(t.lease(t.a, json!("SAME"))["result"]["lease"], "SAME");
}

#[test]
fn a_reissued_id_belongs_to_its_new_holder_only() {
    let mut t = Rig::new();
    t.lease(t.a, json!("L1"));
    t.names(t.a, "example.lessor.release", "L1");
    // The provider reuses the ended id for B (it MUST NOT, but the hub doesn't rely on it).
    assert_eq!(t.lease(t.b, json!("L1"))["result"]["lease"], "L1");
    let (reached, err) = t.names(t.a, "example.lessor.update", "L1");
    assert!(!reached && unknown(&err), "A's old id is not A's any more");
    assert!(closes(&t.r.disconnect(t.a), t.p).is_empty());
    assert_eq!(
        closes(&t.r.disconnect(t.b), t.p),
        [("L1".into(), "disconnect".into())]
    );
}

#[test]
fn no_module_can_send_lease_closed() {
    let mut t = Rig::new();
    // Q is a provider that also holds a lease from P.
    let q = connect(&mut t.r, Q, t.now);
    ready(&mut t.r, q, &["example.other.*"], t.now);
    t.lease(q, json!("LQ"));
    // P tries to make Q drop one of Q's own leases: as a notification addressed to Q's lease,
    // and as a plain one. Neither is passed on.
    for params in [
        json!({"to": "LQ", "lease": "QOWN", "reason": "disconnect"}),
        json!({"lease": "QOWN", "reason": "disconnect"}),
    ] {
        let n = json!({"jsonrpc":"2.0","method":"lease.closed","params":params});
        let out = t.r.receive(t.p, &n.to_string(), t.now);
        assert!(sends(&out, q).is_empty(), "{out:?}");
    }
    // As a call it is a hub-owned name: nobody provides it.
    let out = t.call(
        t.a,
        "lease.closed",
        json!({"lease": "QOWN", "reason": "released"}),
    );
    assert_eq!(sends(&out, t.a)[0]["error"]["code"], -32601);
    assert!(sends(&out, q).is_empty());
}

#[test]
fn a_lease_that_breaks_the_id_rule_is_passed_through_untracked() {
    let mut t = Rig::new();
    // A number (Foxglove's own pipe numbers its leases) and an over-long string.
    assert_eq!(t.lease(t.a, json!(7))["result"]["lease"], 7);
    let long = "x".repeat(hideout_contract::lease::MAX_ID_BYTES + 1);
    assert_eq!(t.lease(t.a, json!(long))["result"]["lease"], json!(long));
    // A number in a call is not a lease to the hub: forwarded as before.
    let out = t.call(t.a, "example.lessor.release", json!({"lease": 7}));
    assert_eq!(sends(&out, t.p).len(), 1);
    let p = t.p;
    t.answer(p, &out, json!({}));
    // The over-long string was never recorded, so it can't be named.
    let (reached, err) = t.names(t.a, "example.lessor.release", &long);
    assert!(!reached && unknown(&err));
    assert!(
        closes(&t.r.disconnect(t.a), t.p).is_empty(),
        "nothing tracked"
    );
}

#[test]
fn a_close_while_the_provider_is_starting_waits_for_its_ready() {
    let mut t = Rig::new();
    t.lease(t.a, json!("LA"));
    // A new instance connects (not ready yet), then the old one ends: the new one holds the
    // module, so the lease lives on.
    let p2 = connect(&mut t.r, P, t.now);
    assert!(sends(&t.r.disconnect(t.p), p2).is_empty());
    // The holder leaves while no connection of the provider is ready: nothing is lost.
    let out = t.r.disconnect(t.a);
    assert!(closes(&out, p2).is_empty(), "p2 isn't ready yet");
    let out = ready(&mut t.r, p2, &["example.lessor.*"], t.now);
    assert_eq!(closes(&out, p2), [("LA".into(), "disconnect".into())]);
}

#[test]
fn a_late_lease_that_reuses_a_live_id_does_not_close_the_holders() {
    let mut t = Rig::new();
    t.lease(t.a, json!("SAME"));
    let out = t.call(t.b, "example.lessor.lease", json!({}));
    t.r.disconnect(t.b);
    let p = t.p;
    let out = t.answer(p, &out, json!({"lease": "SAME"}));
    assert!(closes(&out, t.p).is_empty(), "A's lease must not be closed");
    assert_eq!(t.names(t.a, "example.lessor.update", "SAME"), (true, None));
}

#[test]
fn positional_params_cannot_carry_a_lease_past_the_check() {
    let mut t = Rig::new();
    t.lease(t.a, json!("LA"));
    for params in [json!(["LA"]), json!("LA")] {
        let out = t.call(t.b, "example.lessor.release", params);
        assert!(sends(&out, t.p).is_empty(), "{out:?}");
        assert_eq!(sends(&out, t.b)[0]["error"]["code"], -32602);
    }
}

#[test]
fn one_caller_cannot_take_every_lease_a_provider_has_room_for() {
    use hideout_router::MAX_LEASES_PER_CALLER as CAP;
    let mut t = Rig::new();
    for i in 0..CAP {
        let id = format!("A{i}");
        assert_eq!(t.lease(t.a, json!(id))["result"]["lease"], json!(id));
    }
    // One more: refused with lease.limit, and the provider is told to drop what it made.
    let out = t.call(t.a, "example.lessor.lease", json!({}));
    let p = t.p;
    let out = t.answer(p, &out, json!({"lease": "OVER"}));
    let back = &sends(&out, t.a)[0];
    assert_eq!(back["error"]["code"], 8, "{back}");
    assert_eq!(back["error"]["data"]["id"], "lease.limit");
    assert_eq!(closes(&out, t.p), [("OVER".into(), "revoked".into())]);
    let (reached, err) = t.names(t.a, "example.lessor.release", "OVER");
    assert!(!reached && unknown(&err), "never recorded");
    // A second connection of the same caller shares the room: a reconnect buys none.
    let a2 = connect(&mut t.r, A, t.now);
    assert_eq!(
        t.lease(a2, json!("A2-0"))["error"]["data"]["id"],
        "lease.limit"
    );
    // Another caller is unaffected.
    assert_eq!(t.lease(t.b, json!("B0"))["result"]["lease"], "B0");
    // A gives one back and has room again.
    t.names(t.a, "example.lessor.release", "A0");
    assert_eq!(t.lease(t.a, json!("A-again"))["result"]["lease"], "A-again");
    // Its own id again is no change, even at the cap.
    assert_eq!(t.lease(t.a, json!("A1"))["result"]["lease"], "A1");
}

/// The hub fact `module.<provider>.lease_holders` (v1.17, #1142) the hub's panel host watches.
fn holders(t: &Rig) -> Option<Value> {
    t.r.fact(&format!("module.{P}.lease_holders")).cloned()
}

/// Who holds a lease is a hub fact by name (never ids or reasons), kept on every change; the
/// user's Resume ends every lease of a family at once, each with `revoked`, and the holders'
/// next calls naming them answer `lease.unknown`.
#[test]
fn holders_are_named_and_resume_ends_every_lease_of_the_family() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    assert_eq!(holders(&t), None);
    t.lease(t.a, json!("LA"));
    assert_eq!(holders(&t), Some(json!({ "example.lessor": [A] })));
    t.lease(t.a, json!("LA2"));
    t.lease(t.b, json!("LB"));
    assert_eq!(holders(&t), Some(json!({ "example.lessor": [A, B] })));
    // A release by B leaves A named.
    t.names(t.b, "example.lessor.release", "LB");
    assert_eq!(holders(&t), Some(json!({ "example.lessor": [A] })));
    t.lease(t.b, json!("LB2"));
    // Another family at the same provider is left alone; the fact goes out to the panel host.
    let out = t.r.end_leases(P, "example.other");
    assert!(closes(&out, t.p).is_empty());
    let out = t.r.resume_leases(P, "example.lessor");
    let notes: Vec<Value> = sends(&out, t.p)
        .into_iter()
        .filter(|m| m["method"] == "lease.end")
        .collect();
    assert_eq!(notes.len(), 1, "the provider hears the tray's Resume");
    assert_eq!(notes[0]["params"], json!({ "family": "example.lessor" }));
    let mut ended = closes(&out, t.p);
    ended.sort();
    assert_eq!(
        ended,
        [
            ("LA".into(), "revoked".into()),
            ("LA2".into(), "revoked".into()),
            ("LB2".into(), "revoked".into())
        ]
    );
    assert!(
        sends(&out, t.a).is_empty() && sends(&out, t.b).is_empty(),
        "holders aren't told"
    );
    assert!(out.contains(&Out::Fact {
        key: format!("module.{P}.lease_holders"),
        value: Value::Null
    }));
    assert_eq!(holders(&t), None);
    for (from, id) in [(t.a, "LA"), (t.b, "LB2")] {
        let (reached, err) = t.names(from, "example.lessor.release", id);
        assert!(!reached && unknown(&err), "{id} ended");
    }
    // The caps' counts dropped with them: a disconnect closes nothing more.
    assert!(closes(&t.r.disconnect(t.a), t.p).is_empty());
}

/// A holder's name is drawn: hidden characters dropped, a long one cut; a disconnect takes
/// it off the fact.
#[test]
fn a_holders_name_is_safe_to_draw() {
    let mut t = Rig::new();
    let mut e = entry("example.spoof", &[]);
    e.name = format!("Ar\u{202e}ete\n{}", "x".repeat(60));
    t.r.add_module(e);
    let s = connect(&mut t.r, "example.spoof", t.now);
    t.lease(s, json!("LS"));
    let v = holders(&t).unwrap();
    let name = v["example.lessor"][0].as_str().unwrap().to_string();
    assert!(name.starts_with("Arete x"), "{name}");
    assert!(!name.contains('\u{202e}') && !name.contains('\n'), "{name}");
    assert_eq!(
        name.chars().count(),
        hideout_contract::lease::HOLDER_NAME_MAX
    );
    t.r.disconnect(s);
    assert_eq!(holders(&t), None);
}
/// A provider's own Resume (`lease.end`, v1.17): it ends only its own leases of the family,
/// each closing `revoked`, and the answer counts them.
#[test]
fn a_provider_ends_its_own_leases_of_a_family() {
    let mut t = Rig::new();
    t.lease(t.a, json!("LA"));
    t.lease(t.b, json!("LB"));
    let p = t.p;
    let out = t.call(p, "lease.end", json!({"family": "example.lessor"}));
    let mut ended = closes(&out, p);
    ended.sort();
    assert_eq!(
        ended,
        [
            ("LA".into(), "revoked".into()),
            ("LB".into(), "revoked".into())
        ]
    );
    let back: Vec<Value> = sends(&out, p)
        .into_iter()
        .filter(|m| m.get("id").is_some())
        .collect();
    assert_eq!(back[0]["result"], json!({"ended": 2}), "{back:?}");
    assert_eq!(holders(&t), None);
    // A caller can't end the provider's leases: it holds no provider leases of its own.
    t.lease(t.a, json!("LA2"));
    let out = t.call(t.a, "lease.end", json!({"family": "example.lessor"}));
    assert!(closes(&out, p).is_empty());
    assert_eq!(sends(&out, t.a)[0]["result"], json!({"ended": 0}));
    // A missing family is refused.
    let out = t.call(p, "lease.end", json!({}));
    assert_eq!(sends(&out, p)[0]["error"]["code"], -32602);
}

/// Local holders (v1.21): the module names callers the hub cannot see (its own local API); the
/// hub merges them, cleaned, into the same fact, and drops them with the module's connection.
#[test]
fn a_module_names_its_local_holders_into_the_same_fact() {
    let mut t = Rig::new();
    let p = t.p;
    let _ = t.r.watch_facts(vec![String::new()]);
    let say = |t: &mut Rig, names: Value| {
        t.now += std::time::Duration::from_millis(200);
        let out = t.call(
            p,
            "lease.holders",
            json!({"family": "example.lessor", "names": names}),
        );
        sends(&out, p)
            .into_iter()
            .find(|m| m.get("id").is_some())
            .unwrap()
    };
    // Local only: named with nothing hub-routed.
    assert_eq!(say(&mut t, json!(["Render queue"]))["result"], json!({}));
    assert_eq!(
        holders(&t),
        Some(json!({"example.lessor": ["Render queue"]}))
    );
    // Merged with a hub holder, sorted, each name once; hostile text cleaned and cut at 32.
    t.lease(t.a, json!("LA"));
    let long = "x".repeat(60);
    say(
        &mut t,
        json!([
            "Render queue",
            "Render queue",
            "evil\u{202e}\nname",
            "",
            long
        ]),
    );
    let fact = holders(&t).unwrap();
    let names: Vec<String> = fact["example.lessor"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&A.to_string()) && names.contains(&"Render queue".to_string()));
    assert_eq!(names.iter().filter(|n| *n == "Render queue").count(), 1);
    assert!(
        names
            .iter()
            .all(|n| !n.contains('\u{202e}') && !n.contains('\n')),
        "{names:?}"
    );
    assert!(names
        .iter()
        .all(|n| n.chars().count() <= hideout_contract::lease::HOLDER_NAME_MAX));
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    // More than the cap is cut.
    let many: Vec<String> = (0..40).map(|i| format!("app{i:02}")).collect();
    say(&mut t, json!(many));
    let n = holders(&t).unwrap()["example.lessor"]
        .as_array()
        .unwrap()
        .len();
    assert!(n <= hideout_contract::lease::HOLDERS_MAX_NAMES + 1, "{n}");
    // Bad params are refused; a caller (not a module) is denied.
    assert_eq!(say(&mut t, json!("nope"))["error"]["code"], -32602);
    let out = t.call(
        t.a,
        "lease.holders",
        json!({"family": "example.lessor", "names": ["x"]}),
    );
    assert!(sends(&out, t.a)[0].get("error").is_some());
    // An empty list clears the local names; the hub holder stays.
    say(&mut t, json!([]));
    assert_eq!(holders(&t), Some(json!({"example.lessor": [A]})));
    // They end with the module's connection.
    say(&mut t, json!(["Again"]));
    t.r.disconnect(p);
    assert_eq!(holders(&t), None);
}

/// Local holders are accepted only for a family the module has a route in (a module cannot make
/// the tray show a pause in somebody else's family), and the refusal changes nothing.
#[test]
fn a_module_names_holders_only_in_its_own_family() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    for (from, family) in [
        (t.p, "example.other"),
        (t.p, "wallpaper-engine"),
        (t.a, "example.lessor"),
    ] {
        let out = t.call(
            from,
            "lease.holders",
            json!({"family": family, "names": ["Windows Update"]}),
        );
        let back = sends(&out, from)
            .into_iter()
            .find(|m| m.get("id").is_some())
            .unwrap();
        assert!(back.get("error").is_some(), "{family}: {back}");
    }
    assert_eq!(holders(&t), None);
    let q = connect(&mut t.r, Q, t.now);
    ready(&mut t.r, q, &["example.other.*"], t.now);
    let out = t.call(
        q,
        "lease.holders",
        json!({"family": "example.lessor", "names": ["x"]}),
    );
    assert!(sends(&out, q).iter().any(|m| m.get("error").is_some()));
    assert_eq!(holders(&t), None);
}

/// A swap's new instance does not inherit the old instance's names, and a module cannot flood
/// the fact: a second change inside the gap is refused, a repeat of the same list is free.
#[test]
fn local_holders_end_with_their_connection_and_changes_are_spaced() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let list = |t: &mut Rig, from: ConnId, names: Value| {
        let out = t.call(
            from,
            "lease.holders",
            json!({"family": "example.lessor", "names": names}),
        );
        sends(&out, from)
            .into_iter()
            .find(|m| m.get("id").is_some())
            .unwrap()
    };
    t.now += std::time::Duration::from_millis(500);
    let p = t.p;
    assert!(list(&mut t, p, json!(["Old"])).get("result").is_some());
    assert_eq!(
        list(&mut t, p, json!(["New"]))["error"]["code"],
        8,
        "inside the gap"
    );
    assert_eq!(holders(&t), Some(json!({"example.lessor": ["Old"]})));
    assert!(
        list(&mut t, p, json!(["Old"])).get("result").is_some(),
        "a repeat is free"
    );
    // The new instance connects while the old one is still up, then the old one closes.
    let p2 = connect(&mut t.r, P, t.now);
    ready(&mut t.r, p2, &["example.lessor.*"], t.now);
    t.r.disconnect(p);
    assert_eq!(holders(&t), None, "the old instance's names are gone");
    t.now += std::time::Duration::from_millis(500);
    assert!(list(&mut t, p2, json!(["Mine"])).get("result").is_some());
    assert_eq!(holders(&t), Some(json!({"example.lessor": ["Mine"]})));
}

/// `owed` (v1.23, #1420): a provider that still owes a family a resume keeps the family in the
/// fact as `[]`; the same bounds as the names apply, and names win.
fn say_owed(t: &mut Rig, from: ConnId, family: &str, names: Value, owed: Option<Value>) -> Value {
    t.now += std::time::Duration::from_millis(200);
    let mut params = json!({"family": family, "names": names});
    if let Some(o) = owed {
        params["owed"] = o;
    }
    let out = t.call(from, "lease.holders", params);
    sends(&out, from)
        .into_iter()
        .find(|m| m.get("id").is_some())
        .unwrap()
}

#[test]
fn owed_keeps_an_empty_family_until_it_is_cleared() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let p = t.p;
    let f = "example.lessor";
    assert_eq!(
        say_owed(&mut t, p, f, json!([]), Some(json!(true)))["result"],
        json!({})
    );
    assert_eq!(holders(&t), Some(json!({ f: [] })));
    // The same again is free; owed false with no names removes the family as before.
    assert!(say_owed(&mut t, p, f, json!([]), Some(json!(true)))
        .get("result")
        .is_some());
    assert_eq!(
        say_owed(&mut t, p, f, json!([]), Some(json!(false)))["result"],
        json!({})
    );
    assert_eq!(holders(&t), None);
    // No `owed` at all is false, as before.
    say_owed(&mut t, p, f, json!([]), Some(json!(true)));
    say_owed(&mut t, p, f, json!([]), None);
    assert_eq!(holders(&t), None);
}

#[test]
fn owed_must_be_a_json_bool_and_changes_nothing_when_refused() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let p = t.p;
    for bad in [json!("true"), json!(1), json!(null), json!([true])] {
        let back = say_owed(&mut t, p, "example.lessor", json!([]), Some(bad.clone()));
        assert_eq!(back["error"]["code"], -32602, "{bad}");
        assert!(
            back["error"]["data"]
                .to_string()
                .contains("owed is true or false"),
            "{back}"
        );
    }
    assert_eq!(holders(&t), None);
}

#[test]
fn owed_ends_with_the_connection() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let p = t.p;
    say_owed(&mut t, p, "example.lessor", json!([]), Some(json!(true)));
    assert_eq!(holders(&t), Some(json!({"example.lessor": []})));
    t.r.disconnect(p);
    assert_eq!(holders(&t), None);
}

#[test]
fn owed_is_rate_limited_and_family_checked_like_names() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let p = t.p;
    let f = "example.lessor";
    say_owed(&mut t, p, f, json!([]), Some(json!(true)));
    // A change inside the gap is refused (error 8) and changes nothing.
    let out = t.call(
        p,
        "lease.holders",
        json!({"family": f, "names": [], "owed": false}),
    );
    let back = sends(&out, p)
        .into_iter()
        .find(|m| m.get("id").is_some())
        .unwrap();
    assert_eq!(back["error"]["code"], 8);
    assert_eq!(holders(&t), Some(json!({ f: [] })));
    // A family without a route, or one that is another module's, is refused.
    for (from, fam) in [(p, "example.other"), (p, "wallpaper-engine"), (t.a, f)] {
        let back = say_owed(&mut t, from, fam, json!([]), Some(json!(true)));
        assert!(back.get("error").is_some(), "{fam}: {back}");
    }
    assert_eq!(holders(&t), Some(json!({ f: [] })));
}

#[test]
fn names_win_over_owed_and_no_empty_list_leaks() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let p = t.p;
    let f = "example.lessor";
    say_owed(&mut t, p, f, json!([]), Some(json!(true)));
    // A hub lease joins the owed family: the names, never a `[]` beside them.
    t.lease(t.a, json!("LA"));
    assert_eq!(holders(&t), Some(json!({ f: [A] })));
    // Local names with owed also merge to the names.
    say_owed(&mut t, p, f, json!(["Render queue"]), Some(json!(true)));
    assert_eq!(holders(&t), Some(json!({ f: ["Render queue", A] })));
    // The lease ends: owed shows again as `[]`.
    say_owed(&mut t, p, f, json!([]), Some(json!(true)));
    assert_eq!(holders(&t), Some(json!({ f: [A] })));
    let out = t.call(p, "lease.end", json!({"family": f}));
    let _ = out;
    assert_eq!(holders(&t), Some(json!({ f: [] })));
}

/// #1420 review: a repeat of the same owed state inside the gap is free (not rate limited).
#[test]
fn an_owed_repeat_inside_the_gap_is_free() {
    let mut t = Rig::new();
    let p = t.p;
    let f = "example.lessor";
    say_owed(&mut t, p, f, json!([]), Some(json!(true)));
    // No time passes.
    let out = t.call(
        p,
        "lease.holders",
        json!({"family": f, "names": [], "owed": true}),
    );
    let back = sends(&out, p)
        .into_iter()
        .find(|m| m.get("id").is_some())
        .unwrap();
    assert_eq!(back["result"], json!({}), "{back}");
}

/// #1420 review: owed families count toward the family cap like named ones.
#[test]
fn owed_families_count_toward_the_family_cap() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let max = 8; // HOLDERS_MAX_FAMILIES
    let routes: Vec<String> = (0..=max).map(|i| format!("example.w{i}.*")).collect();
    let routes: Vec<&str> = routes.iter().map(String::as_str).collect();
    t.r.add_module(entry("example.wide", &routes));
    let w = connect(&mut t.r, "example.wide", t.now);
    ready(&mut t.r, w, &routes, t.now);
    for i in 0..max {
        let back = say_owed(
            &mut t,
            w,
            &format!("example.w{i}"),
            json!([]),
            Some(json!(true)),
        );
        assert_eq!(back["result"], json!({}), "family {i}: {back}");
    }
    let over = say_owed(
        &mut t,
        w,
        &format!("example.w{max}"),
        json!([]),
        Some(json!(true)),
    );
    assert!(over.get("error").is_some(), "one past the cap: {over}");
    // A kept family can still change.
    let back = say_owed(&mut t, w, "example.w0", json!(["N"]), None);
    assert_eq!(back["result"], json!({}), "{back}");
}

/// #1420 review: the tray's Resume does not clear `owed`; the provider is the truth and says
/// so itself once its retry settles.
#[test]
fn resume_leaves_the_owed_family_in_place() {
    let mut t = Rig::new();
    let _ = t.r.watch_facts(vec![String::new()]);
    let p = t.p;
    let f = "example.lessor";
    say_owed(&mut t, p, f, json!([]), Some(json!(true)));
    let out = t.r.resume_leases(P, f);
    let notes: Vec<Value> = sends(&out, p)
        .into_iter()
        .filter(|m| m["method"] == "lease.end")
        .collect();
    assert_eq!(notes.len(), 1, "the provider hears the Resume");
    assert_eq!(holders(&t), Some(json!({ f: [] })));
}

/// A program outside Hideout `name` connects and takes a lease from P (it is allowed
/// `lighting.write`, and P provides `lighting.*` here).
fn tool_lease(t: &mut Rig, name: &str, lease: &str) -> ConnId {
    let c = t.r.connect(Origin::Pipe);
    t.r.set_peer_pid(c, 4242);
    t.r.set_peer_identity(
        c,
        Some(hideout_router::ToolId {
            image_path: r"C:\Tools\t.exe".into(),
            image_sha256: "aa".into(),
        }),
    );
    let init = json!({"client":{"name":name,"version":"1"},"hub":"1.4","api":{},
        "permissions":["lighting.write"]});
    let out = t.call(c, "initialize", init);
    assert_eq!(
        sends(&out, c)[0]["result"]["granted"],
        json!(["lighting.write"]),
        "{out:?}"
    );
    let out = t.call(c, "lighting.set", json!({"device": "d", "on": true}));
    let p = t.p;
    let out = t.answer(p, &out, json!({ "lease": lease }));
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    c
}

/// A rig whose provider P also provides `lighting.*`, the family a tool can be let into.
fn lighting_rig() -> Rig {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(P, &["example.lessor.*", "lighting.*"]));
    let mut a = entry(A, &[]);
    a.name = "Arete".into();
    a.granted = vec!["lighting.write".into()];
    r.add_module(a);
    r.add_module(entry(B, &[]));
    let p = connect(&mut r, P, now);
    ready(&mut r, p, &["example.lessor.*", "lighting.*"], now);
    let a = connect(&mut r, A, now);
    let b = connect(&mut r, B, now);
    Rig {
        r,
        now,
        p,
        a,
        b,
        next: 100,
    }
}

/// A program outside Hideout that holds a lease is named by its own word, in quotes (#1466
/// I4): it never reads as the Hideout app of the same name, and never as "An app".
#[test]
fn a_tool_holder_is_its_own_name_in_quotes() {
    let mut t = lighting_rig();
    let c = tool_lease(&mut t, "obs-helper", "L1");
    assert_eq!(
        holders(&t),
        Some(json!({ "lighting": ["\u{201c}obs-helper\u{201d}"] }))
    );
    t.r.disconnect(c);
    assert_eq!(holders(&t), None);

    // A tool that calls itself what a connected app is called: two entries, told apart.
    let out = t.call(t.a, "lighting.set", json!({"device": "d", "on": true}));
    let p = t.p;
    t.answer(p, &out, json!({ "lease": "LA" }));
    assert_eq!(holders(&t), Some(json!({ "lighting": ["Arete"] })));
    let c = tool_lease(&mut t, "Arete", "L2");
    assert_eq!(
        holders(&t),
        Some(json!({ "lighting": ["Arete", "\u{201c}Arete\u{201d}"] }))
    );
    t.r.disconnect(c);
    // The app holder is still its bare display name.
    assert_eq!(holders(&t), Some(json!({ "lighting": ["Arete"] })));
}

/// A tool's long or odd name is cut and cleaned inside the quotes; one with nothing to show
/// is "A program".
#[test]
fn a_tool_holders_name_is_safe_to_draw() {
    let mut t = lighting_rig();
    let long = format!("Ar\u{202e}ete\n\u{7}{}", "x".repeat(100));
    let c = tool_lease(&mut t, &long, "L1");
    let v = holders(&t).unwrap();
    let name = v["lighting"][0].as_str().unwrap().to_string();
    assert!(name.starts_with("\u{201c}Arete x"), "{name}");
    assert!(name.ends_with("\u{2026}\u{201d}"), "{name}");
    assert!(
        name.chars().count() <= hideout_contract::lease::HOLDER_NAME_MAX,
        "{name}"
    );
    assert_eq!(name.chars().count(), Router::TOOL_HOLDER_NAME_MAX + 2);
    assert!(
        !name.chars().any(char::is_control) && !name.contains('\u{202e}'),
        "{name}"
    );
    t.r.disconnect(c);

    // Only hidden characters (a name must not be empty, so it is not blank on the wire).
    let c = tool_lease(&mut t, "\u{202e}\u{200b}", "L2");
    assert_eq!(holders(&t), Some(json!({ "lighting": ["A program"] })));
    t.r.disconnect(c);
    assert_eq!(holders(&t), None);
}
