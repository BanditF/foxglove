//! The legacy stdio bridge (§8.2): a v1 plugin's stdin/stdout on the router, with its identity
//! from the bridge and its grant from the inventory; it speaks the v1 wire unchanged
//! (docs/local-api.md: `initialize` with a string `api`, then facts and pings).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::time::Duration;

use hideout_contract::capability::{Holder, Source};
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Origin, Router, Settings};
use serde_json::{json, Value};

#[test]
fn a_legacy_plugin_works_through_the_bridge() {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(ModuleEntry {
        id: "foxglove.example-weather".into(),
        name: "Example Weather".into(),
        holder: Holder::ThirdParty(Source::Catalog),
        granted: vec!["facts.write:weather.*".into(), "devices.read".into()],
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
    });
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);

    // A tool watching weather facts, and the plugin, both in process.
    let open = |origin: Origin| {
        let (from_hub, to_party) = std::io::pipe().unwrap();
        let (from_party, to_hub) = std::io::pipe().unwrap();
        hub.attach(origin, from_party, to_party);
        (to_hub, BufReader::new(from_hub))
    };
    let call = |(w, r): &mut (std::io::PipeWriter, BufReader<std::io::PipeReader>), v: Value| {
        writeln!(w, "{v}").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let mut tool = open(Origin::Pipe);
    let init = call(
        &mut tool,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "api":"1.0","client":{"name":"watcher","version":"1"},"permissions":["facts.read:weather.*"]}}),
    );
    assert_eq!(init["result"]["granted"], json!(["facts.read:weather.*"]));
    let sub = call(
        &mut tool,
        json!({"jsonrpc":"2.0","id":2,"method":"facts.subscribe","params":{"pattern":"weather.*"}}),
    );
    let sub = sub["result"]["subscription"].clone();

    let mut plugin = open(Origin::Bridged("foxglove.example-weather".into()));
    let init = call(
        &mut plugin,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "api":"1.0","client":{"name":"weather","version":"1.2.0"}}}),
    );
    assert_eq!(
        init["result"]["granted"],
        json!(["devices.read", "facts.write:weather.*"]),
        "{init}"
    );
    assert_eq!(init["result"]["api"], "1.0");
    let ping = call(
        &mut plugin,
        json!({"jsonrpc":"2.0","id":2,"method":"ping","params":{}}),
    );
    assert_eq!(ping["result"], json!({}));
    let set = call(
        &mut plugin,
        json!({"jsonrpc":"2.0","id":3,"method":"facts.set","params":{"key":"weather.temp","value":"14"}}),
    );
    assert_eq!(set["result"], json!({}));
    let denied = call(
        &mut plugin,
        json!({"jsonrpc":"2.0","id":4,"method":"facts.set","params":{"key":"device.x","value":1}}),
    );
    assert_eq!(denied["error"]["code"], 5);

    // The watcher got the change.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        tool.1.read_line(&mut line).unwrap();
        tx.send(line).unwrap();
    });
    let changed: Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(10)).unwrap()).unwrap();
    assert_eq!(changed["method"], "facts.changed");
    assert_eq!(
        changed["params"],
        json!({"subscription": sub, "key": "weather.temp", "value": "14"})
    );
}
