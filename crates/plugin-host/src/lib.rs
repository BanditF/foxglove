//! The Plugins tab: Foxglove's local API host (docs/local-api.md). One [`serve`] call runs one
//! connection over any line-based byte stream, so a plugin's stdio and the named pipe share it.
//!
//! The host does the protocol: the `initialize` gate, version and permission checks, rate
//! limits, facts on the device layer's store, and fact events. Device methods go to a [`Devices`]
//! implementation the app provides; anything it doesn't implement answers "unsupported".

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine};
use foxglove_plugin_api::methods::{self, ClientInfo, Fact, InitializeParams, InitializeResult, Kind};
use foxglove_plugin_api::permission::{Grant, Permission};
use foxglove_plugin_api::rpc::{code, Error, Id, Request, Response, MAX_LINE};
use foxglove_plugin_api::{Version, API_VERSION};
use foxglove_devices::facts::{Facts, Value as DeviceValue};
use serde_json::{json, Value};

/// Device methods, provided by the app (through the device layer). `method` has already passed the
/// permission check; the default answers "unsupported".
pub trait Devices: Send + Sync {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        let _ = params;
        Err(error(code::UNSUPPORTED, format!("{method} is not available yet")))
    }

    /// Like [`Devices::call`], knowing which connection asked (for what ends with it, such as
    /// lighting leases).
    fn call_from(&self, conn: u64, method: &str, params: Value) -> Result<Value, Error> {
        let _ = conn;
        self.call(method, params)
    }

    /// A connection ended (the plugin stopped or crashed, the tool quit).
    fn closed(&self, conn: u64) {
        let _ = conn;
    }
}

/// Each connection's number, for [`Devices::call_from`] and [`Devices::closed`].
static NEXT_CONN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// No devices: facts only.
pub struct NoDevices;
impl Devices for NoDevices {}

/// Decides which requested permissions an outside tool gets.
pub type Approve = Arc<dyn Fn(&ClientInfo, &[Permission]) -> Vec<Permission> + Send + Sync>;

/// Who is connecting and what they may do.
pub enum Client {
    /// A Plugin: permissions from its manifest, approved at install.
    Plugin(Vec<Permission>),
    /// An outside tool: `approve` decides which of the requested permissions it gets (the
    /// app asks the user once per tool and remembers).
    Tool(Approve),
}

pub struct Context {
    pub facts: Facts,
    pub devices: Arc<dyn Devices>,
    pub client: Client,
    /// The app's own version, reported by `initialize`.
    pub app_version: String,
    pub limits: Limits,
}

/// Calls per second a connection may make; past the burst, calls get "rate limited".
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub per_second: f64,
    pub burst: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { per_second: 50.0, burst: 100.0 }
    }
}

fn error(code: i32, message: impl Into<String>) -> Error {
    Error { code, message: message.into(), data: None }
}

fn denied(what: impl std::fmt::Display) -> Error {
    error(code::DENIED, format!("not permitted: needs {what}"))
}

fn to_fact(v: DeviceValue) -> Fact {
    match v {
        DeviceValue::Bool(b) => Fact::Bool(b),
        DeviceValue::Int(i) => Fact::Int(i),
        DeviceValue::Float(f) => Fact::Float(f),
        DeviceValue::Text(t) => Fact::Text(t),
        DeviceValue::Bytes(b) => Fact::Bytes { base64: STANDARD.encode(b) },
    }
}

fn from_fact(f: Fact) -> Result<DeviceValue, Error> {
    Ok(match f {
        Fact::Bool(b) => DeviceValue::Bool(b),
        Fact::Int(i) => DeviceValue::Int(i),
        Fact::Float(f) => DeviceValue::Float(f),
        Fact::Text(t) => DeviceValue::Text(t),
        Fact::Bytes { base64 } => DeviceValue::Bytes(
            STANDARD.decode(base64).map_err(|e| error(code::INVALID_PARAMS, format!("bad base64: {e}")))?,
        ),
    })
}

