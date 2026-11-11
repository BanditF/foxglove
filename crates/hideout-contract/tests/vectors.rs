//! Runs every file in tests/hideout-vectors/ against the crate and matches index.json (#919).
//!
//! Skipped, with the reason recorded:
//! - `trust/*`: root, catalog and archive verification (minisign signatures, epochs, rollback,
//!   `seen` state) is the trust layer's job, not this types-only crate's. `plugin-trust` verifies
//!   today's plugin catalogs, not the Hideout root chain; `hideout-registry` runs the trust and
//!   config vectors (#933).
//! - `transcripts/*`: the step order and `final` assertions are hub behaviour (routing, the
//!   supervisor, swaps). Every wire message in them is still checked here: JSON-RPC 2.0
//!   envelope and the §8.8 params of each message this crate types.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hideout_contract::capability::{check_grantable, Capability, Holder, Source};
use hideout_contract::channel::{is_channel_pipe_in, read_stream};
use hideout_contract::config::{Config, Windows};
use hideout_contract::frame::{decode_wire, Frame};
use hideout_contract::messages::{check_hub_fact, check_message};
use hideout_contract::module::{read_dir, Context, Module};
use hideout_contract::panel::{parse_message, register_with, Register, LIMITS};
use hideout_contract::Version;
use serde_json::{json, Value};

/// The Foxglove API a legacy manifest is checked against.
const APP: Version = Version {
    major: 1,
    minor: 14,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors")
}

#[derive(Default)]
struct Tally {
    passed: usize,
    skipped: BTreeMap<String, usize>,
    failed: Vec<String>,
    messages_checked: usize,
}

impl Tally {
    fn check(&mut self, key: &str, outcome: Result<(), String>) {
        match outcome {
            Ok(()) => self.passed += 1,
            Err(e) => self.failed.push(format!("{key}: {e}")),
        }
    }
}

fn outcome(expected: &Value, got: Result<Value, String>) -> Result<(), String> {
    let want = expected["result"].as_str().unwrap_or_default();
    match (want, got) {
        ("valid", Ok(fields)) => {
            for name in ["normalized", "reads_as"] {
                if let Some(Value::Object(exp)) = expected.get(name) {
                    for (k, v) in exp {
                        let g = &fields[k];
                        let same = match (v, g) {
                            (Value::Object(e), Value::Object(g)) => {
                                e.iter().all(|(ek, ev)| g.get(ek) == Some(ev))
                            }
                            _ => v == g,
                        };
                        if !same {
                            return Err(format!("{name}.{k}: want {v}, got {g}"));
                        }
                    }
                }
            }
            Ok(())
        }
        ("valid", Err(e)) => Err(format!("want valid, got {e}")),
        (_, Ok(_)) => Err(format!("want {}, got valid", expected["id"])),
        (_, Err(e)) => {
            let id = expected["id"].as_str().unwrap_or_default();
            if e.starts_with(&format!("{id}:")) {
                Ok(())
            } else {
                Err(format!("want {id}, got {e}"))
            }
        }
    }
}

fn manifest(path: &str, file: &Path, expected: &Value) -> Result<Value, String> {
    let ctx = Context {
        private_ids: expected["context"]["private_ids"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let err = |e: hideout_contract::Error| e.to_string();
    if path.ends_with('/') {
        let (file, m) = read_dir(file, &ctx, APP).map_err(|e| e.to_string())?;
        let m = m.map_err(err)?;
        if expected["reads"].as_str() != Some(file.name())
            || expected["id"].as_str() != Some(m.id.as_str())
        {
            return Err(format!("read {} ({})", file.name(), m.id));
        }
        return Ok(m.normalized());
    }
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let m = if path.ends_with(".plugin.toml") || path.ends_with(".kit.toml") {
        Module::parse_legacy(&text, APP)
    } else {
        Module::parse(&text, &ctx)
    };
    m.map(|m| m.normalized()).map_err(err)
}

fn panels(file: &Path, expected: &Value) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let msg = match parse_message(&text) {
        Ok(msg) => msg,
        // A message over the nesting cap is refused whole, with the expected id.
        Err(e) if expected["result"] == "invalid" => {
            let id = expected["id"].as_str().unwrap_or_default();
            return if e.starts_with(&format!("{id}:")) {
                Ok(())
            } else {
                Err(format!("want {id}, got {e}"))
            };
        }
        Err(e) => return Err(e),
    };
    check_message(&msg)?;
    let params: Register =
        serde_json::from_value(msg["params"].clone()).map_err(|e| e.to_string())?;
    // The module's [[actions]] ids, when the vector gives them (spec v1.4, `ui.action`).
    let actions: Option<Vec<String>> = expected["context"]["actions"].as_array().map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    });
    let (ok, rejected) = register_with(&params, None, actions.as_deref(), &LIMITS);
    let ok: Vec<&str> = ok.iter().map(|p| p.id.as_str()).collect();
    let rejected = serde_json::to_value(&rejected).unwrap();
    match expected["result"].as_str() {
        Some("valid") if rejected == json!([]) => Ok(()),
        Some("invalid")
            if rejected.as_array().is_some_and(|r| {
                !r.is_empty() && r.iter().all(|r| r["error"] == expected["id"])
            }) =>
        {
            Ok(())
        }
        Some("partial")
            if rejected == expected["rejected"] && json!(ok) == expected["registered"] =>
        {
            Ok(())
        }
        _ => Err(format!("registered {ok:?}, rejected {rejected}")),
    }
}

