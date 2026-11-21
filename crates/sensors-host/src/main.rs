//! `sensors.exe`: the PC sensors as a Hideout module (module.toml). The hub starts it with
//! `HIDEOUT_PIPE`, `HIDEOUT_TOKEN` and `HIDEOUT_INSTANCE`; it registers the facts `sensor.*`
//! and then does nothing until the hub says someone listens (`facts.demand`, contract §8.8,
//! v1.20): no timer, no counters, no NVML. While someone listens it reads once a second
//! (every 10 s during a game, `keyboard.game_mode`) and publishes with `facts.set`.
//!
//! One thread reads the pipe and hands each line to the loop; the loop sleeps until the next
//! read is due or a line arrives. All the rules are in [`foxglove_sensors::host`].

use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;

use foxglove_plugin::hideout::{
    connect_pipe, Handler, Incoming, InitializeModule, Launch, ModulePipe, ModuleRegister, PatchOp,
    Register, RpcError, Session, UiEvent, UiPatch, MODULE_GRANTS,
};
use foxglove_sensors::host::{registered_pattern, Host, Write, GAME_MODE_FACT};
use foxglove_sensors::ui::{self, PageInput, PanelsOut, PanelsView};
use foxglove_sensors::{nvml, pdh, thermal, Config, Source, SourceFactory};
use serde_json::{json, Value};

/// A write as the `facts.set` params; a withdrawal is a `null` value.
fn params(w: &Write) -> Value {
    let value = match &w.value {
        None => Value::Null,
        Some(foxglove_sensors::ValueOut::Int(n)) => json!(n),
        Some(foxglove_sensors::ValueOut::Text(t)) => json!(t),
    };
    json!({"key": w.key, "value": value})
}

/// `facts.get` answers `{value}`, `facts.changed` carries `{key, value}`: on only for `true`.
fn is_on(v: &Value) -> bool {
    v.as_bool() == Some(true)
}

const ID: &str = "org.foxglove.sensors";
/// The hub that sends `facts.demand` (§8.7).
const HUB: &str = "1.5";

/// What the hub sent that is not an answer: notifications for [`Host`], and the exit.
#[derive(Default)]
struct Inbox {
    notes: Vec<(String, Value)>,
    exit: bool,
    /// Try again was clicked on the page.
    retry: bool,
}

impl Handler for Inbox {
    fn notification(&mut self, method: &str, params: Value) {
        self.notes.push((method.into(), params));
    }
    fn ui_event(&mut self, event: UiEvent) -> Result<Value, RpcError> {
        if event.node == ui::RETRY {
            self.retry = true;
        }
        Ok(json!({}))
    }
    fn exit(&mut self) {
        self.exit = true;
    }
}

type Pipe = Session<std::io::Empty, ModulePipe>;

fn main() {
    if let Err(e) = run() {
        eprintln!("{ID}: {e}");
        std::process::exit(1);
    }
}

/// One `ui.patch` of `panel`.
fn patch(
    session: &mut Pipe,
    panel: &str,
    ops: Vec<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let ops: Vec<PatchOp> = ops
        .into_iter()
        .filter_map(|o| serde_json::from_value(o).ok())
        .collect();
    session.ui_patch(&UiPatch {
        panel: panel.into(),
        ops,
    })?;
    Ok(())
}

