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
    let out = t.r.end_leases(P, "example.lessor");
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
