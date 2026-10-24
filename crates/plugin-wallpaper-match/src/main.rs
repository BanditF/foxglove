//! Wallpaper Match (#70), a Foxglove plugin: the keyboard's underglow takes the Bandit
//! wallpaper's accent colour.
//!
//! The wallpaper (bandit-wallpapers `shared/match.js`) connects to `ws://127.0.0.1:8767` with
//! the pairing token and sends `{v: 1, wallpaperId, theme, accent, palette, paused}` when it
//! opens and when that changes. Foxglove accepts the connection for the plugin
//! (`net.listen:tcp:127.0.0.1:8767`), so it stays sandboxed.
//!
//! - **Who may connect:** a page on this PC with the pairing token (a setting Foxglove fills
//!   with a random value; the user pastes it into the wallpaper), and, if the "origins" setting
//!   names any, only those page origins. Until Wallpaper Engine's origin is known (#320), the
//!   token is the check.
//! - **Lights:** a low-priority lease in the accent colour, so notifications still show. It's
//!   changed in place when the colour changes, and taken afresh for another wallpaper.
//! - **Given back** when the wallpaper pauses, its connection closes, the keyboard disconnects,
//!   or the plugin stops. Several wallpapers (one per monitor): the last one that spoke and
//!   isn't paused wins.
//! - Nothing polls: everything waits on a connection, a message or a fact change.

mod matcher;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use foxglove_plugin::ws::{self, Message as Ws};
use foxglove_plugin::{Fact, Hsb, Lease, LeasePriority, Plugin, TcpStream};
use matcher::{Matcher, Message};

/// The wallpaper's default port; `--port N` for tests (the manifest's permission must match).
const PORT: u16 = 8767;

enum In {
    Opened(u64),
    Said(u64, Message),
    Closed(u64),
    Keyboard(bool),
}

/// Who may connect: the pairing token and, when set, the allowed origins.
#[derive(Clone, Default)]
struct Access {
    token: String,
    origins: Vec<String>,
}

impl Access {
    fn from(values: &serde_json::Map<String, serde_json::Value>) -> Access {
        let text = |k: &str| values.get(k).and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
        let origins = text("origins").split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
        Access { token: text("pairing-token"), origins }
    }
}

/// One wallpaper's connection: the handshake, then its messages until it closes.
fn serve(mut stream: TcpStream, conn: u64, access: Access, tx: mpsc::Sender<In>) {
    let shake = match ws::read_handshake(&mut stream) {
        Ok(h) => h,
        Err(_) => return,
    };
    let origins: Vec<&str> = access.origins.iter().map(String::as_str).collect();
    // Without a token nothing is accepted: a page can't reach the lights by guessing.
    let allowed = if access.token.len() < 16 { Err("no pairing token set".to_string()) } else { shake.check(&origins, Some(&access.token)) };
    if let Err(why) = allowed {
        eprintln!("wallpaper-match: refused a connection: {why}");
        let _ = ws::reject(&mut stream, 403, "Forbidden");
        return;
    }
    if ws::accept(&mut stream, &shake, None).is_err() || tx.send(In::Opened(conn)).is_err() {
        return;
    }
    let mut reader = ws::Reader::default();
    loop {
        match reader.read(&mut stream) {
            Ok(Ws::Text(text)) => match serde_json::from_str::<Message>(&text) {
                Ok(m) => {
                    if tx.send(In::Said(conn, m)).is_err() {
                        return;
                    }
                }
                Err(e) => eprintln!("wallpaper-match: a message it doesn't understand: {e}"),
            },
            Ok(Ws::Ping(p)) => {
                let _ = ws::write_frame(&mut stream, ws::PONG, &p);
            }
            Ok(Ws::Close(_)) => {
                let _ = ws::write_frame(&mut stream, ws::CLOSE, &[]);
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = tx.send(In::Closed(conn));
}

/// The lights: one low-priority lease per keyboard, for one wallpaper.
struct Lights {
    held: Option<(String, Hsb, Vec<Lease>)>,
}

impl Lights {
    fn show(&mut self, plugin: &Arc<Plugin>, wanted: Option<(String, Hsb)>) {
        match (&self.held, wanted) {
            (_, None) => self.held = None,
            (Some((w, c, leases)), Some((wanted_w, color))) if *w == wanted_w => {
                if *c != color {
                    for l in leases {
                        if let Err(e) = l.set(color) {
                            eprintln!("wallpaper-match: lighting: {e}");
                        }
                    }
                    if let Some(h) = self.held.as_mut() {
                        h.1 = color;
                    }
                }
            }
            // Another wallpaper (or the first): give the lights back, then take them for it.
            (_, Some((wallpaper, color))) => {
                self.held = None;
                let leases = plugin
                    .devices()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|d| d.connected && d.caps.iter().any(|c| c == "lighting"))
                    .filter_map(|d| plugin.lease(&d.id, color, LeasePriority::Low).map_err(|e| eprintln!("wallpaper-match: lighting: {e}")).ok())
                    .collect();
                self.held = Some((wallpaper, color, leases));
            }
        }
    }
}

fn main() {
    let plugin: Arc<Plugin> = match Plugin::connect("wallpaper-match", env!("CARGO_PKG_VERSION")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("wallpaper-match: can't reach Foxglove: {e}");
            std::process::exit(1);
        }
    };
    let args: Vec<String> = std::env::args().collect();
    let port = args.windows(2).find(|w| w[0] == "--port").and_then(|w| w[1].parse().ok()).unwrap_or(PORT);
    let access = Arc::new(Mutex::new(Access::from(&plugin.settings().unwrap_or_default())));
    let listener = match plugin.listen_tcp(port) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("wallpaper-match: can't listen on 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        }
    };
    let (tx, rx) = mpsc::channel::<In>();

    // Connections: each wallpaper on its own thread.
    let (accept_tx, accept_access) = (tx.clone(), access.clone());
    std::thread::spawn(move || {
        let mut next = 0u64;
        while let Some((stream, _from)) = listener.accept() {
            next += 1;
            let (conn, tx) = (next, accept_tx.clone());
            let access = accept_access.lock().unwrap_or_else(|e| e.into_inner()).clone();
            std::thread::spawn(move || serve(stream, conn, access, tx));
        }
    });
    // The user changing the token or origins: new connections use them.
    if let Some(changes) = plugin.settings_changes() {
        let access = access.clone();
        std::thread::spawn(move || {
            for values in changes {
                *access.lock().unwrap_or_else(|e| e.into_inner()) = Access::from(&values);
            }
        });
    }
    // The keyboard disconnecting gives its lights back; reconnecting takes them again.
    if plugin.subscribe("keyboard.connected").is_ok() {
        if let Some(events) = plugin.events() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for e in events {
                    if e.key == "keyboard.connected" {
                        let connected = !matches!(e.value, Some(Fact::Bool(false)));
                        if tx.send(In::Keyboard(connected)).is_err() {
                            return;
                        }
                    }
                }
            });
        }
    }
    drop(tx);

    let mut matcher = Matcher::default();
    let mut lights = Lights { held: None };
    let mut keyboard = true;
    // Ends when Foxglove goes away (every sender's thread ends with it).
    for event in rx {
        if plugin.is_closed() {
            return;
        }
        match event {
            In::Opened(c) => matcher.opened(c),
            In::Said(c, m) => matcher.message(c, &m),
            In::Closed(c) => matcher.closed(c),
            In::Keyboard(k) => {
                keyboard = k;
                if !k {
                    lights.held = None;
                }
            }
        }
        lights.show(&plugin, matcher.wanted().filter(|_| keyboard));
    }
}
