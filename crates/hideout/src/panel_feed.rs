// SPDX-License-Identifier: MIT
//! The router's side of the panel host's feed (§5, §8.2 v1.4): what modules register and patch,
//! the facts the panels need (module states, `when` facts, bindings), `ui.asset.put` (whose
//! answer goes back to the module) and the answers to the hub's calls go to the shell; `ui.event`, `ui.visibility`, `action.invoke`, `settings.get`
//! and `settings.set` go to the module, bridged stdio plugins included. Event-driven: the
//! router's events thread pushes and wakes the shell; nothing polls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hideout_router::server::Hub;
use hideout_router::Out;
use hideout_shell::hideout_panels::feed::Waker;
use hideout_shell::hideout_panels::stream_io::{system_reduced_motion, StreamIo};
use hideout_shell::hideout_panels::streams::{StreamCommand, Streams};
use hideout_shell::hideout_panels::{HubCall, Inbound, Outbound, PanelFeed};
use serde_json::{json, Value};

/// What a call's tag stands for, to route its answer.
#[derive(Debug)]
enum Tag {
    Event(u64),
    Invoke { module: String, action: String },
    SettingsGet(String),
    SettingsSet(String),
    SecretsStatus(String),
    SetSecret(String),
}

/// Tags start high so they never meet the supervisor's.
const FIRST_TAG: u64 = 1 << 40;

#[derive(Clone)]
struct Shared {
    tx: mpsc::Sender<Inbound>,
    waker: Arc<Mutex<Option<Waker>>>,
    tags: Arc<Mutex<HashMap<u64, Tag>>>,
    /// The router's answers to the panel host's stream opens (`Out::HostChannel`), in order.
    grants: Arc<Mutex<Vec<Grant>>>,
}

/// One `Out::HostChannel`: module, channel, `{pipe, nonce}` or the error.
type Grant = (String, String, Result<Value, Value>);

impl Shared {
    fn wake(&self) {
        if let Some(w) = self.waker.lock().unwrap().as_ref() {
            w();
        }
    }

    fn push(&self, item: Inbound) {
        if self.tx.send(item).is_ok() {
            if let Some(w) = self.waker.lock().unwrap().as_ref() {
                w();
            }
        }
    }
}

/// The router events thread's end: turns router events into panel items.
#[derive(Clone)]
pub struct RouterSide(Shared);

/// The shell's end.
pub struct HubFeed {
    hub: Hub,
    rx: mpsc::Receiver<Inbound>,
    shared: Shared,
    next: AtomicU64,
    /// The live streams' channel readers (H2.9), made on first use.
    io: Option<StreamIo>,
}

/// A feed pair over `hub`.
pub fn feed(hub: Hub) -> (RouterSide, HubFeed) {
    let (tx, rx) = mpsc::channel();
    let shared = Shared {
        tx,
        waker: Arc::new(Mutex::new(None)),
        tags: Arc::new(Mutex::new(HashMap::new())),
        grants: Arc::new(Mutex::new(Vec::new())),
    };
    (
        RouterSide(shared.clone()),
        HubFeed {
            hub,
            rx,
            shared,
            next: AtomicU64::new(FIRST_TAG),
            io: None,
        },
    )
}

