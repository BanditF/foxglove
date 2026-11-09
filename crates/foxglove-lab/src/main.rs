//! Hardware entry point. Listing and behavior resolution never open a device.
use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use foxglove_lab::{
    behaviors::Catalog,
    platform,
    report::Report,
    runner::{Hardware, Runner},
    spec,
};
use foxglove_link::{
    art::RelayWait, hid::Match, link::Link, panel, protocol::capabilities, settings,
};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::Ordering, time::Duration};

#[derive(Parser)]
#[command(version, about = "Foxglove hardware-in-the-loop test runner")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[arg(long, global = true)]
    product: Option<String>,
    #[arg(long, global = true)]
    allow_app: bool,
    /// The app's saved keymap to take behavior ids from; used only when it names the build the
    /// left runs (D7 09), else the lab reads them over Studio (#618).
    #[arg(long, global = true)]
    keymap: Option<PathBuf>,
    /// The left's Studio serial port (COMn) for that read; found by USB ids otherwise.
    #[arg(long, global = true)]
    studio_port: Option<String>,
}
#[derive(Subcommand)]
enum Command {
    Run {
        #[arg(long)]
        suite: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        only: Option<String>,
        #[arg(long)]
        list: bool,
        #[arg(long)]
        non_interactive: bool,
        /// Leave the underglow as the cases leave it: no save at the start, no restore at the
        /// end. For scripts that change the look on the keyboard's side on purpose and put the
        /// user's lights back themselves (scripts/lab/lights_page.py, #677).
        #[arg(long)]
        keep_lights: bool,
    },
    Behaviors,
    Status,
    /// The keyboard's self-report (#705): pass/fail per feature from both halves' D7 13 blocks,
    /// the test after a batch flash. Through the app's health.read while the app runs (no
    /// --allow-app needed: the lab never touches the link then). Exit 0 nothing failed (warnings
    /// print), 1 something failed, 2 blocked.
    Health {
        /// One JSON object instead of lines.
        #[arg(long)]
        json: bool,
        /// Keep the keyboard's D7 93 replies in this file (direct reads only).
        #[arg(long)]
        record: Option<PathBuf>,
        /// Judge a recording instead of the keyboard (no device is opened).
        #[arg(long)]
        replay: Option<PathBuf>,
        /// Leave out the frames and effects cases (#492) that a direct read runs after the
        /// self-report.
        #[arg(long)]
        no_lights: bool,
        /// Leave out the knob cases (#719) that a direct read runs after the lights.
        #[arg(long)]
        no_knob: bool,
    },
    /// The knob end to end (#719): the PC's hold (D7 14 KNOB, HELLO bit 31) on master, on an
    /// app, back to master and released, and the knob's health row (D7 13 block v2). Turns and
    /// clicks need a person: one HUMAN line. `health` runs them too. Exit 0 pass, 1 fail, 2
    /// blocked (no bit 31, no keyboard, the app running).
    Knob {
        /// One JSON object instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// The PC's lights end to end (#492): SET_FRAME (D7 0C, HELLO bit 14) and SET_EFFECT (D7 0F,
    /// bit 19) cases, the replies' status and owner and the LEDs read back (D7 83, bit 24), the
    /// lights handed back at the end. `health` runs them too, without the watchdog. Exit 0 pass,
    /// 1 fail, 2 blocked (a bit missing, no keyboard, the app running).
    Lights {
        /// One JSON object instead of lines.
        #[arg(long)]
        json: bool,
        /// Also wait out the keyboard's frame timeout (30 s) for its hand-back event.
        #[arg(long)]
        watchdog: bool,
        /// Frames sent one in flight in the stream case (0 skips it).
        #[arg(long, default_value_t = 30)]
        stream: u16,
    },
    /// What each light can take (#486): a bounded burst of writes, one in flight, to the
    /// keyboard's frames (SET_FRAME, on the bus the link uses), every catalog board with a driver
    /// and every LampArray Windows lets the lab drive; per light the sustained accepted rate,
    /// write latency p50/p95, errors and CPU, and the interval scenes should use. The user's
    /// lights go back after each burst. Never beside the app. Exit 0 pass, 1 fail, 2 blocked.
    Rates {
        /// One JSON object (rows and the rates file's entries) instead of lines.
        #[arg(long)]
        json: bool,
        /// Seconds per light at most (never over 120).
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// Writes per light at most.
        #[arg(long, default_value_t = 400)]
        writes: u32,
        /// Which lights: keyboard, case, lamps (comma-separated); all by default.
        #[arg(long, value_delimiter = ',', value_parser = ["keyboard", "case", "lamps"])]
        only: Vec<String>,
        /// The rates file to put the measured entries into (crates/devices/light-rates.toml);
        /// it is created from the bundled one when missing.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// How each half shows up on USB (no keyboard link needed).
    Usb,
    /// Whether Foxglove would be away on this PC now (#410, #683): the session locked or the
    /// display off, as JSON ({"locked", "display_off", "away"}; display_off null when Windows
    /// sent no state within 2 s). No keyboard link needed; for scripts/lab (labkit.away).
    Away,
    /// Reboots one half into its UF2 bootloader (D7 08). For scripts/flash only, after the user
    /// typed FLASH there. Exit code 4: this firmware can't (no HELLO bit 9), use a double tap.
    Bootloader {
        #[arg(long, value_parser = ["left", "right"])]
        half: String,
        /// Must be the word FLASH, passed by the flasher after the user typed it.
        #[arg(long)]
        confirmed: String,
    },
    /// Streams colours to the underglow (INVOKE rgb_ug RGB_COLOR_HSB, as the app's lighting does)
    /// and measures how soon each half reports each one (#266). Puts the colour back; JSON out.
    RgbLatency {
        #[arg(long, default_value_t = 60)]
        writes: u16,
        #[arg(long, default_value_t = 5.0)]
        hz: f64,
        /// How often to ask for RGB_STATE, as the app does (D7 83 events arrive on their own). Much
        /// faster restarts the relay read each time, so the right never updates.
        #[arg(long, default_value_t = 250)]
        poll_ms: u64,
        /// How long to keep watching after the last write.
        #[arg(long, default_value_t = 8)]
        settle_s: u64,
    },
    /// Prints the art relay's status (D6 STATUS, asked twice so the right's part is fresh).
    ArtStatus,
    /// The PC state (D7 11 / D7 91, HELLO bit 26, #609): reads the keyboard's state, or sends
    /// away, awake or back (awake with the wake) with a lease, and prints the state after it as
    /// JSON (held, USB, keys, active, lease left, the hold's reason). Exit code 4: no bit 26.
    /// A send competes with a running app's own state: read only while it runs.
    PcState {
        #[arg(long, default_value = "read", value_parser = ["read", "away", "awake", "back"])]
        set: String,
        #[arg(long, default_value_t = 600)]
        lease_s: u16,
    },
    /// Prints what the keyboard reports on its own for `--seconds`: each RGB_STATE event (D7 83,
    /// both halves' shown state: lit, wanted, idle, PC-drawn, unpowered) and each PC_STATE ask
    /// (D7 91), one JSON line each with the milliseconds since the start; first one RGB_STATE
    /// read as the baseline. Event-driven, ends at the deadline (#651's steps 2, 3 and 5).
    RgbWatch {
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
    /// Waits for the left's relay to the right half to be up: asks STATUS once, then waits for
    /// the relay-ready event (D6 85 with link 2) up to the timeout (#594). For scripts/flash.
    /// Exit code 0: up (JSON out); 3: timed out; 4: the firmware sends no relay events (no
    /// HELLO bits 3 and 20), so ask HELLO's right link instead.
    RelayWait {
        #[arg(long, default_value_t = 60)]
        timeout_s: u64,
    },
    /// Prints one half's running firmware build (D7 09) as JSON. Exit code 4: the firmware
    /// can't tell (no HELLO bit 12, or the right is not reachable).
    Build {
        #[arg(long, value_parser = ["left", "right"])]
        half: String,
    },
    /// The LampArray sitting (#88, #115): lights one LED of one half alone (D7 0A LAMP_TEST, dev
    /// LampArray firmware), so a person can name the key above it. `--index off` gives the half
    /// its own lights back.
    Lamp {
        #[arg(long, value_parser = ["left", "right"])]
        half: String,
        /// The LED's place on its half's strip, from 0 (the Corne: 0-20); "off"; or "walk": each
        /// LED of the half in turn, each in the next colour of red, green, blue, white, so the
        /// person reading the keys off in order can keep count, then off.
        #[arg(long)]
        index: String,
        /// Colour as RRGGBB (one index; walk uses its own colours).
        #[arg(long, default_value = "ffffff")]
        rgb: String,
        /// Walk: how long each LED stays lit.
        #[arg(long, default_value_t = 3000)]
        hold_ms: u64,
    },
    /// The LampArray sitting (#272): a white dot through every lamp of the board with SET_FRAME
    /// (D7 0C), one frame per acknowledgement, then RELEASE. Needs HELLO bit 14 (FRAMES).
    FrameDot {
        /// How long each lamp stays lit.
        #[arg(long, default_value_t = 400)]
        step_ms: u64,
    },
}

const SETTINGS: u8 = 0xD7;
const LAMP_TEST: u8 = 0x0A;
const FRAME: u8 = 0x0C;
const FRAMES_CAP: u32 = 1 << 14;

fn rgb_arg(s: &str) -> Result<[u8; 3]> {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16)?;
    ensure!(
        s.trim_start_matches('#').len() == 6,
        "colour must be RRGGBB"
    );
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// One whole frame (reports of up to 9 lamps), SHOW on the last; returns the D7 8C reply.
fn send_frame(link: &Link, rgb: &[[u8; 3]]) -> Result<[u8; foxglove_link::protocol::REPORT]> {
    use foxglove_link::Transport;
    ensure!(
        !rgb.is_empty() && rgb.len() <= foxglove_link::lamps::MAX_LAMPS,
        "a frame has 1-{} lamps",
        foxglove_link::lamps::MAX_LAMPS
    );
    let lamps = rgb.len();
    let mut start = 0;
    loop {
        let n = (lamps - start).min(9);
        let last = start + n == lamps;
        let mut r = vec![SETTINGS, FRAME, u8::from(last), start as u8, n as u8];
        for c in &rgb[start..start + n] {
            r.extend_from_slice(c);
        }
        if last {
            return link.request(&r, SETTINGS, FRAME, Duration::from_secs(2));
        }
        link.send(&r)?;
        start += n;
    }
}
fn sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
fn firmware(h: foxglove_link::protocol::Hello) -> serde_json::Value {
    serde_json::json!({"hello_version":h.version,"panel_width":h.width,"panel_height":h.height,"right_link":h.right_link,"right_battery":h.right_battery,"discovery":h.discovery,"discovery_tries":h.discovery_tries,"debug":h.debug,"capabilities":h.capabilities})
}
/// The board's LEDs by part (#561): what it reports (LAMP_PARTS, HELLO bit 22), else what
/// firmware that doesn't report them has.
fn board_parts(
    link: &Link,
    hello: Option<&foxglove_link::protocol::Hello>,
) -> foxglove_link::lamps::LampParts {
    let reports = hello
        .is_some_and(|h| h.capabilities & foxglove_link::protocol::capabilities::LAMP_PARTS != 0);
    let read = reports.then(|| foxglove_link::settings::lamp_parts(link));
    match read {
        Some(Ok(parts)) => parts,
        Some(Err(e)) => {
            eprintln!("reading the board's LED counts: {e}; using the old firmware's");
            foxglove_link::lamps::LampParts::old_firmware()
        }
        None => foxglove_link::lamps::LampParts::old_firmware(),
    }
}

/// The behavior ids for the firmware on `link` (#618): the saved keymap when it names the
/// build the left runs (D7 09), else the running app's `behaviors.list` when it has every name
/// in `needed` (#639), else the lab's own Studio read.
fn catalog_for(
    link: &impl foxglove_link::Transport,
    hello: Option<&foxglove_link::protocol::Hello>,
    keymap: Option<PathBuf>,
    studio_port: Option<&str>,
    app_running: bool,
    needed: &std::collections::BTreeSet<String>,
) -> Catalog {
    let builds = hello.is_some_and(|h| h.capabilities & capabilities::BUILD_INFO != 0);
    let left = builds
        .then(|| settings::build_info(link, 0).ok())
        .flatten()
        .map(|b| foxglove_lab::behaviors::build_key(&b));
    let app = app_running
        .then(|| foxglove_lab::app_api::behaviors(foxglove_lab::app_api::BEHAVIORS_WAIT));
    foxglove_lab::behaviors::resolve(Catalog::load(keymap), left.as_deref(), app, needed, || {
        foxglove_lab::studio::read(studio_port)
    })
}

/// One half's RGB_SHOWN (#605) as JSON; null when the firmware can't say.
fn shown_json(s: Option<foxglove_link::protocol::RgbShown>) -> serde_json::Value {
    s.map_or(serde_json::Value::Null, |s| {
        serde_json::json!({"lit": s.lit, "wanted": s.wanted, "idle": s.idle, "host": s.host,
            "unpowered": s.unpowered, "following": s.following, "value": s.value, "dim": s.dim})
    })
}

fn pc_state_json(s: &foxglove_link::protocol::PcState) -> serde_json::Value {
    serde_json::json!({"status": s.status, "event": s.lease_ending, "state": format!("{:?}", s.state),
        "held": s.held, "usb": s.usb, "keys": s.keys, "active": s.active, "lease_s": s.lease_s,
        "reason": s.reason, "reason_text": s.reason_text()})
}

fn connect(product: Option<String>) -> Result<Link> {
    Link::hid(Match {
        product,
        ..Default::default()
    })
}
fn main() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    if matches!(cli.command, Command::Behaviors) {
        let catalog = Catalog::load(cli.keymap)?;
        println!("Source: {}", catalog.source);
        for (name, id) in catalog.ids {
            let params = match name.as_str() {
                "mo" | "tog" | "to" | "sl" => "param1=layer index, param2=0",
                "kp" => "sink only: param1=encoded HID usage (0x00070004=A), param2=0",
                "rgb_ug" | "bl" | "ext_power" => "param1=ZMK command, param2=command value",
                "none" | "trans" => "no parameters",
                "bootloader" | "bt" | "out" | "sys_reset" | "soft_off" => {
                    "BLOCKED: HELLO cannot prove firmware refusal"
                }
                _ => "unclassified: injection blocked",
            };
            println!("{name} → {id}: {params}");
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if matches!(cli.command, Command::Away) {
        let locked = foxglove_lab::platform::locked();
        let display_off = foxglove_lab::platform::display_off();
        println!(
            "{}",
            serde_json::json!({
                "locked": locked,
                "display_off": display_off,
                "away": foxglove_lab::platform::away_reason(locked, display_off),
            })
        );
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if matches!(cli.command, Command::Usb) {
        let halves = foxglove_lab::host::classify(&foxglove_lab::host::usb_entries()?);
        for h in &halves {
            println!("{}: {:?}: {}", h.half, h.verdict, h.message);
        }
        let failed = halves
            .iter()
            .any(|h| h.verdict == foxglove_lab::host::Verdict::Fail);
        return Ok(if failed {
            std::process::ExitCode::FAILURE
        } else {
            std::process::ExitCode::SUCCESS
        });
    }
    if let Command::Lamp {
        half,
        index,
        rgb,
        hold_ms,
    } = &cli.command
    {
        use foxglove_link::Transport;
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(300))?;
        let link = connect(cli.product.clone())?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        let half_id = u8::from(half == "right");
        // The half's LEDs, as the board says (#561).
        let count = board_parts(&link, c.hello.as_ref()).count(usize::from(half_id));
        let last = u8::try_from(count.saturating_sub(1)).unwrap_or(u8::MAX);
        let light = |i: u8, c: [u8; 3]| -> Result<u8> {
            let r = link.request(
                &[SETTINGS, LAMP_TEST, half_id, i, c[0], c[1], c[2]],
                SETTINGS,
                LAMP_TEST,
                Duration::from_secs(4),
            )?;
            Ok(r[2])
        };
        let steps: Vec<(u8, [u8; 3])> = match index.as_str() {
            "off" => vec![(0xff, [0; 3])],
            "walk" => {
                const COLOURS: [(&str, [u8; 3]); 4] = [
                    ("red", [255, 0, 0]),
                    ("green", [0, 255, 0]),
                    ("blue", [0, 0, 255]),
                    ("white", [255, 255, 255]),
                ];
                (0..=last)
                    .map(|i| (i, COLOURS[usize::from(i) % COLOURS.len()].1))
                    .chain(std::iter::once((0xff, [0; 3])))
                    .collect()
            }
            n => {
                let i: u8 = n.parse()?;
                ensure!(i <= last, "index is 0-{last}, off or walk");
                vec![(i, rgb_arg(rgb)?)]
            }
        };
        let walk = index == "walk";
        for (i, c) in steps {
            let status = light(i, c)?;
            if status != 0 {
                eprintln!(
                    "LAMP_TEST status {status} ({})",
                    match status {
                        4 => "this firmware has no LAMP_TEST: dev LampArray build only",
                        2 => "no relay link to the right",
                        _ => "bad request",
                    }
                );
                return Ok(std::process::ExitCode::from(if status == 4 {
                    4
                } else {
                    1
                }));
            }
            println!(
                "{}",
                serde_json::json!({"half": half, "index": if i == 0xff { "off".to_string() } else { i.to_string() }, "rgb": format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])})
            );
            if walk && i != 0xff {
                std::thread::sleep(Duration::from_millis(*hold_ms));
            }
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if let Command::FrameDot { step_ms } = &cli.command {
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(300))?;
        let link = connect(cli.product.clone())?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        if c.hello.map_or(0, |h| h.capabilities) & FRAMES_CAP == 0 {
            eprintln!("this firmware has no SET_FRAME (no HELLO bit 14)");
            return Ok(std::process::ExitCode::from(4));
        }
        // The board's LEDs by part, as it says (#561).
        let parts = board_parts(&link, c.hello.as_ref());
        for i in 0..parts.total() {
            let mut frame = vec![[0u8; 3]; parts.total()];
            frame[i] = [255, 255, 255];
            let r = send_frame(&link, &frame)?;
            if r[2] != 0 {
                eprintln!(
                    "SET_FRAME status {} (1 busy: owner {}), stopping",
                    r[2], r[3]
                );
                return Ok(std::process::ExitCode::from(1));
            }
            println!(
                "{}",
                serde_json::json!({"lamp": i, "part": parts.part_of(i).map(|(p, _)| p)})
            );
            std::thread::sleep(Duration::from_millis(*step_ms));
        }
        use foxglove_link::Transport;
        let r = link.request(
            &[SETTINGS, FRAME, 2],
            SETTINGS,
            FRAME,
            Duration::from_secs(2),
        )?;
        println!(
            "{}",
            serde_json::json!({"released": r[2] == 0, "owner": r[3]})
        );
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if let Command::Build { half } = &cli.command {
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(120))?;
        let link = connect(cli.product.clone())?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        let caps = c.hello.map_or(0, |h| h.capabilities);
        if caps & foxglove_link::protocol::capabilities::BUILD_INFO == 0 {
            eprintln!("this firmware doesn't report its build (no HELLO bit 12)");
            return Ok(std::process::ExitCode::from(4));
        }
        return match settings::build_info(&link, u8::from(half == "right")) {
            Ok(b) => {
                let v = b.version;
                println!(
                    "{}",
                    serde_json::json!({
                        "half": half,
                        "version": format!("{}.{}.{}", v.major, v.minor, v.patch),
                        "profile": if v.profile == 1 { "ga" } else { "dev" },
                        "sha": b.sha,
                    })
                );
                Ok(std::process::ExitCode::SUCCESS)
            }
            Err(e) => {
                eprintln!("{half}: {e:#}");
                Ok(std::process::ExitCode::from(4))
            }
        };
    }
    if let Command::RelayWait { timeout_s } = &cli.command {
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(120))?;
        let link = connect(cli.product.clone())?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        let caps = c.hello.map_or(0, |h| h.capabilities);
        let events = capabilities::ART_STATUS_EVENT | capabilities::RIGHT_EVENTS;
        if caps & events != events {
            eprintln!("this firmware sends no relay-ready event (HELLO bits 3 and 20)");
            return Ok(std::process::ExitCode::from(4));
        }
        let text = |s: Option<foxglove_link::protocol::ArtStatus>| serde_json::json!({"right_link": s.map(|s| s.link), "text": s.map(|s| s.to_string())});
        return match foxglove_link::art::wait_relay_ready(&link, Duration::from_secs(*timeout_s))? {
            RelayWait::Ready(s) => {
                println!("{}", text(Some(s)));
                Ok(std::process::ExitCode::SUCCESS)
            }
            RelayWait::TimedOut(s) => {
                eprintln!("relay to the right not up after {timeout_s} s: {}", text(s));
                Ok(std::process::ExitCode::from(3))
            }
        };
    }
    if let Command::PcState { set, lease_s } = &cli.command {
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(120))?;
        let link = connect(cli.product.clone())?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        if c.hello.map_or(0, |h| h.capabilities) & capabilities::PC_STATE == 0 {
            eprintln!("this firmware has no PC state (HELLO bit 26)");
            return Ok(std::process::ExitCode::from(4));
        }
        let report = match set.as_str() {
            "away" => foxglove_link::protocol::pc_away(),
            "awake" => foxglove_link::protocol::pc_awake(*lease_s, false),
            "back" => foxglove_link::protocol::pc_awake(*lease_s, true),
            _ => foxglove_link::protocol::pc_read(),
        };
        let s = settings::pc_state(&link, &report)?;
        println!("{}", pc_state_json(&s));
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if let Command::RgbWatch { seconds } = &cli.command {
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(120))?;
        let link = connect(cli.product.clone())?;
        link.wait_connected(Duration::from_secs(10))?;
        let (tx, rx) = std::sync::mpsc::channel();
        // On the link's reader: only enqueue.
        let _reports = link.subscribe_reports(move |_, r| {
            let _ = tx.send(*r);
        });
        let start = std::time::Instant::now();
        let shown = |r: &foxglove_link::protocol::RgbOutput| serde_json::json!({"left": shown_json(r.left), "right": shown_json(r.right)});
        let (_, base) = settings::rgb_output(&link)?;
        println!(
            "{}",
            serde_json::json!({"ms": start.elapsed().as_millis() as u64, "baseline": shown(&base)})
        );
        let end = start + Duration::from_secs(*seconds);
        // Start: each report the keyboard sends. Stop: the deadline.
        while let Ok(r) = rx.recv_timeout(end.saturating_duration_since(std::time::Instant::now()))
        {
            let ms = start.elapsed().as_millis() as u64;
            if foxglove_link::protocol::is_reply(
                &r,
                foxglove_link::protocol::SETTINGS,
                foxglove_link::protocol::settings::RGB_STATE,
            ) {
                let out = foxglove_link::protocol::RgbOutput::parse(&r);
                println!(
                    "{}",
                    serde_json::json!({"ms": ms, "rgb_state": shown(&out)})
                );
            } else if foxglove_link::protocol::is_pc_state_event(&r) {
                if let Some(s) = foxglove_link::protocol::PcState::parse(&r) {
                    println!(
                        "{}",
                        serde_json::json!({"ms": ms, "pc_state": pc_state_json(&s)})
                    );
                }
            }
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if let Command::Bootloader { half, confirmed } = &cli.command {
        ensure!(
            confirmed == "FLASH",
            "refused: only scripts/flash/foxglove_flash.py calls this, after the user types FLASH"
        );
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(120))?;
        let link = connect(cli.product.clone())?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        let caps = c.hello.map_or(0, |h| h.capabilities);
        if caps & foxglove_link::protocol::capabilities::BOOTLOADER == 0 {
            eprintln!(
                "this firmware has no bootloader command (HELLO bit 9); double-tap reset instead"
            );
            return Ok(std::process::ExitCode::from(4));
        }
        return match settings::enter_bootloader(&link, u8::from(half == "right")) {
            Ok(()) => {
                println!("{half} half asked to enter its bootloader");
                Ok(std::process::ExitCode::SUCCESS)
            }
            Err(e) if e.to_string().contains("not built") => {
                eprintln!("{e:#}; double-tap reset instead");
                Ok(std::process::ExitCode::from(4))
            }
            Err(e) => Err(e),
        };
    }
    // Before the run's own app guard and lab lock below: once the lab holds the app's mutex,
    // guard(false) fails as if the app ran, and every direct read went to the app (blocked).
    if let Command::Health {
        json,
        record,
        replay,
        no_lights,
        no_knob,
    } = &cli.command
    {
        use foxglove_lab::{health as lab_health, host_lights, knob};
        // The probe guard is dropped at once, so the direct read below can take its own.
        let app_running = replay.is_none() && platform::guard(false).is_err();
        let mut out = match (lab_health::source(replay.is_some(), app_running), replay) {
            (lab_health::Source::Replay, Some(path)) => {
                let r = lab_health::Replay::parse(&std::fs::read_to_string(path)?)?;
                lab_health::read(&r, r.capabilities)
            }
            // The app holds the keyboard: ask it (health.read, API 1.11).
            (lab_health::Source::App, _) => {
                let mut out = lab_health::from_app(foxglove_lab::app_api::HEALTH_WAIT);
                if !no_lights {
                    host_lights::Cases::blocked_by(
                        "frames and effects not tested: the app holds the keyboard (quit it, then foxglove-lab lights)",
                    )
                    .fold_into(&mut out);
                }
                if !no_knob {
                    knob::Cases::blocked_by(
                        "the knob not tested: the app holds the keyboard (quit it, then foxglove-lab knob)",
                    )
                    .fold_into(&mut out);
                }
                out
            }
            _ => {
                let _guard = platform::guard(cli.allow_app)?;
                let _lab = platform::lab_lock(Duration::from_secs(120))?;
                match connect(cli.product.clone()).and_then(|l| {
                    let c = l.wait_connected(Duration::from_secs(10))?;
                    Ok((l, c))
                }) {
                    Err(e) => lab_health::Outcome {
                        rows: Vec::new(),
                        blocked: Some(format!("no keyboard: {e:#}")),
                    },
                    Ok((link, c)) => {
                        let caps = c.hello.map_or(0, |h| h.capabilities);
                        let rec = lab_health::Recorder::new(&link);
                        let mut out = lab_health::read(&rec, caps);
                        if let Some(path) = record {
                            std::fs::write(path, rec.text(caps))?;
                            eprintln!("recorded to {}", path.display());
                        }
                        // HELLO v2 (after the recording, which holds D7 93 replies only).
                        let v2 = c.hello.map(|h| foxglove_link::hello_v2::read(&link, &h));
                        out.rows.push(foxglove_link::hello_v2::row(v2.as_ref()));
                        out.rows
                            .push(foxglove_link::hello_v2::caps_row(c.hello.as_ref()));
                        // The PC's lights (#492), on the link itself: the recording keeps D7 93 only.
                        if !no_lights {
                            host_lights::run(&link, caps, None, &host_lights::Options::QUICK)
                                .fold_into(&mut out);
                        }
                        // The knob (#719): its hold, handed back at the end, and its health row.
                        if !no_knob {
                            knob::run(&link, caps).fold_into(&mut out);
                        }
                        out
                    }
                }
            }
        };
        // A live run started from a packaged app's shell writes its AppData files into that
        // app's copy (#894): a WARN, never a failure. A replay is offline and stays as recorded.
        if replay.is_none() {
            out.rows.extend(foxglove_lab::appdata::rows());
        }
        if *json {
            println!("{}", out.json());
        } else {
            for line in out.lines() {
                println!("{line}");
            }
        }
        return Ok(std::process::ExitCode::from(out.code()));
    }
    if let Command::Lights {
        json,
        watchdog,
        stream,
    } = &cli.command
    {
        use foxglove_lab::host_lights::{self, Cases, Events, Options};
        let opts = Options {
            stream: *stream,
            watchdog: *watchdog,
            ..Options::QUICK
        };
        // Never beside the app, --allow-app or not: its own frames and effects would take the
        // lights in the middle of a case.
        let cases = match platform::guard(false) {
            Err(_) => Cases::blocked_by(
                "the app holds the keyboard: quit it (tray, Quit), then run again",
            ),
            Ok(_guard) => {
                let _lab = platform::lab_lock(Duration::from_secs(120))?;
                match connect(cli.product.clone()).and_then(|l| {
                    let c = l.wait_connected(Duration::from_secs(10))?;
                    Ok((l, c))
                }) {
                    Err(e) => Cases::blocked_by(format!("no keyboard: {e:#}")),
                    Ok((link, c)) => {
                        let caps = c.hello.map_or(0, |h| h.capabilities);
                        // The keyboard's own reports (the frames' hand-back event), for the
                        // watchdog; on the link's reader, only enqueue.
                        let (tx, rx) = std::sync::mpsc::channel();
                        let _reports = link.subscribe_reports(move |_, r| {
                            let _ = tx.send(*r);
                        });
                        let wait = move |d: Duration| rx.recv_timeout(d).ok();
                        let events: Events<'_> = &wait;
                        host_lights::run(&link, caps, Some(events), &opts)
                    }
                }
            }
        };
        if *json {
            println!("{}", cases.json());
        } else {
            for line in cases.lines() {
                println!("{line}");
            }
        }
        return Ok(std::process::ExitCode::from(cases.code()));
    }
    if let Command::Knob { json } = &cli.command {
        use foxglove_lab::knob::{self, Cases};
        // Never beside the app, --allow-app or not: it holds the knob itself after each HELLO,
        // and its knob service would take the case's hold back mid-case.
        let cases = match platform::guard(false) {
            Err(_) => Cases::blocked_by(
                "the app holds the keyboard: quit it (tray, Quit), then run again",
            ),
            Ok(_guard) => {
                let _lab = platform::lab_lock(Duration::from_secs(120))?;
                match connect(cli.product.clone()).and_then(|l| {
                    let c = l.wait_connected(Duration::from_secs(10))?;
                    Ok((l, c))
                }) {
                    Err(e) => Cases::blocked_by(format!("no keyboard: {e:#}")),
                    Ok((link, c)) => knob::run(&link, c.hello.map_or(0, |h| h.capabilities)),
                }
            }
        };
        if *json {
            println!("{}", cases.json());
        } else {
            for line in cases.lines() {
                println!("{line}");
            }
        }
        return Ok(std::process::ExitCode::from(cases.code()));
    }
    if let Command::Rates {
        json,
        seconds,
        writes,
        only,
        out,
    } = &cli.command
    {
        use foxglove_lab::rates::{self, Budget, KeyboardFrames, Light, Report};
        let budget = Budget::bounded(*writes, Duration::from_secs(*seconds));
        let wants = |what: &str| only.is_empty() || only.iter().any(|o| o == what);
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let cpu = platform::cpu_time;
        let mut report = Report::default();
        // Never beside the app: its own scene or Lights page writes would share the lights.
        match platform::guard(false) {
            Err(_) => report
                .blocked
                .push("the app holds the lights: quit it (tray, Quit), then run again".into()),
            Ok(_guard) => {
                let _lab = platform::lab_lock(Duration::from_secs(120))?;
                if wants("keyboard") {
                    match connect(cli.product.clone()).and_then(|l| {
                        let c = l.wait_connected(Duration::from_secs(10))?;
                        Ok((l, c))
                    }) {
                        Err(e) => report.blocked.push(format!("keyboard not measured: {e:#}")),
                        Ok((link, c)) => {
                            let caps = c.hello.map_or(0, |h| h.capabilities);
                            let bus = match c.bus {
                                foxglove_link::link::Bus::Usb => Some("usb"),
                                foxglove_link::link::Bus::Bluetooth => Some("ble"),
                                foxglove_link::link::Bus::Unknown => None,
                            };
                            let build = (caps & capabilities::BUILD_INFO != 0)
                                .then(|| settings::build_info(&link, 0).ok())
                                .flatten()
                                .map(|b| foxglove_lab::behaviors::build_key(&b));
                            match KeyboardFrames::new(&link, caps) {
                                None => report.blocked.push(
                                    "keyboard not measured: no SET_FRAME (HELLO bit 14)".into(),
                                ),
                                Some(mut w) => {
                                    let light = Light {
                                        name: format!(
                                            "Keyboard frames ({})",
                                            bus.unwrap_or("bus unknown").to_uppercase()
                                        ),
                                        class: foxglove_devices::pc_lighting::rates::KEYBOARD
                                            .into(),
                                        device: None,
                                        transport: bus.map(String::from),
                                        build,
                                    };
                                    report.measure(&light, &mut w, budget, &at, &cpu);
                                }
                            }
                        }
                    }
                }
                if wants("case") {
                    match rates::case_boards() {
                        Err(e) => report.blocked.push(format!("case boards not listed: {e}")),
                        Ok(boards) if boards.is_empty() => report
                            .blocked
                            .push("no catalog board with a driver on this PC".into()),
                        Ok(boards) => {
                            for (light, mut w) in boards {
                                report.measure(&light, &mut w, budget, &at, &cpu);
                            }
                        }
                    }
                }
                if wants("lamps") {
                    let (lamps, why) = rates::lamp_arrays();
                    report.blocked.extend(why);
                    for (light, mut w) in lamps {
                        report.measure(&light, &mut w, budget, &at, &cpu);
                    }
                }
            }
        }
        if let Some(path) = out {
            if !report.rates.is_empty() {
                let text = std::fs::read_to_string(path)
                    .unwrap_or_else(|_| foxglove_devices::pc_lighting::rates::BUNDLED.into());
                let mut file = foxglove_devices::pc_lighting::rates::Rates::parse(&text)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                report.apply(&mut file);
                let header = foxglove_devices::pc_lighting::rates::header(&text);
                std::fs::write(path, file.to_toml(&header).map_err(anyhow::Error::msg)?)?;
                eprintln!(
                    "{} measured entr{} written to {}",
                    report.rates.len(),
                    if report.rates.len() == 1 { "y" } else { "ies" },
                    path.display()
                );
            }
        }
        if *json {
            println!("{}", report.json());
        } else {
            for line in report.lines() {
                println!("{line}");
            }
        }
        return Ok(std::process::ExitCode::from(report.code()));
    }
    let suite_name = if let Command::Run { suite, .. } = &cli.command {
        suite.clone()
    } else {
        String::new()
    };
    let suite = if let Command::Run { suite, .. } = &cli.command {
        let path = if suite.ends_with(".toml") || suite.contains(['/', '\\']) {
            PathBuf::from(suite)
        } else {
            PathBuf::from("tests/hil").join(format!("{suite}.toml"))
        };
        Some(spec::load(&path)?)
    } else {
        None
    };
    if let Command::Run {
        list: true, only, ..
    } = &cli.command
    {
        for c in &suite.as_ref().unwrap().cases {
            if only.as_ref().is_none_or(|n| c.name.contains(n)) {
                println!(
                    "{}{} — {}",
                    c.name,
                    if c.manual { " [manual]" } else { "" },
                    c.description
                );
            }
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let guard = platform::guard(cli.allow_app)?;
    let _lab = platform::lab_lock(Duration::from_secs(120))?;
    let cancelled = platform::cancellation()?;
    if matches!(cli.command, Command::ArtStatus) {
        let link = connect(cli.product)?;
        link.wait_connected(Duration::from_secs(10))?;
        foxglove_link::art::status(&link)?;
        std::thread::sleep(Duration::from_millis(300));
        let s = foxglove_link::art::status(&link)?;
        println!(
            "{}",
            serde_json::json!({"left_state": s.state, "error": s.error, "length": s.length, "right_state": s.right_state, "right_set": s.right_set, "right_set_count": s.right_set_count, "text": s.to_string()})
        );
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if matches!(cli.command, Command::Status) {
        let link = connect(cli.product)?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        let hello = panel::hello(&link)?;
        println!(
            "{}\nHELLO: {}\nCONN_STATE: {:?}\nRGB_STATE: {:?}",
            c.product,
            firmware(hello),
            settings::conn_state(&link)?,
            settings::rgb_state(&link)?
        );
        // What each half's LEDs show (#605): lit, the user's on/off, idle (the effective one with
        // the PC state's hold), the PC drawing, ext power.
        match settings::rgb_output(&link) {
            Ok((_, out)) => println!(
                "RGB_SHOWN: {}",
                serde_json::json!({"left": shown_json(out.left), "right": shown_json(out.right)})
            ),
            Err(e) => println!("RGB_SHOWN: error {e}"),
        }
        if hello.capabilities & capabilities::PC_STATE != 0 {
            match settings::pc_state(&link, &foxglove_link::protocol::pc_read()) {
                Ok(s) => println!("PC_STATE: {}", pc_state_json(&s)),
                Err(e) => println!("PC_STATE: error {e}"),
            }
        } else {
            println!("PC_STATE: unknown (no HELLO bit 26)");
        }
        // The right's whole-screen rewrites since its power-on (#618), as the left last read
        // them; asked twice so the second answer is fresh.
        if hello.capabilities & foxglove_link::protocol::capabilities::RIGHT_REDRAWS != 0 {
            let _ = settings::battery_dim(&link, None);
            std::thread::sleep(Duration::from_millis(400));
            match settings::battery_dim(&link, None).map(|d| d.right_redraws) {
                Ok(Some(n)) => println!("RIGHT_REDRAWS: {n}"),
                Ok(None) => println!("RIGHT_REDRAWS: unknown"),
                Err(e) => println!("RIGHT_REDRAWS: error {e}"),
            }
        } else {
            println!("RIGHT_REDRAWS: unknown (no HELLO bit 25)");
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if let Command::RgbLatency {
        writes,
        hz,
        poll_ms,
        settle_s,
    } = cli.command
    {
        let link = connect(cli.product)?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        let catalog = catalog_for(
            &link,
            c.hello.as_ref(),
            cli.keymap,
            cli.studio_port.as_deref(),
            guard.app_running(),
            &["rgb_ug".to_string()].into(),
        );
        let rgb_ug = *catalog.ids.get("rgb_ug").ok_or_else(|| {
            anyhow::anyhow!(
                "rgb_ug absent from the keymap metadata ({})",
                catalog.source
            )
        })?;
        let summary = foxglove_lab::rgb_latency::run(
            &link,
            rgb_ug,
            writes,
            hz,
            Duration::from_millis(poll_ms),
            Duration::from_secs(settle_s),
        )?;
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let Command::Run {
        out,
        only,
        non_interactive,
        keep_lights,
        ..
    } = cli.command
    else {
        unreachable!()
    };
    let suite = suite.unwrap();
    ensure!(
        only.as_ref()
            .is_none_or(|n| suite.cases.iter().any(|c| c.name.contains(n))),
        "--only matched no cases"
    );
    let now = chrono::Utc::now();
    let git = sha();
    let out = out.unwrap_or_else(|| {
        PathBuf::from("lab-reports").join(format!("{}-{git}", now.format("%Y%m%dT%H%M%S%.3fZ")))
    });
    std::fs::create_dir_all(&out)?;
    let mut report = Report::new(
        &suite_name,
        now.to_rfc3339(),
        BTreeMap::from([
            ("utc".into(), now.to_rfc3339()),
            ("git_sha".into(), git),
            ("runner_version".into(), env!("CARGO_PKG_VERSION").into()),
            (
                "app_manifest_version".into(),
                toml::from_str::<toml::Value>(include_str!("../../../app/Cargo.toml"))?
                    .get("package")
                    .and_then(|p| p.get("version"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
            ),
            ("os".into(), std::env::consts::OS.into()),
            ("arch".into(), std::env::consts::ARCH.into()),
        ]),
    );
    report.app = foxglove_lab::host::app_info();
    let connected = (|| -> Result<_> {
        let link = connect(cli.product)?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        ensure!(!cancelled.load(Ordering::Relaxed), "interrupted");
        let hello = c.hello.ok_or_else(|| anyhow::anyhow!("HELLO missing"))?;
        Ok((Hardware::new(link), hello, c.product))
    })();
    match connected {
        Ok((device, hello, product)) => {
            report.firmware.hello = firmware(hello);
            if hello.capabilities & foxglove_link::protocol::capabilities::BUILD_INFO != 0 {
                let build = |half| {
                    settings::build_info(&device, half).ok().map(|b| {
                        let v = b.version;
                        foxglove_lab::report::Build {
                            version: format!("{}.{}.{}", v.major, v.minor, v.patch),
                            profile: if v.profile == 1 { "ga" } else { "dev" }.into(),
                            sha: b.sha,
                        }
                    })
                };
                report.firmware.left = build(0);
                report.firmware.right = build(1);
            }
            report.environment.insert("product".into(), product);
            let catalog = catalog_for(
                &device,
                Some(&hello),
                cli.keymap,
                cli.studio_port.as_deref(),
                guard.app_running(),
                &foxglove_lab::behaviors::needed_names(&suite.cases),
            );
            report
                .environment
                .insert("local_ids".into(), catalog.source.clone());
            let mut runner =
                Runner::new(&device, &catalog, hello, cancelled, non_interactive, &out);
            let injections = foxglove_lab::injections::path();
            if let Some(r) = foxglove_lab::appdata::redirect() {
                eprintln!(
                    "{}",
                    foxglove_redirect::note("the lab's injection log", &injections, &r)
                );
            }
            runner.injections = Some(injections);
            // While the app runs, art selects go through its API (#618, #622).
            runner.app = guard
                .app_running()
                .then_some(&foxglove_lab::app_api::Pipe as &dyn foxglove_lab::app_api::AppArt);
            report.environment.insert(
                "art_select_path".into(),
                foxglove_lab::runner::art_select_path(runner.app.is_some(), &suite.cases),
            );
            // The look the keyboard had goes back when the suite ends, however it ends: the
            // guard restores on drop (a `?` out of this loop, Ctrl-C, a panic). A keymap without
            // `&rgb_ug` has nothing to restore; --keep-lights leaves the cases' look (#677).
            let _lights = foxglove_lab::lights::Saved::for_run(
                &device.link,
                catalog.ids.get("rgb_ug").copied(),
                keep_lights,
            );
            for c in &suite.cases {
                let result = runner.case(c, only.as_ref().is_none_or(|n| c.name.contains(n)));
                println!("{} {}", result.status, result.name);
                report.cases.push(result);
                report.write(&out)?;
            }
        }
        Err(e) => {
            report
                .environment
                .insert("connection_error".into(), format!("{e:#}"));
            for c in &suite.cases {
                let selected = only.as_ref().is_none_or(|n| c.name.contains(n));
                report.cases.push(foxglove_lab::report::CaseResult {
                    name: c.name.clone(),
                    description: c.description.clone(),
                    tags: c.tags.clone(),
                    manual: c.manual,
                    status: if selected {
                        foxglove_lab::report::Status::Fail
                    } else {
                        foxglove_lab::report::Status::Skipped
                    },
                    elapsed_ms: 0,
                    message: if selected {
                        format!("connection failed: {e:#}")
                    } else {
                        "filtered by --only".into()
                    },
                    steps: Vec::new(),
                });
            }
        }
    }
    report.write(&out)?;
    println!("Report: {}", out.join("report.html").display());
    Ok(std::process::ExitCode::from(report.exit_code()))
}
