//! Foxglove as the hub's legacy module (v3 H3.1, contract §7.5 and §8, #1143 P5, #1138).
//!
//! `foxglove.exe --module` is the one switch. The hub starts the app that way with
//! `HIDEOUT_PIPE`, `HIDEOUT_TOKEN`, `HIDEOUT_INSTANCE` and `HIDEOUT_HUB_PID` set; without the
//! flag nothing here runs and the app is the standalone one it always was (the values are never
//! read, so a stray `HIDEOUT_PIPE` changes nothing).
//!
//! In module mode:
//!
//! - **Identity.** The module is `foxglove`, kind `adopted` (contract §3: the MSI made this
//!   install, the hub did not; the manifest is [`MANIFEST`], registered with
//!   `hideout.exe --adopt`). `initialize` carries the build's version, which the hub compares
//!   with the registration.
//! - **Authority.** The launch token is held in memory only ([`Launch`] redacts it from `Debug`)
//!   and the `HIDEOUT_*` values are removed from the process environment at start, so no child
//!   process (a plugin, the updater) inherits them. Nothing is sent until the pipe's server is
//!   the hub's own process (`HIDEOUT_HUB_PID`, §7.5); after a hub restart, whose pid nobody
//!   knows, the server must instead be a `hideout.exe` of this user (`owner::probe`). The
//!   module asks for `facts.write:keyboard.*` and registers the fact pattern `keyboard.*`; what
//!   the hub refuses it does not do (no retry loop, one log line).
//! - **Facts.** The `keyboard.*` facts the app already keeps (`keyboard_facts`, `keyboard.game_mode`
//!   included) are mirrored to the hub through `facts.set`, on change only: a snapshot when the
//!   module is ready, then one call per change. Nothing polls.
//! - **Methods (P6).** The v1 methods in [`SERVED`] come through the router: the module declares
//!   them (`method:` in the manifest, `provide:` in its permissions), registers them, and the
//!   hub's router checks the caller's permission *before* forwarding a call, so the module's
//!   handler only ever runs for a call the router let through. The module acts for the caller
//!   and holds no authority of its own to lend: [`Hub`] answers only the names in [`SERVED`]
//!   (each of which carries a permission in the API's method table, tested), everything else is
//!   -32601, and the `lighting.*`, `art.*`, `screens.*`, `devices.*`, `behaviors.*`, `profiles.*`
//!   and `focus.*` areas stay out because the contract gives an adopted module no first-party
//!   area (§6.3). See [`SERVED`] for what is left out and why.
//! - **No tray, no pipe.** The tray is not created ([`crate::tray::Tray::none`]) and the app
//!   serves no `foxglove-<user>` pipe (the hub owns that alias, §8.6); every device duty stays.
//!   The self-restarts that would start a plain copy (GPU reset, identity registration, the
//!   updater) are off: P7 makes the reset a planned exit.
//! - **Liveness (§7.9).** When the hub's pipe closes the app keeps running, retries with a
//!   bounded backoff for the 30-second grace period, and then hands everything back and exits 0.
//!   `module.exit` does the same at once. A hub that refuses `initialize` ends the module.
//!
//! The wire is the SDK's [`Session`]; the state machine here ([`Link`]) is generic over the
//! writer so the tests drive it against the hub's own `Router` with no pipe and no window.

use std::ffi::OsString;
use std::io::{self, Write};
use std::sync::OnceLock;

use foxglove_devices::facts::{Change, Value as Fact};
use foxglove_plugin::hideout::{
    code, DrainResult, Handler, Incoming, InitializeModule, ModuleDrain, ModuleRegister,
    RegisterResult, RpcError, Session, SessionError,
};
use serde_json::{json, Value};

#[cfg(test)]
mod tests;

