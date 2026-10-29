//! The running app's local API (docs/local-api.md), for steps that must go through the app
//! while it runs (#618): the app re-applies its pinned art when the right half changes behind
//! its back (#622), so a direct SELECT is undone; `art.select` (API 1.6, #630) is the app's own
//! choice and sticks. Each call runs on its own thread with a time limit: the pipe has no
//! read timeout.
use anyhow::{anyhow, Result};
use foxglove_plugin_api::methods::ArtStatus;
use serde_json::{json, Value};
use std::time::Duration;

/// Art through the app: what [`crate::runner::Runner`] uses while the app runs.
pub trait AppArt {
    /// `art.select` of a built-in by its manifest label; answers once the app's art job ended.
    fn select_built_in(&self, label: &str, timeout: Duration) -> Result<ArtStatus>;
    /// `art.status`: what the app says the right shows.
    fn status(&self, timeout: Duration) -> Result<ArtStatus>;
}

/// A built-in set's label as `art.select` takes it: the bundled manifest's (the one the app
/// ships, `custom_art.json`) when its set count matches the right's, else the app's "Set n".
pub fn built_in_label(set_count: u8, set: i8) -> String {
    #[derive(serde::Deserialize)]
    struct Set {
        label: String,
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        set_count: u8,
        sets: Vec<Set>,
    }
    const BUNDLED: &str =
        include_str!("../../../boards/shields/nice_view_custom/widgets/custom_art.json");
    serde_json::from_str::<Manifest>(BUNDLED)
        .ok()
        .filter(|m| m.set_count == set_count && m.sets.len() == usize::from(set_count))
        .and_then(|m| {
            usize::try_from(set)
                .ok()
                .and_then(|i| m.sets.into_iter().nth(i))
        })
        .map_or_else(|| format!("Set {set}"), |s| s.label)
}

/// The API the lab asks for: the oldest that has what it calls (`art.select` / `art.status`,
/// 1.6, #630), not the newest this build knows. A lab built on a newer tree (#669's 1.7) then
/// still works with an app that speaks 1.6, and an older app is BLOCKED by name, not a FAIL.
pub const NEEDS: foxglove_plugin_api::Version = foxglove_plugin_api::Version { major: 1, minor: 6 };

/// The running app doesn't serve [`NEEDS`]: its message names the app's version (code 7).
#[derive(Debug)]
pub struct VersionMismatch(pub String);
impl std::fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} (foxglove-lab asks for API {NEEDS}): run the app built from this tree",
            self.0
        )
    }
}
impl std::error::Error for VersionMismatch {}

/// The app over its named pipe; one connection per call.
pub struct Pipe;

#[cfg(windows)]
fn call(method: &'static str, params: Value) -> Result<Value> {
    use foxglove_plugin_api::rpc::{Outcome, Request, Response};
    use std::io::{BufRead, BufReader, Write};
    let mut write = foxglove_plugin_host::pipe::connect()
        .map_err(|e| anyhow!("Foxglove's local API isn't reachable: {e}"))?;
    let mut read = BufReader::new(write.try_clone()?);
    let mut next = 0;
    let mut ask = |method: &str, params: Value| -> Result<Value> {
        next += 1;
        writeln!(
            write,
            "{}",
            serde_json::to_string(&Request::call(next, method, params))?
        )?;
        write.flush()?;
        loop {
            let mut line = String::new();
            anyhow::ensure!(read.read_line(&mut line)? > 0, "Foxglove closed the API");
            // Events can arrive between replies.
            let Ok(r) = serde_json::from_str::<Response>(&line) else {
                continue;
            };
            return match r.outcome {
                Outcome::Result(v) => Ok(v),
                Outcome::Error(e) if e.code == foxglove_plugin_api::rpc::code::VERSION => {
                    Err(VersionMismatch(e.message).into())
                }
                Outcome::Error(e) => Err(anyhow!("{method}: {} (code {})", e.message, e.code)),
            };
        }
    };
    ask(
        "initialize",
        json!({
            "api": NEEDS.to_string(),
            "client": {"name": "foxglove-lab", "version": env!("CARGO_PKG_VERSION")},
            "permissions": ["devices.read", "art.write"],
        }),
    )?;
    let devices: foxglove_plugin_api::methods::Devices =
        serde_json::from_value(ask("devices.list", json!({}))?)?;
    let device = devices
        .devices
        .first()
        .ok_or_else(|| anyhow!("the app has no keyboard with Foxglove's host link"))?
        .id
        .clone();
    let mut params = params;
    params["device"] = json!(device);
    ask(method, params)
}

#[cfg(not(windows))]
fn call(_: &'static str, _: Value) -> Result<Value> {
    anyhow::bail!("the local API is Windows-only")
}

/// `call` on a thread, answered within `timeout` (the thread ends with the process otherwise).
fn bounded(method: &'static str, params: Value, timeout: Duration) -> Result<ArtStatus> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(call(method, params));
    });
    let v = rx.recv_timeout(timeout).map_err(|_| {
        anyhow!(
            "{method}: no answer from the app in {} ms",
            timeout.as_millis()
        )
    })??;
    Ok(serde_json::from_value(v)?)
}

impl AppArt for Pipe {
    fn select_built_in(&self, label: &str, timeout: Duration) -> Result<ArtStatus> {
        bounded(
            "art.select",
            json!({"target": {"built_in": label}}),
            timeout,
        )
    }
    fn status(&self, timeout: Duration) -> Result<ArtStatus> {
        bounded("art.status", json!({}), timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_lab_asks_for_the_oldest_api_with_its_calls() {
        // art.select and art.status came in 1.6 (#630): any 1.x app from 1.6 serves the lab.
        assert_eq!(NEEDS.to_string(), "1.6");
        assert!(NEEDS.served_by(foxglove_plugin_api::API_VERSION));
        assert!(NEEDS.served_by(foxglove_plugin_api::Version { major: 1, minor: 7 }));
        let old = foxglove_plugin_api::Version { major: 1, minor: 5 };
        assert!(!NEEDS.served_by(old));
        let e = VersionMismatch("this Foxglove speaks API 1.5; the client needs 1.6".into());
        assert!(e.to_string().contains("API 1.5") && e.to_string().contains("asks for API 1.6"));
    }

    #[test]
    fn labels_come_from_the_manifest_when_the_count_matches() {
        // One built-in since #671.
        assert_eq!(built_in_label(1, 0), "Bandit emblem");
        // Other firmware (the 17 sets before #671 too): the app's own fallback names.
        assert_eq!(built_in_label(17, 0), "Set 0");
        assert_eq!(built_in_label(5, 1), "Set 1");
        assert_eq!(built_in_label(1, 40), "Set 40");
    }
}
