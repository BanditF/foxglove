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
    serde_json::json!({"hello_version":h.version,"panel_width":h.width,"panel_height":h.height,"right_link":h.right_link,"right_battery":h.right_battery,"discovery":h.discovery,"discovery_tries":h.discovery_tries,"debug":h.debug,"capabilities":h.capabilities,"firmware_sha":null,"note":"HELLO does not advertise firmware SHA, refusal, position or screen support"})
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
    let mut report = Report {
        schema: 1,
        environment: BTreeMap::from([
            ("utc".into(), now.to_rfc3339()),
            ("git_sha".into(), git),
            ("runner_version".into(), env!("CARGO_PKG_VERSION").into()),
            ("app_version".into(), "unknown (app is not queried)".into()),
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
        firmware: serde_json::Value::Null,
        cases: Vec::new(),
    };
    let connected = (|| -> Result<_> {
        let link = connect(cli.product)?;
        let c = link.wait_connected(Duration::from_secs(10))?;
        ensure!(!cancelled.load(Ordering::Relaxed), "interrupted");
        let hello = c.hello.ok_or_else(|| anyhow::anyhow!("HELLO missing"))?;
        Ok((Hardware::new(link), hello, c.product))
    })();
    match connected {
        Ok((device, hello, product)) => {
            report.firmware = firmware(hello);
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