/// The switch the hub passes.
pub const FLAG: &str = "--module";
/// The module's id (contract §4.3: a one-label first-party id).
pub const ID: &str = "foxglove";
/// The `hub` version this module is written for (§8.7); it uses nothing newer than 1.0.
const HUB: &str = "1.0";
/// The fact pattern the module publishes and registers.
const FACTS: &str = "keyboard.*";
/// The permission that lets it publish them (§6.1).
const GRANT: &str = "facts.write:keyboard.*";
/// The Foxglove API this module's facts follow.
const API: &str = "1.13";
/// How long a lost hub is waited for (§7.9).
const GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// The v1 methods this module serves through the router (contract §4.5 `method:`, §6.3).
///
/// Exactly the app's v1 methods that lie outside the areas the contract keeps for first-party
/// catalogs (`lighting.*`, `screens.*`, `art.*`, `devices.*`, `behaviors.*`, `profiles.*`,
/// `focus.*`, `gpu.*` ...: `hideout_router::checks::FIRST_PARTY_AREAS`) and the names the hub
/// answers itself (`facts.*`, `initialize`, `ping` ...). Every entry has a permission in the
/// API's method table, so the router refuses a caller without it before the module is asked
/// (tests pin both).
///
/// Left out on purpose:
/// - the first-party areas above (so `devices.list`, which `foxglove-cli status` calls, and the
///   art/lights/screens/behaviors/profiles/focus methods stay with the standalone pipe until a
///   legacy module can be granted those areas: follow-up issue);
/// - `wallpaper-engine.*`: its lease is tied to the calling connection, which a provider behind
///   the router cannot see, so a dead caller would leave Wallpaper Engine paused;
/// - `lighting.lease/update/release`, `key.event`, `behaviors.invoke` and everything of the
///   contract's never-list: not served, so unreachable here whatever a caller holds.
pub const SERVED: &[&str] = &[
    "health.read",
    "lights.page",
    "scenes.page",
    "overlay.window",
    "overlay.restore",
    "overlay.shown",
    "overlay.set_shown",
    "tray.panel",
    "tray.set_panel",
    "pc_lighting.list",
    "pc_lighting.set_zone",
];

/// The connection number the module hands the app's handlers for a routed call. Plugin-host
/// connections count from 1, so 0 is nobody's; no served method keeps anything per connection.
const ROUTED_CONN: u64 = 0;

/// The manifest `hideout.exe --adopt` reads, with `@VERSION@` and `@EXE@` to fill ([`manifest`]).
#[cfg_attr(not(test), allow(dead_code))] // read by tests and by the lab, which copies the file
pub const MANIFEST: &str = include_str!("hub_link/module.toml");

/// The manifest for this build, with `exe` as `run[0]`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn manifest(exe: &str) -> String {
    MANIFEST
        .replace("@VERSION@", env!("CARGO_PKG_VERSION"))
        .replace("@EXE@", &exe.replace('\\', "\\\\"))
}

/// The values the hub launches the module with (§7.5).
#[derive(Clone, PartialEq, Eq)]
pub struct Launch {
    pub pipe: String,
    pub token: String,
    pub instance: String,
    pub hub_pid: u32,
}

impl std::fmt::Debug for Launch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launch")
            .field("pipe", &self.pipe)
            .field("token", &"<redacted>")
            .field("instance", &self.instance)
            .field("hub_pid", &self.hub_pid)
            .finish()
    }
}

/// What every hub's router pipe name starts with (§8.2). A launch naming any other pipe (the
/// `foxglove-<user>` alias, a stray name) is refused before anything is opened.
const ROUTER_PIPE_PREFIX: &str = r"\\.\pipe\hideout-";

