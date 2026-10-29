//! `foxglove-cli`: talks to the running Foxglove app over its local API (docs/local-api.md).
//!
//! - `foxglove-cli status`: the app's version, the keyboards it sees, and their state facts.
//! - `foxglove-cli facts [pattern]`: facts matching a pattern (default `*`).
//! - `foxglove-cli profile [name|default]`: list the profiles, or switch to one (`base` works too).
//! - `foxglove-cli lights`: the PC lighting devices (cases, coolers) with each zone's colour, and
//!   the keyboard's "right half's lights follow the left" (#617).
//! - `foxglove-cli light <device> <zone> <#rrggbb>`: sets one zone, as the Lights page's Apply
//!   does (saved); the device by key or a unique part of its name, the zone by id or name.
//! - `foxglove-cli sync [on|off]`: reads, or turns on or off, the keyboard's lighting sync.

use std::io::{BufRead, BufReader, Write};

use foxglove_plugin_api::methods::{
    Devices, FactList, InitializeResult, PcDevices, Profiles, SyncState,
};
use foxglove_plugin_api::rpc::{Outcome, Request, Response};
use serde_json::{json, Value};

struct Api<S: BufRead, W: Write> {
    read: S,
    write: W,
    next: i64,
}

impl<S: BufRead, W: Write> Api<S, W> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let line = serde_json::to_string(&Request::call(self.next, method, params)).unwrap();
        writeln!(self.write, "{line}").map_err(|e| e.to_string())?;
        self.write.flush().map_err(|e| e.to_string())?;
        loop {
            let mut reply = String::new();
            if self.read.read_line(&mut reply).map_err(|e| e.to_string())? == 0 {
                return Err("Foxglove closed the connection".into());
            }
            // Events can arrive between replies; skip them.
            let Ok(response) = serde_json::from_str::<Response>(&reply) else {
                continue;
            };
            return match response.outcome {
                Outcome::Result(v) => Ok(v),
                Outcome::Error(e) => Err(format!("{method}: {} (code {})", e.message, e.code)),
            };
        }
    }
}

