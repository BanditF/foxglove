// SPDX-License-Identifier: MIT
//! The hub's own calls that wait for a module to start (contract §4.6, v1.16, #1234): a tray
//! menu press on a module that isn't running starts it and is delivered once it is ready, or
//! fails after the wait the hub gave it. Unlike a bound key (answered "disconnected", the next
//! press works), a menu click that silently does nothing is worse than a late one.
//!
//! Event-driven: the module's `module.ready` delivers, and the driver's deadline timer (the one
//! [`Router::next_deadline`] feeds) fails what waited too long. Nothing polls.

use std::time::{Duration, Instant};

use serde_json::Value;

use super::{code, Out, Router, Waiter};

/// The message of the error a hub call gets when its module didn't start in time.
pub const NOT_STARTED: &str = "not started";

/// Hub calls that may wait for modules to start at one time; more answer error 3 (busy).
pub const MAX_HUB_QUEUED: usize = 8;

/// One hub call waiting for its module's `module.ready`.
pub(super) struct HubQueued {
    module: String,
    method: String,
    params: Value,
    tag: u64,
    deadline: Instant,
}

impl Router {
    /// A call from the hub to `module` that starts it when it has no connection (§4.6, v1.16):
    /// with a connection it is [`Router::call_module`]; without one the module is activated and
    /// the call waits for its `module.ready`, at most `wait`, then fails with error 4 and the
    /// message [`NOT_STARTED`]. One call with the same method and params waits per module (a
    /// second answers busy, so a double click is one press), and an `action.invoke` must name one
    /// of the module's declared `[[actions]]` when the router knows them. The answer comes as
    /// [`Out::HubResult`] with `tag`.
    pub fn call_module_starting(
        &mut self,
        module: &str,
        method: &str,
        params: Value,
        tag: u64,
        now: Instant,
        wait: Duration,
    ) -> Vec<Out> {
        self.last_now = now;
        let mut out = Vec::new();
        if method == "action.invoke" {
            let action = params.get("action").and_then(Value::as_str);
            let declared = self.actions.get(module);
            if declared.is_some_and(|a| !action.is_some_and(|x| a.iter().any(|d| d == x))) {
                self.fail(
                    Waiter::Hub(tag),
                    code::INVALID_PARAMS,
                    "invalid params",
                    Some("Not one of the module's actions."),
                    &mut out,
                );
                return out;
            }
        }
        if self.module_conn(module).is_some() {
            return self.call_module(module, method, params, tag, now);
        }
        if !self.modules.contains_key(module) {
            self.fail(
                Waiter::Hub(tag),
                code::DISCONNECTED,
                "disconnected",
                None,
                &mut out,
            );
            return out;
        }
        let same = self
            .hub_queue
            .iter()
            .any(|q| q.module == module && q.method == method && q.params == params);
        if same || self.hub_queue.len() >= MAX_HUB_QUEUED {
            self.fail(Waiter::Hub(tag), code::BUSY, "busy", None, &mut out);
            return out;
        }
        if !self.hub_queue.iter().any(|q| q.module == module) {
            log::info!("router: {module}: activated by a hub call ({method})");
            out.push(Out::Activate(module.into()));
        }
        self.hub_queue.push(HubQueued {
            module: module.into(),
            method: method.into(),
            params,
            tag,
            deadline: now + wait,
        });
        out
    }

    /// `module` is ready on `conn`: its waiting hub calls go to it, in order.
    pub(super) fn flush_hub_queue(
        &mut self,
        conn: super::ConnId,
        module: &str,
        out: &mut Vec<Out>,
    ) {
        if !self.hub_queue.iter().any(|q| q.module == module) {
            return;
        }
        let (go, keep): (Vec<HubQueued>, Vec<HubQueued>) = std::mem::take(&mut self.hub_queue)
            .into_iter()
            .partition(|q| q.module == module);
        self.hub_queue = keep;
        let deadline = self.last_now + self.settings.call_timeout;
        for q in go {
            self.forward(conn, Waiter::Hub(q.tag), &q.method, q.params, deadline, out);
        }
    }

    /// Fails the waiting hub calls past their wait (`now`), or all of `module`'s when it left.
    pub(super) fn expire_hub_queue(
        &mut self,
        now: Option<Instant>,
        module: Option<&str>,
        out: &mut Vec<Out>,
    ) {
        let late = |q: &HubQueued| {
            now.is_some_and(|n| q.deadline <= n) || module.is_some_and(|m| q.module == m)
        };
        if !self.hub_queue.iter().any(late) {
            return;
        }
        let (gone, keep): (Vec<HubQueued>, Vec<HubQueued>) = std::mem::take(&mut self.hub_queue)
            .into_iter()
            .partition(late);
        self.hub_queue = keep;
        for q in gone {
            log::info!(
                "router: {}: {} not delivered: not started",
                q.module,
                q.method
            );
            self.fail(Waiter::Hub(q.tag), code::TIMEOUT, NOT_STARTED, None, out);
        }
    }

    /// The earliest wait of a hub call, for [`Router::next_deadline`].
    pub(super) fn hub_queue_deadline(&self) -> Option<Instant> {
        self.hub_queue.iter().map(|q| q.deadline).min()
    }
}
