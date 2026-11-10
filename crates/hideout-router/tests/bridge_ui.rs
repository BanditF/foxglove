//! H → M over the legacy stdio bridge (§8.2 v1.4, #991): the manifest's panels and actions are
//! checked live, and `ui.event`, `action.invoke` and `ui.visibility` reach a bridged plugin
//! unchanged, with the answers back to the hub as [`Out::HubResult`]. Facts the hub's panel host
//! watches come out as [`Out::Fact`].
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const ID: &str = "example.bridged";

const MANIFEST: &str = r#"
id = "example.bridged"
name = "Example bridged"
version = "1.0.0"
kind = "service"
family = "foxglove"
hub = "1.0"
transport = "stdio"
api = { foxglove = "1.2" }
run = ["example.exe"]
description = "A bridged example."

[[ui.panels]]
id = "tile"
slot = "tile"
title = "Example"
icon = "mic"

[[actions]]
id = "toggle"
name = "Toggle"
"#;

fn entry() -> ModuleEntry {
    ModuleEntry {
        id: ID.into(),
        name: ID.into(),
        holder: Holder::ThirdParty(Source::Catalog),
        granted: vec![],
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
    }
}

fn tile(action: &str) -> Value {
    json!({"instance": "pid-7", "panels": [{"id": "tile", "slot": "tile", "title": "Example",
        "icon": "mic", "tree": {"type": "column", "id": "tile", "children": [
            {"type": "value", "id": "glance", "props": {"value": "On"}},
            {"type": "button", "id": "go", "props": {"label": "Toggle", "action": action}}]}}]})
}

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "api":"1.3","client":{"name":"example","version":"1.0.0"}}})
}

fn recv(rx: &mpsc::Receiver<Out>, want: impl Fn(&Out) -> bool) -> Out {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        let o = rx.recv_timeout(left).expect("no router event in 5 s");
        if want(&o) {
            return o;
        }
    }
}

