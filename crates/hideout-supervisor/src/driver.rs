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
        let mut runnable = BTreeSet::new();
        if let Some(reg) = &reg {
            for id in reg.inventory.modules.keys() {
                let manifest = reg.manifest(id);
                let activation = manifest
                    .as_ref()
                    .map(|m| m.activation_on.clone())
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| vec!["enable".into()]);
                let runs = manifest.as_ref().is_some_and(|m| m.run.is_some());
                let name = manifest
                    .as_ref()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| id.clone());
                if activation.iter().any(|a| a == "call") || name != *id {
                    if let Some(mut e) = ModuleEntry::from_registry(reg, id) {
                        e.activate_on_call = activation.iter().any(|a| a == "call");
                        e.name = name.clone();
                        hub.act(|r| {
                            r.add_module(e);
                            Vec::new()
                        });
                    }
                }
                if !runs {
                    continue;
                }
                if reg.hub.enabled(id) {
                    runnable.insert(id.clone());
                }
                known.push(Ev::Known {
                    id: id.clone(),
                    name,
                    enabled: reg.hub.enabled(id),
                    at_enable: activation.iter().any(|a| a == "enable"),
                    previous: reg.inventory.modules[id].previous.clone(),
                });
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
            _ => return,
        };
        self.feed(ev);
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
        let saved = Registry::open(&self.0.home, hideout_registry::dev_mode_from_env())
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
        let saved = Registry::open(&self.0.home, hideout_registry::dev_mode_from_env())
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
        Ok(())
    }

    /// **Start normally** (§7.10, the safe-mode banner and the tray menu): clears `hideout.toml`'s
    /// `safe_mode` flag, turns `hub.safe_mode` off and starts the enabled modules, all in this
    /// hub: no restart. Safe to press twice. The flag not being saved is logged; the modules
    /// start anyway, and the next hub start would be in safe mode again.
    pub fn start_normally(&self) {
        let saved = Registry::open(&self.0.home, hideout_registry::dev_mode_from_env())
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
                    if let Err(reason) = self.launch(&id) {
                        log::warn!("supervisor: {id} not started: {reason}");
                        self.feed(Ev::LaunchFailed {
                            id,
                            reason: words::DIDNT_START.into(),
                        });
                    }
                }
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
        let l = reg.pre_launch(id).map_err(|e| format!("{e:?}"))?;
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
        let job = Arc::new(
            Job::create(&Job::name_for(&self.job_user(), id, instance), self.0.caps)
                .map_err(|e| format!("job: {e}"))?,
        );
        let mut cmd = Command::new(&exe);
        cmd.args(&run[1..]).current_dir(&l.dir).stderr(self.log(id));
        let mut token = String::new();
        let child = if l.stdio {
            // The legacy bridge (§8.2): stdin and stdout are the router connection.
            cmd.env("HIDEOUT_INSTANCE", instance.to_string());
            let hub = self.0.hub.clone();
            job.spawn_with(cmd, |c| hub.spawn_stdio(id, c))
                .map_err(|e| format!("{e}"))?
        } else {
            self.0.hub.act(|r| {
                token = r.issue_token(id);
                Vec::new()
            });
            cmd.env("HIDEOUT_PIPE", self.0.hub.router_pipe())
                .env("HIDEOUT_TOKEN", &token)
                .env("HIDEOUT_INSTANCE", instance.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null());
            job.spawn(cmd).map_err(|e| format!("{e}"))?
        };
        let pid = child.id();
        let started = win::child_started(&child)
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
        log::info!("supervisor: started {id} {} (pid {pid})", l.version);
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
                let code = child
                    .wait()
                    .ok()
                    .and_then(|s| s.code())
                    .map_or(1, |c| c as i64);
                ended(me, &id, instance, code);
            })
            .map_err(|e| format!("{e}"))?;
        Ok(())
    }

    /// Takes back a module the last hub started (§7.9), already checked by [`examine`]: a fresh
    /// Job Object (the old one's name went with the last hub), or none if Windows refuses, and
    /// its original token for the reconnect.
    fn adopt(&self, id: &str, rec: &Record, p: Process) -> Result<(), String> {
        let p = Arc::new(p);
        let job = Job::create(
            &Job::name_for(&self.job_user(), id, rec.instance),
            self.0.caps,
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
        let dir = self.0.home.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{id}.log")))
            .map(Stdio::from)
            .unwrap_or_else(|_| Stdio::null())
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