/// `data.reason` of an error result, else its message.
fn reason(e: &Value) -> String {
    e.pointer("/data/reason")
        .or_else(|| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("refused")
        .to_string()
}

impl RouterSide {
    /// Hands a module's manifest to the panel host.
    pub fn manifest(&self, m: hideout_contract::module::Module) {
        // The tray names a stopped module by its name (docs/hideout-supervisor-ux.md).
        hideout_shell::marks::note_name(&m.id, &m.name);
        self.0.push(Inbound::Manifest(Box::new(m)));
    }

    /// The module left the inventory (#1012.4): the shell closes its tabs and pop-outs.
    pub fn removed(&self, module: &str) {
        self.0.push(Inbound::ModuleRemoved {
            module: module.to_string(),
        });
    }

    /// Asks the shell for a frame (the Modules page has a fresh list, #1054.1).
    pub fn wake(&self) {
        self.0.wake();
    }

    /// One router event (the supervisor has seen it too).
    pub fn on_router(&self, out: &Out) {
        if let Out::HostChannel {
            module,
            channel,
            result,
        } = out
        {
            let g = (module.clone(), channel.clone(), result.clone());
            self.0.grants.lock().unwrap().push(g);
            self.0.wake();
            return;
        }
        let item = match out {
            Out::Panels {
                module,
                instance,
                panels,
            } => {
                let params =
                    serde_json::from_value(json!({"instance": instance, "panels": panels}));
                let Ok(params) = params else {
                    return;
                };
                // The router answered the call and checked the panels against the manifest.
                Inbound::Register {
                    call: 0,
                    module: module.clone(),
                    name: module.clone(),
                    declared: None,
                    actions: None,
                    params,
                }
            }
            Out::Patch { module, params } => match serde_json::from_value(params.clone()) {
                Ok(patch) => Inbound::Patch {
                    module: module.clone(),
                    patch,
                },
                Err(e) => {
                    log::warn!("panels: {module}: ui.patch not read: {e}");
                    return;
                }
            },
            Out::Badge { module, params } => match serde_json::from_value(params.clone()) {
                Ok(badge) => Inbound::Badge {
                    module: module.clone(),
                    badge,
                },
                Err(e) => {
                    log::warn!("panels: {module}: ui.badge not read: {e}");
                    return;
                }
            },
            // The router checked the params' shape; the panel host stores the image and its
            // answer goes back through `Router::host_answer` (§5.3, v1.7).
            Out::AssetPut {
                module,
                call,
                params,
            } => match serde_json::from_value(params.clone()) {
                Ok(asset) => Inbound::AssetPut {
                    call: *call,
                    module: module.clone(),
                    asset,
                },
                Err(e) => {
                    log::warn!("panels: {module}: ui.asset.put not read: {e}");
                    return;
                }
            },
            Out::Fact { key, value } => {
                // The tray's dot follows module states and safe mode, even with the window
                // hidden; it wakes the UI thread only when the dot changes.
                hideout_shell::marks::note_fact(key, value);
                Inbound::Fact {
                    name: key.clone(),
                    value: value.clone(),
                }
            }
            Out::HubResult { tag, result } => {
                let Some(t) = self.0.tags.lock().unwrap().remove(tag) else {
                    return;
                };
                match (t, result) {
                    (Tag::Event(seq), r) => Inbound::EventResult {
                        seq,
                        result: r.as_ref().map(|_| ()).map_err(reason),
                    },
                    (Tag::SettingsGet(module), Ok(v)) => Inbound::Settings {
                        module,
                        values: v
                            .get("values")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default(),
                    },
                    (Tag::SettingsGet(module), Err(e)) => {
                        log::info!("panels: {module}: settings.get: {}", reason(e));
                        return;
                    }
                    (Tag::SettingsSet(module), Err(e)) => Inbound::SettingRefused {
                        module,
                        reason: reason(e),
                    },
                    (Tag::SettingsSet(_), Ok(_)) => return,
                    (Tag::SecretsStatus(module), r) => Inbound::SecretsSaved {
                        module,
                        names: r
                            .as_ref()
                            .ok()
                            .and_then(|v| v.get("saved"))
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|n| n.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    },
                    (Tag::SetSecret(module), Err(e)) => Inbound::SecretRefused {
                        module,
                        reason: reason(e),
                    },
                    (Tag::SetSecret(_), Ok(_)) => return,
                    (Tag::Invoke { module, action }, Err(e)) => {
                        log::info!("panels: {module}: action {action}: {}", reason(e));
                        return;
                    }
                    (Tag::Invoke { .. }, Ok(_)) => return,
                }
            }
            _ => return,
        };
        self.0.push(item);
    }
}

impl HubFeed {
    fn tag(&self, t: Tag) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        self.shared.tags.lock().unwrap().insert(n, t);
        n
    }
}

impl PanelFeed for HubFeed {
    fn drain(&mut self) -> Vec<Inbound> {
        self.rx.try_iter().collect()
    }

