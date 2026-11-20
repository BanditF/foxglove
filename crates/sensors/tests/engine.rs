//! The sampling rule against fake sources and a fake clock: no hardware, no timers, no windows.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use foxglove_sensors::nvml::{self, Backend, NvmlSource};
use foxglove_sensors::{
    pattern_wants, write_permission, Config, Engine, Group, Metric, Reading, Sink, Source,
    SourceError, SourceFactory, ValueOut, NAMESPACE,
};

#[derive(Default)]
struct Counters {
    made: u32,
    dropped: u32,
    reads: u32,
}

#[derive(Clone, Default)]
struct Facts(Rc<RefCell<BTreeMap<String, ValueOut>>>);

impl Sink for Facts {
    fn set(&mut self, key: &str, value: ValueOut) {
        assert!(
            key.starts_with("sensor."),
            "wrote outside the namespace: {key}"
        );
        self.0.borrow_mut().insert(key.into(), value);
    }
    fn withdraw(&mut self, key: &str) {
        self.0.borrow_mut().remove(key);
    }
}

impl Facts {
    fn get(&self, key: &str) -> Option<ValueOut> {
        self.0.borrow().get(key).cloned()
    }
    fn state(&self, g: Group) -> Option<&'static str> {
        match self.get(&g.state_key()) {
            Some(ValueOut::Text(t)) => Some(t),
            _ => None,
        }
    }
    fn keys(&self) -> Vec<String> {
        self.0.borrow().keys().cloned().collect()
    }
}

type Answer = Rc<RefCell<Result<Reading, SourceError>>>;

/// A source that answers from a script, repeating the last answer.
struct Fake {
    counters: Rc<RefCell<Counters>>,
    answer: Answer,
}

impl Source for Fake {
    fn read(&mut self) -> Result<Reading, SourceError> {
        self.counters.borrow_mut().reads += 1;
        self.answer.borrow().clone()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.counters.borrow_mut().dropped += 1;
    }
}

fn full() -> Reading {
    Reading::new()
        .with(Metric::CpuLoad, 12)
        .with(Metric::GpuLoad, 34)
        .with(Metric::RamUsed, 56)
        .with(Metric::GpuTemp, 61)
        .with(Metric::GpuClock, 1800)
        .with(Metric::GpuPower, 95)
        .with(Metric::GpuFan, 2100)
}

struct Rig {
    engine: Engine<Facts>,
    facts: Facts,
    counters: Rc<RefCell<Counters>>,
    answer: Answer,
}

