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
//! - **Panels (P8).** Three tiles (Keyboard, Keyboard lights, Keyboard art) and one page
//!   ("Foxglove") in the hub, drawn by [`super::hub_panels`] from the same `keyboard.*` facts and
//!   nothing else: `ui.register` when the module is ready, `ui.patch` on each change (a new
//!   `ui.register` when the shape changes), driven by the fact changes and the page guard's
//!   (event-driven, no poll). The one button, "Open Foxglove settings", comes back as a plain
//!   `ui.event`; the module opens its own Settings window at that part's page. Standalone, none
//!   of this runs.
//! - **Hand-off (P9).** With `facts.read:module.*` the module reads which methods the other
//!   registered modules provide (the hub's `module.<id>.provides`, listed after `initialize` and
//!   followed through `facts.changed`; nothing polls). A feature in [`super::features`] whose
//!   methods another module provides is switched off here: the module registers without those
//!   methods (a new `module.register` replaces the old one, so there is one writer) and the
//!   feature's device duties end ([`super::features::apply`]). When that module goes, the
//!   methods are claimed again at once and a feature that had to be stopped starts with a
//!   planned restart of this process (contract section 7.7; nothing counts as a crash); one that
//!   only refuses writes (`pc_lighting.*`) is on again at once. Standalone none of this runs.
//! - **No tray, no pipe.** The tray is not created ([`crate::tray::Tray::none`]) and the app
//!   serves no `foxglove-<user>` pipe (the hub owns that alias, §8.6); every device duty stays.
//!   The self-restarts that would start a plain copy (identity registration) are off; the GPU
//!   reset and the updater's manual install are planned exits ([`planned_exit`], #1143 P7).
//! - **Liveness (§7.9).** When the hub's pipe closes the app keeps running, retries with a
//!   bounded backoff for the 30-second grace period, and then hands everything back and exits 0.
//!   `module.exit` does the same at once. A hub that refuses `initialize` ends the module.
//!
//! The wire is the SDK's [`Session`]; the state machine here ([`Link`]) is generic over the
//! writer so the tests drive it against the hub's own `Router` with no pipe and no window.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use foxglove_devices::facts::{Change, Value as Fact};
use foxglove_plugin::hideout::{
    code, DrainResult, Handler, Incoming, InitializeModule, ModuleDrain, ModuleRegister, PatchOp,
    Register, RegisterResult, RpcError, Session, SessionError, UiEvent, UiPatch,
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
/// The permission that lets it read the hub's `module.*` facts: which methods the other
/// registered modules provide (`module.<id>.provides`), the registry check of the feature
/// hand-off (P9, [`super::features`]). Without it nothing is handed off and the module serves
/// everything in [`SERVED`], as before.
const GRANT_REGISTRY: &str = "facts.read:module.*";
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

/// Routed calls waiting for the worker, all callers together (#1156 M2). The router already
/// caps one caller at `hideout_router::MAX_INFLIGHT_PER_CALLER` forwarded and unanswered calls
/// (the module never sees who calls, so the per-caller bound lives there); this is the module's
/// own bound on what its one worker has queued. A call over it is answered BUSY (error 3,
/// contract section 8.1 "busy") at once, before anything runs.
pub const MAX_QUEUED_CALLS: usize = 16;

/// How long a routed call may wait for a worker when the router did not say (an older hub):
/// the router answers its caller TIMEOUT after `CALL_TIMEOUT` (30 s) and forgets the call, and
/// running it after that would land side effects for a caller that was told it failed.
pub const CALL_EXPIRY: Duration = Duration::from_secs(20);

/// The router puts the time left before its deadline into a forwarded call's params as
/// `_deadline_ms` (hideout-router `forward_counted`); the module takes it out before the
/// handler sees the params and drops the call this long before that deadline.
const DEADLINE_KEY: &str = "_deadline_ms";
const EXPIRY_MARGIN: Duration = Duration::from_secs(2);

/// Workers per connection: one slow call (a 5-10 s device wait) cannot hold every other
/// caller's call behind it. With the router's provider-wide cap (12 of the queue's 16) and its
/// per-caller cap (4), one caller can occupy at most 3 workers' worth of time only briefly.
pub const CALL_WORKERS: usize = 3;

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

/// A planned exit (#1143 P7, contract §7.7): hands everything back as Quit does and ends with
/// `PLANNED_EXIT_CODE` (contract crate), so the hub's supervisor restarts the module without
/// counting a crash. The code is read from the exit of the process the supervisor launched;
/// nothing is sent on the link for it, so no other module can claim or fake it. Module mode
/// only; the callers check.
pub fn planned_exit(why: &str) -> ! {
    crate::foxglove::quit::hand_back(why);
    end_process(foxglove_plugin_api::hideout::PLANNED_EXIT_CODE)
}

/// Ends the process now with `code`. After a GPU reset the driver may hold a lock in its DLL
/// detach that a normal exit would wait on, so Windows ends the process directly.
fn end_process(code: i32) -> ! {
    #[cfg(windows)]
    // SAFETY: the pseudo-handle for this process needs no closing.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), code as u32);
    }
    std::process::exit(code)
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
    /// Opens Settings at a page for a panel's button; `None` uses the host's
    /// ([`super::settings_open::open_page_from_hub`]).
    opener: Option<Opener>,
    /// `module.<id>.provides` facts the hub sent (`facts.changed`), for the link to read.
    changes: Vec<(String, Value)>,
}

