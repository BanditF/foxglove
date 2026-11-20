//! What the program does with the hub's messages, with no pipe and no clock of its own (the
//! binary owns both): `facts.demand` starts and stops sampling (contract §8.8, v1.20), the
//! `keyboard.game_mode` fact slows it, and every value the engine publishes comes out as a
//! write for the binary to send as `facts.set`. A key outside the namespace never gets this far
//! ([`GuardedSink`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::{pattern_wants, Config, Engine, GuardedSink, Sink, SourceFactory, SubId, ValueOut};

/// The fact that says a game runs (Foxglove's keyboard module).
pub const GAME_MODE_FACT: &str = "keyboard.game_mode";

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
}

impl Host {
    pub fn new(factories: Vec<SourceFactory>, config: Config) -> Host {
        let out = Outbox::default();
        Host {
            engine: Engine::new(factories, GuardedSink::new(out.clone()), config),
            out,
            sub: None,
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
        match (subscribers > 0, self.sub) {
            (true, None) => self.sub = self.engine.subscribe(pattern, now),
            (false, Some(id)) => {
                self.engine.unsubscribe(id, now);
                self.sub = None;
            }
            _ => {}
        }
    }

    /// `keyboard.game_mode` changed (`false` when it is missing or not on).
    pub fn game_mode(&mut self, on: bool, now: Duration) {
        self.engine.set_game_mode(on, now);
    }

    /// The program ends: nothing stays published.
    pub fn stop(&mut self, now: Duration) {
        self.demand(&registered_pattern(), 0, now);
    }
}