fn rig(config: Config) -> Rig {
    let counters = Rc::new(RefCell::new(Counters::default()));
    let answer: Answer = Rc::new(RefCell::new(Ok(full())));
    let (c, a) = (counters.clone(), answer.clone());
    let factory: SourceFactory = Box::new(move || {
        c.borrow_mut().made += 1;
        Box::new(Fake {
            counters: c.clone(),
            answer: a.clone(),
        })
    });
    let facts = Facts::default();
    let engine = Engine::new(vec![factory], facts.clone(), config);
    Rig {
        engine,
        facts,
        counters,
        answer,
    }
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

impl Rig {
    fn reads(&self) -> u32 {
        self.counters.borrow().reads
    }
    /// Runs the fake clock from `from` to `to` seconds, polling each second.
    fn run(&mut self, from: u64, to: u64) {
        for t in from..=to {
            self.engine.poll(secs(t));
        }
    }
}

#[test]
fn first_subscribe_starts_sampling_and_idle_has_no_timer() {
    let mut r = rig(Config::default());
    r.run(0, 5);
    assert_eq!(r.reads(), 0);
    assert_eq!(r.counters.borrow().made, 0, "no sampler exists while idle");
    assert_eq!(r.engine.next_due(), None, "no timer while idle");
    assert!(r.facts.keys().is_empty());

    r.engine.subscribe("sensor.*", secs(5)).unwrap();
    assert!(r.engine.next_due().is_some());
    r.engine.poll(secs(5));
    assert!(r.reads() > 0);
}

#[test]
fn last_unsubscribe_stops_and_releases() {
    let mut r = rig(Config::default());
    let a = r.engine.subscribe("sensor.*", secs(0)).unwrap();
    let b = r.engine.subscribe("sensor.gpu.*", secs(0)).unwrap();
    r.run(0, 2);
    r.engine.unsubscribe(a, secs(2));
    r.run(3, 4);
    assert_eq!(r.reads(), 5, "still sampling for the second subscriber");
    r.engine.unsubscribe(b, secs(4));
    let after = r.reads();
    r.run(5, 20);
    assert_eq!(r.reads(), after);
    assert_eq!(r.engine.next_due(), None);
    assert_eq!(r.counters.borrow().dropped, 1, "the source is released");
    assert!(r.facts.keys().is_empty(), "nothing is left published");
}

#[test]
fn other_patterns_do_not_start_it_and_resubscribe_restarts() {
    let mut r = rig(Config::default());
    assert!(r.engine.subscribe("focus.*", secs(0)).is_none());
    assert!(r.engine.subscribe("sensors.*", secs(0)).is_none());
    r.run(0, 3);
    assert_eq!(r.reads(), 0);
    assert!(pattern_wants("sensor.gpu.*"));
    assert!(pattern_wants("sensor.gpu.temp_c"));
    assert!(pattern_wants("*"));

    let id = r.engine.subscribe("sensor.gpu.*", secs(3)).unwrap();
    r.run(3, 4);
    r.engine.unsubscribe(id, secs(4));
    r.engine.subscribe("sensor.*", secs(10)).unwrap();
    r.run(10, 11);
    assert_eq!(r.counters.borrow().made, 2);
    assert_eq!(
        r.counters.borrow().dropped,
        1,
        "one live source, none leaked"
    );
}

#[test]
fn default_rate_is_one_hertz() {
    let mut r = rig(Config::default());
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    r.run(0, 10);
    assert!((10..=11).contains(&r.reads()), "{}", r.reads());
    // Polling faster than the interval does not read faster.
    let before = r.reads();
    for ms in 1..10 {
        r.engine.poll(secs(10) + Duration::from_millis(ms * 50));
    }
    assert_eq!(r.reads(), before);
}

#[test]
fn key_names_and_units_are_exact() {
    let mut r = rig(Config::default());
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    r.engine.poll(secs(0));
    let expected = [
        "sensor.cpu.load_pct",
        "sensor.cpu.state",
        "sensor.gpu.clock_mhz",
        "sensor.gpu.fan_rpm",
        "sensor.gpu.load_pct",
        "sensor.gpu.power_w",
        "sensor.gpu.state",
        "sensor.gpu.temp_c",
        "sensor.ram.state",
        "sensor.ram.used_pct",
    ];
    assert_eq!(r.facts.keys(), expected);
    assert_eq!(r.facts.get("sensor.gpu.temp_c"), Some(ValueOut::Int(61)));
    assert_eq!(r.facts.state(Group::Gpu), Some("ok"));
    let units: Vec<_> = Metric::ALL.iter().map(|m| m.unit()).collect();
    assert_eq!(units, ["%", "%", "%", "C", "MHz", "W", "RPM"]);
    assert!(
        !r.facts.keys().iter().any(|k| k.contains("cpu.temp")),
        "no CPU temperature without a source for it"
    );
}

#[test]
fn the_namespace_comes_from_one_constant() {
    assert_eq!(NAMESPACE, "sensor");
    assert_eq!(write_permission(), "facts.write:sensor.*");
    for m in Metric::ALL {
        assert!(m.key().starts_with(&format!("{NAMESPACE}.")), "{}", m.key());
    }
    for g in Group::ALL {
        assert!(g.state_key().starts_with(&format!("{NAMESPACE}.")));
    }
    // The literal appears in code only in this crate's definition of the constant.
    let dir = format!("{}/src", env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let literal = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains("\"sensor"))
            .count();
        let allowed = usize::from(path.file_name().unwrap() == "lib.rs");
        assert_eq!(literal, allowed, "{}", path.display());
    }
}

