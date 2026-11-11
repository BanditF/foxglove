//! Runs the supervisor transcripts (tests/hideout-vectors/transcripts/supervisor, and the
//! supervisor's part of lifecycle/ and hub/) against [`Policy`]. Their steps are prose events
//! (crash at t=…, restart after …) plus the facts the hub publishes and an `expect` block; each
//! scenario below plays the events on a fake clock and checks the facts and the expectations.
//! The router test (crates/hideout-router/tests/transcripts.rs) skips these files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hideout_supervisor::policy::{Cmd, Ev, Policy, State};
use hideout_supervisor::words;
use serde_json::Value;

const ART: &str = "foxglove.example-art";
const NAME: &str = "Example Art";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors/transcripts")
}

/// A policy on a fake clock, with every command it gave.
struct Sim {
    p: Policy,
    t0: Instant,
    now: Instant,
    cmds: Vec<(u64, Cmd)>,
    facts: BTreeMap<String, Value>,
    instance: u64,
    /// Each module's current instance (the driver's answers to its launches).
    instances: BTreeMap<String, u64>,
    /// The state fact right after the last crash.
    after_crash: Option<String>,
}

impl Sim {
    fn new() -> Sim {
        let t0 = Instant::now();
        Sim {
            p: Policy::new(),
            t0,
            now: t0,
            cmds: Vec::new(),
            facts: BTreeMap::new(),
            instance: 0,
            instances: BTreeMap::new(),
            after_crash: None,
        }
    }

    fn ms(&self) -> u64 {
        (self.now - self.t0).as_millis() as u64
    }

    fn take(&mut self, cmds: Vec<Cmd>) {
        let mut launched = Vec::new();
        for c in cmds {
            if let Cmd::Publish { key, value } = &c {
                self.facts.insert(key.clone(), value.clone());
            }
            if let Cmd::Launch { id } = &c {
                launched.push(id.clone());
            }
            self.cmds.push((self.ms(), c));
        }
        // The driver answers a launch with the instance it started.
        for id in launched {
            self.instance += 1;
            self.instances.insert(id.clone(), self.instance);
            let ev = Ev::Started {
                id,
                instance: self.instance,
            };
            let more = self.p.handle(ev, self.now);
            self.take(more);
        }
    }

    /// Records commands without answering their launches (a launch that fails in the driver).
    fn take_quiet(&mut self, cmds: Vec<Cmd>) {
        for c in cmds {
            if let Cmd::Publish { key, value } = &c {
                self.facts.insert(key.clone(), value.clone());
            }
            self.cmds.push((self.ms(), c));
        }
    }

    fn ev(&mut self, ev: Ev) {
        let cmds = self.p.handle(ev, self.now);
        self.take(cmds);
    }

    /// Moves the clock to `ms` after start, running every timer due on the way.
    fn at(&mut self, ms: u64) {
        let target = self.t0 + Duration::from_millis(ms);
        while let Some(d) = self.p.next_deadline().filter(|d| *d <= target) {
            self.now = self.now.max(d);
            let cmds = self.p.tick(self.now);
            self.take(cmds);
        }
        self.now = target;
    }

    /// Known, enabled, started at enable; then ready.
    fn running(&mut self) {
        self.ev(Ev::Known {
            id: ART.into(),
            name: NAME.into(),
            enabled: true,
            at_enable: true,
            previous: None,
        });
        self.ev(Ev::Ready { id: ART.into() });
    }

    fn crash(&mut self, s: u64) {
        self.at(s * 1000);
        let instance = self.instances.get(ART).copied().unwrap_or(0);
        self.ev(Ev::Exited {
            id: ART.into(),
            instance,
            code: 1,
        });
        self.after_crash = self.state().map(String::from);
        // A restarted instance gets ready at once.
        if self.p.state(ART) == Some(State::Restarting) {
            let due = self.p.restart_at(ART).unwrap();
            self.at((due - self.t0).as_millis() as u64);
            self.ev(Ev::Ready { id: ART.into() });
        }
    }

    fn state(&self) -> Option<&str> {
        self.facts
            .get(&format!("module.{ART}.state"))
            .and_then(Value::as_str)
    }

    fn launches(&self) -> Vec<u64> {
        self.cmds
            .iter()
            .filter(|(_, c)| matches!(c, Cmd::Launch { .. }))
            .map(|(t, _)| *t)
            .collect()
    }

    fn notifies(&self) -> Vec<&Cmd> {
        self.cmds
            .iter()
            .map(|(_, c)| c)
            .filter(|c| matches!(c, Cmd::Notify { .. }))
            .collect()
    }

    /// A known, enabled module that starts at enable, with an optional previous version.
    fn known(&mut self, id: &str, name: &str, previous: Option<&str>) {
        self.ev(Ev::Known {
            id: id.into(),
            name: name.into(),
            enabled: true,
            at_enable: true,
            previous: previous.map(String::from),
        });
    }

