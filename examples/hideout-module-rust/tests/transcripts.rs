//! Replays the module side of the shared JSON-RPC transcripts (tests/hideout-vectors/transcripts,
//! #906) through the SDK, in process: no pipe, no hub.
//!
//! For each module instance in a transcript (`module`, `old-instance`, `new-instance`, `vulpes`):
//! - a message the module sends is made with the SDK's typed call for that method, and what the
//!   SDK writes must equal the transcript's message (request ids are mapped: the SDK numbers its
//!   own);
//! - a message the hub sends is fed to the SDK, which parses it into the contract's types and
//!   answers through a [`Scripted`] handler; the answer must be the transcript's next message.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use foxglove_plugin::hideout::{Incoming, RpcError, Session};
use hideout_example_module::{Counter, Scripted};
use serde_json::{json, Value};

const ROLES: &[&str] = &["module", "old-instance", "new-instance", "vulpes"];

struct Instance {
    session: Session<std::io::Empty, Vec<u8>>,
    module: Scripted,
    sent: VecDeque<Value>,
    ids: HashMap<i64, i64>,
}

impl Instance {
    fn new() -> Instance {
        Instance {
            session: Session::new(std::io::empty(), Vec::new()),
            module: Scripted::default(),
            sent: VecDeque::new(),
            ids: HashMap::new(),
        }
    }

    fn collect(&mut self) {
        let out = std::mem::take(self.session.writer_mut());
        for line in String::from_utf8(out).unwrap().lines() {
            self.sent.push_back(serde_json::from_str(line).unwrap());
        }
    }
}

/// Drops keys whose value is `null` or `[]` below the top of a params/result value: defaults a
/// serializer may write out or leave off.
fn strip(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, v)| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
                .map(|(k, v)| (k.clone(), strip(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip).collect()),
        other => other.clone(),
    }
}

fn normalize(msg: &Value) -> Value {
    let mut m = msg.as_object().unwrap().clone();
    for key in ["params", "result", "error"] {
        if let Some(v) = m.get(key).cloned() {
            m.insert(key.into(), strip(&v));
        }
    }
    Value::Object(m)
}

fn send(inst: &mut Instance, msg: &Value) -> Result<(), String> {
    let method = msg["method"].as_str().unwrap();
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let s = &mut inst.session;
    let parse = |e: serde_json::Error| format!("{method}: params don't parse: {e}");
    let res = match method {
        "initialize" => s.initialize(&serde_json::from_value(params).map_err(parse)?),
        "module.register" => s.register(&serde_json::from_value(params).map_err(parse)?),
        "ui.register" => s.ui_register(&serde_json::from_value(params).map_err(parse)?),
        "ui.asset.put" => s.asset_put(&serde_json::from_value(params).map_err(parse)?),
        "hub.notify" => s.hub_notify(&serde_json::from_value(params).map_err(parse)?),
        "channel.open" => s.channel_open(&serde_json::from_value(params).map_err(parse)?),
        "facts.set" => s.set_fact(params),
        "module.ready" => s.ready(params["instance"].as_str().unwrap()).map(|_| 0),
        "module.idle" => s.idle().map(|_| 0),
        "ui.patch" => s
            .ui_patch(&serde_json::from_value(params).map_err(parse)?)
            .map(|_| 0),
        _ if msg.get("id").is_some() => s.request(method, params),
        _ => s.notify(method, params).map(|_| 0),
    };
    res.map_err(|e| format!("{method}: {e}"))?;
    Ok(())
}

/// The module's answer to hub request `id`: its next message with that id.
fn answer(steps: &[Value], from: usize, role: &str, id: &Value) -> Option<Result<Value, RpcError>> {
    steps[from..].iter().find_map(|s| {
        let m = s.get("msg")?;
        if s["from"] != role || m.get("method").is_some() || &m["id"] != id {
            return None;
        }
        Some(match m.get("result") {
            Some(r) => Ok(r.clone()),
            None => Err(RpcError {
                code: m["error"]["code"].as_i64().unwrap(),
                message: m["error"]["message"].as_str().unwrap_or("").into(),
                data: m["error"].get("data").cloned(),
            }),
        })
    })
}

