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
    register_all(r, c, &[method], now)
}

fn register_all(r: &mut Router, c: ConnId, methods: &[&str], now: Instant) -> Vec<Value> {
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":methods,"facts":[],"channels":[],"apis":{}}});
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
    r.add_module(entry(
        "foxglove",
        &["provide:health.read", "provide:tray.set_panel"],
    ));
    let text = "id = \"foxglove\"
name = \"foxglove\"
version = \"0.1.0\"
kind = \"adopted\"
        family = \"foxglove\"
hub = \"1.0\"
run = [\"C:/x/x.exe\"]
        provides = [\"method:health.read\", \"method:tray.set_panel\", \"method:pc_lighting.set_zone\"]
";
    let m = hideout_contract::module::Module::parse(
        text,
        &hideout_contract::module::Context::default(),
    )
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
    let rej = register(&mut r, c, "health.read", now);
    assert!(rej.is_empty(), "{rej:?}");
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
    assert!(r
        .module_grants("foxglove")
        .iter()
        .all(|g| g != "provide:health.read"));
    // And only the user's Allow widens again.
    r.set_module_grants("foxglove", vec!["provide:health.read".into()]);
    let rej = register(&mut r, c, "health.read", now);
    assert!(rej.is_empty(), "{rej:?}");
}

/// Calls `method` as `caller`; returns what `provider` was asked (and answers it) and the
/// caller's error code, if any.
fn call_through(
    r: &mut Router,
    caller: ConnId,
    provider: ConnId,
    method: &str,
    n: u64,
    now: Instant,
) -> (usize, Option<i64>) {
    let m = json!({"jsonrpc":"2.0","id":n,"method":method,"params":{}});
    let out = r.receive(caller, &m.to_string(), now);
    let asked = sends(&out, provider);
    for a in &asked {
        let ans = json!({"jsonrpc":"2.0","id":a["id"],"result":{}});
        r.receive(provider, &ans.to_string(), now);
    }
    let code = sends(&out, caller)
        .iter()
        .find_map(|v| v["error"]["code"].as_i64());
    (asked.len(), code)
}

/// Turn off drops the grant and the route with it, for the legacy adopted `foxglove` id too:
/// a call that reached the module before now has no provider, and the other route stays.
#[test]
fn turn_off_drops_the_route_and_calls_fail() {
    let now = Instant::now();
    let mut r = router();
    r.add_module(entry("example.caller", &["devices.read", "app.windows"]));
    let c = connect(&mut r, "foxglove", now);
    let rej = register_all(&mut r, c, &["health.read", "tray.set_panel"], now);
    assert!(rej.is_empty(), "{rej:?}");
    let caller = connect(&mut r, "example.caller", now);
    assert_eq!(
        call_through(&mut r, caller, c, "health.read", 300, now),
        (1, None)
    );

    r.drop_module_grants("foxglove", Some(&["provide:health.read".to_string()]));
    assert_eq!(r.module_grants("foxglove"), vec!["provide:tray.set_panel"]);
    let (asked, code) = call_through(&mut r, caller, c, "health.read", 301, now);
    assert_eq!(asked, 0, "the call still reached the module");
    assert!(code.is_some(), "the caller was not told it failed");
    // The item that stayed is still served.
    assert_eq!(
        call_through(&mut r, caller, c, "tray.set_panel", 302, now),
        (1, None)
    );

    // Turn all off: nothing is served.
    r.drop_module_grants("foxglove", None);
    assert!(r.module_grants("foxglove").is_empty());
    assert_eq!(
        call_through(&mut r, caller, c, "tray.set_panel", 303, now).0,
        0
    );
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
        "modules.follow",
        "modules.keep_followed",
        "modules.permissions.follow",
        "permission.follow",
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
            "params":{"id":"foxglove","items":["net"],"granted":["net"],"list":"0","version":"9",
                "follow":true}});
        let out = r.receive(c, &msg.to_string(), now);
        let answered = sends(&out, c);
        assert!(
            answered
                .iter()
                .all(|v| v.get("result").is_none() || v["result"].get("granted").is_none()),
            "{m}: {answered:?}"
        );
    }
    assert_eq!(
        before,
        (r.module_grants("foxglove"), r.module_grants("evil")),
        "a call changed a grant"
    );
}