/// Reads the launch from the command line and environment. `Ok(None)`: no `--module`, so the
/// standalone app (the environment is not looked at). `Err`: `--module` without what the hub
/// passes, in words for the log; the caller exits and never falls back to the user's pipe.
pub fn parse_launch(
    args: impl IntoIterator<Item = OsString>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<Launch>, String> {
    if !args.into_iter().skip(1).any(|a| a == FLAG) {
        return Ok(None);
    }
    let get = |k: &str| env(k).filter(|v| !v.is_empty());
    let (pipe, token, instance, pid) = (
        get("HIDEOUT_PIPE"),
        get("HIDEOUT_TOKEN"),
        get("HIDEOUT_INSTANCE"),
        get("HIDEOUT_HUB_PID"),
    );
    let missing: Vec<&str> = [
        ("HIDEOUT_PIPE", pipe.is_some()),
        ("HIDEOUT_TOKEN", token.is_some()),
        ("HIDEOUT_INSTANCE", instance.is_some()),
        ("HIDEOUT_HUB_PID", pid.is_some()),
    ]
    .into_iter()
    .filter(|(_, there)| !there)
    .map(|(k, _)| k)
    .collect();
    if !missing.is_empty() {
        return Err(format!(
            "Foxglove: --module is for the Hideout hub, which starts this program with {} set; \
             missing: {}. Nothing was started. Run Foxglove without --module to use it on its own.",
            "HIDEOUT_PIPE, HIDEOUT_TOKEN, HIDEOUT_INSTANCE and HIDEOUT_HUB_PID",
            missing.join(", ")
        ));
    }
    let (pipe, token, instance, pid) = (
        pipe.unwrap(),
        token.unwrap(),
        instance.unwrap(),
        pid.unwrap(),
    );
    if !pipe.to_ascii_lowercase().starts_with(ROUTER_PIPE_PREFIX) {
        return Err(format!(
            "Foxglove: --module: HIDEOUT_PIPE is not a Hideout router pipe name ({ROUTER_PIPE_PREFIX}...); \
             nothing was started."
        ));
    }
    let hub_pid =
        pid.trim().parse::<u32>().ok().filter(|p| *p != 0).ok_or(
            "Foxglove: --module: HIDEOUT_HUB_PID is not a process id; nothing was started.",
        )?;
    Ok(Some(Launch {
        pipe,
        token,
        instance,
        hub_pid,
    }))
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();

/// Whether this process runs as the hub's module. False unless [`enter`] found `--module`.
pub fn module_mode() -> bool {
    LAUNCH.get().is_some()
}

/// First thing in `main`, before any thread: reads the launch, keeps it in memory and removes
/// the `HIDEOUT_*` values from the environment. `Ok(false)` standalone, `Ok(true)` module mode,
/// `Err` for the log and exit code 2.
pub fn enter() -> Result<bool, String> {
    let launch = parse_launch(std::env::args_os(), |k| std::env::var(k).ok());
    // Whatever the outcome, a child process never inherits the hub's values (only with --module
    // is anything read; a standalone start leaves the environment as it found it).
    if !matches!(launch, Ok(None)) {
        for k in [
            "HIDEOUT_PIPE",
            "HIDEOUT_TOKEN",
            "HIDEOUT_INSTANCE",
            "HIDEOUT_HUB_PID",
        ] {
            std::env::remove_var(k);
        }
    }
    match launch? {
        None => Ok(false),
        Some(_) if !cfg!(windows) => {
            Err("Foxglove: --module needs Windows; nothing was started.".into())
        }
        Some(l) => {
            let _ = LAUNCH.set(l);
            Ok(true)
        }
    }
}

// ---- the module's side of the wire, apart from any pipe ----

/// The hub's calls on this module. The legacy module has nothing to hand over, so a drain is
/// done at once; `module.exit` is noted for the runner.
#[derive(Default)]
struct Hub {
    exit: bool,
    /// The app's v1 handlers; `None` answers every routed call with "disconnected".
    devices: Option<std::sync::Arc<dyn foxglove_plugin_host::Devices>>,
    /// The hub has the module (registered and ready): calls are answered only then.
    live: bool,
}

impl Handler for Hub {
    fn drain(&mut self, _drain: ModuleDrain) -> Result<DrainResult, RpcError> {
        Ok(DrainResult::Done)
    }
    fn exit(&mut self) {
        self.exit = true;
    }
    /// A routed v1 call. The router has already checked the caller's permission; the name is
    /// checked again here against [`SERVED`], so a hub that routes more than the manifest names
    /// still reaches nothing else.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        if !SERVED.contains(&method) {
            return Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("no method {method}"),
            ));
        }
        let Some(devices) = self.devices.as_ref().filter(|_| self.live) else {
            return Err(RpcError::new(2, "disconnected"));
        };
        devices
            .call_from(ROUTED_CONN, method, params)
            .map_err(|e| RpcError {
                code: i64::from(e.code),
                message: e.message,
                data: e.data,
            })
    }
}

