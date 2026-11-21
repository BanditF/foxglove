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
    r.set_peer_identity(c, Some(tool(r"C:\Tools\t.exe", "aa")));
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
        why.starts_with("Hideout hasn't let \u{201c}foxglove-cli\u{201d} change your keyboards"),
        "{why}"
    );
    assert!(
        why.ends_with(
            " yet. Open Hideout, go to Apps and press Review\u{2026} on its line. If there is no line, close this program and start it again, then try again."
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

// ---- S1 slice 2: what is remembered is the program, not the name ----

use hideout_router::ToolId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn tool(path: &str, hash: &str) -> ToolId {
    ToolId {
        image_path: path.into(),
        image_sha256: hash.into(),
    }
}

/// A router whose approver grants the free set and counts how often it was asked.
fn counting() -> (Router, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let n = asked.clone();
    let mut r = Router::new(
        Settings::default(),
        Box::new(move |_, a| {
            n.fetch_add(1, Ordering::SeqCst);
            a.iter()
                .filter(|c| tool_class(c, None) == ToolClass::Free)
                .cloned()
                .collect()
        }),
    );
    r.add_module(entry(M, &[]));
    (r, asked)
}

/// A connection whose program the transport named as `id`, then `initialize`.
fn connect_as(r: &mut Router, name: &str, id: Option<ToolId>) -> Value {
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, 4242);
    r.set_peer_identity(c, id);
    let params = json!({"client":{"name":name,"version":"1"},"hub":"1.4","api":{},
        "permissions":["devices.read"]});
    let out = call(r, c, 0, "initialize", params);
    sends(&out, c).remove(0)["result"].clone()
}

#[test]
fn same_program_is_remembered() {
    let (mut r, asked) = counting();
    let id = tool(r"C:\Tools\a.exe", "aa");
    connect_as(&mut r, "x", Some(id.clone()));
    let res = connect_as(&mut r, "x", Some(id));
    // A free-only answer is not kept (the hub grants it afresh each time); the grants are the same.
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert_eq!(res["granted"], json!(["devices.read"]));
}

#[test]
fn same_name_other_image_inherits_nothing() {
    let (mut r, asked) = counting();
    connect_as(&mut r, "Friendly", Some(tool(r"C:\Tools\a.exe", "aa")));
    // Same name, another file; and the same path with other bytes.
    connect_as(&mut r, "Friendly", Some(tool(r"C:\Other\b.exe", "bb")));
    connect_as(&mut r, "Friendly", Some(tool(r"C:\Tools\a.exe", "cc")));
    assert_eq!(asked.load(Ordering::SeqCst), 3);
}

#[test]
fn unknown_identity_never_remembers() {
    let (mut r, asked) = counting();
    connect_as(&mut r, "x", None);
    connect_as(&mut r, "x", None);
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    // Nor does an unknown connection inherit what a known program was given.
    connect_as(&mut r, "x", Some(tool(r"C:\Tools\a.exe", "aa")));
    connect_as(&mut r, "x", None);
    assert_eq!(asked.load(Ordering::SeqCst), 4);
}

#[test]
fn script_runner_not_remembered() {
    let (mut r, asked) = counting();
    let py = tool(r"C:\Python312\python.exe", "aa");
    connect_as(&mut r, "x", Some(py.clone()));
    connect_as(&mut r, "x", Some(py));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

// ---- S1 slice 3: the queue, the words, Allow ----

/// A router whose approver grants the free set, with one program connected as `id` asking
/// `perms` (v2 wire).
fn connect_asking(
    r: &mut Router,
    name: &str,
    id: Option<ToolId>,
    perms: &[&str],
) -> (ConnId, Vec<Out>, Value) {
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, 4242);
    r.set_peer_identity(c, id);
    let params = json!({"client":{"name":name,"version":"1"},"hub":"1.4","api":{},
        "permissions":perms});
    let out = call(r, c, 0, "initialize", params);
    let result = sends(&out, c).remove(0)["result"].clone();
    (c, out, result)
}

#[test]
fn tool_refusal_r3_names_the_program_and_where_to_review() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, "My Tool", Some(id), &["lighting.write"]);
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
        why.starts_with("Hideout hasn't let \u{201c}My Tool\u{201d} change your keyboards"),
        "{why}"
    );
    assert!(
        why.ends_with(
            " yet. Open Hideout, go to Apps and press Review\u{2026} on its line. If there is no line, close this program and start it again, then try again."
        ),
        "{why}"
    );
}