/// #1238: a method the module declares but the user has not allowed answers DENIED with a
/// reason (not "method not found"); after the user's Allow it is served with no restart, even
/// though the module registered while the grant was missing; a never name stays not found.
#[test]
fn a_declared_but_ungranted_method_is_denied_then_served_after_allow_without_a_restart() {
    let now = Instant::now();
    let mut r = router();
    let text = "id = \"foxglove\"
name = \"foxglove\"
version = \"0.2.0\"
kind = \"adopted\"
family = \"foxglove\"
hub = \"1.0\"
run = [\"C:/x/x.exe\"]
provides = [\"method:health.read\", \"method:wallpaper-engine.pause\", \"method:pc_lighting.set_zone\"]
";
    let m = hideout_contract::module::Module::parse(
        text,
        &hideout_contract::module::Context::default(),
    )
    .expect("manifest");
    r.apply_manifest(&m);
    r.add_module(entry("example.caller", &["wallpaper.control"]));
    let c = connect(&mut r, "foxglove", now);
    // The module asks for all it declares at start; the missing grant rejects the new one.
    let rej = register_all(&mut r, c, &["health.read", "wallpaper-engine.pause"], now);
    assert_eq!(rej.len(), 1, "{rej:?}");
    let caller = connect(&mut r, "example.caller", now);
    let m = json!({"jsonrpc":"2.0","id":400,"method":"wallpaper-engine.pause","params":{}});
    let out = r.receive(caller, &m.to_string(), now);
    let e = &sends(&out, caller)[0]["error"];
    assert_eq!(e["code"], 5, "{e}");
    assert!(e["data"]["reason"]
        .as_str()
        .unwrap()
        .contains("Not allowed in Hideout yet"));
    // A name nobody declares stays not found.
    let m = json!({"jsonrpc":"2.0","id":401,"method":"nobody.declares","params":{}});
    let out = r.receive(caller, &m.to_string(), now);
    assert_eq!(sends(&out, caller)[0]["error"]["code"], -32601);

    // The user's Allow adds the item; the call is served at once, no restart, no re-register.
    r.set_module_grants(
        "foxglove",
        vec![
            "provide:health.read".into(),
            "provide:tray.set_panel".into(),
            "provide:wallpaper-engine.pause".into(),
        ],
    );
    let (asked, code) = call_through(&mut r, caller, c, "wallpaper-engine.pause", 402, now);
    assert_eq!((asked, code), (1, None));
    // What was held before is still held.
    assert!(r
        .module_grants("foxglove")
        .contains(&"provide:health.read".to_string()));
}

/// A router with the module `foxglove` (grants as given) declaring the wallpaper pause and a
/// caller that may call it.
fn pause_router(granted: &[&str]) -> (Router, ConnId, ConnId, Instant) {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry("foxglove", granted));
    let text = "id = \"foxglove\"
name = \"foxglove\"
version = \"0.2.0\"
kind = \"adopted\"
family = \"foxglove\"
hub = \"1.0\"
run = [\"C:/x/x.exe\"]
provides = [\"method:health.read\", \"method:wallpaper-engine.pause\"]
";
    let m = hideout_contract::module::Module::parse(
        text,
        &hideout_contract::module::Context::default(),
    )
    .expect("manifest");
    r.apply_manifest(&m);
    r.add_module(entry("example.caller", &["wallpaper.control"]));
    let c = connect(&mut r, "foxglove", now);
    let caller = connect(&mut r, "example.caller", now);
    (r, c, caller, now)
}

#[test]
fn a_wildcard_grant_is_not_told_not_allowed_yet() {
    // The user already allowed `provide:wallpaper-engine.*`; the module just does not serve the
    // pause. The grant is not what keeps it out, so the answer is the plain not found.
    let (mut r, c, caller, now) =
        pause_router(&["provide:health.read", "provide:wallpaper-engine.*"]);
    register_all(&mut r, c, &["health.read"], now);
    let m = json!({"jsonrpc":"2.0","id":500,"method":"wallpaper-engine.pause","params":{}});
    let out = r.receive(caller, &m.to_string(), now);
    assert_eq!(sends(&out, caller)[0]["error"]["code"], -32601);
}

#[test]
fn a_register_that_wants_nothing_clears_what_the_last_one_wanted() {
    let (mut r, c, caller, now) = pause_router(&["provide:health.read"]);
    // First registration asks for the pause too; the missing grant leaves it wanted.
    assert_eq!(
        register_all(&mut r, c, &["health.read", "wallpaper-engine.pause"], now).len(),
        1
    );
    // The module registers again without it: the new registration replaces the old one.
    assert!(register_all(&mut r, c, &["health.read"], now).is_empty());
    r.set_module_grants(
        "foxglove",
        vec![
            "provide:health.read".into(),
            "provide:wallpaper-engine.pause".into(),
        ],
    );
    let (asked, code) = call_through(&mut r, caller, c, "wallpaper-engine.pause", 501, now);
    assert_eq!(asked, 0, "no stale route comes back");
    assert!(code.is_some(), "the call is refused, not served");
}

/// The catalog (accepted after the first registrations) now assigns `zonex.*` to `cat.x`.
fn assign_to_cat_x(r: &mut Router, granted: &[&str]) {
    let mut x = entry("cat.x", granted);
    x.registered_version = None;
    x.catalog_provide = Some(vec!["zonex.*".into()]);
    r.add_module(x);
    r.set_root_areas(vec![("cat.x".into(), "zonex.*".into())]);
}

fn adopted_manifest(id: &str, method: &str) -> hideout_contract::module::Module {
    let text = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"adopted\"\nfamily = \"foxglove\"\nhub = \"1.0\"\nrun = [\"C:/x/x.exe\"]\nprovides = [\"method:{method}\"]\n"
    );
    hideout_contract::module::Module::parse(&text, &hideout_contract::module::Context::default())
        .expect("manifest")
}

/// A catalog module refused a method as `taken` gets it when a registry reload drops the adopted
/// holder's route: no restart, no second `module.register` (#1135). Its grants are rechecked.
#[test]
fn a_refused_taken_registration_applies_when_the_adopted_route_is_dropped() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry("adopted.y", &["provide:zonex.set"]));
    r.apply_manifest(&adopted_manifest("adopted.y", "zonex.set"));
    let mut x = entry("cat.x", &["provide:zonex.set"]);
    x.registered_version = None;
    r.add_module(x);
    r.add_module(entry("example.caller", &[]));
    let y = connect(&mut r, "adopted.y", now);
    assert!(register(&mut r, y, "zonex.set", now).is_empty());
    let cx = connect(&mut r, "cat.x", now);
    let rej = register(&mut r, cx, "zonex.set", now);
    assert_eq!(rej.len(), 1, "refused as taken: {rej:?}");
    let caller = connect(&mut r, "example.caller", now);
    // Before the reload Y serves it.
    assert_eq!(call_through(&mut r, caller, y, "zonex.set", 1, now).0, 1);

    assign_to_cat_x(&mut r, &["provide:zonex.set"]);
    let (asked, code) = call_through(&mut r, caller, cx, "zonex.set", 2, now);
    assert_eq!(
        (asked, code),
        (1, None),
        "the retained registration applied"
    );
    assert_eq!(call_through(&mut r, caller, y, "zonex.set", 3, now).0, 0);
}

