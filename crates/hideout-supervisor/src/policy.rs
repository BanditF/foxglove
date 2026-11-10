// SPDX-License-Identifier: MIT
//! The supervisor's decisions with no I/O (contract §7.5, §7.7 to §7.9): events in, [`Cmd`]s out,
//! the clock passed in. The driver ([`crate::driver`]) carries the commands out with Job Objects,
//! the registry and the router.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::words;

/// Fixed backoff steps (§7.7, v1.1) for the 1st to 7th restart in a run; 60 s after that.
pub const BACKOFF: [u64; 7] = [1, 2, 4, 8, 16, 32, 60];
/// Crashes in [`WINDOW`] that stop a module (§7.7).
pub const CRASH_LIMIT: usize = 5;
/// The crash window (§7.7) and the automatic-rollback window after an update (§7.8).
pub const WINDOW: Duration = Duration::from_secs(600);
/// A module that doesn't send `module.ready` within this long of its start crashed (§7.5).
pub const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a module keeps running without a hub (§7.9): the hub waits as long for a module it
/// takes back before it starts it fresh.
pub const GRACE: Duration = Duration::from_secs(30);
/// After the pipe closes, how long the process has to exit by itself before it counts as a
/// crash and is ended (a clean exit closes the pipe first). Fallback only: the exit event ends
/// the wait.
pub const EXIT_AFTER_PIPE: Duration = Duration::from_secs(5);
/// Lines of history kept per module for Copy diagnostics (oldest dropped first).
pub const HISTORY: usize = 32;

/// The delay before restart number `n` (1-based) in a run.
pub fn backoff(n: u32) -> Duration {
    let i = (n.max(1) as usize - 1).min(BACKOFF.len() - 1);
    Duration::from_secs(BACKOFF[i])
}

/// A module's state (§7.8). The ids are what the fact `module.<id>.state` carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Disabled.
    Off,
    /// Enabled, not running, waiting for an activation event (§4.6), or stopped by its own
    /// clean exit. No fact: nothing is wrong.
    Idle,
    Starting,
    Running,
    Restarting,
    CrashStopped,
    InstallFailed,
}

impl State {
    pub fn id(self) -> Option<&'static str> {
        Some(match self {
            State::Off => "off",
            State::Idle => return None,
            State::Starting => "starting",
            State::Running => "running",
            State::Restarting => "restarting",
            State::CrashStopped => "crash-stopped",
            State::InstallFailed => "install-failed",
        })
    }
}

/// What happened, as the driver saw it.
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    /// A module the hub knows: its display name and whether it starts at enable / hub start.
    Known {
        id: String,
        name: String,
        enabled: bool,
        at_enable: bool,
        previous: Option<String>,
    },
    /// The driver started instance `instance` (after [`Cmd::Launch`]).
    Started {
        id: String,
        instance: u64,
    },
    /// The launch failed before a process ran (the file check, the exe missing).
    LaunchFailed {
        id: String,
        reason: String,
    },
    /// After a hub restart: a recorded instance still runs and passed the pid and exe-hash
    /// check (§7.9). It has [`GRACE`] to reconnect.
    Adopted {
        id: String,
        instance: u64,
    },
    /// `module.ready` (§7.5).
    Ready {
        id: String,
    },
    /// The process of `instance` ended with `code`.
    Exited {
        id: String,
        instance: u64,
        code: i64,
    },
    /// The module's router connection ended.
    PipeGone {
        id: String,
    },
    /// The router closed its connection for a wire fault (`rpc.frame-too-large`, §8.1 v1.2).
    Fault {
        id: String,
    },
    /// The router refused its `initialize`: the pid or exe hash didn't match (§7.5).
    Refused {
        id: String,
    },
    /// An activation event for it (§4.6): a call, a shown panel, a fact.
    Activate {
        id: String,
    },
    /// "Start again" (the count starts fresh).
    StartAgain {
        id: String,
    },
    /// "Start with default settings" (§7.8).
    StartDefaults {
        id: String,
    },
    /// "Go back to <previous>" (the ladder's third step): roll back, then start.
    GoBack {
        id: String,
    },
    /// An update from `previous` to `version` finished.
    Updated {
        id: String,
        version: String,
        previous: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    /// **Start normally** (§7.10): safe mode ends and every enabled module that starts at
    /// enable starts now, without a hub restart.
    StartNormally,
}