fn params<T: serde::de::DeserializeOwned>(p: Value) -> Result<T, Error> {
    serde_json::from_value(if p.is_null() { json!({}) } else { p })
        .map_err(|e| error(code::INVALID_PARAMS, e.to_string()))
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

/// One connection's state once initialized.
struct Session {
    grant: Grant,
    /// Active fact subscriptions by id; one forwarder thread serves them all.
    subscriptions: Arc<Mutex<HashMap<u64, String>>>,
    next_subscription: u64,
    forwarding: bool,
    /// Which plugin this connection is (the runner says), for its secrets; `None` for tools.
    plugin: Option<String>,
    /// This connection's number (0 until initialized).
    conn: u64,
    /// Network streams opened for the plugin (#214).
    net: net::Net,
}

/// Serves one connection until the reader ends or a write fails. Messages are lines; events
/// are written from other threads through the same writer, one whole line at a time.
pub fn serve(reader: impl BufRead, writer: impl Write + Send + 'static, ctx: Context) -> std::io::Result<()> {
    serve_with(reader, writer, ctx, None, |_| {})
}

/// Like [`serve`]; `ready` gets the connection's event channel, through which the app sends
/// events of its own to this client ([`notify`]).
pub(crate) fn serve_with(
    reader: impl BufRead,
    writer: impl Write + Send + 'static,
    ctx: Context,
    plugin: Option<String>,
    ready: impl FnOnce(Sender<String>),
) -> std::io::Result<()> {
    let (out, rx) = mpsc::channel::<String>();
    ready(out.clone());
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let closed = Arc::new(AtomicBool::new(false));
    let writer_closed = closed.clone();
    let writer_thread = std::thread::Builder::new().name("plugin-host-writer".into()).spawn(move || {
        let mut w = writer;
        for line in rx {
            if w.write_all(line.as_bytes()).and_then(|()| w.write_all(b"\n")).and_then(|()| w.flush()).is_err() {
                writer_closed.store(true, Ordering::Relaxed);
                break;
            }
        }
    })?;

    let mut session: Option<Session> = None;
    let mut bucket = Bucket { tokens: ctx.limits.burst, at: Instant::now() };
    let mut lines = reader.lines();
    while !closed.load(Ordering::Relaxed) {
        let Some(line) = lines.next() else { break };
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = if line.len() > MAX_LINE {
            Some(Response::error(Id::Number(0), code::INVALID_REQUEST, "line too long"))
        } else {
            let reply = handle(&line, &ctx, &mut session, &mut bucket, &out, &closed);
            if let Some(s) = session.as_mut().filter(|s| s.conn == 0) {
                s.plugin = plugin.clone();
                s.conn = conn;
            }
            reply
        };
        if let Some(reply) = reply {
            if out.send(serde_json::to_string(&reply).unwrap()).is_err() {
                break;
            }
        }
    }
    closed.store(true, Ordering::Relaxed);
    if let Some(s) = &session {
        s.net.close_all();
    }
    ctx.devices.closed(conn);
    let forwarding = session.as_ref().is_some_and(|s| s.forwarding);
    drop(out);
    // The fact forwarder keeps the writer alive until the next fact change, when it notices the
    // closed connection; don't wait for that. Without one, every reply is out once this returns.
    if !forwarding {
        let _ = writer_thread.join();
    }
    Ok(())
}

fn handle(
    line: &str,
    ctx: &Context,
    session: &mut Option<Session>,
    bucket: &mut Bucket,
    out: &Sender<String>,
    closed: &Arc<AtomicBool>,
) -> Option<Response> {
    let request: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return Some(Response::error(Id::Number(0), code::PARSE_ERROR, e.to_string())),
    };
    let id = request.id.clone();
    let result = dispatch(request, ctx, session, bucket, out, closed);
    // Notifications get no reply, not even an error.
    id.map(|id| match result {
        Ok(value) => Response::ok(id, value),
        Err(e) => Response { outcome: foxglove_plugin_api::rpc::Outcome::Error(e), ..Response::ok(id, Value::Null) },
    })
}

