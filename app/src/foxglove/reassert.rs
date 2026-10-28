//! Re-applies the state Foxglove owns on its devices after events that can lose it (#618
//! theme A, #601).
//!
//! Each subsystem that pushes state to a device sets it on its own trigger and used to assume it
//! stuck. After a reconnect, the right half's re-link, a wake, a display change or an Explorer
//! restart, nothing put it back. Like a reconcile loop, each subsystem keeps its desired state
//! and registers a hook here; every hook hears every [`Trigger`] and re-applies what the device
//! lost, skipping what it already shows (a read where one exists), with one
//! `Foxglove: re-applied …` line per re-apply.
//!
//! The hooks today: the accent colour (`accent.rs`), the right screen's art (`art_service.rs`)
//! and the keyboard lights a lease holds (`lighting_leases.rs`).
//!
//! The triggers are events only, no timer:
//! - [`Trigger::KeyboardConnected`]: the device bus's Connected event;
//! - [`Trigger::RightRelayReady`]: the left's relay to the right half came up (an art STATUS
//!   event with link 2, D6 85, firmware with HELLO bit 20, #594), once per connection;
//! - [`Trigger::PcBack`]: unlocked or the display back on (`away.rs`, #410);
//! - [`Trigger::DisplayChange`] and [`Trigger::TaskbarRestart`]: the overlay's display-change
//!   window and the tray's TaskbarCreated (#612, `overlay_restore.rs`), reused, not new watchers.

use std::sync::{mpsc, Arc, Mutex, Weak};

/// What can have lost a device's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    KeyboardConnected,
    RightRelayReady,
    PcBack,
    DisplayChange,
    TaskbarRestart,
}

impl Trigger {
    #[cfg(test)]
    pub const ALL: [Trigger; 5] = [
        Trigger::KeyboardConnected,
        Trigger::RightRelayReady,
        Trigger::PcBack,
        Trigger::DisplayChange,
        Trigger::TaskbarRestart,
    ];

    /// For the log line.
    pub fn label(self) -> &'static str {
        match self {
            Trigger::KeyboardConnected => "keyboard connected",
            Trigger::RightRelayReady => "right relay ready",
            Trigger::PcBack => "PC back",
            Trigger::DisplayChange => "display change",
            Trigger::TaskbarRestart => "taskbar restart",
        }
    }

    /// A link event: the keyboard or its right half (re)joined. A push that failed is tried
    /// again on these (#601).
    pub fn is_link(self) -> bool {
        matches!(self, Trigger::KeyboardConnected | Trigger::RightRelayReady)
    }
}

/// The log line for a re-apply.
pub fn line(subsystem: &str, trigger: Trigger) -> String {
    format!("Foxglove: re-applied {subsystem} ({})", trigger.label())
}

type Hook = Box<dyn Fn(Trigger) + Send + Sync>;

/// Hooks by subsystem name. A hook runs on the thread that saw the event, with the registry
/// locked: it only enqueues work for its subsystem's own thread.
pub struct Registry {
    hooks: Mutex<Vec<(&'static str, Hook)>>,
}

impl Registry {
    pub const fn new() -> Registry {
        Registry {
            hooks: Mutex::new(Vec::new()),
        }
    }

    /// Adds `hook` for `subsystem`; it lasts as long as the registry (the process).
    pub fn register(
        &self,
        subsystem: &'static str,
        hook: impl Fn(Trigger) + Send + Sync + 'static,
    ) {
        self.hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((subsystem, Box::new(hook)));
    }

    /// Tells every hook about `trigger`; how many heard it.
    pub fn fire(&self, trigger: Trigger) -> usize {
        let hooks = self.hooks.lock().unwrap_or_else(|e| e.into_inner());
        for (subsystem, hook) in hooks.iter() {
            // One hook's panic must not keep the others from hearing it.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(trigger))).is_err() {
                eprintln!(
                    "Foxglove: the {subsystem} re-apply hook panicked ({})",
                    trigger.label()
                );
            }
        }
        hooks.len()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Registry::new()
    }
}

static HOOKS: Registry = Registry::new();

/// Registers a subsystem's re-apply hook (see [`Registry::register`]).
pub fn register(subsystem: &'static str, hook: impl Fn(Trigger) + Send + Sync + 'static) {
    HOOKS.register(subsystem, hook);
}

/// An event that can have lost device state happened: every hook hears it.
pub fn fire(trigger: Trigger) {
    HOOKS.fire(trigger);
}

/// Turns one link's art STATUS reports into relay-ready edges: true when the relay is up (link 2)
/// and wasn't on this connection before. A new connection (generation) starts over, so the
/// relay coming up again after the right half reboots is a new edge, and the reports that
/// follow while it stays up are not.
#[derive(Debug, Default)]
pub struct RelayEdge {
    /// The connection and whether its relay was last seen up.
    last: Option<(u64, bool)>,
}

