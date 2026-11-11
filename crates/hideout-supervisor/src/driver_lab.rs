// SPDX-License-Identifier: MIT
//! The lab hub's update verbs (#1096): drive a running lab module into **Update ready**,
//! **Update waiting for you** or **Updating…** through the supervisor's own update step, so
//! Test lab can check #1093's cards without a second version or a catalog.
//!
//! What is real: the policy's `UpdateStaged` (the step an install of a new version ends with,
//! §7.4), the `module.drain` call to the module over its pipe (§7.6), the answer coming back as
//! `Out::HubResult`, and from there Force, Not now, `module.idle`, `module.exit`, the 5 s exit
//! fallback and the new instance's start. What the lab stands in for:
//! - the new version is the one already loaded (a lab module has one folder): `version` and
//!   `previous` are both its version, and the swap starts the same files again. The policy reads
//!   that as a re-stage: the module's kept `previous` and its last update time stay as they
//!   were, so no "Go back to <same version>" and no automatic rollback after crashes;
//! - the drain answer's content: the first-party plugins have no drain handler, so their answer
//!   is an error (which counts as done). A lab verb sets, once, what the next drain answer of
//!   that module reads as (`busy` with a reason, or done) when it arrives;
//! - the added permission is named by the verb (a lab module's grants aren't saved, so there is
//!   no `pending_permissions` to read).
//!
//! Lab only: each entry point opens the registry from the environment and refuses unless lab
//! mode is on for this hub's folder (`HIDEOUT_LAB=1`, a non-default `HIDEOUT_CONFIG_DIR`) and the
//! module is a lab module (`<config>\lab-modules\<id>\`). The user's hub never has one.

use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use hideout_registry::Registry;

use super::{asks_words, Ev, Supervisor};
use crate::policy::State;

/// The lab's part of the supervisor: answers set by a verb, and a step counter every policy step
/// signals (a verb waits on it for the state it asked for).
#[derive(Default)]
pub(super) struct LabState {
    /// Per module: what its next drain answer reads as (`Some(reason)`: busy; `None`: done).
    answers: Mutex<BTreeMap<String, Option<String>>>,
    steps: Mutex<u64>,
    cv: Condvar,
}

impl LabState {
    /// One policy step ran (`Supervisor::feed`): wakes a verb's wait. Free while none waits.
    pub(super) fn stepped(&self) {
        *self.steps.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.cv.notify_all();
    }
}

/// What a lab update verb asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabUpdate {
    /// **Update ready**: the drain answers busy with `reason` ("Waits until you finish …").
    Ready { reason: String },
    /// **Update waiting for you**: the update adds these permissions (§6.4), so it waits for
    /// Review; nothing is drained.
    Waiting { adds: Vec<String> },
    /// **Updating…**: the drain answers done, the old instance exits and the new one starts.
    Updating,
}

impl LabUpdate {
    /// The state id the module reads once the step happened (§7.8).
    pub fn state(&self) -> &'static str {
        match self {
            LabUpdate::Ready { .. } => "update-busy",
            LabUpdate::Waiting { .. } => "update-waiting",
            LabUpdate::Updating => "updating",
        }
    }
}

/// Lab mode on, and `id` a lab module: or why not.
fn lab_only(reg: &Registry, id: &str) -> Result<(), String> {
    if !reg.lab {
        return Err("lab hub only: lab mode is off for this hub's folder".into());
    }
    if !reg.is_lab(id) {
        return Err(format!("{id} is not a lab module (lab-modules\\{id})"));
    }
    Ok(())
}

impl Supervisor {
    /// What a lab verb set for `id`'s drain answer, taken once (lab modules only).
    pub(super) fn lab_answer(&self, id: &str) -> Option<Option<String>> {
        self.0
            .lab
            .answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
    }

    /// Lab hub only: `Ok(())` when `id` is a lab module of a hub in lab mode.
    pub fn lab_check(&self, id: &str) -> Result<(), String> {
        let reg =
            Registry::open_from_env(&self.0.home).map_err(|e| format!("registry not read: {e}"))?;
        lab_only(&reg, id)
    }

    /// `id`'s state id as the policy holds it (`idle` when it has none, `unknown` when the
    /// policy doesn't know it).
    pub fn lab_state(&self, id: &str) -> String {
        let p = self.0.policy.lock().unwrap_or_else(|e| e.into_inner());
        match p.state(id) {
            None => "unknown".into(),
            Some(s) => s.id().unwrap_or("idle").into(),
        }
    }

    /// Lab hub only (#1096): stages an update of the running lab module `id` the way the update
    /// step does, then waits (at most `wait`) until it reads [`LabUpdate::state`]. Answers that
    /// state, or why not: lab mode off, not a lab module, not running, a permission that isn't
    /// one, or the state not reached in time (what it reads instead).
    pub fn lab_update(&self, id: &str, how: LabUpdate, wait: Duration) -> Result<String, String> {
        let reg =
            Registry::open_from_env(&self.0.home).map_err(|e| format!("registry not read: {e}"))?;
        lab_only(&reg, id)?;
        let state = self
            .0
            .policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state(id);
        if !matches!(state, Some(State::Running | State::Waiting)) {
            return Err(format!(
                "{id} isn't running (it reads {}); start it first",
                self.lab_state(id)
            ));
        }
        let version = reg
            .inventory
            .modules
            .get(id)
            .map(|m| m.version.clone())
            .ok_or_else(|| format!("{id} is not installed"))?;
        let asks = match &how {
            LabUpdate::Waiting { adds } => {
                if adds.is_empty() {
                    return Err("update-waiting needs the permission the update adds".into());
                }
                for a in adds {
                    hideout_contract::capability::Capability::parse(a)
                        .map_err(|e| format!("{a:?} isn't a permission: {e}"))?;
                }
                asks_words(adds)
            }
            LabUpdate::Ready { reason } => {
                self.set_answer(id, Some(reason.clone()));
                None
            }
            LabUpdate::Updating => {
                self.set_answer(id, None);
                None
            }
        };
        log::info!(
            "supervisor: lab: {id}: update of {version} staged, for {}",
            how.state()
        );
        self.feed(Ev::UpdateStaged {
            id: id.into(),
            version: version.clone(),
            previous: version,
            asks,
        });
        let reached = self.lab_wait_for(id, how.state(), wait);
        if reached.is_err() {
            // No drain answer came: the answer set for it mustn't touch a later update.
            self.lab_answer(id);
        }
        reached
    }

    fn set_answer(&self, id: &str, busy: Option<String>) {
        self.0
            .lab
            .answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.into(), busy);
    }

    /// Waits until `id` reads `want` (a §7.8 state id), at most `wait`: woken by every policy
    /// step (start event: the call; stop event: the step that sets it; the timeout only as the
    /// fallback). Answers the state, or what it reads instead.
    pub fn lab_wait_for(&self, id: &str, want: &str, wait: Duration) -> Result<String, String> {
        let deadline = Instant::now() + wait;
        let mut steps = self.0.lab.steps.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let now = self.lab_state(id);
            if now == want {
                return Ok(now);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!(
                    "{id} reads {now}, not {want}, after {} ms",
                    wait.as_millis()
                ));
            }
            steps = self
                .0
                .lab
                .cv
                .wait_timeout(steps, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }
}
