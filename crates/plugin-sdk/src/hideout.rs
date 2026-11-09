//! Write Hideout modules in Rust (docs/hideout-contract.md §7.5, §7.6, §8).
//!
//! A module is a program the hub starts with `HIDEOUT_PIPE`, `HIDEOUT_TOKEN` and
//! `HIDEOUT_INSTANCE` set. It connects to the router pipe and speaks JSON-RPC 2.0, one message per
//! line (§8.1). [`Session`] is the module's side of that wire, over any reader and writer: the
//! pipe ([`connect_env`]), or an in-process buffer in tests.
//!
//! - Calls the module makes are typed: [`Session::initialize`], [`Session::register`],
//!   [`Session::ui_register`], [`Session::ready`], [`Session::ui_patch`], [`Session::idle`], ...
//!   Each request returns its id; [`Session::call`] blocks for the answer.
//! - Calls the hub makes (`module.drain`, `state.export`/`state.import`, `ui.event`,
//!   `channel.create`, ...) go to a [`Handler`], with params parsed into the contract's types.
//!   [`Session::handle`] answers them; [`Session::run`] loops over the wire until it closes.
//!
//! The Foxglove v1 [`crate::Plugin`] API is unchanged; this is the Hideout contract beside it.
//!
//! ```no_run
//! use foxglove_plugin::hideout::{connect_env, Handler, InitializeModule};
//! struct Art;
//! impl Handler for Art {}
//! let (mut session, launch) = connect_env().unwrap();
//! session.call_initialize(&InitializeModule {
//!     id: "example.art".into(), version: "0.1.0".into(), instance: launch.instance.clone(),
//!     token: launch.token.clone(), hub: "1.0".into(), api: Default::default(),
//! }, &mut Art).unwrap();
//! session.run(&mut Art).unwrap();
//! ```

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

pub use foxglove_plugin_api::hideout::messages::{
    ActionInvoke, ChannelCreate, ChannelOpen, ChannelOpened, DrainReason, DrainResult, HubNotify,
    InitializeModule, ModuleDrain, ModuleReady, ModuleRegister, NotifyLevel, PatchOp, PatchOpKind,
    RegisterResult, SettingsSet, UiAssetPut, UiEvent, UiPatch, UiWindowOpen,
};
pub use foxglove_plugin_api::hideout::panel::{Node, Panel, Register};

/// The most a message may be on the wire (§8.1).
pub const MAX_MESSAGE: usize = 256 * 1024;

/// The error codes of §8.1.
pub mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const UNSUPPORTED: i64 = 1;
    pub const DISCONNECTED: i64 = 2;
    pub const BUSY: i64 = 3;
    pub const TIMEOUT: i64 = 4;
    pub const DENIED: i64 = 5;
    pub const BACKEND: i64 = 6;
    pub const VERSION: i64 = 7;
    pub const RATE_LIMITED: i64 = 8;
}

/// A JSON-RPC error, either side's. `data.reason` is plain words for the UI (§8.1).
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// A refusal with a plain-words reason for the UI.
    pub fn refused(code: i64, message: impl Into<String>, reason: impl Into<String>) -> RpcError {
        RpcError {
            data: Some(json!({ "reason": reason.into() })),
            ..RpcError::new(code, message)
        }
    }

    fn from_value(v: &Value) -> RpcError {
        RpcError {
            code: v
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or(code::INTERNAL),
            message: v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            data: v.get("data").cloned(),
        }
    }

    fn to_value(&self) -> Value {
        let mut e = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            e["data"] = data.clone();
        }
        e
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