#[test]
fn tool_refusal_r4_when_the_program_cannot_be_told() {
    let mut r = free_only();
    let r4 = "Hideout couldn't check which program this is, so it may only read. Start it again from a normal window, not as administrator.";
    // No identity at all.
    let (c, _, _) = connect_asking(&mut r, "x", None, &["lighting.write"]);
    let out = call(
        &mut r,
        c,
        1,
        "lighting.set",
        json!({"device": "d", "on": true}),
    );
    assert_eq!(reason(&out, c).1, r4);
    // A contained program has no identity the hub relies on, even when the file was read.
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, 4243);
    r.set_peer_identity(c, Some(tool(r"C:\Tools.exe", "aa")));
    r.set_peer_container(c, "S-1-15-2-1");
    let params = json!({"client":{"name":"x","version":"1"},"hub":"1.4","api":{},
        "permissions":["lighting.write"]});
    call(&mut r, c, 0, "initialize", params);
    let out = call(
        &mut r,
        c,
        1,
        "lighting.set",
        json!({"device": "d", "on": true}),
    );
    assert_eq!(reason(&out, c).1, r4);
    // Nothing was queued for either.
    assert!(r.tool_asks().is_empty());
}

#[test]
fn ask_class_items_are_queued_by_program_and_never_items_are_not() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let (_, out, _) = connect_asking(
        &mut r,
        "Friendly",
        Some(id.clone()),
        &[
            "devices.read",
            "lighting.write",
            "behaviors.invoke",
            "key.events",
            "net",
            "files",
            "channel.open:keyboard.keys/events",
        ],
    );
    assert!(out.contains(&Out::ToolAsk));
    let asks = r.tool_asks();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].id, id);
    assert_eq!(asks[0].name, "Friendly");
    let raws: Vec<&str> = asks[0].items.iter().map(|i| i.raw.as_str()).collect();
    assert_eq!(raws, ["lighting.write", "behaviors.invoke"]);
    let red: Vec<bool> = asks[0].items.iter().map(|i| i.red).collect();
    assert_eq!(red, [false, true]);
    assert!(!asks[0].runs_scripts);
}

#[test]
fn nothing_is_queued_for_free_only_asks_or_unknown_programs() {
    let mut r = free_only();
    connect_asking(
        &mut r,
        "a",
        Some(tool(r"C:\T.exe", "aa")),
        &["devices.read"],
    );
    connect_asking(&mut r, "b", None, &["lighting.write"]);
    assert!(r.tool_asks().is_empty());
}

#[test]
fn queue_holds_five_programs_newest_first_through_the_router() {
    let mut r = free_only();
    for n in 0..7 {
        connect_asking(
            &mut r,
            &format!("tool {n}"),
            Some(tool(&format!(r"C:\T\{n}.exe"), "h")),
            &["lighting.write"],
        );
    }
    let names: Vec<String> = r.tool_asks().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["tool 6", "tool 5", "tool 4", "tool 3", "tool 2"]);
}

#[test]
fn allow_sends_grants_added_and_is_remembered_for_the_file() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let (c, _, res) = connect_asking(
        &mut r,
        "Friendly",
        Some(id.clone()),
        &["devices.read", "lighting.write", "behaviors.invoke"],
    );
    assert_eq!(res["granted"], json!(["devices.read"]));
    let allow = [
        hideout_contract::capability::Capability::parse("lighting.write").unwrap(),
        // Not offered by the program: ignored.
        hideout_contract::capability::Capability::parse("art.write").unwrap(),
    ];
    let out = r.set_tool_grants(&id, &allow);
    let msgs = sends(&out, c);
    assert_eq!(msgs.len(), 1, "{out:?}");
    assert_eq!(msgs[0]["method"], "module.grants");
    assert_eq!(msgs[0]["params"], json!({"added": ["lighting.write"]}));
    assert!(out.contains(&Out::ToolRemembered {
        image_path: id.image_path.clone(),
        image_sha256: "aa".into(),
        granted: vec!["lighting.write".into()],
    }));
    assert!(r.tool_asks().is_empty());
    // The next connection of the same file is granted it without asking again; the other
    // item is asked about again.
    let (_, _, res) = connect_asking(
        &mut r,
        "Another name",
        Some(id),
        &["devices.read", "lighting.write", "behaviors.invoke"],
    );
    assert_eq!(res["granted"], json!(["devices.read", "lighting.write"]));
    assert_eq!(r.tool_asks().len(), 1);
    assert_eq!(r.tool_asks()[0].items.len(), 1);
}

#[test]
fn not_now_drops_the_ask_and_grants_nothing() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, "x", Some(id.clone()), &["lighting.write"]);
    r.dismiss_tool(&id);
    assert!(r.tool_asks().is_empty());
    assert!(r.set_tool_grants(&id, &[]).is_empty());
    let out = call(
        &mut r,
        c,
        1,
        "lighting.set",
        json!({"device": "d", "on": true}),
    );
    assert_eq!(reason(&out, c).0, 5);
}

