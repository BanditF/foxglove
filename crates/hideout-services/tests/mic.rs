//! The microphone through a real hub (contract §8.11), over a fake endpoint layer: no real
//! device is read or muted. `mic.*` facts reach a subscriber with `audio.inputs.read`;
//! `audio.inputs.set_mute` needs `audio.inputs.mute`; another app's mute arrives as an event,
//! the hub's own echo is skipped.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hideout_contract::capability::Holder;
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Origin, Router, Settings};
use hideout_services::mic::{Backend, Endpoint, Event, MicService, MuteError};
use hideout_services::HostServices;
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Fake {
    muted: bool,
    set_calls: Vec<bool>,
}

struct Backend_(Arc<Mutex<Fake>>);

impl Backend for Backend_ {
    fn bind_default(&mut self) -> Result<Option<Endpoint>, String> {
        Ok(Some(Endpoint {
            id: "{0.0.1.00000000}.{fake}".into(),
            name: "Fake microphone".into(),
        }))
    }
    fn muted(&mut self) -> Result<bool, String> {
        Ok(self.0.lock().unwrap().muted)
    }
    fn set_mute(&mut self, muted: bool, _context: u128) -> Result<(), MuteError> {
        let mut f = self.0.lock().unwrap();
        f.set_calls.push(muted);
        f.muted = muted;
        Ok(())
    }
}

fn entry(id: &str, granted: &[&str]) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder: Holder::FirstParty,
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    }
}

struct Raw {
    w: std::io::PipeWriter,
    rx: Receiver<Value>,
    seen: Vec<Value>,
}

impl Raw {
    fn attach(hub: &Hub, id: &str) -> Raw {
        let (from_hub, to_party) = std::io::pipe().unwrap();
        let (from_party, to_hub) = std::io::pipe().unwrap();
        hub.attach(Origin::Bridged(id.into()), from_party, to_party);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(from_hub).lines() {
                let Ok(line) = line else { return };
                if tx
                    .send(serde_json::from_str::<Value>(&line).unwrap())
                    .is_err()
                {
                    return;
                }
            }
        });
        let mut raw = Raw {
            w: to_hub,
            rx,
            seen: Vec::new(),
        };
        let init = raw.call(
            0,
            "initialize",
            json!({"api": "1.3", "client": {"name": id, "version": "1"}}),
        );
        assert!(init.get("result").is_some(), "{init}");
        raw
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        writeln!(
            self.w,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        loop {
            let v = self.rx.recv_timeout(WAIT).expect("an answer");
            if v.get("method").is_some() {
                self.seen.push(v);
            } else if v["id"] == json!(id) {
                return v;
            }
        }
    }

    /// The next `facts.changed` for `key`.
    fn changed(&mut self, key: &str) -> Value {
        if let Some(i) = self.seen.iter().position(|e| e["params"]["key"] == key) {
            return self.seen.remove(i)["params"]["value"].clone();
        }
        loop {
            let v = self
                .rx
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| panic!("no change of {key}"));
            if v["params"]["key"] == key {
                return v["params"]["value"].clone();
            }
            self.seen.push(v);
        }
    }
}

#[test]
fn mic_state_and_set_mute_through_the_hub() {
    let mut router = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    router.add_module(entry(
        "example.mic-key",
        &["audio.inputs.read", "audio.inputs.mute"],
    ));
    router.add_module(entry(
        "example.reader",
        &["audio.inputs.read", "facts.write:mic.*"],
    ));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let fake = Arc::new(Mutex::new(Fake::default()));
    let f = fake.clone();
    let sink = hub.sink();
    let (cb_tx, cb_rx) = mpsc::channel::<Sender<Event>>();
    let mic = MicService::start(
        move |events| {
            let _ = cb_tx.send(events);
            Ok(Backend_(f))
        },
        Box::new(move |k, v| sink.publish(k, v)),
        WAIT,
    );
    // Where Core Audio's callbacks would send their events.
    let callbacks = cb_rx.recv_timeout(WAIT).unwrap();
    let own = mic.context();
    hub.set_services(Arc::new(HostServices {
        net: None,
        mic: Some(mic),
    }));

    // The facts are there before any module connects.
    let mut reader = Raw::attach(&hub, "example.reader");
    let got = reader.call(1, "facts.get", json!({"key": "mic.state"}));
    assert_eq!(got["result"]["value"], "unmuted", "{got}");
    let got = reader.call(2, "facts.get", json!({"key": "mic.endpoint_name"}));
    assert_eq!(got["result"]["value"], "Fake microphone");
    reader.call(3, "facts.subscribe", json!({"pattern": "mic.*"}));
    // A module may not write the hub's mic.* (§8.3), even with facts.write:mic.*.
    let spoof = reader.call(
        4,
        "facts.set",
        json!({"key": "mic.state", "value": "muted"}),
    );
    assert_eq!(spoof["error"]["code"], 5, "{spoof}");
    // Nor mute without audio.inputs.mute.
    let no = reader.call(5, "audio.inputs.set_mute", json!({"muted": true}));
    assert_eq!(no["error"]["code"], 5);
    assert_eq!(no["error"]["data"]["id"], "audio.denied");
    assert!(fake.lock().unwrap().set_calls.is_empty());

    // The mic key: muted, published at once.
    let mut key = Raw::attach(&hub, "example.mic-key");
    let bad = key.call(1, "audio.inputs.set_mute", json!({"muted": "yes"}));
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    let r = key.call(2, "audio.inputs.set_mute", json!({"muted": true}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(fake.lock().unwrap().set_calls, [true]);
    assert_eq!(reader.changed("mic.state"), "muted");
    // Its own OnNotify echo is skipped; Windows' own unmute (another context) arrives.
    callbacks
        .send(Event::Volume {
            muted: true,
            context: own,
        })
        .unwrap();
    callbacks
        .send(Event::Volume {
            muted: false,
            context: own ^ 1,
        })
        .unwrap();
    assert_eq!(reader.changed("mic.state"), "unmuted");
    assert!(reader
        .seen
        .iter()
        .all(|e| e["params"]["key"] != "mic.state"));
}