/// Replays one transcript; `Ok(false)` when it has no module side.
fn replay(path: &Path) -> Result<bool, String> {
    let t: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let steps = t["steps"].as_array().unwrap();
    let mut instances: HashMap<String, Instance> = HashMap::new();
    for (i, step) in steps.iter().enumerate() {
        let Some(msg) = step.get("msg") else { continue };
        let (from, to) = (step["from"].as_str().unwrap(), step["to"].as_str().unwrap());
        let at = |e: String| format!("step {i}: {e}");
        if ROLES.contains(&from) {
            let inst = instances.entry(from.into()).or_insert_with(Instance::new);
            if msg.get("method").is_some() {
                send(inst, msg).map_err(at)?;
            }
            inst.collect();
            let mut got = inst
                .sent
                .pop_front()
                .ok_or_else(|| at(format!("the module sent nothing; expected {msg}")))?;
            if msg.get("method").is_some() {
                if let (Some(tid), Some(sid)) = (msg["id"].as_i64(), got["id"].as_i64()) {
                    inst.ids.insert(tid, sid);
                    got["id"] = json!(tid);
                }
            }
            if normalize(&got) != normalize(msg) {
                return Err(at(format!("the module sent {got}\n  expected {msg}")));
            }
        } else if ROLES.contains(&to) {
            let inst = instances.entry(to.into()).or_insert_with(Instance::new);
            let mut msg = msg.clone();
            if msg.get("method").is_some() {
                if let Some(id) = msg.get("id") {
                    inst.module.answer = answer(steps, i, to, id);
                }
                let got = inst
                    .session
                    .handle(msg, &mut inst.module)
                    .map_err(|e| at(e.to_string()))?;
                if matches!(got, Incoming::Response { .. }) {
                    return Err(at("a hub call read as a response".into()));
                }
            } else {
                let tid = msg["id"].as_i64().unwrap();
                let sid = *inst
                    .ids
                    .get(&tid)
                    .ok_or_else(|| at(format!("no request {tid}")))?;
                msg["id"] = json!(sid);
                match inst
                    .session
                    .handle(msg, &mut inst.module)
                    .map_err(|e| at(e.to_string()))?
                {
                    Incoming::Response { .. } => {}
                    other => return Err(at(format!("an answer read as {other:?}"))),
                }
            }
        }
    }
    for (role, inst) in instances.iter_mut() {
        inst.collect();
        if let Some(extra) = inst.sent.front() {
            return Err(format!("{role} sent more than the transcript: {extra}"));
        }
        if !inst.session.pending().is_empty() {
            return Err(format!(
                "{role} has unanswered requests: {:?}",
                inst.session.pending()
            ));
        }
    }
    Ok(!instances.is_empty())
}

fn transcripts() -> Vec<PathBuf> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors/transcripts");
    let mut out = vec![];
    for dir in std::fs::read_dir(&root).unwrap() {
        let dir = dir.unwrap().path();
        if dir.is_dir() {
            for f in std::fs::read_dir(&dir).unwrap() {
                let f = f.unwrap().path();
                if f.extension().is_some_and(|e| e == "json") {
                    out.push(f);
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
fn module_side_of_every_transcript() {
    let (mut replayed, mut failures) = (0, vec![]);
    for path in transcripts() {
        match replay(&path) {
            Ok(true) => replayed += 1,
            Ok(false) => {}
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    eprintln!("replayed the module side of {replayed} transcripts");
    assert!(
        replayed >= 25,
        "only {replayed} transcripts have a module side"
    );
}

/// The sample module itself: a click patches the count, a busy count-down holds a drain, and the
/// count survives a handover.
#[test]
fn counter_module() {
    let mut s = Session::new(std::io::empty(), Vec::<u8>::new());
    let mut c = Counter::default();
    let call = |m: &str, p: Value| json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p});
    s.handle(
        call(
            "ui.event",
            json!({"panel": "counter", "node": "add", "kind": "click"}),
        ),
        &mut c,
    )
    .unwrap();
    assert_eq!(c.count, 1);
    assert!(c.patch.take().is_some());
    c.counting_down = true;
    s.handle(call("module.drain", json!({"reason": "update"})), &mut c)
        .unwrap();
    s.handle(call("state.export", json!({})), &mut c).unwrap();
    let out: Vec<Value> = String::from_utf8(std::mem::take(s.writer_mut()))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(out[1]["result"]["state"], "busy");
    let mut next = Counter::default();
    s.handle(call("state.import", out[2]["result"].clone()), &mut next)
        .unwrap();
    assert_eq!(next.count, 1);
    let panels = serde_json::to_value(next.panels("inst-1")).unwrap();
    assert_eq!(
        panels["panels"][0]["tree"]["children"][0]["props"]["text"],
        "1"
    );
}
