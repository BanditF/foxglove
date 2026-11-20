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
    check_server, open_pipe, Handler, Incoming, InitializeModule, Launch, ModuleRegister, Session,
};
use foxglove_sensors::host::{registered_pattern, Host, Write, GAME_MODE_FACT};
use foxglove_sensors::{nvml, pdh, Config, Source, SourceFactory};
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
}

impl Handler for Inbox {
    fn notification(&mut self, method: &str, params: Value) {
        self.notes.push((method.into(), params));
    }
    fn exit(&mut self) {
        self.exit = true;
    }
}

type Pipe = Session<std::io::Empty, std::fs::File>;

fn main() {
    if let Err(e) = run() {
        eprintln!("{ID}: {e}");
        std::process::exit(1);
    }
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
    let pipe = open_pipe(&launch.pipe)?;
    // Nothing is sent, the token least of all, until the pipe is the hub's own (§7.5).
    if let Some(hub) = std::env::var("HIDEOUT_HUB_PID")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
    {
        check_server(&pipe, hub)?;
    }
    let reader = BufReader::new(pipe.try_clone()?);
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
    session.ready(&launch.instance)?;

    // Game mode is optional: without the grant the module reads at the normal rate.
    let id = session.request("facts.get", json!({"key": GAME_MODE_FACT}))?;
    let game_mode_get = id;
    session.request("facts.subscribe", json!({"pattern": GAME_MODE_FACT}))?;

    let factories: Vec<SourceFactory> = vec![
        Box::new(|| Box::new(pdh::PdhSource::new()) as Box<dyn Source>),
        Box::new(|| nvml::source_or_unavailable("nvml.dll")),
    ];
    let mut host = Host::new(factories, Config::default());
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
                if let Incoming::Response {
                    id,
                    method,
                    outcome,
                } = session.handle_line(&line, &mut inbox)?
                {
                    match outcome {
                        Ok(v) if id == game_mode_get => game_mode = Some(is_on(&v["value"])),
                        Ok(_) => {}
                        // Without the game-mode grant the module reads at the normal rate.
                        Err(e) => {
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
            match method.as_str() {
                "facts.demand" => {
                    let n = p["subscribers"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
                    host.demand(p["pattern"].as_str().unwrap_or(""), n, now);
                }
                "facts.changed" if p["key"] == GAME_MODE_FACT => {
                    host.game_mode(is_on(&p["value"]), now)
                }
                _ => {}
            }
        }
        host.tick(now);
        for w in host.take_writes() {
            session.set_fact(params(&w))?;
        }
        if inbox.exit {
            break;
        }
    }
    host.stop(start.elapsed());
    for w in host.take_writes() {
        // The hub is going or gone: a failed write ends nothing more.
        let _ = session.set_fact(params(&w));
    }
    Ok(())
}
