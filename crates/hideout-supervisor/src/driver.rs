// SPDX-License-Identifier: MIT
//! Drives the [`Policy`] with real processes (Windows): launches in Job Objects (§7.5, §7.7),
//! the launch check the router runs at `initialize`, adoption after a hub restart (§7.9),
//! `run\instances.json`, stderr to each module's log, and one timer thread that sleeps until
//! the policy's next deadline. Notices ends by events only: a thread per instance blocks on its
//! process handle; the router reports the pipe closing (no heartbeats, #281).

use std::collections::BTreeMap;
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
    job: Arc<Job>,
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

        let reg = match Registry::open(home, hideout_registry::dev_mode_from_env()) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("supervisor: registry not read ({e:?}); nothing to start");
                return sup;
            }
        };
        let old = Instances::load(home);
        let mut known = Vec::new();
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
                if let Some(mut e) = ModuleEntry::from_registry(&reg, id) {
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
            known.push(Ev::Known {
                id: id.clone(),
                name,
                enabled: reg.hub.enabled(id),
                at_enable: activation.iter().any(|a| a == "enable"),
                previous: reg.inventory.modules[id].previous.clone(),
            });
        }
        // Adoption first, so `Known` doesn't start a second instance of a module still running.
        let mut adopted = Vec::new();
        for (id, rec) in old.modules {
            if !reg.hub.enabled(&id) {
                continue;
            }
            match sup.adopt(&id, &rec) {
                Ok(()) => adopted.push(Ev::Adopted {
                    id,
                    instance: rec.instance,
                }),
                Err(e) => log::info!("supervisor: {id} not taken back: {e}"),
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
                        l.job.kill(1);
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
                    let done = Registry::open(&self.0.home, hideout_registry::dev_mode_from_env())
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
        let Ok(reg) = Registry::open(&self.0.home, hideout_registry::dev_mode_from_env()) else {
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
        let reg = Registry::open(&self.0.home, hideout_registry::dev_mode_from_env())
            .map_err(|e| format!("{e:?}"))?;
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
                job: job.clone(),
            },
        );
        self.0.records.lock().unwrap().modules.insert(
            id.into(),
            Record {
                instance,
                pid,
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

    /// Takes back a module the last hub started (§7.9): it still runs, its exe is the one
    /// recorded, and its Job Object is still there.
    fn adopt(&self, id: &str, rec: &Record) -> Result<(), String> {
        let p = Process::open(rec.pid).map_err(|e| format!("pid {}: {e}", rec.pid))?;
        let image = p.image().map_err(|e| format!("image: {e}"))?;
        if !same_path(&image, &rec.exe) {
            return Err(format!("pid {} is now {}", rec.pid, image.display()));
        }
        if sha256_file(&image).map_err(|e| e.to_string())? != rec.sha256 {
            return Err("exe hash changed".into());
        }
        let job = Arc::new(Job::open(&rec.job).map_err(|e| format!("job: {e}"))?);
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
                job,
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