/// What the driver must do.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// Check the files, then start the module in a new Job Object; answer with
    /// [`Ev::Started`] or [`Ev::LaunchFailed`].
    Launch { id: String },
    /// End the module's Job Object (all its processes).
    Kill { id: String },
    /// Publish a hub fact (`null` removes it).
    Publish { key: String, value: Value },
    /// Show a notification (§7.7: once per module per sign-in).
    Notify {
        id: String,
        title: String,
        body: String,
        actions: Vec<String>,
    },
    /// Rename the settings folder to `settings.bad-<time>` (§7.8); comes right before its
    /// [`Cmd::Launch`], once no instance runs.
    ResetSettings { id: String },
    /// Go back to the previous version (§7.8), before the next launch.
    RollBack { id: String },
}

#[derive(Debug)]
struct Slot {
    name: String,
    state: State,
    enabled: bool,
    at_enable: bool,
    previous: Option<String>,
    instance: u64,
    /// The current instance's end was already handled (crash counted or clean stop).
    ended: bool,
    crashes: VecDeque<Instant>,
    /// Restarts in this run, for the backoff step.
    run_restarts: u32,
    restart_at: Option<Instant>,
    ready_by: Option<Instant>,
    /// Taken back after a hub restart: no crash bar, fresh start past this.
    adopt_by: Option<Instant>,
    /// The pipe closed; the process has until then to exit by itself.
    exit_by: Option<Instant>,
    updated_at: Option<Instant>,
    /// The version the update brought, for the rollback note.
    updated_to: String,
    /// Crash stops this sign-in, for the recovery ladder.
    stops: u32,
    /// The launch to run once the current instance is gone.
    then_launch: bool,
    /// Rename the settings folder before the next launch (Start with default settings).
    reset_first: bool,
}

/// The supervisor's state for every module.
#[derive(Debug, Default)]
pub struct Policy {
    slots: BTreeMap<String, Slot>,
    /// Modules already notified this sign-in (§7.7).
    notified: BTreeSet<String>,
    /// Safe mode (§7.10): nothing starts by itself.
    pub safe_mode: bool,
    /// What happened to each module, for Copy diagnostics only (exit codes, crashes, the
    /// ladder steps tried). Never drawn: it holds ids and codes ("Never shown").
    history: BTreeMap<String, VecDeque<(Instant, String)>>,
}

fn publish(out: &mut Vec<Cmd>, key: String, value: Value) {
    out.push(Cmd::Publish { key, value });
}

impl Policy {
    pub fn new() -> Policy {
        Policy::default()
    }

    /// One line of a module's history (known modules only), [`HISTORY`] kept.
    fn note(&mut self, id: &str, now: Instant, line: String) {
        if !self.slots.contains_key(id) {
            return;
        }
        let h = self.history.entry(id.into()).or_default();
        if h.len() == HISTORY {
            h.pop_front();
        }
        h.push_back((now, line));
    }