#[test]
fn a_script_runner_is_allowed_for_now_and_asked_again() {
    let mut r = free_only();
    let py = tool(r"C:\Python312\python.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, "script", Some(py.clone()), &["lighting.write"]);
    assert!(r.tool_asks()[0].runs_scripts);
    let out = r.set_tool_grants(
        &py,
        &[hideout_contract::capability::Capability::parse("lighting.write").unwrap()],
    );
    assert!(!out.iter().any(|o| matches!(o, Out::ToolRemembered { .. })));
    assert_eq!(sends(&out, c).len(), 1);
    let (_, _, res) = connect_asking(&mut r, "script", Some(py), &["lighting.write"]);
    assert_eq!(res["granted"], json!([]));
    assert_eq!(r.tool_asks().len(), 1);
}

#[test]
fn a_contained_program_is_never_remembered() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, 4243);
    r.set_peer_identity(c, Some(id.clone()));
    r.set_peer_container(c, "S-1-15-2-1");
    let params = json!({"client":{"name":"x","version":"1"},"hub":"1.4","api":{},
        "permissions":["devices.read","lighting.write"]});
    call(&mut r, c, 0, "initialize", params);
    assert!(r.tool_asks().is_empty());
    // Even a stray Allow for that file reaches no contained connection and keeps nothing for
    // it from the contained connection.
    let out = r.set_tool_grants(
        &id,
        &[hideout_contract::capability::Capability::parse("lighting.write").unwrap()],
    );
    assert!(out.is_empty());
}

#[test]
fn what_was_kept_binds_to_path_and_hash_and_a_changed_hash_voids_it() {
    let mut r = free_only();
    let old = tool(r"C:\Tools.exe", "aa");
    r.load_tools(vec![(
        old.image_path.clone(),
        "aa".into(),
        vec!["lighting.write".into(), "key.events".into()],
    )]);
    // The same bytes: granted from the record, nothing queued; a never item in the record
    // stays out.
    let (_, out, res) = connect_asking(
        &mut r,
        "x",
        Some(old.clone()),
        &["devices.read", "lighting.write", "key.events"],
    );
    assert_eq!(res["granted"], json!(["devices.read", "lighting.write"]));
    assert!(!out.iter().any(|o| matches!(o, Out::ToolVoided(_))));
    assert!(r.tool_asks().is_empty());
    // The same path with other bytes: the record is void and the program is asked again.
    let (_, out, res) = connect_asking(
        &mut r,
        "x",
        Some(tool(r"C:\Tools.exe", "bb")),
        &["devices.read", "lighting.write"],
    );
    assert_eq!(res["granted"], json!(["devices.read"]));
    assert!(out.contains(&Out::ToolVoided(r"C:\Tools.exe".into())));
    assert_eq!(r.tool_asks().len(), 1);
    // The old bytes do not get their record back.
    let (_, _, res) = connect_asking(&mut r, "x", Some(old), &["devices.read", "lighting.write"]);
    assert_eq!(res["granted"], json!(["devices.read"]));
}

#[test]
fn the_same_name_from_another_program_inherits_nothing_and_queues_apart() {
    let mut r = free_only();
    let a = tool(r"C:\Tools.exe", "aa");
    connect_asking(&mut r, "Friendly", Some(a.clone()), &["lighting.write"]);
    r.set_tool_grants(
        &a,
        &[hideout_contract::capability::Capability::parse("lighting.write").unwrap()],
    );
    let (_, _, res) = connect_asking(
        &mut r,
        "Friendly",
        Some(tool(r"C:\Other.exe", "bb")),
        &["lighting.write"],
    );
    assert_eq!(res["granted"], json!([]));
    assert_eq!(r.tool_asks().len(), 1);
}

#[test]
fn odd_names_stay_plain_text_in_the_queue_and_the_words() {
    let mut r = free_only();
    let name = "<b>x</b> \u{202e}evil\u{7} [link](http://e)";
    let id = tool(r"C:\Tools.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, name, Some(id), &["lighting.write"]);
    // The queue keeps the text as given; the words are cleaned and quoted.
    assert_eq!(r.tool_asks()[0].name, name);
    let out = call(
        &mut r,
        c,
        1,
        "lighting.set",
        json!({"device": "d", "on": true}),
    );
    let (_, why) = reason(&out, c);
    assert!(!why.contains('\u{7}') && !why.contains('\u{202e}'), "{why}");
    assert!(
        why.contains("\u{201c}") && why.contains("\u{201d}"),
        "{why}"
    );
}

#[test]
fn a_blank_name_says_this_program_never_an_app() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, "\u{200b} \u{202e}", Some(id), &["lighting.write"]);
    let out = call(
        &mut r,
        c,
        1,
        "lighting.set",
        json!({"device": "d", "on": true}),
    );
    let (_, why) = reason(&out, c);
    assert!(why.starts_with("Hideout hasn't let this program "), "{why}");
    assert!(!why.contains("An app"), "{why}");
}

// ---- S1 slice 3 review fixes ----

