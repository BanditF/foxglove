//! Mic mute (#210), a Foxglove plugin: shows whether the microphone is muted, on the left panel
//! and the underglow, and mutes or unmutes it from a key.
//!
//! Foxglove does the Core Audio work (#237): this plugin reads `mic.state` (`audio.inputs.read`)
//! and asks for changes with `audio.inputs.set_mute` (`audio.inputs.mute`), so it stays in the
//! sandbox. It's about the default communications microphone's own mute, Windows' mute, which
//! isn't a privacy guarantee: apps can mute themselves, and exclusive-mode streams bypass it.
//!
//! - **Muted:** red underglow (a high-priority lease) and Art's inverted MUTED screen, held for
//!   as long as it lasts.
//! - **Unmuted:** a brief white flash, then the lights go back; LIVE shows for a few seconds.
//! - **Unknown** (couldn't be read): amber, and the `?` screen for a few seconds.
//! - **Unavailable** (no microphone): nothing held.
//! - **Keys** (bind them in the Plugins tab): `toggle`, `mute`, `unmute`.
//!
//! Options: `--no-lights`, `--no-panel`.

mod screens;

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use foxglove_plugin::{Fact, Hsb, Lease, LeasePriority, Plugin};
use screens::{Mic, Shown};

/// What arrives: a fact change or a key.
enum In {
    State(Mic),
    Key(String),
}

fn parse(value: Option<&Fact>) -> Mic {
    match value {
        Some(Fact::Text(s)) => Mic::from_fact(s),
        _ => Mic::Unknown,
    }
}

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
        .filter_map(|(id, _, _)| plugin.lease(&id, color, priority).map_err(|e| eprintln!("mic-mute: lighting: {e}")).ok())
        .collect()
}

/// What's shown: the held lights, and until when the screen and the white flash last.
struct Out {
    lights: bool,
    art: Option<screens::Art>,
    leases: Vec<Lease>,
    screen_until: Option<Instant>,
    flash_until: Option<Instant>,
}

impl Out {
    fn show(&mut self, plugin: &Arc<Plugin>, mic: Mic, changed: bool) {
        let shown = Shown::of(mic);
        if self.lights {
            self.leases.clear();
            if let Some((color, priority)) = shown.light {
                self.leases = lease_all(plugin, color, priority);
            } else if changed && mic == Mic::Unmuted {
                // A brief white acknowledgement, then the lights go back.
                self.leases = lease_all(plugin, screens::WHITE, LeasePriority::Normal);
                self.flash_until = Some(Instant::now() + screens::FLASH);
            }
        }
        self.screen_until = None;
        if let (Some(art), Some(frame)) = (&self.art, shown.frame) {
            let ttl = if shown.hold { screens::HOLD_TTL } else { screens::BRIEF };
            for (id, _, _) in keyboards(plugin).into_iter().filter(|k| k.2) {
                if let Err(e) = plugin.panel_override(&id, screens::W, screens::H, art.frame(frame), ttl) {
                    eprintln!("mic-mute: panel: {e}");
                }
            }
            // A held screen is sent again before its lifetime runs out.
            self.screen_until = shown.hold.then(|| Instant::now() + screens::HOLD_TTL / 2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lights = !args.iter().any(|a| a == "--no-lights");
    let panel = !args.iter().any(|a| a == "--no-panel");
    let plugin: Arc<Plugin> = match Plugin::connect("mic-mute", env!("CARGO_PKG_VERSION")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mic-mute: can't reach Foxglove: {e}");
            std::process::exit(1);
        }
    };
    let art = if panel { screens::Art::load().map_err(|e| eprintln!("mic-mute: panel art: {e}")).ok() } else { None };

    // Fact changes and keys, into one queue; nothing polls.
    let (tx, rx) = mpsc::channel::<In>();
    if let Err(e) = plugin.subscribe("mic.state") {
        eprintln!("mic-mute: can't follow the microphone: {e}");
    }
    if let Some(events) = plugin.events() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for e in events {
                if e.key == "mic.state" && tx.send(In::State(parse(e.value.as_ref()))).is_err() {
                    return;
                }
            }
        });
    }
    if let Some(keys) = plugin.actions() {
        std::thread::spawn(move || {
            for k in keys {
                if tx.send(In::Key(k)).is_err() {
                    return;
                }
            }
        });
    }

    let mut mic = parse(plugin.fact("mic.state").ok().flatten().as_ref());
    let mut out = Out { lights, art, leases: Vec::new(), screen_until: None, flash_until: None };
    out.show(&plugin, mic, false);

    loop {
        if plugin.is_closed() {
            return;
        }
        // Wake for the next fact or key, or to end the flash, or to re-send a held screen
        // before its lifetime runs out.
        let next = [out.screen_until, out.flash_until].into_iter().flatten().min();
        let got = match next {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            },
        };
        match got {
            Some(In::State(m)) => {
                let changed = m != mic;
                mic = m;
                out.flash_until = None;
                out.show(&plugin, mic, changed);
            }
            Some(In::Key(k)) => {
                let muted = match k.as_str() {
                    "toggle" => mic != Mic::Muted,
                    "mute" => true,
                    "unmute" => false,
                    _ => continue,
                };
                if let Err(e) = plugin.call("audio.inputs.set_mute", serde_json::json!({ "muted": muted })) {
                    eprintln!("mic-mute: {e}");
                }
                // The new state arrives as a fact change.
            }
            None => {
                let now = Instant::now();
                if out.flash_until.is_some_and(|t| t <= now) {
                    out.flash_until = None;
                    out.leases.clear();
                }
                if out.screen_until.is_some_and(|t| t <= now) {
                    out.show(&plugin, mic, false);
                }
            }
        }
    }
}
