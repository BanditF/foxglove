// SPDX-License-Identifier: MIT
//! The routing logic, with no I/O (§8.1 to §8.8).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use hideout_contract::capability::{check_grantable, Capability, Holder};
use hideout_contract::panel::{self, Register, LIMITS};
use hideout_contract::permission::key_matches;
use hideout_contract::{Version, HUB_VERSION};
use hideout_registry::Registry;
use serde_json::{json, Map, Value};

use crate::checks::HUB_OWNED;
use crate::checks::{
    covers, event_permission, may_call, overlap, watches_app, Caps, FIRST_PARTY_AREAS,
};

/// A call with no answer fails after this long on the caller's side (§8.1, error 4).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The most one message may be (§8.1).
pub const MAX_MESSAGE: usize = 256 * 1024;

/// The consumer name of the hub's own panel host in `channel.create` (§5.4).
pub const HOST_CONSUMER: &str = "hub.panel-host";

mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const DISCONNECTED: i64 = 2;
    pub const TIMEOUT: i64 = 4;
    pub const DENIED: i64 = 5;
    pub const VERSION: i64 = 7;
}

pub type ConnId = u64;

/// How a connection reached the router.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The router pipe or its alias: a module with its launch token, or an outside tool.
    Pipe,
    /// A legacy stdio plugin the hub started and bridges (§8.2): its identity is known.
    Bridged(String),
}

/// What the router asks its driver to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Write one message to a connection.
    Send(ConnId, Value),
    /// Close a connection (after any `Send` before it).
    Close(ConnId),
    /// Start this module: a call arrived for it and its activation has `call` (§4.6).
    Activate(String),
    /// A module's connection ended (the supervisor decides what that means).
    ModuleGone(String),
    /// `module.idle` from a module (§7.6).
    Idle(String),
    /// `module.ready` from a module (§7.5): the supervisor marks it running.
    Ready(String),
    /// A module's `initialize` had a good token but failed the launch check (pid or exe hash,
    /// §7.5, §8.2 v1.3): refused with error 5; the supervisor shows `install-failed`.
    Refused(String),
    /// A module's channel broke the wire rules (a frame over 256 KiB, §8.1): one crash toward
    /// the crash bar (§7.7). Also counted in [`Router::module_faults`].
    ModuleFault(String),
    /// `hub.notify` from a module, already permission-checked.
    Notify { module: String, params: Value },
    /// Accepted panels from `ui.register`, for the panel host (§5.1), with the instance that
    /// registered them.
    Panels {
        module: String,
        instance: String,
        panels: Value,
    },
    /// A fact the hub watches changed (`null`: removed), for the panel host: module states,
    /// `when` facts and bindings (§4.6, §5.2). See [`Router::watch_facts`].
    Fact { key: String, value: Value },
    /// `ui.patch` from a module, for the panel host (§5.3).
    Patch { module: String, params: Value },
    /// `ui.badge` from a module (v1.3, §5.9), for the panel host: its tab's badge.
    Badge { module: String, params: Value },
    /// The answer to [`Router::open_host_channel`]: `Ok({pipe, nonce})` or the error.
    HostChannel {
        module: String,
        channel: String,
        result: Result<Value, Value>,
    },
    /// `ui.asset.put` from a module (§5.3), for the panel host, which answers it with
    /// [`Router::host_answer`] and the same `call`; the answer goes back to the module.
    AssetPut {
        module: String,
        call: u64,
        params: Value,
    },
    /// The answer to [`Router::call_module`].
    HubResult {
        tag: u64,
        result: Result<Value, Value>,
    },
}

/// The supervisor's launch check at `initialize` (§7.5, §8.2 v1.3): given the module id and the
/// connecting process's id (when the transport knows it), whether this is the process the
/// supervisor started or took back. `Err` carries the reason for the log.
pub type LaunchCheck = Box<dyn FnMut(&str, Option<u32>) -> Result<(), String> + Send>;

/// Approves an outside tool's asks once per tool (§6.4): given the tool's name and what it asks
/// for (never-list items already left out), returns what the user approved.
pub type Approver = Box<dyn FnMut(&str, &[Capability]) -> Vec<Capability> + Send>;

/// One module the router knows: from the registry (grants, catalog entry) and its manifest.
#[derive(Clone, Debug)]
pub struct ModuleEntry {
    pub id: String,
    /// The module's display name, for "Waiting for <name>".
    pub name: String,
    pub holder: Holder,
    /// The inventory's `granted` (§6.4).
    pub granted: Vec<String>,
    /// The method patterns its catalog entry lets it provide (§6.3); `None` = no catalog entry
    /// (a sideload): the grant alone decides.
    pub catalog_provide: Option<Vec<String>>,
    /// Channels its catalog entry offers, with the risk class (§6.2).
    pub channel_risk: BTreeMap<String, String>,
    /// `activation.on` contains `call` (§4.6).
    pub activate_on_call: bool,
    /// Panel ids declared in `[[ui.panels]]`; `None` = not checked.
    pub panels: Option<Vec<String>>,
    pub settings_dir: String,
    pub data_dir: String,
}