fn cap(s: &str) -> hideout_contract::capability::Capability {
    hideout_contract::capability::Capability::parse(s).unwrap()
}

fn allow_lighting(r: &mut Router, id: &ToolId, name: &str) {
    connect_asking(r, name, Some(id.clone()), &["lighting.write"]);
    r.set_tool_grants(id, &[cap("lighting.write")]);
}

fn lighting_call(r: &mut Router, c: ConnId) -> Vec<Out> {
    call(r, c, 9, "lighting.set", json!({"device": "d", "on": true}))
}

#[test]
fn free_only_programs_never_push_out_an_allowed_one() {
    let mut r = free_only();
    let a = tool(r"C:\A.exe", "aa");
    allow_lighting(&mut r, &a, "a");
    for n in 0..100 {
        connect_asking(
            &mut r,
            "free",
            Some(tool(&format!(r"C:\F\{n}.exe"), "h")),
            &["devices.read"],
        );
    }
    let (_, _, res) = connect_asking(&mut r, "a", Some(a), &["devices.read", "lighting.write"]);
    assert_eq!(res["granted"], json!(["devices.read", "lighting.write"]));
    assert!(r.tool_asks().is_empty());
}

#[test]
fn allowed_programs_are_forgotten_oldest_first() {
    let mut r = free_only();
    let ids: Vec<ToolId> = (0..=hideout_router::APPROVED_MAX)
        .map(|n| tool(&format!(r"C:\P\{n}.exe"), "h"))
        .collect();
    for (n, id) in ids.iter().enumerate() {
        allow_lighting(&mut r, id, &format!("p{n}"));
    }
    let asked = ["devices.read", "lighting.write"];
    // The newest and the second oldest are still allowed; reconnecting does not reorder.
    for id in [&ids[1], ids.last().unwrap(), &ids[1]] {
        let (_, _, res) = connect_asking(&mut r, "x", Some(id.clone()), &asked);
        assert_eq!(res["granted"], json!(["devices.read", "lighting.write"]));
    }
    // The first one allowed was dropped: it is asked again.
    let (_, _, res) = connect_asking(&mut r, "x", Some(ids[0].clone()), &asked);
    assert_eq!(res["granted"], json!(["devices.read"]));
    assert_eq!(r.tool_asks().len(), 1);
}

#[test]
fn an_ask_pushed_out_of_the_queue_cannot_be_allowed() {
    let mut r = free_only();
    let ids: Vec<ToolId> = (0..7)
        .map(|n| tool(&format!(r"C:\T\{n}.exe"), "h"))
        .collect();
    let mut conns = Vec::new();
    for (n, id) in ids.iter().enumerate() {
        let (c, _, _) = connect_asking(
            &mut r,
            &format!("t{n}"),
            Some(id.clone()),
            &["lighting.write"],
        );
        conns.push(c);
    }
    // The two oldest are no longer waiting; an Allow for one of them does nothing.
    let out = r.set_tool_grants(&ids[0], &[cap("lighting.write")]);
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(reason(&lighting_call(&mut r, conns[0]), conns[0]).0, 5);
    // One still waiting can be allowed.
    assert!(!r
        .set_tool_grants(&ids[6], &[cap("lighting.write")])
        .is_empty());
}

#[test]
fn allow_grants_only_what_was_asked_and_drawn() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let (c1, _, _) = connect_asking(&mut r, "x", Some(id.clone()), &["lighting.write"]);
    // The program asks for more after the dialog was drawn for the first list.
    let (c2, _, _) = connect_asking(
        &mut r,
        "x",
        Some(id.clone()),
        &["lighting.write", "behaviors.invoke"],
    );
    let out = r.set_tool_grants(&id, &[cap("lighting.write")]);
    assert_eq!(
        sends(&out, c1)[0]["params"],
        json!({"added": ["lighting.write"]})
    );
    assert_eq!(
        sends(&out, c2)[0]["params"],
        json!({"added": ["lighting.write"]})
    );
    let out = call(
        &mut r,
        c2,
        2,
        "behaviors.invoke",
        json!({"device": "d", "behavior": "x"}),
    );
    assert_eq!(reason(&out, c2).0, 5);
}

#[test]
fn a_contained_connection_is_left_out_when_the_same_file_is_allowed() {
    let mut r = free_only();
    let id = tool(r"C:\Tools.exe", "aa");
    let (plain, _, _) = connect_asking(&mut r, "x", Some(id.clone()), &["lighting.write"]);
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, 4244);
    r.set_peer_identity(c, Some(id.clone()));
    r.set_peer_container(c, "S-1-15-2-1");
    let params = json!({"client":{"name":"x","version":"1"},"hub":"1.4","api":{},
        "permissions":["lighting.write"]});
    call(&mut r, c, 0, "initialize", params);
    let out = r.set_tool_grants(&id, &[cap("lighting.write")]);
    assert_eq!(sends(&out, plain).len(), 1);
    assert!(sends(&out, c).is_empty(), "{out:?}");
    assert_eq!(reason(&lighting_call(&mut r, c), c).0, 5);
}