/// How far the handshake is (§7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Fresh,
    Initializing,
    Registering,
    Live,
}

/// What a hub line led to.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing for the caller to do.
    Nothing,
    /// The hub has the module (`module.ready` sent): send the snapshot of the facts.
    Ready,
    /// `module.exit`: hand back and exit 0.
    Exit,
    /// The hub refused the module; the text is for the log. The module ends.
    Refused(String),
}

/// One connection's worth of module protocol over `W`.
pub struct Link<W: Write> {
    session: Session<io::Empty, W>,
    launch: Launch,
    hub: Hub,
    phase: Phase,
    /// Whether the hub lets this module publish `keyboard.*`.
    publishing: bool,
    /// What went wrong with facts, said once.
    said: bool,
    /// Log lines for the caller (kept apart so tests can read them).
    log: Vec<String>,
}

impl<W: Write> Link<W> {
    pub fn new(writer: W, launch: Launch) -> Link<W> {
        Link {
            session: Session::new(io::empty(), writer),
            launch,
            hub: Hub::default(),
            phase: Phase::Fresh,
            publishing: false,
            said: false,
            log: Vec::new(),
        }
    }

    /// Gives the module the app's v1 handlers to answer routed calls with.
    pub fn with_devices(
        mut self,
        devices: std::sync::Arc<dyn foxglove_plugin_host::Devices>,
    ) -> Self {
        self.hub.devices = Some(devices);
        self
    }

    /// The writer (in tests: what the module sent).
    #[cfg(test)]
    pub fn writer_mut(&mut self) -> &mut W {
        self.session.writer_mut()
    }

