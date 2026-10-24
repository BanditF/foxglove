//! Broadcast Status (#211), a Foxglove plugin: whether OBS is recording or streaming, and its
//! scene, on the left panel and the underglow.
//!
//! It connects to OBS's WebSocket server (obs-websocket v5, `ws://127.0.0.1:4455`) through
//! Foxglove (`net.connect:127.0.0.1:4455`), so it stays in the sandbox, and the password comes
//! from Windows' credential store (`secrets`, set in the Plugins tab). It only listens: a
//! snapshot when it connects, then OBS's events. It never starts, stops or switches anything.
//!
//! - **Streaming:** red underglow (a high-priority lease) and the bright LIVE screen, held.
//! - **Recording:** red (normal priority) and REC, held; amber and the pause bars while paused.
//! - **Both:** red (high) and the LIVE + REC screen.
//! - **Nothing running:** READY for a few seconds; the lights are left alone.
//! - **OBS gone:** NOT CONNECTED for a few seconds, then it tries again (2 s, doubling to 30 s).
//! - **Facts:** `broadcast.connected`, `broadcast.streaming`, `broadcast.recording`,
//!   `broadcast.paused`, `broadcast.scene`.
//!
//! The scene name goes on the panel under the symbol. Options: `--port N` (4455; the manifest's
//! permission has to match), `--no-lights`, `--no-panel`.

mod obs;
mod screens;
mod ws;

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use foxglove_plugin::{Fact, Hsb, Lease, LeasePriority, NetEvent, NetKind, Plugin};
use obs::Status;
use screens::State;

const HOST: &str = "127.0.0.1";
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// The keyboards: (id, has lights, has a left panel).
fn keyboards(plugin: &Plugin) -> Vec<(String, bool, bool)> {
    plugin
        .devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.connected)
        .map(|d| {
            let has = |c: &str| d.caps.iter().any(|x| x == c);
            (d.id.clone(), has("lighting"), has("screens"))
        })
        .collect()
}

fn lease_all(plugin: &Arc<Plugin>, color: Hsb, priority: LeasePriority) -> Vec<Lease> {
    keyboards(plugin)
        .into_iter()
        .filter(|k| k.1)
        .filter_map(|(id, _, _)| plugin.lease(&id, color, priority).map_err(|e| eprintln!("broadcast-status: lighting: {e}")).ok())
        .collect()
}

/// What's shown: the state and scene, the held lights, and when a held screen is re-sent.
struct Out {
    lights: bool,
    art: Option<screens::Art>,
    leases: Vec<Lease>,
    shown: Option<(State, Option<String>)>,
    screen_until: Option<Instant>,
}

impl Out {
    fn show(&mut self, plugin: &Arc<Plugin>, connected: bool, status: &Status) {
        let state = State::of(connected, status);
        let now = (state, status.scene.clone().filter(|_| connected));
        let before = self.shown.replace(now.clone());
        if before.as_ref() == Some(&now) {
            return;
        }
        publish(plugin, connected, status);
        let light_before = before.as_ref().and_then(|b| b.0.light());
        if self.lights && light_before != state.light() {
            self.leases.clear();
            if let Some((color, priority)) = state.light() {
                self.leases = lease_all(plugin, color, priority);
            }
        }
        // NOT CONNECTED shows only when a connection is lost, not while OBS isn't running.
        let lost = state == State::Disconnected && before.is_some_and(|b| b.0 != State::Disconnected);
        if state != State::Disconnected || lost {
            self.send_screen(plugin);
        } else {
            self.screen_until = None;
        }
    }

    fn send_screen(&mut self, plugin: &Arc<Plugin>) {
        self.screen_until = None;
        let (Some(art), Some((state, scene))) = (&self.art, &self.shown) else { return };
        let frame = art.frame(*state, scene.as_deref());
        let ttl = if state.hold() { screens::HOLD_TTL } else { screens::BRIEF };
        for (id, _, _) in keyboards(plugin).into_iter().filter(|k| k.2) {
            if let Err(e) = plugin.panel_override(&id, screens::W, screens::H, &frame, ttl) {
                eprintln!("broadcast-status: panel: {e}");
            }
        }
        // A held screen is sent again before its lifetime runs out.
        self.screen_until = state.hold().then(|| Instant::now() + screens::HOLD_TTL / 2);
    }
}

