// SPDX-License-Identifier: MIT
//! Drives the [`Policy`] with real processes (Windows): launches in Job Objects (§7.5, §7.7),
//! the launch check the router runs at `initialize`, adoption after a hub restart (§7.9: pid,
//! start time and exe hash from `run\instances.json`, then a fresh Job Object; #986),
//! `run\instances.json`, stderr to each module's log, and one timer thread that sleeps until
//! the policy's next deadline. Notices ends by events only: a thread per instance blocks on its
//! process handle; the router reports the pipe closing (no heartbeats, #281).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use hideout_registry::Registry;
use hideout_router::server::{user, Hub};
use hideout_router::{ModuleEntry, Out};
use serde_json::Value;

use crate::instances::{Instances, Record};
use crate::policy::{Cmd, Ev, Policy};
use crate::sha256_file;
use crate::win::{self, Caps, Job, Process};
use crate::words;

/// A notification for the shell (§7.7): title, body, buttons, the module it is about.
pub type Notifier = Box<dyn FnMut(&str, &str, &str, &[String]) + Send>;

/// Fetches the catalog of a module's source now and accepts it into the hub's trust store
/// (§12.4): one fetch, no retry loop. `Err` says why the source couldn't be reached. The
/// catalog fetch registers one; without one, Check for updates reads "Couldn't check" (#1026).
pub type Refresh = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// What Copy diagnostics says about a withdrawal (#1026).
#[derive(Clone, Debug, Default)]
struct Withdrawal {
    version: String,
    reason: String,
    source: String,
    previous: Option<String>,
    go_back: Option<String>,
    at: Option<SystemTime>,
    leases_ended: usize,
    /// The last Check for updates: when, and what came of it.
    last_check: Option<(SystemTime, String)>,
}

/// What the router and the policy learn of an installed module, at the start and when it
/// joins later (#1076): its router entry with the manifest's name and `call` activation, and
/// its `Known` (`None` when the manifest has no `run`: nothing to start).
fn known_event(
    reg: &Registry,
    id: &str,
    manifest: Option<&hideout_contract::module::Module>,
) -> (Option<ModuleEntry>, Option<Ev>) {
    let activation = manifest
        .map(|m| m.activation_on.clone())
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| vec!["enable".into()]);
    let on_call = activation.iter().any(|a| a == "call");
    let name = manifest.map_or_else(|| id.to_string(), |m| m.name.clone());
    let entry = ModuleEntry::from_registry(reg, id).map(|mut e| {
        e.activate_on_call = on_call;
        e.name = name.clone();
        e
    });
    let runs = manifest.is_some_and(|m| m.run.is_some());
    let ev = runs.then(|| Ev::Known {
        id: id.into(),
        name,
        enabled: reg.hub.enabled(id),
        at_enable: activation.iter().any(|a| a == "enable"),
        previous: reg
            .inventory
            .modules
            .get(id)
            .and_then(|m| m.previous.clone()),
    });
    (entry, ev)
}

/// An installed manifest as the supervisor reads it: the module (`None`: unreadable), or one
/// whose `hub` this hub doesn't serve (§7.8 needs-hub), with the `Known` it is learnt by.
// Short-lived, once per module read: the size difference doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Read {
    Module(Option<hideout_contract::module::Module>),
    NeedsHub(Vec<Ev>),
}

fn read_manifest(reg: &Registry, id: &str) -> Read {
    match reg.manifest_checked(id) {
        Err(e) if e.id() == Some(NEEDS_HUB) => {
            // The hub went back a version, or the module joined needing a newer one: known, so
            // its row and card read Needs a newer Hideout (§7.8), never started.
            log::warn!("supervisor: {id}: {e}; needs a newer hub");
            Read::NeedsHub(vec![Ev::Known {
                id: id.into(),
                name: id.into(),
                enabled: reg.hub.enabled(id),
                at_enable: false,
                previous: reg
                    .inventory
                    .modules
                    .get(id)
                    .and_then(|m| m.previous.clone()),
            }])
        }
        m => Read::Module(m.ok()),
    }
}

/// The root-cause graph's edges from `id` (§4.4), when it has any.
fn depends_event(id: &str, m: Option<&hideout_contract::module::Module>) -> Option<Ev> {
    let on = depends_on(m?);
    (!on.is_empty()).then(|| Ev::Depends { id: id.into(), on })
}

/// `run\withdrawn-notified.json`: the withdrawn versions already notified (`id@version`), so a
/// later sign-in shows the card and the dot but no second notification (decision 3, 11/10).
fn notified_path(home: &Path) -> PathBuf {
    home.join("run").join("withdrawn-notified.json")
}

fn load_notified(home: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(notified_path(home))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v["notified"].as_array().map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn save_notified(home: &Path, set: &BTreeSet<String>) -> std::io::Result<()> {
    let path = notified_path(home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&serde_json::json!({ "notified": set }))?,
    )?;
    std::fs::rename(tmp, path)
}

/// What the launch check compares (§7.5): the pid the hub started or took back, and its exe.
#[derive(Clone, Debug)]
struct Expected {
    pid: u32,
    exe: PathBuf,
    sha256: String,
}

/// One running instance.
struct Live {
    instance: u64,
    pid: u32,
    /// Its Job Object; `None` for one taken back that couldn't be put in a fresh job.
    job: Option<Arc<Job>>,
    /// Its process, for one taken back (a started one is ended through its job).
    process: Option<Arc<Process>>,
}

impl Live {
    fn kill(&self, code: u32) {
        if let Some(j) = &self.job {
            j.kill(code);
        }
        if let Some(p) = &self.process {
            p.kill(code);
        }
    }
}

/// What a record from the last hub turned out to be (§7.9, #986).
enum Found {
    /// The process the last hub started (pid and start time), still running the recorded exe.
    Survivor(Process),
    /// The process the last hub started, but its exe no longer matches the record: an orphan
    /// to end (this pid only).
    Orphan(Process, String),
    /// Not running, or its pid now names another process (left alone): nothing to take back.
    Gone(String),
}

/// Checks a record against the process now at its pid.
fn examine(rec: &Record) -> Found {
    let p = match Process::open(rec.pid) {
        Ok(p) => p,
        Err(e) => return Found::Gone(format!("pid {} not running ({e})", rec.pid)),
    };
    if !p.running() {
        return Found::Gone(format!("pid {} has ended", rec.pid));
    }
    let started = match p.started() {
        Ok(t) => t,
        Err(e) => return Found::Gone(format!("pid {}: start time unknown ({e})", rec.pid)),
    };
    // A record from before start times (#982) can't prove the process is ours: a mismatch below
    // then means another process may have the pid, and it is left alone.
    let ours = match rec.started {
        Some(t) if t != started => {
            return Found::Gone(format!(
                "pid {} now belongs to another process (started at another time)",
                rec.pid
            ))
        }
        Some(_) => true,
        None => false,
    };
    let image = match p.image() {
        Ok(i) => i,
        Err(e) => return verdict(ours, p, format!("pid {}: image unknown ({e})", rec.pid)),
    };
    if !same_path(&image, &rec.exe) {
        return verdict(ours, p, format!("pid {} runs {}", rec.pid, image.display()));
    }
    match sha256_file(&image) {
        Ok(h) if h == rec.sha256 => Found::Survivor(p),
        Ok(_) => verdict(ours, p, format!("exe hash of {} changed", image.display())),
        Err(e) => verdict(ours, p, format!("hash of {}: {e}", image.display())),
    }
}