    /// Whether the hub has the module (registered, `module.ready` sent).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn registered(&self) -> bool {
        self.phase == Phase::Live
    }

    /// Whether facts are going to the hub.
    pub fn live(&self) -> bool {
        self.phase == Phase::Live && self.publishing
    }

    /// Takes the log lines so far.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// `initialize` (§7.5 step 2).
    pub fn start(&mut self) -> Result<(), SessionError> {
        self.session.initialize(&InitializeModule {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            instance: self.launch.instance.clone(),
            token: self.launch.token.clone(),
            hub: HUB.into(),
            api: [("foxglove".to_string(), API.to_string())].into(),
        })?;
        self.phase = Phase::Initializing;
        Ok(())
    }

    /// One line from the hub.
    pub fn line(&mut self, text: &str) -> Result<Step, SessionError> {
        let got = self.session.handle_line(text, &mut self.hub)?;
        if self.hub.exit {
            return Ok(Step::Exit);
        }
        let Incoming::Response {
            method, outcome, ..
        } = got
        else {
            return Ok(Step::Nothing);
        };
        match (method.as_str(), outcome) {
            ("initialize", Ok(result)) => {
                let granted = result["granted"]
                    .as_array()
                    .is_some_and(|g| g.iter().any(|p| p.as_str() == Some(GRANT)));
                self.publishing = granted;
                if !granted {
                    self.say(format!(
                        "Foxglove: the hub has not granted {GRANT}; the module runs without \
                         publishing the keyboard's facts to it"
                    ));
                }
                self.session.register(&ModuleRegister {
                    facts: vec![FACTS.into()],
                    methods: SERVED.iter().map(|m| m.to_string()).collect(),
                    apis: [("foxglove".to_string(), API.to_string())].into(),
                    ..Default::default()
                })?;
                self.phase = Phase::Registering;
                Ok(Step::Nothing)
            }
            ("initialize", Err(e)) => Ok(Step::Refused(format!(
                "Foxglove: the hub refused this module: {}",
                reason(&e)
            ))),
            ("module.register", Ok(result)) => {
                let result: RegisterResult = serde_json::from_value(result)
                    .map_err(|e| SessionError::Protocol(e.to_string()))?;
                if !result.rejected.is_empty() {
                    let items: Vec<String> = result
                        .rejected
                        .iter()
                        .map(|r| format!("{} ({})", r.item, r.error))
                        .collect();
                    self.say(format!(
                        "Foxglove: the hub refused to register {}",
                        items.join(", ")
                    ));
                }
                self.session.ready(&self.launch.instance)?;
                self.phase = Phase::Live;
                self.hub.live = true;
                Ok(Step::Ready)
            }
            ("module.register", Err(e)) => Ok(Step::Refused(format!(
                "Foxglove: the hub refused the registration: {}",
                reason(&e)
            ))),
            ("facts.set", Err(e)) => {
                if e.code == code::DENIED {
                    // The grant went (or never was): stop, once. A later grant takes a restart.
                    self.publishing = false;
                }
                self.say(format!(
                    "Foxglove: the hub refused a keyboard fact: {}",
                    reason(&e)
                ));
                Ok(Step::Nothing)
            }
            _ => Ok(Step::Nothing),
        }
    }

    /// The facts the module holds now, sent when it is ready (and again after a reconnect).
    pub fn snapshot(&mut self, facts: Vec<(String, Fact)>) -> Result<(), SessionError> {
        for (key, value) in facts {
            self.set(&key, Some(&value))?;
        }
        Ok(())
    }

    /// One change of a fact (`None`: it is gone).
    pub fn change(&mut self, change: &Change) -> Result<(), SessionError> {
        self.set(&change.key, change.value.as_ref())
    }

    fn set(&mut self, key: &str, value: Option<&Fact>) -> Result<(), SessionError> {
        // Only the pattern the module registered, whatever the caller hands it.
        if !self.live() || !key.starts_with("keyboard.") {
            return Ok(());
        }
        let value = match value {
            Some(v) => serde_json::to_value(to_wire(v))
                .map_err(|e| SessionError::Protocol(e.to_string()))?,
            None => Value::Null,
        };
        self.session
            .set_fact(json!({ "key": key, "value": value }))?;
        Ok(())
    }

    fn say(&mut self, text: String) {
        if !self.said {
            self.said = true;
            self.log.push(text);
        }
    }
}

fn reason(e: &RpcError) -> String {
    e.data
        .as_ref()
        .and_then(|d| d["reason"].as_str())
        .map_or_else(|| e.message.clone(), String::from)
}

fn to_wire(v: &Fact) -> foxglove_plugin::Fact {
    use base64::{engine::general_purpose::STANDARD, Engine};
    match v {
        Fact::Bool(b) => foxglove_plugin::Fact::Bool(*b),
        Fact::Int(i) => foxglove_plugin::Fact::Int(*i),
        Fact::Float(f) => foxglove_plugin::Fact::Float(*f),
        Fact::Text(t) => foxglove_plugin::Fact::Text(t.clone()),
        Fact::Bytes(b) => foxglove_plugin::Fact::Bytes {
            base64: STANDARD.encode(b),
        },
    }
}

// ---- the runner: a pipe, two threads and a bounded backoff ----

/// Starts the link when the app is the hub's module. Call once, after the devices are up.
pub fn start() {
    #[cfg(windows)]
    if let Some(launch) = LAUNCH.get() {
        let launch = launch.clone();
        super::services::spawn_named("hub-link", move || runner::run(launch));
    }
}

