// SPDX-License-Identifier: MIT
//! `module.grants` (contract §8.8, v1.22): when the user's Allow widens what a running module
//! holds, the hub tells that module (and nobody else) what was added, and the SDK session sends
//! the subscriptions the hub had denied again. So a module that subscribed before the Allow
//! hears the fact without a restart. The real router and the real SDK session, joined in
//! memory. Window-free.

use std::collections::BTreeMap;
use std::time::Instant;

use foxglove_plugin::hideout::{Handler, Incoming, Session};
use hideout_contract::capability::{Holder, Source};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const ID: &str = "example.sensors";
const OTHER: &str = "example.other";
const GAME: &str = "keyboard.game_mode";
const READ_GAME: &str = "facts.read:keyboard.game_mode";

fn entry(id: &str, granted: &[&str]) -> ModuleEntry {
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

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn sends(out: &[Out], to: ConnId) -> Vec<Value> {
    out.iter()
        .filter_map(|o| match o {
            Out::Send(c, v) if *c == to => Some(v.clone()),
            _ => None,
        })
        .collect()
}

fn grants(out: &[Out], to: ConnId) -> Vec<Value> {
    sends(out, to)
        .into_iter()
        .filter(|v| v["method"] == "module.grants")
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
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":[],"facts":[],"channels":[],"apis":{}}});
    r.receive(c, &reg.to_string(), now);
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    c
}

/// What the module's handler saw.
#[derive(Default)]
struct Seen {
    notes: Vec<(String, Value)>,
}
impl Handler for Seen {
    fn notification(&mut self, method: &str, params: Value) {
        self.notes.push((method.into(), params));
    }
}

/// A module's SDK session on connection `conn` of the router: what the session writes goes to
/// the router, and what the router sends that connection goes back into the session, until
/// both are quiet.
struct Wire {
    conn: ConnId,
    session: Session<std::io::Empty, Vec<u8>>,
    seen: Seen,
    /// Every answer the session got: (method, ok).
    answers: Vec<(String, bool)>,
    /// The session's ids start above the ones `connect` used.
    now: Instant,
}

impl Wire {
    fn new(conn: ConnId, now: Instant) -> Wire {
        Wire {
            conn,
            session: Session::new(std::io::empty(), Vec::new()),
            seen: Seen::default(),
            answers: Vec::new(),
            now,
        }
    }

    /// Hands the router's output for this connection to the session, and what the session
    /// sends in return to the router, until nothing moves.
    fn pump(&mut self, r: &mut Router, mut out: Vec<Out>) {
        for _ in 0..16 {
            for msg in sends(&out, self.conn) {
                if let Incoming::Response {
                    method, outcome, ..
                } = self.session.handle(msg, &mut self.seen).unwrap()
                {
                    self.answers.push((method, outcome.is_ok()));
                }
            }
            let sent = std::mem::take(self.session.writer_mut());
            if sent.is_empty() {
                return;
            }
            out = Vec::new();
            for line in String::from_utf8(sent).unwrap().lines() {
                out.extend(r.receive(self.conn, line, self.now));
            }
        }
        panic!("the session and the router never went quiet");
    }

    fn changed(&self) -> Vec<Value> {
        self.seen
            .notes
            .iter()
            .filter(|(m, _)| m == "facts.changed")
            .map(|(_, p)| p["value"].clone())
            .collect()
    }
}

/// The real case (the sensors module and `keyboard.game_mode`): subscribed before the user's
/// Allow and denied, then allowed while it runs. The fact arrives with no restart.
#[test]
fn a_subscription_denied_before_the_allow_delivers_after_it_without_a_restart() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    // Adopted into the hub, nothing allowed yet.
    r.add_module(entry(ID, &[]));
    let conn = connect(&mut r, ID, now);
    let mut m = Wire::new(conn, now);

    // Ids 0 and 1 went to `connect`; the session's own start at 1, so move past them.
    m.session.request("ping", json!({})).unwrap();
    m.session.request("ping", json!({})).unwrap();
    m.pump(&mut r, Vec::new());
    m.answers.clear();

    m.session.subscribe(GAME).unwrap();
    m.pump(&mut r, Vec::new());
    assert_eq!(m.answers, [("facts.subscribe".to_string(), false)]);
    assert_eq!(
        m.session.denied_subscriptions(),
        [json!({ "pattern": GAME })]
    );
    // Denied means denied: the fact changes and the module hears nothing.
    let out = r.publish(GAME, json!(true));
    m.pump(&mut r, out);
    assert!(m.changed().is_empty());

    // The user's Allow, while the module runs.
    let out = r.set_module_grants(ID, s(&[READ_GAME]));
    assert_eq!(grants(&out, conn), [json!({ "added": [READ_GAME] })]);
    m.pump(&mut r, out);
    assert_eq!(
        m.answers,
        [
            ("facts.subscribe".to_string(), false),
            ("facts.subscribe".to_string(), true)
        ],
        "the session did not subscribe again"
    );
    assert!(m.session.denied_subscriptions().is_empty());
    // The module's own code hears the notification too (to read the value as it is now).
    assert!(m.seen.notes.iter().any(|(n, _)| n == "module.grants"));

    let out = r.publish(GAME, json!(false));
    m.pump(&mut r, out);
    assert_eq!(m.changed(), [json!(false)]);
}

