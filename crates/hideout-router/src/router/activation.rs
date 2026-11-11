// SPDX-License-Identifier: MIT
//! Activation events other than `enable` and `call` (contract §4.6, v1.10): `open`,
//! `fact:<pattern>`, `app:<name>.exe` and `device:<class>`. Each comes out as [`Out::Activate`]
//! for a module that isn't connected; the supervisor starts it only when it is enabled and idle
//! (and not held by safe mode), so a second event for a module already starting does nothing.
//!
//! Event-driven only: a fact change ([`Router::publish`], a module's `facts.set`, the app
//! watch's `apps.running.<name>`), a shown panel ([`Router::panel_opened`]), and the moment a
//! module is armed ([`Router::activation_due`]: hub start, Turn on). No timer.

use hideout_contract::module::{activation_fact, fact_true};
use hideout_contract::permission::key_matches;
use serde_json::Value;

use super::{Out, Router};

/// The activation events of one module the router acts on: `open`, and the fact patterns its
/// `fact:`, `app:` and `device:` entries wait for. `enable` is the supervisor's; `call` is
/// [`super::ModuleEntry::activate_on_call`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Triggers {
    pub open: bool,
    pub facts: Vec<String>,
}

impl Triggers {
    /// From a manifest's `activation.on` (already checked by the reader).
    pub fn from_on(on: &[String]) -> Triggers {
        Triggers {
            open: on.iter().any(|e| e == "open"),
            facts: on.iter().filter_map(|e| activation_fact(e)).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.open && self.facts.is_empty()
    }
}

impl Router {
    /// Sets the activation events of module `id` (from its manifest; [`Router::apply_manifest`]
    /// does this). Nothing starts here: [`Router::activation_due`] reports facts already set.
    pub fn set_triggers(&mut self, id: &str, t: Triggers) {
        if t.is_empty() {
            self.triggers.remove(id);
        } else {
            self.triggers.insert(id.into(), t);
        }
    }

    /// The activation events the router holds for `id`.
    pub fn triggers(&self, id: &str) -> Option<&Triggers> {
        self.triggers.get(id)
    }

    /// Whether `id` has no connection: an activation for it is worth sending.
    fn cold(&self, id: &str) -> bool {
        self.module_conn(id).is_none()
    }

    /// One of `module`'s panels was shown (§4.6 `open`): the hub's panel host calls this when a
    /// page of the module comes on screen. Starts the module when it has `open` and isn't
    /// connected.
    pub fn panel_opened(&mut self, module: &str) -> Vec<Out> {
        let armed = self.triggers.get(module).is_some_and(|t| t.open);
        if armed && self.cold(module) {
            log::info!("router: {module}: activated by open");
            vec![Out::Activate(module.into())]
        } else {
            Vec::new()
        }
    }

    /// Modules (`id`, or every module for `None`) whose activation fact is already set, true by
    /// the `when` rule: for the moment a module is armed (hub start, after the supervisor knows
    /// its modules; Turn on). A fact set while it was off counts once it is on.
    pub fn activation_due(&self, id: Option<&str>) -> Vec<Out> {
        self.triggers
            .iter()
            .filter(|(m, _)| id.is_none_or(|i| i == m.as_str()))
            .filter(|(m, t)| {
                self.cold(m)
                    && t.facts.iter().any(|p| {
                        self.facts
                            .iter()
                            .any(|(k, v)| key_matches(p, k) && fact_true(v))
                    })
            })
            .map(|(m, _)| {
                log::info!("router: {m}: activated by a fact already set");
                Out::Activate(m.clone())
            })
            .collect()
    }

    /// A fact changed to `value` (§4.6 `fact:`, and `app:` and `device:` through their facts):
    /// every cold module waiting for a matching fact starts, once per change to a true value.
    pub(super) fn fact_activations(&self, key: &str, value: &Value, out: &mut Vec<Out>) {
        if !fact_true(value) {
            return;
        }
        for (m, t) in &self.triggers {
            if t.facts.iter().any(|p| key_matches(p, key)) && self.cold(m) {
                log::info!("router: {m}: activated by {key}");
                out.push(Out::Activate(m.clone()));
            }
        }
    }
}