fn dispatch(
    request: Request,
    ctx: &Context,
    session: &mut Option<Session>,
    bucket: &mut Bucket,
    out: &Sender<String>,
    closed: &Arc<AtomicBool>,
) -> Result<Value, Error> {
    let now = Instant::now();
    bucket.tokens = (bucket.tokens + now.duration_since(bucket.at).as_secs_f64() * ctx.limits.per_second)
        .min(ctx.limits.burst);
    bucket.at = now;
    if bucket.tokens < 1.0 {
        return Err(error(code::RATE_LIMITED, "too many calls; slow down"));
    }
    bucket.tokens -= 1.0;

    if request.method == "initialize" {
        return initialize(request.params, ctx, session);
    }
    let Some(s) = session.as_mut() else {
        return Err(error(code::INVALID_REQUEST, "call initialize first"));
    };
    let method = methods::find(&request.method)
        .filter(|m| m.kind == Kind::Call)
        .ok_or_else(|| error(code::METHOD_NOT_FOUND, format!("no method {}", request.method)))?;
    if let Some(p) = method.permission {
        let p = Permission::try_from(p.to_string()).expect("the method table's permissions parse");
        if !s.grant.has(&p) {
            return Err(denied(String::from(p)));
        }
    }
    match method.name {
        "ping" => Ok(json!({})),
        "facts.get" => {
            let p: methods::KeyParams = params(request.params)?;
            if !s.grant.can_read_fact(&p.key) {
                return Err(denied(format!("facts.read covering {}", p.key)));
            }
            Ok(json!(methods::FactResult { value: ctx.facts.get(&p.key).map(to_fact) }))
        }
        "facts.list" => {
            let p: methods::SubscribeParams = params(request.params)?;
            if !s.grant.can_subscribe(&p.pattern) {
                return Err(denied(format!("facts.read covering {}", p.pattern)));
            }
            let facts = ctx
                .facts
                .list(&p.pattern)
                .into_iter()
                .map(|(key, value)| methods::FactEntry { key, value: to_fact(value) })
                .collect();
            Ok(json!(methods::FactList { facts }))
        }
        "facts.set" => {
            let p: methods::SetFactParams = params(request.params)?;
            if !s.grant.can_write_fact(&p.key) {
                return Err(denied(format!("facts.write covering {}", p.key)));
            }
            match p.value {
                Some(v) => ctx.facts.set(p.key, from_fact(v)?),
                None => {
                    ctx.facts.remove(&p.key);
                }
            }
            Ok(json!({}))
        }
        "facts.subscribe" => {
            let p: methods::SubscribeParams = params(request.params)?;
            if !s.grant.can_subscribe(&p.pattern) {
                return Err(denied(format!("facts.read covering {}", p.pattern)));
            }
            s.next_subscription += 1;
            let id = s.next_subscription;
            s.subscriptions.lock().unwrap_or_else(|e| e.into_inner()).insert(id, p.pattern);
            if !s.forwarding {
                s.forwarding = true;
                forward(ctx.facts.clone(), s.subscriptions.clone(), out.clone(), closed.clone());
            }
            Ok(json!(methods::Subscription { subscription: id }))
        }
        "secrets.get" | "secrets.set" => {
            let plugin = s.plugin.as_deref().ok_or_else(|| denied("a plugin connection"))?;
            let failed = |e: String| error(code::INVALID_PARAMS, e);
            if method.name == "secrets.get" {
                let p: methods::SecretName = params(request.params)?;
                Ok(json!(methods::SecretValue { value: secrets::get(plugin, &p.name).map_err(failed)? }))
            } else {
                let p: methods::SetSecretParams = params(request.params)?;
                secrets::set(plugin, &p.name, p.value.as_deref()).map_err(failed)?;
                Ok(json!({}))
            }
        }
        "net.open" => {
            let p: methods::NetOpenParams = params(request.params)?;
            if !net::allowed(&s.grant, &p) {
                let need = match p.kind {
                    methods::NetKind::Tcp => format!("net.connect:{}:{}", p.host, p.port),
                    methods::NetKind::Udp => format!("net.listen:udp:127.0.0.1:{}", p.port),
                };
                return Err(denied(need));
            }
            Ok(json!(methods::NetStream { stream: s.net.open(&p, out.clone())? }))
        }
        "net.send" => {
            let p: methods::NetSendParams = params(request.params)?;
            s.net.send(p.stream, &p.data)?;
            Ok(json!({}))
        }
        "net.close" => {
            let p: methods::NetStream = params(request.params)?;
            s.net.close(p.stream);
            Ok(json!({}))
        }
        "facts.unsubscribe" => {
            let p: methods::Subscription = params(request.params)?;
            s.subscriptions.lock().unwrap_or_else(|e| e.into_inner()).remove(&p.subscription);
            Ok(json!({}))
        }
        name => ctx.devices.call_from(s.conn, name, request.params),
    }
}

