//! Panic boundaries for Foxglove workers, independent of the overlay and keyboard link.
//!
//! [`spawn_supervised`] runs a long-lived service and restarts it after a panic, with a backoff
//! that doubles from 1 s to 60 s; five failures in ten minutes stop it, and the diagnostics
//! report and the Troubleshooting card say so. A healthy service has no timer: the supervisor
//! blocks on `join`, and sleeps only between a failure and the next attempt. [`guard`] is the
//! same boundary for a one-shot job, without the restart.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceStatus {
    pub running: bool,
    pub failures: u64,
    pub last_failure: Option<String>,
    /// Gave up after [`MAX_CRASHES`] quick failures: the feature is off until Foxglove restarts.
    pub stopped: bool,
    /// Failed and waiting out the backoff before the next attempt.
    pub restarting: bool,
}

/// Where a service is, for [`record`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    /// Returned, or a one-shot job that panicked: nothing is coming back.
    Ended,
    Restarting,
    Stopped,
}

static STATUS: Mutex<BTreeMap<&'static str, ServiceStatus>> = Mutex::new(BTreeMap::new());

/// A snapshot of every service started so far. A restart keeps the last failure visible.
pub fn status() -> BTreeMap<&'static str, ServiceStatus> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// One line per service that has failed, stopped ones first, for the diagnostics report.
pub fn problems() -> Vec<String> {
    let mut lines: Vec<(bool, String)> = status()
        .into_iter()
        .filter(|(_, s)| s.failures > 0)
        .map(|(name, s)| {
            let last = s.last_failure.unwrap_or_default();
            let failures = match s.failures {
                1 => "1 failure".to_string(),
                n => format!("{n} failures"),
            };
            let line = if s.stopped {
                format!("{name}: service stopped after {failures} ({last})")
            } else {
                format!("{name}: {failures}, running again ({last})")
            };
            (!s.stopped, line)
        })
        .collect();
    lines.sort();
    lines.into_iter().map(|(_, line)| line).collect()
}

/// The services that gave up, for a note the user can see.
pub fn stopped() -> Vec<&'static str> {
    status()
        .into_iter()
        .filter(|(_, s)| s.stopped)
        .map(|(name, _)| name)
        .collect()
}

fn record(name: &'static str, phase: Phase, failure: Option<String>) {
    let mut statuses = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    let status = statuses.entry(name).or_default();
    status.running = phase == Phase::Running;
    status.stopped = phase == Phase::Stopped;
    status.restarting = phase == Phase::Restarting;
    if let Some(failure) = failure {
        status.failures += 1;
        status.last_failure = Some(failure);
    }
}

/// The first attempt's handle (a channel or subscription made before the thread starts, so
/// nothing sent in between is missed); a restarted attempt makes a fresh one.
pub struct First<T>(Mutex<Option<T>>);

impl<T> First<T> {
    pub fn new(first: T) -> First<T> {
        First(Mutex::new(Some(first)))
    }

    pub fn take_or(&self, make: impl FnOnce() -> T) -> T {
        let first = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        first.unwrap_or_else(make)
    }
}

/// Locks `m`. If a panic poisoned it, the value goes back to its default first, as the panic
/// may have left it half-written.
pub fn lock_or_reset<T: Default>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = T::default();
            m.clear_poison();
            guard
        }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload");
    format!("Panicked: {message}")
}

/// Runs a one-shot job (a request or a check) behind a panic boundary on the calling thread:
/// `None` if it panicked, which is logged and listed in the diagnostics report. Nothing
/// restarts it; the caller resets whatever the job would have finished.
pub fn guard<R>(name: &'static str, job: impl FnOnce() -> R) -> Option<R> {
    match catch_unwind(AssertUnwindSafe(job)) {
        Ok(r) => Some(r),
        Err(payload) => {
            let failure = panic_text(&*payload);
            eprintln!("Foxglove {name}: {failure}");
            record(name, Phase::Ended, Some(failure));
            None
        }
    }
}

#[derive(Default)]
struct Backoff {
    next: Option<Duration>,
}

