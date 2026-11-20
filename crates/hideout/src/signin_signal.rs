// SPDX-License-Identifier: MIT
//! The hub's half of the sign-in hand-over wait (sign-in hand-over slice 3). A `foxglove.exe
//! --tray` that was told "Hideout starts you" waits, at most 30 s, on one named kernel event
//! (`hideout_acl::signin_event`). The hub sets it once it has decided about Foxglove's row: the
//! row is running as an app, or definitely not starting (safe mode, stopped, waiting for an
//! answer, off), or Foxglove is not an app in this hub at all. The event is a hint only: the
//! waiter then runs its own verified check. Only a `--tray` (sign-in) start in a normal hub
//! (never a `HIDEOUT_CONFIG_DIR` / `HIDEOUT_HOME` run) opens it. Event driven, no polling.

use hideout_router::Out;
use serde_json::Value;

/// The module the waiting copy is.
const MODULE: &str = hideout_registry::keyboard::LEGACY_MODULE;

/// What the hub decided about Foxglove's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Running as an app.
    Running,
    /// Not starting, and why (the log's words).
    NotRunning(&'static str),
}

/// A router fact `key` = `value` as a decision, or `None` while the row is still deciding
/// (starting, restarting, updating) or the fact is not about this row.
pub fn decide(key: &str, value: &Value) -> Option<Decision> {
    if key == "hub.safe_mode" {
        return (value == &Value::Bool(true)).then_some(Decision::NotRunning("safe mode"));
    }
    if key != format!("module.{MODULE}.state") {
        return None;
    }
    Some(match value.as_str()? {
        "running" => Decision::Running,
        "crash-stopped" => Decision::NotRunning("stopped after crashes"),
        "waiting" | "needs-hub" | "update-waiting" => Decision::NotRunning("waiting for an answer"),
        "off" | "install-failed" | "revoked" | "quarantined" => Decision::NotRunning("not started"),
        _ => return None,
    })
}

/// Foxglove is not an app in this hub's inventory: nothing will start it, so answer at once.
pub fn decision_for_inventory(ids: &[String]) -> Option<Decision> {
    (!ids.iter().any(|i| i == MODULE)).then_some(Decision::NotRunning("runs on its own"))
}

/// The log line for a decision (`after`: time since the hub started).
pub fn line(d: &Decision, after: std::time::Duration) -> String {
    let s = format!("{:.1} s", after.as_secs_f64());
    match d {
        Decision::Running => {
            format!("sign-in: Foxglove running as an app after {s}; told a waiting copy")
        }
        Decision::NotRunning(why) => format!(
            "sign-in: Foxglove not started ({why}); told a waiting copy to start on its own"
        ),
    }
}

/// Where a decision goes (a kernel event on Windows; a fake in tests).
pub trait Sink: Send + Sync {
    fn set(&self) -> std::io::Result<()>;
}

impl Sink for hideout_acl::signin_event::SignInEvent {
    fn set(&self) -> std::io::Result<()> {
        hideout_acl::signin_event::SignInEvent::set(self)
    }
}

/// Tells the waiting copy once, at the first decision.
pub struct Signal {
    sink: Box<dyn Sink>,
    since: std::time::Instant,
    done: std::sync::atomic::AtomicBool,
}

impl Signal {
    pub fn new(sink: Box<dyn Sink>, since: std::time::Instant) -> Self {
        Self {
            sink,
            since,
            done: Default::default(),
        }
    }

    /// Sets the event for `d` (once); `log` gets the line.
    pub fn tell(&self, d: &Decision, now: std::time::Instant, log: impl FnOnce(&str)) {
        if self.done.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        match self.sink.set() {
            Ok(()) => log(&line(d, now.saturating_duration_since(self.since))),
            Err(e) => log(&format!("sign-in: could not tell a waiting copy: {e}")),
        }
    }
}

static SIGNAL: std::sync::OnceLock<Signal> = std::sync::OnceLock::new();