/// Waits for the answer to request `id`; everything else that arrives is kept for the loop.
fn answer(
    session: &mut Pipe,
    lines: &mpsc::Receiver<String>,
    inbox: &mut Inbox,
    id: i64,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let line = lines.recv().map_err(|_| "the hub closed the pipe")?;
        if let Incoming::Response {
            id: got, outcome, ..
        } = session.handle_line(&line, inbox)?
        {
            if got == id {
                return Ok(outcome.map_err(|e| format!("{} ({})", e.message, e.code))?);
            }
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let launch = Launch::from_env().ok_or(
        "HIDEOUT_PIPE, HIDEOUT_TOKEN and HIDEOUT_INSTANCE are not set: the hub starts modules",
    )?;
    // Overlapped, so the reader thread's blocked read never holds up a write (#1347).
    let (read, pipe) = connect_pipe(&launch.pipe)?;
    let reader = BufReader::new(read);
    let (tx, lines) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if !line.trim().is_empty() && tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut session: Pipe = Session::new(std::io::empty(), pipe);
    let mut inbox = Inbox::default();

    let id = session.initialize(&InitializeModule {
        id: ID.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        instance: launch.instance.clone(),
        token: launch.token.clone(),
        hub: HUB.into(),
        api: Default::default(),
    })?;
    answer(&mut session, &lines, &mut inbox, id)?;
    let id = session.register(&ModuleRegister {
        facts: vec![registered_pattern()],
        ..Default::default()
    })?;
    let result = answer(&mut session, &lines, &mut inbox, id)?;
    if result["rejected"].as_array().is_some_and(|r| !r.is_empty()) {
        return Err(format!("the hub refused {}", result["rejected"]).into());
    }
    // The page (contract order: module.register, ui.register, module.ready). It starts as all
    // "Reading…": nothing has been asked of the sensors yet.
    let mut view = PanelsView::new();
    let first = PageInput {
        game_mode_visible: true,
        ..PageInput::default()
    };
    for out in view.next(ui::tile(&first), ui::page(&first)) {
        if let PanelsOut::Register(panels) = out {
            let id = session.ui_register(&Register {
                instance: launch.instance.clone(),
                panels,
            })?;
            let result = answer(&mut session, &lines, &mut inbox, id)?;
            if result["rejected"].as_array().is_some_and(|r| !r.is_empty()) {
                return Err(format!("the hub refused the panels {}", result["rejected"]).into());
            }
        }
    }
    session.ready(&launch.instance)?;

    // Game mode is optional: without the grant the module reads at the normal rate.
    let id = session.request("facts.get", json!({"key": GAME_MODE_FACT}))?;
    let mut game_mode_get = id;
    // Denied before the user's Allow: the session asks again by itself on `module.grants`.
    session.subscribe(GAME_MODE_FACT)?;

    let factories: Vec<SourceFactory> = vec![
        Box::new(|| Box::new(pdh::PdhSource::new()) as Box<dyn Source>),
        Box::new(|| nvml::source_or_unavailable("nvml.dll")),
        Box::new(|| Box::new(thermal::ThermalSource::new()) as Box<dyn Source>),
    ];
    let mut host = Host::new(factories, Config::default());
    host.on_release(foxglove_sensors::trim_memory);
    let start = Instant::now();
    let mut game_mode = None;
    let mut logged = std::collections::HashSet::new();
    loop {
        let now = start.elapsed();
        let line = match host.next_due() {
            Some(due) => lines.recv_timeout(due.saturating_sub(now)),
            None => lines.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match line {
            Ok(line) => {
                let incoming = session.handle_line(&line, &mut inbox)?;
                if matches!(&incoming, Incoming::Notified { method } if method == MODULE_GRANTS) {
                    // The game-mode grant may have come (the user's Allow) or gone (a
                    // turn-off): read the value as it is now. The session subscribes again by
                    // itself after an Allow.
                    game_mode_get = session.request("facts.get", json!({"key": GAME_MODE_FACT}))?;
                }
                if let Incoming::Response {
                    id,
                    method,
                    outcome,
                } = incoming
                {
                    match outcome {
                        Ok(v) if id == game_mode_get => {
                            host.set_game_mode_visible(true);
                            game_mode = Some(is_on(&v["value"]));
                        }
                        Ok(_) => {}
                        // Without the game-mode grant the module reads at the normal rate.
                        Err(e) => {
                            if id == game_mode_get {
                                host.set_game_mode_visible(false);
                            }
                            // Once per method: a revoked grant must not flood the log.
                            if logged.insert(method.clone()) {
                                eprintln!("{ID}: {method}: {} ({})", e.message, e.code);
                            }
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = start.elapsed();
        if let Some(on) = game_mode.take() {
            host.game_mode(on, now);
        }
        for (method, p) in std::mem::take(&mut inbox.notes) {
            host.notify(&method, &p, now);
        }
        if std::mem::take(&mut inbox.retry) {
            host.retry(now);
        }
        host.tick(now);
        for w in host.take_writes() {
            session.set_fact(params(&w))?;
        }
        for out in view.next(host.tile_tree(), host.page_tree()) {
            match out {
                PanelsOut::Register(panels) => {
                    session.ui_register(&Register {
                        instance: launch.instance.clone(),
                        panels,
                    })?;
                }
                PanelsOut::Patch { panel, ops } => patch(&mut session, panel, ops)?,
            }
        }
        if inbox.exit {
            break;
        }
    }
    host.stop(start.elapsed());
    for out in view.next(host.tile_tree(), host.page_tree()) {
        if let PanelsOut::Patch { panel, ops } = out {
            // The hub is going or gone: a failed patch ends nothing more.
            let _ = patch(&mut session, panel, ops);
        }
    }
    for w in host.take_writes() {
        // The hub is going or gone: a failed write ends nothing more.
        let _ = session.set_fact(params(&w));
    }
    Ok(())
}
