//! Corsair mirror (#73), a Foxglove plugin: the keyboard's underglow follows a Corsair
//! device's colours in iCUE. Mirroring only: it reads colours and never takes control of
//! Corsair lighting (no `CorsairRequestControl`).
//!
//! - **Corsair's SDK:** the user downloads Corsair's iCUE SDK and points the "sdk-dll" setting
//!   at `iCUESDK.x64_2019.dll`. Foxglove ships no Corsair files (their licence grants no
//!   redistribution). iCUE must be running with SDK access on.
//! - **When it reads (#283):** iCUE's events start and stop it. Reading starts when the SDK
//!   session connects and the chosen device is present. It stops when the session ends, that
//!   device disconnects, or the plugin stops. Only in between does it read the colours
//!   `max-hz` times a second, since iCUE has no colour-change event.
//! - **Lights:** a low-priority lease in the device's average colour, changed in place, given
//!   back whenever reading stops.

mod follow;
mod module_ui;
mod sdk;

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use follow::Follow;
use foxglove_plugin::ui::Panels;
use foxglove_plugin::{Hsb, Lease, LeasePriority, Plugin};
use module_ui::Mirror;
use sdk::{Icue, Notice, Sdk};

/// Loads Corsair's SDK from `dll`; `None` with a log line when it is unset or won't load.
fn load(dll: &str, tx: &mpsc::Sender<Notice>) -> Option<Icue> {
    if dll.is_empty() {
        eprintln!("icue-mirror: set the path to Corsair's iCUESDK.x64_2019.dll in its settings");
        return None;
    }
    Icue::load(dll, tx.clone())
        .map_err(|e| eprintln!("icue-mirror: {e}"))
        .ok()
}

/// What the tile and page show now.
fn mirror(icue: Option<&Icue>, dll: &str, follow: &Follow) -> Mirror {
    let Some(sdk) = icue else {
        return Mirror::NeedsSetup {
            bad: !dll.is_empty(),
        };
    };
    if !follow.session() {
        return Mirror::Closed;
    }
    match follow.device() {
        Some(id) => Mirror::Following {
            model: sdk
                .devices()
                .unwrap_or_default()
                .into_iter()
                .find(|d| d.id == id)
                .map_or_else(|| "Corsair device".into(), |d| d.model),
        },
        None => Mirror::NoDevice {
            wanted: follow.wanted.clone(),
        },
    }
}

enum In {
    Icue(Notice),
    Settings(serde_json::Map<String, serde_json::Value>),
}

fn text(values: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    values
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn period(values: &serde_json::Map<String, serde_json::Value>) -> Duration {
    let hz = values
        .get("max-hz")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .clamp(1, 30);
    Duration::from_millis(1000 / hz)
}

fn lease_all(plugin: &Arc<Plugin>, color: Hsb) -> Vec<Lease> {
    plugin
        .devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.connected && d.caps.iter().any(|c| c == "lighting"))
        .filter_map(|d| {
            plugin
                .lease(&d.id, color, LeasePriority::Low)
                .map_err(|e| eprintln!("icue-mirror: lighting: {e}"))
                .ok()
        })
        .collect()
}

fn main() {
    let plugin: Arc<Plugin> = match Plugin::connect("icue-mirror", env!("CARGO_PKG_VERSION")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("icue-mirror: can't reach Foxglove: {e}");
            std::process::exit(1);
        }
    };
    let settings = plugin.settings().unwrap_or_default();
    let (tx, rx) = mpsc::channel::<In>();

    // iCUE's notices, through a channel of their own (the SDK calls back on its threads).
    let (notice_tx, notices) = mpsc::channel::<Notice>();
    let mut dll = text(&settings, "sdk-dll");
    let mut icue = load(&dll, &notice_tx);
    let forward = tx.clone();
    std::thread::spawn(move || {
        for n in notices {
            if forward.send(In::Icue(n)).is_err() {
                return;
            }
        }
    });
    if let Some(changes) = plugin.settings_changes() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for values in changes {
                if tx.send(In::Settings(values)).is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);

    let mut follow = Follow::new(&text(&settings, "device"));
    let mut every = period(&settings);
    let mut leases: Vec<Lease> = Vec::new();
    let mut shown: Option<Hsb> = None;
    let mut next_read = Instant::now();
    // Hideout's tile and page (Foxglove refuses them once and they stay off).
    let mut panels = Panels::new(plugin.clone());
    panels.show(module_ui::panels(&mirror(icue.as_ref(), &dll, &follow)));
    loop {
        if plugin.is_closed() {
            return;
        }
        // Following: wake for the next read too. Otherwise wait only for iCUE or the user.
        let event = if follow.following() {
            match rx.recv_timeout(next_read.saturating_duration_since(Instant::now())) {
                Ok(e) => Some(e),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(e) => Some(e),
                Err(_) => return,
            }
        };
        let event_seen = event.is_some();
        match (event, &icue) {
            (Some(In::Icue(n)), Some(sdk)) => follow.notice(&n, sdk as &dyn Sdk),
            (Some(In::Icue(_)), None) => {}
            (Some(In::Settings(values)), _) => {
                every = period(&values);
                let wanted = text(&values, "device");
                if let (true, Some(sdk)) = (wanted.to_ascii_lowercase() != follow.wanted, &icue) {
                    follow.want(&wanted, sdk);
                }
                // A new SDK file takes effect at once (contract §4.7, kind = "file").
                let path = text(&values, "sdk-dll");
                if path != dll {
                    dll = path;
                    if let Some(new) = load(&dll, &notice_tx) {
                        // The old SDK stays loaded (Icue never unloads its DLL): it may still
                        // call back on its threads.
                        icue = Some(new);
                        follow = Follow::new(&wanted);
                        leases.clear();
                        shown = None;
                    } else if icue.is_some() {
                        eprintln!("icue-mirror: keeps the SDK it has");
                    }
                }
            }
            (None, Some(sdk)) => {
                next_read = Instant::now() + every;
                if let Some(color) = follow.read(sdk) {
                    if leases.is_empty() {
                        leases = lease_all(&plugin, color);
                    } else if shown != Some(color) {
                        for l in &leases {
                            if let Err(e) = l.set(color) {
                                eprintln!("icue-mirror: lighting: {e}");
                            }
                        }
                    }
                    shown = Some(color);
                }
            }
            (None, None) => {}
        }
        if event_seen {
            panels.show(module_ui::panels(&mirror(icue.as_ref(), &dll, &follow)));
        }
        if !follow.following() && !leases.is_empty() {
            // Reading stopped: the lights go back.
            leases.clear();
            shown = None;
        }
        if follow.following() && leases.is_empty() && shown.is_none() {
            next_read = Instant::now();
        }
    }
}
