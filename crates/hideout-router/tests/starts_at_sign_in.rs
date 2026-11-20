// SPDX-License-Identifier: MIT
//! `module.<id>.starts_at_sign_in` (sign-in hand-over slice 2): the hub tells each running module
//! whether Hideout starts at sign-in, read-only, so an app can tell whether it needs its own
//! startup entry. Mock-only: no pipe, no window, no system change.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::Holder;
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const M: &str = "foxglove";
const KEY: &str = "module.foxglove.starts_at_sign_in";

fn router() -> Router {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(ModuleEntry {
        id: M.into(),
        name: M.into(),
        holder: Holder::FirstParty,
        granted: vec!["facts.read:module.*".to_string()],
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    });
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

fn ready(r: &mut Router, now: Instant) -> ConnId {
    r.set_token(M, "tok");
    let c = r.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":M,"version":"0.1.0","instance":"i","token":"tok","hub":"1.3",
        "api":{"foxglove":"1.13"}}});
    let out = r.receive(c, &init.to_string(), now);
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    c
}

#[test]
fn a_module_that_becomes_ready_gets_the_current_value() {
    let now = Instant::now();
    let mut r = router();
    assert_eq!(r.fact(KEY), None, "nothing before the module runs");
    r.set_starts_at_sign_in(true);
    assert_eq!(
        r.fact(KEY),
        None,
        "a module that is not running gets no fact"
    );
    ready(&mut r, now);
    assert_eq!(r.fact("module.foxglove.state"), Some(&json!("running")));
    assert_eq!(r.fact(KEY), Some(&json!(true)));
}

#[test]
fn a_ready_module_without_the_switch_gets_false() {
    let now = Instant::now();
    let mut r = router();
    ready(&mut r, now);
    assert_eq!(r.fact(KEY), Some(&json!(false)));
}

#[test]
fn a_change_reaches_running_modules_and_only_changes_publish() {
    let now = Instant::now();
    let mut r = router();
    let c = ready(&mut r, now);
    let sub = json!({"jsonrpc":"2.0","id":5,"method":"facts.subscribe",
                     "params":{"pattern":"module.*"}});
    r.receive(c, &sub.to_string(), now);
    let changes = |out: &[Out]| {
        sends(out, c)
            .into_iter()
            .filter(|m| m["method"] == "facts.changed" && m["params"]["key"] == KEY)
            .count()
    };
    assert_eq!(changes(&r.set_starts_at_sign_in(true)), 1);
    assert_eq!(r.fact(KEY), Some(&json!(true)));
    assert_eq!(
        changes(&r.set_starts_at_sign_in(true)),
        0,
        "same value, no event"
    );
    assert_eq!(changes(&r.set_starts_at_sign_in(false)), 1);
    assert_eq!(r.fact(KEY), Some(&json!(false)));
}

#[test]
fn the_module_cannot_write_it() {
    let now = Instant::now();
    let mut r = router();
    let c = ready(&mut r, now);
    let set = json!({"jsonrpc":"2.0","id":9,"method":"facts.set",
                     "params":{"key":KEY,"value":true}});
    let out = r.receive(c, &set.to_string(), now);
    assert_eq!(sends(&out, c)[0]["error"]["code"], 5, "{out:?}");
    assert_eq!(r.fact(KEY), Some(&json!(false)));
}

#[test]
fn the_apps_line_gets_the_running_apps_by_name_and_follows_the_switch() {
    let now = Instant::now();
    let mut r = router();
    assert_eq!(r.apps_at_sign_in(), (vec![], vec![]), "nothing runs");
    ready(&mut r, now);
    // Running, switch off: named as running, not as told.
    assert_eq!(r.apps_at_sign_in(), (vec![M.to_string()], vec![]));
    r.set_starts_at_sign_in(true);
    assert_eq!(
        r.apps_at_sign_in(),
        (vec![M.to_string()], vec![M.to_string()])
    );
    r.set_starts_at_sign_in(false);
    assert_eq!(r.apps_at_sign_in().1, Vec::<String>::new());
}
