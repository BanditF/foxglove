// SPDX-License-Identifier: MIT
//! A test module for tests/appcontainer.rs, never shipped, never windowed. It writes what it
//! saw on stderr (its log).
//!
//! - `hideout-ac-probe <module id> [<file outside its folder> | -] [--exit]`: connects to
//!   `HIDEOUT_PIPE` the way every module does (the SDK's `connect_env`), says `initialize` and
//!   `module.ready`, and says whether it could read the file, one outside its folder. Then it
//!   waits for the hub to end its job (60 s at most), or with `--exit` exits 0 at once. It
//!   exits 3 when the pipe can't be opened.
//! - `--provide <module id>`: the same, offering the channel `c` (#1092): each `channel.create`
//!   is served with the SDK's `serve_create`, and the consumer gets one frame, `from <id>`.
//! - `--consume <module id> <provider>/<channel>`: the same, then `channel.open` on the
//!   provider's channel, `connect_to` with the grant's pid, and one frame read: it says
//!   `probe: channel frame: <text>` and exits 0, or says why and exits 5.
//! - `--stdio <file outside its folder>`: a bridged plugin (the Foxglove SDK's
//!   `Plugin::connect` over stdin and stdout, §8.2): says whether it could read the file and
//!   write `data\probe.txt`, then waits for the hub (60 s at most).

use std::time::Duration;

use foxglove_plugin::channel::{self, ChannelConn};
use foxglove_plugin::hideout::{
    connect_env, ChannelCreate, Handler, InitializeModule, ModuleRegister, RpcError,
};
use serde_json::{json, Value};

/// The channel `--provide` offers.
const CHANNEL: &str = "c";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    match arg(1).as_str() {
        "--provide" => module(&arg(2), Mode::Provide),
        "--consume" => module(&arg(2), Mode::Consume(arg(3))),
        "--stdio" => stdio(&arg(2)),
        _ => {
            if let Some(p) = args.get(2).filter(|p| *p != "-") {
                outside(p);
            }
            let exit = args.iter().any(|a| a == "--exit");
            module(&arg(1), if exit { Mode::Exit } else { Mode::Wait });
        }
    }
}

enum Mode {
    Wait,
    Exit,
    Provide,
    Consume(String),
}

/// Says whether `path`, outside the module's folder, could be read.
fn outside(path: &str) {
    let what = match std::fs::read(path) {
        Ok(_) => "read".to_string(),
        Err(e) => format!("refused ({:?})", e.kind()),
    };
    eprintln!("probe: outside file: {what}");
}

struct Provider(String);

impl Handler for Provider {
    fn channel_create(&mut self, c: ChannelCreate) -> Result<Value, RpcError> {
        let frame = format!("from {}", self.0);
        let pipe = channel::serve_create(&self.0, &c, move |mut conn: ChannelConn| {
            let _ = channel::write_frame(&mut conn, frame.as_bytes());
            // Until the consumer hangs up.
            let _ = channel::read_frame(&mut conn);
        })
        .map_err(|e| {
            eprintln!("probe: channel.create: {e}");
            RpcError::new(6, e.to_string())
        })?;
        eprintln!("probe: serving {pipe}");
        Ok(json!({ "pipe": pipe }))
    }
}

fn module(id: &str, mode: Mode) {
    let (mut s, launch) = match connect_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("probe: connect failed: {e}");
            std::process::exit(3);
        }
    };
    if matches!(mode, Mode::Wait | Mode::Exit) {
        // Said without waiting for answers: the pipe test's hub knows no module.
        let said = s
            .request(
                "initialize",
                json!({"id": id, "version": "0.1.0", "instance": launch.instance,
                       "token": launch.token, "hub": "1.0", "api": {}}),
            )
            .and_then(|_| s.notify("module.ready", json!({})));
        match said {
            Ok(()) if matches!(mode, Mode::Exit) => std::process::exit(0),
            Ok(()) => eprintln!("probe: ready sent"),
            Err(e) => {
                eprintln!("probe: write failed: {e:?}");
                std::process::exit(4);
            }
        }
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    let mut handler = Provider(id.to_string());
    let init = InitializeModule {
        id: id.into(),
        version: "0.1.0".into(),
        instance: launch.instance.clone(),
        token: launch.token.clone(),
        hub: "1.0".into(),
        api: Default::default(),
    };
    let said = s.call_initialize(&init, &mut handler).map(|_| ());
    let said = said.and_then(|()| match mode {
        Mode::Provide => s
            .call_register(
                &ModuleRegister {
                    channels: vec![CHANNEL.into()],
                    ..Default::default()
                },
                &mut handler,
            )
            .map(|_| ()),
        _ => Ok(()),
    });
    if let Err(e) = said.and_then(|()| s.ready(&launch.instance)) {
        eprintln!("probe: write failed: {e:?}");
        std::process::exit(4);
    }
    match mode {
        Mode::Wait | Mode::Exit => {}
        Mode::Provide => {
            eprintln!("probe: ready sent");
            let _ = s.run(&mut handler);
        }
        Mode::Consume(target) => {
            let (module, chan) = target.split_once('/').unwrap_or((&target, ""));
            let got = s
                .call(
                    "channel.open",
                    json!({"module": module, "channel": chan}),
                    &mut handler,
                )
                .map_err(|e| format!("channel.open: {e:?}"))
                .and_then(|g| {
                    let pid = g["pid"].as_u64().map(|p| p as u32);
                    let pipe = g["pipe"].as_str().unwrap_or_default();
                    eprintln!("probe: granted {pipe} (pid {pid:?})");
                    let mut c = channel::connect_to(pipe, g["nonce"].as_str().unwrap_or(""), pid)
                        .map_err(|e| format!("connect: {e}"))?;
                    channel::read_frame(&mut c)
                        .map_err(|e| format!("read: {e}"))?
                        .ok_or_else(|| "closed before a frame".to_string())
                });
            match got {
                Ok(frame) => {
                    eprintln!("probe: channel frame: {}", String::from_utf8_lossy(&frame));
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("probe: channel failed: {e}");
                    std::process::exit(5);
                }
            }
        }
    }
}

/// A bridged plugin over stdin and stdout.
fn stdio(outside_file: &str) {
    // Both before connecting: once the hub has it ready, its log says them.
    outside(outside_file);
    let data = foxglove_plugin::Plugin::data_dir().join("probe.txt");
    match std::fs::write(&data, "the module's own") {
        Ok(()) => eprintln!("probe: data write: ok"),
        Err(e) => eprintln!("probe: data write: refused ({:?})", e.kind()),
    }
    let plugin = match foxglove_plugin::Plugin::connect("ac-probe", "0.1.0") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("probe: stdio connect failed: {e}");
            std::process::exit(3);
        }
    };
    eprintln!("probe: stdio ready");
    std::thread::sleep(Duration::from_secs(60));
    drop(plugin);
}