    /// The module's history, oldest first: when, and what (state ids, exit codes, steps).
    pub fn history(&self, id: &str) -> Vec<(Instant, String)> {
        self.history
            .get(id)
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Crash stops this sign-in (the recovery ladder's count).
    pub fn stops(&self, id: &str) -> u32 {
        self.slots.get(id).map_or(0, |s| s.stops)
    }

    pub fn state(&self, id: &str) -> Option<State> {
        self.slots.get(id).map(|s| s.state)
    }

    /// Crashes in the window ending at `now`.
    pub fn crashes(&self, id: &str, now: Instant) -> usize {
        self.slots
            .get(id)
            .map(|s| s.crashes.iter().filter(|t| now - **t < WINDOW).count())
            .unwrap_or(0)
    }

    /// When a restarting module starts again.
    pub fn restart_at(&self, id: &str) -> Option<Instant> {
        self.slots.get(id).and_then(|s| s.restart_at)
    }

    /// The current instance number of `id` (0 before its first start).
    pub fn instance(&self, id: &str) -> u64 {
        self.slots.get(id).map(|s| s.instance).unwrap_or(0)
    }

    /// The earliest timer: a restart, a ready deadline, an adoption grace, a pipe-close wait.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.slots
            .values()
            .flat_map(|s| [s.restart_at, s.ready_by, s.adopt_by, s.exit_by])
            .flatten()
            .min()
    }

    fn set_state(&mut self, id: &str, state: State, reason: Option<String>, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        s.state = state;
        publish(
            out,
            format!("module.{id}.state"),
            state.id().map_or(Value::Null, |v| json!(v)),
        );
        publish(
            out,
            format!("module.{id}.reason"),
            reason.map_or(Value::Null, Value::String),
        );
    }

    fn launch(&mut self, id: &str, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        s.restart_at = None;
        s.exit_by = None;
        s.adopt_by = None;
        s.then_launch = false;
        let reset = std::mem::take(&mut s.reset_first);
        self.set_state(id, State::Starting, None, out);
        // The stopped card's and the launch failure's facts end with the stop.
        for k in ["recovery", "previous", "failed"] {
            publish(out, format!("module.{id}.{k}"), Value::Null);
        }
        if reset {
            out.push(Cmd::ResetSettings { id: id.into() });
        }
        out.push(Cmd::Launch { id: id.into() });
    }

    /// One event; returns what to do.
    pub fn handle(&mut self, ev: Ev, now: Instant) -> Vec<Cmd> {
        let mut out = Vec::new();
        if let Some((id, line)) = history_line(&ev, self) {
            self.note(&id, now, line);
        }
        match ev {
            Ev::Known {
                id,
                name,
                enabled,
                at_enable,
                previous,
            } => {
                let s = self.slots.entry(id.clone()).or_insert_with(|| Slot {
                    name: name.clone(),
                    state: State::Idle,
                    enabled,
                    at_enable,
                    previous: None,
                    instance: 0,
                    ended: true,
                    crashes: VecDeque::new(),
                    run_restarts: 0,
                    restart_at: None,
                    ready_by: None,
                    adopt_by: None,
                    exit_by: None,
                    updated_at: None,
                    updated_to: String::new(),
                    stops: 0,
                    then_launch: false,
                    reset_first: false,
                });
                s.name = name;
                s.enabled = enabled;
                s.at_enable = at_enable;
                s.previous = previous;
                let idle = s.state == State::Idle || s.state == State::Off;
                if !enabled && idle {
                    self.set_state(&id, State::Off, None, &mut out);
                } else if enabled && idle && at_enable && !self.safe_mode {
                    self.launch(&id, &mut out);
                }
            }
            Ev::Started { id, instance } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.instance = instance;
                    s.ended = false;
                    s.ready_by = Some(now + READY_TIMEOUT);
                }
            }
            Ev::LaunchFailed { id, reason } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.ready_by = None;
                }
                self.set_state(&id, State::InstallFailed, Some(reason), &mut out);
                // It came from launching: "Couldn't start <Module>" (UX plan, #983).
                publish(&mut out, format!("module.{id}.failed"), json!("launch"));
            }
            Ev::Adopted { id, instance } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.instance = instance;
                    s.ended = false;
                    s.adopt_by = Some(now + GRACE);
                    // "Starting…" until it answers; no crash bar (UX plan, hub restart).
                    self.set_state(&id, State::Starting, None, &mut out);
                }
            }
            Ev::Ready { id } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    if s.ended {
                        return out;
                    }
                    s.ready_by = None;
                    s.adopt_by = None;
                    self.set_state(&id, State::Running, None, &mut out);
                }
            }
            Ev::Exited { id, instance, code } => {
                let Some(s) = self.slots.get_mut(&id) else {
                    return out;
                };
                if instance != s.instance {
                    return out;
                }
                let was_ended = std::mem::replace(&mut s.ended, true);
                s.exit_by = None;
                s.ready_by = None;
                let adopting = s.adopt_by.take().is_some();
                if s.then_launch {
                    self.launch(&id, &mut out);
                } else if was_ended {
                    // Its end was already counted (a fault, a ready timeout, a pipe close).
                } else if code == 0 {
                    // A stop, not a crash (§7.7). A module taken back that gave up waiting for
                    // the hub starts fresh (§7.9).
                    let s = &self.slots[&id];
                    if adopting && s.enabled && s.at_enable && !self.safe_mode {
                        self.launch(&id, &mut out);
                    } else if s.state != State::Off {
                        self.set_state(&id, State::Idle, None, &mut out);
                    }
                } else {
                    self.crash(&id, now, &mut out);
                }
            }
            Ev::PipeGone { id } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    if !s.ended && s.exit_by.is_none() && !s.then_launch {
                        s.exit_by = Some(now + EXIT_AFTER_PIPE);
                    }
                }
            }
            Ev::Fault { id } => {
                // One crash (§7.7); the router already closed the connection.
                if self.slots.get(&id).is_some_and(|s| !s.ended) {
                    self.slots.get_mut(&id).unwrap().ended = true;
                    out.push(Cmd::Kill { id: id.clone() });
                    self.crash(&id, now, &mut out);
                }
            }
            Ev::Refused { id } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.ended = true;
                    s.ready_by = None;
                    s.adopt_by = None;
                    s.exit_by = None;
                    out.push(Cmd::Kill { id: id.clone() });
                    self.set_state(
                        &id,
                        State::InstallFailed,
                        Some(words::FILES_DONT_MATCH.into()),
                        &mut out,
                    );
                    publish(&mut out, format!("module.{id}.failed"), json!("launch"));
                }
            }
            Ev::Activate { id } => {
                if self.safe_mode {
                    return out;
                }
                if let Some(s) = self.slots.get(&id) {
                    if s.enabled && s.state == State::Idle {
                        self.launch(&id, &mut out);
                    }
                }
            }
            Ev::StartAgain { id } => self.start_by_hand(&id, false, &mut out),
            Ev::StartDefaults { id } => self.start_by_hand(&id, true, &mut out),
            Ev::GoBack { id } => {
                let stopped = self.slots.get(&id).is_some_and(|s| {
                    s.state == State::CrashStopped && s.ended && s.previous.is_some()
                });
                if stopped {
                    // The rolled-back-from version isn't offered again: the ladder ends at
                    // its last step if this one stops too.
                    self.slots.get_mut(&id).unwrap().previous = None;
                    out.push(Cmd::RollBack { id: id.clone() });
                    self.start_by_hand(&id, false, &mut out);
                }
            }
            Ev::Updated {
                id,
                version,
                previous,
            } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.updated_at = Some(now);
                    s.updated_to = version;
                    s.previous = Some(previous);
                }
            }
            Ev::Enable { id } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.enabled = true;
                    if s.state == State::Off {
                        let start = s.at_enable && !self.safe_mode;
                        if start {
                            self.launch(&id, &mut out);
                        } else {
                            self.set_state(&id, State::Idle, None, &mut out);
                        }
                    }
                }
            }
            Ev::Disable { id } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.enabled = false;
                    s.restart_at = None;
                    s.ready_by = None;
                    s.adopt_by = None;
                    s.exit_by = None;
                    s.then_launch = false;
                    if !s.ended {
                        s.ended = true;
                        out.push(Cmd::Kill { id: id.clone() });
                    }
                    self.set_state(&id, State::Off, None, &mut out);
                }
            }
            Ev::StartNormally => {
                self.safe_mode = false;
                let waiting: Vec<String> = self
                    .slots
                    .iter()
                    .filter(|(_, s)| s.enabled && s.at_enable && s.state == State::Idle)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in waiting {
                    self.note(&id, now, "safe mode ended: started normally".into());
                    self.launch(&id, &mut out);
                }
            }
        }
        out
    }

    /// "Start again" / "Start with default settings": the count starts fresh (§7.8, UX ladder).
    fn start_by_hand(&mut self, id: &str, defaults: bool, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        if !matches!(
            s.state,
            State::CrashStopped | State::InstallFailed | State::Idle | State::Restarting
        ) {
            return;
        }
        s.crashes.clear();
        s.run_restarts = 0;
        s.reset_first = defaults;
        if s.ended {
            self.launch(id, out);
        } else {
            s.then_launch = true;
            out.push(Cmd::Kill { id: id.into() });
        }
    }

    /// Counts a crash at `now` and restarts with backoff, or stops after five in ten minutes.
    fn crash(&mut self, id: &str, now: Instant, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        s.ended = true;
        s.ready_by = None;
        s.exit_by = None;
        s.crashes.retain(|t| now - *t < WINDOW);
        if s.crashes.is_empty() {
            s.run_restarts = 0;
        }
        s.crashes.push_back(now);
        let k = s.crashes.len();
        let name = s.name.clone();
        self.note(id, now, format!("crash {k} of {CRASH_LIMIT} in 10 min"));
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        if k >= CRASH_LIMIT {
            s.restart_at = None;
            s.stops += 1;
            let rollback = s.previous.is_some()
                && s.updated_at
                    .is_some_and(|u| now.saturating_duration_since(u) < WINDOW);
            if rollback {
                // §7.8: back to the previous version by itself, and say so.
                let new_prev = s.previous.take().unwrap();
                let line =
                    format!("rolled back to {new_prev} by itself (crashed after the update)");
                s.updated_at = None;
                s.crashes.clear();
                s.run_restarts = 0;
                out.push(Cmd::RollBack { id: id.into() });
                self.note(id, now, line);
                let Some(s) = self.slots.get_mut(id) else {
                    return;
                };
                let (title, body) = words::rollback(&name, &new_prev, &s.updated_to);
                out.push(Cmd::Notify {
                    id: id.into(),
                    title,
                    body,
                    actions: vec![words::OK.into(), words::OPEN_LOG.into()],
                });
                self.launch(id, out);
                return;
            }
            let stops = s.stops;
            let previous = s.previous.clone();
            let step = words::Recovery::for_stops(stops, previous.is_some()).id();
            self.note(
                id,
                now,
                format!("crash-stopped (stop {stops}); next step {step}"),
            );
            self.set_state(
                id,
                State::CrashStopped,
                Some(words::stopped_body(&name)),
                out,
            );
            publish(
                out,
                format!("module.{id}.recovery"),
                json!(words::Recovery::for_stops(stops, previous.is_some()).id()),
            );
            // The version "Go back to <previous>" names.
            publish(
                out,
                format!("module.{id}.previous"),
                previous.map_or(Value::Null, Value::String),
            );
            if self.notified.insert(id.into()) {
                let (title, body, actions) = words::stopped_notification(&name);
                out.push(Cmd::Notify {
                    id: id.into(),
                    title,
                    body,
                    actions,
                });
            }
            return;
        }
        s.run_restarts += 1;
        let delay = backoff(s.run_restarts);
        s.restart_at = Some(now + delay);
        self.set_state(
            id,
            State::Restarting,
            Some(words::crash_bar(&name, delay.as_secs(), k).0),
            out,
        );
        publish(
            out,
            format!("module.{id}.restart"),
            json!({"delay_s": delay.as_secs(), "crash": k, "of": CRASH_LIMIT}),
        );
    }

    /// Runs the timers due at `now`.
    pub fn tick(&mut self, now: Instant) -> Vec<Cmd> {
        let mut out = Vec::new();
        let ids: Vec<String> = self.slots.keys().cloned().collect();
        for id in ids {
            let s = self.slots.get_mut(&id).unwrap();
            if s.restart_at.is_some_and(|t| t <= now) {
                s.restart_at = None;
                if s.enabled {
                    publish(&mut out, format!("module.{id}.restart"), Value::Null);
                    self.launch(&id, &mut out);
                }
                continue;
            }
            if s.ready_by.is_some_and(|t| t <= now) || s.exit_by.is_some_and(|t| t <= now) {
                // No module.ready in 30 s (§7.5), or the pipe closed and it didn't exit: a crash.
                let line = if s.ready_by.is_some_and(|t| t <= now) {
                    "no module.ready within 30 s"
                } else {
                    "pipe closed; the process didn't exit within 5 s"
                };
                self.note(&id, now, line.into());
                let s = self.slots.get_mut(&id).unwrap();
                s.ready_by = None;
                s.exit_by = None;
                out.push(Cmd::Kill { id: id.clone() });
                self.crash(&id, now, &mut out);
                continue;
            }
            if s.adopt_by.is_some_and(|t| t <= now) {
                // Not back within the grace period: started fresh, not a crash (§7.9).
                s.adopt_by = None;
                s.ended = true;
                out.push(Cmd::Kill { id: id.clone() });
                if s.enabled && !self.safe_mode {
                    self.launch(&id, &mut out);
                }
            }
        }
        out
    }
}

