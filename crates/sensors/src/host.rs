//! What the program does with the hub's messages, with no pipe and no clock of its own (the
//! binary owns both): `facts.demand` starts and stops sampling (contract §8.8, v1.20), the
//! `keyboard.game_mode` fact slows it, the module's own page counts as a listener while it is on
//! screen (`ui.visibility`, contract §5.9: a hub `bind` creates no demand), and every value the engine publishes comes out as a
//! write for the binary to send as `facts.set`. A key outside the namespace never gets this far
//! ([`GuardedSink`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::{pattern_wants, Config, Engine, GuardedSink, Sink, SourceFactory, SubId, ValueOut};

/// The fact that says a game runs (Foxglove's keyboard module).
pub const GAME_MODE_FACT: &str = "keyboard.game_mode";

/// The id of the module's page panel (module.toml `[[ui.panels]]`).
pub const PAGE_PANEL: &str = "page";

/// The pattern the module registers and the hub's demand names.
pub fn registered_pattern() -> String {
    format!("{}.*", crate::NAMESPACE)
}

/// One write the engine wants: `value` `None` withdraws the key (a `facts.set` of `null`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub key: String,
    pub value: Option<ValueOut>,
}

/// Collects the engine's writes until the binary sends them.
#[derive(Clone, Default)]
struct Outbox(Rc<RefCell<Vec<Write>>>);

impl Sink for Outbox {
    fn set(&mut self, key: &str, value: ValueOut) {
        self.0.borrow_mut().push(Write {
            key: key.into(),
            value: Some(value),
        });
    }
    fn withdraw(&mut self, key: &str) {
        self.0.borrow_mut().push(Write {
            key: key.into(),
            value: None,
        });
    }
}

pub struct Host {
    engine: Engine<GuardedSink<Outbox>>,
    out: Outbox,
    sub: Option<SubId>,
    /// The hub reports someone listening (`facts.demand` above 0).
    external: bool,
    /// The module's page is on screen (`ui.visibility visible`).
    page_visible: bool,
    /// Called each time sampling ends and every source is dropped (the binary trims memory).
    on_release: Option<Box<dyn FnMut()>>,
}

impl Host {
    pub fn new(factories: Vec<SourceFactory>, config: Config) -> Host {
        let out = Outbox::default();
        Host {
            engine: Engine::new(factories, GuardedSink::new(out.clone()), config),
            out,
            sub: None,
            external: false,
            page_visible: false,
            on_release: None,
        }
    }

    /// `f` runs right after sampling ends and every source has been dropped (demand 0, a game
    /// that stops sampling, the program's end), never while a source is open.
    pub fn on_release(&mut self, f: impl FnMut() + 'static) {
        self.on_release = Some(Box::new(f));
    }

    /// Runs `change`, then the release hook if it ended sampling.
    fn watching(&mut self, change: impl FnOnce(&mut Self)) {
        let was = self.engine.sampling();
        change(self);
        if was && !self.engine.sampling() {
            if let Some(f) = self.on_release.as_mut() {
                f();
            }
        }
    }

    /// A notification from the hub, as the binary's loop receives it: `facts.demand`,
    /// `ui.visibility` and `facts.changed` of [`GAME_MODE_FACT`]. Anything else changes nothing.
    pub fn notify(&mut self, method: &str, params: &serde_json::Value, now: Duration) {
        match method {
            "facts.demand" => {
                let n = params["subscribers"]
                    .as_u64()
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32;
                self.demand(params["pattern"].as_str().unwrap_or(""), n, now);
            }
            "ui.visibility" => self.visibility(
                params["panel"].as_str().unwrap_or(""),
                params["state"].as_str().unwrap_or(""),
                now,
            ),
            "facts.changed" if params["key"] == GAME_MODE_FACT => {
                self.game_mode(params["value"].as_bool() == Some(true), now)
            }
            _ => {}
        }
    }

    /// Whether anything is being read.
    pub fn sampling(&self) -> bool {
        self.engine.sampling()
    }

    /// The one deadline the binary needs a timer for; `None` means no timer at all.
    pub fn next_due(&self) -> Option<Duration> {
        self.engine.next_due()
    }

    /// The writes since the last call, in order.
    pub fn take_writes(&mut self) -> Vec<Write> {
        std::mem::take(&mut *self.out.0.borrow_mut())
    }

    /// Reads when a read is due.
    pub fn tick(&mut self, now: Duration) {
        self.engine.poll(now);
    }

    /// `facts.demand {pattern, subscribers}` (H → M): sampling runs while someone listens.
    /// Repeats and patterns outside the namespace change nothing.
    pub fn demand(&mut self, pattern: &str, subscribers: u32, now: Duration) {
        if !pattern_wants(pattern) {
            return;
        }
        self.watching(|h| {
            h.external = subscribers > 0;
            h.reconcile(now);
        });
    }

    /// `ui.visibility {panel, state}` (H → M): the page on screen is a listener, so sampling runs
    /// while it is `visible` and stops at once on `hidden` or `closed` unless an app still
    /// listens. Another panel id or state changes nothing.
    pub fn visibility(&mut self, panel: &str, state: &str, now: Duration) {
        if panel != PAGE_PANEL {
            return;
        }
        let visible = match state {
            "visible" => true,
            "hidden" | "closed" => false,
            _ => return,
        };
        self.watching(|h| {
            h.page_visible = visible;
            h.reconcile(now);
        });
    }

    /// Sampling follows `external || page_visible`; repeating the same listening state does
    /// nothing.
    fn reconcile(&mut self, now: Duration) {
        match (self.external || self.page_visible, self.sub) {
            (true, None) => self.sub = self.engine.subscribe(&registered_pattern(), now),
            (false, Some(id)) => {
                self.engine.unsubscribe(id, now);
                self.sub = None;
            }
            _ => {}
        }
    }

    /// `keyboard.game_mode` changed (`false` when it is missing or not on).
    pub fn game_mode(&mut self, on: bool, now: Duration) {
        self.watching(|h| h.engine.set_game_mode(on, now));
    }

    /// The program ends: nothing stays published.
    pub fn stop(&mut self, now: Duration) {
        self.watching(|h| {
            h.external = false;
            h.page_visible = false;
            h.reconcile(now);
        });
    }
}
