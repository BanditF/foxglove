// SPDX-License-Identifier: MIT
//! The router's side of the panel host's feed (§5, §8.2 v1.4): what modules register and patch,
//! the facts the panels need (module states, `when` facts, bindings) and the answers to the
//! hub's calls go to the shell; `ui.event`, `ui.visibility`, `action.invoke`, `settings.get`
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
use hideout_shell::hideout_panels::{HubCall, Inbound, Outbound, PanelFeed};
use serde_json::{json, Value};

/// What a call's tag stands for, to route its answer.
#[derive(Debug)]
enum Tag {
    Event(u64),
    Invoke { module: String, action: String },
    SettingsGet(String),
    SettingsSet(String),
}

/// Tags start high so they never meet the supervisor's.
const FIRST_TAG: u64 = 1 << 40;

#[derive(Clone)]
struct Shared {
    tx: mpsc::Sender<Inbound>,
    waker: Arc<Mutex<Option<Waker>>>,
    tags: Arc<Mutex<HashMap<u64, Tag>>>,
}

impl Shared {
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
}

/// A feed pair over `hub`.
pub fn feed(hub: Hub) -> (RouterSide, HubFeed) {
    let (tx, rx) = mpsc::channel();
    let shared = Shared {
        tx,
        waker: Arc::new(Mutex::new(None)),
        tags: Arc::new(Mutex::new(HashMap::new())),
    };
    (
        RouterSide(shared.clone()),
        HubFeed {
            hub,
            rx,
            shared,
            next: AtomicU64::new(FIRST_TAG),
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
        self.0.push(Inbound::Manifest(Box::new(m)));
    }

    /// One router event (the supervisor has seen it too).
    pub fn on_router(&self, out: &Out) {
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
            Out::Fact { key, value } => Inbound::Fact {
                name: key.clone(),
                value: value.clone(),
            },
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
            // The router answered `ui.register` itself; `ui.asset.put` isn't routed yet.
            Outbound::Registered { .. } | Outbound::Asset { .. } => {}
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
}