#[test]
fn a_failed_read_is_unavailable_not_zero_and_recovers() {
    let mut r = rig(Config::default());
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    r.engine.poll(secs(0));
    assert_eq!(r.facts.state(Group::Gpu), Some("ok"));

    *r.answer.borrow_mut() = Err(SourceError("gone".into()));
    r.engine.poll(secs(1));
    for g in Group::ALL {
        assert_eq!(r.facts.state(g), Some("unavailable"));
    }
    for m in Metric::ALL {
        assert_eq!(
            r.facts.get(&m.key()),
            None,
            "{} must be absent, not 0",
            m.key()
        );
    }

    *r.answer.borrow_mut() = Ok(full());
    r.engine.poll(secs(2));
    assert_eq!(r.facts.state(Group::Gpu), Some("ok"));
    assert_eq!(r.facts.get("sensor.gpu.temp_c"), Some(ValueOut::Int(61)));
}

#[test]
fn a_partial_reading_publishes_what_it_has() {
    let mut r = rig(Config::default());
    *r.answer.borrow_mut() = Ok(Reading::new()
        .with(Metric::GpuTemp, 50)
        .with(Metric::RamUsed, 40));
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    r.engine.poll(secs(0));
    assert_eq!(r.facts.get("sensor.gpu.temp_c"), Some(ValueOut::Int(50)));
    assert_eq!(r.facts.get("sensor.gpu.fan_rpm"), None);
    assert_eq!(r.facts.state(Group::Gpu), Some("ok"));
    assert_eq!(r.facts.state(Group::Ram), Some("ok"));
    assert_eq!(r.facts.state(Group::Cpu), Some("unavailable"));
}

#[test]
fn state_is_unknown_before_the_first_read() {
    let mut r = rig(Config::default());
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    for g in Group::ALL {
        assert_eq!(r.facts.state(g), Some("unknown"));
    }
    assert_eq!(r.facts.get("sensor.gpu.temp_c"), None);
}

#[test]
fn game_mode_stops_sampling_and_leaves_nothing_stale() {
    let mut r = rig(Config::default());
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    r.run(0, 3);
    let before = r.reads();
    r.engine.set_game_mode(true, secs(3));
    r.run(4, 30);
    assert_eq!(r.reads(), before, "no reads during a game");
    assert_eq!(r.engine.next_due(), None);
    assert_eq!(r.counters.borrow().dropped, 1, "the sampler is released");
    assert_eq!(r.facts.get("sensor.gpu.temp_c"), None, "no frozen numbers");
    for g in Group::ALL {
        assert_eq!(r.facts.state(g), Some("unknown"), "never a stale ok");
    }
    // A new subscriber during the game does not wake it.
    r.engine.subscribe("sensor.cpu.*", secs(31)).unwrap();
    r.run(31, 33);
    assert_eq!(r.reads(), before);

    r.engine.set_game_mode(false, secs(40));
    r.run(40, 42);
    assert!(r.reads() > before);
    assert_eq!(r.facts.state(Group::Gpu), Some("ok"));
}

#[test]
fn game_mode_can_throttle_instead() {
    let mut r = rig(Config {
        interval: secs(1),
        game_interval: Some(secs(10)),
    });
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    r.run(0, 1);
    r.engine.set_game_mode(true, secs(1));
    let before = r.reads();
    r.run(2, 31);
    let during = r.reads() - before;
    assert!((2..=4).contains(&during), "{during}");
    r.engine.set_game_mode(false, secs(31));
    let before = r.reads();
    r.run(32, 36);
    assert!(r.reads() - before >= 4, "back to 1 Hz");
}

