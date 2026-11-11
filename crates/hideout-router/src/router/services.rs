// SPDX-License-Identifier: MIT
//! Host services (contract §8.3, §8.10, §8.11, v1.12): methods the hub answers itself, the
//! network broker `net.*` and the microphone `audio.inputs.set_mute`. The router checks each
//! call (its params and the caller's grant) and hands it to the hub's services as
//! [`Out::Service`]; the answer comes back through [`Router::service_answer`]. When a
//! connection ends, [`Out::ServiceGone`] lets the services drop what it held (its streams).
//!
//! The router does no I/O here: the broker and the audio service live in `hideout-services`.

use std::time::Instant;

use hideout_contract::capability::Capability;
use hideout_contract::permission::Permission;
use hideout_contract::services::{
    error_id, NetKind, NetOpen, NetSend, NetStream, SetMute, LOOPBACK, NET_METHODS, SET_MUTE,
};
use serde_json::{json, Value};

use super::{code, ok, ConnId, HostCall, Out, Router, Who};

/// A host-service call the router accepted, for the hub's services: answer it with
/// [`Router::service_answer`] and the same `call`.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceCall {
    pub call: u64,
    /// The connection that called: a stream belongs to it, and events go to it alone.
    pub conn: ConnId,
    /// Who called, for the log: a module id or a tool's name.
    pub caller: String,
    pub method: String,
    pub params: Value,
}

/// A host service's refusal: the JSON-RPC code (§8.1) and the §15 id that travels in
/// `data.id`, with plain words in `data.reason`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    pub code: i64,
    pub id: &'static str,
    pub reason: String,
}

impl ServiceError {
    pub fn new(code: i64, id: &'static str, reason: impl Into<String>) -> ServiceError {
        ServiceError {
            code,
            id,
            reason: reason.into(),
        }
    }

    /// −32602 with this id.
    pub fn argument(id: &'static str, reason: impl Into<String>) -> ServiceError {
        ServiceError::new(code::INVALID_PARAMS, id, reason)
    }

    /// The error object (`{code, message, data: {id, reason}}`).
    pub fn to_value(&self) -> Value {
        let message = match self.code {
            code::INVALID_PARAMS => "invalid params",
            code::DENIED => "denied",
            6 => "backend",
            8 => "rate-limited",
            _ => "error",
        };
        json!({"code": self.code, "message": message,
               "data": {"id": self.id, "reason": self.reason}})
    }
}

/// Whether `grant` covers a `net.open` of `p` (§8.10): the scoped forms, or `net`.
fn net_allowed(grant: &hideout_contract::permission::Grant, p: &NetOpen) -> bool {
    if grant.has(&Permission::Net) {
        return true;
    }
    match p.kind {
        NetKind::Tcp => grant.can_connect(&p.host, p.port),
        NetKind::Udp => grant.can_listen_udp(p.port),
        NetKind::TcpListen => grant.can_listen_tcp(p.port),
    }
}

/// The permission a `net.open` of `p` needs, as the dialog writes it.
fn net_needs(p: &NetOpen) -> String {
    match p.kind {
        NetKind::Tcp => format!("net.connect:{}:{}", p.host, p.port),
        NetKind::Udp => format!("net.listen:udp:{LOOPBACK}:{}", p.port),
        NetKind::TcpListen => format!("net.listen:tcp:{LOOPBACK}:{}", p.port),
    }
}

