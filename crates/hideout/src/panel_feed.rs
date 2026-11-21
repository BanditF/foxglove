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
use hideout_shell::quick_actions::{self, Notice};
use serde_json::{json, Value};

/// What a call's tag stands for, to route its answer.
#[derive(Debug)]
enum Tag {
    Event(u64),
    Invoke {
        module: String,
        action: String,
    },
    /// A tray menu quick action (#1234): a refusal or a start that didn't come is told.
    Tray {
        module: String,
        action: String,
    },
    SettingsGet(String),
    SettingsSet(String),
    SecretsStatus(String),
    SetSecret(String),
}

/// Tags start high so they never meet the supervisor's.
const FIRST_TAG: u64 = 1 << 40;

/// Bound keys' calls (#991) number from here, apart from the shell's own tags.
const FIRST_KEY_TAG: u64 = 1 << 41;

/// Tray presses' calls (#1234) number from here.
const FIRST_TRAY_TAG: u64 = 1 << 42;

/// How long a tray press waits for its module to start (§4.6, v1.16).
pub const TRAY_COLD_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Shows a notification about a tray press that didn't happen.
pub type Notifier = Arc<dyn Fn(Notice) + Send + Sync>;

#[derive(Clone)]
struct Shared {
    tx: mpsc::Sender<Inbound>,
    waker: Arc<Mutex<Option<Waker>>>,
    tags: Arc<Mutex<HashMap<u64, Tag>>>,
    /// The router's answers to the panel host's stream opens (`Out::HostChannel`), in order.
    grants: Arc<Mutex<Vec<Grant>>>,
    notifier: Notifier,
    next_tray: Arc<AtomicU64>,
}

/// The real notifier: a toast with **Open <Module>**.
fn toast(n: Notice) {
    let arg = format!("open:{}", n.module);
    hideout_shell::notify::toast(
        n.title,
        n.body,
        vec![(n.button, arg.clone())],
        arg,
        Arc::new(|a: &str| {
            if let Some(("open", id)) = a.split_once(':') {
                hideout_shell::open_module(id);
            }
        }),
    );
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
    feed_with(hub, Arc::new(toast))
}