#[test]
fn ui_event_action_invoke_and_visibility_reach_a_bridged_plugin() {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(entry());
    let manifest = Module::parse(MANIFEST, &Context::default()).expect("manifest parses");
    router.apply_manifest(&manifest);
    assert!(router.watch_facts(vec!["module.".into()]).is_empty());
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let (from_hub, to_party) = std::io::pipe().unwrap();
    let (from_party, mut to_hub) = std::io::pipe().unwrap();
    hub.attach(Origin::Bridged(ID.into()), from_party, to_party);
    let mut from_hub = BufReader::new(from_hub);
    let mut read = || {
        let mut line = String::new();
        from_hub.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let mut send = |v: Value| writeln!(to_hub, "{v}").unwrap();

    send(initialize());
    assert!(read().get("result").is_some());
    // The manifest's declared actions apply live: an undeclared action is refused.
    send(json!({"jsonrpc":"2.0","id":2,"method":"ui.register","params": tile("nope")}));
    assert_eq!(
        read()["result"]["rejected"],
        json!([{"panel": "tile", "error": "ui.action"}])
    );
    send(json!({"jsonrpc":"2.0","id":3,"method":"ui.register","params": tile("toggle")}));
    assert_eq!(read()["result"]["rejected"], json!([]));
    let registered = recv(
        &rx,
        |o| matches!(o, Out::Panels { panels, .. } if panels.as_array().is_some_and(|a| !a.is_empty())),
    );
    let Out::Panels {
        module, instance, ..
    } = registered
    else {
        unreachable!()
    };
    assert_eq!((module.as_str(), instance.as_str()), (ID, "pid-7"));

    // ui.visibility: a notification, passed unchanged; no module.ready needed (§8.2).
    hub.act(|r| {
        r.notify_module(
            ID,
            "ui.visibility",
            json!({"panel": "tile", "state": "visible"}),
        )
    });
    let v = read();
    assert_eq!(v["method"], "ui.visibility");
    assert_eq!(v["params"], json!({"panel": "tile", "state": "visible"}));
    assert!(v.get("id").is_none(), "a notification has no id: {v}");

    // action.invoke from the tile's button: a call; the plugin's answer comes back by tag.
    let now = Instant::now();
    hub.act(|r| {
        r.call_module(
            ID,
            "action.invoke",
            json!({"action": "toggle", "source": "panel"}),
            41,
            now,
        )
    });
    let call = read();
    assert_eq!(call["method"], "action.invoke");
    assert_eq!(
        call["params"],
        json!({"action": "toggle", "source": "panel"})
    );
    send(json!({"jsonrpc":"2.0","id": call["id"],"result":{}}));
    let r = recv(&rx, |o| matches!(o, Out::HubResult { tag: 41, .. }));
    assert_eq!(
        r,
        Out::HubResult {
            tag: 41,
            result: Ok(json!({}))
        }
    );

    // ui.event refused: the error, with data.reason, comes back to the hub.
    hub.act(|r| {
        r.call_module(
            ID,
            "ui.event",
            json!({"panel": "tile", "node": "retry", "kind": "click", "value": null}),
            42,
            now,
        )
    });
    let call = read();
    assert_eq!(call["method"], "ui.event");
    assert_eq!(call["params"]["node"], "retry");
    send(json!({"jsonrpc":"2.0","id": call["id"],
        "error":{"code": 5, "message": "refused", "data": {"reason": "Windows said no"}}}));
    let Out::HubResult { result: Err(e), .. } =
        recv(&rx, |o| matches!(o, Out::HubResult { tag: 42, .. }))
    else {
        panic!("ui.event should be refused")
    };
    assert_eq!(e["data"]["reason"], "Windows said no");

    // A watched fact: the module state the supervisor publishes reaches the panel host.
    hub.act(|r| r.publish(&format!("module.{ID}.state"), json!("restarting")));
    let f = recv(&rx, |o| matches!(o, Out::Fact { .. }));
    assert_eq!(
        f,
        Out::Fact {
            key: format!("module.{ID}.state"),
            value: json!("restarting")
        }
    );

    // ui.badge: a module notification for the panel host (v1.3), passed unchanged (#991).
    send(json!({"jsonrpc":"2.0","method":"ui.badge",
        "params":{"panel": "tile", "badge": "count", "count": 3}}));
    let b = recv(&rx, |o| matches!(o, Out::Badge { .. }));
    assert_eq!(
        b,
        Out::Badge {
            module: ID.into(),
            params: json!({"panel": "tile", "badge": "count", "count": 3})
        }
    );
}

#[test]
fn a_manifest_without_panels_declares_none() {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(entry());
    let mut m = Module::parse(MANIFEST, &Context::default()).unwrap();
    m.panels.clear();
    router.apply_manifest(&m);
    let conn = router.connect(Origin::Bridged(ID.into()));
    let now = Instant::now();
    router.receive(conn, &initialize().to_string(), now);
    let line = json!({"jsonrpc":"2.0","id":2,"method":"ui.register","params": tile("toggle")});
    let out = router.receive(conn, &line.to_string(), now);
    let reply = out.iter().find_map(|o| match o {
        Out::Send(_, v) if v["id"] == 2 => Some(v.clone()),
        _ => None,
    });
    assert_eq!(
        reply.unwrap()["result"]["rejected"],
        json!([{"panel": "tile", "error": "ui.undeclared"}])
    );
}

/// A v1 plugin's settings live in the hub (§4.10): its `settings.get` is answered from the
/// manifest defaults and the saved file, and a change from the hub's form is checked, saved and
/// sent as `settings.changed`.
#[test]
fn the_hub_keeps_a_v1_plugins_settings() {
    let dir = std::env::temp_dir().join(format!("hideout-bridge-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(ModuleEntry {
        settings_dir: dir.display().to_string(),
        ..entry()
    });
    let text = format!(
        "{MANIFEST}\n[[settings]]\nname = \"device\"\nlabel = \"Device\"\nkind = \"text\"\ndefault = \"\"\n"
    );
    router.apply_manifest(&Module::parse(&text, &Context::default()).unwrap());
    let conn = router.connect(Origin::Bridged(ID.into()));
    let now = Instant::now();
    router.receive(conn, &initialize().to_string(), now);
    let ask = |r: &mut Router, n: i64| {
        let line = json!({"jsonrpc":"2.0","id": n,"method":"settings.get","params":{}});
        let out = r.receive(conn, &line.to_string(), now);
        out.iter()
            .find_map(|o| match o {
                Out::Send(_, v) if v["id"] == n => Some(v["result"].clone()),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(ask(&mut router, 2), json!({"values": {"device": ""}}));
    let out = router.set_setting(ID, "device", json!("K70"), 9, now);
    let changed = out.iter().find_map(|o| match o {
        Out::Send(_, v) if v["method"] == "settings.changed" => Some(v["params"].clone()),
        _ => None,
    });
    assert_eq!(changed, Some(json!({"values": {"device": "K70"}})));
    assert!(out.contains(&Out::HubResult {
        tag: 9,
        result: Ok(json!({}))
    }));
    assert_eq!(ask(&mut router, 3), json!({"values": {"device": "K70"}}));
    // A value of the wrong kind is refused with a reason, and nothing changes.
    let out = router.set_setting(ID, "device", json!(5), 10, now);
    assert!(
        matches!(&out[..], [Out::HubResult { tag: 10, result: Err(e) }] if e["data"]["reason"].is_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `[[secrets]]` live in the hub (Credential Manager; memory here): the form saves one and
/// learns only that it is saved; the module reads its own declared secret, nothing else.
#[test]
fn secrets_are_kept_by_the_hub_and_read_by_their_module_only() {
    use hideout_router::secrets::MemorySecrets;
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.set_secret_store(Box::new(MemorySecrets::default()));
    router.add_module(entry());
    let text = format!(
        "{}\n[[secrets]]\nname = \"pw\"\nlabel = \"Password\"\n",
        MANIFEST.replace(
            "description =",
            "permissions = [\"secrets\"]\ndescription ="
        )
    );
    router.apply_manifest(&Module::parse(&text, &Context::default()).unwrap());
    let conn = router.connect(Origin::Bridged(ID.into()));
    let now = Instant::now();
    router.receive(conn, &initialize().to_string(), now);
    let get = |r: &mut Router, n: i64, name: &str| {
        let line = json!({"jsonrpc":"2.0","id": n,"method":"secrets.get","params":{"name": name}});
        r.receive(conn, &line.to_string(), now)
            .into_iter()
            .find_map(|o| match o {
                Out::Send(_, v) if v["id"] == n => Some(v),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(get(&mut router, 2, "pw")["result"], json!({"value": null}));
    assert_eq!(
        router.secrets_status(ID, 1),
        [Out::HubResult {
            tag: 1,
            result: Ok(json!({"saved": []}))
        }]
    );
    let out = router.set_secret(ID, "pw", Some("hunter2".into()), 2);
    assert_eq!(
        out,
        [Out::HubResult {
            tag: 2,
            result: Ok(json!({}))
        }]
    );
    assert_eq!(router.secrets_saved(ID), ["pw"]);
    assert_eq!(
        get(&mut router, 3, "pw")["result"],
        json!({"value": "hunter2"})
    );
    // Not declared: refused both ways.
    assert!(get(&mut router, 4, "other").get("error").is_some());
    assert!(matches!(
        &router.set_secret(ID, "other", Some("x".into()), 5)[..],
        [Out::HubResult { result: Err(_), .. }]
    ));
}
