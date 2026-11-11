// SPDX-License-Identifier: MIT
//! #1096 review: the lab's stand-in update stages the running version again (`version ==
//! previous`). The policy reads it as a re-stage: it swaps as usual, but keeps no fake previous
//! version or update time, so five crashes after it stop the module without an automatic
//! rollback and no card offers "Go back to <the same version>". A real update in the same steps
//! still rolls back (the test tells the two apart). Pure policy on a fake clock.

use std::time::{Duration, Instant};

use hideout_supervisor::policy::{Cmd, Ev, Policy, State};
use serde_json::Value;

const ID: &str = "example.lab.swap";

struct Sim {
    p: Policy,
    now: Instant,
    instance: u64,
    cmds: Vec<Cmd>,
    previous_fact: Option<Value>,
}

impl Sim {
    fn new() -> Sim {
        Sim {
            p: Policy::new(),
            now: Instant::now(),
            instance: 0,
            cmds: Vec::new(),
            previous_fact: None,
        }
    }

    fn take(&mut self, cmds: Vec<Cmd>) {
        let mut launched = false;
        for c in cmds {
            match &c {
                Cmd::Publish { key, value } if key == &format!("module.{ID}.previous") => {
                    self.previous_fact = Some(value.clone());
                }
                Cmd::Launch { .. } => launched = true,
                _ => {}
            }
            self.cmds.push(c);
        }
        if launched {
            self.instance += 1;
            let more = self.p.handle(
                Ev::Started {
                    id: ID.into(),
                    instance: self.instance,
                },
                self.now,
            );
            self.take(more);
        }
    }

    fn ev(&mut self, ev: Ev) {
        let cmds = self.p.handle(ev, self.now);
        self.take(cmds);
    }

    fn advance(&mut self, by: Duration) {
        let target = self.now + by;
        while let Some(d) = self.p.next_deadline().filter(|d| *d <= target) {
            self.now = self.now.max(d);
            let cmds = self.p.tick(self.now);
            self.take(cmds);
        }
        self.now = target;
    }

    fn exited(&mut self) {
        let instance = self.instance;
        self.ev(Ev::Exited {
            id: ID.into(),
            instance,
            code: 1,
        });
    }

    /// Running, an update staged and swapped (drain done, old instance gone, new one ready),
    /// then five crashes a few seconds apart. Answers whether a rollback was asked for.
    fn swap_then_crash(&mut self, version: &str, previous: &str) -> bool {
        self.ev(Ev::Known {
            id: ID.into(),
            name: "Swap".into(),
            enabled: true,
            at_enable: true,
            previous: None,
        });
        self.ev(Ev::Ready { id: ID.into() });
        self.ev(Ev::UpdateStaged {
            id: ID.into(),
            version: version.into(),
            previous: previous.into(),
            asks: None,
        });
        assert!(self.cmds.iter().any(|c| matches!(c, Cmd::Drain { .. })));
        self.ev(Ev::Drained {
            id: ID.into(),
            busy: None,
        });
        assert_eq!(self.p.state(ID), Some(State::Updating));
        self.exited();
        self.ev(Ev::Ready { id: ID.into() });
        assert_eq!(self.p.state(ID), Some(State::Running));
        for _ in 0..5 {
            self.advance(Duration::from_secs(2));
            self.exited();
            if self.p.state(ID) == Some(State::Restarting) {
                let due = self.p.restart_at(ID).unwrap();
                self.advance(due.saturating_duration_since(self.now));
                self.ev(Ev::Ready { id: ID.into() });
            }
        }
        self.cmds.iter().any(|c| matches!(c, Cmd::RollBack { .. }))
    }
}

#[test]
fn a_lab_restage_keeps_no_previous_and_never_rolls_back() {
    let mut lab = Sim::new();
    let rolled = lab.swap_then_crash("0.1.0", "0.1.0");
    assert!(!rolled, "a re-stage of the same version rolled back");
    assert_eq!(lab.p.state(ID), Some(State::CrashStopped));
    assert!(
        lab.previous_fact.as_ref().is_none_or(Value::is_null),
        "Go back offered: {:?}",
        lab.previous_fact
    );

    // The same steps with a real update do roll back: the test tells them apart.
    let mut real = Sim::new();
    assert!(
        real.swap_then_crash("0.5.0", "0.4.0"),
        "a real update didn't roll back"
    );
}
