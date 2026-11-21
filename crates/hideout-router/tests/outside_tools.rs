//! Programs outside Hideout on the router pipe (S1, contract section 6.4): the hub grants the
//! free set, and the rest is refused with words that say what to do next. Mock-only: no pipe,
//! no window. The approver here is the free-set rule (`tool_class`) the hub installs; a router
//! test cannot reach the hub's own closure.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::{tool_class, Holder, ToolClass};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const M: &str = "foxglove";
const KEYS: &str = "keyboard.keys";

fn entry(id: &str, risk: &[(&str, &str)]) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder: Holder::FirstParty,
        granted: vec![],
        catalog_provide: Some(vec![]),
        channel_risk: risk
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    }
}

/// The hub's rule for a tool: the free set only.
fn free_only() -> Router {
    let mut r = Router::new(
        Settings::default(),
        Box::new(|_, a| {
            a.iter()
                .filter(|c| tool_class(c, None) == ToolClass::Free)
                .cloned()
                .collect()
        }),
    );
    r.add_module(entry(M, &[]));
    r.add_module(entry(KEYS, &[("events", "keys"), ("status", "normal")]));
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

fn call(r: &mut Router, c: ConnId, id: i64, method: &str, params: Value) -> Vec<Out> {
    let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    r.receive(c, &msg.to_string(), Instant::now())
}

/// The foxglove-cli's list, as its source asks it (a v1 client: `api` is a string).
const CLI_LIST: [&str; 6] = [
    "devices.read",
    "facts.read:*",
    "profiles.write",
    "lighting.write",
    "art.write",
    "app.windows",
];

fn connect(r: &mut Router, name: &str, v1: bool, perms: &[&str]) -> (ConnId, Value) {
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, 4242);
    let api = if v1 { json!("1.1") } else { json!({}) };
    let params = if v1 {
        json!({"client":{"name":name,"version":"1"},"api":api,"permissions":perms})
    } else {
        json!({"client":{"name":name,"version":"1"},"hub":"1.4","api":api,"permissions":perms})
    };
    let out = call(r, c, 0, "initialize", params);
    let reply = sends(&out, c).remove(0);
    assert!(reply.get("result").is_some(), "{reply}");
    (c, reply["result"].clone())
}

fn reason(out: &[Out], c: ConnId) -> (i64, String) {
    let e = sends(out, c).remove(0)["error"].clone();
    (
        e["code"].as_i64().expect("an error"),
        e["data"]["reason"].as_str().unwrap_or_default().to_string(),
    )
}

#[test]
fn the_cli_list_is_granted_devices_read_only() {
    let mut r = free_only();
    let (_, result) = connect(&mut r, "foxglove-cli", true, &CLI_LIST);
    assert_eq!(result["granted"], json!(["devices.read"]));
}

#[test]
fn lighting_set_is_denied_with_the_ask_words() {
    let mut r = free_only();
    let (c, _) = connect(&mut r, "foxglove-cli", true, &CLI_LIST);
    let out = call(
        &mut r,
        c,
        1,
        "lighting.set",
        json!({"device": "d", "on": true}),
    );
    let (code, why) = reason(&out, c);
    assert_eq!(code, 5);
    assert!(
        why.starts_with("Hideout didn't let \u{201c}foxglove-cli\u{201d} change your keyboards"),
        "{why}"
    );
    assert!(
        why.ends_with(
            "Programs outside Hideout can't be allowed this yet. To do it now, use the app's own page in Hideout."
        ),
        "{why}"
    );
    assert!(
        !why.contains("module") && !why.contains("permissions for"),
        "{why}"
    );
}

#[test]
fn a_long_name_is_cut_and_cleaned_in_the_words() {
    let mut r = free_only();
    let name = format!("a\u{7}{}", "x".repeat(80));
    let (c, _) = connect(&mut r, &name, false, &[]);
    let out = call(
        &mut r,
        c,
        1,
        "lighting.set",
        json!({"device": "d", "on": true}),
    );
    let (_, why) = reason(&out, c);
    let inner = why
        .split_once('\u{201c}')
        .and_then(|(_, r)| r.split_once('\u{201d}'))
        .map(|(n, _)| n.to_string())
        .expect("a quoted name");
    assert_eq!(inner.chars().count(), 40, "{inner}");
    assert!(
        inner.ends_with('\u{2026}') && !inner.contains('\u{7}'),
        "{inner}"
    );
}

#[test]
fn a_never_call_gets_the_never_words() {
    let mut r = free_only();
    let (c, _) = connect(&mut r, "some-tool", false, &["key.events"]);
    // Nothing routes key events as a method; a never-class call still says only an app may.
    let out = call(&mut r, c, 1, "modules.list", json!({}));
    let (code, why) = reason(&out, c);
    assert_eq!(code, 5);
    assert_eq!(
        why,
        "Hideout never lets a program outside Hideout manage your apps. Only an app you allowed in Hideout can."
    );
}

#[test]
fn the_hosted_check_still_passes() {
    let mut r = free_only();
    let (c, result) = connect(
        &mut r,
        "foxglove-app",
        false,
        &["facts.read:module.foxglove.state"],
    );
    assert_eq!(
        result["granted"],
        json!(["facts.read:module.foxglove.state"])
    );
    let out = call(
        &mut r,
        c,
        1,
        "facts.get",
        json!({"key": "module.foxglove.state"}),
    );
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    let out = call(&mut r, c, 2, "hub.show", json!({"id": M}));
    assert_eq!(sends(&out, c)[0]["result"], json!({}), "{out:?}");
}

#[test]
fn the_mcp_prefix_never_widens() {
    let mut r = free_only();
    let (_, plain) = connect(&mut r, "foxglove-cli", true, &CLI_LIST);
    let (_, mcp) = connect(&mut r, "mcp:foxglove-cli", true, &CLI_LIST);
    assert_eq!(plain["granted"], mcp["granted"]);
    // Asking for more than the free set with the prefix, even through an approver that says
    // yes to everything, leaves the Mcp holder's own never-list in force.
    let mut all = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    all.add_module(entry(KEYS, &[("events", "keys")]));
    let (_, mcp) = connect(
        &mut all,
        "mcp:agent",
        false,
        &["wallpaper.control", "key.events", "devices.read"],
    );
    assert_eq!(mcp["granted"], json!(["devices.read"]));
}

#[test]
fn a_keys_or_unknown_channel_is_never_a_tools() {
    // Even a router whose approver says yes to everything: the hub never offers it.
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(entry(KEYS, &[("events", "keys"), ("status", "normal")]));
    let (c, result) = connect(
        &mut r,
        "some-tool",
        false,
        &[
            "channel.open:keyboard.keys/events",
            "channel.open:keyboard.keys/other",
            "channel.open:keyboard.keys/status",
        ],
    );
    assert_eq!(
        result["granted"],
        json!(["channel.open:keyboard.keys/status"])
    );
    for (i, ch) in ["events", "other"].iter().enumerate() {
        let out = call(
            &mut r,
            c,
            1 + i as i64,
            "channel.open",
            json!({"module": KEYS, "channel": ch}),
        );
        let (code, why) = reason(&out, c);
        assert_eq!(code, 5, "{ch}");
        assert!(
            why.starts_with("Hideout never lets a program outside Hideout"),
            "{why}"
        );
    }
}