#[test]
fn a_script_runners_allow_reaches_only_the_connection_that_asked() {
    let mut r = free_only();
    let py = tool(r"C:\Python312\python.exe", "aa");
    let (first, _, _) = connect_asking(&mut r, "one", Some(py.clone()), &["lighting.write"]);
    let (second, _, _) = connect_asking(&mut r, "two", Some(py.clone()), &["lighting.write"]);
    // The waiting ask is the newest script's; only that connection gets what is allowed.
    assert_eq!(r.tool_asks()[0].name, "two");
    let out = r.set_tool_grants(&py, &[cap("lighting.write")]);
    assert_eq!(sends(&out, second).len(), 1, "{out:?}");
    assert!(sends(&out, first).is_empty(), "{out:?}");
    assert_eq!(reason(&lighting_call(&mut r, first), first).0, 5);
}

// ---- Forget on the Apps page (#1470) ----

const R5_TAIL: &str = ". This program was forgotten on Hideout's Apps page. Close this program and start it again, then allow it in Hideout.";

fn subscribe(r: &mut Router, c: ConnId, pattern: &str) -> u64 {
    let out = call(r, c, 5, "facts.subscribe", json!({"pattern": pattern}));
    sends(&out, c)[0]["result"]["subscription"]
        .as_u64()
        .unwrap_or_else(|| panic!("{out:?}"))
}

/// Whether the permission check lets `c` change the lighting (no module answers the call in
/// these tests, so any other answer than "no permission" is a yes).
fn may_light(r: &mut Router, c: ConnId) -> bool {
    let out = lighting_call(r, c);
    sends(&out, c)
        .iter()
        .all(|m| m["error"]["code"].as_i64() != Some(5))
}

#[test]
fn forget_narrows_live_connection_keeps_free_set() {
    let mut r = free_only();
    let id = tool(r"C:\Tools\lights.exe", "aa");
    let (c, _, _) = connect_asking(
        &mut r,
        "Lights",
        Some(id.clone()),
        &["devices.read", "lighting.write", "behaviors.invoke"],
    );
    r.set_tool_grants(&id, &[cap("lighting.write"), cap("behaviors.invoke")]);
    assert!(may_light(&mut r, c));
    // The path compares without regard to case, like the record.
    r.forget_tool(r"c:\tools\LIGHTS.exe");
    assert_eq!(reason(&lighting_call(&mut r, c), c).0, 5);
    // What any program may have without asking stays.
    let out = call(&mut r, c, 2, "devices.list", json!({}));
    assert!(
        sends(&out, c)
            .iter()
            .all(|m| m["error"]["code"].as_i64() != Some(5)),
        "{out:?}"
    );
    // The record is gone: the next connection is asked about again and has only the free set.
    let (_, _, res) = connect_asking(
        &mut r,
        "Lights",
        Some(id),
        &["devices.read", "lighting.write"],
    );
    assert_eq!(res["granted"], json!(["devices.read"]));
    assert_eq!(r.tool_asks().len(), 1);
}

#[test]
fn forget_ends_subscriptions_and_tells_v2() {
    let mut r = free_only();
    let id = tool(r"C:\Tools\w.exe", "aa");
    let perms = ["facts.read:weather.*", "facts.read:volume.*"];
    let (c, _, _) = connect_asking(&mut r, "w", Some(id.clone()), &perms);
    // A v1 connection of the same file.
    let old = r.connect(Origin::Pipe);
    r.set_peer_pid(old, 4243);
    r.set_peer_identity(old, Some(id.clone()));
    let params = json!({"client":{"name":"w","version":"1"},"api":"1.1","permissions":perms});
    call(&mut r, old, 0, "initialize", params);
    r.set_tool_grants(&id, &[cap("facts.read:weather.*")]);
    let asked = subscribe(&mut r, c, "weather.*");
    let free = subscribe(&mut r, c, "volume.*");
    subscribe(&mut r, old, "weather.*");
    let out = r.forget_tool(&id.image_path);
    let told = sends(&out, c);
    assert_eq!(told.len(), 1, "{out:?}");
    assert_eq!(told[0]["method"], "module.grants");
    assert_eq!(told[0]["params"], json!({"added": [], "ended": [asked]}));
    assert_ne!(asked, free);
    // A v1 connection loses the same and is not sent a message it does not know.
    assert!(sends(&out, old).is_empty(), "{out:?}");
    let out = call(
        &mut r,
        old,
        6,
        "facts.subscribe",
        json!({"pattern": "weather.*"}),
    );
    assert!(sends(&out, old)[0].get("error").is_some(), "{out:?}");
    // Pressing Forget again changes nothing and says nothing.
    assert!(r.forget_tool(&id.image_path).is_empty());
}

