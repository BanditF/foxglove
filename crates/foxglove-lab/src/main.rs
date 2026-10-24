//! Hardware entry point. Listing and behavior resolution never open a device.
use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use digitalis::{hid::Match, link::Link, panel, settings};
use foxglove_lab::{
    behaviors::Catalog,
    platform,
    report::Report,
    runner::{Hardware, Runner},
    spec,
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
    /// Prints one half's running firmware build (D7 09) as JSON. Exit code 4: the firmware
    /// can't tell (no HELLO bit 12, or the right is not reachable).
    Build {
        #[arg(long, value_parser = ["left", "right"])]
        half: String,
    },
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
fn firmware(h: digitalis::protocol::Hello) -> serde_json::Value {
    serde_json::json!({"hello_version":h.version,"panel_width":h.width,"panel_height":h.height,"right_link":h.right_link,"right_battery":h.right_battery,"discovery":h.discovery,"discovery_tries":h.discovery_tries,"debug":h.debug,"capabilities":h.capabilities})
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
    if let Command::Build { half } = &cli.command {
        let _guard = platform::guard(cli.allow_app)?;
        let _lab = platform::lab_lock(Duration::from_secs(120))?;
        let link = connect(cli.product.clone())?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        let caps = c.hello.map_or(0, |h| h.capabilities);
        if caps & digitalis::protocol::capabilities::BUILD_INFO == 0 {
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
        if caps & digitalis::protocol::capabilities::BOOTLOADER == 0 {
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
            if hello.capabilities & digitalis::protocol::capabilities::BUILD_INFO != 0 {
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