impl ModuleEntry {
    /// The entry for an installed module, from the registry: its grants, holder and catalog
    /// entry. Name and directories default to the id and the module's folders.
    pub fn from_registry(reg: &Registry, id: &str) -> Option<ModuleEntry> {
        let m = reg.inventory.modules.get(id)?;
        let entry = reg.trust.runnable(id, &m.version).ok().map(|(_, e)| e);
        let dir = reg.module_dir(id, &m.version);
        Some(ModuleEntry {
            id: id.into(),
            name: id.into(),
            holder: m.holder(),
            granted: m.granted.clone(),
            catalog_provide: entry.map(|e| e.provide.clone()),
            channel_risk: entry
                .map(|e| {
                    e.channels
                        .iter()
                        .map(|(k, v)| (k.clone(), v.risk.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            activate_on_call: false,
            panels: None,
            settings_dir: dir.join("settings").display().to_string(),
            data_dir: dir.join(hideout_registry::DATA_DIR).display().to_string(),
        })
    }
}

/// Router-wide settings.
pub struct Settings {
    /// `initialize`'s `app` for tools.
    pub app: String,
    pub safe_mode: bool,
    pub call_timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            app: "hideout".into(),
            safe_mode: false,
            call_timeout: CALL_TIMEOUT,
        }
    }
}

/// A random secret (launch tokens, channel nonces): 128 bits from two OS-seeded SipHash keys
/// over a counter and the clock.
pub fn fresh_secret() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let a = RandomState::new().hash_one((n, t));
    let b = RandomState::new().hash_one((t, n, a));
    format!("{a:016x}{b:016x}")
}

#[derive(Clone, Debug, PartialEq)]
enum Who {
    /// Before `initialize`.
    New,
    Module {
        id: String,
        ready: bool,
        v1: bool,
    },
    Tool {
        name: String,
        holder: Holder,
        v1: bool,
    },
}

struct Conn {
    origin: Origin,
    who: Who,
    caps: Caps,
    /// The peer's process id, when the transport knows it (the router pipe).
    pid: Option<u32>,
}

/// Who waits for an answer.
#[derive(Clone, Debug)]
enum Waiter {
    Caller(ConnId, Value),
    /// A `channel.open` caller: answered `{pipe, nonce}`.
    Channel {
        caller: ConnId,
        id: Value,
        nonce: String,
    },
    Host {
        module: String,
        channel: String,
        nonce: String,
    },
    Hub(u64),
}

struct Pending {
    waiter: Waiter,
    deadline: Instant,
}

/// A call waiting for its provider to start (activation by `call`).
struct Queued {
    module: String,
    caller: ConnId,
    id: Value,
    method: String,
    params: Value,
    deadline: Instant,
}

struct Route {
    pattern: String,
    module: String,
}

/// A module's call the hub's panel host answers (`ui.asset.put`, §5.3).
struct HostCall {
    caller: ConnId,
    id: Value,
    deadline: Instant,
}

/// A lease a provider handed out (§8.5): who holds it.
struct Lease {
    holder: ConnId,
}

pub struct Router {
    settings: Settings,
    approve: Approver,
    approved: HashMap<String, Vec<Capability>>,
    modules: BTreeMap<String, ModuleEntry>,
    tokens: HashMap<String, String>,
    conns: HashMap<ConnId, Conn>,
    next_conn: ConnId,
    /// Accepted method patterns, live once their module is ready.
    routes: Vec<Route>,
    /// Facts patterns each module registered (`facts:` provides).
    fact_owners: Vec<Route>,
    channels: BTreeMap<String, Vec<String>>,
    apis: BTreeMap<String, String>,
    facts: BTreeMap<String, Value>,
    subs: BTreeMap<u64, (ConnId, String)>,
    next_sub: u64,
    pending: HashMap<(ConnId, i64), Pending>,
    next_call: i64,
    queue: VecDeque<Queued>,
    faults: HashMap<String, u32>,
    /// Leases by (provider module, lease id) (§8.5).
    leases: HashMap<(String, String), Lease>,
    /// The supervisor's pid and exe-hash check (§7.5); `None` = tokens only (tests, lab).
    launch_check: Option<LaunchCheck>,
    /// Each module's `[[actions]]` ids, when known: a button's `action` must name one
    /// (`ui.action`, spec v1.4).
    actions: HashMap<String, Vec<String>>,
    /// Fact key prefixes the hub's panel host watches (`""` = every fact).
    hub_watch: Vec<String>,
    /// Each module's `[[settings]]` from its manifest, for the settings a v1 plugin keeps in
    /// the hub (§4.10: it asks with `settings.get` and hears `settings.changed`).
    settings_decl: HashMap<String, Vec<hideout_contract::module::ModuleSetting>>,
    /// Each module's `[[secrets]]` names: the only ones it may read or set.
    secrets_decl: HashMap<String, Vec<String>>,
    /// Each module's `provides = ["channel:<name>"]` names from its manifest (§4.5, §8.4): the
    /// declaration is the module's `channel.offer:<name>` for its own `<id>/<name>` (#1013).
    channels_decl: HashMap<String, Vec<String>>,
    secret_store: Box<dyn crate::secrets::SecretStore>,
    /// Module calls waiting for the panel host's answer, by call number.
    host_calls: HashMap<u64, HostCall>,
    next_host_call: u64,
}

/// The file in a v1 plugin's settings folder where the hub keeps the values the user changed.
pub const SETTINGS_FILE: &str = "settings.json";

fn error(id: &Value, code: i64, message: &str, reason: Option<&str>) -> Value {
    let mut e = json!({"code": code, "message": message});
    if let Some(r) = reason {
        e["data"] = json!({ "reason": r });
    }
    json!({"jsonrpc": "2.0", "id": id, "error": e})
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn denied(id: &Value, reason: Option<&str>) -> Value {
    error(id, code::DENIED, "denied", reason)
}

impl Router {
    pub fn new(settings: Settings, approve: Approver) -> Router {
        let mut r = Router {
            settings,
            approve,
            approved: HashMap::new(),
            modules: BTreeMap::new(),
            tokens: HashMap::new(),
            conns: HashMap::new(),
            next_conn: 1,
            routes: Vec::new(),
            fact_owners: Vec::new(),
            channels: BTreeMap::new(),
            apis: BTreeMap::new(),
            facts: BTreeMap::new(),
            subs: BTreeMap::new(),
            next_sub: 1,
            faults: HashMap::new(),
            leases: HashMap::new(),
            launch_check: None,
            pending: HashMap::new(),
            next_call: 1,
            queue: VecDeque::new(),
            actions: HashMap::new(),
            hub_watch: Vec::new(),
            settings_decl: HashMap::new(),
            secrets_decl: HashMap::new(),
            channels_decl: HashMap::new(),
            secret_store: crate::secrets::default_store(),
            host_calls: HashMap::new(),
            next_host_call: 1,
        };
        let safe = r.settings.safe_mode;
        r.facts.insert("hub.safe_mode".into(), json!(safe));
        r
    }

    /// Adds or replaces a module the router knows.
    pub fn add_module(&mut self, m: ModuleEntry) {
        self.modules.insert(m.id.clone(), m);
    }

    /// The module's `[[actions]]` ids (from its manifest), so `ui.register` refuses a button
    /// whose `action` names none of them (`ui.action`, spec v1.4).
    pub fn set_actions(&mut self, module: &str, actions: Vec<String>) {
        self.actions.insert(module.into(), actions);
    }

    /// Every installed module from the registry, with what its manifest declares (its name,
    /// `[[ui.panels]]` and `[[actions]]`), so `ui.register` is checked against them live.
    pub fn load_registry(&mut self, reg: &Registry) {
        for id in reg.inventory.modules.keys() {
            if let Some(m) = ModuleEntry::from_registry(reg, id) {
                self.add_module(m);
                match reg.manifest(id) {
                    Some(manifest) => self.apply_manifest(&manifest),
                    None => log::warn!("router: {id}: manifest not read; panels unchecked"),
                }
            }
        }
    }

    /// Fills a known module's entry from its manifest (§4.7, §4.10): the name people read, the
    /// declared panel ids (a `ui.register` panel not declared is `ui.undeclared`; a legacy
    /// `plugin.toml` declares none), `activation.on = call`, and the `[[actions]]` ids
    /// (`ui.action`). A module the router doesn't know is left alone.
    pub fn apply_manifest(&mut self, m: &hideout_contract::module::Module) {
        let Some(e) = self.modules.get_mut(&m.id) else {
            return;
        };
        e.name = m.name.clone();
        e.panels = Some(m.panels.iter().map(|p| p.id.clone()).collect());
        e.activate_on_call = m.activation_on.iter().any(|a| a == "call");
        let actions = m.actions.iter().map(|a| a.decl.id.clone()).collect();
        self.set_actions(&m.id, actions);
        self.settings_decl.insert(m.id.clone(), m.settings.clone());
        let secrets = m.secrets.iter().map(|s| s.name.clone()).collect();
        self.secrets_decl.insert(m.id.clone(), secrets);
        let channels = m
            .provides
            .iter()
            .filter_map(|p| p.strip_prefix("channel:"))
            .map(String::from)
            .collect();
        self.channels_decl.insert(m.id.clone(), channels);
    }

    /// Where module secrets live (Windows Credential Manager by default; tests use memory).
    pub fn set_secret_store(&mut self, store: Box<dyn crate::secrets::SecretStore>) {
        self.secret_store = store;
    }

    fn secret_declared(&self, module: &str, name: &str) -> bool {
        self.secrets_decl
            .get(module)
            .is_some_and(|d| d.iter().any(|n| n == name))
    }

    /// The module's declared secrets that have a value saved. The value itself never leaves
    /// the router except to its own module.
    pub fn secrets_saved(&self, module: &str) -> Vec<String> {
        self.secrets_decl
            .get(module)
            .into_iter()
            .flatten()
            .filter(|n| {
                self.secret_store
                    .get(&crate::secrets::target(module, n))
                    .is_some()
            })
            .cloned()
            .collect()
    }

    /// The hub's form asks which secrets are saved: [`Out::HubResult`] `{saved: [names]}`.
    pub fn secrets_status(&mut self, module: &str, tag: u64) -> Vec<Out> {
        vec![Out::HubResult {
            tag,
            result: Ok(json!({ "saved": self.secrets_saved(module) })),
        }]
    }

    /// The user typed a secret in the hub's form (`None` clears it). Only a declared name, at
    /// most 2048 bytes. The answer is [`Out::HubResult`]; the module reads it with
    /// `secrets.get` when it next needs it.
    pub fn set_secret(
        &mut self,
        module: &str,
        name: &str,
        value: Option<String>,
        tag: u64,
    ) -> Vec<Out> {
        let result = self.store_secret(module, name, value.as_deref());
        vec![Out::HubResult {
            tag,
            result: result.map(|_| json!({})).map_err(|r| {
                json!({"code": code::INVALID_PARAMS, "message": "invalid params",
                    "data": {"reason": r}})
            }),
        }]
    }

    fn store_secret(
        &mut self,
        module: &str,
        name: &str,
        value: Option<&str>,
    ) -> Result<(), String> {
        if !self.secret_declared(module, name) {
            return Err("This module has no such secret.".into());
        }
        if value.is_some_and(|v| v.len() > crate::secrets::MAX_SECRET) {
            return Err("That is too long (at most 2048 bytes).".into());
        }
        self.secret_store
            .set(&crate::secrets::target(module, name), value)
            .map_err(|e| {
                log::warn!("router: {module}: secret {name} not saved: {e}");
                "Hideout couldn't save it in Windows' credential store.".to_string()
            })
    }

    /// True when `module` is connected over the v1 wire (a Foxglove plugin, bridged): the hub
    /// keeps its settings and answers its `settings.get` (§4.10, §8.2).
    fn hub_keeps_settings(&self, module: &str) -> bool {
        self.module_conn(module)
            .is_some_and(|c| matches!(&self.conns[&c].who, Who::Module { v1: true, .. }))
    }

    /// A v1 plugin's settings: every declared setting with the user's value, else its default.
    pub fn kept_settings(&self, module: &str) -> Map<String, Value> {
        let saved: Map<String, Value> = self
            .modules
            .get(module)
            .filter(|m| !m.settings_dir.is_empty())
            .and_then(|m| {
                std::fs::read_to_string(std::path::Path::new(&m.settings_dir).join(SETTINGS_FILE))
                    .ok()
            })
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let mut values = Map::new();
        for d in self.settings_decl.get(module).into_iter().flatten() {
            let v = saved
                .get(&d.name)
                .and_then(|v| d.accept(v))
                .unwrap_or_else(|| d.default.clone());
            values.insert(d.name.clone(), v);
        }
        values
    }

    /// The hub's settings form asks for a module's values (§4.7): `settings.get` to the module,
    /// or the values the hub keeps for a v1 plugin. The answer is [`Out::HubResult`] `{values}`.
    pub fn get_settings(&mut self, module: &str, tag: u64, now: Instant) -> Vec<Out> {
        if self.hub_keeps_settings(module) {
            let values = self.kept_settings(module);
            return vec![Out::HubResult {
                tag,
                result: Ok(json!({ "values": values })),
            }];
        }
        self.call_module(module, "settings.get", json!({}), tag, now)
    }

    /// The user changed one setting in the hub's form (§4.7, §8.8): `settings.set {name, value}`
    /// to the module, which may refuse with `data.reason`. A v1 plugin's value is checked
    /// against its declaration, saved in its settings folder and sent as `settings.changed`
    /// with every value. The answer is [`Out::HubResult`].
    pub fn set_setting(
        &mut self,
        module: &str,
        name: &str,
        value: Value,
        tag: u64,
        now: Instant,
    ) -> Vec<Out> {
        if !self.hub_keeps_settings(module) {
            let params = json!({"name": name, "value": value});
            return self.call_module(module, "settings.set", params, tag, now);
        }
        let refused = |reason: &str| {
            vec![Out::HubResult {
                tag,
                result: Err(
                    json!({"code": code::INVALID_PARAMS, "message": "invalid params",
                    "data": {"reason": reason}}),
                ),
            }]
        };
        let decl = self
            .settings_decl
            .get(module)
            .and_then(|d| d.iter().find(|d| d.name == name));
        let Some(accepted) = decl.and_then(|d| d.accept(&value)) else {
            return refused("That value doesn't fit this setting.");
        };
        let mut values = self.kept_settings(module);
        values.insert(name.into(), accepted);
        let dir = self.modules.get(module).map(|m| m.settings_dir.clone());
        if let Some(dir) = dir.filter(|d| !d.is_empty()) {
            let path = std::path::Path::new(&dir).join(SETTINGS_FILE);
            let text = serde_json::to_string_pretty(&values).unwrap_or_default();
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text))
            {
                log::warn!("router: {module}: settings not saved: {e}");
                return refused("Hideout couldn't save the setting.");
            }
        }
        let mut out = self.notify_module(module, "settings.changed", json!({ "values": values }));
        out.push(Out::HubResult {
            tag,
            result: Ok(json!({})),
        });
        out
    }