/// What a panel's button opens Settings with.
pub type Opener = Arc<dyn Fn(super::shell::Page) + Send + Sync>;

impl Handler for Hub {
    fn drain(&mut self, _drain: ModuleDrain) -> Result<DrainResult, RpcError> {
        Ok(DrainResult::Done)
    }
    fn exit(&mut self) {
        self.exit = true;
    }
    fn notification(&mut self, method: &str, params: Value) {
        if method != "facts.changed" {
            return;
        }
        if let Some(key) = params["key"].as_str().filter(|k| is_provides(k)) {
            self.changes
                .push((key.to_string(), params["value"].clone()));
        }
    }
    /// A click on one of the panels' buttons (the hub sends `kind: "click"`): opens Settings at
    /// the page of that part. Only while the hub has the module; nothing else is a button.
    fn ui_event(&mut self, event: UiEvent) -> Result<Value, RpcError> {
        if !self.live {
            return Err(RpcError::new(code::DISCONNECTED, "disconnected"));
        }
        if event.kind == "click" {
            if let Some(part) = super::hub_panels::Part::of_event(&event.panel, &event.node) {
                let page = part.settings_page();
                match &self.opener {
                    Some(open) => open(page),
                    None => super::settings_open::open_page_from_hub(page),
                }
            }
        }
        Ok(json!({}))
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
            return Err(RpcError::new(code::DISCONNECTED, "disconnected"));
        };
        run_handler(devices.as_ref(), method, params)
    }
}

/// Whether `key` is `module.<id>.provides`.
fn is_provides(key: &str) -> bool {
    key.strip_prefix("module.")
        .and_then(|k| k.strip_suffix(".provides"))
        .is_some_and(|id| !id.is_empty())
}

/// The module id of a `module.<id>.provides` key.
fn provider_of(key: &str) -> &str {
    key.strip_prefix("module.")
        .and_then(|k| k.strip_suffix(".provides"))
        .unwrap_or(key)
}

/// The app's handler for one routed call (it may block for seconds).
fn run_handler(
    devices: &dyn foxglove_plugin_host::Devices,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    devices
        .call_from(ROUTED_CONN, method, params)
        .map_err(|e| RpcError {
            code: i64::from(e.code),
            message: e.message,
            data: e.data,
        })
}

/// One routed call waiting for the worker.
pub(super) struct Job {
    id: Value,
    method: String,
    params: Value,
    /// Past this the router has timed the call out (or is about to): never run it.
    expires: Instant,
}

