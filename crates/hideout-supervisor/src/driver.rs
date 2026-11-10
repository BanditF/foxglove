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
use hideout_router::server::{router_pipe_name, user, Hub};
use hideout_router::{ModuleEntry, Out};
use serde_json::Value;

use crate::instances::{Instances, Record};
use crate::policy::{Cmd, Ev, Policy};
use crate::sha256_file;
use crate::win::{self, Caps, Job, Process};
use crate::words;

/// A notification for the shell (§7.7): title, body, buttons, the module it is about.
pub type Notifier = Box<dyn FnMut(&str, &str, &str, &[String]) + Send>;

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
}

/// The supervisor. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Supervisor(Arc<Inner>);

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
        }));
        hub.act(|r| {
            r.set_launch_check(Box::new(move |id, pid| launch_check(&expected, id, pid)));
            Vec::new()
        });
        let safe = hub.read(|r| r.fact("hub.safe_mode").and_then(Value::as_bool)) == Some(true);
        sup.0.policy.lock().unwrap().safe_mode = safe;
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
        sup
    }

    /// The pid of the module's live instance (started or taken back), if any.
    pub fn pid(&self, id: &str) -> Option<u32> {
        self.0.live.lock().unwrap().get(id).map(|l| l.pid)
    }

    /// Where the shell's notifications go (§7.7). Without one they are logged.
    pub fn set_notifier(&self, n: Notifier) {
        *self.0.notifier.lock().unwrap() = Some(n);
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

    /// One event into the policy; carries out what it returns.
    pub fn feed(&self, ev: Ev) {
        let cmds = self.0.policy.lock().unwrap().handle(ev, Instant::now());
        self.run(cmds);
    }

    fn run(&self, cmds: Vec<Cmd>) {
        for c in cmds {
            match c {
                Cmd::Launch { id } => {
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
                    if let Some(n) = self.0.notifier.lock().unwrap().as_mut() {
                        n(&id, &title, &body, &actions);
                    }
                }
                Cmd::ResetSettings { id } => self.reset_settings(&id),
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
            Job::create(&Job::name_for(&user(), id, instance), self.0.caps)
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
            cmd.env("HIDEOUT_PIPE", router_pipe_name())
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
        let job = Job::create(&Job::name_for(&user(), id, rec.instance), self.0.caps)
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
