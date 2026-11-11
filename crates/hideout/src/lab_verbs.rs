// SPDX-License-Identifier: MIT
//! The hub's own lab verbs (#1096), behind `hideout.exe --lab-do`: the supervisor's update states
//! and a panel quarantine for a lab module, so Test lab checks #1093's cards without code
//! changes. Set only in a lab hub (`lab_control::enabled`); the supervisor refuses them again
//! unless lab mode is on for the folder and the module is a lab module
//! (`hideout_supervisor::driver::lab`).
//!
//! - `update-ready <module> [<reason>]`: **Update ready** ("Waits until you finish <reason>").
//! - `update-waiting <module> <permission>...`: **Update waiting for you** for an update that
//!   adds those permissions.
//! - `updating <module>`: **Updating…**, then the module starts again.
//! - `quarantine <module>`: only the check; lab control queues the panel host's quarantine.
//! - `module-state <module> [<state>]`: the supervisor's state, after waiting for `<state>`.

use std::time::Duration;

use hideout_supervisor::driver::lab::LabUpdate;
use hideout_supervisor::driver::Supervisor;
use serde_json::{json, Value};

/// How long a verb waits for its state: the drain answer comes within the router's call timeout
/// (a module without a drain handler may never answer), plus a margin. Only the fallback: the
/// policy step that sets the state ends the wait.
pub const WAIT: Duration = Duration::from_secs(35);
/// The reason `update-ready` gives when none is named.
pub const DEFAULT_REASON: &str = "a lab check";

/// Answers one hub verb for lab control.
pub fn answer(sup: &Supervisor, verb: &str, rest: &str) -> Result<Value, String> {
    answer_within(sup, verb, rest, WAIT)
}

fn answer_within(
    sup: &Supervisor,
    verb: &str,
    rest: &str,
    wait: Duration,
) -> Result<Value, String> {
    let rest = rest.trim();
    let (module, more) = rest.split_once(' ').unwrap_or((rest, ""));
    let more = more.trim();
    if module.is_empty() {
        return Err(format!("{verb} <module> ..."));
    }
    let state = match verb {
        "update-ready" => {
            let reason = if more.is_empty() {
                DEFAULT_REASON
            } else {
                more
            };
            sup.lab_update(
                module,
                LabUpdate::Ready {
                    reason: reason.into(),
                },
                wait,
            )?
        }
        "update-waiting" => {
            let adds: Vec<String> = more.split_whitespace().map(String::from).collect();
            sup.lab_update(module, LabUpdate::Waiting { adds }, wait)?
        }
        "updating" => sup.lab_update(module, LabUpdate::Updating, wait)?,
        "quarantine" => {
            sup.lab_check(module)?;
            sup.lab_state(module)
        }
        "module-state" => {
            sup.lab_check(module)?;
            if more.is_empty() {
                sup.lab_state(module)
            } else {
                sup.lab_wait_for(module, more, wait)?
            }
        }
        other => return Err(format!("unknown hub verb `{other}`")),
    };
    Ok(json!({"module": module, "state": state}))
}

/// Gives lab control these verbs; `main` calls it only for a lab hub.
pub fn register(sup: Supervisor) {
    hideout_shell::lab_control::set_hub_verbs(Box::new(move |verb, rest| answer(&sup, verb, rest)));
}

#[cfg(test)]
mod tests;