    fn ready(&mut self, id: &str) {
        self.ev(Ev::Ready { id: id.into() });
    }

    /// `module.<id>.<key>` as a string, `None` for null or unset.
    fn fact(&self, id: &str, key: &str) -> Option<&str> {
        self.facts
            .get(&format!("module.{id}.{key}"))
            .and_then(Value::as_str)
    }

    /// The commands given since `from` (an index into `cmds`).
    fn since(&self, from: usize) -> Vec<&Cmd> {
        self.cmds[from..].iter().map(|(_, c)| c).collect()
    }

    /// Ids of the launches and kills since `from`.
    fn touched(&self, from: usize) -> BTreeSet<String> {
        self.since(from)
            .into_iter()
            .filter_map(|c| match c {
                Cmd::Launch { id } | Cmd::Kill { id } | Cmd::Exit { id } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    /// Running Art, with an update staged (`asks`: the permissions it adds, in words).
    fn staged(&mut self, asks: Option<&str>) {
        self.known(ART, NAME, Some("0.3.0"));
        self.ready(ART);
        self.ev(Ev::UpdateStaged {
            id: ART.into(),
            version: "0.5.0".into(),
            previous: "0.4.0".into(),
            asks: asks.map(String::from),
        });
    }
}

const LINK: &str = "foxglove.example-link";
const LINK_NAME: &str = "Example Link";
const LIGHTS: &str = "hideout.example-lights";

fn has(cmds: &[&Cmd], f: impl Fn(&Cmd) -> bool) -> bool {
    cmds.iter().any(|c| f(c))
}

fn order(cmds: &[&Cmd]) -> (Option<usize>, Option<usize>) {
    (
        cmds.iter().position(|c| matches!(c, Cmd::RollBack { .. })),
        cmds.iter().position(|c| matches!(c, Cmd::Launch { .. })),
    )
}

/// The ways out and the UI name in a state transcript's `expect`, against the words.
fn names(ex: &Value, ui: &str, ways: &[&str]) -> Result<(), String> {
    // The vectors write "..." for the ellipsis and "<previous>" for a version.
    let plain = |s: &str| s.replace('\u{2026}', "...");
    if ex["ui_name"].as_str().map(plain) != Some(plain(ui)) {
        return Err(format!("ui name {} vs {ui}", ex["ui_name"]));
    }
    let want: Vec<&str> = ex["ways_out"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let same = want.len() == ways.len()
        && want
            .iter()
            .zip(ways)
            .all(|(w, g)| match w.split_once(" <") {
                Some((head, _)) => g.starts_with(&format!("{head} ")),
                None => w == g,
            });
    if !same {
        return Err(format!("ways out {want:?} vs {ways:?}"));
    }
    Ok(())
}

/// The `facts.changed` values a transcript says the hub publishes for `module.<id>.state`.
fn published_states(doc: &Value) -> Vec<String> {
    doc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| {
            let p = &s["msg"]["params"];
            (s["msg"]["method"] == "facts.changed" && p["key"].as_str()?.ends_with(".state"))
                .then(|| p["value"].as_str().unwrap().to_string())
        })
        .collect()
}

fn expect(doc: &Value) -> Value {
    doc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|s| s.get("expect").cloned())
        .unwrap_or(Value::Null)
}

fn five_crashes(s: &mut Sim, at: [u64; 5]) {
    for t in at {
        s.crash(t);
    }
}

/// Plays one transcript; `Err` says what differs.
fn play(file: &str, doc: &Value) -> Result<(), String> {
    let mut s = Sim::new();
    let ex = expect(doc);
    let check = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_string()) };
    match file {
        "supervisor/crash-backoff-and-stop.json" => {
            s.running();
            s.crash(0);
            s.crash(20);
            s.crash(60);
            s.crash(200);
            s.crash(500);
            // The first launch at 0, then exactly 1, 2, 4, 8 s after crashes 1 to 4.
            check(
                s.launches() == vec![0, 1_000, 22_000, 64_000, 208_000],
                &format!("launches {:?}", s.launches()),
            )?;
            check(s.state() == Some("crash-stopped"), "not crash-stopped")?;
            check(s.notifies().len() == 1, "not exactly one notification")?;
            let before = s.launches().len();
            s.at(5_000_000);
            let after = s.launches().len() - before;
            check(
                after as u64 == ex["restart_attempts_after_5th"].as_u64().unwrap(),
                "restarted after the 5th",
            )?;
            // A second stop in the same sign-in doesn't notify again.
            s.ev(Ev::StartAgain { id: ART.into() });
            s.ev(Ev::Ready { id: ART.into() });
            five_crashes(&mut s, [6_000, 6_010, 6_020, 6_030, 6_040]);
            check(s.notifies().len() == 1, "notified twice in one sign-in")?;
            check(
                s.facts[&format!("module.{ART}.recovery")] == "defaults",
                "the ladder's second step",
            )
        }
        "supervisor/crashes-spread-out.json" => {
            s.running();
            let mut most = 0;
            for t in [0, 200, 400, 600, 800] {
                s.crash(t);
                most = most.max(s.p.crashes(ART, s.now));
            }
            check(most <= 4, "the window held 5")?;
            check(s.after_crash.as_deref() == ex["state"].as_str(), "state")
        }
        "supervisor/exit-zero-is-stop.json" => {
            s.running();
            s.at(5_000);
            s.ev(Ev::PipeGone { id: ART.into() });
            s.ev(Ev::Exited {
                id: ART.into(),
                instance: 1,
                code: 0,
            });
            s.at(600_000);
            check(s.p.crashes(ART, s.now) == 0, "counted as a crash")?;
            check(s.launches().len() == 1, "restarted")?;
            check(
                matches!(s.p.state(ART), Some(State::Idle | State::Off)),
                "not off or idle",
            )?;
            // The next activation starts it again.
            s.ev(Ev::Activate { id: ART.into() });
            check(s.launches().len() == 2, "not started on activation")
        }
        "supervisor/no-rollback-after-10-min.json" | "supervisor/rollback-after-update.json" => {
            s.ev(Ev::Known {
                id: ART.into(),
                name: NAME.into(),
                enabled: true,
                at_enable: true,
                previous: None,
            });
            s.ev(Ev::Updated {
                id: ART.into(),
                version: "0.5.0".into(),
                previous: "0.4.0".into(),
            });
            s.ev(Ev::Ready { id: ART.into() });
            let late = file.contains("no-rollback");
            if late {
                five_crashes(&mut s, [900, 1000, 1100, 1200, 1300]);
            } else {
                five_crashes(&mut s, [100, 200, 300, 350, 400]);
            }
            let rolled = s
                .cmds
                .iter()
                .any(|(_, c)| matches!(c, Cmd::RollBack { .. }));
            if late {
                check(!rolled, "rolled back after 10 minutes")?;
                check(s.state() == ex["state"].as_str(), "state")
            } else {
                check(rolled, "no rollback")?;
                let said = s.notifies().iter().any(|c| {
                    matches!(c, Cmd::Notify { title, body, .. }
                        if title == "Went back to Example Art 0.4.0"
                            && body == "0.5.0 crashed 5 times after the update.")
                });
                check(said == ex["said_so"].as_bool().unwrap(), "didn't say so")?;
                // The rollback comes before the start of the previous version.
                let i = s
                    .cmds
                    .iter()
                    .position(|(_, c)| matches!(c, Cmd::RollBack { .. }))
                    .unwrap();
                check(
                    matches!(s.cmds.last().unwrap().1, Cmd::Launch { .. })
                        || s.cmds[i..]
                            .iter()
                            .any(|(_, c)| matches!(c, Cmd::Launch { .. })),
                    "the previous version wasn't started",
                )
            }
        }
        "supervisor/start-with-defaults.json" => {
            s.running();
            five_crashes(&mut s, [0, 10, 20, 30, 40]);
            check(s.state() == Some("crash-stopped"), "not stopped")?;
            let n = s.cmds.len();
            s.ev(Ev::StartDefaults { id: ART.into() });
            let after: Vec<&Cmd> = s.cmds[n..].iter().map(|(_, c)| c).collect();
            let reset = after
                .iter()
                .position(|c| matches!(c, Cmd::ResetSettings { .. }));
            let launch = after.iter().position(|c| matches!(c, Cmd::Launch { .. }));
            check(
                matches!((reset, launch), (Some(r), Some(l)) if r < l),
                "settings not reset before the start",
            )?;
            check(s.p.crashes(ART, s.now) == 0, "the count didn't start fresh")
        }
        "supervisor/state-crash-stopped.json" => {
            s.running();
            five_crashes(&mut s, [0, 10, 20, 30, 40]);
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            check(
                s.facts[&format!("module.{ART}.reason")]
                    .as_str()
                    .is_some_and(|r| !r.is_empty()),
                "no reason",
            )
        }
        "supervisor/state-restarting.json" => {
            s.running();
            s.at(1_000);
            s.ev(Ev::Exited {
                id: ART.into(),
                instance: 1,
                code: 3,
            });
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )
        }
        "supervisor/backoff-resets-after-quiet.json" => {
            // v1.8 §7.7: a run ends when the ten-minute window is empty; the next crash is step 1.
            s.running();
            let crashes = [0u64, 30, 90, 720];
            for t in crashes {
                s.crash(t);
            }
            // Each restart's delay: its launch less the crash before it.
            let launches = s.launches();
            let delays: Vec<u64> = crashes
                .iter()
                .zip(&launches[1..])
                .map(|(c, l)| (l - c * 1000) / 1000)
                .collect();
            check(
                Value::from(delays.clone()) == ex["restart_delays_s"],
                &format!("delays {delays:?}"),
            )?;
            check(
                s.p.crashes(ART, s.now) as u64 == ex["crash_in_window"].as_u64().unwrap(),
                "crashes in the window",
            )
        }
        "supervisor/outside-app-missing.json" => {
            // v1.4 §7.7: a busy port or a closed outside app is a module state. The module keeps
            // running and only patches its tile; the supervisor sees no exit and counts nothing.
            s.running();
            s.at(60_000);
            let ok = |s: &Sim| {
                s.state() == ex["module_state"].as_str()
                    && s.p.crashes(ART, s.now) as u64
                        == ex["supervisor_crashes_counted"].as_u64().unwrap()
            };
            check(ok(&s), "state or crash count while the port is busy")?;
            s.at(120_000);
            check(ok(&s), "state or crash count after Try again")?;
            check(s.launches().len() == 1, "restarted")
        }
        "supervisor/state-running.json" => {
            s.running();
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )
        }
        "supervisor/state-starting.json" => {
            s.ev(Ev::Known {
                id: ART.into(),
                name: NAME.into(),
                enabled: true,
                at_enable: true,
                previous: None,
            });
            check(s.launches() == vec![0], "not launched")?;
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )
        }
        "supervisor/state-off.json" => {
            s.running();
            s.ev(Ev::Disable { id: ART.into() });
            check(
                s.cmds.iter().any(|(_, c)| matches!(c, Cmd::Kill { .. })),
                "not stopped",
            )?;
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )
        }
        "supervisor/state-install-failed.json" => {
            s.ev(Ev::Known {
                id: ART.into(),
                name: NAME.into(),
                enabled: true,
                at_enable: true,
                previous: None,
            });
            s.ev(Ev::LaunchFailed {
                id: ART.into(),
                reason: "The download did not match its signature".into(),
            });
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact (file check)",
            )?;
            // The launch check at initialize (#959) reads the same.
            let mut s = Sim::new();
            s.running();
            s.ev(Ev::Refused { id: ART.into() });
            check(
                s.state() == Some("install-failed"),
                "state fact (launch check)",
            )
        }
        "supervisor/state-revoked.json" => {
            // v1.9 (#1026): the router's fact comes from Router::revoke (its own test); here the
            // stop, the state and the ways out that start nothing.
            s.ev(Ev::Known {
                id: ART.into(),
                name: NAME.into(),
                enabled: true,
                at_enable: true,
                previous: Some("0.4.2".into()),
            });
            s.ev(Ev::Ready { id: ART.into() });
            let launched = s.launches().len();
            s.ev(Ev::Revoked {
                id: ART.into(),
                version: "0.5.0".into(),
                go_back: Some("0.4.2".into()),
            });
            check(
                s.cmds.iter().any(|(_, c)| matches!(c, Cmd::Kill { .. })),
                "not stopped",
            )?;
            let states: Vec<String> = published_states(doc);
            check(
                states == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            check(
                s.facts.get(&format!("module.{ART}.previous")) == Some(&Value::from("0.4.2")),
                "previous fact",
            )?;
            check(s.notifies().len() == 1, "not notified once")?;
            for ev in [
                Ev::StartAgain { id: ART.into() },
                Ev::Activate { id: ART.into() },
                Ev::Enable { id: ART.into() },
            ] {
                s.ev(ev);
            }
            check(s.launches().len() == launched, "started again")?;
            check(s.state() == Some("revoked"), "left revoked")?;
            check(
                ex["ways_out"][0] == "Check for updates",
                "Check for updates is the first way out",
            )
        }
        "lifecycle/register-not-ready-in-30s.json" => {
            s.ev(Ev::Known {
                id: ART.into(),
                name: NAME.into(),
                enabled: true,
                at_enable: true,
                previous: None,
            });
            s.at(29_999);
            check(s.p.crashes(ART, s.now) == 0, "crashed before 30 s")?;
            s.at(30_000);
            check(
                s.cmds.iter().any(|(_, c)| matches!(c, Cmd::Kill { .. })),
                "not ended in its Job Object",
            )?;
            check(s.p.crashes(ART, s.now) == 1, "not counted as a crash")
        }
        "hub/hub-restart-takes-back.json" => {
            // The router runs the initialize step; the supervisor takes the instance back.
            s.ev(Ev::Known {
                id: ART.into(),
                name: NAME.into(),
                enabled: true,
                at_enable: false,
                previous: None,
            });
            s.ev(Ev::Adopted {
                id: ART.into(),
                instance: 7,
            });
            check(
                s.state() == Some("starting"),
                "not starting while taken back",
            )?;
            s.at(4_000);
            s.ev(Ev::Ready { id: ART.into() });
            check(s.state() == Some("running"), "not running")?;
            check(s.launches().is_empty(), "started a second instance")?;
            check(
                !s.facts.contains_key(&format!("module.{ART}.restart")),
                "showed a crash bar",
            )?;
            check(s.p.crashes(ART, s.now) == 0, "counted a crash")
        }
        "supervisor/root-cause-once.json" => {
            // Art requires Link; Lights opens a channel on Art. Link crashes: both read Waiting
            // for Link (the root, named once), and only Link restarts (§4.4, §7.8).
            for (id, name) in [(LINK, LINK_NAME), (ART, NAME), (LIGHTS, "Example Lights")] {
                s.known(id, name, None);
            }
            s.ev(Ev::Depends {
                id: ART.into(),
                on: [LINK.to_string()].into(),
            });
            s.ev(Ev::Depends {
                id: LIGHTS.into(),
                on: [ART.to_string()].into(),
            });
            for id in [LINK, ART, LIGHTS] {
                s.ready(id);
            }
            let from = s.cmds.len();
            s.at(1_000);
            let instance = s.instances[LINK];
            s.ev(Ev::Exited {
                id: LINK.into(),
                instance,
                code: 1,
            });
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            check(
                s.fact(LIGHTS, "state") == Some("waiting"),
                "lights not waiting",
            )?;
            check(
                s.fact(ART, "waiting") == Some(LINK) && s.fact(LIGHTS, "waiting") == Some(LINK),
                "the root isn't the one named",
            )?;
            check(
                s.fact(ART, "reason").is_some_and(|r| r.contains(LINK_NAME)),
                "reason doesn't name the root",
            )?;
            // The tray: only the root has a status line of its own ("Example Link:
            // Restarting"); the waiting ones point at it.
            let own: Vec<&str> = [LINK, ART, LIGHTS]
                .into_iter()
                .filter(|id| !matches!(s.fact(id, "state"), Some("running" | "waiting")))
                .collect();
            check(
                own == vec![LINK]
                    && ex["tray_lines"][0] == format!("{LINK_NAME}: Restarting")
                    && s.fact(LINK, "state") == Some("restarting"),
                "the tray shows more than the root",
            )?;
            // Back: the restart starts only Link, and the dependents read running again.
            let due = s.p.restart_at(LINK).unwrap();
            s.at((due - s.t0).as_millis() as u64);
            s.ready(LINK);
            let restarted: Vec<String> = ex["hub_restarted"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            check(
                s.touched(from).into_iter().collect::<Vec<_>>() == restarted,
                "restarted a dependent",
            )?;
            check(
                s.fact(ART, "state") == Some("running")
                    && s.fact(LIGHTS, "state") == Some("running")
                    && s.fact(ART, "waiting").is_none(),
                "dependents not running again",
            )
        }
        "supervisor/state-waiting.json" => {
            s.known(LINK, LINK_NAME, None);
            s.known(ART, NAME, None);
            s.ev(Ev::Depends {
                id: ART.into(),
                on: [LINK.to_string()].into(),
            });
            s.ready(LINK);
            s.ready(ART);
            // The provider turned off (a channel provider can be; `requires` refuses it).
            let from = s.cmds.len();
            s.ev(Ev::Disable { id: LINK.into() });
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            check(!s.touched(from).contains(ART), "the dependent was stopped")?;
            let root = s.fact(ART, "waiting").unwrap();
            check(root == LINK, "waiting names the wrong module")?;
            names(
                &ex,
                &words::waiting_for(LINK_NAME),
                &[&words::go_to(LINK_NAME)],
            )
        }
        "supervisor/state-needs-hub.json" => {
            // At hub start: the manifest's hub isn't served, so the driver knows it without a
            // start and says so.
            s.ev(Ev::Known {
                id: ART.into(),
                name: NAME.into(),
                enabled: true,
                at_enable: false,
                previous: Some("0.4.0".into()),
            });
            s.ev(Ev::NeedsHub { id: ART.into() });
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            check(s.fact(ART, "previous") == Some("0.4.0"), "previous fact")?;
            for ev in [
                Ev::Activate { id: ART.into() },
                Ev::StartAgain { id: ART.into() },
                Ev::Enable { id: ART.into() },
            ] {
                s.ev(ev);
            }
            check(s.launches().is_empty(), "started")?;
            names(
                &ex,
                words::NEEDS_HUB,
                &[words::UPDATE_HIDEOUT, &words::go_back("0.4.0")],
            )?;
            // Go back: the rollback, then the start.
            let from = s.cmds.len();
            s.ev(Ev::GoBack { id: ART.into() });
            check(
                matches!(order(&s.since(from)), (Some(r), Some(l)) if r < l),
                "Go back didn't roll back then start",
            )?;
            // At a launch (the driver's answer instead of Started): the same state.
            let mut s = Sim::new();
            s.p.handle(
                Ev::Known {
                    id: ART.into(),
                    name: NAME.into(),
                    enabled: true,
                    at_enable: false,
                    previous: None,
                },
                s.now,
            );
            let cmds = s.p.handle(Ev::Activate { id: ART.into() }, s.now);
            s.take_quiet(cmds);
            s.ev(Ev::NeedsHub { id: ART.into() });
            check(s.state() == Some("needs-hub"), "not needs-hub at a launch")
        }
        "supervisor/state-quarantined.json" => {
            s.known(ART, NAME, Some("0.4.0"));
            s.ready(ART);
            s.ev(Ev::Quarantine { id: ART.into() });
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            check(
                has(&s.since(0), |c| matches!(c, Cmd::Kill { .. })),
                "not stopped",
            )?;
            check(
                s.fact(ART, "reason").is_some_and(|r| !r.is_empty()),
                "no reason",
            )?;
            // Its exit is no crash; nothing starts it but the card.
            s.ev(Ev::Exited {
                id: ART.into(),
                instance: s.instances[ART],
                code: 1,
            });
            s.ev(Ev::Activate { id: ART.into() });
            s.at(120_000);
            check(s.launches().len() == 1, "started by itself")?;
            check(s.p.crashes(ART, s.now) == 0, "counted as a crash")?;
            names(
                &ex,
                &words::quarantined_title(NAME),
                &[words::START_AGAIN, &words::go_back("0.4.0")],
            )?;
            s.ev(Ev::StartAgain { id: ART.into() });
            check(s.launches().len() == 2, "Start again didn't start it")?;
            // Go back instead: the rollback before the start.
            let mut s = Sim::new();
            s.known(ART, NAME, Some("0.4.0"));
            s.ready(ART);
            s.ev(Ev::Quarantine { id: ART.into() });
            let from = s.cmds.len();
            s.ev(Ev::GoBack { id: ART.into() });
            check(
                matches!(order(&s.since(from)), (Some(r), Some(l)) if r < l),
                "Go back didn't roll back then start",
            )
        }
        "supervisor/state-update-busy.json" => {
            s.staged(None);
            check(
                has(&s.since(0), |c| {
                    matches!(c, Cmd::Drain { force: false, .. })
                }),
                "no drain",
            )?;
            let busy = |s: &mut Sim| {
                s.ev(Ev::Drained {
                    id: ART.into(),
                    busy: Some("a flash running".into()),
                })
            };
            busy(&mut s);
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            let said = doc["steps"][2]["msg"]["params"]["value"].as_str();
            check(s.fact(ART, "reason") == said, "reason line")?;
            names(&ex, words::UPDATE_READY, &[words::FORCE, words::NOT_NOW])?;
            // Never a timer: drained again only on module.idle or Force (UX6).
            let from = s.cmds.len();
            s.at(3_600_000);
            check(s.since(from).is_empty(), "drained on a timer")?;
            s.ev(Ev::Idle { id: ART.into() });
            check(
                has(&s.since(from), |c| {
                    matches!(c, Cmd::Drain { force: false, .. })
                }),
                "not drained on module.idle",
            )?;
            busy(&mut s);
            s.ev(Ev::Force { id: ART.into() });
            check(
                has(&s.since(from), |c| {
                    matches!(c, Cmd::Drain { force: true, .. })
                }),
                "Force didn't drain",
            )?;
            // A forced drain swaps whatever it answers.
            busy(&mut s);
            check(s.state() == Some("updating"), "Force didn't swap")?;
            // Not now on another busy update keeps the old version running.
            let mut s = Sim::new();
            s.staged(None);
            busy(&mut s);
            s.ev(Ev::NotNow { id: ART.into() });
            check(
                s.state() == Some("running") && s.launches().len() == 1,
                "Not now",
            )
        }
        "supervisor/state-updating.json" => {
            s.staged(None);
            s.at(10_000);
            let from = s.cmds.len();
            s.ev(Ev::Drained {
                id: ART.into(),
                busy: None,
            });
            check(
                published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                "state fact",
            )?;
            check(
                s.fact(ART, "reason") == Some(words::PLACE_KEPT),
                "reason line",
            )?;
            check(
                has(&s.since(from), |c| matches!(c, Cmd::Exit { .. })),
                "no module.exit",
            )?;
            check(s.launches().len() == 1, "started before the old one exited")?;
            // The old one exits: the new one starts, still "Updating…", then running.
            s.at(10_200);
            s.ev(Ev::Exited {
                id: ART.into(),
                instance: s.instances[ART],
                code: 0,
            });
            check(s.launches().len() == 2, "the new version didn't start")?;
            check(
                s.state() == Some("updating"),
                "not updating while it starts",
            )?;
            s.at(10_600);
            s.ready(ART);
            check(s.state() == Some("running"), "not running after the swap")?;
            let states: Vec<&str> = s.cmds[from..]
                .iter()
                .filter_map(|(_, c)| match c {
                    Cmd::Publish { key, value } if key.ends_with(".state") => value.as_str(),
                    _ => None,
                })
                .collect();
            check(
                states == ["updating", "running"],
                &format!("states in the swap {states:?}"),
            )?;
            check(s.p.crashes(ART, s.now) == 0, "the swap counted a crash")?;
            names(&ex, words::UPDATING, &[])?;
            // An old instance that doesn't exit is ended after 5 s: no crash, and the new one
            // starts on its end.
            let mut s = Sim::new();
            s.staged(None);
            s.ev(Ev::Drained {
                id: ART.into(),
                busy: None,
            });
            s.at(5_000);
            check(
                has(&s.since(0), |c| matches!(c, Cmd::Kill { .. })),
                "not ended after 5 s",
            )?;
            s.ev(Ev::Exited {
                id: ART.into(),
                instance: s.instances[ART],
                code: 1,
            });
            check(
                s.p.crashes(ART, s.now) == 0 && s.launches().len() == 2,
                "a stuck old instance counted a crash or blocked the start",
            )
        }
        "supervisor/state-update-waiting.json" | "supervisor/update-waiting-permission.json" => {
            let asks = words::update_asks(Some("see every key you press"), false);
            s.staged(Some(&asks));
            if file.ends_with("state-update-waiting.json") {
                check(
                    published_states(doc) == vec![s.state().unwrap_or_default().to_string()],
                    "state fact",
                )?;
                names(&ex, words::UPDATE_WAITING, &[words::REVIEW, words::NOT_NOW])?;
                check(s.fact(ART, "reason") == Some(asks.as_str()), "reason line")?;
            } else {
                check(s.state() == ex["state"].as_str(), "state")?;
            }
            // The old version keeps running: no drain, stop or start; no modal (notification).
            let quiet = !has(&s.since(0), |c| {
                matches!(
                    c,
                    Cmd::Drain { .. } | Cmd::Exit { .. } | Cmd::Kill { .. } | Cmd::Notify { .. }
                )
            });
            check(
                quiet && s.launches().len() == 1,
                "the old version didn't keep running quietly",
            )?;
            check(
                ex.get("modal_at_signin").is_none_or(|m| m == false) && s.notifies().is_empty(),
                "a modal",
            )?;
            // Not now, then Review from the Modules page: drained, then the swap.
            s.ev(Ev::NotNow { id: ART.into() });
            check(s.state() == Some("running"), "Not now")?;
            s.ev(Ev::Review { id: ART.into() });
            check(
                has(&s.since(0), |c| matches!(c, Cmd::Drain { .. })),
                "Review didn't drain",
            )?;
            s.ev(Ev::Drained {
                id: ART.into(),
                busy: None,
            });
            let after = doc["steps"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find_map(|st| st["expect"]["state"].as_str())
                .unwrap_or("updating");
            check(s.state() == Some(after), "not updating after Review")
        }
        other => Err(format!("no scenario for {other}")),
    }
}

/// Supervisor files with no scenario here, and why.
const NOT_HERE: &[(&str, &str)] = &[(
    "supervisor/provider-gone-error-2.json",
    "router behaviour, run by the router test",
)];

/// Files from other folders that are the supervisor's.
const ALSO: &[&str] = &[
    "lifecycle/register-not-ready-in-30s.json",
    "hub/hub-restart-takes-back.json",
];

#[test]
fn supervisor_transcripts_pass() {
    let mut files: Vec<String> = std::fs::read_dir(root().join("supervisor"))
        .unwrap()
        .map(|e| format!("supervisor/{}", e.unwrap().file_name().to_string_lossy()))
        .collect();
    files.sort();
    files.extend(ALSO.iter().map(|s| s.to_string()));
    let mut ran = 0;
    let mut failed = Vec::new();
    for f in &files {
        if let Some((_, why)) = NOT_HERE.iter().find(|(n, _)| n == f) {
            println!("skipped {f}: {why}");
            continue;
        }
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(root().join(f)).unwrap()).unwrap();
        match play(f, &doc) {
            Ok(()) => {
                ran += 1;
                println!("  ok {f}");
            }
            Err(e) => failed.push(format!("{f}: {e}")),
        }
    }
    assert!(failed.is_empty(), "failed:\n{}", failed.join("\n"));
    assert!(ran >= 22, "only {ran} ran");
}

