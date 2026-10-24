//! Pomodoro (#209), Foxglove's focus timer plugin: focus periods, short rests and a long rest
//! every few rounds, shown on the left panel (Art's countdown screen) and on the underglow (red
//! to focus, green to rest, through a lighting lease, so the lights go back afterwards).
//!
//! - **Survives restarts:** the clock is kept as deadlines in `data/state.json`, so a restart,
//!   sleep or late wake-up picks up where the time is now.
//! - **Keys** (bind them in the Plugins tab): `pause` pauses or resumes, `reset` starts a new
//!   focus period.
//! - **Facts:** `focus.state` (`focus`, `rest`, `long-rest`), `focus.remaining` (seconds),
//!   `focus.paused`, `focus.round`.
//!
//! It needs no network or files, so Foxglove runs it in the sandbox. Options (in `plugin.toml`'s
//! `run`): `--focus-minutes N` (25), `--rest-minutes N` (5), `--long-rest-minutes N` (15),
//! `--rounds N` (4); `--focus-seconds N` and `--rest-seconds N` for testing; `--no-lights`,
//! `--no-panel`.

mod panel;
mod timer;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use foxglove_plugin::{Fact, Hsb, Lease, LeasePriority, Plugin};
use timer::{Durations, Phase, Timer};

struct Options {
    durations: Durations,
    lights: bool,
    panel: bool,
}

fn options() -> Options {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |flag: &str| -> Option<u64> { args.windows(2).find(|w| w[0] == flag).and_then(|w| w[1].parse().ok()) };
    let secs = |minutes: &str, seconds: &str, default_min: u64| num(seconds).unwrap_or(num(minutes).unwrap_or(default_min) * 60).max(1);
    Options {
        durations: Durations {
            focus: secs("--focus-minutes", "--focus-seconds", 25),
            rest: secs("--rest-minutes", "--rest-seconds", 5),
            long_rest: secs("--long-rest-minutes", "--long-rest-seconds", 15),
            rounds: num("--rounds").map_or(4, |n| n.clamp(1, 12) as u32),
        },
        lights: !args.iter().any(|a| a == "--no-lights"),
        panel: !args.iter().any(|a| a == "--no-panel"),
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn state_file() -> std::path::PathBuf {
    Plugin::data_dir().join("state.json")
}

fn load(d: &Durations) -> Timer {
    std::fs::read_to_string(state_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| Timer::start(now(), d))
}

fn save(t: &Timer) {
    if let Ok(text) = serde_json::to_string(t) {
        if let Err(e) = std::fs::write(state_file(), text) {
            eprintln!("focus-timer: saving the timer: {e}");
        }
    }
}

/// The keyboards to show it on: (id, has lights, has a left panel).
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

fn colour(phase: Phase) -> Hsb {
    match phase {
        Phase::Focus => Hsb { hue: 0, saturation: 100, brightness: 60 },
        Phase::Rest | Phase::LongRest => Hsb { hue: 120, saturation: 100, brightness: 60 },
    }
}

fn hint(t: &Timer, d: &Durations) -> String {
    if t.paused() {
        return "PRESS TO RESUME".into();
    }
    match t.phase {
        Phase::Focus if t.round >= d.rounds => format!("NEXT REST {}M", d.long_rest / 60),
        Phase::Focus => format!("NEXT REST {}M", d.rest / 60),
        Phase::Rest | Phase::LongRest => "STAND UP".into(),
    }
}

fn main() {
    let opts = options();
    let d = opts.durations;
    let plugin: Arc<Plugin> = match Plugin::connect("focus-timer", env!("CARGO_PKG_VERSION")) {
        Ok(plugin) => plugin,
        Err(e) => {
            eprintln!("focus-timer: can't reach Foxglove: {e}");
            std::process::exit(1);
        }
    };
    let art = if opts.panel {
        panel::Art::load().map_err(|e| eprintln!("focus-timer: panel art: {e}")).ok()
    } else {
        None
    };
    let actions = plugin.actions();
    let mut t = load(&d);
    t.tick(now(), &d);
    save(&t);
    // The lights, held for the phase; dropping a lease gives them back.
    let mut leases: Vec<Lease> = Vec::new();
    let mut leased: Option<(Phase, bool)> = None;
    loop {
        if plugin.is_closed() {
            return;
        }
        let at = now();
        if t.tick(at, &d) {
            save(&t);
        }
        let left = t.left(at);
        let state = match t.phase {
            Phase::Focus => "focus",
            Phase::Rest => "rest",
            Phase::LongRest => "long-rest",
        };
        let _ = plugin.set_fact("focus.state", Some(Fact::Text(state.into())));
        let _ = plugin.set_fact("focus.remaining", Some(Fact::Int(left as i64)));
        let _ = plugin.set_fact("focus.paused", Some(Fact::Bool(t.paused())));
        let _ = plugin.set_fact("focus.round", Some(Fact::Int(i64::from(t.round))));
        let boards = keyboards(&plugin);
        // Lights: a lease per keyboard, renewed when the phase or the pause changes; paused
        // gives the lights back.
        if opts.lights && leased != Some((t.phase, t.paused())) {
            leases.clear();
            if !t.paused() {
                for (id, lights, _) in boards.iter().filter(|b| b.1) {
                    let _ = lights;
                    match plugin.lease(id, colour(t.phase), LeasePriority::Normal) {
                        Ok(l) => leases.push(l),
                        Err(e) => eprintln!("focus-timer: lighting: {e}"),
                    }
                }
            }
            leased = Some((t.phase, t.paused()));
        }
        // Panel: held until the next frame, and given back when the plugin stops (#281).
        if let Some(art) = &art {
            let hint = hint(&t, &d);
            let view = panel::View { phase: t.phase, paused: t.paused(), round: t.round, rounds: d.rounds, left, progress: t.progress(at, &d), hint: &hint };
            let frame = art.frame(&view);
            for (id, _, _) in boards.iter().filter(|b| b.2) {
                if let Err(e) = plugin.panel_hold(id, panel::W as u16, panel::H as u16, &frame) {
                    eprintln!("focus-timer: panel: {e}");
                }
            }
        }
        // Running: wait for the next second (the countdown), or a key. Paused: nothing changes
        // until a key, so wait only for that (#281).
        let wait = Duration::from_millis(1000 - u64::from(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_millis())));
        let pressed = match &actions {
            Some(rx) if t.paused() => match rx.recv() {
                Ok(k) => Some(k),
                Err(_) => return,
            },
            Some(rx) => match rx.recv_timeout(wait) {
                Ok(k) => Some(k),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => {
                std::thread::sleep(wait);
                None
            }
        };
        match pressed.as_deref() {
            Some("pause") => {
                t.toggle_pause(now());
                save(&t);
            }
            Some("reset") => {
                t = Timer::start(now(), &d);
                save(&t);
            }
            _ => {}
        }
    }
}