/// Grants are rechecked at apply time: a grant taken away while the registration was retained
/// leaves the route unbound, and a disconnect drops the retained registration.
#[test]
fn a_retained_registration_needs_its_grant_and_its_connection() {
    let now = Instant::now();
    let build = || {
        let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
        r.add_module(entry("adopted.y", &["provide:zonex.set"]));
        r.apply_manifest(&adopted_manifest("adopted.y", "zonex.set"));
        let mut x = entry("cat.x", &["provide:zonex.set"]);
        x.registered_version = None;
        r.add_module(x);
        r.add_module(entry("example.caller", &[]));
        let y = connect(&mut r, "adopted.y", now);
        register(&mut r, y, "zonex.set", now);
        let cx = connect(&mut r, "cat.x", now);
        assert_eq!(register(&mut r, cx, "zonex.set", now).len(), 1);
        let caller = connect(&mut r, "example.caller", now);
        (r, cx, caller)
    };
    // Grant removed meanwhile: nothing is bound.
    let (mut r, cx, caller) = build();
    assign_to_cat_x(&mut r, &[]);
    assert_eq!(call_through(&mut r, caller, cx, "zonex.set", 1, now).0, 0);
    // Disconnected: the retained registration is gone, a reconnect must register itself.
    let (mut r, cx, caller) = build();
    r.disconnect(cx);
    assign_to_cat_x(&mut r, &["provide:zonex.set"]);
    let cx2 = connect(&mut r, "cat.x", now);
    assert_eq!(call_through(&mut r, caller, cx2, "zonex.set", 2, now).0, 0);
}

/// A route the adopted module still owns is never taken: nothing is dropped, nothing moves.
#[test]
fn a_retained_registration_never_takes_a_route_still_owned() {
    let now = Instant::now();
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry("adopted.y", &["provide:zonex.set"]));
    r.apply_manifest(&adopted_manifest("adopted.y", "zonex.set"));
    let mut x = entry("cat.x", &["provide:zonex.set"]);
    x.registered_version = None;
    r.add_module(x);
    r.add_module(entry("example.caller", &[]));
    let y = connect(&mut r, "adopted.y", now);
    register(&mut r, y, "zonex.set", now);
    let cx = connect(&mut r, "cat.x", now);
    assert_eq!(register(&mut r, cx, "zonex.set", now).len(), 1);
    let caller = connect(&mut r, "example.caller", now);
    r.set_root_areas(vec![]);
    assert_eq!(call_through(&mut r, caller, cx, "zonex.set", 1, now).0, 0);
    assert_eq!(call_through(&mut r, caller, y, "zonex.set", 2, now).0, 1);
}
