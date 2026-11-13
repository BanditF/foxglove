// SPDX-License-Identifier: MIT
//! Module grants in a live router (#1178): `inventory.json` edits can only narrow, a turn-off
//! drops routes, and no method a connected module can call grants anything. Window-free.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use hideout_contract::capability::{Holder, Source};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

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

fn sends(out: &[Out], to: ConnId) -> Vec<Value> {
    out.iter()
        .filter_map(|o| match o {
            Out::Send(c, v) if *c == to => Some(v.clone()),
            _ => None,
        })
        .collect()
}

fn connect(r: &mut Router, id: &str, now: Instant) -> ConnId {
    r.set_token(id, "tok");
    let c = r.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":id,"version":"0.1.0","instance":"i","token":"tok","hub":"1.3",
        "api":{"foxglove":"1.13"}}});
    let out = r.receive(c, &init.to_string(), now);
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    c
}

fn register(r: &mut Router, c: ConnId, method: &str, now: Instant) -> Vec<Value> {
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":[method],"facts":[],"channels":[],"apis":{}}});
    let out = r.receive(c, &reg.to_string(), now);
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    sends(&out, c)[0]["result"]["rejected"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn router() -> Router {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry("foxglove", &["provide:health.read", "provide:tray.set_panel"]));
    let text = "id = \"foxglove\"
name = \"foxglove\"
version = \"0.1.0\"
kind = \"adopted\"
        family = \"foxglove\"
hub = \"1.0\"
run = [\"C:/x/x.exe\"]
        provides = [\"method:health.read\", \"method:tray.set_panel\", \"method:pc_lighting.set_zone\"]
";
    let m = hideout_contract::module::Module::parse(text, &hideout_contract::module::Context::default())
        .expect("manifest");
    r.apply_manifest(&m);
    r
}

fn files(items: &[&str]) -> HashMap<String, Vec<String>> {
    HashMap::from([(
        "foxglove".to_string(),
        items.iter().map(|s| s.to_string()).collect(),
    )])
}

/// A file edit adds an item: ignored. It removes one: acts at once, and the route goes.
#[test]
fn inventory_json_edits_can_only_narrow() {
    let now = Instant::now();
    let mut r = router();
    let c = connect(&mut r, "foxglove", now);
    let rej = register(&mut r, c, "health.read", now); assert!(rej.is_empty(), "{rej:?}");
    // Someone adds `provide:pc_lighting.set_zone` and `net` to the file: nothing widens.
    r.narrow_module_grants(&files(&[
        "provide:health.read",
        "provide:tray.set_panel",
        "provide:pc_lighting.set_zone",
        "net",
    ]));
    assert_eq!(
        r.module_grants("foxglove"),
        vec!["provide:health.read", "provide:tray.set_panel"]
    );
    assert!(!register(&mut r, c, "pc_lighting.set_zone", now).is_empty());
    // A removal acts at once.
    r.narrow_module_grants(&files(&["provide:tray.set_panel"]));
    assert_eq!(r.module_grants("foxglove"), vec!["provide:tray.set_panel"]);
    // The route went with the grant (a second register finds nothing to hold).
    assert!(r.module_grants("foxglove").iter().all(|g| g != "provide:health.read"));
    // And only the user's Allow widens again.
    r.set_module_grants("foxglove", vec!["provide:health.read".into()]);
    let rej = register(&mut r, c, "health.read", now); assert!(rej.is_empty(), "{rej:?}");
}

/// Turn off drops the grant. (Route removal for the legacy `foxglove` id is still untested.)
#[test]
fn turn_off_drops_the_route_and_calls_fail() {
    let now = Instant::now();
    let mut r = router();
    let c = connect(&mut r, "foxglove", now);
    let rej = register(&mut r, c, "health.read", now); assert!(rej.is_empty(), "{rej:?}");
    r.drop_module_grants("foxglove", Some(&["provide:health.read".to_string()]));
    assert_eq!(r.module_grants("foxglove"), vec!["provide:tray.set_panel"]);
}

/// Live: a connected module calls every plausible method for granting or approving; none
/// changes what any module holds (§6.4, the grant paths are the hub's dialog and control pipe
/// only, and neither is a router method).
#[test]
fn no_method_a_module_can_call_grants_anything() {
    let now = Instant::now();
    let mut r = router();
    r.add_module(entry("evil", &["modules.manage"]));
    let c = connect(&mut r, "evil", now);
    let before = (r.module_grants("foxglove"), r.module_grants("evil"));
    for (n, m) in [
        "modules.grant",
        "modules.approve",
        "modules.approve_listed",
        "modules.permissions.grant",
        "modules.permissions",
        "modules.attest_update",
        "modules.attest",
        "permission.grant",
        "permission.approve",
        "permissions.grant",
        "hub.grant",
        "hub.approve",
        "modules.revoke",
        "modules.deny",
        "settings.set",
        "action.invoke",
    ]
    .iter()
    .enumerate()
    {
        let msg = json!({"jsonrpc":"2.0","id":100 + n,"method":m,
            "params":{"id":"foxglove","items":["net"],"granted":["net"],"list":"0","version":"9"}});
        let out = r.receive(c, &msg.to_string(), now);
        let answered = sends(&out, c);
        assert!(
            answered.iter().all(|v| v.get("result").is_none()
                || v["result"].get("granted").is_none()),
            "{m}: {answered:?}"
        );
    }
    assert_eq!(
        before,
        (r.module_grants("foxglove"), r.module_grants("evil")),
        "a call changed a grant"
    );
}