impl Backoff {
    fn failed(&mut self, uptime: Duration) -> Duration {
        if uptime >= Duration::from_secs(60) {
            self.next = None;
        }
        let delay = self.next.unwrap_or(Duration::from_secs(1));
        self.next = Some((delay * 2).min(Duration::from_secs(60)));
        delay
    }
}

/// `std::thread::spawn` with a name, so a profiler can tell Foxglove's threads apart.
pub fn spawn_named<F: FnOnce() + Send + 'static>(name: &str, f: F) -> JoinHandle<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("failed to spawn thread")
}

/// Failures in [`CRASH_WINDOW`] that stop a service for good, like the plugin runner (#279).
const MAX_CRASHES: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(600);

/// Runs each attempt on its own thread. Panics restart, until five in ten minutes stop it; a
/// normal return stops the service. `run` makes what it owns (channels, subscriptions,
/// watchers) itself, or takes it from a [`First`] or a shared `Mutex`, so an attempt can start
/// again. The supervisor waits on join, sleeping only for backoff after a failure.
pub fn spawn_supervised(
    name: &'static str,
    run: impl Fn() + Send + Sync + 'static,
) -> Option<JoinHandle<()>> {
    spawn_with(name, run, thread::sleep)
}

fn spawn_with(
    name: &'static str,
    run: impl Fn() + Send + Sync + 'static,
    sleep: impl FnMut(Duration) + Send + 'static,
) -> Option<JoinHandle<()>> {
    match thread::Builder::new()
        .name(format!("foxglove-{name}-supervisor"))
        .spawn(move || supervise(name, Arc::new(run), sleep))
    {
        Ok(handle) => Some(handle),
        Err(e) => {
            let failure = format!("Could not start supervisor: {e}");
            eprintln!("Foxglove {name}: {failure}; service stopped");
            record(name, Phase::Stopped, Some(failure));
            None
        }
    }
}

