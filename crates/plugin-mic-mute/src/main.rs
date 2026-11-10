//! Mic mute (#210), a Foxglove plugin: shows whether the microphone is muted, on the Info screen
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

mod module_ui;
mod screens;

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use foxglove_plugin::ui::{Panels, UiIn};
use foxglove_plugin::{Fact, Hsb, Lease, LeasePriority, PanelTarget, Plugin};
use screens::{Mic, Shown};

/// What arrives: a fact change or a key.
enum In {
    State(Mic),
    Key(String),
    /// Hideout: a tile or page button.
    Ui(UiIn),
}

fn parse(value: Option<&Fact>) -> Mic {
    match value {
        Some(Fact::Text(s)) => Mic::from_fact(s),
        _ => Mic::Unknown,
    }
}

/// The keyboards: (id, has lights, has an Info screen).
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
        .filter_map(|(id, _, _)| {
            plugin
                .lease(&id, color, priority)
                .map_err(|e| eprintln!("mic-mute: lighting: {e}"))
                .ok()
        })
        .collect()
}

/// What's shown: the held lights, whether a screen is held, and until when the white flash
/// lasts.
struct Out {
    lights: bool,
    art: Option<screens::Art>,
    leases: Vec<Lease>,
    held: bool,
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
        let Some(art) = &self.art else { return };
        // MUTED is held until the next screen replaces it or the plugin stops (#281); LIVE and
        // `?` last a few seconds; nothing gives the panel back.
        for (id, _, _) in keyboards(plugin).into_iter().filter(|k| k.2) {
            // Drawn at the info screen's own size (#560).
            let fitted = |t: &PanelTarget, frame| t.fit(art.frame(frame), screens::W, screens::H);
            let sent = match (shown.frame, plugin.info_screen_target(&id)) {
                (Some(frame), Some(t)) if shown.hold => {
                    plugin.info_screen_show(&id, &t, &fitted(&t, frame), None)
                }
                (Some(frame), Some(t)) => {
                    plugin.info_screen_show(&id, &t, &fitted(&t, frame), Some(screens::BRIEF))
                }
                (Some(_), None) => {
                    eprintln!("mic-mute: {id} has no info screen to draw on");
                    Ok(())
                }
                (None, _) if self.held => plugin.info_screen_release(&id),
                (None, _) => Ok(()),
            };
            if let Err(e) = sent {
                eprintln!("mic-mute: panel: {e}");
            }
        }
        self.held = shown.frame.is_some() && shown.hold;
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
    let art = if panel {
        screens::Art::load()
            .map_err(|e| eprintln!("mic-mute: panel art: {e}"))
            .ok()
    } else {
        None
    };

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
    if let Some(ui) = plugin.ui_inbox() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for m in ui {
                if tx.send(In::Ui(m)).is_err() {
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
    let mut out = Out {
        lights,
        art,
        leases: Vec::new(),
        held: false,
        flash_until: None,
    };
    out.show(&plugin, mic, false);
    // Hideout's tile and page (Foxglove refuses them once and they stay off).
    let mut panels = Panels::new(plugin.clone());
    let mut refused = false;
    let mut last_ask: Option<bool> = None;
    panels.show(module_ui::panels(mic, refused));
    let ask = |muted: bool| -> bool {
        match plugin.call(
            "audio.inputs.set_mute",
            serde_json::json!({ "muted": muted }),
        ) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("mic-mute: {e}");
                false
            }
        }
    };

    loop {
        if plugin.is_closed() {
            return;
        }
        // Wake for the next fact or key, or to end the flash (a one-shot deadline).
        let got = match out.flash_until {
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
                refused = false;
                out.flash_until = None;
                out.show(&plugin, mic, changed);
                panels.show(module_ui::panels(mic, refused));
            }
            Some(In::Key(k)) => {
                let muted = match k.as_str() {
                    "toggle" => mic != Mic::Muted,
                    "mute" => true,
                    "unmute" => false,
                    _ => continue,
                };
                last_ask = Some(muted);
                refused = !ask(muted);
                panels.show(module_ui::panels(mic, refused));
                // The new state arrives as a fact change.
            }
            Some(In::Ui(m)) => {
                if let Some(id) = &m.id {
                    // Try again: the last change Windows refused.
                    let result = match (m.node(), last_ask) {
                        ("retry", Some(muted)) => {
                            refused = !ask(muted);
                            panels.show(module_ui::panels(mic, refused));
                            if refused {
                                Err("Windows said no.".to_string())
                            } else {
                                Ok(())
                            }
                        }
                        _ => Ok(()),
                    };
                    let _ = plugin.reply(id, result);
                }
            }
            None => {
                let now = Instant::now();
                if out.flash_until.is_some_and(|t| t <= now) {
                    out.flash_until = None;
                    out.leases.clear();
                }
            }
        }
    }
}