#[test]
fn forget_then_call_gets_r5() {
    let mut r = free_only();
    let id = tool(r"C:\Tools\lights.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, "Lights", Some(id.clone()), &["lighting.write"]);
    r.set_tool_grants(&id, &[cap("lighting.write")]);
    let (blank, _, _) = connect_asking(&mut r, " ", Some(id.clone()), &["lighting.write"]);
    r.forget_tool(&id.image_path);
    let (code, why) = reason(&lighting_call(&mut r, c), c);
    assert_eq!(code, 5);
    assert!(
        why.starts_with("Hideout no longer lets \u{201c}Lights\u{201d} change your keyboards"),
        "{why}"
    );
    assert!(why.ends_with(R5_TAIL), "{why}");
    let (_, why) = reason(&lighting_call(&mut r, blank), blank);
    assert!(
        why.starts_with("Hideout no longer lets this program change your keyboards"),
        "{why}"
    );
    // A never-class call keeps its own words.
    let out = call(&mut r, c, 3, "keys.subscribe", json!({}));
    let (_, never) = reason(&out, c);
    assert!(!never.contains("forgotten"), "{never}");
    // A connection opened after Forget was never allowed: the not-yet words, and it asks.
    let (later, _, res) = connect_asking(&mut r, "Lights", Some(id.clone()), &["lighting.write"]);
    assert_eq!(res["granted"], json!([]));
    let (_, why) = reason(&lighting_call(&mut r, later), later);
    assert!(why.starts_with("Hideout hasn't let "), "{why}");
    // Allowed again: every open connection of the file has it back, and the words are gone.
    let out = r.set_tool_grants(&id, &[cap("lighting.write")]);
    assert_eq!(
        sends(&out, c)[0]["params"],
        json!({"added": ["lighting.write"]})
    );
    for conn in [c, later] {
        assert!(may_light(&mut r, conn));
    }
}

#[test]
fn forget_leaves_another_program_alone() {
    let mut r = free_only();
    let a = tool(r"C:\Tools\a.exe", "aa");
    let b = tool(r"C:\Tools\b.exe", "bb");
    let (ca, _, _) = connect_asking(&mut r, "a", Some(a.clone()), &["lighting.write"]);
    r.set_tool_grants(&a, &[cap("lighting.write")]);
    let (cb, _, _) = connect_asking(&mut r, "b", Some(b.clone()), &["lighting.write"]);
    r.set_tool_grants(&b, &[cap("lighting.write")]);
    // A third program waits for the user: its ask stays where it is.
    let w = tool(r"C:\Tools\w.exe", "cc");
    connect_asking(&mut r, "w", Some(w), &["lighting.write"]);
    let out = r.forget_tool(&a.image_path);
    assert!(sends(&out, cb).is_empty(), "{out:?}");
    assert_eq!(reason(&lighting_call(&mut r, ca), ca).0, 5);
    assert!(may_light(&mut r, cb));
    assert_eq!(r.tool_asks().len(), 1);
    let (_, _, res) = connect_asking(&mut r, "b", Some(b), &["lighting.write"]);
    assert_eq!(res["granted"], json!(["lighting.write"]));
}

#[test]
fn forget_ends_a_lease_held_under_what_was_lost() {
    let mut r = free_only();
    let mut e = entry("example.lights", &[]);
    e.granted = vec!["provide:lighting.*".into()];
    e.catalog_provide = Some(vec!["lighting.*".into()]);
    r.add_module(e);
    r.set_token("example.lights", "tok");
    let p = r.connect(Origin::Pipe);
    let init = json!({"id":"example.lights","version":"0.1.0","instance":"i","token":"tok",
        "hub":"1.3","api":{}});
    let out = call(&mut r, p, 0, "initialize", init);
    assert!(sends(&out, p)[0].get("result").is_some(), "{out:?}");
    let reg = json!({"methods":["lighting.*"],"facts":[],"channels":[],"apis":{}});
    call(&mut r, p, 1, "module.register", reg);
    r.receive(
        p,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        Instant::now(),
    );
    let id = tool(r"C:\Tools\lights.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, "Lights", Some(id.clone()), &["lighting.write"]);
    r.set_tool_grants(&id, &[cap("lighting.write")]);
    let out = lighting_call(&mut r, c);
    let fwd = sends(&out, p);
    assert_eq!(fwd.len(), 1, "{out:?}");
    let ans = json!({"jsonrpc":"2.0","id":fwd[0]["id"],"result":{"lease":"L1"}});
    r.receive(p, &ans.to_string(), Instant::now());
    let out = r.forget_tool(&id.image_path);
    let closed: Vec<Value> = sends(&out, p)
        .into_iter()
        .filter(|m| m["method"] == "lease.closed")
        .collect();
    assert_eq!(closed.len(), 1, "{out:?}");
    assert_eq!(
        closed[0]["params"],
        json!({"lease": "L1", "reason": "revoked"})
    );
}

/// A ready provider `id` that provides `methods` and offers `channels`.
fn provider(r: &mut Router, id: &str, methods: &[&str], channels: &[&str]) -> ConnId {
    let mut e = entry(id, &[]);
    e.granted = methods
        .iter()
        .map(|m| format!("provide:{m}"))
        .chain(channels.iter().map(|c| format!("channel.offer:{c}")))
        .collect();
    e.catalog_provide = Some(methods.iter().map(|m| m.to_string()).collect());
    e.channel_risk = channels
        .iter()
        .map(|c| (c.to_string(), "normal".to_string()))
        .collect();
    r.add_module(e);
    r.set_token(id, "tok");
    let p = r.connect(Origin::Pipe);
    let init = json!({"id":id,"version":"0.1.0","instance":"i","token":"tok",
        "hub":"1.3","api":{}});
    let out = call(r, p, 0, "initialize", init);
    assert!(sends(&out, p)[0].get("result").is_some(), "{out:?}");
    let reg = json!({"methods":methods,"facts":[],"channels":channels,"apis":{}});
    let out = call(r, p, 1, "module.register", reg);
    assert!(sends(&out, p)[0].get("result").is_some(), "{out:?}");
    r.receive(
        p,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        Instant::now(),
    );
    p
}

fn closed(out: &[Out], p: ConnId, method: &str) -> Vec<Value> {
    sends(out, p)
        .into_iter()
        .filter(|m| m["method"] == method)
        .map(|m| m["params"].clone())
        .collect()
}

/// A call under way when the user presses Forget does not make the program a lease holder
/// afterwards: the lease ends at the provider and the caller is refused.
#[test]
fn forget_during_a_call_leaves_no_lease_behind() {
    let mut r = free_only();
    let p = provider(&mut r, "example.lights", &["lighting.*"], &[]);
    let id = tool(r"C:\Tools\lights.exe", "aa");
    let (c, _, _) = connect_asking(&mut r, "Lights", Some(id.clone()), &["lighting.write"]);
    r.set_tool_grants(&id, &[cap("lighting.write")]);
    let out = lighting_call(&mut r, c);
    let fwd = sends(&out, p);
    assert_eq!(fwd.len(), 1, "{out:?}");
    // Forget while the provider works: nothing is held yet, so nothing is closed yet.
    let out = r.forget_tool(&id.image_path);
    assert!(closed(&out, p, "lease.closed").is_empty(), "{out:?}");
    let ans = json!({"jsonrpc":"2.0","id":fwd[0]["id"],"result":{"lease":"L1"}});
    let out = r.receive(p, &ans.to_string(), Instant::now());
    assert_eq!(
        closed(&out, p, "lease.closed"),
        [json!({"lease": "L1", "reason": "revoked"})],
        "{out:?}"
    );
    let (code, why) = reason(&out, c);
    assert_eq!(code, 5, "{out:?}");
    assert!(why.contains("was forgotten"), "{why}");
    // And nothing is held: a second Forget has no lease to close.
    let out = r.forget_tool(&id.image_path);
    assert!(closed(&out, p, "lease.closed").is_empty(), "{out:?}");
    // The forgotten connection stays refused.
    let out = lighting_call(&mut r, c);
    assert_eq!(reason(&out, c).0, 5, "{out:?}");
}

const FEED: &str = "example.feed";

fn open_status(r: &mut Router, c: ConnId, p: ConnId) -> (Vec<Out>, Value) {
    let out = call(
        r,
        c,
        7,
        "channel.open",
        json!({"module": FEED, "channel": "status"}),
    );
    let fwd = sends(&out, p);
    assert_eq!(fwd.len(), 1, "{out:?}");
    assert_eq!(fwd[0]["method"], "channel.create");
    (out, fwd[0].clone())
}

fn pipe_answer(create: &Value) -> String {
    json!({"jsonrpc":"2.0","id":create["id"],
        "result":{"pipe": r"\\.\pipe\hideout-ch-1-0-a-b"}})
    .to_string()
}

/// Forget ends the channels the program was handed: the provider is told to close them, also
/// when the program has closed its connection to the hub and kept the stream.
#[test]
fn forget_ends_the_channels_the_program_was_handed() {
    for hub_conn_open in [true, false] {
        let mut r = free_only();
        let p = provider(&mut r, FEED, &[], &["status"]);
        let id = tool(r"C:\Tools\feed.exe", "aa");
        let perm = format!("channel.open:{FEED}/status");
        let (c, _, _) = connect_asking(&mut r, "Feed", Some(id.clone()), &[&perm]);
        r.set_tool_grants(&id, &[cap(&perm)]);
        let (_, create) = open_status(&mut r, c, p);
        assert_eq!(create["params"]["consumer_pid"], 4242);
        let out = r.receive(p, &pipe_answer(&create), Instant::now());
        let got = sends(&out, c).remove(0);
        assert!(got["result"]["nonce"].is_string(), "{got}");
        if !hub_conn_open {
            r.disconnect(c);
        }
        // Another program's channel is left alone.
        let other = tool(r"C:\Tools\other.exe", "bb");
        let (co, _, _) = connect_asking(&mut r, "Other", Some(other.clone()), &[&perm]);
        r.set_tool_grants(&other, &[cap(&perm)]);
        let (_, create) = open_status(&mut r, co, p);
        r.receive(p, &pipe_answer(&create), Instant::now());

        let out = r.forget_tool(&id.image_path);
        assert_eq!(
            closed(&out, p, "channel.closed"),
            [json!({"channel": "status", "consumer_pid": 4242, "reason": "revoked"})],
            "open={hub_conn_open} {out:?}"
        );
        // Once: a second Forget has nothing left to end.
        let out = r.forget_tool(&id.image_path);
        assert!(closed(&out, p, "channel.closed").is_empty(), "{out:?}");
        if hub_conn_open {
            // The forgotten connection can't open it again, and is told why.
            let out = call(
                &mut r,
                c,
                8,
                "channel.open",
                json!({"module": FEED, "channel": "status"}),
            );
            assert!(sends(&out, p).is_empty(), "{out:?}");
            let (code, why) = reason(&out, c);
            assert_eq!(code, 5);
            assert!(why.contains("was forgotten"), "{why}");
        }
    }
}

/// A channel being made when the user presses Forget does not reach the program: the grant is
/// not handed out, and the provider is told to drop the pipe it made.
#[test]
fn forget_during_channel_open_hands_out_nothing() {
    let mut r = free_only();
    let p = provider(&mut r, FEED, &[], &["status"]);
    let id = tool(r"C:\Tools\feed.exe", "aa");
    let perm = format!("channel.open:{FEED}/status");
    let (c, _, _) = connect_asking(&mut r, "Feed", Some(id.clone()), &[&perm]);
    r.set_tool_grants(&id, &[cap(&perm)]);
    let (_, create) = open_status(&mut r, c, p);
    let out = r.forget_tool(&id.image_path);
    assert!(closed(&out, p, "channel.closed").is_empty(), "{out:?}");
    let out = r.receive(p, &pipe_answer(&create), Instant::now());
    let reply = sends(&out, c).remove(0);
    assert!(reply.get("result").is_none(), "{reply}");
    assert!(!reply.to_string().contains("nonce"), "{reply}");
    let (code, why) = reason(&out, c);
    assert_eq!(code, 5);
    assert!(why.contains("was forgotten"), "{why}");
    assert_eq!(
        closed(&out, p, "channel.closed"),
        [json!({"channel": "status", "consumer_pid": 4242, "reason": "revoked"})],
        "{out:?}"
    );
}

/// What the Apps page and `tools.json` are written from: every answer the hub holds, oldest
/// first, without the free set.
#[test]
fn approved_tools_lists_what_the_hub_holds() {
    let mut r = free_only();
    let a = tool(r"C:\Tools\a.exe", "aa");
    let b = tool(r"C:\Tools\b.exe", "bb");
    connect_asking(
        &mut r,
        "a",
        Some(a.clone()),
        &["lighting.write", "devices.read"],
    );
    r.set_tool_grants(&a, &[cap("lighting.write")]);
    connect_asking(&mut r, "b", Some(b.clone()), &["art.write"]);
    r.set_tool_grants(&b, &[cap("art.write")]);
    let held = r.approved_tools();
    assert_eq!(
        held,
        [
            (a.clone(), vec!["lighting.write".to_string()]),
            (b.clone(), vec!["art.write".to_string()])
        ]
    );
    r.forget_tool(&a.image_path);
    assert_eq!(r.approved_tools().len(), 1);
    assert_eq!(r.approved_tools()[0].0, b);
}

#[test]
fn the_apps_page_lists_only_what_a_program_can_be_allowed() {
    let r = free_only();
    let items = r.tool_items(&[
        "lighting.write".into(),
        "behaviors.invoke".into(),
        "devices.read".into(),
        "key.events".into(),
        "not a permission".into(),
        format!("channel.open:{KEYS}/events"),
        format!("channel.open:{KEYS}/status"),
    ]);
    let raw: Vec<&str> = items.iter().map(|i| i.raw.as_str()).collect();
    assert_eq!(
        raw,
        [
            "lighting.write",
            "behaviors.invoke",
            &format!("channel.open:{KEYS}/status")
        ]
    );
    assert!(!items[0].red && items[1].red);
}