/// An exe check failed: an orphan to end when pid and start time proved it ours.
fn verdict(ours: bool, p: Process, why: String) -> Found {
    if ours {
        Found::Orphan(p, why)
    } else {
        Found::Gone(why)
    }
}

struct Inner {
    home: PathBuf,
    hub: Hub,
    policy: Mutex<Policy>,
    live: Mutex<BTreeMap<String, Live>>,
    expected: Arc<Mutex<BTreeMap<String, Expected>>>,
    records: Mutex<Instances>,
    next_instance: AtomicU64,
    nudge: Mutex<Sender<()>>,
    notifier: Mutex<Option<Notifier>>,
    caps: Caps,
    refresh: Mutex<Option<Refresh>>,
    withdrawals: Mutex<BTreeMap<String, Withdrawal>>,
    /// Notifications made before the shell's notifier was set (a withdrawal found at hub start):
    /// delivered by [`Supervisor::set_notifier`]. At most [`EARLY_NOTES`].
    early: Mutex<Vec<EarlyNote>>,
    /// Hub calls the supervisor made (`module.drain`, `module.exit`), by tag: the module and
    /// whether its answer is a drain's. Tags stay below the panel feed's (`1 << 40`).
    calls: Mutex<BTreeMap<u64, (String, bool)>>,
    next_tag: AtomicU64,
}

/// What [`Supervisor::launch`] answers when the manifest's `hub` isn't served (§7.8 needs-hub).
const NEEDS_HUB: &str = "manifest.hub";
/// How [`Supervisor::launch`]'s answer starts when containment is on and the module may not
/// run without one (§7.7).
const NO_SANDBOX: &str = "no sandbox";

/// The modules a manifest needs (§4.4): its `requires` ids and its `channel.open:` providers.
fn depends_on(m: &hideout_contract::module::Module) -> BTreeSet<String> {
    use hideout_contract::capability::Capability;
    let required = m
        .requires
        .iter()
        .map(|r| r.split(">=").next().unwrap_or(r).trim().to_string());
    let opened = m.permissions.iter().filter_map(|c| match c {
        Capability::ChannelOpen { module, .. } => Some(module.clone()),
        _ => None,
    });
    required
        .chain(opened)
        .filter(|d| !d.is_empty() && *d != m.id)
        .collect()
}

/// The `update-waiting` reason for the permissions an update adds (§6.4): the first in plain
/// words, when it has them.
fn asks_words(pending: &[String]) -> Option<String> {
    use hideout_contract::capability::Capability;
    let first = pending.first()?;
    let plain = match Capability::parse(first) {
        Ok(Capability::Kept(p)) => Some(p.describe()),
        _ => None,
    };
    Some(words::update_asks(plain.as_deref(), pending.len() > 1))
}

/// A notification held for the notifier: module id, title, body, buttons.
type EarlyNote = (String, String, String, Vec<String>);

/// The most notifications held for a notifier that isn't set yet.
const EARLY_NOTES: usize = 16;

/// The supervisor. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Supervisor(Arc<Inner>);

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `path` relative to `base` when it is inside it (case-insensitive, either slash).
fn under(path: &str, base: &str) -> Option<String> {
    let (p, b) = (path.replace('/', "\\"), base.replace('/', "\\"));
    let b = b.trim_end_matches('\\');
    let head = p.get(..b.len())?;
    let rest = p[b.len()..].strip_prefix('\\')?;
    head.eq_ignore_ascii_case(b).then(|| rest.to_string())
}

/// The launch check the router runs at a module's `initialize` (§7.5, §8.2 v1.3).
fn launch_check(
    expected: &Mutex<BTreeMap<String, Expected>>,
    id: &str,
    pid: Option<u32>,
) -> Result<(), String> {
    let Some(want) = expected.lock().unwrap().get(id).cloned() else {
        return Err("no instance of it was started or taken back".into());
    };
    let Some(pid) = pid else {
        return Err("the connection's process is unknown".into());
    };
    if pid != want.pid {
        return Err(format!("pid {pid}, started {}", want.pid));
    }
    let image = win::image_of_pid(pid).map_err(|e| format!("image of {pid}: {e}"))?;
    if !same_path(&image, &want.exe) {
        return Err(format!("pid {pid} runs {}", image.display()));
    }
    let hash = sha256_file(&image).map_err(|e| format!("hash of {}: {e}", image.display()))?;
    if hash != want.sha256 {
        return Err(format!("exe hash of {} changed", image.display()));
    }
    Ok(())
}

