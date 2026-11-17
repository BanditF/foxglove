// SPDX-License-Identifier: MIT
//! A tray menu quick action at the router (contract §4.6 and §5.1, v1.16, #1234): the hub's
//! press is `action.invoke {source: "tray"}`; a cold module is started by the press and gets it
//! once ready, exactly once, or the press fails after its wait; a tray item never starts a module
//! by itself (not at enable, not on game mode). Window-free.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings, NOT_STARTED};
use serde_json::{json, Value};

const ARETE: &str = "vulpes.hub";
const WAIT: Duration = Duration::from_secs(10);

fn adopted(id: &str) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: "AI".into(),
        holder: Holder::ThirdParty(Source::Sideload),
        granted: vec!["provide:gpu.*".into(), "provide:vulpes.*".into()],
        catalog_provide: None,
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: Some("0.1.0".into()),
    }
}

/// Arete's manifest as the UX plan gives it: on demand, two actions, two `action` panels.
fn manifest() -> Module {
    let text = r#"id = "arete.hub"
name = "AI"
version = "0.1.0"
kind = "adopted"
family = "arete"
hub = "1.0"
run = ["C:\\x\\x.exe", "hub"]
provides = ["method:gpu.*", "method:vulpes.*", "facts:gpu.*"]

[activation]
on = ["open", "call"]

[[ui.panels]]
id = "free-gpu"
slot = "action"
title = "Free GPU"
icon = "sparkle"
when = "!gpu.reserved"

[[ui.panels]]
id = "release-gpu-reservation"
slot = "action"
title = "Release GPU reservation"
icon = "sparkle"
when = "gpu.reserved"

[[actions]]
id = "free-gpu"
name = "Free GPU"

[[actions]]
id = "release-gpu-reservation"
name = "Release GPU reservation"
"#;
    Module::parse(text, &Context::default()).expect("manifest")
}

fn rig() -> Router {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(ARETE));
    r.apply_manifest(&manifest());
    r.set_actions(
        ARETE,
        vec!["free-gpu".into(), "release-gpu-reservation".into()],
    );
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

fn invokes(out: &[Out], to: ConnId) -> Vec<Value> {
    sends(out, to)
        .into_iter()
        .filter(|m| m["method"] == "action.invoke")
        .collect()
}

fn activations(out: &[Out]) -> usize {
    out.iter().filter(|o| matches!(o, Out::Activate(_))).count()
}

fn results(out: &[Out]) -> Vec<(u64, Result<Value, Value>)> {
    out.iter()
        .filter_map(|o| match o {
            Out::HubResult { tag, result } => Some((*tag, result.clone())),
            _ => None,
        })
        .collect()
}

/// Connects Arete; `ready` also sends `module.ready`. Returns the connection and what came out.
fn connect(r: &mut Router, ready: bool, now: Instant) -> (ConnId, Vec<Out>) {
    r.set_token(ARETE, "tok-a");
    let c = r.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":ARETE,"version":"0.1.0","instance":"i","token":"tok-a","hub":"1.3","api":{}}});
    let mut out = r.receive(c, &init.to_string(), now);
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    if ready {
        out.extend(r.receive(
            c,
            r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
            now,
        ));
    }
    (c, out)
}

fn press(action: &str) -> Value {
    json!({"action": action, "source": "tray"})
}

#[test]
fn a_tray_press_on_a_running_module_is_one_tray_invoke() {
    let now = Instant::now();
    let mut r = rig();
    let (a, _) = connect(&mut r, true, now);
    let out = r.call_module_starting(ARETE, "action.invoke", press("free-gpu"), 7, now, WAIT);
    assert_eq!(activations(&out), 0);
    let calls = invokes(&out, a);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["params"], press("free-gpu"));
    // The module's answer reaches the hub under the press's tag.
    let id = calls[0]["id"].clone();
    let answer = json!({"jsonrpc": "2.0", "id": id, "result": {}});
    let out = r.receive(a, &answer.to_string(), now);
    assert_eq!(results(&out), vec![(7, Ok(json!({})))]);
}

/// Cold (on demand): the press starts it, waits, and goes exactly once at `module.ready`.
#[test]
fn a_cold_press_starts_the_module_and_is_delivered_once_ready() {
    let now = Instant::now();
    let mut r = rig();
    let out = r.call_module_starting(ARETE, "action.invoke", press("free-gpu"), 1, now, WAIT);
    assert_eq!(out, vec![Out::Activate(ARETE.into())]);
    // A double click while it starts is the same press: busy, nothing more starts.
    let out = r.call_module_starting(ARETE, "action.invoke", press("free-gpu"), 2, now, WAIT);
    assert_eq!(activations(&out), 0);
    let r2 = results(&out);
    assert_eq!(r2.len(), 1);
    assert_eq!(r2[0].1.as_ref().unwrap_err()["code"], 3);
    // The deadline is the wait, for the driver's timer.
    assert_eq!(r.next_deadline(), Some(now + WAIT));
    // Connected but not ready yet: still waiting.
    let (a, out) = connect(&mut r, false, now + Duration::from_secs(2));
    assert!(invokes(&out, a).is_empty());
    // Ready: exactly one press arrives, with the tray source.
    let out = r.receive(
        a,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now + Duration::from_secs(3),
    );
    let calls = invokes(&out, a);
    assert_eq!(calls.len(), 1, "{out:?}");
    assert_eq!(calls[0]["params"], press("free-gpu"));
    // Its wait no longer runs: the tick at 10 s fails nothing.
    let out = r.tick(now + WAIT + Duration::from_millis(1));
    assert!(results(&out).is_empty(), "{out:?}");
}

