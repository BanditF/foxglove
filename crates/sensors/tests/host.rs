//! The host rules with no pipe and no clock: sampling follows the hub's demand (§8.8), a game
//! slows it, and nothing leaves the `sensor.*` namespace.

use std::time::Duration;

use foxglove_sensors::host::{registered_pattern, Host, Write};
use foxglove_sensors::{Config, Metric, Reading, Source, SourceError, SourceFactory};
use std::cell::Cell;
use std::rc::Rc;

struct Counting(Rc<Cell<u32>>);

impl Source for Counting {
    fn read(&mut self) -> Result<Reading, SourceError> {
        self.0.set(self.0.get() + 1);
        Ok(Reading::new().with(Metric::CpuLoad, 7))
    }
}

fn host() -> (Host, Rc<Cell<u32>>) {
    let reads = Rc::new(Cell::new(0));
    let r = reads.clone();
    let f: SourceFactory = Box::new(move || Box::new(Counting(r.clone())) as Box<dyn Source>);
    (Host::new(vec![f], Config::default()), reads)
}

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn nothing_runs_without_demand() {
    let (mut h, reads) = host();
    assert!(!h.sampling());
    assert_eq!(h.next_due(), None);
    h.tick(s(5));
    assert_eq!(reads.get(), 0);
    assert!(h.take_writes().is_empty());
}

#[test]
fn demand_starts_and_zero_stops_and_repeats_change_nothing() {
    let (mut h, reads) = host();
    let p = registered_pattern();
    h.demand(&p, 2, s(0));
    assert!(h.sampling() && h.next_due().is_some());
    h.demand(&p, 3, s(0));
    h.tick(s(0));
    let first = reads.get();
    assert!(first >= 1);
    assert!(h
        .take_writes()
        .iter()
        .all(|w: &Write| w.key.starts_with("sensor.")));
    h.demand(&p, 0, s(1));
    assert!(!h.sampling());
    assert_eq!(h.next_due(), None);
    h.tick(s(10));
    assert_eq!(reads.get(), first);
    h.demand(&p, 0, s(11));
    assert!(!h.sampling());
}

#[test]
fn a_pattern_outside_the_namespace_is_ignored() {
    let (mut h, _) = host();
    h.demand("keyboard.*", 4, s(0));
    assert!(!h.sampling());
}

#[test]
fn a_game_slows_the_reads() {
    let (mut h, reads) = host();
    h.demand(&registered_pattern(), 1, s(0));
    h.tick(s(0));
    h.game_mode(true, s(0));
    let before = reads.get();
    h.tick(s(3));
    h.tick(s(6));
    assert_eq!(reads.get(), before, "no read inside the game interval");
    h.game_mode(false, s(6));
    h.tick(s(8));
    assert!(reads.get() > before);
}

#[test]
fn stop_withdraws_everything() {
    let (mut h, _) = host();
    h.demand(&registered_pattern(), 1, s(0));
    h.tick(s(0));
    h.take_writes();
    h.stop(s(1));
    assert!(!h.sampling());
    let w = h.take_writes();
    assert!(w.iter().any(|w| w.value.is_none()));
}

/// A source that counts the times it was opened (made by the factory) and closed (dropped), as a
/// PDH query or an NVML load does.
struct Handle {
    #[allow(dead_code)]
    open: Rc<Cell<u32>>,
    close: Rc<Cell<u32>>,
}

impl Source for Handle {
    fn read(&mut self) -> Result<Reading, SourceError> {
        Ok(Reading::new().with(Metric::CpuLoad, 1))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.close.set(self.close.get() + 1);
    }
}

/// The same path the binary's loop takes: the hub's `facts.demand` through [`Host::notify`],
/// `tick`, and the release hook.
#[test]
fn every_open_is_closed_when_demand_returns_to_zero_and_a_resubscribe_opens_again() {
    use serde_json::json;
    let (open, close, trims) = (
        Rc::new(Cell::new(0)),
        Rc::new(Cell::new(0)),
        Rc::new(Cell::new(0)),
    );
    let (o, c) = (open.clone(), close.clone());
    // Two factories, like the PDH query and the NVML load.
    let factories: Vec<SourceFactory> = (0..2)
        .map(|_| {
            let (o, c) = (o.clone(), c.clone());
            Box::new(move || {
                o.set(o.get() + 1);
                Box::new(Handle {
                    open: o.clone(),
                    close: c.clone(),
                }) as Box<dyn Source>
            }) as SourceFactory
        })
        .collect();
    let mut h = Host::new(factories, Config::default());
    let (t, c2) = (trims.clone(), close.clone());
    // The hook runs after the drops: nothing is open any more.
    h.on_release(move || {
        assert_eq!(c2.get() % 2, 0);
        t.set(t.get() + 1);
    });
    let demand = |n: u32| json!({"pattern": registered_pattern(), "subscribers": n});
    assert_eq!((open.get(), close.get()), (0, 0));

    h.notify("facts.demand", &demand(1), s(0));
    h.tick(s(0));
    h.tick(s(1));
    assert_eq!((open.get(), close.get()), (2, 0));
    h.notify("facts.demand", &demand(0), s(2));
    assert_eq!((open.get(), close.get()), (2, 2));
    assert_eq!(trims.get(), 1);
    assert!(!h.sampling() && h.next_due().is_none());

    // Nothing opens while nobody listens, however long the loop ticks.
    h.tick(s(60));
    h.notify("facts.demand", &demand(0), s(61));
    assert_eq!((open.get(), close.get(), trims.get()), (2, 2, 1));

    h.notify("facts.demand", &demand(3), s(70));
    h.tick(s(70));
    assert_eq!((open.get(), close.get()), (4, 2));
    h.notify("facts.demand", &demand(0), s(80));
    assert_eq!((open.get(), close.get(), trims.get()), (4, 4, 2));

    // The program's end with something listening closes too.
    h.notify("facts.demand", &demand(1), s(90));
    h.stop(s(91));
    assert_eq!((open.get(), close.get(), trims.get()), (6, 6, 3));
}

#[test]
fn a_game_that_stops_sampling_closes_and_the_end_of_it_opens_again() {
    use serde_json::json;
    let (open, close) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let (o, c) = (open.clone(), close.clone());
    let f: SourceFactory = Box::new(move || {
        o.set(o.get() + 1);
        Box::new(Handle {
            open: o.clone(),
            close: c.clone(),
        }) as Box<dyn Source>
    });
    let config = Config {
        game_interval: None,
        ..Config::default()
    };
    let mut h = Host::new(vec![f], config);
    h.notify(
        "facts.demand",
        &json!({"pattern": registered_pattern(), "subscribers": 1}),
        s(0),
    );
    h.notify(
        "facts.changed",
        &json!({"key": "keyboard.game_mode", "value": true}),
        s(1),
    );
    assert_eq!((open.get(), close.get()), (1, 1));
    h.notify(
        "facts.changed",
        &json!({"key": "keyboard.game_mode", "value": false}),
        s(2),
    );
    assert_eq!((open.get(), close.get()), (2, 1));
}