fn fact_text(v: &Value) -> String {
    match v {
        Value::Object(o) if o.contains_key("base64") => format!("<bytes {}>", o["base64"]),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `light <device> <zone> <#rrggbb>`'s call.
fn light_params(args: &[String]) -> Result<Value, String> {
    match args {
        [device, zone, color] => Ok(json!({"device": device, "zone": zone, "color": color})),
        _ => Err("write: light <device> <zone> <#rrggbb> (see `lights`)".into()),
    }
}

/// `sync`'s argument: none reads it.
fn sync_arg(arg: Option<&str>) -> Result<Option<bool>, String> {
    match arg.map(str::to_ascii_lowercase).as_deref() {
        None => Ok(None),
        Some("on" | "true" | "1") => Ok(Some(true)),
        Some("off" | "false" | "0") => Ok(Some(false)),
        Some(other) => Err(format!("sync takes on or off, not {other:?}")),
    }
}

/// The devices and zones as `lights` prints them, one zone a line.
fn lights_text(list: &PcDevices) -> String {
    let mut out = String::new();
    if list.devices.is_empty() {
        out.push_str("No PC lighting devices found.\n");
    }
    for d in &list.devices {
        let how = match (d.writable, d.match_keyboard) {
            (false, _) => " (Foxglove can't set it)",
            (true, true) => " (matches the keyboard)",
            (true, false) => "",
        };
        out.push_str(&format!("{} [{}]{how}\n", d.name, d.device));
        for z in &d.zones {
            let color = z.color.as_deref().unwrap_or("not set");
            out.push_str(&format!("  zone {} {}: {color}\n", z.id, z.name));
        }
    }
    for c in &list.conflicts {
        out.push_str(&format!("Running: {c} (it may put its own colours back)\n"));
    }
    if let Some(status) = &list.status {
        out.push_str(&format!("Last: {status}\n"));
    }
    out
}

fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("status");
    #[cfg(windows)]
    let pipe = foxglove_plugin_host::pipe::connect()
        .map_err(|e| format!("Foxglove isn't running (or its API is off): {e}"))?;
    #[cfg(not(windows))]
    return Err("the local API is Windows-only for now".into());
    #[cfg(windows)]
    {
        let read = BufReader::new(pipe.try_clone().map_err(|e| e.to_string())?);
        let mut api = Api {
            read,
            write: pipe,
            next: 0,
        };
        let init: InitializeResult = serde_json::from_value(api.call(
            "initialize",
            json!({
                "api": foxglove_plugin_api::API_VERSION.to_string(),
                "client": {"name": "foxglove-cli", "version": env!("CARGO_PKG_VERSION")},
                "permissions": ["devices.read", "facts.read:*", "profiles.write", "lighting.write"],
            }),
        )?)
        .map_err(|e| e.to_string())?;
        match command {
            "status" => {
                println!("Foxglove {} (API {})", init.app, init.api);
                let devices: Devices = serde_json::from_value(api.call("devices.list", json!({}))?)
                    .map_err(|e| e.to_string())?;
                if devices.devices.is_empty() {
                    println!("No keyboard with Foxglove's host link yet.");
                }
                for d in devices.devices {
                    println!(
                        "{} [{}] {}: {}",
                        d.name,
                        d.backend,
                        if d.connected {
                            "connected"
                        } else {
                            "disconnected"
                        },
                        d.caps.join(", ")
                    );
                    let facts: FactList = serde_json::from_value(api.call(
                        "facts.list",
                        json!({"pattern": format!("device.{}.*", d.id)}),
                    )?)
                    .map_err(|e| e.to_string())?;
                    let prefix = format!("device.{}.", d.id);
                    for f in facts.facts {
                        let value = serde_json::to_value(&f.value).unwrap();
                        println!(
                            "  {} = {}",
                            f.key.trim_start_matches(&prefix),
                            fact_text(&value)
                        );
                    }
                }
            }
            "facts" => {
                let pattern = args.get(1).map_or("*", String::as_str);
                let facts: FactList =
                    serde_json::from_value(api.call("facts.list", json!({"pattern": pattern}))?)
                        .map_err(|e| e.to_string())?;
                for f in facts.facts {
                    println!(
                        "{} = {}",
                        f.key,
                        fact_text(&serde_json::to_value(&f.value).unwrap())
                    );
                }
            }
            "profile" => {
                let profiles: Profiles = match args.get(1).map(String::as_str) {
                    None => serde_json::from_value(api.call("profiles.list", json!({}))?),
                    Some(name) => {
                        // "default" is the profile the app calls Default; "base" is its old name.
                        let base = name.eq_ignore_ascii_case("default")
                            || name.eq_ignore_ascii_case("base");
                        let name = (!base).then_some(name);
                        serde_json::from_value(
                            api.call("profiles.activate", json!({"name": name}))?,
                        )
                    }
                }
                .map_err(|e| e.to_string())?;
                let active = profiles.active.as_deref();
                println!("{} Default", if active.is_none() { "*" } else { " " });
                for n in &profiles.names {
                    println!("{} {n}", if active == Some(n.as_str()) { "*" } else { " " });
                }
            }
            "lights" => {
                let list: PcDevices =
                    serde_json::from_value(api.call("pc_lighting.list", json!({}))?)
                        .map_err(|e| e.to_string())?;
                print!("{}", lights_text(&list));
                let devices: Devices = serde_json::from_value(api.call("devices.list", json!({}))?)
                    .map_err(|e| e.to_string())?;
                for d in devices.devices {
                    if let Ok(v) = api.call("lighting.sync", json!({"device": d.id})) {
                        let s: SyncState = serde_json::from_value(v).map_err(|e| e.to_string())?;
                        let on = if s.on { "on" } else { "off" };
                        println!("{}: lighting sync (right follows left) {on}", d.name);
                    }
                }
            }
            "light" => {
                let list: PcDevices = serde_json::from_value(
                    api.call("pc_lighting.set_zone", light_params(&args[1..])?)?,
                )
                .map_err(|e| e.to_string())?;
                print!("{}", lights_text(&list));
            }
            "sync" => {
                let set = sync_arg(args.get(1).map(String::as_str))?;
                let devices: Devices = serde_json::from_value(api.call("devices.list", json!({}))?)
                    .map_err(|e| e.to_string())?;
                let mut last = Err("no keyboard with Foxglove's host link".to_string());
                for d in devices.devices {
                    last = match set {
                        None => api.call("lighting.sync", json!({"device": d.id})),
                        Some(on) => api.call("lighting.set_sync", json!({"device": d.id, "on": on})),
                    };
                    if last.is_ok() {
                        break;
                    }
                }
                let s: SyncState = serde_json::from_value(last?).map_err(|e| e.to_string())?;
                println!(
                    "lighting sync (right follows left): {} (read back)",
                    if s.on { "on" } else { "off" }
                );
            }
            other => {
                return Err(format!(
                "unknown command {other:?}; try status, facts [pattern], profile [name|default], lights, light <device> <zone> <#rrggbb> or sync [on|off]"
            ))
            }
        }
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("foxglove-cli: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_plugin_api::methods::{PcDevice, PcZone};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn light_and_sync_parse_their_arguments() {
        assert_eq!(
            light_params(&args(&["103c:84fd", "1", "#a57500"])).unwrap(),
            json!({"device": "103c:84fd", "zone": "1", "color": "#a57500"})
        );
        assert!(light_params(&args(&["103c:84fd", "1"])).is_err());
        assert_eq!(sync_arg(None), Ok(None));
        assert_eq!(sync_arg(Some("ON")), Ok(Some(true)));
        assert_eq!(sync_arg(Some("off")), Ok(Some(false)));
        assert!(sync_arg(Some("maybe")).is_err());
    }

    #[test]
    fn lights_prints_each_zone_with_its_colour() {
        let list = PcDevices {
            devices: vec![PcDevice {
                device: "103c:84fd".into(),
                name: "Case".into(),
                writable: true,
                match_keyboard: false,
                zones: vec![
                    PcZone {
                        id: "1".into(),
                        name: "Logo".into(),
                        color: Some("#a57500".into()),
                    },
                    PcZone {
                        id: "2".into(),
                        name: "Fan".into(),
                        color: None,
                    },
                ],
            }],
            status: Some("Case: colours set".into()),
            conflicts: vec!["Light Studio on Case".into()],
        };
        assert_eq!(
            lights_text(&list),
            "Case [103c:84fd]\n  zone 1 Logo: #a57500\n  zone 2 Fan: not set\nRunning: Light Studio on Case (it may put its own colours back)\nLast: Case: colours set\n"
        );
    }
}