#[test]
fn taken_back_module_not_back_in_grace_starts_fresh() {
    let mut s = Sim::new();
    s.ev(Ev::Known {
        id: ART.into(),
        name: NAME.into(),
        enabled: true,
        at_enable: false,
        previous: None,
    });
    s.ev(Ev::Adopted {
        id: ART.into(),
        instance: 7,
    });
    s.at(29_000);
    assert!(s.launches().is_empty());
    s.at(30_000);
    assert!(s.cmds.iter().any(|(_, c)| matches!(c, Cmd::Kill { .. })));
    assert_eq!(s.launches(), vec![30_000]);
    assert_eq!(s.p.crashes(ART, s.now), 0);
    // The old instance's exit afterwards changes nothing.
    s.ev(Ev::Exited {
        id: ART.into(),
        instance: 7,
        code: 1,
    });
    assert_eq!(s.p.crashes(ART, s.now), 0);
}

#[test]
fn a_frame_too_large_fault_is_one_crash() {
    let mut s = Sim::new();
    s.running();
    s.at(1_000);
    s.ev(Ev::Fault { id: ART.into() });
    s.ev(Ev::PipeGone { id: ART.into() });
    s.ev(Ev::Exited {
        id: ART.into(),
        instance: 1,
        code: 1,
    });
    assert_eq!(s.p.crashes(ART, s.now), 1);
    assert_eq!(s.state(), Some("restarting"));
}