fn publish(plugin: &Plugin, connected: bool, s: &Status) {
    let _ = plugin.set_fact("broadcast.connected", Some(Fact::Bool(connected)));
    let _ = plugin.set_fact("broadcast.streaming", Some(Fact::Bool(connected && s.streaming)));
    let _ = plugin.set_fact("broadcast.recording", Some(Fact::Bool(connected && s.recording)));
    let _ = plugin.set_fact("broadcast.paused", Some(Fact::Bool(connected && s.paused)));
    let _ = plugin.set_fact("broadcast.scene", s.scene.clone().filter(|_| connected).map(Fact::Text));
}

/// One connection, until it ends: why it ended, and whether it got as far as OBS's status.
fn session(plugin: &Arc<Plugin>, out: &mut Out, stream: u64, rx: Receiver<NetEvent>, port: u16) -> (String, bool) {
    let send = |bytes: Vec<u8>| plugin.net_send(stream, &bytes).map_err(|e| e.to_string());
    if let Err(e) = send(ws::upgrade_request(HOST, port).into_bytes()) {
        return (e, false);
    }
    let password = plugin.secret("obs-password").map_err(|e| eprintln!("broadcast-status: the password: {e}")).ok().flatten();
    let mut obs = obs::Session::new(password);
    let mut decoder = ws::Decoder::default();
    loop {
        let event = match out.screen_until {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let bytes = match event {
            Ok(NetEvent::Data(bytes, _)) => bytes,
            Ok(NetEvent::Closed(why)) => return (format!("the connection closed: {why}"), obs.ready()),
            Err(RecvTimeoutError::Timeout) => {
                out.send_screen(plugin);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return ("Foxglove went away".into(), obs.ready()),
        };
        let messages = match decoder.feed(&bytes) {
            Ok(m) => m,
            Err(e) => return (e, obs.ready()),
        };
        for m in messages {
            let sent = match m {
                ws::Msg::Text(text) => match obs.on_message(&text) {
                    Ok(replies) => replies.iter().try_for_each(|r| send(ws::frame(ws::TEXT, r.to_string().as_bytes()))),
                    Err(e) => return (e, obs.ready()),
                },
                ws::Msg::Ping(p) => send(ws::frame(ws::PONG, &p)),
                ws::Msg::Close(code) => {
                    let _ = send(ws::frame(ws::CLOSE, &[]));
                    return (obs::closed(code), obs.ready());
                }
            };
            if let Err(e) = sent {
                return (e, obs.ready());
            }
        }
        if obs.ready() {
            out.show(plugin, true, &obs.status);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port = args.windows(2).find(|w| w[0] == "--port").and_then(|w| w[1].parse().ok()).unwrap_or(4455);
    let lights = !args.iter().any(|a| a == "--no-lights");
    let panel = !args.iter().any(|a| a == "--no-panel");
    let plugin: Arc<Plugin> = match Plugin::connect("broadcast-status", env!("CARGO_PKG_VERSION")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("broadcast-status: can't reach Foxglove: {e}");
            std::process::exit(1);
        }
    };
    let art = if panel { screens::Art::load().map_err(|e| eprintln!("broadcast-status: panel art: {e}")).ok() } else { None };
    let mut out = Out { lights, art, leases: Vec::new(), shown: None, screen_until: None };
    out.show(&plugin, false, &Status::default());

    let mut retry = RETRY_FIRST;
    let mut last_error = String::new();
    while !plugin.is_closed() {
        let (why, got_status) = match plugin.net_open(NetKind::Tcp, HOST, port) {
            Ok((stream, rx)) => {
                let ended = session(&plugin, &mut out, stream, rx, port);
                let _ = plugin.net_close(stream);
                ended
            }
            Err(e) => (format!("OBS isn't reachable on port {port}: {e}"), false),
        };
        out.show(&plugin, false, &Status::default());
        if got_status {
            retry = RETRY_FIRST;
        }
        if why != last_error {
            eprintln!("broadcast-status: {why}");
            last_error = why;
        }
        // OBS can't announce itself, so try again later, less often the longer it's away.
        std::thread::sleep(retry);
        retry = (retry * 2).min(RETRY_MAX);
    }
}