/// Checks a host-service call's params and the caller's grant (§8.10, §8.11).
fn check(
    caps: &super::Caps,
    caller: &str,
    method: &str,
    params: &Value,
) -> Result<(), ServiceError> {
    match method {
        "net.open" => {
            let p: NetOpen = serde_json::from_value(params.clone()).map_err(|e| {
                ServiceError::argument(error_id::NET_ARGUMENT, format!("net.open: {e}"))
            })?;
            if p.port == 0 {
                return Err(ServiceError::argument(
                    error_id::NET_ARGUMENT,
                    "net.open: port 0 is not a port",
                ));
            }
            if p.kind != NetKind::Tcp && p.host != LOOPBACK {
                return Err(ServiceError::argument(
                    error_id::NET_ARGUMENT,
                    format!("net.open: a udp or tcp-listen stream is on {LOOPBACK} only"),
                ));
            }
            if !net_allowed(&caps.kept(), &p) {
                let needs = net_needs(&p);
                return Err(ServiceError::new(
                    code::DENIED,
                    error_id::NET_DENIED,
                    format!(
                        "{caller} needs permission {needs} to open {}:{}. Allow it in Hideout's permissions for {caller}.",
                        p.host, p.port
                    ),
                ));
            }
            Ok(())
        }
        "net.send" => serde_json::from_value::<NetSend>(params.clone())
            .map(|_| ())
            .map_err(|e| ServiceError::argument(error_id::NET_ARGUMENT, format!("net.send: {e}"))),
        "net.close" => serde_json::from_value::<NetStream>(params.clone())
            .map(|_| ())
            .map_err(|e| ServiceError::argument(error_id::NET_ARGUMENT, format!("net.close: {e}"))),
        SET_MUTE => {
            if !caps.has(&Capability::Kept(Permission::AudioInputsMute)) {
                return Err(ServiceError::new(
                    code::DENIED,
                    error_id::AUDIO_DENIED,
                    format!(
                        "{caller} needs permission audio.inputs.mute to mute the microphone. Allow it in Hideout's permissions for {caller}."
                    ),
                ));
            }
            serde_json::from_value::<SetMute>(params.clone())
                .map(|_| ())
                .map_err(|e| ServiceError {
                    code: code::INVALID_PARAMS,
                    id: "",
                    reason: format!("{SET_MUTE}: {e}"),
                })
        }
        _ => Ok(()),
    }
}

/// Every host-service method there is (§8.3).
pub fn all_methods() -> Vec<&'static str> {
    let mut v = NET_METHODS.to_vec();
    v.push(SET_MUTE);
    v
}

impl Router {
    /// The hub answers these host-service methods (§8.3, v1.12): the network broker
    /// ([`NET_METHODS`]) and the microphone ([`SET_MUTE`]). A method left out answers −32601,
    /// as every hub-owned method no one serves.
    pub fn serve(&mut self, methods: &[&str]) {
        self.services.extend(methods.iter().map(|m| m.to_string()));
    }

    /// Whether the hub serves host-service `method`.
    pub fn serves(&self, method: &str) -> bool {
        self.services.contains(method)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn service_call(
        &mut self,
        conn: ConnId,
        who: &Who,
        caps: &super::Caps,
        id: Value,
        method: &str,
        params: Value,
        now: Instant,
        out: &mut Vec<Out>,
    ) {
        let caller = match who {
            Who::Module { id, .. } => id.clone(),
            Who::Tool { name, .. } => name.clone(),
            Who::New => String::new(),
        };
        let shown = match who {
            Who::Module { id, .. } => self
                .modules
                .get(id)
                .map_or_else(|| id.clone(), |m| m.name.clone()),
            _ => caller.clone(),
        };
        if let Err(e) = check(caps, &shown, method, &params) {
            out.push(Out::Send(conn, refusal(&id, &e)));
            return;
        }
        let call = self.next_host_call;
        self.next_host_call += 1;
        self.host_calls.insert(
            call,
            HostCall {
                caller: conn,
                id,
                deadline: now + self.settings.call_timeout,
            },
        );
        out.push(Out::Service(ServiceCall {
            call,
            conn,
            caller,
            method: method.into(),
            params,
        }));
    }

    /// A host service's answer to an [`Out::Service`]: the result, or its refusal with the §15
    /// id in `data.id`. It goes to the connection that called; an answer nobody waits for
    /// (late, or the caller is gone) is dropped.
    pub fn service_answer(&mut self, call: u64, result: Result<Value, ServiceError>) -> Vec<Out> {
        let Some(h) = self.host_calls.remove(&call) else {
            log::debug!("router: a host service answer nobody waits for (call {call})");
            return Vec::new();
        };
        let reply = match result {
            Ok(v) => ok(&h.id, v),
            Err(e) => refusal(&h.id, &e),
        };
        vec![Out::Send(h.caller, reply)]
    }

    /// Whether the caller of host-service call `call` still waits for its answer (it hasn't
    /// timed out or gone).
    pub fn waits_for(&self, call: u64) -> bool {
        self.host_calls.contains_key(&call)
    }

    /// Sends a host service's event (`net.data`, `net.closed`, `net.accepted`) to `conn` alone,
    /// if it is still connected.
    pub fn service_event(&self, conn: ConnId, method: &str, params: Value) -> Vec<Out> {
        if !self.conns.contains_key(&conn) {
            return Vec::new();
        }
        vec![Out::Send(
            conn,
            json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )]
    }
}

pub(super) fn refusal(id: &Value, e: &ServiceError) -> Value {
    let mut err = e.to_value();
    if e.id.is_empty() {
        err["data"] = json!({ "reason": e.reason });
    }
    json!({"jsonrpc": "2.0", "id": id, "error": err})
}