/// How long the hub has to answer `initialize` and `module.register` (L1 of the review).
#[cfg_attr(not(windows), allow(dead_code))]
const HANDSHAKE: std::time::Duration = std::time::Duration::from_secs(5);

/// Who the hub is, for a reconnect (M1 of the review): the process the launch named and the
/// full path of its image, recorded while that process was alive.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HubId {
    pub pid: u32,
    pub path: Option<std::path::PathBuf>,
}

/// What to do about a reconnect.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Pick {
    /// Open the pipe, requiring the server to be this process.
    Connect(u32),
    /// Nothing to connect to yet: try again within the grace period.
    Wait(String),
    /// A server that must not get the token.
    Refuse(String),
}

/// Chooses the server pid a reconnect may send the token to. While the original hub process is
/// alive it is the only one allowed (a different server on the pipe is refused by the pid check
/// in `connect_router`, never downgraded to a name check). Once it is gone, a new hub is taken
/// only if its image is exactly the recorded path, it runs as this user (the probe checks) at
/// Medium integrity or above, and it is no AppContainer.
#[cfg(windows)]
pub(super) fn pick(
    hub: &HubId,
    probe: impl FnOnce() -> foxglove_plugin::owner::Probe,
    image_of: impl Fn(u32) -> Option<std::path::PathBuf>,
    peer: impl Fn(u32) -> std::io::Result<hideout_acl::Peer>,
) -> Pick {
    use foxglove_plugin::owner::Probe;
    if let Some(image) = image_of(hub.pid) {
        if hub.path.as_ref().is_none_or(|p| same_path(p, &image)) {
            return Pick::Connect(hub.pid);
        }
    }
    let Some(path) = &hub.path else {
        return Pick::Wait("the hub is gone and its path was never known".into());
    };
    match probe() {
        Probe::Free => Pick::Wait("the hub's pipe is not there".into()),
        Probe::Foreign { why, .. } => {
            Pick::Refuse(format!("the pipe's server is not the hub: {why}"))
        }
        Probe::Verified { pid, image } => {
            if !same_path(path, &image) {
                return Pick::Refuse(format!(
                    "pid {pid} serves the pipe from {}, not the hub's {}",
                    image.display(),
                    path.display()
                ));
            }
            match peer(pid) {
                Ok(p) if p.app_container.is_none() && p.integrity >= hideout_acl::MEDIUM_RID => {
                    Pick::Connect(pid)
                }
                Ok(_) => Pick::Refuse(format!(
                    "pid {pid} is below Medium integrity or in an AppContainer"
                )),
                Err(e) => Pick::Refuse(format!("pid {pid}: its token is unreadable ({e})")),
            }
        }
    }
}

#[cfg(windows)]
fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// The grace period: it starts when the app starts and restarts only after a connection reached
/// `module.ready`, so a server that accepts and sends garbage cannot keep the app waiting.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct Grace {
    since: std::time::Instant,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Grace {
    pub fn new(now: std::time::Instant) -> Grace {
        Grace { since: now }
    }
    /// A connection reached ready and was then lost.
    pub fn lost_live(&mut self, now: std::time::Instant) {
        self.since = now;
    }
    pub fn expired(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.since) >= GRACE
    }
}

/// Fact changes waiting for the link, by key: the latest value wins, so the queue is never
/// longer than the set of keys, whatever the hub does.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
pub(super) struct Pending {
    inner: std::sync::Mutex<(std::collections::BTreeMap<String, Option<Fact>>, bool)>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Pending {
    /// Records a change; true when the link needs waking (nothing was waiting).
    pub fn push(&self, c: Change) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.0.insert(c.key, c.value);
        !std::mem::replace(&mut g.1, true)
    }
    pub fn take(&self) -> Vec<Change> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.1 = false;
        std::mem::take(&mut g.0)
            .into_iter()
            .map(|(key, value)| Change { key, value })
            .collect()
    }
}

