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
use foxglove_plugin::{Fact, Hsb, Lease, LeasePriority, Plugin};
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

/// Hideout's app watch fact for iCUE (contract §4.6, v1.7): `true` while iCUE runs, `false`
/// while it doesn't; unset under Foxglove or a hub without the watch.
const ICUE_RUNNING: &str = "apps.running.icue";

/// How long iCUE counts as starting after [`ICUE_RUNNING`] turns true (#991). iCUE starts to
/// the tray and its SDK session comes up a few seconds later; until then the tile says
/// "iCUE isn't open" rather than "iCUE said no".
const STARTING: Duration = Duration::from_secs(10);

/// The one-shot "iCUE is starting" timer: armed by the event that turns [`ICUE_RUNNING`] true,
/// disarmed when it turns false or the time is up. The main loop wakes once at [`Starting::until`].
#[derive(Debug, Default)]
struct Starting {
    running: Option<bool>,
    until: Option<Instant>,
}

impl Starting {
    fn new(running: Option<bool>) -> Self {
        Starting {
            running,
            until: None,
        }
    }

    /// [`ICUE_RUNNING`] changed to `r` at `now`.
    fn set(&mut self, r: Option<bool>, now: Instant) {
        if r == Some(true) && self.running != Some(true) {
            self.until = Some(now + STARTING);
        } else if r != Some(true) {
            self.until = None;
        }
        self.running = r;
    }

    /// Whether iCUE runs as far as the tile is concerned at `now`: unknown while it starts.
    fn running(&mut self, now: Instant) -> Option<bool> {
        if self.until.is_some_and(|u| now >= u) {
            self.until = None;
        }
        if self.until.is_some() {
            None
        } else {
            self.running
        }
    }
}

/// What the tile and page show now. `running` is [`ICUE_RUNNING`] (`None`: unknown, or iCUE is
/// still [`Starting`]).
fn mirror(icue: Option<&Icue>, dll: &str, follow: &Follow, running: Option<bool>) -> Mirror {
    let Some(sdk) = icue else {
        return Mirror::NeedsSetup {
            bad: !dll.is_empty(),
        };
    };
    if !follow.session() {
        // iCUE runs but gives no session: its SDK access is off. Unknown reads as closed.
        return if running == Some(true) {
            Mirror::Refused
        } else {
            Mirror::Closed
        };
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

fn as_bool(f: Option<Fact>) -> Option<bool> {
    match f {
        Some(Fact::Bool(b)) => Some(b),
        _ => None,
    }
}

enum In {
    Icue(Notice),
    /// [`ICUE_RUNNING`] changed (`None`: removed).
    Running(Option<bool>),
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
    // Hideout's app watch: whether iCUE runs, so "iCUE said no" and "iCUE isn't open" differ.
    // Foxglove has no such fact; the calls fail there and the state stays unknown.
    let mut running = Starting::new(as_bool(plugin.fact(ICUE_RUNNING).ok().flatten()));
    if plugin.subscribe(ICUE_RUNNING).is_ok() {
        if let Some(events) = plugin.events() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for e in events {
                    if e.key == ICUE_RUNNING && tx.send(In::Running(as_bool(e.value))).is_err() {
                        return;
                    }
                }
            });
        }
    }
    drop(tx);

    let mut follow = Follow::new(&text(&settings, "device"));
    let mut every = period(&settings);
    let mut leases: Vec<Lease> = Vec::new();
    let mut shown: Option<Hsb> = None;
    let mut next_read = Instant::now();
    // Hideout's tile and page (Foxglove refuses them once and they stay off).
    let mut panels = Panels::new(plugin.clone());
    panels.show(module_ui::panels(&mirror(
        icue.as_ref(),
        &dll,
        &follow,
        running.running(Instant::now()),
    )));
    loop {
        if plugin.is_closed() {
            return;
        }
        // Following: wake for the next read too; iCUE starting: wake once when that time is up.
        // Otherwise wait only for iCUE or the user.
        let wake = [follow.following().then_some(next_read), running.until]
            .into_iter()
            .flatten()
            .min();
        let event = if let Some(wake) = wake {
            match rx.recv_timeout(wake.saturating_duration_since(Instant::now())) {
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
        let starting = running.until.is_some();
        let event_seen = event.is_some();
        match (event, &icue) {
            (Some(In::Icue(n)), Some(sdk)) => follow.notice(&n, sdk as &dyn Sdk),
            (Some(In::Icue(_)), None) => {}
            (Some(In::Running(r)), _) => running.set(r, Instant::now()),
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
            (None, _) if !follow.following() || Instant::now() < next_read => {}
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
        let now = Instant::now();
        let shown_running = running.running(now);
        if event_seen || starting != running.until.is_some() {
            panels.show(module_ui::panels(&mirror(
                icue.as_ref(),
                &dll,
                &follow,
                shown_running,
            )));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icue_counts_as_starting_for_a_while_after_it_starts() {
        let t0 = Instant::now();
        let mut r = Starting::new(Some(false));
        assert_eq!(r.running(t0), Some(false));
        r.set(Some(true), t0);
        assert_eq!(
            r.until,
            Some(t0 + STARTING),
            "the event arms the one-shot timer"
        );
        assert_eq!(
            r.running(t0 + Duration::from_secs(9)),
            None,
            "still starting"
        );
        assert_eq!(r.running(t0 + STARTING), Some(true), "time is up");
        assert_eq!(r.until, None, "the timer fires once");
        // A repeat of true doesn't re-arm it; false disarms it.
        r.set(Some(true), t0 + STARTING);
        assert_eq!(r.until, None);
        r.set(Some(false), t0 + STARTING);
        r.set(Some(true), t0 + STARTING);
        r.set(Some(false), t0 + STARTING);
        assert_eq!(r.until, None);
        assert_eq!(r.running(t0 + STARTING), Some(false));
        // Already running when the module starts: no wait (the app watch saw it first).
        assert_eq!(Starting::new(Some(true)).running(t0), Some(true));
        assert_eq!(Starting::new(None).running(t0), None);
    }
}
