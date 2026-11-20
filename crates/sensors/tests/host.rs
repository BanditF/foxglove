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