#[test]
fn pipe_closed_without_exit_is_a_crash_after_the_wait() {
    let mut s = Sim::new();
    s.running();
    s.at(1_000);
    s.ev(Ev::PipeGone { id: ART.into() });
    s.at(5_999);
    assert_eq!(s.p.crashes(ART, s.now), 0);
    s.at(6_000);
    assert_eq!(s.p.crashes(ART, s.now), 1);
}

#[test]
fn backoff_steps_then_60() {
    use hideout_supervisor::policy::backoff;
    let got: Vec<u64> = (1..=9).map(|n| backoff(n).as_secs()).collect();
    assert_eq!(got, vec![1, 2, 4, 8, 16, 32, 60, 60, 60]);
}

#[test]
fn safe_mode_starts_nothing() {
    let mut s = Sim::new();
    s.p.safe_mode = true;
    s.ev(Ev::Known {
        id: ART.into(),
        name: NAME.into(),
        enabled: true,
        at_enable: true,
        previous: None,
    });
    s.ev(Ev::Activate { id: ART.into() });
    assert!(s.launches().is_empty());
}

/// The ladder (#983): start again, defaults, then go back with a previous version, or the last
/// step without one. Go back rolls back before the launch and clears the card's facts.
#[test]
fn the_ladder_ends_at_go_back_or_the_last_step() {
    let fact = |s: &Sim, k: &str| s.facts.get(&format!("module.{ART}.{k}")).cloned();
    for previous in [Some("0.4.2"), None] {
        let mut s = Sim::new();
        s.ev(Ev::Known {
            id: ART.into(),
            name: NAME.into(),
            enabled: true,
            at_enable: true,
            previous: previous.map(String::from),
        });
        s.ev(Ev::Ready { id: ART.into() });
        let mut steps = Vec::new();
        for round in 0..3u64 {
            let t = round * 10_000;
            five_crashes(&mut s, [t, t + 10, t + 20, t + 30, t + 40]);
            steps.push(fact(&s, "recovery").unwrap());
            if round < 2 {
                s.ev(Ev::StartAgain { id: ART.into() });
                s.ev(Ev::Ready { id: ART.into() });
            }
        }
        let want = if previous.is_some() {
            "go-back"
        } else {
            "last"
        };
        assert_eq!(steps, vec!["start-again", "defaults", want]);
        let shown = fact(&s, "previous").unwrap();
        assert_eq!(shown.as_str(), previous);
        let before = s.cmds.len();
        s.ev(Ev::GoBack { id: ART.into() });
        let after: Vec<&Cmd> = s.cmds[before..].iter().map(|(_, c)| c).collect();
        let rolled = after.iter().position(|c| matches!(c, Cmd::RollBack { .. }));
        let launched = after.iter().position(|c| matches!(c, Cmd::Launch { .. }));
        if previous.is_some() {
            assert!(rolled.unwrap() < launched.unwrap(), "{after:?}");
            assert_eq!(fact(&s, "recovery"), Some(Value::Null));
            assert_eq!(fact(&s, "previous"), Some(Value::Null));
        } else {
            // Nothing to go back to: the card stays on its last step.
            assert!(rolled.is_none() && launched.is_none(), "{after:?}");
        }
    }
}

/// A failed launch says it came from launching, so the tab reads "Couldn't start <Module>".
#[test]
fn a_launch_failure_is_marked_as_one() {
    let mut s = Sim::new();
    s.running();
    s.ev(Ev::LaunchFailed {
        id: ART.into(),
        reason: "x".into(),
    });
    assert_eq!(s.state(), Some("install-failed"));
    assert_eq!(s.facts[&format!("module.{ART}.failed")], "launch");
    s.ev(Ev::StartAgain { id: ART.into() });
    let instance = s.instance;
    s.ev(Ev::Exited {
        id: ART.into(),
        instance,
        code: 1,
    });
    assert_eq!(s.facts[&format!("module.{ART}.failed")], Value::Null);
}
