//! Focus timer, an example Foxglove Kit: alternate focus and rest periods, publish them as
//! facts (`focus.state` = "focus" / "rest", `focus.remaining` in seconds), and colour every
//! keyboard's underglow red while focusing and green while resting.
//!
//! It needs no network or files, so Foxglove runs it in a sandbox. Options (in `kit.toml`'s
//! `run`): `--focus-minutes N` (25), `--rest-minutes N` (5), `--focus-seconds N` and
//! `--rest-seconds N` for testing, `--no-lights` to publish facts only.

use std::time::Duration;

use foxglove_kit::{Fact, Hsb, Kit, SetLightingParams};

struct Options {
    focus: Duration,
    rest: Duration,
    lights: bool,
}

fn options() -> Options {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |flag: &str| -> Option<u64> {
        args.windows(2).find(|w| w[0] == flag).and_then(|w| w[1].parse().ok())
    };
    let pick = |minutes: &str, seconds: &str, default_min: u64| {
        num(seconds).map(Duration::from_secs).unwrap_or(Duration::from_secs(num(minutes).unwrap_or(default_min) * 60))
    };
    Options {
        focus: pick("--focus-minutes", "--focus-seconds", 25),
        rest: pick("--rest-minutes", "--rest-seconds", 5),
        lights: !args.iter().any(|a| a == "--no-lights"),
    }
}

fn colour(kit: &Kit, focus: bool) {
    let color = if focus { Hsb { hue: 0, saturation: 100, brightness: 60 } } else { Hsb { hue: 120, saturation: 100, brightness: 60 } };
    for device in kit.devices().unwrap_or_default() {
        if device.connected && device.caps.iter().any(|c| c == "lighting") {
            let set = SetLightingParams { device: device.id, on: Some(true), color: Some(color), effect: Some("0".into()) };
            if let Err(e) = kit.set_lighting(&set) {
                eprintln!("focus-timer: lighting: {e}");
            }
        }
    }
}

fn main() {
    let opts = options();
    let kit = match Kit::connect("focus-timer", env!("CARGO_PKG_VERSION")) {
        Ok(kit) => kit,
        Err(e) => {
            eprintln!("focus-timer: can't reach Foxglove: {e}");
            std::process::exit(1);
        }
    };
    let mut focus = true;
    loop {
        let period = if focus { opts.focus } else { opts.rest };
        let state = if focus { "focus" } else { "rest" };
        if kit.set_fact("focus.state", Some(Fact::Text(state.into()))).is_err() {
            return; // Foxglove went away
        }
        if opts.lights {
            colour(&kit, focus);
        }
        // Count down once a second so the panel or other Kits can show the time left.
        let mut left = period.as_secs();
        while left > 0 {
            let _ = kit.set_fact("focus.remaining", Some(Fact::Int(left as i64)));
            std::thread::sleep(Duration::from_secs(1));
            left -= 1;
        }
        focus = !focus;
    }
}
