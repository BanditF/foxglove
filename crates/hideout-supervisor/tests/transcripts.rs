//! Runs the supervisor transcripts (tests/hideout-vectors/transcripts/supervisor, and the
//! supervisor's part of lifecycle/ and hub/) against [`Policy`]. Their steps are prose events
//! (crash at t=…, restart after …) plus the facts the hub publishes and an `expect` block; each
//! scenario below plays the events on a fake clock and checks the facts and the expectations.
//! The router test (crates/hideout-router/tests/transcripts.rs) skips these files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hideout_supervisor::policy::{Cmd, Ev, Policy, State};
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
            after_crash: None,
        }
    }

    fn ms(&self) -> u64 {
        (self.now - self.t0).as_millis() as u64
    }

    fn take(&mut self, cmds: Vec<Cmd>) {
        let mut launched = false;
        for c in cmds {
            if let Cmd::Publish { key, value } = &c {
                self.facts.insert(key.clone(), value.clone());
            }
            launched |= matches!(c, Cmd::Launch { .. });
            self.cmds.push((self.ms(), c));
        }
        // The driver answers a launch with the instance it started.
        if launched {
            self.instance += 1;
            let ev = Ev::Started {
                id: ART.into(),
                instance: self.instance,
            };
            let more = self.p.handle(ev, self.now);
            self.take(more);
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
        let instance = self.instance;
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
        other => Err(format!("no scenario for {other}")),
    }
}

/// Supervisor files with no scenario here, and why.
const NOT_HERE: &[(&str, &str)] = &[
    (
        "supervisor/provider-gone-error-2.json",
        "router behaviour, run by the router test",
    ),
    (
        "supervisor/root-cause-once.json",
        "the requires graph and Waiting for (later phase)",
    ),
    (
        "supervisor/state-waiting.json",
        "the requires graph and Waiting for (later phase)",
    ),
    ("supervisor/state-needs-hub.json", "the update step (H2.x)"),
    (
        "supervisor/state-quarantined.json",
        "the panel host's quarantine (§5.5)",
    ),
    ("supervisor/state-update-busy.json", "hot swap (§7.6)"),
    ("supervisor/state-update-waiting.json", "hot swap (§7.6)"),
    ("supervisor/state-updating.json", "hot swap (§7.6)"),
    (
        "supervisor/update-waiting-permission.json",
        "hot swap and the permission dialog",
    ),
];

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
    assert!(ran >= 14, "only {ran} ran");
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
