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
    #[arg(long, global = true)]
    keymap: Option<PathBuf>,
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
    },
    Behaviors,
    Status,
    /// How each half shows up on USB (no keyboard link needed).
    Usb,
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
    let _guard = platform::guard(cli.allow_app)?;
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
        println!(
            "{}\nHELLO: {}\nCONN_STATE: {:?}\nRGB_STATE: {:?}",
            c.product,
            firmware(panel::hello(&link)?),
            settings::conn_state(&link)?,
            settings::rgb_state(&link)?
        );
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
        link.wait_connected(Duration::from_secs(10))?;
        let catalog = Catalog::load(cli.keymap)?;
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
    let catalog = Catalog::load(cli.keymap).unwrap_or_else(|e| Catalog {
        source: format!("No cached IDs: {e:#}; named invokes will fail safely"),
        ..Default::default()
    });
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
            ("local_ids".into(), catalog.source.clone()),
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
            let mut runner =
                Runner::new(&device, &catalog, hello, cancelled, non_interactive, &out);
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
    Ok(if report.exit_success() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    })
}
