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
    /// Its publisher withdrew the installed version (a catalog revoked it, §12.4): stopped, not
    /// started again until another version is installed or gone back to (§7.8 v1.9, #1026).
    Revoked,
    /// Running, but a module it needs (`requires` or a `channel.open:` provider, §4.4) is
    /// down: "Waiting for <root>" (§7.8). Nothing is restarted or stopped for it.
    Waiting,
    /// Its manifest's `hub` isn't served by this hub (§7.8): not started until Hideout is
    /// updated or the previous version is gone back to.
    NeedsHub,
    /// The panel guard's breadcrumb rule (§5.5): two hub restarts while drawing its panel.
    /// Not started until Start again or Go back.
    Quarantined,
    /// A planned swap (§7.6): drain answered done; the new instance is starting.
    Updating,
    /// An update waits because drain answered busy (§7.6): Force or Not now.
    UpdateBusy,
    /// An update that adds permissions waits for Review (§6.4); the old version keeps running.
    UpdateWaiting,
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
            State::Revoked => "revoked",
            State::Waiting => "waiting",
            State::NeedsHub => "needs-hub",
            State::Quarantined => "quarantined",
            State::Updating => "updating",
            State::UpdateBusy => "update-busy",
            State::UpdateWaiting => "update-waiting",
        })
    }

    /// Down for its dependents (§4.4 "provider gone"): they read Waiting for it. A module
    /// starting, updating or waiting for a call (idle) is not down.
    pub fn down(self) -> bool {
        matches!(
            self,
            State::Off
                | State::Restarting
                | State::CrashStopped
                | State::InstallFailed
                | State::Revoked
                | State::NeedsHub
                | State::Quarantined
        )
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
    /// An accepted catalog revoked the installed `version` (§12.4; found at a launch, at hub
    /// start or when a catalog is accepted). `go_back` is the kept previous version that isn't
    /// withdrawn itself, for "Go back to <version>". The router has already ended its leases.
    Revoked {
        id: String,
        version: String,
        go_back: Option<String>,
    },
    /// The installed version is no longer withdrawn: another one was installed, or the catalog
    /// no longer revokes it. It starts as an enabled module does.
    Cleared {
        id: String,
    },
    /// **Check for updates** on the withdrawn card: the check starts ([`Cmd::CheckUpdates`]).
    Check {
        id: String,
    },
    /// The check ended: `reached` the source or not. A newer version that isn't withdrawn comes
    /// as [`Ev::Cleared`] (the update path), not here.
    Checked {
        id: String,
        reached: bool,
    },
    /// A round of finding the module that breaks Hideout (§7.10, #1033): in safe mode only
    /// these modules start (`None`: none, the search is over). The ones running that aren't in
    /// it stop for the session; nothing saved changes.
    Round {
        on: Option<BTreeSet<String>>,
    },
    /// The modules `id` needs (§4.4): its `requires` ids and the modules its `channel.open:`
    /// permissions name. The root-cause graph for Waiting (§7.8).
    Depends {
        id: String,
        on: BTreeSet<String>,
    },
    /// The launch found that its manifest's `hub` isn't served (`manifest.hub`, §7.8).
    NeedsHub {
        id: String,
    },
    /// The panel host started its panels quarantined (§5.5 breadcrumb rule).
    Quarantine {
        id: String,
    },
    /// The update step installed `version` beside the running `previous` (§7.4). `asks`: the
    /// permissions it adds, in plain words (`None`: it adds none).
    UpdateStaged {
        id: String,
        version: String,
        previous: String,
        asks: Option<String>,
    },
    /// **Review** on Update waiting for you, approved (the grants are already recorded).
    Review {
        id: String,
    },
    /// **Not now** on Update waiting for you or Update ready: the old version keeps running.
    NotNow {
        id: String,
    },
    /// **Force** on Update ready.
    Force {
        id: String,
    },
    /// The answer to [`Cmd::Drain`]: `busy` with its reason, or done (`None`). An error answer
    /// (no drain handler, disconnected, timed out) counts as done.
    Drained {
        id: String,
        busy: Option<String>,
    },
    /// `module.idle` (§7.6): the work a busy drain waited for has ended.
    Idle {
        id: String,
    },
}

/// Where a waiting update is (§7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Update waiting for you: Review or Not now.
    Review,
    /// Not now: the old version keeps running; the next start runs the new one.
    Deferred,
    /// `module.drain` sent.
    Draining,
    /// `module.drain` sent again by Force: the swap goes on whatever it answers.
    Forced,
    /// Drain answered busy: waits for `module.idle` or Force (never a timer, UX6).
    Busy,
    /// The old instance was asked to exit; the new one starts on its exit.
    Swapping,
}