    fn send(&mut self, out: Outbound) {
        let now = Instant::now();
        match out {
            // The router answered `ui.register` itself.
            Outbound::Registered { .. } => {}
            Outbound::Asset {
                call,
                module,
                result,
            } => {
                if let Err(e) = &result {
                    log::info!("panels: {module}: ui.asset.put refused: {e}");
                }
                self.hub
                    .act(|r| r.host_answer(call, result.map(|()| json!({}))));
            }
            Outbound::Event(e) => {
                let tag = self.tag(Tag::Event(e.seq));
                let params = serde_json::to_value(&e.event).unwrap_or_default();
                self.hub
                    .act(|r| r.call_module(&e.module, "ui.event", params, tag, now));
            }
            Outbound::Visibility { module, visibility } => {
                let params = serde_json::to_value(&visibility).unwrap_or_default();
                self.hub
                    .act(|r| r.notify_module(&module, "ui.visibility", params));
            }
            Outbound::Call(HubCall::Invoke { module, action }) => {
                let params = json!({"action": action, "source": "panel"});
                let tag = self.tag(Tag::Invoke {
                    module: module.clone(),
                    action,
                });
                self.hub
                    .act(|r| r.call_module(&module, "action.invoke", params, tag, now));
            }
            Outbound::Call(HubCall::SettingsGet { module }) => {
                let tag = self.tag(Tag::SettingsGet(module.clone()));
                self.hub.act(|r| r.get_settings(&module, tag, now));
            }
            Outbound::Call(HubCall::SecretsStatus { module }) => {
                let tag = self.tag(Tag::SecretsStatus(module.clone()));
                self.hub.act(|r| r.secrets_status(&module, tag));
            }
            Outbound::Call(HubCall::SetSecret {
                module,
                name,
                value,
            }) => {
                let tag = self.tag(Tag::SetSecret(module.clone()));
                self.hub.act(|r| r.set_secret(&module, &name, value, tag));
            }
            Outbound::Call(HubCall::SettingsSet {
                module,
                name,
                value,
            }) => {
                let tag = self.tag(Tag::SettingsSet(module.clone()));
                self.hub
                    .act(|r| r.set_setting(&module, &name, value, tag, now));
            }
        }
    }

    fn set_waker(&mut self, waker: Waker) {
        *self.shared.waker.lock().unwrap() = Some(waker);
    }

    /// The `open` activation event (§4.6, v1.10): the router sends `Activate` to the supervisor
    /// when the module waits for `open` and isn't connected.
    fn opened(&mut self, module: &str) {
        self.hub.act(|r| r.panel_opened(module));
    }