fn initialize(p: Value, ctx: &Context, session: &mut Option<Session>) -> Result<Value, Error> {
    if session.is_some() {
        return Err(error(code::INVALID_REQUEST, "already initialized"));
    }
    let p: InitializeParams = params(p)?;
    let api = Version::parse(&p.api).ok_or_else(|| error(code::INVALID_PARAMS, "api must be major.minor"))?;
    if !api.served_by(API_VERSION) {
        return Err(error(code::VERSION, format!("this Foxglove speaks API {API_VERSION}; the client needs {api}")));
    }
    let granted = match &ctx.client {
        Client::Plugin(permissions) => permissions.clone(),
        Client::Tool(approve) => approve(&p.client, &p.permissions),
    };
    *session = Some(Session {
        grant: Grant(granted.clone()),
        subscriptions: Arc::default(),
        next_subscription: 0,
        forwarding: false,
        net: net::Net::default(),
        plugin: None,
        conn: 0,
    });
    Ok(json!(InitializeResult { api: API_VERSION.to_string(), app: ctx.app_version.clone(), granted }))
}

/// One thread per connection forwards fact changes to its subscriptions. It stops at the first
/// change after the connection closes.
fn forward(facts: Facts, subscriptions: Arc<Mutex<HashMap<u64, String>>>, out: Sender<String>, closed: Arc<AtomicBool>) {
    let changes = facts.subscribe("*");
    let _ = std::thread::Builder::new().name("plugin-host-facts".into()).spawn(move || {
        while let Ok(change) = changes.recv() {
            if closed.load(Ordering::Relaxed) {
                break;
            }
            let subs = subscriptions.lock().unwrap_or_else(|e| e.into_inner()).clone();
            for (id, pattern) in subs {
                if foxglove_plugin_api::permission::key_matches(&pattern, &change.key) {
                    let event = Request::notification(
                        "facts.changed",
                        json!(methods::FactChanged {
                            subscription: id,
                            key: change.key.clone(),
                            value: change.value.clone().map(to_fact),
                        }),
                    );
                    if out.send(serde_json::to_string(&event).unwrap()).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

/// How long a plugin may stay silent to a ping before the runner restarts it.
pub const PING_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(windows)]
mod net;
pub mod secrets;
pub mod pipe;

/// Each running plugin's event channel, by plugin id.
static PLUGINS: Mutex<Option<HashMap<String, Sender<String>>>> = Mutex::new(None);

pub(crate) fn register(id: &str, out: Option<Sender<String>>) {
    let mut plugins = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let map = plugins.get_or_insert_with(HashMap::new);
    match out {
        Some(out) => {
            map.insert(id.to_string(), out);
        }
        None => {
            map.remove(id);
        }
    }
}

/// Sends an event to a running plugin, such as `action.invoked` when its key is pressed.
/// Only events in the method table go out; false if the plugin isn't running.
pub fn notify(plugin: &str, method: &str, params: Value) -> bool {
    if methods::find(method).is_none_or(|m| m.kind != Kind::Event) {
        return false;
    }
    let plugins = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(out) = plugins.as_ref().and_then(|m| m.get(plugin)) else { return false };
    out.send(serde_json::to_string(&Request::notification(method, params)).unwrap()).is_ok()
}
pub mod runner;
#[cfg(windows)]
mod sandbox;
#[cfg(windows)]
pub use sandbox::delete_container;

#[cfg(test)]
mod tests;