fn supervise(
    name: &'static str,
    run: Arc<impl Fn() + Send + Sync + 'static>,
    mut sleep: impl FnMut(Duration),
) {
    let mut backoff = Backoff::default();
    let mut crashes: std::collections::VecDeque<Instant> = std::collections::VecDeque::new();
    loop {
        let started = Instant::now();
        let run = run.clone();
        record(name, Phase::Running, None);
        let failure = match thread::Builder::new()
            .name(format!("foxglove-{name}"))
            .spawn(move || catch_unwind(AssertUnwindSafe(|| run())))
        {
            Ok(child) => match child.join() {
                Ok(Ok(())) => {
                    record(name, Phase::Ended, None);
                    return;
                }
                Ok(Err(payload)) | Err(payload) => panic_text(&*payload),
            },
            Err(e) => format!("Could not start worker: {e}"),
        };
        let now = Instant::now();
        crashes.push_back(now);
        while crashes
            .front()
            .is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW)
        {
            crashes.pop_front();
        }
        if crashes.len() >= MAX_CRASHES {
            eprintln!(
                "Foxglove {name}: {failure}; service stopped after {MAX_CRASHES} failures in ten minutes"
            );
            record(name, Phase::Stopped, Some(failure));
            return;
        }
        let delay = backoff.failed(started.elapsed());
        eprintln!(
            "Foxglove {name}: {failure}; restarting in {} s",
            delay.as_secs()
        );
        record(name, Phase::Restarting, Some(failure));
        sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    #[test]
    fn backoff_doubles_caps_and_resets_after_a_healthy_minute() {
        let mut backoff = Backoff::default();
        for seconds in [1, 2, 4, 8, 16, 32, 60, 60] {
            assert_eq!(backoff.failed(Duration::ZERO), Duration::from_secs(seconds));
        }
        assert_eq!(
            backoff.failed(Duration::from_secs(59)),
            Duration::from_secs(60)
        );
        assert_eq!(
            backoff.failed(Duration::from_secs(60)),
            Duration::from_secs(1)
        );
        assert_eq!(backoff.failed(Duration::ZERO), Duration::from_secs(2));
    }

    #[test]
    fn a_service_that_keeps_failing_is_stopped_and_reported() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = attempts.clone();
        supervise(
            "crash-cap-test",
            Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                panic!("always");
            }),
            |_| {},
        );
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_CRASHES);
        let service = &status()["crash-cap-test"];
        assert!(service.stopped && !service.running);
        assert_eq!(service.last_failure.as_deref(), Some("Panicked: always"));
        assert!(stopped().contains(&"crash-cap-test"));
        assert!(problems().contains(
            &"crash-cap-test: service stopped after 5 failures (Panicked: always)".to_string()
        ));
    }

    #[test]
    fn panics_restart_and_normal_return_stops() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = attempts.clone();
        let mut delays = Vec::new();
        supervise(
            "restart-test",
            Arc::new(move || {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    panic!("deliberate service failure");
                }
            }),
            |delay| {
                let service = &status()["restart-test"];
                assert!(!service.running && service.restarting);
                delays.push(delay);
            },
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(delays, [Duration::from_secs(1), Duration::from_secs(2)]);
        let service = &status()["restart-test"];
        assert!(!service.running && !service.stopped && !service.restarting);
        assert_eq!(service.failures, 2);
        assert_eq!(
            service.last_failure.as_deref(),
            Some("Panicked: deliberate service failure")
        );
        assert!(!stopped().contains(&"restart-test"));
    }

    /// A fake service like `pc-lights`: its queue is made outside and shared, and one message
    /// panics it. The restart keeps the queue, so the sender stays good and nothing is lost.
    #[test]
    fn a_restarted_service_keeps_its_queue_and_backs_off() {
        let (tx, rx) = mpsc::channel::<u32>();
        let rx = Mutex::new(rx);
        let (seen_tx, seen) = mpsc::channel();
        let (delays_tx, delays) = mpsc::channel();
        let supervisor = spawn_with(
            "fake-queue-service",
            move || {
                let rx = rx.lock().unwrap_or_else(|e| e.into_inner());
                for n in rx.iter() {
                    match n {
                        0 => return,
                        13 => panic!("bad message {n}"),
                        n => seen_tx.send(n).unwrap(),
                    }
                }
            },
            move |delay| delays_tx.send(delay).unwrap(),
        )
        .expect("supervisor");
        for n in [1, 13, 2, 13, 3, 0] {
            tx.send(n).expect("the queue outlives a panic");
        }
        supervisor.join().unwrap();
        assert_eq!(seen.try_iter().collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(
            delays.try_iter().collect::<Vec<_>>(),
            [Duration::from_secs(1), Duration::from_secs(2)]
        );
        let service = &status()["fake-queue-service"];
        assert_eq!(service.failures, 2);
        assert_eq!(
            service.last_failure.as_deref(),
            Some("Panicked: bad message 13")
        );
        assert!(problems().contains(
            &"fake-queue-service: 2 failures, running again (Panicked: bad message 13)".to_string()
        ));
    }

    #[test]
    fn first_hands_over_its_value_once() {
        let first = First::new(1);
        assert_eq!(first.take_or(|| 2), 1);
        assert_eq!(first.take_or(|| 2), 2);
    }

    #[test]
    fn a_poisoned_lock_is_reset_and_cleared() {
        let m = Arc::new(Mutex::new(Some("half-written".to_string())));
        let poisoner = m.clone();
        let _ = thread::spawn(move || {
            let _held = poisoner.lock().unwrap();
            panic!("poison it");
        })
        .join();
        assert!(m.is_poisoned());
        assert_eq!(*lock_or_reset(&m), None);
        assert!(!m.is_poisoned());
        *lock_or_reset(&m) = Some("kept".into());
        assert_eq!(lock_or_reset(&m).as_deref(), Some("kept"));
    }

    #[test]
    fn guard_catches_a_one_shot_panic() {
        assert_eq!(guard("guard-ok-test", || 7), Some(7));
        assert_eq!(guard("guard-test", || -> u8 { panic!("once") }), None);
        let service = &status()["guard-test"];
        assert_eq!(service.failures, 1);
        assert!(!service.stopped);
    }
}