#[derive(Clone, Debug)]
struct Update {
    asks: Option<String>,
    phase: Phase,
    /// The version that runs staged again (`version == previous`: the lab's stand-in update,
    /// #1096): nothing to go back to, so no `previous`, update time or automatic rollback.
    restage: bool,
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
    /// Fetch the module's source's catalog now (one fetch, no retry loop) and answer with
    /// [`Ev::Cleared`] or [`Ev::Checked`] (#1026).
    CheckUpdates { id: String },
    /// Remember that the withdrawal of `id@version` was notified, in the hub's own state, so
    /// no later sign-in notifies it again (decision 3, 11/10).
    RememberNotified { id: String, version: String },
    /// Send `module.drain {reason}` (§7.6): `"update"`, or `"force"` after Force. Answer with
    /// [`Ev::Drained`].
    Drain { id: String, force: bool },
    /// Send `module.exit` (§7.6): the old instance finishes and exits; its exit starts the new
    /// one. Ended through its job after [`EXIT_AFTER_PIPE`] if it doesn't.
    Exit { id: String },
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
    /// The installed version its publisher withdrew: it doesn't start (#1026).
    withdrawn: Option<String>,
    /// The version the withdrawn card's Go back names (kept, not withdrawn itself).
    go_back: Option<String>,
    /// The modules it needs (§4.4): `requires` and `channel.open:` providers.
    deps: BTreeSet<String>,
    /// The root module its Waiting names (`module.<id>.waiting`).
    waiting_for: Option<String>,
    /// Its panels are quarantined (§5.5): it doesn't start until Start again or Go back.
    quarantined: bool,
    /// Its manifest needs a newer hub (§7.8): it doesn't start until Go back.
    needs_hub: bool,
    /// An update waiting to be swapped in (§7.6).
    update: Option<Update>,
}

/// The supervisor's state for every module.
#[derive(Debug, Default)]
pub struct Policy {
    slots: BTreeMap<String, Slot>,
    /// Modules already notified this sign-in (§7.7).
    notified: BTreeSet<String>,
    /// Withdrawn versions already notified, `(id, version)`: once per version, ever (seeded
    /// from the hub's state with [`Policy::already_notified`]).
    notified_withdrawn: BTreeSet<(String, String)>,
    /// Safe mode (§7.10): nothing starts by itself.
    pub safe_mode: bool,
    /// The round of a search (§7.10, #1033): in safe mode these modules start anyway.
    pub round: Option<BTreeSet<String>>,
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

    /// Safe mode holds `id` back: on, and not in the search's round.
    fn held(&self, id: &str) -> bool {
        self.safe_mode && !self.round.as_ref().is_some_and(|r| r.contains(id))
    }

    /// The modules **Start normally** would start (enabled, starting at enable, idle): the
    /// search's candidates (#1033). Id and name.
    pub fn startable(&self) -> Vec<(String, String)> {
        self.slots
            .iter()
            .filter(|(_, s)| s.enabled && s.at_enable && s.state == State::Idle)
            .map(|(id, s)| (id.clone(), s.name.clone()))
            .collect()
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

    /// A withdrawal notified before (by an earlier hub): it isn't notified again.
    pub fn already_notified(&mut self, id: &str, version: &str) {
        self.notified_withdrawn.insert((id.into(), version.into()));
    }

    /// The withdrawn version of `id`, while it is withdrawn.
    pub fn withdrawn(&self, id: &str) -> Option<&str> {
        self.slots.get(id).and_then(|s| s.withdrawn.as_deref())
    }

    /// `id` left the inventory (#1054): its slot (state, crashes, a waiting restart) and its
    /// history go, as if the hub had started without it. Answers whether it was known. The
    /// withdrawn versions already notified stay, so a reinstall isn't notified again.
    pub fn forget(&mut self, id: &str) -> bool {
        self.notified.remove(id);
        self.history.remove(id);
        if let Some(r) = self.round.as_mut() {
            r.remove(id);
        }
        self.slots.remove(id).is_some()
    }

    /// The ids of every module the policy knows.
    pub fn known(&self) -> Vec<String> {
        self.slots.keys().cloned().collect()
    }

    /// Crash stops this sign-in (the recovery ladder's count).
    pub fn stops(&self, id: &str) -> u32 {
        self.slots.get(id).map_or(0, |s| s.stops)
    }

    pub fn state(&self, id: &str) -> Option<State> {
        self.slots.get(id).map(|s| s.state)
    }

    /// Whether the policy holds `id` as on (`None`: not known).
    pub fn enabled(&self, id: &str) -> Option<bool> {
        self.slots.get(id).map(|s| s.enabled)
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
        if state != State::Waiting && s.waiting_for.take().is_some() {
            publish(out, format!("module.{id}.waiting"), Value::Null);
        }
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
        if s.withdrawn.is_some() {
            // A withdrawn version never starts: whatever asked (Turn on from a file, an
            // activation, a restart timer), it stays Withdrawn (#1026).
            s.restart_at = None;
            s.then_launch = false;
            if s.enabled {
                self.show_withdrawn(id, out);
            }
            return;
        }
        if s.quarantined || s.needs_hub {
            // Only Start again (quarantined) or Go back starts it (§5.5, §7.8).
            s.restart_at = None;
            s.then_launch = false;
            if s.enabled {
                self.show_blocked(id, out);
            }
            return;
        }
        s.restart_at = None;
        s.exit_by = None;
        s.adopt_by = None;
        s.then_launch = false;
        let reset = std::mem::take(&mut s.reset_first);
        let swapping = s
            .update
            .as_ref()
            .is_some_and(|u| u.phase == Phase::Swapping);
        if !swapping {
            // A planned swap keeps "Updating…" until the new instance is ready (§7.6).
            self.set_state(id, State::Starting, None, out);
        }
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
        let mut out = self.handle_one(ev, now);
        self.refresh_waiting(&mut out);
        out
    }

    /// The first module down on the way from `id` through what it needs (§4.4), and below that
    /// the deepest one down: the root cause, named once (§7.8). `None`: nothing it needs is
    /// down.
    fn root(&self, id: &str, seen: &mut BTreeSet<String>) -> Option<String> {
        if !seen.insert(id.to_string()) {
            return None; // cycles are refused at enable; never loop on one anyway
        }
        let deps = &self.slots.get(id)?.deps;
        let down = deps
            .iter()
            .find(|d| self.slots.get(*d).is_some_and(|s| s.state.down()));
        if let Some(d) = down {
            return Some(self.root(d, seen).unwrap_or_else(|| d.clone()));
        }
        deps.iter().find_map(|d| self.root(d, seen))
    }

    /// Running modules with something they need down read Waiting for its root; once it is
    /// back they read running again. Nothing is started, stopped or counted (§4.4).
    fn refresh_waiting(&mut self, out: &mut Vec<Cmd>) {
        let ids: Vec<String> = self
            .slots
            .iter()
            .filter(|(_, s)| !s.deps.is_empty())
            .filter(|(_, s)| matches!(s.state, State::Running | State::Waiting))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let root = self.root(&id, &mut BTreeSet::new());
            let s = &self.slots[&id];
            match root {
                Some(r) if s.state != State::Waiting || s.waiting_for.as_ref() != Some(&r) => {
                    let name = self.slots[&r].name.clone();
                    self.set_state(&id, State::Waiting, Some(words::waiting_reason(&name)), out);
                    self.slots.get_mut(&id).unwrap().waiting_for = Some(r.clone());
                    publish(out, format!("module.{id}.waiting"), json!(r));
                }
                None if s.state == State::Waiting => {
                    self.set_state(&id, State::Running, None, out);
                }
                _ => {}
            }
        }
    }