/// A feed pair over `hub` whose tray press failures go to `notifier` (tests show no toast).
pub fn feed_with(hub: Hub, notifier: Notifier) -> (RouterSide, HubFeed) {
    // The tray's Resume (#1142): the router ends the family's leases at the provider.
    let resumer = hub.clone();
    hideout_shell::marks::set_resumer(Box::new(move |provider: &str, family: &str| {
        resumer.act(|r| r.resume_leases(provider, family));
    }));
    let (tx, rx) = mpsc::channel();
    let shared = Shared {
        tx,
        waker: Arc::new(Mutex::new(None)),
        tags: Arc::new(Mutex::new(HashMap::new())),
        grants: Arc::new(Mutex::new(Vec::new())),
        notifier,
        next_tray: Arc::new(AtomicU64::new(FIRST_TRAY_TAG)),
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

/// A bound key's `action.invoke` params (§8.8: `source` is `key`).
fn key_params(action: &str) -> Value {
    json!({"action": action, "source": "key"})
}

/// A tray menu press's `action.invoke` params (§8.8: `source` is `tray`, v1.16 a user source).
fn tray_params(action: &str) -> Value {
    json!({"action": action, "source": "tray"})
}

/// The notification for a tray press the router answered with `e`; `None` for a double press
/// (busy: the first one is still on its way).
fn tray_notice(module: &str, action: &str, e: &Value) -> Option<Notice> {
    let code = e.get("code").and_then(Value::as_i64);
    if code == Some(3) {
        return None;
    }
    let not_started = e.get("message").and_then(Value::as_str) == Some(hideout_router::NOT_STARTED)
        || code == Some(2);
    let (name, title) = quick_actions::words_for(module, action);
    // The module's own reason; else a press it took but never answered (router timeout).
    let fallback = if code == Some(4) {
        "It didn't answer in time."
    } else {
        ""
    };
    let r = e
        .pointer("/data/reason")
        .and_then(Value::as_str)
        .filter(|r| !r.trim().is_empty())
        .unwrap_or(fallback);
    Some(quick_actions::failure_notice(
        &name,
        &title,
        module,
        not_started,
        r,
    ))
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
        // Its `action` panels are tray menu items (#1234).
        quick_actions::note_manifest(&m);
        self.0.push(Inbound::Manifest(Box::new(m)));
    }

    /// The module left the inventory (#1012.4): the shell closes its tabs and pop-outs.
    pub fn removed(&self, module: &str) {
        quick_actions::note_removed(module);
        hideout_shell::marks::note_removed(module);
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
                quick_actions::note_panels(module, panels);
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
                Ok(patch) => {
                    quick_actions::note_patch(module, params);
                    Inbound::Patch {
                        module: module.clone(),
                        patch,
                    }
                }
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
            // `hub.show` (v1.19): a verified same-user peer asked for this module's page.
            Out::Show { module } => {
                hideout_shell::open_module(module);
                return;
            }
            Out::Fact { key, value } => {
                // The tray's dot follows module states and safe mode, even with the window
                // hidden; it wakes the UI thread only when the dot changes.
                hideout_shell::marks::note_fact(key, value);
                // The tray menu's quick actions follow the cached facts (§4.6 `when`).
                quick_actions::note_fact(key, value);
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
                    // Silent success: the item's next words say it (#1234, UX Q3).
                    (Tag::Tray { .. }, Ok(_)) => return,
                    (Tag::Tray { module, action }, Err(e)) => {
                        log::info!("panels: {module}: tray {action}: {}", reason(e));
                        if let Some(n) = tray_notice(&module, &action, e) {
                            (self.0.notifier)(n);
                        }
                        return;
                    }
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

    /// A bound key (#991): `action.invoke {action, source: "key"}` from the hotkey thread; a
    /// refusal is logged like a button's.
    fn key_invoker(&self) -> Option<hideout_shell::hideout_panels::KeyInvoker> {
        let hub = self.hub.clone();
        let tags = self.shared.tags.clone();
        let next = Arc::new(AtomicU64::new(FIRST_KEY_TAG));
        Some(Arc::new(move |module: &str, action: &str| {
            let tag = next.fetch_add(1, Ordering::Relaxed);
            tags.lock().unwrap().insert(
                tag,
                Tag::Invoke {
                    module: module.to_string(),
                    action: action.to_string(),
                },
            );
            let params = key_params(action);
            // The press goes first; a module with no connection answers "disconnected" and
            // is started for the next one (the keys are armed at start, so this is a late join).
            hub.act(|r| {
                let mut out = r.call_module(module, "action.invoke", params, tag, Instant::now());
                out.extend(r.key_bound(module));
                out
            });
        }))
    }

    /// A tray menu quick action (#1234): `action.invoke {action, source: "tray"}`. A module with
    /// no connection is started and gets the press once ready, within [`TRAY_COLD_WAIT`], else
    /// the user gets a notification (§4.6, v1.16). The tags' lock is let go before the router's
    /// is taken.
    fn tray_invoker(&self) -> Option<hideout_shell::hideout_panels::KeyInvoker> {
        let hub = self.hub.clone();
        let tags = self.shared.tags.clone();
        let next = self.shared.next_tray.clone();
        Some(Arc::new(move |module: &str, action: &str| {
            let tag = next.fetch_add(1, Ordering::Relaxed);
            tags.lock().unwrap().insert(
                tag,
                Tag::Tray {
                    module: module.to_string(),
                    action: action.to_string(),
                },
            );
            let params = tray_params(action);
            hub.act(|r| {
                r.call_module_starting(
                    module,
                    "action.invoke",
                    params,
                    tag,
                    Instant::now(),
                    TRAY_COLD_WAIT,
                )
            });
        }))
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

    /// Bound keys (§4.6): each module with one starts when it isn't running.
    fn keys_bound(&mut self, modules: &[String]) {
        for m in modules {
            self.hub.act(|r| r.key_bound(m));
        }
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
            registered_version: None,
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

    /// A bound key (#991): the hotkey thread's call reaches the module over the router pipe as
    /// `action.invoke {action, source: "key"}`, with no panel host frame in between.
    #[test]
    fn a_bound_key_invokes_the_action_over_the_router_pipe() {
        const KEY_ID: &str = "foxglove.example-keys";
        let mut router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
        router.add_module(ModuleEntry {
            id: KEY_ID.into(),
            name: "Example Keys".into(),
            holder: Holder::FirstParty,
            granted: vec![],
            catalog_provide: None,
            channel_risk: Default::default(),
            activate_on_call: false,
            panels: None,
            settings_dir: String::new(),
            data_dir: String::new(),
            registered_version: None,
        });
        router.set_token(KEY_ID, "tok-keys");
        let (events, _rx) = mpsc::channel();
        let hub = Hub::new(router, events);
        let (_side, feed) = feed(hub.clone());
        let name = format!(r"\\.\pipe\hideout-key-invoke-test-{}", std::process::id());
        hub.serve_pipe(&name).expect("serve the test pipe");
        let pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("router pipe");
        let mut from_hub = BufReader::new(pipe.try_clone().unwrap());
        let mut read = move || {
            let mut line = String::new();
            from_hub.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        let mut to_hub = pipe;
        writeln!(
            to_hub,
            "{}",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "id": KEY_ID, "token": "tok-keys", "instance": "i1", "version": "0.1.0", "hub": "1.0",
            "api": {}, "client": {"name": "example-keys", "version": "0.1.0"}}})
        )
        .unwrap();
        assert!(read().get("result").is_some());
        let invoke = feed.key_invoker().expect("the hub feed has a key invoker");
        invoke(KEY_ID, "toggle");
        let call = read();
        assert_eq!(call["method"], "action.invoke");
        assert_eq!(call["params"], json!({"action": "toggle", "source": "key"}));
    }

    /// A press that never got an answer says so; busy (a double click) says nothing.
    #[test]
    fn a_tray_timeout_says_it_did_not_answer() {
        let n = tray_notice("x.y", "free-gpu", &json!({"code": 4, "message": "timeout"}))
            .expect("a notice");
        assert!(n.body.contains("didn't answer in time"), "{n:?}");
        assert!(!n.title.ends_with("didn't start"), "{n:?}");
        assert!(tray_notice("x.y", "free-gpu", &json!({"code": 3, "message": "busy"})).is_none());
    }

    /// A tray menu quick action (#1234), end to end over the router pipe: the press reaches the
    /// running module as `action.invoke {action, source: "tray"}`; its success says nothing; its
    /// refusal becomes one notification with the module's reason and Open <Module>; game mode
    /// presses nothing. No toast is shown: the notifier is the test's.
    #[test]
    fn a_tray_press_reaches_the_module_and_a_refusal_is_told_once() {
        use hideout_contract::module::{Context, Module};
        const VULPES: &str = "vulpes";
        let manifest = Module::parse(
            include_str!("../../hideout-panels/tests/vectors/vulpes.module.toml"),
            &Context::default(),
        )
        .expect("Arete's manifest");
        let mut router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
        router.add_module(ModuleEntry {
            id: VULPES.into(),
            name: "AI".into(),
            holder: Holder::FirstParty,
            granted: vec![],
            catalog_provide: None,
            channel_risk: Default::default(),
            activate_on_call: false,
            panels: None,
            settings_dir: String::new(),
            data_dir: String::new(),
            registered_version: None,
        });
        router.set_token(VULPES, "tok-tray");
        let (events, rx) = mpsc::channel();
        let hub = Hub::new(router, events);
        let (notes_tx, notes) = mpsc::channel::<Notice>();
        let notes_tx = Mutex::new(notes_tx);
        let (side, feed) = feed_with(
            hub.clone(),
            Arc::new(move |n| {
                let _ = notes_tx.lock().unwrap().send(n);
            }),
        );
        side.manifest(manifest);
        std::thread::spawn(move || {
            for o in rx {
                side.on_router(&o);
            }
        });
        let name = format!(r"\\.\pipe\hideout-tray-press-test-{}", std::process::id());
        hub.serve_pipe(&name).expect("serve the test pipe");
        let pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("router pipe");
        let mut from_hub = BufReader::new(pipe.try_clone().unwrap());
        let mut to_hub = pipe;
        writeln!(
            to_hub,
            "{}",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "id": VULPES, "token": "tok-tray", "instance": "i1", "version": "0.1.0", "hub": "1.0",
            "api": {}, "client": {"name": "arete", "version": "0.1.0"}}})
        )
        .unwrap();
        // One synchronous pipe handle serialises its reads and writes, so reads happen only
        // when a line is due; the order of the lines proves nothing came in between.
        let mut read = move || {
            let mut line = String::new();
            from_hub.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        assert!(read()["result"].is_object());
        // A press waits for `module.ready` (review of #1240), so the module says it.
        writeln!(
            to_hub,
            "{}",
            json!({"jsonrpc": "2.0", "method": "module.ready", "params": {"instance": "i1"}})
        )
        .unwrap();
        let wait = Duration::from_millis(500);
        let press = feed
            .tray_invoker()
            .expect("the hub feed has a tray invoker");

        // Game mode alone sends nothing: the next line is the press's call.
        hub.act(|r| r.publish("keyboard.game_mode", json!(true)));
        hub.act(|r| r.publish("keyboard.game_mode", json!(false)));

        // A press: one call with the tray source; success says nothing.
        press(VULPES, "free-gpu");
        let call = read();
        assert_eq!(call["method"], "action.invoke", "game mode sent {call}");
        assert_eq!(
            call["params"],
            json!({"action": "free-gpu", "source": "tray"})
        );
        writeln!(
            to_hub,
            "{}",
            json!({"jsonrpc": "2.0", "id": call["id"], "result": {}})
        )
        .unwrap();
        assert!(notes.recv_timeout(wait).is_err(), "success is silent");

        // A refusal: one notification with the module's reason.
        press(VULPES, "free-gpu");
        let call = read();
        assert_eq!(
            call["params"]["source"], "tray",
            "exactly one call per press"
        );
        writeln!(
            to_hub,
            "{}",
            json!({"jsonrpc": "2.0", "id": call["id"], "error": {"code": 6, "message": "refused",
                "data": {"reason": "The chat job didn't stop."}}})
        )
        .unwrap();
        let n = notes
            .recv_timeout(Duration::from_secs(5))
            .expect("one notice");
        assert_eq!(n.title, "Couldn't free GPU");
        assert_eq!(n.body, "The chat job didn't stop.");
        assert_eq!((n.button.as_str(), n.module.as_str()), ("Open AI", VULPES));
        assert!(notes.recv_timeout(wait).is_err(), "exactly one notice");

        // A press for a module the hub doesn't know: told it didn't start, nothing sent.
        press("nobody.here", "free-gpu");
        let n = notes
            .recv_timeout(Duration::from_secs(5))
            .expect("a notice");
        assert!(n.title.ends_with("didn't start"), "{n:?}");
    }

    /// The Free GPU key (#1121, H4.7), end to end from the bindings to the router pipe: a key bound
    /// to Vulpes's declared `free-gpu` reaches Vulpes as `action.invoke {action, source: "key"}`; a
    /// key nobody bound sends nothing; and `keyboard.game_mode` being true changes nothing about
    /// that (game mode alone acts on nothing, §8.9). The press is the hotkey thread's dispatch
    /// (a bound chord looked up in `bound_keys`, then the feed's invoker); no window, no OS hook.
    #[test]
    fn a_key_bound_to_free_gpu_sends_the_action_and_game_mode_changes_nothing() {
        use hideout_contract::module::{Context, Module};
        const VULPES: &str = "vulpes";
        let manifest = Module::parse(
            include_str!("../../hideout-panels/tests/vectors/vulpes.module.toml"),
            &Context::default(),
        )
        .expect("Arete's manifest parses");
        assert!(manifest.actions.iter().any(|a| a.decl.id == "free-gpu"));
        let mut host = PanelHost::default();
        host.set_manifest(manifest);
        host.set_key(VULPES, "free-gpu", Some("F13".into()));
        let bound = host.bound_keys();
        assert_eq!(
            bound,
            [(
                VULPES.to_string(),
                "free-gpu".to_string(),
                "F13".to_string()
            )]
        );

        let mut router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
        router.add_module(ModuleEntry {
            id: VULPES.into(),
            name: "AI".into(),
            holder: Holder::FirstParty,
            granted: vec![],
            catalog_provide: None,
            channel_risk: Default::default(),
            activate_on_call: false,
            panels: None,
            settings_dir: String::new(),
            data_dir: String::new(),
            registered_version: None,
        });
        router.set_token(VULPES, "tok-vulpes");
        let (events, _rx) = mpsc::channel();
        let hub = Hub::new(router, events);
        let (_side, feed) = feed(hub.clone());
        let name = format!(r"\\.\pipe\hideout-free-gpu-key-test-{}", std::process::id());
        hub.serve_pipe(&name).expect("serve the test pipe");
        let pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("router pipe");
        let mut from_hub = BufReader::new(pipe.try_clone().unwrap());
        let mut to_hub = pipe;
        writeln!(
            to_hub,
            "{}",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "id": VULPES, "token": "tok-vulpes", "instance": "i1", "version": "0.1.0", "hub": "1.0",
            "api": {}, "client": {"name": "arete", "version": "0.1.0"}}})
        )
        .unwrap();
        let mut line = String::new();
        from_hub.read_line(&mut line).unwrap();
        assert!(serde_json::from_str::<Value>(&line).unwrap()["result"].is_object());

        let invoke = feed.key_invoker().expect("the hub feed has a key invoker");
        // What the hotkey thread does for a pressed chord: the binding for it, else nothing.
        let press = |chord: &str| {
            for (m, a, _) in bound.iter().filter(|(_, _, k)| k == chord) {
                invoke(m, a);
            }
        };
        let expect = json!({"action": "free-gpu", "source": "key"});
        // Lines from the pipe, read on a thread so "nothing arrived" can be told from "not yet".
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut line = String::new();
            if from_hub.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if tx
                .send(serde_json::from_str::<Value>(&line).unwrap())
                .is_err()
            {
                break;
            }
        });
        let wait = Duration::from_millis(500);
        // An unbound key sends nothing; the bound one sends exactly one call.
        press("F14");
        assert!(
            lines.recv_timeout(wait).is_err(),
            "an unbound key sends nothing"
        );
        press("F13");
        let call = lines.recv_timeout(wait).expect("the bound key's call");
        assert_eq!(call["method"], "action.invoke");
        assert_eq!(call["params"], expect);
        assert!(lines.recv_timeout(wait).is_err(), "exactly one call");
        // Game mode true (published into the hub as the link does): the same key, the same call.
        hub.act(|r| r.publish("keyboard.game_mode", json!(true)));
        press("F14");
        assert!(
            lines.recv_timeout(wait).is_err(),
            "game mode: an unbound key still sends nothing"
        );
        press("F13");
        let call = lines
            .recv_timeout(wait)
            .expect("the bound key's call in game mode");
        assert_eq!(call["method"], "action.invoke");
        assert_eq!(call["params"], expect, "game mode alone changes nothing");
        assert!(lines.recv_timeout(wait).is_err(), "exactly one call");
    }
}
