// SPDX-License-Identifier: MIT
//! Activation events `open`, `fact:<pattern>`, `app:<name>.exe` and `device:<class>` (contract
//! §4.6, v1.10, #983): for each kind, no `Activate` before its event and exactly one after it;
//! none for a false value, another key or a module already connected. Window-free: the router
//! has no I/O.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::Holder;
use hideout_contract::module::{Context, Module};
use hideout_router::{ModuleEntry, Origin, Out, Router, Settings, Triggers};
use serde_json::json;

const ID: &str = "foxglove.example-art";

fn entry() -> ModuleEntry {
    ModuleEntry {
        id: ID.into(),
        name: "Example Art".into(),
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

/// A router that knows the module, armed from a manifest with `on`.
fn router(on: &str) -> Router {
    let text = format!(
        r#"id = "{ID}"
name = "Example Art"
version = "0.4.0"
kind = "service"
family = "foxglove"
hub = "1.0"
run = ["example-art.exe"]

[activation]
on = [{on}]
"#
    );
    let m = Module::parse(&text, &Context::default()).expect("manifest");
    let mut r = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    r.add_module(entry());
    r.apply_manifest(&m);
    r
}

fn activations(out: &[Out]) -> usize {
    out.iter()
        .filter(|o| matches!(o, Out::Activate(id) if id == ID))
        .count()
}

#[test]
fn manifest_entries_become_triggers() {
    let r = router(r#""open", "fact:keyboard.*", "app:Example-App.exe", "device:keyboard.split""#);
    assert_eq!(
        r.triggers(ID),
        Some(&Triggers {
            open: true,
            facts: vec![
                "keyboard.*".into(),
                "apps.running.example-app".into(),
                "devices.present.keyboard.split".into(),
            ],
        })
    );
    // enable and call alone hold no router trigger.
    assert_eq!(router(r#""enable", "call""#).triggers(ID), None);
}

#[test]
fn open_starts_the_module_once_its_page_is_shown() {
    let mut r = router(r#""open""#);
    // Facts and other modules' pages start nothing.
    assert_eq!(
        activations(&r.publish("keyboard.connected", json!(true))),
        0
    );
    assert_eq!(activations(&r.panel_opened("foxglove.other")), 0);
    assert_eq!(activations(&r.panel_opened(ID)), 1);
    // A module without `open` doesn't start on its page.
    let mut r = router(r#""enable""#);
    assert_eq!(activations(&r.panel_opened(ID)), 0);
}

#[test]
fn fact_starts_the_module_when_a_matching_fact_turns_true() {
    let mut r = router(r#""fact:keyboard.*""#);
    assert_eq!(activations(&r.publish("mouse.connected", json!(true))), 0);
    // Set but false by the `when` rule: not yet.
    for v in [json!(false), json!(0), json!(""), json!(null)] {
        assert_eq!(activations(&r.publish("keyboard.connected", v)), 0);
    }
    assert_eq!(
        activations(&r.publish("keyboard.connected", json!(true))),
        1
    );
    // The same value again is no change: nothing more.
    assert_eq!(
        activations(&r.publish("keyboard.connected", json!(true))),
        0
    );
}

#[test]
fn app_starts_the_module_when_the_app_watch_sees_it_run() {
    let mut r = router(r#""app:Example-App.exe""#);
    // The app watch's first word: not running.
    assert_eq!(
        activations(&r.publish("apps.running.example-app", json!(false))),
        0
    );
    assert_eq!(
        activations(&r.publish("apps.seen.example-app", json!(true))),
        0
    );
    assert_eq!(
        activations(&r.publish("apps.running.other", json!(true))),
        0
    );
    assert_eq!(
        activations(&r.publish("apps.running.example-app", json!(true))),
        1
    );
}

#[test]
fn device_starts_the_module_when_its_class_is_present() {
    let mut r = router(r#""device:keyboard""#);
    assert_eq!(
        activations(&r.publish("devices.present.mouse", json!(true))),
        0
    );
    assert_eq!(
        activations(&r.publish("devices.present.keyboard", json!(false))),
        0
    );
    assert_eq!(
        activations(&r.publish("devices.present.keyboard", json!(true))),
        1
    );
}

#[test]
fn a_fact_set_before_arming_counts_once_armed() {
    let mut r = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    let _ = r.publish("devices.present.keyboard", json!(true));
    assert!(r.activation_due(None).is_empty(), "nothing armed yet");
    let text = format!(
        "id = \"{ID}\"\nname = \"A\"\nversion = \"0.4.0\"\nkind = \"service\"\nfamily = \"foxglove\"\nhub = \"1.0\"\nrun = [\"a.exe\"]\n[activation]\non = [\"device:keyboard\"]\n"
    );
    r.add_module(entry());
    r.apply_manifest(&Module::parse(&text, &Context::default()).unwrap());
    assert_eq!(activations(&r.activation_due(None)), 1);
    assert_eq!(activations(&r.activation_due(Some(ID))), 1);
    assert_eq!(activations(&r.activation_due(Some("foxglove.other"))), 0);
}

#[test]
fn a_connected_module_is_not_activated_again_and_a_forgotten_one_never() {
    let mut r = router(r#""open", "fact:keyboard.*""#);
    r.set_token(ID, "tok");
    let c = r.connect(Origin::Pipe);
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"id": ID, "version": "0.4.0", "instance": "i", "token": "tok",
                   "hub": "1.0", "api": {"foxglove": "1.13"}}});
    let _ = r.receive(c, &msg.to_string(), Instant::now());
    assert_eq!(activations(&r.panel_opened(ID)), 0);
    assert_eq!(
        activations(&r.publish("keyboard.connected", json!(true))),
        0
    );
    assert!(r.activation_due(None).is_empty());
    // Gone again (its own clean exit): the next change starts it.
    let _ = r.disconnect(c);
    assert_eq!(
        activations(&r.publish("keyboard.connected", json!(false))),
        0
    );
    assert_eq!(
        activations(&r.publish("keyboard.connected", json!(true))),
        1
    );
    // Removed from the inventory: no trigger left.
    let _ = r.forget_module(ID);
    assert_eq!(r.triggers(ID), None);
    assert_eq!(activations(&r.panel_opened(ID)), 0);
}
