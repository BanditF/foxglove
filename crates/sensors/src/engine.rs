//! The sampling rule, with no thread and no timer: the host calls [`Engine::poll`] when
//! [`Engine::next_due`] says so. Time is a `Duration` the caller supplies (any fixed origin), so
//! tests drive a fake clock.

use std::collections::BTreeSet;
use std::time::Duration;

use crate::{pattern_wants, Group, Metric, Reading, Source, Zone, NAMESPACE};

/// A value on its way to the facts store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueOut {
    Int(i64),
    Text(&'static str),
}

/// Where the engine writes. Only `sensor.*` keys are ever passed.
pub trait Sink {
    fn set(&mut self, key: &str, value: ValueOut);
    fn withdraw(&mut self, key: &str);
}

/// A sink that refuses any key outside [`NAMESPACE`]: whatever sits behind it can't publish
/// anything else, whatever the caller asks.
pub struct GuardedSink<K: Sink> {
    inner: K,
    refused: u32,
}

impl<K: Sink> GuardedSink<K> {
    pub fn new(inner: K) -> Self {
        GuardedSink { inner, refused: 0 }
    }

    /// How many writes or withdrawals were refused.
    pub fn refused(&self) -> u32 {
        self.refused
    }

    pub fn inner(&self) -> &K {
        &self.inner
    }

    fn allowed(&mut self, key: &str) -> bool {
        let ok = key
            .strip_prefix(NAMESPACE)
            .is_some_and(|rest| rest.starts_with('.'));
        if !ok {
            self.refused += 1;
        }
        ok
    }
}

impl<K: Sink> Sink for GuardedSink<K> {
    fn set(&mut self, key: &str, value: ValueOut) {
        if self.allowed(key) {
            self.inner.set(key, value);
        }
    }
    fn withdraw(&mut self, key: &str) {
        if self.allowed(key) {
            self.inner.withdraw(key);
        }
    }
}

/// Makes a source when sampling starts; the source is dropped when it stops.
pub type SourceFactory = Box<dyn FnMut() -> Box<dyn Source>>;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Time between reads (1 s).
    pub interval: Duration,
    /// While game mode is on: `Some(d)` reads every `d` and the last values stay published (the
    /// default, 10 s); `None` stops sampling and withdraws every value.
    pub game_interval: Option<Duration>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            interval: Duration::from_secs(1),
            game_interval: Some(Duration::from_secs(10)),
        }
    }
}

/// One subscription, from [`Engine::subscribe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubId(u64);

pub struct Engine<K: Sink> {
    factories: Vec<SourceFactory>,
    sink: K,
    config: Config,
    subs: Vec<SubId>,
    next_id: u64,
    game_mode: bool,
    /// The live sources: empty unless sampling.
    sources: Vec<Box<dyn Source>>,
    next_due: Option<Duration>,
    published: BTreeSet<String>,
    /// The thermal zones of the last read (page details); empty unless sampling.
    zones: Vec<Zone>,
}

impl<K: Sink> Engine<K> {
    pub fn new(factories: Vec<SourceFactory>, sink: K, config: Config) -> Self {
        Engine {
            factories,
            sink,
            config,
            subs: Vec::new(),
            next_id: 0,
            game_mode: false,
            sources: Vec::new(),
            next_due: None,
            published: BTreeSet::new(),
            zones: Vec::new(),
        }
    }

    pub fn sink(&self) -> &K {
        &self.sink
    }

    /// The thermal zones of the last read; empty while not sampling.
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    /// Whether any source exists (a sampler is open).
    pub fn sampling(&self) -> bool {
        !self.sources.is_empty()
    }

    /// The only deadline the host needs a timer for; `None` means no timer at all.
    pub fn next_due(&self) -> Option<Duration> {
        self.next_due
    }

    /// A `facts.subscribe pattern`. `None` when the pattern doesn't reach this namespace.
    /// The first one starts sampling.
    pub fn subscribe(&mut self, pattern: &str, now: Duration) -> Option<SubId> {
        if !pattern_wants(pattern) {
            return None;
        }
        let id = SubId(self.next_id);
        self.next_id += 1;
        self.subs.push(id);
        self.reconcile(now);
        Some(id)
    }