#[test]
fn a_pdh_less_pc_still_reports_memory() {
    let mut r = rig(Config::default());
    *r.answer.borrow_mut() = Ok(Reading::new().with(Metric::RamUsed, 70));
    r.engine.subscribe("sensor.*", secs(0)).unwrap();
    r.engine.poll(secs(0));
    assert_eq!(r.facts.state(Group::Cpu), Some("unavailable"));
    assert_eq!(r.facts.state(Group::Ram), Some("ok"));
    assert_eq!(r.facts.get("sensor.cpu.load_pct"), None);
}

// NVML

struct FakeNvml {
    devices: Result<u32, String>,
    drops: Rc<RefCell<u32>>,
}

impl Backend for FakeNvml {
    fn device_count(&mut self) -> Result<u32, String> {
        self.devices.clone()
    }
    fn temperature_c(&mut self) -> Result<u32, String> {
        Ok(64)
    }
    fn graphics_clock_mhz(&mut self) -> Result<u32, String> {
        Ok(1950)
    }
    fn power_mw(&mut self) -> Result<u32, String> {
        Ok(215_000)
    }
    fn fan_rpm(&mut self) -> Result<u32, String> {
        Err("no RPM call".into())
    }
}

impl Drop for FakeNvml {
    fn drop(&mut self) {
        *self.drops.borrow_mut() += 1;
    }
}

#[test]
fn nvml_units_and_a_missing_fan() {
    let drops = Rc::new(RefCell::new(0));
    let mut s = NvmlSource::new(FakeNvml {
        devices: Ok(1),
        drops: drops.clone(),
    });
    let r = s.read().unwrap();
    assert_eq!(r.get(Metric::GpuPower), Some(215), "milliwatts to watts");
    assert_eq!(r.get(Metric::GpuTemp), Some(64));
    assert_eq!(r.get(Metric::GpuClock), Some(1950));
    assert_eq!(r.get(Metric::GpuFan), None, "no RPM: absent, not 0");
    drop(s);
    assert_eq!(*drops.borrow(), 1, "NVML is shut down with the source");
}

#[test]
fn nvml_without_a_gpu_or_after_an_error_is_an_error_not_zero() {
    let drops = Rc::new(RefCell::new(0));
    let mut none = NvmlSource::new(FakeNvml {
        devices: Ok(0),
        drops: drops.clone(),
    });
    assert!(none.read().is_err());
    let mut bad = NvmlSource::new(FakeNvml {
        devices: Err("driver not loaded".into()),
        drops,
    });
    assert!(bad.read().is_err());
}

#[test]
fn a_missing_nvml_dll_is_unavailable_all_the_way_to_the_facts() {
    assert!(NvmlSource::load("nvml-does-not-exist.dll").is_err());
    assert_eq!(nvml::DLL, "nvml.dll");

    let facts = Facts::default();
    let factory: SourceFactory =
        Box::new(|| nvml::source_or_unavailable("nvml-does-not-exist.dll"));
    let mut engine = Engine::new(vec![factory], facts.clone(), Config::default());
    engine.subscribe("sensor.*", secs(0)).unwrap();
    engine.poll(secs(0));
    assert_eq!(facts.state(Group::Gpu), Some("unavailable"));
    for m in [
        Metric::GpuTemp,
        Metric::GpuClock,
        Metric::GpuPower,
        Metric::GpuFan,
    ] {
        assert_eq!(facts.get(&m.key()), None);
    }
}

#[test]
fn the_docs_list_every_fact() {
    let path = format!("{}/../../docs/local-api.md", env!("CARGO_MANIFEST_DIR"));
    let docs = std::fs::read_to_string(path).unwrap();
    for m in Metric::ALL {
        assert!(
            docs.contains(&m.key()),
            "docs/local-api.md misses {}",
            m.key()
        );
    }
    for g in Group::ALL {
        assert!(
            docs.contains(&g.state_key()),
            "docs/local-api.md misses {}",
            g.state_key()
        );
    }
}