/// What went wrong on the module's side of the wire.
#[derive(Debug)]
pub enum SessionError {
    Io(io::Error),
    /// The hub answered with an error.
    Rpc(RpcError),
    /// A line that isn't a JSON-RPC 2.0 message, or a result of the wrong shape.
    Protocol(String),
    /// The pipe closed (the hub went: §7.9 grace applies).
    Closed,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "pipe: {e}"),
            SessionError::Rpc(e) => write!(f, "{e}"),
            SessionError::Protocol(e) => write!(f, "protocol: {e}"),
            SessionError::Closed => write!(f, "the hub closed the pipe"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<io::Error> for SessionError {
    fn from(e: io::Error) -> SessionError {
        SessionError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, SessionError>;

/// The hub's calls on a module. Every method has a default: a module overrides what it supports.
/// Params arrive parsed into the contract's types; params that don't parse answer -32602 before
/// the handler is called.
pub trait Handler {
    /// `module.drain` (§7.6): `Done`, or `Busy` with a reason; when busy, send
    /// [`Session::idle`] once the work ends, and the hub drains again.
    fn drain(&mut self, _drain: ModuleDrain) -> std::result::Result<DrainResult, RpcError> {
        Ok(DrainResult::Done)
    }
    /// `state.export` (handover): the blob for the next instance, at most 256 KiB, or `None`.
    fn state_export(&mut self) -> std::result::Result<Option<Value>, RpcError> {
        Ok(None)
    }
    /// `state.import {blob, from_version}` on the new instance.
    fn state_import(&mut self, _params: Value) -> std::result::Result<Value, RpcError> {
        Ok(json!({}))
    }
    /// `ui.event` from one of the module's panels.
    fn ui_event(&mut self, _event: UiEvent) -> std::result::Result<Value, RpcError> {
        Ok(json!({}))
    }
    /// `ui.window.open`.
    fn window_open(&mut self, _open: UiWindowOpen) -> std::result::Result<Value, RpcError> {
        Ok(json!({}))
    }
    /// `action.invoke` (a key, the tray, the palette, a panel).
    fn action(&mut self, _action: ActionInvoke) -> std::result::Result<Value, RpcError> {
        Ok(json!({}))
    }
    /// `settings.get`: the module's settings.
    fn settings_get(&mut self, _params: Value) -> std::result::Result<Value, RpcError> {
        Ok(json!({}))
    }
    /// `settings.set {name, value}`.
    fn settings_set(&mut self, _set: SettingsSet) -> std::result::Result<Value, RpcError> {
        Ok(json!({}))
    }
    /// `channel.create` (§8.4): make a one-time pipe for `consumer` and answer `{pipe}`; see
    /// `crate::channel::ChannelPipe` (current-user ACL, Windows).
    fn channel_create(&mut self, _create: ChannelCreate) -> std::result::Result<Value, RpcError> {
        Err(RpcError::new(code::UNSUPPORTED, "no channels"))
    }
    /// `module.exit`: finish up and exit with code 0.
    fn exit(&mut self) {}
    /// Any other notification (`facts.changed`, ...).
    fn notification(&mut self, _method: &str, _params: Value) {}
    /// Any other request: a provided method (`example.art.status`, ...).
    fn call(&mut self, method: &str, _params: Value) -> std::result::Result<Value, RpcError> {
        Err(RpcError::new(
            code::METHOD_NOT_FOUND,
            format!("no method {method}"),
        ))
    }
}

/// What [`Session::handle`] did with one incoming message.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// The answer to one of the module's requests, with the method it was for.
    Response {
        id: i64,
        method: String,
        outcome: std::result::Result<Value, RpcError>,
    },
    /// A hub request, answered.
    Answered { method: String },
    /// A hub notification, handled.
    Notified { method: String },
}

/// The launch values the hub sets (§7.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub pipe: String,
    pub token: String,
    pub instance: String,
}

impl Launch {
    pub fn from_env() -> Option<Launch> {
        let var = |k| std::env::var(k).ok().filter(|v: &String| !v.is_empty());
        Some(Launch {
            pipe: var("HIDEOUT_PIPE")?,
            token: var("HIDEOUT_TOKEN")?,
            instance: var("HIDEOUT_INSTANCE")?,
        })
    }
}

/// The router pipe's name for a user (§8.2): `\\.\pipe\hideout-<user>`, keeping ASCII letters,
/// digits, `-` and `_` of the name.
pub fn router_pipe_name(user: &str) -> String {
    let user: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    format!(r"\\.\pipe\hideout-{user}")
}

/// Opens a named pipe as a client: the router pipe, or a channel pipe a provider made.
pub fn open_pipe(name: &str) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name)
}

/// Connects to the router pipe the hub named at launch.
pub fn connect_env() -> io::Result<(Session<BufReader<std::fs::File>, std::fs::File>, Launch)> {
    let launch = Launch::from_env().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HIDEOUT_PIPE, HIDEOUT_TOKEN and HIDEOUT_INSTANCE are not set: the hub starts modules",
        )
    })?;
    let pipe = open_pipe(&launch.pipe)?;
    let reader = BufReader::new(pipe.try_clone()?);
    Ok((Session::new(reader, pipe), launch))
}

/// The module's side of one JSON-RPC connection.
pub struct Session<R, W> {
    reader: R,
    writer: W,
    next_id: i64,
    pending: HashMap<i64, String>,
}

impl<R: BufRead, W: Write> Session<R, W> {
    pub fn new(reader: R, writer: W) -> Session<R, W> {
        Session {
            reader,
            writer,
            next_id: 1,
            pending: HashMap::new(),
        }
    }

    /// The writer (in tests: what the module sent).
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Requests sent and not answered yet, by id.
    pub fn pending(&self) -> &HashMap<i64, String> {
        &self.pending
    }