/// A sign-in (`--tray`) start of a normal hub opens the event. Anything else opens nothing.
pub fn install(tray_start: bool, overridden: bool) {
    if !tray_start || overridden {
        return;
    }
    match hideout_acl::signin_event::SignInEvent::open() {
        Ok(ev) => {
            let _ = SIGNAL.set(Signal::new(Box::new(ev), std::time::Instant::now()));
        }
        Err(e) => log::warn!("sign-in: the hand-over event could not be opened: {e}"),
    }
}

/// A router event: the first fact that settles Foxglove's row sets the event.
pub fn on_event(o: &Out) {
    if let (Some(sig), Out::Fact { key, value }) = (SIGNAL.get(), o) {
        if let Some(d) = decide(key, value) {
            sig.tell(&d, std::time::Instant::now(), |l| log::info!("{l}"));
        }
    }
}

/// The inventory is read: Foxglove not being an app here settles its row too.
pub fn on_inventory(ids: &[String]) {
    if let (Some(sig), Some(d)) = (SIGNAL.get(), decision_for_inventory(ids)) {
        sig.tell(&d, std::time::Instant::now(), |l| log::info!("{l}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn state(v: &str) -> Option<Decision> {
        decide(&format!("module.{MODULE}.state"), &json!(v))
    }

    #[test]
    fn running_and_every_definite_no_settle_the_row() {
        assert_eq!(state("running"), Some(Decision::Running));
        assert_eq!(
            state("crash-stopped"),
            Some(Decision::NotRunning("stopped after crashes"))
        );
        assert_eq!(
            state("waiting"),
            Some(Decision::NotRunning("waiting for an answer"))
        );
        assert!(matches!(state("off"), Some(Decision::NotRunning(_))));
    }

    #[test]
    fn a_row_still_deciding_settles_nothing() {
        for v in ["starting", "restarting", "updating", "update-busy"] {
            assert_eq!(state(v), None, "{v}");
        }
        assert_eq!(
            decide(&format!("module.{MODULE}.state"), &Value::Null),
            None
        );
        assert_eq!(decide("module.other.state", &json!("running")), None);
    }

    #[test]
    fn safe_mode_answers_at_once_and_off_does_not() {
        assert_eq!(
            decide("hub.safe_mode", &json!(true)),
            Some(Decision::NotRunning("safe mode"))
        );
        assert_eq!(decide("hub.safe_mode", &json!(false)), None);
    }

    #[test]
    fn foxglove_missing_from_the_inventory_answers_at_once() {
        assert!(decision_for_inventory(&[]).is_some());
        assert_eq!(decision_for_inventory(&[MODULE.to_string()]), None);
    }

    #[test]
    fn the_lines_are_exact() {
        let t = std::time::Duration::from_millis(3800);
        assert_eq!(
            line(&Decision::Running, t),
            "sign-in: Foxglove running as an app after 3.8 s; told a waiting copy"
        );
        assert_eq!(
            line(&Decision::NotRunning("safe mode"), t),
            "sign-in: Foxglove not started (safe mode); told a waiting copy to start on its own"
        );
    }

    struct Count(Arc<Mutex<u32>>);
    impl Sink for Count {
        fn set(&self) -> std::io::Result<()> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[test]
    fn only_the_first_decision_sets_the_event() {
        let n = Arc::new(Mutex::new(0));
        let t0 = std::time::Instant::now();
        let s = Signal::new(Box::new(Count(n.clone())), t0);
        let mut lines = Vec::new();
        s.tell(&Decision::Running, t0, |l| lines.push(l.to_string()));
        s.tell(&Decision::NotRunning("safe mode"), t0, |l| {
            lines.push(l.to_string())
        });
        assert_eq!(*n.lock().unwrap(), 1);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn nothing_opens_without_a_sign_in_start_or_in_a_test_run() {
        install(false, false);
        install(true, true);
        assert!(SIGNAL.get().is_none());
    }
}