fn frame_row(row: &Value) -> Result<(), String> {
    let hex = row["wire_hex"].as_str().unwrap_or_default();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let exp = &row["expect"];
    let got = decode_wire(&bytes);
    let fields = match &got {
        Err(_) => json!({"accept": false}),
        Ok(Frame::Image {
            seq,
            width,
            height,
            rgba,
        }) => {
            json!({"accept": true, "kind": "image", "width": width, "height": height, "seq": seq, "pixels_rgba": rgba})
        }
        Ok(Frame::Canvas { seq, colours }) => {
            json!({"accept": true, "kind": "canvas", "count": colours.len(), "seq": seq, "colours_rgba_by_shape_order": colours})
        }
        Ok(Frame::Text { seq, append }) => {
            json!({"accept": true, "kind": "text", "seq": seq, "append": append})
        }
    };
    for (k, v) in exp.as_object().unwrap() {
        if k != "reason" && &fields[k] != v {
            return Err(format!("{k}: want {v}, got {} ({got:?})", fields[k]));
        }
    }
    Ok(())
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// Every row of `channels/channel-wire.json`, as (name, outcome).
fn channel_rows(doc: &Value) -> Vec<(String, Result<(), String>)> {
    let nonce = doc["nonce"].as_str().unwrap();
    let mut rows = Vec::new();
    for row in doc["streams"].as_array().unwrap() {
        let (frames, end) = read_stream(&unhex(row["hex"].as_str().unwrap()), nonce);
        let got = json!({
            "frames_hex": frames.iter().map(|f| f.iter().map(|b| format!("{b:02x}")).collect::<String>()).collect::<Vec<_>>(),
            "end": end.as_str(),
        });
        let result = if got == row["expect"] {
            Ok(())
        } else {
            Err(format!("want {}, got {got}", row["expect"]))
        };
        rows.push((row["name"].as_str().unwrap().to_string(), result));
    }
    for row in doc["pipes"].as_array().unwrap() {
        let pipe = row["pipe"].as_str().unwrap();
        let want = row["result"] == "valid";
        // The reader's session, as the table states it (#1092 review).
        let session = doc["session"].as_u64().unwrap() as u32;
        let result = if is_channel_pipe_in(pipe, session) == want {
            Ok(())
        } else {
            Err(format!("want {}", row["result"]))
        };
        rows.push((format!("pipe:{pipe}"), result));
    }
    rows
}

fn holder(ctx: &Value) -> Option<Holder> {
    let source = || match (ctx["source"].as_str(), ctx["anchor"].as_str()) {
        (Some("sideload"), _) => Source::Sideload,
        (Some("unsigned"), _) => Source::Unsigned,
        (_, Some("user")) => Source::UserKey,
        _ => Source::Catalog,
    };
    Some(match ctx["holder"].as_str()? {
        "third-party module" => Holder::ThirdParty(source()),
        "private module" => Holder::Private(source()),
        "MCP client" => Holder::Mcp,
        "hideout.modules" => Holder::ModulesModule,
        "first-party hideout CLI" => Holder::HideoutCli,
        other => panic!("unmapped holder {other}"),
    })
}

fn permission_row(row: &Value) -> Result<(), String> {
    let text = row["permission"].as_str().unwrap();
    let got = Capability::parse(text).and_then(|cap| match holder(&row["context"]) {
        Some(h) => check_grantable(&cap, h).map(|_| cap),
        None => Ok(cap),
    });
    let want_ok = row["result"] == "valid";
    match got {
        Ok(cap) if want_ok && cap.to_string() == text => Ok(()),
        Err(e) if !want_ok && row["id"] == e.id() => Ok(()),
        other => Err(format!(
            "want {} {}, got {other:?}",
            row["result"], row["id"]
        )),
    }
}

fn config(file: &Path, expected: &Value) -> Result<Value, String> {
    if file
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("windows.")
    {
        // `[windows]` (§2.5, spec v1.6): read on its own; the sources aren't looked at.
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let w = Windows::parse(&text).map_err(|e| e.to_string())?;
        w.validate().map_err(|e| e.to_string())?;
        return Ok(json!({ "pages": w.windows.len() }));
    }
    let ctx = &expected["context"];
    let families = match ctx.get("root_scopes") {
        Some(s) => s.clone(),
        None => {
            let root = root().join(ctx["root"].as_str().unwrap());
            let root: Value =
                serde_json::from_str(&std::fs::read_to_string(root).unwrap()).unwrap();
            root["families"].clone()
        }
    };
    let scopes: BTreeMap<String, Vec<String>> = families
        .as_object()
        .unwrap()
        .iter()
        .map(|(f, v)| {
            (
                f.clone(),
                serde_json::from_value(v["scope"].clone()).unwrap(),
            )
        })
        .collect();
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let cfg = Config::parse(&text).map_err(|e| e.to_string())?;
    cfg.validate(&scopes).map_err(|e| e.to_string())?;
    Ok(json!({}))
}

fn transcript(t: &mut Tally, key: &str, file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else {
        *t.skipped
            .entry("transcripts: index-only entry, hub behaviour (no file)".into())
            .or_default() += 1;
        return;
    };
    let doc = parse_message(&text).unwrap();
    let mut bad = Vec::new();
    for (i, step) in doc["steps"].as_array().into_iter().flatten().enumerate() {
        if let Some(msg) = step.get("msg") {
            t.messages_checked += 1;
            if let Err(e) = check_message(msg) {
                bad.push(format!("step {i}: {e}"));
            }
        }
    }
    if !bad.is_empty() {
        t.failed.push(format!("{key}: {}", bad.join("; ")));
    }
    *t.skipped
        .entry("transcripts: final assertions are hub behaviour (messages checked)".into())
        .or_default() += 1;
}

#[test]
fn every_vector_matches_the_index() {
    let index: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("index.json")).unwrap()).unwrap();
    let vectors = index["vectors"].as_object().unwrap();
    let mut t = Tally::default();
    for (key, expected) in vectors {
        let file = root().join(key);
        match key.split('/').next().unwrap() {
            "manifests" | "legacy" => {
                t.check(key, outcome(expected, manifest(key, &file, expected)))
            }
            "panels" => t.check(key, panels(&file, expected)),
            "config" => t.check(key, outcome(expected, config(&file, expected))),
            "frames" | "permissions" => {
                let doc: Value =
                    serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
                let rows = doc
                    .get("frames")
                    .or(doc.get("vectors"))
                    .and_then(|r| r.as_array())
                    .unwrap();
                for row in rows {
                    let (name, result) = match key.as_str() {
                        "frames/panel-frames.json" => {
                            (row["name"].as_str().unwrap(), frame_row(row))
                        }
                        _ => (row["permission"].as_str().unwrap(), permission_row(row)),
                    };
                    let ctx = row
                        .get("context")
                        .map(|c| format!(" {c}"))
                        .unwrap_or_default();
                    let row_key = format!("{key}#{name}");
                    t.check(&row_key, result.map_err(|e| format!("{e}{ctx}")));
                }
                if let Some(sensitive) = doc.get("sensitive").and_then(|s| s.as_array()) {
                    for s in sensitive.iter().filter_map(|s| s.as_str()) {
                        let cap = Capability::parse(s)
                            .or_else(|_| Capability::parse(&s.replace('*', "example")));
                        let ok = cap.as_ref().is_ok_and(|c| c.sensitive());
                        t.check(
                            &format!("{key}#sensitive:{s}"),
                            if ok {
                                Ok(())
                            } else {
                                Err(format!("{cap:?} not sensitive"))
                            },
                        );
                    }
                }
            }
            // The supervisor's facts (spec v1.4): rows of {fact, value, result}.
            "facts" => {
                let doc: Value =
                    serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
                for row in doc["vectors"].as_array().unwrap() {
                    let fact = row["fact"].as_str().unwrap();
                    let got = check_hub_fact(fact, &row["value"]);
                    let result = match (row["result"].as_str(), got) {
                        (Some("valid"), Ok(())) | (Some("invalid"), Err(_)) => Ok(()),
                        (want, got) => Err(format!("want {want:?}, got {got:?}")),
                    };
                    t.check(&format!("{key}#{fact}={}", row["value"]), result);
                }
            }
            // The channel wire (v1.4, #977): streams as the provider reads them, and pipe names.
            "channels" => {
                let doc: Value =
                    serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
                for (name, result) in channel_rows(&doc) {
                    t.check(&format!("{key}#{name}"), result);
                }
            }
            "transcripts" => transcript(&mut t, key, &file),
            "trust" => {
                *t.skipped
                    .entry("trust: signature and catalog verification is not in this crate".into())
                    .or_default() += 1
            }
            other => t.failed.push(format!("{key}: no reader for {other}")),
        }
    }
    println!(
        "passed {} checks; {} wire messages checked",
        t.passed, t.messages_checked
    );
    for (why, n) in &t.skipped {
        println!("skipped {n}: {why}");
    }
    assert!(
        t.failed.is_empty(),
        "{} failed:\n{}",
        t.failed.len(),
        t.failed.join("\n")
    );
}