#[cfg(windows)]
mod runner {
    use std::io::{BufRead, BufReader, Read};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use foxglove_plugin::channel::{connect_router, ChannelConn};
    use foxglove_plugin::hideout::MAX_MESSAGE;
    use foxglove_plugin::owner::{self, Expect};

    use super::*;

    /// What the link thread waits on: the one place hub lines and fact changes meet.
    pub(super) enum Msg {
        /// A line from connection `.0`.
        Line(u64, String),
        /// Connection `.0` ended.
        Closed(u64),
        /// Changes are waiting in [`Pending`].
        Facts,
    }

    /// Why a connection ended.
    pub(super) enum Ended {
        /// The pipe closed, broke or went quiet.
        Lost(String),
        Exit,
        Refused(String),
    }

    /// Reader threads running now (tests: none is left behind after a connection ends).
    pub(super) static READERS: AtomicUsize = AtomicUsize::new(0);

    /// Backoff between attempts: 0.5 s doubling to 8 s, within [`GRACE`] of the last time the
    /// hub was there.
    pub(super) fn delay(attempt: u32) -> Duration {
        Duration::from_millis(500u64 << attempt.min(4))
    }

    pub fn run(launch: Launch) {
        let facts = crate::foxglove::devices::bus().facts().clone();
        let (tx, rx) = mpsc::channel::<Msg>();
        // Changes are forwarded for the life of the process, coalesced by key; while no hub is
        // connected they wait and are dropped on reconnect (the snapshot has the values).
        let pending = Arc::new(Pending::default());
        let changes = facts.subscribe(FACTS);
        let (ftx, fpending) = (tx.clone(), pending.clone());
        crate::foxglove::services::spawn_named("hub-link-facts", move || {
            for c in changes {
                if fpending.push(c) && ftx.send(Msg::Facts).is_err() {
                    break;
                }
            }
        });
        // Recorded while the hub's process is alive: a restarted hub must run this exact image.
        let mut hub = HubId {
            pid: launch.hub_pid,
            path: owner::image_of(launch.hub_pid),
        };
        let mut generation = 0u64;
        let mut grace = Grace::new(Instant::now());
        let mut attempt = 0u32;
        loop {
            generation += 1;
            let picked = pick(
                &hub,
                || owner::probe(&launch.pipe, Expect::HIDEOUT, owner::TIMEOUT),
                owner::image_of,
                hideout_acl::peer,
            );
            let ended = match picked {
                Pick::Connect(pid) => match connect_router(&launch.pipe, pid) {
                    Ok(conn) => {
                        let (ended, live) =
                            serve(&launch, conn, generation, &tx, &rx, &pending, &facts);
                        if live {
                            hub.pid = pid;
                            grace.lost_live(Instant::now());
                            attempt = 0;
                        }
                        ended
                    }
                    Err(e) => Ended::Lost(e.to_string()),
                },
                Pick::Wait(why) | Pick::Refuse(why) => Ended::Lost(why),
            };
            match ended {
                Ended::Exit => finish("the hub asked the module to exit", 0),
                Ended::Refused(why) => {
                    crate::foxglove::app_log::early(&why);
                    finish("the hub refused the module", 1)
                }
                Ended::Lost(why) => {
                    if grace.expired(Instant::now()) {
                        crate::foxglove::app_log::early(&format!(
                            "Foxglove: no hub for {} s ({why}); handing everything back and exiting",
                            GRACE.as_secs()
                        ));
                        finish("the hub is gone", 0);
                    }
                    crate::foxglove::app_log::line(&format!(
                        "Foxglove: not connected to the hub ({why}); trying again"
                    ));
                    std::thread::sleep(delay(attempt));
                    attempt += 1;
                }
            }
        }
    }

    /// Hands the app's devices back and ends the process, as the tray's Quit does.
    fn finish(why: &str, code: i32) -> ! {
        crate::foxglove::quit::hand_back(why);
        std::process::exit(code)
    }

