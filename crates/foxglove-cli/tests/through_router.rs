//! `foxglove-cli status` through the Hideout router (#934): the router serves a test-named pipe
//! (never the user's Foxglove or hub pipe), a stub first-party module provides `devices.*` and
//! publishes its device facts, and the real CLI binary talks to the router as an outside tool.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;

use hideout_contract::capability::Holder;
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Origin, Router, Settings};
use serde_json::{json, Value};

const LINK: &str = "foxglove.example-link";

#[test]
fn status_works_through_the_router() {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(ModuleEntry {
        id: LINK.into(),
        name: "Example Link".into(),
        holder: Holder::FirstParty,
        granted: vec!["provide:devices.*".into()],
        catalog_provide: Some(vec!["devices.*".into()]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    });
    let token = router.issue_token(LINK);
    let (events, _events_rx) = mpsc::channel();
    let hub = Hub::new(router, events);

    // The stub module, over in-process pipes.
    let (from_hub, to_module) = std::io::pipe().unwrap();
    let (from_module, to_hub) = std::io::pipe().unwrap();
    hub.attach(Origin::Pipe, from_module, to_module);
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut w = to_hub;
        let mut r = BufReader::new(from_hub);
        let mut send = |v: Value| writeln!(w, "{v}").unwrap();
        let mut line = String::new();
        let mut read = |line: &mut String| -> Value {
            line.clear();
            r.read_line(line).unwrap();
            serde_json::from_str(line).unwrap()
        };
        send(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "id":LINK,"version":"0.1.0","instance":"i1","token":token,"hub":"1.0",
            "api":{"foxglove":"1.16"}}}),
        );
        assert!(read(&mut line).get("result").is_some());
        send(
            json!({"jsonrpc":"2.0","id":2,"method":"module.register","params":{
            "methods":["devices.*"],"facts":["device.*"],"channels":[],"apis":{"foxglove":"1.16"}}}),
        );
        assert_eq!(read(&mut line)["result"]["rejected"], json!([]));
        send(
            json!({"jsonrpc":"2.0","id":3,"method":"facts.set","params":{
            "key":"device.board-1.layer","value":"base"}}),
        );
        assert_eq!(read(&mut line)["result"], json!({}));
        send(json!({"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i1"}}));
        ready_tx.send(()).unwrap();
        loop {
            let call = read(&mut line);
            if call["method"] == "devices.list" {
                send(
                    json!({"jsonrpc":"2.0","id":call["id"],"result":{"devices":[{
                    "id":"board-1","name":"Example Board","kind":"keyboard","backend":"example",
                    "connected":true,"caps":["lighting"]}]}}),
                );
            }
        }
    });
    ready_rx.recv().unwrap();

    let pipe = format!(r"\\.\pipe\hideout-test-{}", std::process::id());
    hub.serve_pipe(&pipe).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_foxglove-cli"))
        .arg("status")
        .env("FOXGLOVE_PIPE", &pipe)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("Foxglove hideout (API 1.16)"), "{stdout}");
    assert!(
        stdout.contains("Example Board [example] connected: lighting"),
        "{stdout}"
    );
    assert!(stdout.contains("layer = base"), "{stdout}");
}