    /// A `facts.unsubscribe`. The last one stops sampling and withdraws everything.
    pub fn unsubscribe(&mut self, id: SubId, now: Duration) {
        self.subs.retain(|s| *s != id);
        self.reconcile(now);
    }

    /// The `keyboard.game_mode` fact changed.
    pub fn set_game_mode(&mut self, on: bool, now: Duration) {
        if self.game_mode != on {
            self.game_mode = on;
            let was_sampling = self.sampling();
            self.reconcile(now);
            if was_sampling && self.sampling() {
                // Still sampling, at the other rate: the next read follows it.
                self.next_due = Some(now + self.interval());
            }
        }
    }

    /// Reads when a read is due.
    pub fn poll(&mut self, now: Duration) {
        if !self.sampling() || self.next_due.is_none_or(|due| now < due) {
            return;
        }
        let readings: Vec<Reading> = self
            .sources
            .iter_mut()
            .filter_map(|s| s.read().ok())
            .collect();
        self.publish(&readings);
        self.next_due = Some(now + self.interval());
    }

    fn interval(&self) -> Duration {
        match (self.game_mode, self.config.game_interval) {
            (true, Some(slow)) => slow,
            _ => self.config.interval,
        }
    }

    /// Brings the sources in line with who subscribes and whether a game runs.
    fn reconcile(&mut self, now: Duration) {
        let wanted =
            !self.subs.is_empty() && (!self.game_mode || self.config.game_interval.is_some());
        match (wanted, self.sampling()) {
            (true, false) => {
                self.sources = self.factories.iter_mut().map(|f| f()).collect();
                self.publish_unknown();
                self.next_due = Some(now);
            }
            (true, true) => {}
            (false, true) => {
                // Dropping the sources is the release: `PdhSampler` closes its query and `Nvml`
                // calls `nvmlShutdown` and frees the DLL (tests: `last_unsubscribe_stops_and_
                // releases`, `other_patterns_do_not_start_it_and_resubscribe_restarts`). What Windows itself keeps after that (PDH's
                // loaded perf providers, their handles and pages) is not ours to free.
                self.sources.clear();
                self.next_due = None;
                if self.subs.is_empty() {
                    self.withdraw_all();
                } else {
                    // Held for a game: nothing stale stays `ok`.
                    self.withdraw_all();
                    self.publish_unknown();
                }
            }
            (false, false) => {
                // A subscriber arriving during a game that stops sampling hears `unknown`.
                if !self.subs.is_empty() && self.published.is_empty() {
                    self.publish_unknown();
                }
                if self.subs.is_empty() {
                    self.withdraw_all();
                }
            }
        }
    }

    fn publish_unknown(&mut self) {
        for group in Group::ALL {
            self.put(group.state_key(), ValueOut::Text("unknown"));
        }
    }

    fn publish(&mut self, readings: &[Reading]) {
        self.zones = readings
            .iter()
            .flat_map(|r| r.zones().iter().cloned())
            .collect();
        for metric in Metric::ALL {
            let value = readings.iter().find_map(|r| r.get(metric));
            match value {
                Some(v) => self.put(metric.key(), ValueOut::Int(v)),
                None => self.take(&metric.key()),
            }
        }
        for group in Group::ALL {
            let any = Metric::ALL
                .iter()
                .filter(|m| m.group() == group)
                .any(|m| readings.iter().any(|r| r.get(*m).is_some()));
            let state = if any { "ok" } else { "unavailable" };
            self.put(group.state_key(), ValueOut::Text(state));
        }
    }

    fn put(&mut self, key: String, value: ValueOut) {
        debug_assert!(key.starts_with(&format!("{NAMESPACE}.")));
        self.sink.set(&key, value);
        self.published.insert(key);
    }

    fn take(&mut self, key: &str) {
        if self.published.remove(key) {
            self.sink.withdraw(key);
        }
    }

    fn withdraw_all(&mut self) {
        self.zones.clear();
        for key in std::mem::take(&mut self.published) {
            self.sink.withdraw(&key);
        }
    }
}