    /// Ends the reader: cancels its blocked read until it has gone, so no thread or handle
    /// outlives the connection (bounded: 40 tries of 50 ms).
    fn stop_reader(conn: &ChannelConn, done: &Receiver<()>) {
        for _ in 0..40 {
            conn.cancel();
            match done.recv_timeout(Duration::from_millis(50)) {
                Err(RecvTimeoutError::Timeout) => continue,
                _ => break,
            }
        }
    }

    /// One connection. The flag says whether it reached ready.
    pub(super) fn serve(
        launch: &Launch,
        conn: ChannelConn,
        generation: u64,
        tx: &Sender<Msg>,
        rx: &Receiver<Msg>,
        pending: &Pending,
        facts: &foxglove_devices::facts::Facts,
    ) -> (Ended, bool) {
        // Changes and lines from before are stale: the snapshot says it all.
        for _ in rx.try_iter() {}
        pending.take();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let reader = conn.clone();
        let canceller = conn.clone();
        let ltx = tx.clone();
        READERS.fetch_add(1, Ordering::SeqCst);
        crate::foxglove::services::spawn_named("hub-link-read", move || {
            let _done = done_tx;
            let mut reader = BufReader::new(reader);
            loop {
                let mut line = String::new();
                match (&mut reader)
                    .take(MAX_MESSAGE as u64 + 2)
                    .read_line(&mut line)
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) if !line.ends_with('\n') => break, // over the wire limit: end it
                    Ok(_) => {
                        if ltx.send(Msg::Line(generation, line)).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = ltx.send(Msg::Closed(generation));
            READERS.fetch_sub(1, Ordering::SeqCst);
        });
        let (ended, live) = converse(launch, conn, generation, rx, pending, facts);
        stop_reader(&canceller, &done_rx);
        (ended, live)
    }

    fn converse(
        launch: &Launch,
        conn: ChannelConn,
        generation: u64,
        rx: &Receiver<Msg>,
        pending: &Pending,
        facts: &foxglove_devices::facts::Facts,
    ) -> (Ended, bool) {
        let mut link = Link::new(conn, launch.clone())
            .with_devices(Arc::new(crate::foxglove::api::AppDevices));
        let mut live = false;
        let lost = |e: SessionError, live| (Ended::Lost(e.to_string()), live);
        if let Err(e) = link.start() {
            return lost(e, live);
        }
        let started = Instant::now();
        loop {
            // Until the hub has the module it has HANDSHAKE to answer; after that, no clock.
            let msg = if link.registered() {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                rx.recv_timeout(HANDSHAKE.saturating_sub(started.elapsed()))
            };
            let msg = match msg {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    return (
                        Ended::Lost("the hub did not answer the handshake in time".into()),
                        live,
                    )
                }
                Err(_) => return (Ended::Lost("the link's queue closed".into()), live),
            };
            let step = match msg {
                Msg::Line(g, line) if g == generation => match link.line(&line) {
                    Ok(step) => step,
                    Err(e) => return lost(e, live),
                },
                Msg::Closed(g) if g == generation => {
                    return (Ended::Lost("the hub closed the pipe".into()), live)
                }
                Msg::Facts => {
                    for change in pending.take() {
                        if let Err(e) = link.change(&change) {
                            return lost(e, live);
                        }
                    }
                    Step::Nothing
                }
                _ => Step::Nothing, // a line of an older connection
            };
            for l in link.take_log() {
                crate::foxglove::app_log::line(&l);
            }
            match step {
                Step::Nothing => {}
                Step::Exit => return (Ended::Exit, live),
                Step::Refused(why) => return (Ended::Refused(why), live),
                Step::Ready => {
                    live = true;
                    crate::foxglove::app_log::line("Foxglove: registered with the hub as a module");
                    if let Err(e) = link.snapshot(facts.list(FACTS)) {
                        return lost(e, live);
                    }
                }
            }
        }
    }
}