    fn handle_one(&mut self, ev: Ev, now: Instant) -> Vec<Cmd> {
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
                    withdrawn: None,
                    go_back: None,
                    deps: BTreeSet::new(),
                    waiting_for: None,
                    quarantined: false,
                    needs_hub: false,
                    update: None,
                });
                s.name = name;
                s.enabled = enabled;
                s.at_enable = at_enable;
                s.previous = previous;
                let idle = s.state == State::Idle || s.state == State::Off;
                if !enabled && idle {
                    self.set_state(&id, State::Off, None, &mut out);
                } else if enabled && idle && at_enable && !self.held(&id) {
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
                    match s.update.take() {
                        // Restarted (a crash) before Review: the new version runs on the old
                        // grants; Update waiting for you again, and Review swaps it once more
                        // so it connects with the new ones.
                        Some(u) if u.asks.is_some() && u.phase != Phase::Swapping => {
                            let asks = u.asks.clone();
                            s.update = Some(Update {
                                phase: Phase::Review,
                                ..u
                            });
                            self.set_state(&id, State::UpdateWaiting, asks, &mut out);
                        }
                        u => {
                            if u.is_some_and(|u| u.phase != Phase::Swapping && !u.restage) {
                                // A restart ran the new version: the update happened now.
                                s.updated_at = Some(now);
                            }
                            self.set_state(&id, State::Running, None, &mut out);
                        }
                    }
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
                    if adopting && s.enabled && s.at_enable && !self.held(&id) {
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
                if self.held(&id) {
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
            Ev::GoBack { id } if self.state(&id) == Some(State::Revoked) => {
                // The withdrawn card's Go back: the user's click, never automatic. The
                // registry refuses a revoked version, and the launch checks it again.
                let s = self.slots.get_mut(&id).unwrap();
                if s.ended && s.go_back.take().is_some() {
                    s.withdrawn = None;
                    s.previous = None;
                    s.crashes.clear();
                    s.run_restarts = 0;
                    publish(&mut out, format!("module.{id}.check"), Value::Null);
                    out.push(Cmd::RollBack { id: id.clone() });
                    self.launch(&id, &mut out);
                }
            }
            Ev::GoBack { id } => {
                let stopped = self.slots.get(&id).is_some_and(|s| {
                    matches!(
                        s.state,
                        State::CrashStopped | State::NeedsHub | State::Quarantined
                    ) && s.ended
                        && s.previous.is_some()
                });
                if stopped {
                    // The rolled-back-from version isn't offered again: the ladder ends at
                    // its last step if this one stops too.
                    let s = self.slots.get_mut(&id).unwrap();
                    s.previous = None;
                    s.needs_hub = false;
                    s.quarantined = false;
                    out.push(Cmd::RollBack { id: id.clone() });
                    self.fresh_start(&id, false, &mut out);
                }
            }
            Ev::Depends { id, on } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.deps = on;
                    s.deps.remove(&id);
                }
            }
            Ev::NeedsHub { id } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.needs_hub = true;
                    s.restart_at = None;
                    s.ready_by = None;
                    s.then_launch = false;
                    s.update = None;
                    self.show_blocked(&id, &mut out);
                }
            }
            Ev::Quarantine { id } => {
                if let Some(s) = self.slots.get_mut(&id) {
                    s.quarantined = true;
                    s.restart_at = None;
                    s.ready_by = None;
                    s.adopt_by = None;
                    s.exit_by = None;
                    s.then_launch = false;
                    s.update = None;
                    if !s.ended {
                        // Its panels come with its instance: it doesn't run while they are
                        // quarantined (not a crash).
                        s.ended = true;
                        out.push(Cmd::Kill { id: id.clone() });
                    }
                    if s.enabled {
                        self.show_blocked(&id, &mut out);
                    }
                }
            }
            Ev::UpdateStaged {
                id,
                version,
                previous,
                asks,
            } => self.update_staged(&id, version, previous, asks, now, &mut out),
            Ev::Review { id } => {
                if let Some(u) = self.waiting_update(&id, &[Phase::Review, Phase::Deferred]) {
                    u.asks = None;
                    u.phase = Phase::Draining;
                    out.push(Cmd::Drain { id, force: false });
                }
            }
            Ev::NotNow { id } => {
                if let Some(u) = self.waiting_update(&id, &[Phase::Review, Phase::Busy]) {
                    u.phase = Phase::Deferred;
                    self.set_state(&id, State::Running, None, &mut out);
                }
            }
            Ev::Force { id } => {
                if let Some(u) = self.waiting_update(&id, &[Phase::Busy]) {
                    u.phase = Phase::Forced;
                    out.push(Cmd::Drain { id, force: true });
                }
            }
            Ev::Idle { id } => {
                if let Some(u) = self.waiting_update(&id, &[Phase::Busy]) {
                    u.phase = Phase::Draining;
                    out.push(Cmd::Drain { id, force: false });
                }
            }
            Ev::Drained { id, busy } => {
                let Some(u) = self.waiting_update(&id, &[Phase::Draining, Phase::Forced]) else {
                    return out;
                };
                match busy {
                    Some(reason) if u.phase == Phase::Draining => {
                        u.phase = Phase::Busy;
                        self.set_state(
                            &id,
                            State::UpdateBusy,
                            Some(words::update_busy(&reason)),
                            &mut out,
                        );
                    }
                    _ => self.swap(&id, now, &mut out),
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
                let held = self.held(&id);
                if let Some(s) = self.slots.get_mut(&id) {
                    s.enabled = true;
                    if s.state == State::Off {
                        let start = s.at_enable && !held;
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
                    s.update = None;
                    if !s.ended {
                        s.ended = true;
                        out.push(Cmd::Kill { id: id.clone() });
                    }
                    self.set_state(&id, State::Off, None, &mut out);
                }
            }
            Ev::Revoked {
                id,
                version,
                go_back,
            } => self.revoked(&id, version, go_back, &mut out),
            Ev::Cleared { id } => {
                let Some(s) = self.slots.get_mut(&id) else {
                    return out;
                };
                if s.withdrawn.take().is_none() {
                    return out;
                }
                s.go_back = None;
                s.crashes.clear();
                s.run_restarts = 0;
                let start = s.enabled && s.at_enable && !self.safe_mode;
                let was = s.state;
                publish(&mut out, format!("module.{id}.check"), Value::Null);
                // The router's notice ends too (also when the same version is no longer
                // revoked, which no new connection would clear).
                publish(&mut out, format!("module.{id}.revoked"), Value::Null);
                if was == State::Revoked {
                    publish(&mut out, format!("module.{id}.previous"), Value::Null);
                    if start {
                        self.launch(&id, &mut out);
                    } else {
                        self.set_state(&id, State::Idle, None, &mut out);
                    }
                }
            }
            Ev::Check { id } => {
                if self.slots.get(&id).is_some_and(|s| s.withdrawn.is_some()) {
                    publish(&mut out, format!("module.{id}.check"), json!("checking"));
                    out.push(Cmd::CheckUpdates { id });
                }
            }
            Ev::Checked { id, reached } => {
                if self.slots.get(&id).is_some_and(|s| s.withdrawn.is_some()) {
                    let v = if reached { "none" } else { "failed" };
                    publish(&mut out, format!("module.{id}.check"), json!(v));
                }
            }
            Ev::Round { on } => self.round(on, now, &mut out),
            Ev::StartNormally => {
                self.safe_mode = false;
                self.round = None;
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

    /// The installed version was withdrawn (§12.4, #1026): the module stops (the router ended
    /// its leases; the Job Object ends the process), its timers end, and it reads Withdrawn
    /// until another version is installed or gone back to. Files, data, settings and grants
    /// are kept, and it stays enabled, so a fixed version starts by itself. Turned off, it
    /// stays Off (the row says the version was withdrawn). One notification per withdrawn
    /// version, ever.
    fn revoked(&mut self, id: &str, version: String, go_back: Option<String>, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        let again = s.withdrawn.as_deref() == Some(version.as_str());
        s.withdrawn = Some(version.clone());
        s.go_back = go_back;
        s.restart_at = None;
        s.ready_by = None;
        s.adopt_by = None;
        s.exit_by = None;
        s.then_launch = false;
        s.reset_first = false;
        s.update = None;
        if !s.ended {
            s.ended = true;
            out.push(Cmd::Kill { id: id.into() });
        }
        if !s.enabled {
            return;
        }
        let name = s.name.clone();
        if again && s.state == State::Revoked {
            // Seen again (another catalog, a second look): Go back's version may have changed.
            let go_back = s.go_back.clone();
            publish(
                out,
                format!("module.{id}.previous"),
                go_back.map_or(Value::Null, Value::String),
            );
        }
        self.show_withdrawn(id, out);
        if self.notified_withdrawn.insert((id.into(), version.clone())) {
            let (title, body, actions) = words::withdrawn_notification(&name, &version);
            out.push(Cmd::Notify {
                id: id.into(),
                title,
                body,
                actions,
            });
            out.push(Cmd::RememberNotified {
                id: id.into(),
                version,
            });
        }
    }

    /// The state `revoked` with its facts: the reason line, Go back's version, and the crash
    /// bar's, ladder's and launch failure's facts cleared.
    fn show_withdrawn(&mut self, id: &str, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get(id) else {
            return;
        };
        if s.state == State::Revoked {
            return;
        }
        let go_back = s.go_back.clone();
        self.set_state(id, State::Revoked, Some(words::WITHDRAWN_LINE.into()), out);
        for k in ["restart", "recovery", "failed"] {
            publish(out, format!("module.{id}.{k}"), Value::Null);
        }
        publish(
            out,
            format!("module.{id}.previous"),
            go_back.map_or(Value::Null, Value::String),
        );
    }

    /// A search's round (#1033): the modules out of it stop (not a crash, not saved), the
    /// enabled ones in it that start at enable start.
    fn round(&mut self, on: Option<BTreeSet<String>>, now: Instant, out: &mut Vec<Cmd>) {
        self.round = on;
        let ids: Vec<String> = self.slots.keys().cloned().collect();
        for id in ids {
            let held = self.held(&id);
            let in_round = self.round.as_ref().is_some_and(|r| r.contains(&id));
            let s = self.slots.get_mut(&id).unwrap();
            if held && s.enabled && !matches!(s.state, State::Off | State::InstallFailed) {
                let live = !s.ended || s.restart_at.is_some();
                s.restart_at = None;
                s.ready_by = None;
                s.adopt_by = None;
                s.exit_by = None;
                s.then_launch = false;
                if !s.ended {
                    s.ended = true;
                    out.push(Cmd::Kill { id: id.clone() });
                }
                if live {
                    publish(out, format!("module.{id}.restart"), Value::Null);
                    self.set_state(&id, State::Idle, None, out);
                    self.note(&id, now, "stopped: off for this round".into());
                }
            } else if in_round && s.enabled && s.at_enable && s.state == State::Idle {
                self.note(&id, now, "started: on in this round".into());
                self.launch(&id, out);
            }
        }
    }

    /// The update of a running module that hasn't been swapped in yet, in one of `phases`.
    fn waiting_update(&mut self, id: &str, phases: &[Phase]) -> Option<&mut Update> {
        let s = self.slots.get_mut(id)?;
        if s.ended {
            return None;
        }
        s.update.as_mut().filter(|u| phases.contains(&u.phase))
    }

    /// The update step installed a new version beside the running one (§7.4): it waits for
    /// Review if it adds permissions (§6.4, never a modal), else drains and swaps (§7.6). A
    /// module that isn't running runs the new version at its next start.
    fn update_staged(
        &mut self,
        id: &str,
        version: String,
        previous: String,
        asks: Option<String>,
        now: Instant,
        out: &mut Vec<Cmd>,
    ) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        // The same version again (the lab's stand-in, #1096): what was kept and when the last
        // real update ran stay as they were, so no "Go back to <same>" and no false rollback.
        let restage = version == previous;
        if !restage {
            s.previous = Some(previous);
            s.updated_to = version;
        }
        let live = !s.ended && matches!(s.state, State::Running | State::Waiting);
        if !live {
            s.update = None;
            if !restage {
                s.updated_at = Some(now);
            }
            return;
        }
        match asks {
            Some(a) => {
                s.update = Some(Update {
                    asks: Some(a.clone()),
                    phase: Phase::Review,
                    restage,
                });
                self.set_state(id, State::UpdateWaiting, Some(a), out);
            }
            None => {
                s.update = Some(Update {
                    asks: None,
                    phase: Phase::Draining,
                    restage,
                });
                out.push(Cmd::Drain {
                    id: id.into(),
                    force: false,
                });
            }
        }
    }

    /// Drain said done (or Force): "Updating…", the old instance exits, its exit starts the
    /// new version (stop then start, for both `swap` modes; see §7.6 v1.10).
    fn swap(&mut self, id: &str, now: Instant, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        let mut restage = false;
        if let Some(u) = s.update.as_mut() {
            u.phase = Phase::Swapping;
            restage = u.restage;
        }
        if !restage {
            s.updated_at = Some(now);
        }
        s.then_launch = true;
        s.exit_by = Some(now + EXIT_AFTER_PIPE);
        self.set_state(id, State::Updating, Some(words::PLACE_KEPT.into()), out);
        out.push(Cmd::Exit { id: id.into() });
    }

    /// NeedsHub or Quarantined with their facts: the card's Go back version, and the crash
    /// bar's, ladder's and launch failure's facts cleared.
    fn show_blocked(&mut self, id: &str, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get(id) else {
            return;
        };
        let (state, reason) = if s.needs_hub {
            (State::NeedsHub, None)
        } else {
            (
                State::Quarantined,
                Some(words::QUARANTINED_LINE.to_string()),
            )
        };
        if s.state == state {
            return;
        }
        let previous = s.previous.clone();
        self.set_state(id, state, reason, out);
        for k in ["restart", "recovery", "failed"] {
            publish(out, format!("module.{id}.{k}"), Value::Null);
        }
        publish(
            out,
            format!("module.{id}.previous"),
            previous.map_or(Value::Null, Value::String),
        );
    }

    /// "Start again" / "Start with default settings": the count starts fresh (§7.8, UX ladder).
    fn start_by_hand(&mut self, id: &str, defaults: bool, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        if !matches!(
            s.state,
            State::CrashStopped
                | State::InstallFailed
                | State::Idle
                | State::Restarting
                | State::Quarantined
        ) {
            return;
        }
        s.quarantined = false;
        self.fresh_start(id, defaults, out);
    }

    /// Starts `id` with a fresh count, after its instance is gone.
    fn fresh_start(&mut self, id: &str, defaults: bool, out: &mut Vec<Cmd>) {
        let Some(s) = self.slots.get_mut(id) else {
            return;
        };
        s.update = None;
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
        if s.update
            .as_ref()
            .is_some_and(|u| u.phase == Phase::Swapping)
        {
            // The new version crashed: the swap is over; the crash path takes it from here.
            s.update = None;
        }
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
            s.update = None;
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
            if s.then_launch && s.exit_by.is_some_and(|t| t <= now) {
                // Asked to exit for a swap or Start again and still there: ended through its
                // job, not a crash; its exit starts the next instance.
                s.exit_by = None;
                out.push(Cmd::Kill { id: id.clone() });
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
                let held = self.held(&id);
                let s = self.slots.get_mut(&id).unwrap();
                if s.enabled && !held {
                    self.launch(&id, &mut out);
                }
            }
        }
        self.refresh_waiting(&mut out);
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
        Ev::Revoked { id, version, .. } => {
            if p.slots.get(id).and_then(|s| s.withdrawn.as_deref()) == Some(version.as_str()) {
                return None;
            }
            (
                id.clone(),
                format!("stopped: its publisher withdrew {version} (catalog revocation)"),
            )
        }
        Ev::Cleared { id } => (id.clone(), "no longer withdrawn".into()),
        Ev::Check { id } => (id.clone(), "check for updates: started".into()),
        Ev::Checked { id, reached } => (
            id.clone(),
            if *reached {
                "check for updates: nothing newer".into()
            } else {
                "check for updates: couldn't reach the source".into()
            },
        ),
        Ev::NeedsHub { id } => (
            id.clone(),
            "not started: its manifest needs a newer hub (manifest.hub)".into(),
        ),
        Ev::Quarantine { id } => (
            id.clone(),
            "panels quarantined: two hub restarts while drawing them".into(),
        ),
        Ev::UpdateStaged {
            id,
            version,
            previous,
            asks,
        } => (
            id.clone(),
            format!(
                "update {previous} -> {version} staged{}",
                if asks.is_some() {
                    "; adds permissions, waits for Review"
                } else {
                    ""
                }
            ),
        ),
        Ev::Review { id } => (id.clone(), "update reviewed and approved".into()),
        Ev::NotNow { id } => (id.clone(), "update: not now".into()),
        Ev::Force { id } => (id.clone(), "update: forced".into()),
        Ev::Drained { id, busy } => (
            id.clone(),
            match busy {
                Some(r) => format!("drain: busy ({r})"),
                None => "drain: done".into(),
            },
        ),
        Ev::Idle { id } => (id.clone(), "module.idle".into()),
        Ev::Known { .. }
        | Ev::Activate { .. }
        | Ev::StartNormally
        | Ev::Round { .. }
        | Ev::Depends { .. } => return None,
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

    /// A search's rounds (#1033): in safe mode only the round's modules start; the next round
    /// stops the ones out of it (no crash, still enabled); `None` stops them all; activation
    /// stays held for modules out of the round; Start normally ends the round and starts all.
    #[test]
    fn rounds_start_their_half_and_stop_the_rest() {
        let mut p = Policy::new();
        p.safe_mode = true;
        let t = Instant::now();
        for id in ["m.a", "m.b", "m.c"] {
            assert!(launches(&p.handle(known(id, true, true), t)).is_empty());
        }
        p.handle(known("m.off", false, true), t);
        let set = |ids: &[&str]| Some(ids.iter().map(|s| s.to_string()).collect());
        assert_eq!(p.startable().len(), 3);
        let cmds = p.handle(
            Ev::Round {
                on: set(&["m.a", "m.b", "m.off"]),
            },
            t,
        );
        assert_eq!(launches(&cmds), ["m.a", "m.b"]);
        p.handle(
            Ev::Started {
                id: "m.a".into(),
                instance: 1,
            },
            t,
        );
        p.handle(Ev::Ready { id: "m.a".into() }, t);
        p.handle(
            Ev::Started {
                id: "m.b".into(),
                instance: 1,
            },
            t,
        );
        assert!(launches(&p.handle(Ev::Activate { id: "m.c".into() }, t)).is_empty());
        let cmds = p.handle(Ev::Round { on: set(&["m.c"]) }, t);
        assert_eq!(launches(&cmds), ["m.c"]);
        let kills: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                Cmd::Kill { id } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(kills, ["m.a", "m.b"]);
        assert_eq!(p.state("m.a"), Some(State::Idle));
        // The killed instance's exit is no crash.
        p.handle(
            Ev::Exited {
                id: "m.a".into(),
                instance: 1,
                code: 1,
            },
            t,
        );
        assert_eq!(p.crashes("m.a", t), 0);
        assert_eq!(p.state("m.a"), Some(State::Idle));
        p.handle(
            Ev::Started {
                id: "m.c".into(),
                instance: 1,
            },
            t,
        );
        let cmds = p.handle(Ev::Round { on: None }, t);
        assert!(launches(&cmds).is_empty());
        assert!(cmds.contains(&Cmd::Kill { id: "m.c".into() }));
        assert!(p.safe_mode && p.round.is_none());
        let cmds = p.handle(Ev::StartNormally, t);
        assert_eq!(launches(&cmds), ["m.a", "m.b", "m.c"]);
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

#[cfg(test)]
mod withdrawn_tests {
    use super::*;

    const ID: &str = "m.art";

    fn running(p: &mut Policy, t: Instant, previous: Option<&str>) {
        p.handle(
            Ev::Known {
                id: ID.into(),
                name: "Art".into(),
                enabled: true,
                at_enable: true,
                previous: previous.map(String::from),
            },
            t,
        );
        p.handle(
            Ev::Started {
                id: ID.into(),
                instance: 1,
            },
            t,
        );
        p.handle(Ev::Ready { id: ID.into() }, t);
    }

    fn revoked(go_back: Option<&str>) -> Ev {
        Ev::Revoked {
            id: ID.into(),
            version: "0.5.0".into(),
            go_back: go_back.map(String::from),
        }
    }

    fn fact<'a>(cmds: &'a [Cmd], key: &str) -> Option<&'a Value> {
        let want = format!("module.{ID}.{key}");
        cmds.iter().rev().find_map(|c| match c {
            Cmd::Publish { key: k, value } if *k == want => Some(value),
            _ => None,
        })
    }

    fn count(cmds: &[Cmd], f: impl Fn(&Cmd) -> bool) -> usize {
        cmds.iter().filter(|c| f(c)).count()
    }

    /// Stopped, Withdrawn, one notification per version, and nothing starts it.
    #[test]
    fn a_withdrawn_running_module_stops_and_stays_stopped() {
        let mut p = Policy::new();
        let t = Instant::now();
        running(&mut p, t, Some("0.4.2"));
        let cmds = p.handle(revoked(Some("0.4.2")), t);
        assert_eq!(count(&cmds, |c| matches!(c, Cmd::Kill { .. })), 1);
        assert_eq!(p.state(ID), Some(State::Revoked));
        assert_eq!(fact(&cmds, "state"), Some(&json!("revoked")));
        assert_eq!(fact(&cmds, "reason"), Some(&json!(words::WITHDRAWN_LINE)));
        assert_eq!(fact(&cmds, "previous"), Some(&json!("0.4.2")));
        let notes: Vec<&Cmd> = cmds
            .iter()
            .filter(|c| matches!(c, Cmd::Notify { .. }))
            .collect();
        assert_eq!(notes.len(), 1);
        if let Cmd::Notify { title, body, .. } = notes[0] {
            assert_eq!(title, "Art 0.5.0 was withdrawn");
            assert_eq!(body, "Its publisher withdrew it, so Hideout stopped it.");
        }
        assert!(cmds.contains(&Cmd::RememberNotified {
            id: ID.into(),
            version: "0.5.0".into()
        }));
        // Its exit afterwards is no crash; nothing restarts it.
        let cmds = p.handle(
            Ev::Exited {
                id: ID.into(),
                instance: 1,
                code: 1,
            },
            t,
        );
        assert!(cmds.is_empty(), "{cmds:?}");
        assert_eq!(p.crashes(ID, t), 0);
        // Start again, an activation, Enable and Start normally start nothing.
        for ev in [
            Ev::StartAgain { id: ID.into() },
            Ev::Activate { id: ID.into() },
            Ev::Enable { id: ID.into() },
            Ev::StartNormally,
        ] {
            let cmds = p.handle(ev.clone(), t);
            assert_eq!(
                count(&cmds, |c| matches!(c, Cmd::Launch { .. })),
                0,
                "{ev:?} started it"
            );
            assert_eq!(p.state(ID), Some(State::Revoked), "{ev:?}");
        }
        // Seen again: no second notification.
        let cmds = p.handle(revoked(Some("0.4.2")), t);
        assert_eq!(count(&cmds, |c| matches!(c, Cmd::Notify { .. })), 0);
    }

    /// A later sign-in: the card again (state revoked), no notification.
    #[test]
    fn a_withdrawal_notified_before_is_not_notified_again() {
        let mut p = Policy::new();
        p.already_notified(ID, "0.5.0");
        let t = Instant::now();
        p.handle(
            Ev::Known {
                id: ID.into(),
                name: "Art".into(),
                enabled: true,
                at_enable: false,
                previous: None,
            },
            t,
        );
        let cmds = p.handle(revoked(None), t);
        assert_eq!(p.state(ID), Some(State::Revoked));
        assert_eq!(count(&cmds, |c| matches!(c, Cmd::Notify { .. })), 0);
        assert_eq!(count(&cmds, |c| matches!(c, Cmd::Kill { .. })), 0);
        // Another withdrawn version is a new event: notified once.
        let cmds = p.handle(
            Ev::Revoked {
                id: ID.into(),
                version: "0.5.1".into(),
                go_back: None,
            },
            t,
        );
        assert_eq!(count(&cmds, |c| matches!(c, Cmd::Notify { .. })), 1);
    }

    /// Turn off: Off (the dot goes), still withdrawn; a Turn on that arrives anyway doesn't
    /// start it.
    #[test]
    fn turned_off_it_stays_off_and_withdrawn() {
        let mut p = Policy::new();
        let t = Instant::now();
        running(&mut p, t, None);
        p.handle(revoked(None), t);
        let cmds = p.handle(Ev::Disable { id: ID.into() }, t);
        assert_eq!(fact(&cmds, "state"), Some(&json!("off")));
        assert_eq!(p.withdrawn(ID), Some("0.5.0"));
        let cmds = p.handle(Ev::Enable { id: ID.into() }, t);
        assert_eq!(count(&cmds, |c| matches!(c, Cmd::Launch { .. })), 0);
        assert_eq!(p.state(ID), Some(State::Revoked));
        // Withdrawn while off: nothing to stop, no notification.
        let mut p = Policy::new();
        p.handle(
            Ev::Known {
                id: ID.into(),
                name: "Art".into(),
                enabled: false,
                at_enable: true,
                previous: None,
            },
            t,
        );
        let cmds = p.handle(revoked(None), t);
        assert_eq!(p.state(ID), Some(State::Off));
        assert!(cmds.iter().all(|c| !matches!(c, Cmd::Notify { .. })));
    }

    /// Check for updates: checking, then nothing newer or couldn't check; a fixed version
    /// clears it and the module starts.
    #[test]
    fn check_for_updates_then_a_fixed_version_starts() {
        let mut p = Policy::new();
        let t = Instant::now();
        running(&mut p, t, None);
        p.handle(revoked(None), t);
        let cmds = p.handle(Ev::Check { id: ID.into() }, t);
        assert_eq!(fact(&cmds, "check"), Some(&json!("checking")));
        assert!(cmds.contains(&Cmd::CheckUpdates { id: ID.into() }));
        let cmds = p.handle(
            Ev::Checked {
                id: ID.into(),
                reached: true,
            },
            t,
        );
        assert_eq!(fact(&cmds, "check"), Some(&json!("none")));
        let cmds = p.handle(
            Ev::Checked {
                id: ID.into(),
                reached: false,
            },
            t,
        );
        assert_eq!(fact(&cmds, "check"), Some(&json!("failed")));
        let cmds = p.handle(Ev::Cleared { id: ID.into() }, t);
        assert_eq!(fact(&cmds, "check"), Some(&Value::Null));
        assert_eq!(count(&cmds, |c| matches!(c, Cmd::Launch { .. })), 1);
        assert_eq!(p.state(ID), Some(State::Starting));
        // Not withdrawn: a check does nothing.
        assert!(p.handle(Ev::Check { id: ID.into() }, t).is_empty());
    }

    /// Go back: only with a kept version that isn't withdrawn; a rollback, then a start.
    #[test]
    fn go_back_rolls_back_and_starts() {
        let mut p = Policy::new();
        let t = Instant::now();
        running(&mut p, t, Some("0.4.2"));
        p.handle(revoked(None), t);
        let cmds = p.handle(Ev::GoBack { id: ID.into() }, t);
        assert!(cmds.is_empty(), "no Go back without a version: {cmds:?}");
        p.handle(revoked(Some("0.4.2")), t);
        let cmds = p.handle(Ev::GoBack { id: ID.into() }, t);
        let order: Vec<&Cmd> = cmds
            .iter()
            .filter(|c| matches!(c, Cmd::RollBack { .. } | Cmd::Launch { .. }))
            .collect();
        assert_eq!(
            order,
            [
                &Cmd::RollBack { id: ID.into() },
                &Cmd::Launch { id: ID.into() }
            ]
        );
        assert_eq!(p.withdrawn(ID), None);
        assert!(p
            .history(ID)
            .iter()
            .any(|(_, l)| l == "stopped: its publisher withdrew 0.5.0 (catalog revocation)"));
    }

    /// A restart that was waiting is called off, and the crash bar's fact goes.
    #[test]
    fn a_waiting_restart_is_called_off() {
        let mut p = Policy::new();
        let t = Instant::now();
        running(&mut p, t, None);
        p.handle(
            Ev::Exited {
                id: ID.into(),
                instance: 1,
                code: 1,
            },
            t,
        );
        assert_eq!(p.state(ID), Some(State::Restarting));
        let cmds = p.handle(revoked(None), t);
        assert_eq!(fact(&cmds, "restart"), Some(&Value::Null));
        assert_eq!(p.next_deadline(), None);
        assert!(p.tick(t + Duration::from_secs(120)).is_empty());
        assert_eq!(p.state(ID), Some(State::Revoked));
    }

    /// A module removed while its restart waited (#1054): forgotten, nothing starts later, and
    /// it isn't among the startable modules or its history.
    #[test]
    fn a_removed_module_is_forgotten() {
        let mut p = Policy::new();
        let t = Instant::now();
        running(&mut p, t, None);
        p.handle(
            Ev::Exited {
                id: ID.into(),
                instance: 1,
                code: 1,
            },
            t,
        );
        assert_eq!(p.state(ID), Some(State::Restarting));
        assert!(p.forget(ID));
        assert_eq!(p.state(ID), None);
        assert!(p.known().is_empty() && p.history(ID).is_empty());
        assert_eq!(p.next_deadline(), None);
        assert!(p.tick(t + Duration::from_secs(120)).is_empty());
        assert!(!p.forget(ID), "forgotten twice");
    }
}
