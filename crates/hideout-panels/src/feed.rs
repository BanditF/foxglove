// SPDX-License-Identifier: MIT
//! How panels reach the hub's window. The router (H2.3) implements [`PanelFeed`]: it turns the
//! modules' `ui.register`, `ui.patch` and `ui.asset.put` into [`Inbound`] items, and sends the
//! [`Outbound`] answers and `ui.event` calls to the owning module. Nothing here polls: the feed
//! calls its waker when it has something, and the hub drains it on the frame that follows.
//!
//! [`channel`] is an in-process pair for tests and for a router that runs on its own thread.

use crate::model::{Outgoing, PanelHost};
use hideout_contract::error::Error;
use hideout_contract::messages::{UiAssetPut, UiBadge, UiPatch, UiVisibility};
use hideout_contract::panel::{Register, Rejected};
use serde_json::Value;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Calls the hub back (it requests a repaint) when the feed has something to drain.
pub type Waker = Box<dyn Fn() + Send + Sync>;

/// From the modules (through the router) to the panel host.
#[derive(Clone, Debug, PartialEq)]
pub enum Inbound {
    /// `ui.register` (a call: answer with [`Outbound::Registered`] and the same `call`).
    Register {
        call: u64,
        module: String,
        /// The module's name for people.
        name: String,
        /// The module's `[[ui.panels]]` ids, when the router knows its manifest.
        declared: Option<Vec<String>>,
        /// The module's `[[actions]]` ids, when known (`ui.action`, spec v1.4).
        actions: Option<Vec<String>>,
        params: Register,
    },
    /// `ui.patch` (a notification).
    Patch { module: String, patch: UiPatch },
    /// `ui.badge` (a notification, v1.3).
    Badge { module: String, badge: UiBadge },
    /// `ui.asset.put` (a call: answer with [`Outbound::Asset`]).
    AssetPut {
        call: u64,
        module: String,
        asset: UiAssetPut,
    },
    /// The module's answer to `ui.event` `seq`: `Err(data.reason)` when refused.
    EventResult {
        seq: u64,
        result: Result<(), String>,
    },
    /// A fact a panel binds changed (from `facts.changed`).
    Fact { name: String, value: Value },
    /// The module's instance went away; its panels grey with `state` (§7.8).
    ModuleGone { module: String, state: String },
}

/// From the panel host to the modules.
#[derive(Clone, Debug, PartialEq)]
pub enum Outbound {
    /// The `ui.register` result `{rejected}`.
    Registered {
        call: u64,
        module: String,
        rejected: Vec<Rejected>,
    },
    /// The `ui.asset.put` result: `{}` or an error.
    Asset {
        call: u64,
        module: String,
        result: Result<(), Error>,
    },
    /// A `ui.event` call for the owning module only.
    Event(Outgoing),
    /// A `ui.visibility` notification for the owning module (v1.3), sent on change only.
    Visibility {
        module: String,
        visibility: UiVisibility,
    },
}

/// What the router implements for the hub.
pub trait PanelFeed: Send {
    /// Everything that arrived since the last call. Never blocks.
    fn drain(&mut self) -> Vec<Inbound>;
    fn send(&mut self, out: Outbound);
    /// The feed calls `waker` whenever new items arrive.
    fn set_waker(&mut self, waker: Waker);
}

impl PanelHost {
    /// Applies what `feed` has, answers its calls, sends input out, and flushes coalesced
    /// patches. Returns when it needs to run again for a held-back patch (the hub schedules one
    /// repaint then, not a timer loop). `true` in the pair means something changed on screen.
    pub fn pump(&mut self, feed: &mut dyn PanelFeed, now: Instant) -> (bool, Option<Instant>) {
        let mut changed = false;
        for item in feed.drain() {
            match item {
                Inbound::Register {
                    call,
                    module,
                    name,
                    declared,
                    actions,
                    params,
                } => {
                    let rejected = self.register_with(
                        &module,
                        &name,
                        declared.as_deref(),
                        actions.as_deref(),
                        &params,
                    );
                    for r in &rejected {
                        log::warn!("{module}: panel {} rejected: {}", r.panel, r.error);
                    }
                    feed.send(Outbound::Registered {
                        call,
                        module,
                        rejected,
                    });
                    changed = true;
                }
                Inbound::Patch { module, patch } => {
                    if let Err(e) = self.patch(&module, &patch, now) {
                        log::warn!("{module}: ui.patch on {} refused: {e}", patch.panel);
                    }
                    changed = true;
                }
                Inbound::Badge { module, badge } => {
                    self.set_badge(&module, &badge);
                    changed = true;
                }
                Inbound::AssetPut {
                    call,
                    module,
                    asset,
                } => {
                    let result = self.assets.put(&asset);
                    changed |= result.is_ok();
                    feed.send(Outbound::Asset {
                        call,
                        module,
                        result,
                    });
                }
                Inbound::EventResult { seq, result } => {
                    self.event_result(seq, result);
                    changed = true;
                }
                Inbound::Fact { name, value } => changed |= self.set_fact(&name, value),
                Inbound::ModuleGone { module, state } => {
                    self.module_gone(&module, &state);
                    changed = true;
                }
            }
        }
        for e in self.take_events() {
            feed.send(Outbound::Event(e));
        }
        for (module, visibility) in self.take_visibility() {
            feed.send(Outbound::Visibility { module, visibility });
        }
        (changed, self.flush(now))
    }
}

/// The router's end of [`channel`].
pub struct RouterEnd {
    tx: mpsc::Sender<Inbound>,
    rx: mpsc::Receiver<Outbound>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl RouterEnd {
    /// Hands an item to the hub and wakes it.
    pub fn push(&self, item: Inbound) {
        if self.tx.send(item).is_ok() {
            if let Ok(w) = self.waker.lock() {
                if let Some(w) = w.as_ref() {
                    w();
                }
            }
        }
    }

    /// What the hub sent, without blocking.
    pub fn take(&self) -> Vec<Outbound> {
        self.rx.try_iter().collect()
    }
}

/// The hub's end of [`channel`].
pub struct ChannelFeed {
    rx: mpsc::Receiver<Inbound>,
    tx: mpsc::Sender<Outbound>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl PanelFeed for ChannelFeed {
    fn drain(&mut self) -> Vec<Inbound> {
        self.rx.try_iter().collect()
    }

    fn send(&mut self, out: Outbound) {
        let _ = self.tx.send(out);
    }

    fn set_waker(&mut self, waker: Waker) {
        if let Ok(mut w) = self.waker.lock() {
            *w = Some(waker);
        }
    }
}

/// An in-process feed: the router keeps [`RouterEnd`], the hub takes [`ChannelFeed`].
pub fn channel() -> (RouterEnd, ChannelFeed) {
    let (in_tx, in_rx) = mpsc::channel();
    let (out_tx, out_rx) = mpsc::channel();
    let waker = Arc::new(Mutex::new(None));
    (
        RouterEnd {
            tx: in_tx,
            rx: out_rx,
            waker: waker.clone(),
        },
        ChannelFeed {
            rx: in_rx,
            tx: out_tx,
            waker,
        },
    )
}