/// What the worker made of a [`Job`]: the answer for the hub.
pub(super) struct Reply {
    id: Value,
    outcome: Result<Value, RpcError>,
}

/// The bounded queue between the link thread and the worker. Closed with its connection: a
/// call queued for a link that has gone is never run.
#[derive(Default)]
pub(super) struct CallQueue {
    state: Mutex<(VecDeque<Job>, bool)>,
    wake: Condvar,
}

impl CallQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, (VecDeque<Job>, bool)> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues a job; `false` when the queue is full (or closed). Jobs already past
    /// [`CALL_EXPIRY`] are dropped first: their callers were answered TIMEOUT by the router.
    fn push(&self, job: Job, now: Instant) -> bool {
        let mut g = self.lock();
        g.0.retain(|j| !expired(j, now));
        if g.1 || g.0.len() >= MAX_QUEUED_CALLS {
            return false;
        }
        g.0.push_back(job);
        self.wake.notify_one();
        true
    }

    /// The next job without waiting (tests; the worker uses [`CallQueue::wait`]).
    #[cfg(test)]
    fn try_pop(&self) -> Option<Job> {
        self.lock().0.pop_front()
    }

    /// Waits for the next job; `None` when the queue is closed.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn wait(&self) -> Option<Job> {
        let mut g = self.lock();
        loop {
            if g.1 {
                return None;
            }
            if let Some(j) = g.0.pop_front() {
                return Some(j);
            }
            g = self.wake.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close(&self) {
        let mut g = self.lock();
        g.1 = true;
        g.0.clear();
        self.wake.notify_all();
    }
}

fn expired(job: &Job, now: Instant) -> bool {
    now >= job.expires
}

/// Runs one job unless it expired while queued (then `None`: dropped, never run).
pub(super) fn execute(
    devices: &dyn foxglove_plugin_host::Devices,
    job: Job,
    now: Instant,
) -> Option<Reply> {
    if expired(&job, now) {
        return None;
    }
    // A handler that panics answers an internal error: the caller is not left to the router's
    // timeout and the worker lives on (#1156 review).
    let (method, params) = (job.method, job.params);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_handler(devices, &method, params)
    }))
    .unwrap_or_else(|_| {
        crate::foxglove::app_log::line(&format!(
            "Foxglove: the handler of {method} panicked; answered an internal error"
        ));
        Err(RpcError::new(code::INTERNAL, "internal error"))
    });
    Some(Reply {
        outcome,
        id: job.id,
    })
}

/// The worker: runs queued jobs one at a time until the queue closes, handing each answer to
/// `deliver` (which wakes the link thread; the link thread alone writes to the hub).
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn work(
    queue: &CallQueue,
    devices: &dyn foxglove_plugin_host::Devices,
    mut deliver: impl FnMut(Reply),
) {
    while let Some(job) = queue.wait() {
        if let Some(reply) = execute(devices, job, Instant::now()) {
            deliver(reply);
        }
    }
}

/// [`work`] that cannot end quietly: when the worker dies (a panic outside the handler, which
/// `execute` already answers), the queue is closed and `lost` runs, so the link ends the
/// connection instead of keeping a link whose calls nobody runs.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn run_worker(
    queue: &CallQueue,
    devices: &dyn foxglove_plugin_host::Devices,
    deliver: impl FnMut(Reply),
    lost: impl FnOnce(),
) {
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        work(queue, devices, deliver)
    }));
    if ended.is_err() {
        queue.close();
        lost();
    }
}

/// How far the handshake is (§7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Fresh,
    Initializing,
    /// Asked the hub which methods the others provide; registers when it answers.
    Surveying,
    Registering,
    Live,
}