    /// The hub's panel host watches facts starting with any of `prefixes` (`""` = all): each
    /// change comes out as [`Out::Fact`]. Replaces the last list. No permission applies: the
    /// panel host is the hub.
    pub fn watch_facts(&mut self, prefixes: Vec<String>) -> Vec<Out> {
        self.hub_watch = prefixes;
        let mut out = Vec::new();
        for (key, value) in &self.facts {
            if self.hub_watch.iter().any(|p| key.starts_with(p.as_str())) {
                out.push(Out::Fact {
                    key: key.clone(),
                    value: value.clone(),
                });
            }
        }
        out
    }

    /// A notification from the hub to a module (`ui.visibility`, §5.9). Like
    /// [`Router::call_module`] it needs no `module.ready` (a bridged plugin may skip it, §8.2);
    /// with no connection it is dropped (a notification has no answer).
    pub fn notify_module(&mut self, module: &str, method: &str, params: Value) -> Vec<Out> {
        match self.module_conn(module) {
            Some(c) => vec![Out::Send(
                c,
                json!({"jsonrpc": "2.0", "method": method, "params": params}),
            )],
            None => {
                log::debug!("router: {method} for {module} dropped: not connected");
                Vec::new()
            }
        }
    }

    /// A fresh launch token for `id`, to pass as `HIDEOUT_TOKEN` (§7.5). Replaces the last one.
    pub fn issue_token(&mut self, id: &str) -> String {
        let t = fresh_secret();
        self.tokens.insert(id.into(), t.clone());
        t
    }

    /// Sets the launch token for `id` (a hub restart taking back a running module, tests).
    pub fn set_token(&mut self, id: &str, token: &str) {
        self.tokens.insert(id.into(), token.into());
    }

    pub fn connect(&mut self, origin: Origin) -> ConnId {
        let id = self.next_conn;
        self.next_conn += 1;
        self.conns.insert(
            id,
            Conn {
                origin,
                who: Who::New,
                caps: Caps::default(),
                pid: None,
            },
        );
        id
    }

    /// Records the peer process id of `conn` (the router pipe's client, §7.5).
    pub fn set_peer_pid(&mut self, conn: ConnId, pid: u32) {
        if let Some(c) = self.conns.get_mut(&conn) {
            c.pid = Some(pid);
        }
    }

    /// Installs the supervisor's launch check, run at every module `initialize` that carries a
    /// good token (§7.5, §8.2 v1.3).
    pub fn set_launch_check(&mut self, check: LaunchCheck) {
        self.launch_check = Some(check);
    }

    /// The fact cache's value for `key`.
    pub fn fact(&self, key: &str) -> Option<&Value> {
        self.facts.get(key)
    }

    /// Publishes a hub fact (`module.<id>.state`, `hub.safe_mode`, ...): `null` removes it.
    pub fn publish(&mut self, key: &str, value: Value) -> Vec<Out> {
        let mut out = Vec::new();
        self.set_fact(key, value, &mut out);
        out
    }

    /// Safe mode on or off (§7.10): the `initialize` result's `safe_mode` and the fact
    /// `hub.safe_mode`. **Start normally** turns it off without a hub restart.
    pub fn set_safe_mode(&mut self, on: bool) -> Vec<Out> {
        self.settings.safe_mode = on;
        self.publish("hub.safe_mode", json!(on))
    }

    /// How many wire faults (§8.1 frame-too-large) module `id` has caused; the supervisor counts
    /// each as one crash (§7.7).
    pub fn module_faults(&self, id: &str) -> u32 {
        self.faults.get(id).copied().unwrap_or(0)
    }