/// Clicks while the module is connected but not ready yet (review of #1240): they wait for
/// `module.ready` like the first, a repeat is busy, and nothing reaches the module early.
#[test]
fn a_press_while_starting_waits_for_ready_and_is_not_doubled() {
    let now = Instant::now();
    let mut r = rig();
    let out = r.call_module_starting(ARETE, "action.invoke", press("free-gpu"), 1, now, WAIT);
    assert_eq!(activations(&out), 1);
    let (a, _) = connect(&mut r, false, now);
    let out = r.call_module_starting(ARETE, "action.invoke", press("free-gpu"), 2, now, WAIT);
    assert!(
        invokes(&out, a).is_empty(),
        "went to a module not ready: {out:?}"
    );
    assert_eq!(results(&out)[0].1.as_ref().unwrap_err()["code"], 3);
    // Another item while starting: queued too, no second start.
    let other = press("release-gpu-reservation");
    let out = r.call_module_starting(ARETE, "action.invoke", other, 3, now, WAIT);
    assert_eq!(activations(&out), 0);
    assert!(invokes(&out, a).is_empty(), "{out:?}");
    let out = r.receive(
        a,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    let calls = invokes(&out, a);
    assert_eq!(calls.len(), 2, "{out:?}");
    assert_eq!(calls[0]["params"], press("free-gpu"));
}

#[test]
fn a_cold_press_that_never_starts_fails_once_after_its_wait() {
    let now = Instant::now();
    let mut r = rig();
    r.call_module_starting(
        ARETE,
        "action.invoke",
        press("release-gpu-reservation"),
        5,
        now,
        WAIT,
    );
    assert!(results(&r.tick(now + Duration::from_secs(9))).is_empty());
    let out = r.tick(now + WAIT);
    let res = results(&out);
    assert_eq!(res.len(), 1);
    let e = res[0].1.as_ref().unwrap_err();
    assert_eq!((res[0].0, e["code"].as_i64()), (5, Some(4)));
    assert_eq!(e["message"], NOT_STARTED);
    // Once only; a later start delivers nothing.
    assert!(results(&r.tick(now + WAIT * 2)).is_empty());
    let (a, out) = connect(&mut r, true, now + WAIT * 2);
    assert!(invokes(&out, a).is_empty());
}

#[test]
fn a_press_names_a_declared_action_only() {
    let now = Instant::now();
    let mut r = rig();
    let out = r.call_module_starting(ARETE, "action.invoke", press("format-disk"), 9, now, WAIT);
    assert_eq!(activations(&out), 0, "an undeclared action starts nothing");
    let res = results(&out);
    assert_eq!(res[0].1.as_ref().unwrap_err()["code"], -32602);
    // An unknown module: disconnected, nothing starts.
    let out = r.call_module_starting("nobody.here", "action.invoke", press("x"), 10, now, WAIT);
    assert_eq!(activations(&out), 0);
    assert_eq!(results(&out)[0].1.as_ref().unwrap_err()["code"], 2);
}

/// A module that leaves the inventory while a press waits: the press fails, once.
#[test]
fn a_waiting_press_fails_when_its_module_is_removed() {
    let now = Instant::now();
    let mut r = rig();
    r.call_module_starting(ARETE, "action.invoke", press("free-gpu"), 3, now, WAIT);
    let out = r.forget_module(ARETE);
    let res = results(&out);
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].1.as_ref().unwrap_err()["message"], NOT_STARTED);
    assert_eq!(r.next_deadline(), None);
}

/// (b) An `action` panel of an on-demand module does not start it: nothing is due when it is
/// armed, and game mode coming and going starts and presses nothing, with or without a press
/// waiting (§8.9: only the user's click presses it).
#[test]
fn a_tray_item_never_starts_or_presses_by_itself() {
    let now = Instant::now();
    let mut r = rig();
    assert!(r.activation_due(None).is_empty(), "a tray item started it");
    assert_eq!(r.next_deadline(), None);
    for i in 0..100 {
        let out = r.publish("keyboard.game_mode", json!(i % 2 == 0));
        assert_eq!(activations(&out), 0, "game mode started the module");
        assert!(results(&out).is_empty());
    }
    // Running: game mode sends it no `action.invoke`, whatever the source.
    let (a, _) = connect(&mut r, true, now);
    for i in 0..100 {
        let out = r.publish("keyboard.game_mode", json!(i % 2 == 0));
        assert!(invokes(&out, a).is_empty(), "{out:?}");
        assert!(sends(&out, a).iter().all(|m| m["method"] != "ui.event"));
    }
}