/// What a hub line led to.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing for the caller to do.
    Nothing,
    /// The hub has the module (`module.ready` sent): send the snapshot of the facts, and apply
    /// [`Link::off`].
    Ready,
    /// Another module started or stopped providing methods of a feature: [`Link::off`] is the
    /// set of features that are off now, and the registration was sent again without them.
    Handoff,
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
    /// Where routed calls go to run off the link thread; `None` runs them inline (tests).
    queue: Option<Arc<CallQueue>>,
    /// The panels the hub has now (what `ui.register` or the last patches made), for the next diff.
    shown: Option<Vec<Value>>,
    /// The hub lets the module read `module.*` (the registry check can run).
    watching: bool,
    /// Each other module's provided method patterns, from `module.<id>.provides`.
    provided: std::collections::BTreeMap<String, Vec<String>>,
    /// The features switched off for another module's sake now.
    off: Vec<&'static str>,
}

impl<W: Write> Drop for Link<W> {
    fn drop(&mut self) {
        if let Some(q) = &self.queue {
            q.close();
        }
    }
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
            queue: None,
            shown: None,
            watching: false,
            provided: Default::default(),
            off: Vec::new(),
        }
    }

    /// The features that are off in this module because another module provides their methods.
    pub fn off(&self) -> &[&'static str] {
        &self.off
    }

    /// The v1 methods this module claims now: [`SERVED`] less the areas of the features that
    /// are off.
    pub fn claimed(&self) -> Vec<String> {
        SERVED
            .iter()
            .filter(|m| !super::features::withheld(m, &self.off))
            .map(|m| m.to_string())
            .collect()
    }

    fn send_register(&mut self) -> Result<(), SessionError> {
        let methods = self.claimed();
        self.session.register(&ModuleRegister {
            facts: vec![FACTS.into()],
            methods,
            apis: [("foxglove".to_string(), API.to_string())].into(),
            ..Default::default()
        })?;
        Ok(())
    }

    /// Takes the `module.<id>.provides` changes the hub sent into the registry view.
    fn absorb(&mut self, changes: Vec<(String, Value)>) {
        for (key, value) in changes {
            let id = provider_of(&key).to_string();
            if id == ID {
                continue;
            }
            let list: Vec<String> = value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            if list.is_empty() {
                self.provided.remove(&id);
            } else {
                self.provided.insert(id, list);
            }
        }
    }

    /// Re-reads the plan: when the set of features to turn off changed, registers again
    /// without (or with) their methods and says so.
    fn rehand(&mut self) -> Result<bool, SessionError> {
        let want = super::features::plan(ID, &self.provided);
        if want == self.off {
            return Ok(false);
        }
        let gone: Vec<&str> = want
            .iter()
            .filter(|f| !self.off.contains(f))
            .copied()
            .collect();
        let back: Vec<&str> = self
            .off
            .iter()
            .filter(|f| !want.contains(f))
            .copied()
            .collect();
        if !gone.is_empty() {
            self.log.push(format!(
                "Foxglove: another module provides the methods of {}; those features are off here",
                gone.join(", ")
            ));
        }
        if !back.is_empty() {
            self.log.push(format!(
                "Foxglove: the module that provided {} is gone; claiming them again",
                back.join(", ")
            ));
        }
        self.off = want;
        self.send_register()?;
        Ok(true)
    }

    /// Opens Settings through `opener` for the panels' buttons (tests; the app uses the host's).
    #[cfg(test)]
    pub fn with_opener(mut self, opener: Opener) -> Self {
        self.hub.opener = Some(opener);
        self
    }

    /// Shows the panels for `facts` (the module's `keyboard.*`) and the guard's state: the
    /// first time `ui.register`, afterwards `ui.patch` of what changed (a new `ui.register` when
    /// the shape did). Nothing before the hub has the module, or when nothing changed.
    pub fn refresh_panels(
        &mut self,
        facts: &[(String, Fact)],
        stopped: super::hub_panels::Stopped,
    ) -> Result<(), SessionError> {
        if self.phase != Phase::Live {
            return Ok(());
        }
        let panels = super::hub_panels::panels(&super::hub_panels::View::of(facts, stopped));
        if self.shown.as_ref() == Some(&panels) {
            return Ok(());
        }
        let patches = self.shown.as_ref().and_then(|old| {
            if old.len() != panels.len() {
                return None;
            }
            let mut all = Vec::new();
            for (o, n) in old.iter().zip(&panels) {
                if (&o["id"], &o["title"], &o["icon"], &o["slot"])
                    != (&n["id"], &n["title"], &n["icon"], &n["slot"])
                {
                    return None;
                }
                let ops = foxglove_plugin::ui::diff(&o["tree"], &n["tree"])?;
                if !ops.is_empty() {
                    all.push((n["id"].as_str()?.to_string(), ops));
                }
            }
            Some(all)
        });
        match patches {
            Some(all) => {
                for (panel, ops) in all {
                    let ops: Vec<PatchOp> = serde_json::from_value(Value::Array(ops))
                        .map_err(|e| SessionError::Protocol(e.to_string()))?;
                    self.session.ui_patch(&UiPatch { panel, ops })?;
                }
            }
            None => {
                self.session.ui_register(&Register {
                    instance: self.launch.instance.clone(),
                    panels: panels.clone(),
                })?;
            }
        }
        self.shown = Some(panels);
        Ok(())
    }

    /// Runs routed calls on a worker instead of inline (#1156 M2): the link thread only reads
    /// lines and writes answers, so a 5-10 s handler cannot hold up the keyboard facts or
    /// `module.exit`. Returns the queue for the worker ([`work`]).
    pub(super) fn with_worker(&mut self) -> Arc<CallQueue> {
        let q = Arc::new(CallQueue::default());
        self.queue = Some(q.clone());
        q
    }

    /// Sends the worker's answer to the hub.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn respond(&mut self, reply: Reply) -> Result<(), SessionError> {
        self.session.respond(reply.id, reply.outcome)
    }

    /// A routed request for a served name, taken apart; `None` for anything else (the SDK
    /// handles those, `-32601` for a name that is not served included).
    fn routed(text: &str) -> Option<(Value, String, Value)> {
        let msg: Value = serde_json::from_str(text).ok()?;
        if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return None;
        }
        let method = msg.get("method")?.as_str()?;
        let id = msg.get("id")?.clone();
        SERVED.contains(&method).then(|| {
            (
                id,
                method.to_string(),
                msg.get("params").cloned().unwrap_or_else(|| json!({})),
            )
        })
    }

    /// A routed call: refused at once when the hub does not have the module or the queue is
    /// full (BUSY, error 3), else queued for the worker (or run inline without one).
    fn submit(
        &mut self,
        (id, method, mut params): (Value, String, Value),
        now: Instant,
    ) -> Result<(), SessionError> {
        // The router's remaining time, out of the params the handler sees.
        let left = params
            .as_object_mut()
            .and_then(|m| m.remove(DEADLINE_KEY))
            .and_then(|v| v.as_u64())
            .map(Duration::from_millis);
        let expires = now
            + left
                .map(|l| l.saturating_sub(EXPIRY_MARGIN))
                .unwrap_or(CALL_EXPIRY);
        let live = self.hub.devices.is_some() && self.hub.live;
        let outcome = match (&self.queue, live) {
            (_, false) => Err(RpcError::new(code::DISCONNECTED, "disconnected")),
            (None, true) => self.hub.call(&method, params),
            (Some(q), true) => {
                let job = Job {
                    id: id.clone(),
                    method,
                    params,
                    expires,
                };
                // Already past what the router allows: it has answered the caller or is about
                // to, and nothing may run.
                if expired(&job, now) {
                    return Ok(());
                }
                if q.push(job, now) {
                    return Ok(());
                }
                Err(RpcError::new(code::BUSY, "busy"))
            }
        };
        self.session.respond(id, outcome)
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
        self.line_at(text, Instant::now())
    }

    /// [`Link::line`] with the clock given (tests use a fake one).
    pub(super) fn line_at(&mut self, text: &str, now: Instant) -> Result<Step, SessionError> {
        if let Some(call) = Self::routed(text) {
            self.submit(call, now)?;
            return Ok(Step::Nothing);
        }
        let got = self.session.handle_line(text, &mut self.hub)?;
        if self.hub.exit {
            return Ok(Step::Exit);
        }
        let changes = std::mem::take(&mut self.hub.changes);
        if !changes.is_empty() {
            self.absorb(changes);
            if self.phase == Phase::Live && self.rehand()? {
                return Ok(Step::Handoff);
            }
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
                self.watching = result["granted"]
                    .as_array()
                    .is_some_and(|g| g.iter().any(|p| p.as_str() == Some(GRANT_REGISTRY)));
                if self.watching {
                    // Subscribe first, list second: a change between the two is not lost.
                    self.session
                        .request("facts.subscribe", json!({ "pattern": "module.*" }))?;
                    self.session
                        .request("facts.list", json!({ "pattern": "module.*" }))?;
                    self.phase = Phase::Surveying;
                    return Ok(Step::Nothing);
                }
                self.send_register()?;
                self.phase = Phase::Registering;
                Ok(Step::Nothing)
            }
            ("facts.list", outcome) if self.phase == Phase::Surveying => {
                match outcome {
                    Ok(result) => {
                        let found = result["facts"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|f| f["key"].as_str().is_some_and(is_provides))
                            .map(|f| {
                                (
                                    f["key"].as_str().unwrap_or_default().to_string(),
                                    f["value"].clone(),
                                )
                            })
                            .collect();
                        self.absorb(found);
                    }
                    Err(e) => self.say(format!(
                        "Foxglove: the hub would not list the other modules ({}); no feature is \
                         handed off",
                        reason(&e)
                    )),
                }
                self.off = super::features::plan(ID, &self.provided);
                self.send_register()?;
                self.phase = Phase::Registering;
                Ok(Step::Nothing)
            }
            ("initialize", Err(e)) => Ok(Step::Refused(format!(
                "Foxglove: the hub refused this module: {}",
                reason(&e)
            ))),
            ("module.register", Ok(_)) if self.phase == Phase::Live => Ok(Step::Nothing),
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
                // A change that came while registering.
                self.rehand()?;
                Ok(Step::Ready)
            }
            ("module.register", Err(e)) => Ok(Step::Refused(format!(
                "Foxglove: the hub refused the registration: {}",
                reason(&e)
            ))),
            ("ui.register", Ok(result)) => {
                if let Some(rejected) = result["rejected"].as_array().filter(|r| !r.is_empty()) {
                    self.say(format!("Foxglove: the hub refused panels: {rejected:?}"));
                }
                Ok(Step::Nothing)
            }
            ("ui.register", Err(e)) => {
                self.say(format!(
                    "Foxglove: the hub refused the panels: {}",
                    reason(&e)
                ));
                Ok(Step::Nothing)
            }
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
        /// A page's guard stopped or retried: the panels say so.
        Guard,
        /// The worker answered a routed call of connection `.0`.
        Reply(u64, super::Reply),
        /// A call worker of connection `.0` died: the connection ends.
        WorkerLost(u64),
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
        // The panels follow the page guard (P8): told by its change, never polled.
        let gtx = tx.clone();
        crate::foxglove::hub_panels::on_guard_change(move || {
            let _ = gtx.send(Msg::Guard);
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

    /// Makes `off` the set of handed-off features. True when a feature that was stopped is
    /// wanted back: it cannot start again inside this process, so the module ends with a
    /// planned exit (contract section 7.7) and the hub starts it again, with the feature.
    fn apply_handoff(off: &[&'static str]) -> bool {
        let done = crate::foxglove::features::apply(off);
        for f in &done.off {
            crate::foxglove::app_log::line(&format!("Foxglove: {f} is handed to another module"));
        }
        if done.restart {
            crate::foxglove::app_log::early(
                "Foxglove: the module that took a feature over is gone; restarting to bring it back",
            );
            planned_exit("a handed-off feature is wanted back");
        }
        false
    }

    /// Hands the app's devices back and ends the process, as the tray's Quit does.
    fn finish(why: &str, code: i32) -> ! {
        crate::foxglove::quit::hand_back(why);
        std::process::exit(code)
    }

    /// Ends the reader: cancels its blocked read until it has gone, so no thread or handle
    /// outlives the connection (bounded: 40 tries of 50 ms).
    fn stop_reader(conn: &ChannelConn, done: &Receiver<()>, stop: &AtomicBool) {
        // A hub that floods lines can have the reader between two reads when a cancel lands,
        // which then cancels nothing: the flag ends it at the next line instead.
        stop.store(true, Ordering::SeqCst);
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
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let reader_stop = stop.clone();
        READERS.fetch_add(1, Ordering::SeqCst);
        crate::foxglove::services::spawn_named("hub-link-read", move || {
            let _done = done_tx;
            let mut reader = BufReader::new(reader);
            loop {
                let mut line = String::new();
                if reader_stop.load(Ordering::SeqCst) {
                    break;
                }
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
        let (ended, live) = converse(launch, conn, generation, tx, rx, pending, facts);
        stop_reader(&canceller, &done_rx, &stop);
        (ended, live)
    }

    fn converse(
        launch: &Launch,
        conn: ChannelConn,
        generation: u64,
        tx: &Sender<Msg>,
        rx: &Receiver<Msg>,
        pending: &Pending,
        facts: &foxglove_devices::facts::Facts,
    ) -> (Ended, bool) {
        let devices: Arc<dyn foxglove_plugin_host::Devices> =
            Arc::new(crate::foxglove::api::AppDevices);
        let mut link = Link::new(conn, launch.clone()).with_devices(devices.clone());
        // Routed calls run on one worker of this connection (it ends when the link drops and
        // closes the queue), never on the link thread.
        let queue = link.with_worker();
        for _ in 0..CALL_WORKERS {
            let (queue, devices, wtx) = (queue.clone(), devices.clone(), tx.clone());
            crate::foxglove::services::spawn_named("hub-link-calls", move || {
                // However the worker ends (a panic outside the handler included) the queue
                // closes and the link thread is told, so the connection ends and is rebuilt
                // rather than silently losing its workers.
                let (rtx, ltx) = (wtx.clone(), wtx);
                run_worker(
                    &queue,
                    devices.as_ref(),
                    |r| {
                        let _ = rtx.send(Msg::Reply(generation, r));
                    },
                    || {
                        let _ = ltx.send(Msg::WorkerLost(generation));
                    },
                );
            });
        }
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
                Msg::Reply(g, reply) if g == generation => match link.respond(reply) {
                    Ok(()) => Step::Nothing,
                    Err(e) => return lost(e, live),
                },
                Msg::WorkerLost(g) if g == generation => {
                    return (Ended::Lost("a call worker died".into()), live)
                }
                Msg::Facts => {
                    for change in pending.take() {
                        if let Err(e) = link.change(&change) {
                            return lost(e, live);
                        }
                    }
                    if let Err(e) = link.refresh_panels(
                        &facts.list(FACTS),
                        crate::foxglove::hub_panels::Stopped::now(),
                    ) {
                        return lost(e, live);
                    }
                    Step::Nothing
                }
                Msg::Guard => {
                    if let Err(e) = link.refresh_panels(
                        &facts.list(FACTS),
                        crate::foxglove::hub_panels::Stopped::now(),
                    ) {
                        return lost(e, live);
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
                Step::Handoff => {
                    if apply_handoff(link.off()) {
                        return (Ended::Exit, live);
                    }
                }
                Step::Ready => {
                    live = true;
                    if apply_handoff(link.off()) {
                        return (Ended::Exit, live);
                    }
                    crate::foxglove::app_log::line("Foxglove: registered with the hub as a module");
                    let all = facts.list(FACTS);
                    if let Err(e) = link.snapshot(all.clone()) {
                        return lost(e, live);
                    }
                    if let Err(e) =
                        link.refresh_panels(&all, crate::foxglove::hub_panels::Stopped::now())
                    {
                        return lost(e, live);
                    }
                }
            }
        }
    }
}
