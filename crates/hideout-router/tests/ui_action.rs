//! `ui.register` over the legacy stdio bridge (§8.2, §4.10): a bridged plugin registers a tile,
//! and a button whose `action` names none of its `[[actions]]` is refused with `ui.action`
//! (spec v1.4, S3).
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;

use hideout_contract::capability::{Holder, Source};
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Origin, Router, Settings};
use serde_json::{json, Value};

const ID: &str = "foxglove.example-mute";

fn tile(action: &str) -> Value {
    json!({"instance": "pid-1", "panels": [{"id": "tile", "slot": "tile", "title": "Example",
        "icon": "mic", "tree": {"type": "column", "id": "tile", "children": [
            {"type": "value", "id": "glance", "props": {"value": "Muted", "tone": "live"}},
            {"type": "button", "id": "go", "props": {"label": "Unmute", "action": action}}]}}]})
}

#[test]
fn a_bridged_plugin_registers_a_tile_and_unknown_actions_are_refused() {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(ModuleEntry {
        id: ID.into(),
        name: "Example mute".into(),
        holder: Holder::ThirdParty(Source::Catalog),
        granted: vec![],
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: Some(vec!["tile".into()]),
        settings_dir: String::new(),
        data_dir: String::new(),
    });
    router.set_actions(ID, vec!["toggle".into(), "mute".into()]);
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let (from_hub, to_party) = std::io::pipe().unwrap();
    let (from_party, mut to_hub) = std::io::pipe().unwrap();
    hub.attach(Origin::Bridged(ID.into()), from_party, to_party);
    let mut from_hub = BufReader::new(from_hub);
    let mut call = |v: Value| {
        writeln!(to_hub, "{v}").unwrap();
        let mut line = String::new();
        from_hub.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let init = call(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "api":"1.3","client":{"name":"example","version":"1.0.0"}}}),
    );
    assert!(init.get("result").is_some(), "{init}");
    let ok = call(json!({"jsonrpc":"2.0","id":2,"method":"ui.register","params": tile("toggle")}));
    assert_eq!(ok["result"]["rejected"], json!([]), "{ok}");
    let refused =
        call(json!({"jsonrpc":"2.0","id":3,"method":"ui.register","params": tile("nope")}));
    assert_eq!(
        refused["result"]["rejected"],
        json!([{"panel": "tile", "error": "ui.action"}]),
        "{refused}"
    );
}
