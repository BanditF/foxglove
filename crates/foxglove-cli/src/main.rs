//! `foxglove-cli`: talks to the running Foxglove app over its local API (docs/local-api.md).
//!
//! - `foxglove-cli status`: the app's version, the keyboards it sees, and their state facts.
//! - `foxglove-cli facts [pattern]`: facts matching a pattern (default `*`).
//! - `foxglove-cli profile [name|base]`: list the profiles, or switch to one.

use std::io::{BufRead, BufReader, Write};

use foxglove_kit_api::methods::{Devices, FactList, InitializeResult, Profiles};
use foxglove_kit_api::rpc::{Outcome, Request, Response};
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
            let Ok(response) = serde_json::from_str::<Response>(&reply) else { continue };
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

fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("status");
    #[cfg(windows)]
    let pipe = den::pipe::connect().map_err(|e| format!("Foxglove isn't running (or its API is off): {e}"))?;
    #[cfg(not(windows))]
    return Err("the local API is Windows-only for now".into());
    #[cfg(windows)]
    {
        let read = BufReader::new(pipe.try_clone().map_err(|e| e.to_string())?);
        let mut api = Api { read, write: pipe, next: 0 };
        let init: InitializeResult = serde_json::from_value(api.call(
            "initialize",
            json!({
                "api": foxglove_kit_api::API_VERSION.to_string(),
                "client": {"name": "foxglove-cli", "version": env!("CARGO_PKG_VERSION")},
                "permissions": ["devices.read", "facts.read:*", "profiles.write"],
            }),
        )?)
        .map_err(|e| e.to_string())?;
        match command {
            "status" => {
                println!("Foxglove {} (API {})", init.app, init.api);
                let devices: Devices =
                    serde_json::from_value(api.call("devices.list", json!({}))?).map_err(|e| e.to_string())?;
                if devices.devices.is_empty() {
                    println!("No keyboard with Foxglove's host link yet.");
                }
                for d in devices.devices {
                    println!(
                        "{} [{}] {}: {}",
                        d.name,
                        d.backend,
                        if d.connected { "connected" } else { "disconnected" },
                        d.caps.join(", ")
                    );
                    let facts: FactList = serde_json::from_value(
                        api.call("facts.list", json!({"pattern": format!("device.{}.*", d.id)}))?,
                    )
                    .map_err(|e| e.to_string())?;
                    let prefix = format!("device.{}.", d.id);
                    for f in facts.facts {
                        let value = serde_json::to_value(&f.value).unwrap();
                        println!("  {} = {}", f.key.trim_start_matches(&prefix), fact_text(&value));
                    }
                }
            }
            "facts" => {
                let pattern = args.get(1).map_or("*", String::as_str);
                let facts: FactList = serde_json::from_value(api.call("facts.list", json!({"pattern": pattern}))?)
                    .map_err(|e| e.to_string())?;
                for f in facts.facts {
                    println!("{} = {}", f.key, fact_text(&serde_json::to_value(&f.value).unwrap()));
                }
            }
            "profile" => {
                let profiles: Profiles = match args.get(1).map(String::as_str) {
                    None => serde_json::from_value(api.call("profiles.list", json!({}))?),
                    Some(name) => {
                        let name = (!name.eq_ignore_ascii_case("base")).then_some(name);
                        serde_json::from_value(api.call("profiles.activate", json!({"name": name}))?)
                    }
                }
                .map_err(|e| e.to_string())?;
                let active = profiles.active.as_deref();
                println!("{} Base settings", if active.is_none() { "*" } else { " " });
                for n in &profiles.names {
                    println!("{} {n}", if active == Some(n.as_str()) { "*" } else { " " });
                }
            }
            other => return Err(format!("unknown command {other:?}; try status, facts [pattern] or profile [name|base]")),
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