    /// Whether `id` is a request of `conn` still waiting for its answer (§8.1).
    fn in_flight(&self, conn: ConnId, id: &Value) -> bool {
        self.pending.values().any(|p| {
            matches!(&p.waiter, Waiter::Caller(c, i) | Waiter::Channel { caller: c, id: i, .. }
                if *c == conn && i == id)
        }) || self.queue.iter().any(|q| q.caller == conn && q.id == *id)
            || self
                .host_calls
                .values()
                .any(|h| h.caller == conn && h.id == *id)
    }

    /// The earliest deadline of a pending call, for the driver's timer.
    pub fn next_deadline(&self) -> Option<Instant> {
        let p = self.pending.values().map(|p| p.deadline);
        let q = self.queue.iter().map(|q| q.deadline);
        let h = self.host_calls.values().map(|h| h.deadline);
        p.chain(q).chain(h).min()
    }

    /// Fails every call past its deadline with error 4 (§8.1).
    pub fn tick(&mut self, now: Instant) -> Vec<Out> {
        let mut out = Vec::new();
        let late: Vec<(ConnId, i64)> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(k, _)| *k)
            .collect();
        for k in late {
            if let Some(p) = self.pending.remove(&k) {
                self.fail(p.waiter, code::TIMEOUT, "timeout", None, &mut out);
            }
        }
        while let Some(i) = self.queue.iter().position(|q| q.deadline <= now) {
            let q = self.queue.remove(i).unwrap();
            out.push(Out::Send(
                q.caller,
                error(&q.id, code::TIMEOUT, "timeout", None),
            ));
        }
        // The panel host draws only on the hub's frames; a hub that never takes its answer
        // (closed for good) fails the call like any provider.
        let mut late: Vec<u64> = self
            .host_calls
            .iter()
            .filter(|(_, h)| h.deadline <= now)
            .map(|(k, _)| *k)
            .collect();
        late.sort_unstable();
        for k in late {
            let h = self.host_calls.remove(&k).unwrap();
            out.push(Out::Send(
                h.caller,
                error(&h.id, code::TIMEOUT, "timeout", None),
            ));
        }
        out
    }

    /// The panel host's answer to an [`Out::AssetPut`] (§5.3): `{}`, or the refusal as error
    /// −32602 with the §15 id in `data.id` and the message in `data.reason`. It goes to the
    /// module that called. An answer nobody waits for (late, or the module is gone) is dropped.
    pub fn host_answer(
        &mut self,
        call: u64,
        result: Result<Value, hideout_contract::error::Error>,
    ) -> Vec<Out> {
        let Some(h) = self.host_calls.remove(&call) else {
            log::debug!("router: a panel host answer nobody waits for (call {call})");
            return Vec::new();
        };
        let reply = match result {
            Ok(v) => ok(&h.id, v),
            Err(e) => json!({"jsonrpc": "2.0", "id": h.id, "error": {
                "code": code::INVALID_PARAMS,
                "message": "invalid params",
                "data": {"id": e.id(), "reason": e.message},
            }}),
        };
        vec![Out::Send(h.caller, reply)]
    }

    fn fail(
        &mut self,
        waiter: Waiter,
        code: i64,
        message: &str,
        reason: Option<&str>,
        out: &mut Vec<Out>,
    ) {
        let e = |id: &Value| error(id, code, message, reason);
        match waiter {
            Waiter::Caller(c, id) | Waiter::Channel { caller: c, id, .. } => {
                out.push(Out::Send(c, e(&id)))
            }
            Waiter::Host {
                module, channel, ..
            } => out.push(Out::HostChannel {
                module,
                channel,
                result: Err(e(&Value::Null)["error"].clone()),
            }),
            Waiter::Hub(tag) => out.push(Out::HubResult {
                tag,
                result: Err(e(&Value::Null)["error"].clone()),
            }),
        }
    }

    /// A connection ended: everything it held ends with it (§8.5).
    pub fn disconnect(&mut self, conn: ConnId) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(c) = self.conns.remove(&conn) else {
            return out;
        };
        self.subs.retain(|_, (owner, _)| *owner != conn);
        // Leases it held end: tell each provider (§8.5).
        let held: Vec<(String, String)> = self
            .leases
            .iter()
            .filter(|(_, l)| l.holder == conn)
            .map(|(k, _)| k.clone())
            .collect();
        for k in held {
            self.leases.remove(&k);
            self.lease_closed(&k.0, &k.1, "disconnect", &mut out);
        }
        self.queue.retain(|q| q.caller != conn);
        self.host_calls.retain(|_, h| h.caller != conn);
        // Calls this connection was answering fail with error 2; its own calls are dropped.
        let keys: Vec<(ConnId, i64)> = self.pending.keys().copied().collect();
        for k in keys {
            if k.0 == conn {
                let p = self.pending.remove(&k).unwrap();
                self.fail(p.waiter, code::DISCONNECTED, "disconnected", None, &mut out);
            } else if matches!(&self.pending[&k].waiter,
                Waiter::Caller(c, _) | Waiter::Channel { caller: c, .. } if *c == conn)
            {
                self.pending.remove(&k);
            }
        }
        if let Who::Module { id, .. } = c.who {
            // Another connection may already hold the module (a swap's new instance).
            if self.module_conn(&id).is_none() {
                self.routes.retain(|r| r.module != id);
                self.fact_owners.retain(|r| r.module != id);
                self.channels.remove(&id);
                self.leases.retain(|k, _| k.0 != id);
                out.push(Out::ModuleGone(id));
            }
        }
        out
    }

    /// The hub ended `module`'s run because an accepted catalog revoked its `version` (§12.4;
    /// the registry's `revoked()` names them). Every lease the module holds ends now, and each
    /// provider gets `lease.closed {reason: "revoked"}` once (§8.5); the later disconnect sends
    /// nothing more. The hub fact `module.<id>.revoked` becomes `{version, reason}` with the
    /// catalog's reason (§7.8, v1.6) until another version of the module connects. Stopping
    /// the process stays with the supervisor.
    pub fn revoke(&mut self, module: &str, version: &str, reason: &str) -> Vec<Out> {
        let mut out = Vec::new();
        let conns: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(&c.who, Who::Module { id, .. } if id == module))
            .map(|(k, _)| *k)
            .collect();
        let mut held: Vec<(String, String)> = self
            .leases
            .iter()
            .filter(|(_, l)| conns.contains(&l.holder))
            .map(|(k, _)| k.clone())
            .collect();
        held.sort();
        for k in held {
            self.leases.remove(&k);
            self.lease_closed(&k.0, &k.1, "revoked", &mut out);
        }
        self.set_fact(
            &format!("module.{module}.revoked"),
            json!({"version": version, "reason": reason}),
            &mut out,
        );
        out
    }

    /// Tells `provider` a lease it handed out has ended (§8.5, v1.3).
    fn lease_closed(&self, provider: &str, lease: &str, reason: &str, out: &mut Vec<Out>) {
        if let Some(p) = self.ready_conn(provider) {
            out.push(Out::Send(
                p,
                json!({"jsonrpc": "2.0", "method": "lease.closed",
                       "params": {"lease": lease, "reason": reason}}),
            ));
        }
    }

    fn module_conn(&self, id: &str) -> Option<ConnId> {
        self.conns.iter().find_map(|(k, c)| match &c.who {
            Who::Module { id: m, .. } if m == id => Some(*k),
            _ => None,
        })
    }

    fn ready_conn(&self, id: &str) -> Option<ConnId> {
        self.conns.iter().find_map(|(k, c)| match &c.who {
            Who::Module { id: m, ready, .. } if m == id && *ready => Some(*k),
            _ => None,
        })
    }

    /// One line from a connection.
    pub fn receive(&mut self, conn: ConnId, line: &str, now: Instant) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.conns.contains_key(&conn) {
            return out;
        }
        if line.len() > MAX_MESSAGE {
            out.push(Out::Send(
                conn,
                json!({"jsonrpc": "2.0", "id": null, "error": {
                    "code": code::INVALID_REQUEST,
                    "message": "Invalid Request",
                    "data": {
                        "id": "rpc.frame-too-large",
                        "reason": "a message line is at most 256 KiB",
                    },
                }}),
            ));
            out.push(Out::Close(conn));
            if let Who::Module { id, .. } = &self.conns[&conn].who {
                *self.faults.entry(id.clone()).or_default() += 1;
                out.push(Out::ModuleFault(id.clone()));
            }
            return out;
        }
        let msg = match panel::parse_message(line) {
            Ok(Value::Object(m)) => m,
            Ok(_) | Err(_) => {
                out.push(Out::Send(
                    conn,
                    error(&Value::Null, code::PARSE, "parse error", None),
                ));
                return out;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).map(String::from);
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        match (method, id) {
            (Some(m), Some(id)) => self.request(conn, id, &m, params, now, &mut out),
            (Some(m), None) => self.notification(conn, &m, params, &mut out),
            (None, Some(id)) => self.response(conn, id, &msg, &mut out),
            (None, None) => out.push(Out::Send(
                conn,
                error(&Value::Null, code::INVALID_REQUEST, "invalid request", None),
            )),
        }
        out
    }

    fn request(
        &mut self,
        conn: ConnId,
        id: Value,
        method: &str,
        params: Value,
        now: Instant,
        out: &mut Vec<Out>,
    ) {
        let who = self.conns[&conn].who.clone();
        if method != "initialize" && self.in_flight(conn, &id) {
            // Not routed; id null, so the first call's answer isn't mistaken (§8.1, v1.2).
            out.push(Out::Send(
                conn,
                json!({"jsonrpc": "2.0", "id": null, "error": {
                    "code": code::INVALID_REQUEST,
                    "message": "Invalid Request",
                    "data": {
                        "id": "rpc.duplicate-request-id",
                        "reason": format!("request id {id} is already in flight on this connection"),
                    },
                }}),
            ));
            return;
        }
        if method == "initialize" {
            if who != Who::New {
                out.push(Out::Send(
                    conn,
                    error(
                        &id,
                        code::INVALID_REQUEST,
                        "invalid request",
                        Some("already initialized"),
                    ),
                ));
            } else {
                self.initialize(conn, &id, &params, out);
            }
            return;
        }
        if who == Who::New {
            out.push(Out::Send(conn, denied(&id, Some("initialize first"))));
            return;
        }
        let caps = self.conns[&conn].caps.clone();
        let module = match &who {
            Who::Module { id, .. } => Some(id.clone()),
            _ => None,
        };
        let answer = |v| Out::Send(conn, v);
        match method {
            "ping" => out.push(answer(ok(&id, json!({})))),
            // A v1 plugin's own settings, kept by the hub (§4.10).
            "settings.get"
                if module
                    .as_deref()
                    .is_some_and(|m| self.hub_keeps_settings(m)) =>
            {
                let values = self.kept_settings(module.as_deref().unwrap_or_default());
                out.push(answer(ok(&id, json!({ "values": values }))));
            }
            // A module's own `[[secrets]]` (Foxglove API 1.2), kept by the hub.
            "secrets.get" | "secrets.set" if module.is_some() => {
                let m = module.as_deref().unwrap_or_default();
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let reply = if !self.secret_declared(m, name) {
                    denied(&id, Some("not one of this module's secrets"))
                } else if method == "secrets.get" {
                    let value = self.secret_store.get(&crate::secrets::target(m, name));
                    ok(&id, json!({ "value": value }))
                } else {
                    let value = params
                        .get("value")
                        .and_then(Value::as_str)
                        .map(String::from);
                    match self.store_secret(m, name, value.as_deref()) {
                        Ok(()) => ok(&id, json!({})),
                        Err(r) => error(&id, code::INVALID_PARAMS, "invalid params", Some(&r)),
                    }
                };
                out.push(answer(reply));
            }
            "facts.get" | "facts.list" | "facts.subscribe" | "facts.unsubscribe" | "facts.set" => {
                let reply =
                    self.facts_call(conn, &caps, module.as_deref(), &id, method, &params, out);
                out.push(answer(reply));
            }
            "module.register" => {
                let reply = match &module {
                    Some(m) => self.register(m, &caps, &id, &params),
                    None => denied(&id, Some("only modules register")),
                };
                out.push(answer(reply));
            }
            "ui.register" => {
                let reply = match &module {
                    Some(m) => self.ui_register(m, &id, &params, out),
                    None => denied(&id, Some("only modules register panels")),
                };
                out.push(answer(reply));
            }
            // Answered by the hub's panel host (§5.3): the call goes there, its answer comes
            // back through `host_answer`.
            "ui.asset.put" => match &module {
                Some(m) => {
                    if serde_json::from_value::<hideout_contract::messages::UiAssetPut>(
                        params.clone(),
                    )
                    .is_err()
                    {
                        out.push(answer(error(
                            &id,
                            code::INVALID_PARAMS,
                            "invalid params",
                            None,
                        )));
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
                    out.push(Out::AssetPut {
                        module: m.clone(),
                        call,
                        params,
                    });
                }
                None => out.push(answer(denied(&id, Some("only modules put assets")))),
            },
            "hub.notify" => match &module {
                Some(m) if caps.has(&Capability::HubNotify) => {
                    out.push(Out::Notify {
                        module: m.clone(),
                        params,
                    });
                    out.push(answer(ok(&id, json!({}))));
                }
                _ => out.push(answer(denied(&id, None))),
            },
            "channel.open" => self.channel_open(conn, &who, &caps, id, &params, now, out),
            m if HUB_OWNED.iter().any(|p| covers(p, m)) => out.push(answer(error(
                &id,
                code::METHOD_NOT_FOUND,
                "method not found",
                None,
            ))),
            m => {
                if !may_call(&caps, m, &params) {
                    let reason = self.needs_permission(&who, m);
                    out.push(answer(denied(&id, Some(&reason))));
                    return;
                }
                self.route(conn, id, m, params, now, out);
            }
        }
    }

    /// The refusal text for a call the caller has no permission for: names the module the call
    /// goes to and says where to allow it.
    fn needs_permission(&self, who: &Who, method: &str) -> String {
        let target = self
            .routes
            .iter()
            .filter(|r| covers(&r.pattern, method))
            .max_by_key(|r| r.pattern.len())
            .map(|r| r.module.clone())
            .or_else(|| {
                self.modules
                    .values()
                    .find(|m| {
                        m.catalog_provide
                            .as_ref()
                            .is_some_and(|c| c.iter().any(|c| covers(c, method)))
                    })
                    .map(|m| m.id.clone())
            });
        let target = target
            .and_then(|t| self.modules.get(&t).map(|m| m.name.clone()))
            .unwrap_or_else(|| "this module".into());
        let caller = match who {
            Who::Tool { name, .. } => name.clone(),
            Who::Module { id, .. } => self
                .modules
                .get(id)
                .map_or_else(|| id.clone(), |m| m.name.clone()),
            Who::New => String::new(),
        };
        format!(
            "{caller} needs permission to call {method} on {target}. Allow it in Hideout's permissions for {caller}."
        )
    }

    fn initialize(&mut self, conn: ConnId, id: &Value, params: &Value, out: &mut Vec<Out>) {
        let origin = self.conns[&conn].origin.clone();
        let refuse = |out: &mut Vec<Out>, code: i64, message: &str, reason: &str| {
            out.push(Out::Send(conn, error(id, code, message, Some(reason))));
            out.push(Out::Close(conn));
        };
        // The hub version, when given, must be served (§8.7).
        if let Some(h) = params.get("hub").and_then(Value::as_str) {
            let served = Version::parse(h).is_some_and(|v| v.served_by(HUB_VERSION));
            if !served {
                let reason = format!("hub {h} is not served; this hub serves {HUB_VERSION}");
                // A bad token still answers denied first: the version is no secret, the token is.
                if origin == Origin::Pipe && params.get("token").is_some() && !self.token_ok(params)
                {
                    return refuse(out, code::DENIED, "denied", "bad launch token");
                }
                return refuse(out, code::VERSION, "version", &reason);
            }
        }
        let v1 = params.get("api").is_some_and(Value::is_string);
        let api = match self.serve_api(params.get("api")) {
            Ok(api) => api,
            Err(reason) => return refuse(out, code::VERSION, "version", &reason),
        };
        let bridged = match &origin {
            Origin::Bridged(m) => Some(m.clone()),
            Origin::Pipe => None,
        };
        let module_id = match (&bridged, params.get("token")) {
            (Some(m), _) => Some(m.clone()),
            (None, Some(_)) => {
                if !self.token_ok(params) {
                    return refuse(out, code::DENIED, "denied", "bad launch token");
                }
                let mid = params["id"].as_str().unwrap_or_default().to_string();
                let pid = self.conns[&conn].pid;
                if let Some(check) = self.launch_check.as_mut() {
                    if let Err(why) = check(&mid, pid) {
                        log::warn!("router: {mid} failed the launch check: {why}");
                        out.push(Out::Refused(mid));
                        return refuse(out, code::DENIED, "denied", "launch check failed");
                    }
                }
                Some(mid)
            }
            (None, None) => None,
        };
        if let Some(mid) = module_id {
            let Some(entry) = self.modules.get(&mid).cloned() else {
                return refuse(out, code::DENIED, "denied", "unknown module");
            };
            let caps = Caps::parse(&entry.granted);
            let version = params.get("version").cloned().unwrap_or(Value::Null);
            let c = self.conns.get_mut(&conn).unwrap();
            c.caps = caps.clone();
            c.who = Who::Module {
                id: mid.clone(),
                ready: false,
                v1,
            };
            let result = if v1 {
                json!({"api": api, "app": self.settings.app, "granted": caps.v1_strings()})
            } else {
                json!({
                    "hub": HUB_VERSION.to_string(),
                    "api": api,
                    "granted": caps.strings(),
                    "settings_dir": entry.settings_dir,
                    "data_dir": entry.data_dir,
                    "safe_mode": self.settings.safe_mode,
                })
            };
            if !version.is_null() {
                // Another version connecting ends a revocation notice (v1.6).
                let key = format!("module.{mid}.revoked");
                if self
                    .facts
                    .get(&key)
                    .is_some_and(|r| r["version"] != version)
                {
                    self.set_fact(&key, Value::Null, out);
                }
                self.set_fact(&format!("module.{mid}.version"), version, out);
            }
            out.push(Out::Send(conn, ok(id, result)));
            if bridged.is_some() {
                // A bridged plugin MAY skip `module.ready` (§8.2): the Foxglove plugins speak the
                // v1 wire and never send it. The bridge says it for them once `initialize` is
                // answered, so the supervisor's 30 s ready timer (§7.5) doesn't end every
                // bridged plugin as crashed (#1001).
                self.mark_ready(conn, &mid, out);
            }
            return;
        }
        // An outside tool (§8.2): approved once per tool (§6.4).
        let name = params["client"]["name"].as_str().unwrap_or("").to_string();
        if name.is_empty() {
            return refuse(
                out,
                code::INVALID_PARAMS,
                "invalid params",
                "client.name is missing",
            );
        }
        let holder = if name.starts_with("mcp:") {
            Holder::Mcp
        } else {
            Holder::OutsideTool
        };
        let asked: Vec<Capability> = params
            .get("permissions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|s| Capability::parse(s).ok())
            .filter(|c| check_grantable(c, holder).is_ok())
            .collect();
        let known = self.approved.entry(name.clone()).or_default().clone();
        let new: Vec<Capability> = asked
            .iter()
            .filter(|c| !known.contains(c))
            .cloned()
            .collect();
        let mut granted = known;
        if !new.is_empty() {
            let yes = (self.approve)(&name, &new);
            granted.extend(yes.into_iter().filter(|c| new.contains(c)));
            self.approved.insert(name.clone(), granted.clone());
        }
        let caps = Caps(granted.into_iter().filter(|c| asked.contains(c)).collect());
        let result = if v1 {
            json!({"api": api, "app": self.settings.app, "granted": caps.v1_strings()})
        } else {
            json!({"hub": HUB_VERSION.to_string(), "app": self.settings.app, "api": api, "granted": caps.strings()})
        };
        let c = self.conns.get_mut(&conn).unwrap();
        c.caps = caps;
        c.who = Who::Tool { name, holder, v1 };
        out.push(Out::Send(conn, ok(id, result)));
    }

    fn token_ok(&self, params: &Value) -> bool {
        let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
        let token = params
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        !token.is_empty() && self.tokens.get(id).is_some_and(|t| t == token)
    }

    /// The API versions served for what a client asks (§8.7): a family a provider declared must
    /// serve the asked minor; a family nobody declared yet is answered as asked.
    fn serve_api(&self, asked: Option<&Value>) -> Result<Value, String> {
        let check = |family: &str, want: &str| -> Result<String, String> {
            match self.apis.get(family) {
                None => Ok(want.to_string()),
                Some(have) => {
                    let ok = match (Version::parse(want), Version::parse(have)) {
                        (Some(w), Some(h)) => w.served_by(h),
                        _ => false,
                    };
                    if ok {
                        Ok(have.clone())
                    } else {
                        Err(format!("{family} API {want} is not served; {have} is"))
                    }
                }
            }
        };
        match asked {
            Some(Value::String(v)) => check("foxglove", v).map(Value::String),
            Some(Value::Object(m)) => {
                let mut served = Map::new();
                for (family, v) in m {
                    let v = v.as_str().unwrap_or_default();
                    served.insert(family.clone(), Value::String(check(family, v)?));
                }
                Ok(Value::Object(served))
            }
            _ => Ok(json!({})),
        }
    }

    fn register(&mut self, module: &str, caps: &Caps, id: &Value, params: &Value) -> Value {
        let Some(entry) = self.modules.get(module).cloned() else {
            return denied(id, None);
        };
        let list = |k: &str| -> Vec<String> {
            params
                .get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        };
        let mut rejected = Vec::new();
        let mut reject = |item: &str, error: &str| {
            rejected.push(json!({"item": item, "error": error}));
        };
        let first_party = matches!(entry.holder, Holder::FirstParty | Holder::ModulesModule);
        let others = |r: &Route| r.module != module;
        let mut methods = Vec::new();
        for p in list("methods") {
            let catalog_ok = entry
                .catalog_provide
                .as_ref()
                .is_none_or(|c| c.iter().any(|c| covers(c, &p)));
            let reserved = HUB_OWNED.iter().any(|a| overlap(a, &p))
                || (!first_party && FIRST_PARTY_AREAS.iter().any(|a| overlap(a, &p)));
            let taken = self
                .routes
                .iter()
                .any(|r| others(r) && overlap(&r.pattern, &p));
            if !caps.provides(&p) || !catalog_ok || reserved || taken {
                reject(&p, "permission.provider");
            } else {
                methods.push(p);
            }
        }
        let mut facts = Vec::new();
        for p in list("facts") {
            // Device facts come from a first-party module (the link); nobody else writes them.
            let bad = p == "*"
                || ["module.*", "hub.*", "apps.*"]
                    .iter()
                    .any(|a| overlap(a, &p))
                || (!first_party && overlap("device.*", &p))
                || self
                    .fact_owners
                    .iter()
                    .any(|r| others(r) && overlap(&r.pattern, &p));
            if bad {
                reject(&p, "permission.provider");
            } else {
                facts.push(p);
            }
        }
        let mut channels = Vec::new();
        for ch in list("channels") {
            let catalog_ok =
                entry.catalog_provide.is_none() || entry.channel_risk.contains_key(&ch);
            // The manifest's `provides = ["channel:<name>"]` offers it as the grant
            // `channel.offer:<name>` does (§8.4): a channel is always under the module's own id,
            // and the catalog still decides for a module that has an entry (§6.2).
            let declared = self
                .channels_decl
                .get(module)
                .is_some_and(|d| d.contains(&ch));
            if (caps.offers(&ch) || declared) && catalog_ok {
                channels.push(ch);
            } else {
                reject(&ch, "permission.provider");
            }
        }
        // A new registration replaces the module's old one.
        self.routes.retain(|r| r.module != module);
        self.fact_owners.retain(|r| r.module != module);
        for pattern in methods {
            self.routes.push(Route {
                pattern,
                module: module.into(),
            });
        }
        for pattern in facts {
            self.fact_owners.push(Route {
                pattern,
                module: module.into(),
            });
        }
        self.channels.insert(module.into(), channels);
        if let Some(Value::Object(apis)) = params.get("apis") {
            for (family, v) in apis {
                if let Some(v) = v.as_str() {
                    self.apis.insert(family.clone(), v.into());
                }
            }
        }
        ok(id, json!({ "rejected": rejected }))
    }

    fn ui_register(
        &mut self,
        module: &str,
        id: &Value,
        params: &Value,
        out: &mut Vec<Out>,
    ) -> Value {
        let Ok(reg) = serde_json::from_value::<Register>(params.clone()) else {
            return error(id, code::INVALID_PARAMS, "invalid params", None);
        };
        let declared = self.modules.get(module).and_then(|m| m.panels.clone());
        let actions = self.actions.get(module).map(Vec::as_slice);
        let (accepted, rejected) =
            panel::register_with(&reg, declared.as_deref(), actions, &LIMITS);
        out.push(Out::Panels {
            module: module.into(),
            instance: reg.instance.clone(),
            panels: serde_json::to_value(&accepted).unwrap_or_default(),
        });
        ok(id, json!({ "rejected": rejected }))
    }

    /// `module.ready` (§7.5 step 5), from the module or, for a bridged plugin, from the bridge:
    /// the connection takes traffic, the supervisor marks it running, and the calls that waited
    /// go in order. A second one on a ready connection changes nothing.
    fn mark_ready(&mut self, conn: ConnId, module: &str, out: &mut Vec<Out>) {
        let Some(c) = self.conns.get_mut(&conn) else {
            return;
        };
        match &mut c.who {
            Who::Module { ready: true, .. } => return,
            Who::Module { ready, .. } => *ready = true,
            _ => return,
        }
        self.set_fact(&format!("module.{module}.state"), json!("running"), out);
        out.push(Out::Ready(module.to_string()));
        let mut keep = VecDeque::new();
        while let Some(q) = self.queue.pop_front() {
            if q.module == module {
                self.forward(
                    conn,
                    Waiter::Caller(q.caller, q.id),
                    &q.method,
                    q.params,
                    q.deadline,
                    out,
                );
            } else {
                keep.push_back(q);
            }
        }
        self.queue = keep;
    }

    fn notification(&mut self, conn: ConnId, method: &str, params: Value, out: &mut Vec<Out>) {
        let who = self.conns[&conn].who.clone();
        let Who::Module { id: module, .. } = who else {
            return;
        };
        match method {
            "module.ready" => self.mark_ready(conn, &module, out),
            "module.idle" => out.push(Out::Idle(module)),
            "ui.patch" => out.push(Out::Patch { module, params }),
            "ui.badge" => out.push(Out::Badge { module, params }),
            m => {
                // An event for one lease's holder only (§8.8, v1.3).
                if let Some(lease) = params.get("to").and_then(Value::as_str) {
                    let msg = json!({"jsonrpc": "2.0", "method": m, "params": params});
                    match self.leases.get(&(module.clone(), lease.to_string())) {
                        Some(l) => out.push(Out::Send(l.holder, msg)),
                        None => log::debug!(
                            "router: {module} sent {m} to unknown lease {lease}; dropped"
                        ),
                    }
                    return;
                }
                let Some(need) = event_permission(m) else {
                    log::debug!("router: {module} sent {m}, which has no audience; dropped");
                    return;
                };
                let msg = json!({"jsonrpc": "2.0", "method": m, "params": params});
                for (k, c) in &self.conns {
                    if *k != conn && c.who != Who::New && c.caps.has(&need) {
                        out.push(Out::Send(*k, msg.clone()));
                    }
                }
            }
        }
    }

    fn response(&mut self, conn: ConnId, id: Value, msg: &Map<String, Value>, out: &mut Vec<Out>) {
        let Some(n) = id.as_i64() else { return };
        let Some(p) = self.pending.remove(&(conn, n)) else {
            log::debug!("router: an answer nobody waits for (late or unknown id {n})");
            return;
        };
        let result = match (msg.get("result"), msg.get("error")) {
            (Some(r), _) => Ok(r.clone()),
            (None, Some(e)) => Err(e.clone()),
            _ => Err(json!({"code": code::INVALID_REQUEST, "message": "invalid response"})),
        };
        let envelope = |id: &Value, r: Result<Value, Value>| match r {
            Ok(v) => ok(id, v),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        match p.waiter {
            Waiter::Caller(c, id) => {
                // A result carrying `lease` makes the caller its holder (§8.5, v1.3).
                if let (Ok(r), Some(Who::Module { id: provider, .. })) =
                    (&result, self.conns.get(&conn).map(|x| &x.who))
                {
                    if let (Some(lease), true) = (
                        r.get("lease").and_then(Value::as_str),
                        self.conns.contains_key(&c),
                    ) {
                        self.leases
                            .insert((provider.clone(), lease.to_string()), Lease { holder: c });
                    }
                }
                out.push(Out::Send(c, envelope(&id, result)))
            }
            Waiter::Channel { caller, id, nonce } => {
                let r = crate::channels::grant(result, &nonce, &self.provider_name(conn));
                out.push(Out::Send(caller, envelope(&id, r)));
            }
            Waiter::Host {
                module,
                channel,
                nonce,
            } => out.push(Out::HostChannel {
                module,
                channel,
                result: crate::channels::grant(result, &nonce, &self.provider_name(conn)),
            }),
            Waiter::Hub(tag) => out.push(Out::HubResult { tag, result }),
        }
    }

    /// Sends a call to `provider` on behalf of `waiter`.
    fn forward(
        &mut self,
        provider: ConnId,
        waiter: Waiter,
        method: &str,
        params: Value,
        deadline: Instant,
        out: &mut Vec<Out>,
    ) {
        let n = self.next_call;
        self.next_call += 1;
        self.pending
            .insert((provider, n), Pending { waiter, deadline });
        out.push(Out::Send(
            provider,
            json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params}),
        ));
    }

    /// Routes a provider method (§8.3): to the ready provider, or error 2 / −32601 (§8.1).
    fn route(
        &mut self,
        conn: ConnId,
        id: Value,
        method: &str,
        params: Value,
        now: Instant,
        out: &mut Vec<Out>,
    ) {
        let deadline = now + self.settings.call_timeout;
        let live = self
            .routes
            .iter()
            .filter(|r| covers(&r.pattern, method))
            .max_by_key(|r| r.pattern.len())
            .map(|r| r.module.clone());
        if let Some(m) = &live {
            if let Some(p) = self.ready_conn(m) {
                // The holder gives a lease back: the provider is told it ended (§8.5).
                let released = method
                    .ends_with(".release")
                    .then(|| params.get("lease").and_then(Value::as_str))
                    .flatten()
                    .map(|l| (m.clone(), l.to_string()))
                    .filter(|k| self.leases.get(k).is_some_and(|l| l.holder == conn));
                self.forward(p, Waiter::Caller(conn, id), method, params, deadline, out);
                if let Some(k) = released {
                    self.leases.remove(&k);
                    self.lease_closed(&k.0, &k.1, "released", out);
                }
                return;
            }
        }
        // Not live: a module whose catalog entry provides it, or the last registration.
        let owner = live.or_else(|| {
            self.modules
                .values()
                .find(|m| {
                    m.catalog_provide
                        .as_ref()
                        .is_some_and(|c| c.iter().any(|c| covers(c, method)))
                })
                .map(|m| m.id.clone())
        });
        let Some(owner) = owner else {
            return out.push(Out::Send(
                conn,
                error(&id, code::METHOD_NOT_FOUND, "method not found", None),
            ));
        };
        let entry = &self.modules[&owner];
        if entry.activate_on_call {
            if self.module_conn(&owner).is_none() && !self.queue.iter().any(|q| q.module == owner) {
                out.push(Out::Activate(owner.clone()));
            }
            self.queue.push_back(Queued {
                module: owner,
                caller: conn,
                id,
                method: method.into(),
                params,
                deadline,
            });
            return;
        }
        let reason = format!("Waiting for {}", entry.name);
        out.push(Out::Send(
            conn,
            error(&id, code::DISCONNECTED, "disconnected", Some(&reason)),
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn facts_call(
        &mut self,
        conn: ConnId,
        caps: &Caps,
        module: Option<&str>,
        id: &Value,
        method: &str,
        params: &Value,
        out: &mut Vec<Out>,
    ) -> Value {
        let grant = caps.kept();
        let s = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match method {
            "facts.get" => {
                let key = s("key");
                if !grant.can_read_fact(&key) && !watches_app(&grant, &key) {
                    return denied(id, None);
                }
                ok(
                    id,
                    json!({"value": self.facts.get(&key).cloned().unwrap_or(Value::Null)}),
                )
            }
            "facts.list" => {
                let pattern = s("pattern");
                if !grant.can_subscribe(&pattern) && !watches_app(&grant, &pattern) {
                    return denied(id, None);
                }
                let facts: Vec<Value> = self
                    .facts
                    .iter()
                    .filter(|(k, _)| key_matches(&pattern, k))
                    .map(|(k, v)| json!({"key": k, "value": v}))
                    .collect();
                ok(id, json!({ "facts": facts }))
            }
            "facts.subscribe" => {
                let pattern = s("pattern");
                if !grant.can_subscribe(&pattern) && !watches_app(&grant, &pattern) {
                    return denied(id, None);
                }
                let n = self.next_sub;
                self.next_sub += 1;
                self.subs.insert(n, (conn, pattern));
                ok(id, json!({ "subscription": n }))
            }
            "facts.unsubscribe" => {
                let n = params
                    .get("subscription")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if self.subs.get(&n).is_some_and(|(c, _)| *c == conn) {
                    self.subs.remove(&n);
                }
                ok(id, json!({}))
            }
            _ => {
                let key = s("key");
                let owned = module.is_some_and(|m| {
                    self.fact_owners
                        .iter()
                        .any(|r| r.module == m && key_matches(&r.pattern, &key))
                });
                // The app watch's facts are the hub's too (§4.6, v1.7).
                let hub_fact = ["module.", "hub.", "apps."]
                    .iter()
                    .any(|p| key.starts_with(p));
                // `facts.write` never covers device.* (§6.6); a registered owner may.
                let ok_write = !hub_fact && (owned || grant.can_write_fact(&key));
                if !ok_write {
                    return denied(id, None);
                }
                let value = params.get("value").cloned().unwrap_or(Value::Null);
                self.set_fact(&key, value, out);
                ok(id, json!({}))
            }
        }
    }

    fn set_fact(&mut self, key: &str, value: Value, out: &mut Vec<Out>) {
        let changed = if value.is_null() {
            self.facts.remove(key).is_some()
        } else {
            self.facts.insert(key.into(), value.clone()).as_ref() != Some(&value)
        };
        if !changed {
            return;
        }
        if self.hub_watch.iter().any(|p| key.starts_with(p.as_str())) {
            out.push(Out::Fact {
                key: key.into(),
                value: value.clone(),
            });
        }
        for (n, (c, pattern)) in &self.subs {
            if key_matches(pattern, key) {
                out.push(Out::Send(
                    *c,
                    json!({"jsonrpc": "2.0", "method": "facts.changed",
                           "params": {"subscription": n, "key": key, "value": value}}),
                ));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn channel_open(
        &mut self,
        conn: ConnId,
        who: &Who,
        caps: &Caps,
        id: Value,
        params: &Value,
        now: Instant,
        out: &mut Vec<Out>,
    ) {
        let module = params["module"].as_str().unwrap_or_default().to_string();
        let channel = params["channel"].as_str().unwrap_or_default().to_string();
        let consumer = match who {
            Who::Tool { name, .. } => format!("tool:{name}"),
            Who::Module { id, .. } => format!("module:{id}"),
            Who::New => String::new(),
        };
        let mcp = matches!(
            who,
            Who::Tool {
                holder: Holder::Mcp,
                ..
            }
        );
        let risk = self
            .modules
            .get(&module)
            .and_then(|m| m.channel_risk.get(&channel))
            .map(String::as_str);
        if mcp && risk == Some("keys") {
            let reason = "Key presses are never shared with AI agents";
            return out.push(Out::Send(conn, denied(&id, Some(reason))));
        }
        if !caps.opens(&module, &channel) {
            let name = self
                .modules
                .get(&module)
                .map_or(module.as_str(), |m| m.name.as_str());
            let reason = format!("Needs permission to open {name}'s {channel}");
            return out.push(Out::Send(conn, denied(&id, Some(&reason))));
        }
        let nonce = fresh_secret();
        let waiter = Waiter::Channel {
            caller: conn,
            id,
            nonce: nonce.clone(),
        };
        self.create_channel(&module, &channel, &consumer, nonce, waiter, now, out);
    }

    #[allow(clippy::too_many_arguments)]
    fn create_channel(
        &mut self,
        module: &str,
        channel: &str,
        consumer: &str,
        nonce: String,
        waiter: Waiter,
        now: Instant,
        out: &mut Vec<Out>,
    ) {
        let offered = self
            .channels
            .get(module)
            .is_some_and(|c| c.iter().any(|c| c == channel));
        match self.ready_conn(module) {
            Some(p) if offered => {
                let params = json!({"channel": channel, "consumer": consumer, "nonce": nonce});
                let deadline = now + self.settings.call_timeout;
                self.forward(p, waiter, "channel.create", params, deadline, out);
            }
            Some(_) => {
                let name = self.modules.get(module).map_or(module, |m| m.name.as_str());
                let reason = format!(
                    "{name} has no channel called {channel}. Check the channel name, or update {name}."
                );
                self.fail(
                    waiter,
                    code::METHOD_NOT_FOUND,
                    "method not found",
                    Some(&reason),
                    out,
                )
            }
            None => {
                let name = self.modules.get(module).map_or(module, |m| m.name.as_str());
                let reason = format!("Waiting for {name}");
                self.fail(
                    waiter,
                    code::DISCONNECTED,
                    "disconnected",
                    Some(&reason),
                    out,
                )
            }
        }
    }

    /// A provider connection's display name, for refusals.
    fn provider_name(&self, conn: ConnId) -> String {
        match self.conns.get(&conn).map(|c| &c.who) {
            Some(Who::Module { id, .. }) => {
                self.modules.get(id).map_or(id.clone(), |m| m.name.clone())
            }
            _ => "The module".into(),
        }
    }

    /// The hub's panel host opens a module's stream channel (§5.4): the same grant as any
    /// consumer, the module's own `channel.offer`. The answer comes as [`Out::HostChannel`].
    pub fn open_host_channel(&mut self, module: &str, channel: &str, now: Instant) -> Vec<Out> {
        let mut out = Vec::new();
        let nonce = fresh_secret();
        let waiter = Waiter::Host {
            module: module.into(),
            channel: channel.into(),
            nonce: nonce.clone(),
        };
        self.create_channel(module, channel, HOST_CONSUMER, nonce, waiter, now, &mut out);
        out
    }

    /// A call from the hub to a ready module (`module.drain`, `settings.get`, `ui.event`, ...).
    /// The answer comes as [`Out::HubResult`] with `tag`.
    pub fn call_module(
        &mut self,
        module: &str,
        method: &str,
        params: Value,
        tag: u64,
        now: Instant,
    ) -> Vec<Out> {
        let mut out = Vec::new();
        match self.module_conn(module) {
            Some(p) => {
                let deadline = now + self.settings.call_timeout;
                self.forward(p, Waiter::Hub(tag), method, params, deadline, &mut out);
            }
            None => self.fail(
                Waiter::Hub(tag),
                code::DISCONNECTED,
                "disconnected",
                None,
                &mut out,
            ),
        }
        out
    }
}