/// The history line an event leaves, for the events worth one (Copy diagnostics).
fn history_line(ev: &Ev, p: &Policy) -> Option<(String, String)> {
    let state = |id: &str| p.slots.get(id).map(|s| s.state);
    Some(match ev {
        Ev::Started { id, instance } => (id.clone(), format!("started instance {instance}")),
        Ev::LaunchFailed { id, reason } => (id.clone(), format!("launch failed: {reason}")),
        Ev::Adopted { id, instance } => (
            id.clone(),
            format!("taken back instance {instance} after a hub restart"),
        ),
        Ev::Ready { id } => (id.clone(), "module.ready".into()),
        Ev::Exited { id, instance, code } => {
            let current = p.slots.get(id).is_some_and(|s| s.instance == *instance);
            if !current {
                return None;
            }
            (id.clone(), format!("exited with code {}", exit_code(*code)))
        }
        Ev::PipeGone { id } => (id.clone(), "router connection closed".into()),
        Ev::Fault { id } => (id.clone(), "channel closed: rpc.frame-too-large".into()),
        Ev::Refused { id } => (
            id.clone(),
            "initialize refused: pid or exe hash check".into(),
        ),
        Ev::StartAgain { id } => (id.clone(), "step tried: start-again".into()),
        Ev::StartDefaults { id } => (id.clone(), "step tried: defaults".into()),
        Ev::GoBack { id } => (id.clone(), "step tried: go-back".into()),
        Ev::Updated {
            id,
            version,
            previous,
        } => (id.clone(), format!("updated {previous} -> {version}")),
        Ev::Enable { id } => (id.clone(), "enabled".into()),
        Ev::Disable { id } if state(id) == Some(State::CrashStopped) => {
            (id.clone(), "step tried: turn-off".into())
        }
        Ev::Disable { id } => (id.clone(), "disabled".into()),
        Ev::Known { .. } | Ev::Activate { .. } | Ev::StartNormally => return None,
    })
}