    fn streams(&mut self, streams: &mut Streams, now: Instant) {
        let shared = self.shared.clone();
        let io = self
            .io
            .get_or_insert_with(|| StreamIo::new(Arc::new(move || shared.wake())));
        let grants = std::mem::take(&mut *self.shared.grants.lock().unwrap());
        for (module, channel, result) in grants {
            io.granted(streams, &module, &channel, result);
        }
        streams.reduced_motion = system_reduced_motion();
        let (_, opens) = io.pump(streams, now);
        for o in opens {
            if let StreamCommand::Open {
                module, channel, ..
            } = o
            {
                self.hub
                    .act(|r| r.open_host_channel(&module, &channel, now));
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    //! `ui.asset.put` end to end over a real router pipe (§5.3, v1.7): the module's call reaches
    //! the hub's panel host through this feed, the panel host's answer goes back to the module,
    //! and the stored image is the one its panel's `image` node names. Window-free.
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;

    use hideout_contract::capability::Holder;
    use hideout_router::{ModuleEntry, Router, Settings};
    use hideout_shell::hideout_panels::{PanelHost, PanelKey};

    const ID: &str = "foxglove.example-art";
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// The hub's feed, counting the panel host's `ui.asset.put` answers it sends on.
    struct Spy {
        feed: HubFeed,
        answers: usize,
    }

    impl PanelFeed for Spy {
        fn drain(&mut self) -> Vec<Inbound> {
            self.feed.drain()
        }

        fn send(&mut self, out: Outbound) {
            if matches!(out, Outbound::Asset { .. }) {
                self.answers += 1;
            }
            self.feed.send(out);
        }

        fn set_waker(&mut self, waker: Waker) {
            self.feed.set_waker(waker);
        }
    }

    #[test]
    fn a_module_puts_an_asset_over_the_router_pipe_and_its_panel_has_it() {
        let mut router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
        router.add_module(ModuleEntry {
            id: ID.into(),
            name: "Example Art".into(),
            holder: Holder::FirstParty,
            granted: vec![],
            catalog_provide: None,
            channel_risk: Default::default(),
            activate_on_call: false,
            panels: None,
            settings_dir: String::new(),
            data_dir: String::new(),
        });
        router.set_token(ID, "tok-art");
        let (events, rx) = mpsc::channel();
        let hub = Hub::new(router, events);
        let (side, feed) = feed(hub.clone());
        let mut feed = Spy { feed, answers: 0 };
        let (wake_tx, wake) = mpsc::channel::<()>();
        let wake_tx = Mutex::new(wake_tx);
        feed.set_waker(Box::new(move || {
            let _ = wake_tx.lock().unwrap().send(());
        }));
        std::thread::spawn(move || {
            for o in rx {
                side.on_router(&o);
            }
        });
        let name = format!(r"\\.\pipe\hideout-asset-put-test-{}", std::process::id());
        hub.serve_pipe(&name).expect("serve the test pipe");

        let pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("router pipe");
        // One synchronous pipe handle serialises its reads and writes, so the module side
        // reads only once its answer was sent.
        let mut from_hub = BufReader::new(pipe.try_clone().unwrap());
        let mut read = move || {
            let mut line = String::new();
            from_hub.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        let mut to_hub = pipe;
        let mut send = |v: Value| writeln!(to_hub, "{v}").unwrap();
        // The panel host runs on the hub's frames: it pumps when the feed wakes it, until it
        // has answered `n` asset calls in all.
        let mut host = PanelHost::default();
        let mut pump_until_answers = |host: &mut PanelHost, n: usize| {
            let until = Instant::now() + Duration::from_secs(10);
            while feed.answers < n {
                let left = until.saturating_duration_since(Instant::now());
                wake.recv_timeout(left)
                    .expect("the panel host got nothing in 10 s");
                host.pump(&mut feed, Instant::now());
            }
        };
        let mut answer = |_: &mut PanelHost| read();

        send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "id": ID, "token": "tok-art", "instance": "i1", "version": "0.4.0", "hub": "1.0",
            "api": {}, "client": {"name": "example-art", "version": "0.4.0"}}}),
        );
        assert!(answer(&mut host).get("result").is_some());
        send(
            json!({"jsonrpc": "2.0", "id": 2, "method": "ui.register", "params": {
            "instance": "i1", "panels": [{"id": "art", "slot": "page", "title": "Art",
            "icon": "image", "tree": {"type": "column", "id": "root", "children": [
                {"type": "image", "id": "shot", "props": {"hash": HASH, "label": "Art"}}]}}]}}),
        );
        assert_eq!(answer(&mut host)["result"]["rejected"], json!([]));

        // A non-image: the panel host refuses it, and the refusal reaches the module.
        send(
            json!({"jsonrpc": "2.0", "id": 3, "method": "ui.asset.put", "params": {
            "hash": HASH, "mime": "text/plain", "data": "aGVsbG8="}}),
        );
        pump_until_answers(&mut host, 1);
        let refused = answer(&mut host);
        assert_eq!(refused["id"], 3);
        assert_eq!(refused["error"]["code"], -32602);
        assert_eq!(refused["error"]["data"]["id"], "ui.limits");
        assert!(host.assets.get(HASH).is_none());

        // An image: stored, answered `{}`, and it is the image the panel's node names.
        send(
            json!({"jsonrpc": "2.0", "id": 4, "method": "ui.asset.put", "params": {
            "hash": HASH, "mime": "image/png", "data": "iVBORw0KGgo="}}),
        );
        pump_until_answers(&mut host, 2);
        assert_eq!(
            answer(&mut host),
            json!({"jsonrpc": "2.0", "id": 4, "result": {}})
        );
        let key = PanelKey::new(ID, "art");
        let tree = serde_json::to_value(&host.panel(&key).unwrap().tree).unwrap();
        assert_eq!(tree["children"][0]["props"]["hash"], HASH);
        assert_eq!(host.assets.get(HASH).unwrap().mime, "image/png");
    }
}