    fn write(&mut self, msg: &Value) -> Result<()> {
        let mut line =
            serde_json::to_vec(msg).map_err(|e| SessionError::Protocol(e.to_string()))?;
        if line.len() > MAX_MESSAGE {
            return Err(SessionError::Protocol(format!(
                "a message is at most {MAX_MESSAGE} bytes"
            )));
        }
        line.push(b'\n');
        self.writer.write_all(&line)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Sends a request and returns its id; the answer comes back through [`Session::handle`].
    pub fn request(&mut self, method: &str, params: impl Serialize) -> Result<i64> {
        let id = self.next_id;
        self.next_id += 1;
        let params = to_value(params)?;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.pending.insert(id, method.to_string());
        Ok(id)
    }

    /// Sends a notification (no answer).
    pub fn notify(&mut self, method: &str, params: impl Serialize) -> Result<()> {
        let params = to_value(params)?;
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// `initialize` (§7.5 step 2): first on the connection.
    pub fn initialize(&mut self, params: &InitializeModule) -> Result<i64> {
        self.request("initialize", params)
    }

    /// `module.register {methods, facts, channels, apis}` (§7.5 step 4).
    pub fn register(&mut self, params: &ModuleRegister) -> Result<i64> {
        self.request("module.register", params)
    }

    /// `ui.register {instance, panels}` (§5.2).
    pub fn ui_register(&mut self, params: &Register) -> Result<i64> {
        self.request("ui.register", params)
    }

    /// `module.ready {instance}` (§7.5 step 5): only now does the router send traffic.
    pub fn ready(&mut self, instance: &str) -> Result<()> {
        self.notify(
            "module.ready",
            ModuleReady {
                instance: instance.to_string(),
            },
        )
    }

    /// `ui.patch {panel, ops}` (§5.3).
    pub fn ui_patch(&mut self, params: &UiPatch) -> Result<()> {
        self.notify("ui.patch", params)
    }

    /// `ui.asset.put {hash, mime, data}`.
    pub fn asset_put(&mut self, params: &UiAssetPut) -> Result<i64> {
        self.request("ui.asset.put", params)
    }

    /// `hub.notify {title, body, level}`.
    pub fn hub_notify(&mut self, params: &HubNotify) -> Result<i64> {
        self.request("hub.notify", params)
    }

    /// `module.idle` (§7.6): the work a `busy` drain waited for has ended.
    pub fn idle(&mut self) -> Result<()> {
        self.notify("module.idle", json!({}))
    }

    /// `facts.set {key, value}`.
    pub fn set_fact(&mut self, params: Value) -> Result<i64> {
        self.request("facts.set", params)
    }

    /// `channel.open {module, channel}` (§8.4): the answer is a [`ChannelOpened`].
    pub fn channel_open(&mut self, params: &ChannelOpen) -> Result<i64> {
        self.request("channel.open", params)
    }

    /// Handles one line from the hub: an answer to a request, or a hub call for `handler`.
    pub fn handle_line(&mut self, line: &str, handler: &mut dyn Handler) -> Result<Incoming> {
        let msg: Value =
            serde_json::from_str(line).map_err(|e| SessionError::Protocol(e.to_string()))?;
        self.handle(msg, handler)
    }

    /// Handles one message from the hub.
    pub fn handle(&mut self, msg: Value, handler: &mut dyn Handler) -> Result<Incoming> {
        if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(SessionError::Protocol("jsonrpc must be \"2.0\"".into()));
        }
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            let id = msg
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| SessionError::Protocol("a response without an id".into()))?;
            let method = self
                .pending
                .remove(&id)
                .ok_or_else(|| SessionError::Protocol(format!("no request {id} is waiting")))?;
            let outcome = match (msg.get("result"), msg.get("error")) {
                (Some(r), None) => Ok(r.clone()),
                (None, Some(e)) => Err(RpcError::from_value(e)),
                _ => return Err(SessionError::Protocol("one of result and error".into())),
            };
            return Ok(Incoming::Response {
                id,
                method,
                outcome,
            });
        };
        let method = method.to_string();
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = msg.get("id").cloned() else {
            match method.as_str() {
                "module.exit" => handler.exit(),
                _ => handler.notification(&method, params),
            }
            return Ok(Incoming::Notified { method });
        };
        let outcome = dispatch(handler, &method, params);
        let reply = match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e.to_value()}),
        };
        self.write(&reply)?;
        Ok(Incoming::Answered { method })
    }

    /// Reads the next line from the hub; `None` when the pipe closed.
    pub fn read_line(&mut self) -> Result<Option<String>> {
        let mut line = String::new();
        let n = (&mut self.reader)
            .take(MAX_MESSAGE as u64 + 2)
            .read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(line))
    }

    /// Sends a request and waits for its answer, handling hub calls that arrive meanwhile.
    pub fn call(
        &mut self,
        method: &str,
        params: impl Serialize,
        handler: &mut dyn Handler,
    ) -> Result<Value> {
        let id = self.request(method, params)?;
        loop {
            let line = self.read_line()?.ok_or(SessionError::Closed)?;
            if line.trim().is_empty() {
                continue;
            }
            if let Incoming::Response {
                id: got, outcome, ..
            } = self.handle_line(&line, handler)?
            {
                if got == id {
                    return outcome.map_err(SessionError::Rpc);
                }
            }
        }
    }

    /// [`Session::call`] with a typed answer.
    pub fn call_typed<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: impl Serialize,
        handler: &mut dyn Handler,
    ) -> Result<T> {
        let value = self.call(method, params, handler)?;
        serde_json::from_value(value).map_err(|e| SessionError::Protocol(e.to_string()))
    }

    /// `initialize`, waiting for the result (`granted`, `settings_dir`, `data_dir`, ...).
    pub fn call_initialize(
        &mut self,
        params: &InitializeModule,
        handler: &mut dyn Handler,
    ) -> Result<Value> {
        self.call("initialize", params, handler)
    }

    /// `module.register`, waiting for what the hub refused.
    pub fn call_register(
        &mut self,
        params: &ModuleRegister,
        handler: &mut dyn Handler,
    ) -> Result<RegisterResult> {
        self.call_typed("module.register", params, handler)
    }

    /// Answers hub calls until the pipe closes or `module.exit` arrives.
    pub fn run(&mut self, handler: &mut dyn Handler) -> Result<()> {
        while let Some(line) = self.read_line()? {
            if line.trim().is_empty() {
                continue;
            }
            if let Incoming::Notified { method } = self.handle_line(&line, handler)? {
                if method == "module.exit" {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

fn to_value(params: impl Serialize) -> Result<Value> {
    serde_json::to_value(params).map_err(|e| SessionError::Protocol(e.to_string()))
}

fn parse<T: DeserializeOwned>(params: Value) -> std::result::Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn dispatch(
    handler: &mut dyn Handler,
    method: &str,
    params: Value,
) -> std::result::Result<Value, RpcError> {
    match method {
        "module.drain" => ser(handler.drain(parse(params)?)?),
        "state.export" => ser(handler.state_export()?),
        "state.import" => handler.state_import(params),
        "ui.event" => handler.ui_event(parse(params)?),
        "ui.window.open" => handler.window_open(parse(params)?),
        "action.invoke" => handler.action(parse(params)?),
        "settings.get" => handler.settings_get(params),
        "settings.set" => handler.settings_set(parse(params)?),
        "channel.create" => handler.channel_create(parse(params)?),
        _ => handler.call(method, params),
    }
}

fn ser(value: impl Serialize) -> std::result::Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|e| RpcError::new(code::INTERNAL, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Busy;
    impl Handler for Busy {
        fn drain(&mut self, d: ModuleDrain) -> std::result::Result<DrainResult, RpcError> {
            assert_eq!(d.reason, DrainReason::Update);
            Ok(DrainResult::Busy {
                reason: Some("a flash running".into()),
            })
        }
    }

    #[test]
    fn drain_busy_then_idle() {
        let mut s = Session::new(io::empty(), Vec::new());
        let got = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":10,"method":"module.drain","params":{"reason":"update"}}"#,
                &mut Busy,
            )
            .unwrap();
        assert_eq!(
            got,
            Incoming::Answered {
                method: "module.drain".into()
            }
        );
        s.idle().unwrap();
        let out = String::from_utf8(s.writer_mut().clone()).unwrap();
        let lines: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines[0]["result"],
            json!({"state": "busy", "reason": "a flash running"})
        );
        assert_eq!(lines[1]["method"], "module.idle");
    }

    #[test]
    fn bad_params_answer_invalid_params() {
        let mut s = Session::new(io::empty(), Vec::new());
        s.handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"module.drain","params":{"reason":"nope"}}"#,
            &mut Busy,
        )
        .unwrap();
        let reply: Value = serde_json::from_slice(s.writer_mut()).unwrap();
        assert_eq!(reply["error"]["code"], code::INVALID_PARAMS);
    }

    #[test]
    fn call_handles_hub_calls_meanwhile() {
        let hub = concat!(
            r#"{"jsonrpc":"2.0","id":7,"method":"module.drain","params":{"reason":"update"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":1,"result":{"rejected":[]}}"#,
            "\n"
        );
        let mut s = Session::new(hub.as_bytes(), Vec::new());
        let r = s
            .call_register(&ModuleRegister::default(), &mut Busy)
            .unwrap();
        assert!(r.rejected.is_empty());
        assert!(s.pending().is_empty());
    }

    #[test]
    fn pipe_name() {
        assert_eq!(router_pipe_name("Ana Ö-x_1"), r"\\.\pipe\hideout-Ana-x_1");
    }
}
