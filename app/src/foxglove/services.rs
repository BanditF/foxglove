//! Panic boundaries for Foxglove workers, independent of the overlay and keyboard link.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceStatus {
    pub running: bool,
    pub failures: u64,
    pub last_failure: Option<String>,
}

static STATUS: Mutex<BTreeMap<&'static str, ServiceStatus>> = Mutex::new(BTreeMap::new());

/// A snapshot for the future Screens tab. A restart keeps the last failure visible.
#[allow(dead_code)]
pub fn status() -> BTreeMap<&'static str, ServiceStatus> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn record(name: &'static str, running: bool, failure: Option<String>) {
    let mut statuses = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    let status = statuses.entry(name).or_default();
    status.running = running;
    if let Some(failure) = failure {
        eprintln!("Foxglove {name}: {failure}");
        status.failures += 1;
        status.last_failure = Some(failure);
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

/// Runs each attempt on its own thread. Panics restart; a normal return stops the service.
/// The supervisor waits on join, sleeping only for backoff after a failure.
pub fn spawn_supervised(
    name: &'static str,
    run: impl Fn() + Send + Sync + 'static,
) -> Option<JoinHandle<()>> {
    match thread::Builder::new()
        .name(format!("foxglove-{name}-supervisor"))
        .spawn(move || supervise(name, Arc::new(run), thread::sleep))
    {
        Ok(handle) => Some(handle),
        Err(e) => {
            record(
                name,
                false,
                Some(format!("Could not start supervisor: {e}")),
            );
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
    loop {
        let started = Instant::now();
        let run = run.clone();
        record(name, true, None);
        let failure = match thread::Builder::new()
            .name(format!("foxglove-{name}"))
            .spawn(move || catch_unwind(AssertUnwindSafe(|| run())))
        {
            Ok(child) => match child.join() {
                Ok(Ok(())) => {
                    record(name, false, None);
                    return;
                }
                Ok(Err(payload)) | Err(payload) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("non-string panic payload");
                    format!("Panicked: {message}")
                }
            },
            Err(e) => format!("Could not start worker: {e}"),
        };
        record(name, false, Some(failure));
        sleep(backoff.failed(started.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
                let snapshot = status();
                assert!(!snapshot["restart-test"].running);
                delays.push(delay);
            },
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(delays, [Duration::from_secs(1), Duration::from_secs(2)]);
        let snapshot = status();
        let service = &snapshot["restart-test"];
        assert!(!service.running);
        assert_eq!(service.failures, 2);
        assert_eq!(
            service.last_failure.as_deref(),
            Some("Panicked: deliberate service failure")
        );
    }
}