/// Only what was added, only on a widening, only to the module it is about.
#[test]
fn only_a_widening_is_announced_and_only_to_that_module() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(ID, &["net"]));
    r.add_module(entry(OTHER, &[]));
    let a = connect(&mut r, ID, now);
    let a2 = connect(&mut r, ID, now);
    let b = connect(&mut r, OTHER, now);

    // The same set again: nothing.
    let out = r.set_module_grants(ID, s(&["net"]));
    assert!(grants(&out, a).is_empty() && grants(&out, a2).is_empty());

    // One more item: that item only, on each of the module's connections, never to another.
    let out = r.set_module_grants(ID, s(&["net", READ_GAME]));
    assert_eq!(grants(&out, a), [json!({ "added": [READ_GAME] })]);
    assert_eq!(grants(&out, a2), [json!({ "added": [READ_GAME] })]);
    assert!(sends(&out, b).is_empty(), "{out:?}");

    // Narrowing (an Allow that names less, a turn-off, the file) announces nothing.
    let out = r.set_module_grants(ID, s(&["net"]));
    assert!(grants(&out, a).is_empty());
    let out = r.drop_module_grants(ID, None);
    assert!(grants(&out, a).is_empty());
    let out = r.narrow_module_grants(&Default::default());
    assert!(grants(&out, a).is_empty());

    // A module that is not connected gets nothing queued for later.
    let out = r.set_module_grants("example.absent", s(&["net"]));
    assert!(out.iter().all(|o| !matches!(o, Out::Send(..))), "{out:?}");
}

/// The notification grants nothing by itself: a session told its grants widened asks again,
/// and the router's own check still decides.
#[test]
fn a_resubscribe_the_grant_does_not_cover_stays_denied() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(ID, &[]));
    let conn = connect(&mut r, ID, now);
    let mut m = Wire::new(conn, now);
    m.session.request("ping", json!({})).unwrap();
    m.session.request("ping", json!({})).unwrap();
    m.pump(&mut r, Vec::new());
    m.answers.clear();

    m.session.subscribe(GAME).unwrap();
    m.session.subscribe(GAME).unwrap();
    m.pump(&mut r, Vec::new());
    // The same pattern twice is kept once.
    assert_eq!(m.session.denied_subscriptions().len(), 1);

    // An Allow for something else: asked again once, denied again, kept for the next Allow.
    let out = r.set_module_grants(ID, s(&["net"]));
    m.pump(&mut r, out);
    assert_eq!(m.answers.len(), 3);
    assert!(m.answers.iter().all(|(_, ok)| !ok));
    assert_eq!(m.session.denied_subscriptions().len(), 1);
    let out = r.publish(GAME, json!(true));
    m.pump(&mut r, out);
    assert!(m.changed().is_empty());

    // A forged `module.grants` from the module's side is not a hub message: it changes nothing.
    let out = r.receive(
        conn,
        &json!({"jsonrpc":"2.0","method":"module.grants","params":{"added":[READ_GAME]}})
            .to_string(),
        now,
    );
    m.pump(&mut r, out);
    let out = r.publish(GAME, json!(false));
    m.pump(&mut r, out);
    assert!(m.changed().is_empty());
    assert!(r.module_grants(ID) == s(&["net"]));
}

/// The other way round: a turn-off reaches the running connection at once. Its subscription
/// ends and its next read is denied, with no restart.
#[test]
fn a_turn_off_ends_the_running_modules_subscription_and_reads() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(ID, &[READ_GAME]));
    let conn = connect(&mut r, ID, now);
    let mut m = Wire::new(conn, now);
    m.session.request("ping", json!({})).unwrap();
    m.session.request("ping", json!({})).unwrap();
    m.pump(&mut r, Vec::new());
    m.answers.clear();

    m.session.subscribe(GAME).unwrap();
    m.pump(&mut r, Vec::new());
    assert_eq!(m.answers, [("facts.subscribe".to_string(), true)]);
    let out = r.publish(GAME, json!(true));
    m.pump(&mut r, out);
    assert_eq!(m.changed(), [json!(true)]);

    let out = r.drop_module_grants(ID, None);
    assert!(grants(&out, conn).is_empty());
    m.pump(&mut r, out);
    let out = r.publish(GAME, json!(false));
    m.pump(&mut r, out);
    assert_eq!(
        m.changed(),
        [json!(true)],
        "heard a fact after the turn-off"
    );
    m.session
        .request("facts.get", json!({ "key": GAME }))
        .unwrap();
    m.pump(&mut r, Vec::new());
    assert_eq!(m.answers.last(), Some(&("facts.get".to_string(), false)));
}

/// The session alone: only a denial is kept, at most 64 of them (the oldest goes first).
#[test]
fn the_session_keeps_only_denials_and_caps_them() {
    let mut session = Session::new(std::io::empty(), Vec::new());
    let mut seen = Seen::default();
    let id = session.subscribe("gpu.*").unwrap();
    session
        .handle(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":4,"message":"timeout"}}),
            &mut seen,
        )
        .unwrap();
    assert!(session.denied_subscriptions().is_empty());
    for n in 0..70 {
        let id = session.subscribe(&format!("a.{n}")).unwrap();
        session
            .handle(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":5,"message":"denied"}}),
                &mut seen,
            )
            .unwrap();
    }
    let kept = session.denied_subscriptions();
    assert_eq!(kept.len(), 64);
    assert_eq!(kept[0], json!({"pattern": "a.6"}));
    assert_eq!(kept[63], json!({"pattern": "a.69"}));
}