impl RelayEdge {
    pub fn status(&mut self, generation: u64, link: u8) -> bool {
        let up = link == 2;
        let was_up = matches!(self.last, Some((g, true)) if g == generation);
        self.last = Some((generation, up));
        up && !was_up
    }
}

/// Starts the shared triggers that need a listener here: keyboard arrivals (the device bus),
/// relay-ready (each shared link's events) and coming back (#410). Display changes and taskbar
/// restarts are fired by `overlay_restore`, which already gets them. Every listener sleeps on
/// its channel; nothing polls.
pub fn start() {
    if super::repaint_trace::disabled("reassert") {
        return;
    }
    // The accent and the art register when they start; the lights' hook needs a thread.
    super::lighting_leases::register_reassert();
    // Keyboard arrivals.
    let events = super::services::First::new(super::devices::bus().subscribe(false));
    let _ = super::services::spawn_supervised("reassert-arrivals", move || {
        for e in events.take_or(|| super::devices::bus().subscribe(false)) {
            if e.event == foxglove_devices::DeviceEvent::Connected {
                fire(Trigger::KeyboardConnected);
            }
        }
    });
    // Coming back: the away listener runs with its list locked, and fire only enqueues.
    let away = super::away::subscribe(|away| {
        if !away {
            fire(Trigger::PcBack);
        }
    });
    std::mem::forget(away);
    // Relay-ready, per shared link. `subscribe_links` calls back with the link list locked, so
    // the links go to a thread that subscribes to their events.
    let (tx, rx) = mpsc::channel::<Arc<foxglove_link::link::Link>>();
    let alive = super::link::subscribe_links(move |link| {
        let _ = tx.send(link);
    });
    super::services::spawn_named("reassert-links", move || {
        let _alive = alive;
        let mut seen: Vec<(
            Weak<foxglove_link::link::Link>,
            foxglove_link::link::EventSubscription,
        )> = Vec::new();
        for link in rx {
            seen.retain(|(l, _)| l.strong_count() > 0);
            if seen
                .iter()
                .any(|(l, _)| Weak::ptr_eq(l, &Arc::downgrade(&link)))
            {
                continue;
            }
            let edge = Mutex::new(RelayEdge::default());
            // Runs on the link's reader thread: only enqueues (through the hooks).
            let subscription = link.subscribe_events(move |event| {
                if let foxglove_link::link::Event::ArtStatus { generation, report } = event {
                    let status = foxglove_link::protocol::ArtStatus::parse(&report);
                    if edge
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .status(generation, status.link)
                    {
                        fire(Trigger::RightRelayReady);
                    }
                }
            });
            seen.push((Arc::downgrade(&link), subscription));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn each_trigger_reaches_each_hook() {
        let registry = Registry::new();
        let heard: Arc<Mutex<Vec<(&str, Trigger)>>> = Arc::default();
        for subsystem in ["accent", "art", "lights"] {
            let heard = heard.clone();
            registry.register(subsystem, move |t| {
                heard.lock().unwrap().push((subsystem, t));
            });
        }
        for t in Trigger::ALL {
            assert_eq!(registry.fire(t), 3);
        }
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 15);
        for subsystem in ["accent", "art", "lights"] {
            for t in Trigger::ALL {
                assert_eq!(
                    heard.iter().filter(|h| **h == (subsystem, t)).count(),
                    1,
                    "{subsystem} heard {t:?} once"
                );
            }
        }
    }

    #[test]
    fn a_panicking_hook_does_not_stop_the_others() {
        let registry = Registry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        registry.register("broken", |_| panic!("boom"));
        let c = calls.clone();
        registry.register("fine", move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        });
        registry.fire(Trigger::PcBack);
        registry.fire(Trigger::PcBack);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn only_the_relay_coming_up_is_an_edge() {
        let mut edge = RelayEdge::default();
        assert!(!edge.status(1, 1), "connected, still discovering");
        assert!(edge.status(1, 2), "the relay came up");
        assert!(!edge.status(1, 2), "another report while up: no edge");
        assert!(!edge.status(1, 2));
        assert!(!edge.status(1, 0), "the right half went");
        assert!(edge.status(1, 2), "and came back");
        // A new connection: its first relay-up is an edge, even straight after the last one.
        assert!(edge.status(2, 2));
        assert!(!edge.status(2, 2));
    }

    #[test]
    fn link_triggers_are_the_keyboard_and_the_right_half() {
        let link: Vec<_> = Trigger::ALL.into_iter().filter(|t| t.is_link()).collect();
        assert_eq!(link, [Trigger::KeyboardConnected, Trigger::RightRelayReady]);
    }

    #[test]
    fn the_log_line_names_the_subsystem_and_the_trigger() {
        assert_eq!(
            line("the accent colour", Trigger::RightRelayReady),
            "Foxglove: re-applied the accent colour (right relay ready)"
        );
    }
}
