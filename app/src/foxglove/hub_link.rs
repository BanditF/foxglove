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
}

impl Handler for Hub {
    fn drain(&mut self, _drain: ModuleDrain) -> Result<DrainResult, RpcError> {
        Ok(DrainResult::Done)
    }
    fn exit(&mut self) {
        self.exit = true;
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

    /// The writer (in tests: what the module sent).
    #[cfg(test)]
    pub fn writer_mut(&mut self) -> &mut W {
        self.session.writer_mut()
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
                for r in &result.rejected {
                    self.say(format!(
                        "Foxglove: the hub refused to register {} ({})",
                        r.item, r.error
                    ));
                }
                self.session.ready(&self.launch.instance)?;
                self.phase = Phase::Live;
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

#[cfg(windows)]
mod runner {
    use std::io::{BufRead, BufReader, Read};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    use foxglove_plugin::channel::{connect_router, ChannelConn};
    use foxglove_plugin::hideout::MAX_MESSAGE;
    use foxglove_plugin::owner::{self, Expect, Probe};

    use super::*;

    /// What the link thread waits on: the one place hub lines and fact changes meet.
    enum Msg {
        /// A line from connection `.0`.
        Line(u64, String),
        /// Connection `.0` ended.
        Closed(u64),
        Fact(Change),
    }

    /// Why a connection ended.
    enum Ended {
        /// The pipe closed or broke after the handshake began.
        Lost(String),
        Exit,
        Refused(String),
    }

    /// Backoff between attempts: 0.5 s doubling to 8 s, within [`GRACE`] of the last time the
    /// hub was there.
    pub(super) fn delay(attempt: u32) -> Duration {
        Duration::from_millis(500u64 << attempt.min(4))
    }

    pub fn run(launch: Launch) {
        let facts = crate::foxglove::devices::bus().facts().clone();
        let (tx, rx) = mpsc::channel::<Msg>();
        // Changes are forwarded for the life of the process; while no hub is connected they
        // wait here and are dropped on reconnect (the snapshot has the current values).
        let changes = facts.subscribe(FACTS);
        let ftx = tx.clone();
        crate::foxglove::services::spawn_named("hub-link-facts", move || {
            for c in changes {
                if ftx.send(Msg::Fact(c)).is_err() {
                    break;
                }
            }
        });
        let mut generation = 0u64;
        let mut first = true;
        let mut lost_since = Instant::now();
        let mut attempt = 0u32;
        loop {
            generation += 1;
            // The first connection is to the pid the hub named; later ones (the hub may have
            // restarted, §7.9) to a hideout.exe of this user, whose pid is read from the pipe.
            let conn = server_pid(&launch, first)
                .and_then(|pid| connect_router(&launch.pipe, pid).map_err(|e| e.to_string()));
            let ended = match conn {
                Ok(conn) => {
                    let ended = serve(&launch, conn, generation, &tx, &rx, &facts);
                    if let Ended::Lost(_) = &ended {
                        // The hub was there: the grace period starts now.
                        lost_since = Instant::now();
                        attempt = 0;
                    }
                    ended
                }
                Err(e) => Ended::Lost(e),
            };
            first = false;
            match ended {
                Ended::Exit => finish("the hub asked the module to exit", 0),
                Ended::Refused(why) => {
                    crate::foxglove::app_log::early(&why);
                    finish("the hub refused the module", 1)
                }
                Ended::Lost(why) => {
                    if lost_since.elapsed() >= GRACE {
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

    fn server_pid(launch: &Launch, first: bool) -> Result<u32, String> {
        if first {
            return Ok(launch.hub_pid);
        }
        match owner::probe(&launch.pipe, Expect::HIDEOUT, owner::TIMEOUT) {
            Probe::Verified { pid, .. } => Ok(pid),
            Probe::Free => Err("the hub's pipe is not there".into()),
            Probe::Foreign { why, .. } => {
                Err(format!("the pipe's server is not hideout.exe: {why}"))
            }
        }
    }

    /// Hands the app's devices back and ends the process, as the tray's Quit does.
    fn finish(why: &str, code: i32) -> ! {
        crate::foxglove::quit::hand_back(why);
        std::process::exit(code)
    }

    fn serve(
        launch: &Launch,
        conn: ChannelConn,
        generation: u64,
        tx: &Sender<Msg>,
        rx: &Receiver<Msg>,
        facts: &foxglove_devices::facts::Facts,
    ) -> Ended {
        // Changes that piled up while no hub was there are stale: the snapshot says it all.
        for _ in rx.try_iter() {}
        let reader = conn.clone();
        let ltx = tx.clone();
        crate::foxglove::services::spawn_named("hub-link-read", move || {
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
                            return;
                        }
                    }
                }
            }
            let _ = ltx.send(Msg::Closed(generation));
        });
        let mut link = Link::new(conn, launch.clone());
        let lost = |e: SessionError| Ended::Lost(e.to_string());
        if let Err(e) = link.start() {
            return lost(e);
        }
        loop {
            let Ok(msg) = rx.recv() else {
                return Ended::Lost("the link's queue closed".into());
            };
            let step = match msg {
                Msg::Line(g, line) if g == generation => match link.line(&line) {
                    Ok(step) => step,
                    Err(e) => return lost(e),
                },
                Msg::Closed(g) if g == generation => {
                    return Ended::Lost("the hub closed the pipe".into())
                }
                Msg::Fact(change) => {
                    if let Err(e) = link.change(&change) {
                        return lost(e);
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
                Step::Exit => return Ended::Exit,
                Step::Refused(why) => return Ended::Refused(why),
                Step::Ready => {
                    crate::foxglove::app_log::line("Foxglove: registered with the hub as a module");
                    if let Err(e) = link.snapshot(facts.list(FACTS)) {
                        return lost(e);
                    }
                }
            }
        }
    }
}