/// An exit code as decimal and, for Windows NTSTATUS-style codes, hex (`3221225477 (0xC0000005)`).
pub fn exit_code(code: i64) -> String {
    let u = code as u32;
    if code < 0 || u >= 0x8000_0000 {
        format!("{u} (0x{u:08X})")
    } else {
        code.to_string()
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    /// Copy diagnostics' history: exit codes (hex for NTSTATUS), crash numbers, the stop and
    /// the ladder steps tried, in order.
    #[test]
    fn the_history_has_exit_codes_crashes_and_steps() {
        let mut p = Policy::new();
        let t0 = Instant::now();
        let id = "m.art".to_string();
        p.handle(
            Ev::Known {
                id: id.clone(),
                name: "Art".into(),
                enabled: true,
                at_enable: true,
                previous: None,
            },
            t0,
        );
        for n in 1..=5u64 {
            let t = t0 + Duration::from_secs(n);
            p.handle(
                Ev::Started {
                    id: id.clone(),
                    instance: n,
                },
                t,
            );
            p.handle(
                Ev::Exited {
                    id: id.clone(),
                    instance: n,
                    code: 0xC000_0005,
                },
                t,
            );
        }
        p.handle(
            Ev::StartAgain { id: id.clone() },
            t0 + Duration::from_secs(9),
        );
        let h: Vec<String> = p.history(&id).into_iter().map(|(_, l)| l).collect();
        assert!(
            h.contains(&"exited with code 3221225477 (0xC0000005)".to_string()),
            "{h:?}"
        );
        assert!(h.contains(&"crash 5 of 5 in 10 min".to_string()), "{h:?}");
        assert!(
            h.iter()
                .any(|l| l.starts_with("crash-stopped (stop 1); next step start-again")),
            "{h:?}"
        );
        assert_eq!(
            h.last().map(String::as_str),
            Some("step tried: start-again")
        );
        assert_eq!(p.stops(&id), 1);
        assert_eq!(exit_code(1), "1");
        assert_eq!(exit_code(-1), "4294967295 (0xFFFFFFFF)");
    }
}

#[cfg(test)]
mod safe_mode_tests {
    use super::*;

    fn known(id: &str, enabled: bool, at_enable: bool) -> Ev {
        Ev::Known {
            id: id.into(),
            name: id.into(),
            enabled,
            at_enable,
            previous: None,
        }
    }

    fn launches(cmds: &[Cmd]) -> Vec<String> {
        cmds.iter()
            .filter_map(|c| match c {
                Cmd::Launch { id } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    /// Safe mode starts nothing; Start normally starts each enabled module that starts at
    /// enable, once, and nothing that is off or waits for a call.
    #[test]
    fn start_normally_starts_the_enabled_modules() {
        let mut p = Policy::new();
        p.safe_mode = true;
        let t = Instant::now();
        let mut cmds = Vec::new();
        for ev in [
            known("m.art", true, true),
            known("m.beat", true, true),
            known("m.call", true, false),
            known("m.off", false, true),
        ] {
            cmds.extend(p.handle(ev, t));
        }
        assert!(launches(&cmds).is_empty(), "safe mode started {cmds:?}");
        assert!(p
            .handle(
                Ev::Activate {
                    id: "m.call".into()
                },
                t
            )
            .is_empty());
        let cmds = p.handle(Ev::StartNormally, t);
        assert_eq!(launches(&cmds), ["m.art", "m.beat"]);
        assert!(!p.safe_mode);
        assert_eq!(p.state("m.art"), Some(State::Starting));
        // Pressed again (the tray and the banner): nothing starts twice.
        assert!(launches(&p.handle(Ev::StartNormally, t)).is_empty());
        // Activation works again outside safe mode.
        assert_eq!(
            launches(&p.handle(
                Ev::Activate {
                    id: "m.call".into()
                },
                t
            )),
            ["m.call"]
        );
        assert!(p
            .history("m.art")
            .iter()
            .any(|(_, l)| l == "safe mode ended: started normally"));
    }

    /// Turn on in safe mode (#1033): saved on, not started, no state id (the row reads "On ·
    /// not started in safe mode"); Start normally then starts it.
    #[test]
    fn turn_on_in_safe_mode_waits_for_start_normally() {
        let mut p = Policy::new();
        p.safe_mode = true;
        let t = Instant::now();
        p.handle(known("m.oak", false, true), t);
        assert_eq!(p.state("m.oak"), Some(State::Off));
        let cmds = p.handle(Ev::Enable { id: "m.oak".into() }, t);
        assert!(launches(&cmds).is_empty(), "safe mode started {cmds:?}");
        assert_eq!(p.state("m.oak"), Some(State::Idle));
        assert_eq!(State::Idle.id(), None);
        assert_eq!(launches(&p.handle(Ev::StartNormally, t)), ["m.oak"]);
    }
}