impl Supervisor {
    /// Takes back what still runs from the last hub (§7.9), then starts what is enabled.
    pub fn start(home: &Path, hub: Hub) -> Supervisor {
        let (nudge, wake) = mpsc::channel();
        let expected: Arc<Mutex<BTreeMap<String, Expected>>> = Arc::default();
        let sup = Supervisor(Arc::new(Inner {
            home: home.to_path_buf(),
            hub: hub.clone(),
            policy: Mutex::new(Policy::new()),
            live: Mutex::new(BTreeMap::new()),
            expected: expected.clone(),
            records: Mutex::new(Instances::default()),
            next_instance: AtomicU64::new(unix_ms()),
            nudge: Mutex::new(nudge),
            notifier: Mutex::new(None),
            caps: Caps::default(),
            refresh: Mutex::new(None),
            withdrawals: Mutex::new(BTreeMap::new()),
            early: Mutex::new(Vec::new()),
            calls: Mutex::new(BTreeMap::new()),
            next_tag: AtomicU64::new(1),
        }));
        hub.act(|r| {
            r.set_launch_check(Box::new(move |id, pid| launch_check(&expected, id, pid)));
            Vec::new()
        });
        let safe = hub.read(|r| r.fact("hub.safe_mode").and_then(Value::as_bool)) == Some(true);
        {
            let mut p = sup.0.policy.lock().unwrap();
            p.safe_mode = safe;
            for k in load_notified(home) {
                if let Some((id, v)) = k.split_once('@') {
                    p.already_notified(id, v);
                }
            }
        }
        let weak = Arc::downgrade(&sup.0);
        std::thread::Builder::new()
            .name("hideout-supervisor-timer".into())
            .spawn(move || timer(weak, wake))
            .expect("supervisor timer thread");

        let old = Instances::load(home);
        log::info!(
            "supervisor: starting; {} instance(s) recorded in run\\instances.json{}",
            old.modules.len(),
            if safe { "; safe mode" } else { "" }
        );
        let reg = match Registry::open_from_env(home) {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("supervisor: registry not read ({e:?}); nothing to start");
                None
            }
        };
        let mut known = Vec::new();
        let mut later = Vec::new();
        let mut runnable = BTreeSet::new();
        if let Some(reg) = &reg {
            for id in reg.inventory.modules.keys() {
                let manifest = match read_manifest(reg, id) {
                    Read::NeedsHub(evs) => {
                        known.extend(evs.into_iter().take(1));
                        later.push(Ev::NeedsHub { id: id.clone() });
                        continue;
                    }
                    Read::Module(m) => m,
                };
                later.extend(depends_event(id, manifest.as_ref()));
                let (entry, ev) = known_event(reg, id, manifest.as_ref());
                if let Some(e) = entry.filter(|e| e.activate_on_call || e.name != *id) {
                    hub.act(|r| {
                        r.add_module(e);
                        Vec::new()
                    });
                }
                let Some(ev) = ev else {
                    continue;
                };
                if reg.hub.enabled(id) {
                    runnable.insert(id.clone());
                }
                known.push(ev);
            }
        }
        // Take back first, so `Known` never starts a second instance of a module still running
        // (§7.9). Every record is settled, wanted or not: what the last hub left is taken back
        // or ended, by its own pid only, never by name (#986).
        let mut adopted = Vec::new();
        for (id, rec) in old.modules {
            match examine(&rec) {
                Found::Survivor(p) => {
                    let why_not = if !runnable.contains(&id) {
                        Some("no longer enabled or installed")
                    } else if rec.token.is_empty() {
                        // A stdio module's pipes went with the last hub; it can't reconnect.
                        Some("its stdio connection went with the last hub")
                    } else {
                        None
                    };
                    if let Some(why) = why_not {
                        log::info!("supervisor: {id} (pid {}) ended: {why}", rec.pid);
                        p.kill(1);
                        continue;
                    }
                    match sup.adopt(&id, &rec, p) {
                        Ok(()) => adopted.push(Ev::Adopted {
                            id,
                            instance: rec.instance,
                        }),
                        Err(e) => log::warn!("supervisor: {id} not taken back: {e}"),
                    }
                }
                Found::Orphan(p, why) => {
                    log::warn!(
                        "supervisor: {id} (pid {}) left by the last hub doesn't match its record ({why}); ended",
                        rec.pid
                    );
                    p.kill(1);
                }
                Found::Gone(why) => log::info!("supervisor: {id} not taken back: {why}"),
            }
        }
        sup.save_records();
        for ev in known {
            let id = match &ev {
                Ev::Known { id, .. } => id.clone(),
                _ => unreachable!(),
            };
            let taken = adopted
                .iter()
                .any(|a| matches!(a, Ev::Adopted { id: i, .. } if *i == id));
            if taken {
                // Known without a start: the adopted instance is it.
                if let Ev::Known {
                    id,
                    name,
                    enabled,
                    previous,
                    ..
                } = ev
                {
                    sup.feed(Ev::Known {
                        id,
                        name,
                        enabled,
                        at_enable: false,
                        previous,
                    });
                }
            } else {
                sup.feed(ev);
            }
        }
        for ev in later {
            sup.feed(ev);
        }
        for ev in adopted {
            sup.feed(ev);
        }
        // A version withdrawn while the hub was away (or turned off, or waiting for a call):
        // the card, the dot and the Off row say so from the start (§12.5, #1026).
        sup.catalog_accepted();
        sup
    }

    /// Where Check for updates fetches a source's catalog (#1026). The catalog fetch sets it;
    /// a lab hub sets one that re-reads `lab-withdrawn.json`.
    pub fn set_refresh(&self, r: Refresh) {
        *self.0.refresh.lock().unwrap() = Some(r);
    }

    /// An accepted catalog may have revoked or restored installed versions (§12.4): call after
    /// every catalog the hub accepts, and once at start. A newly withdrawn version is stopped
    /// (its leases end with `"revoked"` first) and reads Withdrawn; a version no longer
    /// withdrawn (an update installed, the revocation gone) starts as usual.
    pub fn catalog_accepted(&self) {
        let reg = match Registry::open_from_env(&self.0.home) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("supervisor: registry not read for withdrawals ({e:?})");
                return;
            }
        };
        let known = self.0.policy.lock().unwrap().known();
        for id in known {
            let now = self
                .0
                .policy
                .lock()
                .unwrap()
                .withdrawn(&id)
                .map(String::from);
            match reg.revocation_of(&id) {
                Some(r) if now.as_deref() != Some(r.version.as_str()) => {
                    self.withdraw(&reg, &id, r)
                }
                Some(_) => {}
                None if now.is_some() && !reg.withdrawals_known(&id) => {
                    log::warn!(
                        "supervisor: {id}: no catalog of its source could be read; it stays withdrawn"
                    );
                }
                None if now.is_some() => {
                    log::info!("supervisor: {id}: no longer withdrawn");
                    self.0.withdrawals.lock().unwrap().remove(&id);
                    self.feed(Ev::Cleared { id });
                }
                None => {}
            }
        }
    }

    /// Ends `id`'s leases with `"revoked"` and publishes `module.<id>.revoked` (the router),
    /// then stops it (the policy, #1026).
    fn withdraw(&self, reg: &Registry, id: &str, r: hideout_registry::Revoked) {
        let go_back = reg.go_back_version(id);
        let mut ended = 0;
        self.0.hub.act(|rt| {
            let out = rt.revoke(id, &r.version, &r.reason);
            ended = out
                .iter()
                .filter(|o| matches!(o, Out::Send(_, m) if m["method"] == "lease.closed"))
                .count();
            out
        });
        let m = reg.inventory.modules.get(id);
        log::warn!(
            "supervisor: {id} {} was withdrawn by its publisher (source {}); stopping it, {ended} lease(s) ended",
            r.version,
            m.map_or("unknown", |m| m.source.as_str())
        );
        self.0.withdrawals.lock().unwrap().insert(
            id.into(),
            Withdrawal {
                version: r.version.clone(),
                reason: r.reason.clone(),
                source: m.map(|m| m.source.clone()).unwrap_or_default(),
                previous: m.and_then(|m| m.previous.clone()),
                go_back: go_back.clone(),
                at: Some(SystemTime::now()),
                leases_ended: ended,
                last_check: None,
            },
        );
        self.feed(Ev::Revoked {
            id: id.into(),
            version: r.version,
            go_back,
        });
    }

    /// **Check for updates** on the withdrawn card, its notification and its Modules row:
    /// fetches the source's catalog once, off the caller's thread, then answers on the card
    /// (`module.<id>.check`, #1026).
    pub fn check_for_updates(&self, id: &str) {
        self.feed(Ev::Check { id: id.into() });
    }

    /// Runs one check (after [`Cmd::CheckUpdates`]): the refresh, then what it changed.
    fn run_check(&self, id: &str) {
        let refresh = self.0.refresh.lock().unwrap().clone();
        let sup = self.clone();
        let id = id.to_string();
        let spawned = std::thread::Builder::new()
            .name(format!("hideout-check-{id}"))
            .spawn(move || {
                let got = match refresh {
                    Some(f) => f(&id),
                    None => Err("no catalog fetch in this hub".into()),
                };
                let outcome = match &got {
                    Ok(()) => {
                        sup.catalog_accepted();
                        let still = sup.0.policy.lock().unwrap().withdrawn(&id).is_some();
                        if still {
                            sup.feed(Ev::Checked {
                                id: id.clone(),
                                reached: true,
                            });
                            "nothing newer".to_string()
                        } else {
                            "found a version that isn't withdrawn".to_string()
                        }
                    }
                    Err(e) => {
                        log::warn!("supervisor: {id}: check for updates: {e}");
                        sup.feed(Ev::Checked {
                            id: id.clone(),
                            reached: false,
                        });
                        format!("couldn't reach the source ({e})")
                    }
                };
                if let Some(w) = sup.0.withdrawals.lock().unwrap().get_mut(&id) {
                    w.last_check = Some((SystemTime::now(), outcome));
                }
            });
        if let Err(e) = spawned {
            log::warn!("supervisor: check for updates not started: {e}");
        }
    }

    /// `id` left the inventory (#1054): the supervisor drops its records (policy slot,
    /// expected launch, `run\instances.json` entry, withdrawal note). The registry refuses to
    /// remove a running module; one still live anyway is kept (logged) until it ends, and the
    /// next start settles it. Answers whether anything was dropped.
    pub fn forget(&self, id: &str) -> bool {
        if self.0.live.lock().unwrap().contains_key(id) {
            log::warn!("supervisor: {id} left the inventory but still runs; kept");
            return false;
        }
        let known = self.0.policy.lock().unwrap().forget(id);
        // Its AppContainer profile, if it ever ran in one (§7.7); nothing when it didn't.
        if let Some(sid) = crate::sandbox::profile_sid(id) {
            if let Err(e) = self.0.hub.disallow_pipe_client(&sid) {
                log::warn!("supervisor: {id}: AppContainer not taken off the router pipe: {e}");
            }
        }
        crate::sandbox::delete_profile(id);
        self.0.expected.lock().unwrap().remove(id);
        self.0.withdrawals.lock().unwrap().remove(id);
        let recorded = self.0.records.lock().unwrap().modules.remove(id).is_some();
        if recorded {
            self.save_records();
        }
        if known || recorded {
            log::info!("supervisor: {id} left the inventory; forgotten");
        }
        // The timer's next deadline may have been this module's restart.
        let _ = self.0.nudge.lock().unwrap().send(());
        known || recorded
    }

    /// `id` joined the inventory while the hub runs (#1076; the inventory watch, #1058): the
    /// router learns it (its entry and manifest) and the policy gets its `Known`, so an enabled
    /// module with `[activation] on = ["enable"]` starts now, as it would at a hub start, and
    /// an activation fact already set starts it too (§4.6). `None` when the registry doesn't
    /// hold it yet (a lab folder whose manifest isn't written yet): the caller asks again at
    /// the next change. `Some(manifest)` once learned. A module the router already knows
    /// answers `Some(None)`; if `hideout.toml` turned it on or off since the policy learnt it
    /// (an install that enables in a second write, after the watch read the first), the
    /// policy gets its `Known` again: turned on, it starts; turned off, an idle one reads Off
    /// (a running one keeps running until Turn off, as at a start).
    pub fn learn(&self, id: &str) -> Option<Option<hideout_contract::module::Module>> {
        match Registry::open_from_env(&self.0.home) {
            Ok(reg) => self.learn_from(&reg, id),
            Err(e) => {
                log::warn!(
                    "supervisor: {id}: registry not read ({e:?}); asked again at the next change"
                );
                None
            }
        }
    }

    /// [`Supervisor::learn`] from a registry read just now (the inventory watch reads one per
    /// change and lists the Modules page from the same read, so the page never shows what the
    /// supervisor hasn't seen).
    pub fn learn_from(
        &self,
        reg: &Registry,
        id: &str,
    ) -> Option<Option<hideout_contract::module::Module>> {
        if !reg.inventory.modules.contains_key(id) {
            return None;
        }
        if self.0.hub.read(|r| r.knows_module(id)) {
            let on = reg.hub.enabled(id);
            let held = self.0.policy.lock().unwrap().enabled(id);
            if held.is_some_and(|h| h != on) {
                if let (_, Some(ev)) = known_event(reg, id, reg.manifest(id).as_ref()) {
                    log::info!(
                        "supervisor: {id} turned {} in hideout.toml",
                        if on { "on" } else { "off" }
                    );
                    self.feed(ev);
                    self.0.hub.act(|r| r.activation_due(Some(id)));
                }
            }
            return Some(None);
        }
        let manifest = match read_manifest(reg, id) {
            Read::NeedsHub(evs) => {
                // A late module that needs a newer hub reads so, as at a start (§7.8).
                for ev in evs {
                    self.feed(ev);
                }
                self.feed(Ev::NeedsHub { id: id.into() });
                return Some(None);
            }
            Read::Module(m) => m,
        };
        let (entry, ev) = known_event(reg, id, manifest.as_ref());
        let depends = depends_event(id, manifest.as_ref());
        let m = manifest.clone();
        self.0.hub.act(|r| {
            if let Some(e) = entry {
                r.add_module(e);
            }
            if let Some(m) = &m {
                r.apply_manifest(m);
            }
            Vec::new()
        });
        match ev {
            Some(ev) => {
                let on = reg.hub.enabled(id);
                log::info!(
                    "supervisor: {id} joined the inventory ({})",
                    if on { "on" } else { "off" }
                );
                self.feed(ev);
                // Its place in the Waiting graph (§4.4): it may need, or be needed by, a
                // module already running.
                if let Some(d) = depends {
                    self.feed(d);
                }
                self.0.hub.act(|r| r.activation_due(Some(id)));
            }
            None => log::info!("supervisor: {id} joined the inventory (nothing to run)"),
        }
        Some(manifest)
    }

    /// Whether the policy knows `id` (an installed module it could start).
    pub fn knows(&self, id: &str) -> bool {
        self.0.policy.lock().unwrap().state(id).is_some()
    }

    /// The pid of the module's live instance (started or taken back), if any.
    pub fn pid(&self, id: &str) -> Option<u32> {
        self.0.live.lock().unwrap().get(id).map(|l| l.pid)
    }

    /// Where the shell's notifications go (§7.7). Those made before it was set (at hub start)
    /// go to it now; without one they are logged.
    pub fn set_notifier(&self, n: Notifier) {
        let mut slot = self.0.notifier.lock().unwrap();
        let n = slot.insert(n);
        for (id, title, body, actions) in std::mem::take(&mut *self.0.early.lock().unwrap()) {
            n(&id, &title, &body, &actions);
        }
    }

    /// What the router reported (the hub's events thread forwards every [`Out`] here).
    pub fn on_router(&self, out: &Out) {
        let ev = match out {
            Out::ModuleGone(id) => Ev::PipeGone { id: id.clone() },
            Out::ModuleFault(id) => Ev::Fault { id: id.clone() },
            Out::Ready(id) => Ev::Ready { id: id.clone() },
            Out::Refused(id) => Ev::Refused { id: id.clone() },
            Out::Activate(id) => Ev::Activate { id: id.clone() },
            Out::Idle(id) => Ev::Idle { id: id.clone() },
            Out::HubResult { tag, result } => {
                let Some((id, drain)) = self.0.calls.lock().unwrap().remove(tag) else {
                    return; // the panel feed's
                };
                if !drain {
                    return; // module.exit: its exit is what counts
                }
                let busy = match result {
                    Ok(v) if v["state"] == "busy" => Some(
                        v["reason"]
                            .as_str()
                            .filter(|r| !r.trim().is_empty())
                            .unwrap_or("what it is doing")
                            .to_string(),
                    ),
                    Ok(_) => None,
                    Err(e) => {
                        // No drain handler, gone or timed out: nothing to wait for (§7.6).
                        log::info!("supervisor: {id}: module.drain answered {e}; swapping");
                        None
                    }
                };
                Ev::Drained { id, busy }
            }
            _ => return,
        };
        self.feed(ev);
    }

    /// One hub call to `id` (`module.drain`, `module.exit`); the answer comes back through
    /// [`Supervisor::on_router`].
    fn call(&self, id: &str, method: &str, params: Value, drain: bool) {
        let tag = self.0.next_tag.fetch_add(1, Ordering::Relaxed);
        self.0
            .calls
            .lock()
            .unwrap()
            .insert(tag, (id.to_string(), drain));
        let id = id.to_string();
        let method = method.to_string();
        self.0
            .hub
            .act(|r| r.call_module(&id, &method, params, tag, Instant::now()));
    }

    /// The update step's last half (§7.4): call after a new version of `id` was installed
    /// beside the running one. With permissions to approve it shows Update waiting for you
    /// (never a modal); else it drains and swaps when the module says done.
    pub fn update_staged(&self, id: &str) {
        let Ok(reg) = Registry::open_from_env(&self.0.home) else {
            return;
        };
        let Some(m) = reg.inventory.modules.get(id) else {
            return;
        };
        let Some(previous) = m.previous.clone() else {
            log::warn!("supervisor: {id}: update staged with no previous version; nothing to swap");
            return;
        };
        self.feed(Ev::UpdateStaged {
            id: id.into(),
            version: m.version.clone(),
            previous,
            asks: asks_words(&m.pending_permissions),
        });
    }

    /// **Review** on Update waiting for you, after the user approved the permission dialog:
    /// the grants are recorded (`inventory.json`), then the swap runs.
    pub fn review_approved(&self, id: &str) -> Result<(), String> {
        let never = Registry::open(&self.0.home, hideout_registry::dev_mode_from_env())
            .map_err(|e| format!("{e:?}"))
            .and_then(|mut r| r.approve(id).map_err(|e| format!("{e:?}")))?;
        for e in never {
            log::warn!("supervisor: {id}: not granted: {e}");
        }
        self.feed(Ev::Review { id: id.into() });
        Ok(())
    }

    /// **Not now** on Update waiting for you or Update ready.
    pub fn not_now(&self, id: &str) {
        self.feed(Ev::NotNow { id: id.into() });
    }

    /// **Force** on Update ready.
    pub fn force_update(&self, id: &str) {
        self.feed(Ev::Force { id: id.into() });
    }

    /// The panel host started `id`'s panels quarantined (§5.5): it stops and reads
    /// quarantined until Start again or Go back.
    pub fn quarantine(&self, id: &str) {
        self.feed(Ev::Quarantine { id: id.into() });
    }

    /// `<user>` plus the hub's folder suffix for Job Object names (#1010), so a lab hub never
    /// opens the user hub's jobs. The default folder keeps `<user>`.
    fn job_user(&self) -> String {
        format!("{}{}", user(), self.0.hub.folder_suffix())
    }

    /// "Start again" (§7.8).
    pub fn start_again(&self, id: &str) {
        self.feed(Ev::StartAgain { id: id.into() });
    }

    /// "Start with default settings" (§7.8).
    pub fn start_with_defaults(&self, id: &str) {
        self.feed(Ev::StartDefaults { id: id.into() });
    }

    /// "Go back to <previous>" (the ladder's third step).
    pub fn go_back(&self, id: &str) {
        self.feed(Ev::GoBack { id: id.into() });
    }

    /// "Turn off <Module>" (the ladder's last step): saved as off, like the Modules page's
    /// switch, then stopped.
    pub fn turn_off(&self, id: &str) {
        // From the environment, so a lab hub's lab modules are known and saved too (#1076).
        let saved = Registry::open_from_env(&self.0.home)
            .map_err(|e| format!("{e:?}"))
            .and_then(|mut r| r.set_enabled(id, false).map_err(|e| format!("{e:?}")));
        if let Err(e) = saved {
            log::warn!("supervisor: {id}: off not saved: {e}");
        }
        self.feed(Ev::Disable { id: id.into() });
    }

    /// **Turn on** from the Modules page: saved as on, then started as `enable` does (§7.4).
    /// A module with permissions still waiting for the permission dialog (§6.4) isn't turned
    /// on here: that dialog isn't built, so it answers why and the caller opens the module. In
    /// safe mode it is saved as on and not started ("On · not started in safe mode").
    pub fn turn_on(&self, id: &str) -> Result<(), String> {
        // A withdrawn version is never turned on, however the ask came (#1026).
        let withdrawn = Registry::open_from_env(&self.0.home)
            .ok()
            .and_then(|r| r.revocation_of(id));
        if let Some(w) = withdrawn {
            let e = format!("trust.revoked: {id}@{} was withdrawn", w.version);
            log::warn!("supervisor: {id}: not turned on: {e}");
            return Err(e);
        }
        let saved = Registry::open_from_env(&self.0.home)
            .map_err(|e| format!("{e:?}"))
            .and_then(|mut r| {
                let pending = r
                    .inventory
                    .modules
                    .get(id)
                    .is_some_and(|m| !m.pending_permissions.is_empty());
                if pending {
                    return Err("permissions wait for the permission dialog".into());
                }
                r.set_enabled(id, true).map_err(|e| format!("{e:?}"))
            });
        if let Err(e) = saved {
            log::warn!("supervisor: {id}: not turned on: {e}");
            return Err(e);
        }
        if self.safe_mode() {
            log::info!("supervisor: {id}: turned on; not started in safe mode");
        }
        self.feed(Ev::Enable { id: id.into() });
        // A fact it waits for that was set while it was off starts it now (§4.6, v1.10).
        self.0.hub.act(|r| r.activation_due(Some(id)));
        Ok(())
    }

    /// **Start normally** (§7.10, the safe-mode banner and the tray menu): clears `hideout.toml`'s
    /// `safe_mode` flag, turns `hub.safe_mode` off and starts the enabled modules, all in this
    /// hub: no restart. Safe to press twice. The flag not being saved is logged; the modules
    /// start anyway, and the next hub start would be in safe mode again.
    pub fn start_normally(&self) {
        let saved = Registry::open_from_env(&self.0.home)
            .map_err(|e| format!("{e:?}"))
            .and_then(|mut r| r.set_safe_mode(false).map_err(|e| format!("{e:?}")));
        if let Err(e) = saved {
            log::warn!("supervisor: safe mode off not saved: {e}");
        }
        if let Err(e) = crate::closes::clear_safe_mode_cause(&self.0.home) {
            log::warn!("supervisor: run\\hub-run.json not saved: {e}");
        }
        self.0.hub.act(|r| r.set_safe_mode(false));
        let cmds = self
            .0
            .policy
            .lock()
            .unwrap()
            .handle(Ev::StartNormally, Instant::now());
        let starting = cmds
            .iter()
            .filter(|c| matches!(c, Cmd::Launch { .. }))
            .count();
        log::info!(
            "supervisor: Start normally: safe mode off, {starting} module(s) starting (no hub restart)"
        );
        self.run(cmds);
        // Activation facts set while safe mode held the modules (§4.6, v1.10).
        self.0.hub.act(|r| r.activation_due(None));
    }

    /// Whether the supervisor is in safe mode (nothing starts by itself).
    pub fn safe_mode(&self) -> bool {
        self.0.policy.lock().unwrap().safe_mode
    }

    /// A round of finding the module that breaks Hideout (§7.10, #1033): in safe mode only
    /// `on` starts; the modules out of it stop for the session (nothing saved changes). `None`
    /// ends the rounds: nothing runs until Start normally.
    pub fn set_round(&self, on: Option<BTreeSet<String>>) {
        let n = on.as_ref().map_or(0, BTreeSet::len);
        log::info!("supervisor: round: {n} module(s) on");
        self.feed(Ev::Round { on });
    }

    /// The modules Start normally would start (id, name): the search's candidates.
    pub fn startable(&self) -> Vec<(String, String)> {
        self.0.policy.lock().unwrap().startable()
    }

    /// Back to the version kept of `id` (the search's Go back), before Start normally.
    pub fn roll_back(&self, id: &str) {
        self.run(vec![Cmd::RollBack { id: id.into() }]);
    }

    /// The hub folder.
    pub fn home(&self) -> &Path {
        &self.0.home
    }

    /// The text of **Copy diagnostics** for a module (`crate::diagnostics`): ids, exit codes,
    /// crash times, the ladder steps tried, versions and the log tail; the profile folder and
    /// the user name redacted. `hub_version` is hideout.exe's.
    pub fn diagnostics(&self, id: &str, hub_version: &str) -> String {
        use crate::diagnostics::{log_tail, report, Input};
        let (now, wall) = (Instant::now(), SystemTime::now());
        let mut i = Input {
            module: id.into(),
            name: id.into(),
            hub_version: hub_version.into(),
            pid: self.pid(id),
            ..Input::default()
        };
        {
            let p = self.0.policy.lock().unwrap();
            i.state = p.state(id).and_then(|s| s.id()).map(String::from);
            i.crashes_in_window = p.crashes(id, now);
            i.stops = p.stops(id);
            i.history = p
                .history(id)
                .into_iter()
                .map(|(at, what)| (wall - now.saturating_duration_since(at), what))
                .collect();
        }
        let fact = |k: &str| {
            self.0.hub.read(|r| {
                r.fact(&format!("module.{id}.{k}"))
                    .and_then(Value::as_str)
                    .map(String::from)
            })
        };
        i.recovery = fact("recovery");
        i.previous = fact("previous");
        if let Ok(reg) = Registry::open_from_env(&self.0.home) {
            i.version = reg.inventory.modules.get(id).map(|m| m.version.clone());
            i.catalog = catalog_line(&reg, id);
            if let Some(m) = reg.manifest(id) {
                i.name = m.name;
            }
        }
        i.log_tail = log_tail(&self.0.home.join("logs").join(format!("{id}.log")));
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            let profile = profile.to_string_lossy().into_owned();
            i.hub_under_profile = under(&self.0.home.to_string_lossy(), &profile);
            i.redact.push((profile, "%USERPROFILE%".into()));
        }
        let name = user();
        if name.len() >= 3 {
            i.redact.push((name, "<user>".into()));
        } else if !name.is_empty() {
            // Short names only inside paths and pipe names, never as ordinary words.
            i.redact_words.push(crate::diagnostics::Word {
                from: name,
                to: "<user>".into(),
                in_paths: true,
            });
        }
        if let Some(pc) = std::env::var_os("COMPUTERNAME").filter(|c| !c.is_empty()) {
            i.redact_words.push(crate::diagnostics::Word {
                from: pc.to_string_lossy().into_owned(),
                to: "<computer>".into(),
                in_paths: false,
            });
        }
        i.windows = win::windows_version();
        if let Some(w) = self.0.withdrawals.lock().unwrap().get(id) {
            i.withdrawn = withdrawn_lines(w);
        }
        report(&i)
    }

    /// One event into the policy; carries out what it returns.
    pub fn feed(&self, ev: Ev) {
        let cmds = self.0.policy.lock().unwrap().handle(ev, Instant::now());
        self.run(cmds);
    }

    fn run(&self, cmds: Vec<Cmd>) {
        for c in cmds {
            match c {
                Cmd::Launch { id } => {
                    // The shallow verify refuses a revoked version (§12.5): it reads
                    // Withdrawn, not Couldn't start.
                    if let Some((reg, r)) = Registry::open_from_env(&self.0.home)
                        .ok()
                        .and_then(|reg| reg.revocation_of(&id).map(|r| (reg, r)))
                    {
                        self.withdraw(&reg, &id, r);
                        continue;
                    }
                    match self.launch(&id) {
                        Ok(()) => {}
                        Err(r) if r == NEEDS_HUB => {
                            log::warn!("supervisor: {id} not started: needs a newer hub");
                            self.feed(Ev::NeedsHub { id });
                        }
                        Err(r) if r.starts_with(NO_SANDBOX) => {
                            log::warn!("supervisor: {id} not started: {r}");
                            self.feed(Ev::LaunchFailed {
                                id,
                                reason: words::NO_SANDBOX.into(),
                            });
                        }
                        Err(reason) => {
                            log::warn!("supervisor: {id} not started: {reason}");
                            self.feed(Ev::LaunchFailed {
                                id,
                                reason: words::DIDNT_START.into(),
                            });
                        }
                    }
                }
                Cmd::Drain { id, force } => {
                    let reason = if force { "force" } else { "update" };
                    self.call(
                        &id,
                        "module.drain",
                        serde_json::json!({ "reason": reason }),
                        true,
                    );
                }
                Cmd::Exit { id } => self.call(&id, "module.exit", serde_json::json!({}), false),
                Cmd::Kill { id } => {
                    if let Some(l) = self.0.live.lock().unwrap().get(&id) {
                        l.kill(1);
                    }
                }
                Cmd::Publish { key, value } => self.0.hub.act(|r| r.publish(&key, value)),
                Cmd::Notify {
                    id,
                    title,
                    body,
                    actions,
                } => {
                    log::info!("supervisor: notify {id}: {title} / {body}");
                    match self.0.notifier.lock().unwrap().as_mut() {
                        Some(n) => n(&id, &title, &body, &actions),
                        None => {
                            let mut early = self.0.early.lock().unwrap();
                            if early.len() < EARLY_NOTES {
                                early.push((id, title, body, actions));
                            }
                        }
                    }
                }
                Cmd::ResetSettings { id } => self.reset_settings(&id),
                Cmd::CheckUpdates { id } => self.run_check(&id),
                Cmd::RememberNotified { id, version } => {
                    let mut set = load_notified(&self.0.home);
                    set.insert(format!("{id}@{version}"));
                    if let Err(e) = save_notified(&self.0.home, &set) {
                        log::warn!("supervisor: run\\withdrawn-notified.json not written: {e}");
                    }
                }
                Cmd::RollBack { id } => {
                    let done = Registry::open_from_env(&self.0.home)
                        .map_err(|e| format!("{e:?}"))
                        .and_then(|mut r| r.roll_back(&id).map_err(|e| format!("{e:?}")));
                    match done {
                        Ok(v) => log::info!("supervisor: {id} went back to {v}"),
                        Err(e) => log::warn!("supervisor: {id} rollback failed: {e}"),
                    }
                }
            }
        }
        let _ = self.0.nudge.lock().unwrap().send(());
    }

    /// The settings folder becomes `settings.bad-<time>` (§7.8); the hub never reads it.
    fn reset_settings(&self, id: &str) {
        let Ok(reg) = Registry::open_from_env(&self.0.home) else {
            return;
        };
        let Some(m) = reg.inventory.modules.get(id) else {
            return;
        };
        let dir = reg.module_dir(id, &m.version);
        let from = dir.join("settings");
        if from.exists() {
            let to = dir.join(format!("settings.bad-{}", unix_ms() / 1000));
            if let Err(e) = std::fs::rename(&from, &to) {
                log::warn!("supervisor: {id}: settings not renamed: {e}");
            }
        }
    }

    /// The file check, then the exe in a new Job Object with the launch token (§7.5).
    fn launch(&self, id: &str) -> Result<(), String> {
        let reg = Registry::open_from_env(&self.0.home).map_err(|e| format!("{e:?}"))?;
        let l = reg.pre_launch(id).map_err(|e| match e.id() {
            Some(NEEDS_HUB) => NEEDS_HUB.to_string(),
            _ => format!("{e:?}"),
        })?;
        let run = l.run.clone().ok_or("no run in the manifest")?;
        let exe = {
            let p = PathBuf::from(&run[0]);
            if p.is_absolute() {
                p
            } else {
                l.dir.join(p)
            }
        };
        let sha256 = sha256_file(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        let instance = self.0.next_instance.fetch_add(1, Ordering::Relaxed);
        let caps = self.caps_of(Some(&reg), id);
        let job = Arc::new(
            Job::create(&Job::name_for(&self.job_user(), id, instance), caps)
                .map_err(|e| format!("job: {e}"))?,
        );
        // An AppContainer when the hub's setting asks for one (§7.7), planned from what the
        // user GRANTED: a granted `net` or `files` runs outside (its permission line said so);
        // everything else, stdio modules included (#1092), is contained; a plan that can't be
        // made or a container that can't be set up fails the launch. A module never runs
        // uncontained without a word.
        let container = if reg.hub.appcontainer() {
            self.container_for(id, &l.dir, &l.granted, l.stdio)?
        } else {
            None
        };
        let mut token = String::new();
        let child = if let (Some(c), true) = (&container, l.stdio) {
            // The bridge (§8.2) in its container (#1092): stdin and stdout are anonymous pipes
            // the hub made, given in the handle list, attached before it runs.
            let env = [("HIDEOUT_INSTANCE", instance.to_string())];
            let (s, to_child, from_child) = c
                .spawn_stdio(&job, &exe, &run[1..], &l.dir, &env, self.log_file(id))
                .map_err(|e| format!("AppContainer start: {e}"))?;
            self.0.hub.attach_in(
                hideout_router::Origin::Bridged(id.into()),
                Some(s.pid),
                Some(&c.sid_string),
                from_child,
                to_child,
            );
            let (p, _) = s.resume().map_err(|e| format!("AppContainer start: {e}"))?;
            Child::Contained(p)
        } else if let Some(c) = &container {
            self.0.hub.act(|r| {
                token = r.issue_token(id);
                Vec::new()
            });
            let env = [
                ("HIDEOUT_PIPE", self.0.hub.router_pipe()),
                ("HIDEOUT_TOKEN", token.clone()),
                ("HIDEOUT_INSTANCE", instance.to_string()),
                ("HIDEOUT_HUB_PID", std::process::id().to_string()),
            ];
            let (p, _) = c
                .spawn(&job, &exe, &run[1..], &l.dir, &env, self.log_file(id))
                .and_then(|s| s.resume())
                .map_err(|e| format!("AppContainer start: {e}"))?;
            Child::Contained(p)
        } else if l.stdio {
            let mut cmd = Command::new(&exe);
            cmd.args(&run[1..]).current_dir(&l.dir).stderr(self.log(id));
            // The legacy bridge (§8.2): stdin and stdout are the router connection.
            cmd.env("HIDEOUT_INSTANCE", instance.to_string());
            let hub = self.0.hub.clone();
            Child::Plain(
                job.spawn_with(cmd, |c| hub.spawn_stdio(id, c))
                    .map_err(|e| format!("{e}"))?,
            )
        } else {
            self.0.hub.act(|r| {
                token = r.issue_token(id);
                Vec::new()
            });
            let mut cmd = Command::new(&exe);
            cmd.args(&run[1..]).current_dir(&l.dir).stderr(self.log(id));
            cmd.env("HIDEOUT_PIPE", self.0.hub.router_pipe())
                .env("HIDEOUT_TOKEN", &token)
                .env("HIDEOUT_INSTANCE", instance.to_string())
                .env("HIDEOUT_HUB_PID", std::process::id().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null());
            Child::Plain(job.spawn(cmd).map_err(|e| format!("{e}"))?)
        };
        let pid = child.id();
        let started = child
            .started()
            .map_err(|e| log::warn!("supervisor: {id}: start time of pid {pid}: {e}"))
            .ok();
        self.0.expected.lock().unwrap().insert(
            id.into(),
            Expected {
                pid,
                exe: exe.clone(),
                sha256: sha256.clone(),
            },
        );
        self.0.live.lock().unwrap().insert(
            id.into(),
            Live {
                instance,
                pid,
                job: Some(job.clone()),
                process: None,
            },
        );
        self.0.records.lock().unwrap().modules.insert(
            id.into(),
            Record {
                instance,
                pid,
                started,
                token,
                exe,
                sha256,
                job: job.name.clone(),
            },
        );
        self.save_records();
        log::info!(
            "supervisor: started {id} {} (pid {pid}){}",
            l.version,
            container.as_ref().map_or(String::new(), |c| format!(
                " in AppContainer {}",
                c.sid_string
            ))
        );
        self.feed(Ev::Started {
            id: id.into(),
            instance,
        });
        let me = Arc::downgrade(&self.0);
        let id = id.to_string();
        let mut child = child;
        std::thread::Builder::new()
            .name(format!("hideout-supervisor-{id}"))
            .spawn(move || {
                let code = child.wait();
                ended(me, &id, instance, code);
            })
            .map_err(|e| format!("{e}"))?;
        Ok(())
    }

    /// Module `id`'s Job Object caps (§7.7, v1.10): its manifest's `[limits]`, clamped to the
    /// hub's maximum. A value over the maximum is logged once per launch, not refused.
    fn caps_of(&self, reg: Option<&Registry>, id: &str) -> Caps {
        let limits = reg.and_then(|r| r.manifest(id)).and_then(|m| m.limits);
        let max = self.0.caps;
        let caps = Caps::for_module(limits.as_ref(), max);
        if let Some(l) = limits {
            let asked = Caps::for_module(
                Some(&l),
                Caps {
                    memory: usize::MAX,
                    cpu_rate: u32::MAX,
                },
            );
            if asked.memory > caps.memory || asked.cpu_rate > caps.cpu_rate {
                log::info!(
                    "supervisor: {id}: [limits] over the hub's maximum; clamped to {} MiB, {} %",
                    caps.memory >> 20,
                    caps.cpu_rate / 100
                );
            }
        }
        caps
    }

    /// Takes back a module the last hub started (§7.9), already checked by [`examine`]: a fresh
    /// Job Object (the old one's name went with the last hub), or none if Windows refuses, and
    /// its original token for the reconnect.
    fn adopt(&self, id: &str, rec: &Record, p: Process) -> Result<(), String> {
        let p = Arc::new(p);
        let reg = Registry::open_from_env(&self.0.home).ok();
        let job = Job::create(
            &Job::name_for(&self.job_user(), id, rec.instance),
            self.caps_of(reg.as_ref(), id),
        )
        .and_then(|j| j.assign(&p).map(|()| j));
        let job = match job {
            Ok(j) => Some(Arc::new(j)),
            Err(e) => {
                log::warn!(
                    "supervisor: {id} (pid {}) taken back without a Job Object ({e}); its first job's caps still hold",
                    rec.pid
                );
                None
            }
        };
        // A contained survivor keeps its AppContainer: this hub's pipe lets its SID in before
        // it reconnects (§7.7, §7.9).
        match crate::sandbox::container_sid(&p) {
            // Its own profile's container only: a survivor in any other container isn't
            // let in (its token can't be spoofed, but its record could be another's).
            Ok(Some(sid)) if crate::sandbox::profile_sid(id).as_deref() != Some(sid.as_str()) => {
                return Err(format!(
                    "pid {} runs in another AppContainer ({sid})",
                    rec.pid
                ));
            }
            Ok(Some(sid)) => self
                .0
                .hub
                .allow_pipe_client(&sid)
                .map_err(|e| format!("router pipe for its AppContainer: {e}"))?,
            Ok(None) => {}
            Err(e) => log::warn!(
                "supervisor: {id}: AppContainer of pid {} unknown: {e}",
                rec.pid
            ),
        }
        let mut rec = rec.clone();
        rec.job = job.as_ref().map(|j| j.name.clone()).unwrap_or_default();
        self.0.hub.act(|r| {
            r.set_token(id, &rec.token);
            Vec::new()
        });
        self.0.expected.lock().unwrap().insert(
            id.into(),
            Expected {
                pid: rec.pid,
                exe: rec.exe.clone(),
                sha256: rec.sha256.clone(),
            },
        );
        self.0.live.lock().unwrap().insert(
            id.into(),
            Live {
                instance: rec.instance,
                pid: rec.pid,
                job,
                process: Some(p.clone()),
            },
        );
        self.0
            .records
            .lock()
            .unwrap()
            .modules
            .insert(id.into(), rec.clone());
        let me = Arc::downgrade(&self.0);
        let (tid, instance) = (id.to_string(), rec.instance);
        std::thread::Builder::new()
            .name(format!("hideout-supervisor-{id}"))
            .spawn(move || {
                let code = p.wait().unwrap_or(1);
                ended(me, &tid, instance, code);
            })
            .map_err(|e| e.to_string())?;
        log::info!("supervisor: taking back {id} (pid {})", rec.pid);
        Ok(())
    }

    /// The module's log: stderr appended to `logs\<id>.log` (§7.7).
    fn log(&self, id: &str) -> Stdio {
        self.log_file(id)
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null)
    }

    fn log_file(&self, id: &str) -> Option<std::fs::File> {
        let dir = self.0.home.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{id}.log")))
            .ok()
    }

    /// Module `id`'s AppContainer (§7.7), set up: its version folder readable, `settings\`
    /// and `data\` writable, the router pipe open to it. `None` when its grants keep it outside
    /// (logged); an error starting with [`NO_SANDBOX`] when it may not run at all.
    fn container_for(
        &self,
        id: &str,
        dir: &Path,
        granted: &[String],
        stdio: bool,
    ) -> Result<Option<crate::sandbox::Container>, String> {
        use crate::contain::Plan;
        let caps = match crate::contain::plan_for(granted) {
            Plan::Contained(c) => c,
            Plan::Outside(why) => {
                log::info!("supervisor: {id} runs outside an AppContainer: {why}");
                return Ok(None);
            }
            Plan::Refused(why) => return Err(format!("{NO_SANDBOX}: {why}")),
        };
        let set_up = || -> std::io::Result<crate::sandbox::Container> {
            let c = crate::sandbox::Container::for_module(id, &caps)?;
            c.grant(dir, false)?;
            for sub in ["settings", hideout_registry::DATA_DIR] {
                let d = dir.join(sub);
                std::fs::create_dir_all(&d)?;
                c.grant(&d, true)?;
            }
            // A stdio module never opens the router pipe: it isn't let in, and the pipe keeps
            // its Medium label for it (#1092).
            if !stdio {
                self.0.hub.allow_pipe_client(&c.sid_string)?;
            }
            Ok(c)
        };
        set_up()
            .map(Some)
            .map_err(|e| format!("AppContainer for {id}: {e}"))
    }

    fn save_records(&self) {
        if let Err(e) = self.0.records.lock().unwrap().save(&self.0.home) {
            log::warn!("supervisor: run\\instances.json not written: {e}");
        }
    }
}

/// Copy diagnostics' catalog line (#1049): the source a catalog module came from, with the
/// sequence and accepted time of the catalog stored for it. `None` for a sideload or lab module.
fn catalog_line(reg: &Registry, id: &str) -> Option<String> {
    let m = reg.inventory.modules.get(id)?;
    if m.source == hideout_registry::inventory::SIDELOAD
        || m.source == hideout_registry::inventory::LAB
    {
        return None;
    }
    Some(match reg.stored_catalog(&m.source) {
        Some(s) => format!(
            "source {}, sequence {}, accepted {}",
            m.source, s.sequence, s.accepted
        ),
        None => format!("source {}, no catalog accepted yet", m.source),
    })
}

/// Copy diagnostics' withdrawn lines (#1026): ids and the note as written, escaped.
fn withdrawn_lines(w: &Withdrawal) -> Vec<(String, String)> {
    let at = |t: SystemTime| {
        t.duration_since(UNIX_EPOCH)
            .map(|d| format!("{} (unix s)", d.as_secs()))
            .unwrap_or_default()
    };
    let mut v = vec![
        ("state".into(), "revoked".into()),
        ("withdrawn version".into(), w.version.clone()),
        (
            "previous version".into(),
            match (&w.previous, &w.go_back) {
                (None, _) => "none kept".into(),
                (Some(p), Some(_)) => format!("{p} (not withdrawn; Go back offered)"),
                (Some(p), None) => format!("{p} (withdrawn or its folder is gone; not offered)"),
            },
        ),
        ("source".into(), w.source.clone()),
        ("revocation action".into(), "revoke".into()),
        (
            "publisher's reason".into(),
            if w.reason.is_empty() {
                "(none given)".into()
            } else {
                format!("\"{}\"", words::escape_note(&w.reason))
            },
        ),
        (
            "leases ended with \"revoked\"".into(),
            w.leases_ended.to_string(),
        ),
    ];
    if let Some(t) = w.at {
        v.push(("stopped at".into(), at(t)));
    }
    v.push((
        "last check for updates".into(),
        w.last_check
            .as_ref()
            .map_or("not yet".into(), |(t, o)| format!("{}: {o}", at(*t))),
    ));
    v
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// A started instance's process: a plain child, or one in an AppContainer.
enum Child {
    Plain(std::process::Child),
    Contained(Process),
}

impl Child {
    fn id(&self) -> u32 {
        match self {
            Child::Plain(c) => c.id(),
            Child::Contained(p) => p.pid(),
        }
    }

    fn started(&self) -> std::io::Result<u64> {
        match self {
            Child::Plain(c) => win::child_started(c),
            Child::Contained(p) => p.started(),
        }
    }

    /// Blocks until it ends; its exit code (1 when it can't be read).
    fn wait(&mut self) -> i64 {
        match self {
            Child::Plain(c) => c.wait().ok().and_then(|s| s.code()).map_or(1, |c| c as i64),
            Child::Contained(p) => p.wait().unwrap_or(1),
        }
    }
}

/// An instance's process ended: forget it, then tell the policy.
fn ended(me: Weak<Inner>, id: &str, instance: u64, code: i64) {
    let Some(inner) = me.upgrade() else { return };
    let sup = Supervisor(inner);
    {
        let mut live = sup.0.live.lock().unwrap();
        if live.get(id).is_some_and(|l| l.instance == instance) {
            live.remove(id);
            sup.0.expected.lock().unwrap().remove(id);
            sup.0.records.lock().unwrap().modules.remove(id);
        }
    }
    sup.save_records();
    log::info!("supervisor: {id} ended with {code}");
    sup.feed(Ev::Exited {
        id: id.into(),
        instance,
        code,
    });
}

/// Sleeps until the policy's next deadline, or until nudged after any change.
fn timer(me: Weak<Inner>, wake: Receiver<()>) {
    loop {
        let Some(inner) = me.upgrade() else { return };
        let deadline = inner.policy.lock().unwrap().next_deadline();
        drop(inner);
        let got = match deadline {
            None => wake.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(d) => wake.recv_timeout(d.saturating_duration_since(Instant::now())),
        };
        match got {
            Ok(()) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                let Some(inner) = me.upgrade() else { return };
                let sup = Supervisor(inner);
                let cmds = sup.0.policy.lock().unwrap().tick(Instant::now());
                sup.run(cmds);
            }
        }
    }
}
