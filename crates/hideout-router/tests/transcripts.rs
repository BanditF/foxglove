//! Replays the hub-side steps of the #906 transcripts (tests/hideout-vectors/transcripts)
//! against the router: every `party -> hub` message is fed in, every `hub -> party` message must
//! come out, in order. The router-free parts (supervisor, swaps, safe mode, the panel host's
//! drawing, MCP, Vulpes's own GPU broker) are listed with the reason they are not run here.
//!
//! Each transcript's prose `event`s and the state it starts in (who is connected, with which
//! grant) are set up below; the wire steps are the data. Message text and `data.reason` are free
//! (README); codes, results and params are compared. Ids and nonces the hub picks are bound on
//! first sight and must stay consistent.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hideout_contract::capability::{Capability, Holder};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const ART: &str = "foxglove.example-art";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors")
}

/// Transcripts that are not router behaviour, with the reason.
const NOT_ROUTER: &[(&str, &str)] = &[
    (
        "transcripts/supervisor/",
        "supervisor (H2.4): states, backoff, rollback",
    ),
    (
        "transcripts/swap/",
        "hot swap (H2.5): drain, handover, overlap",
    ),
    ("transcripts/hub/", "hub restarts, safe mode, bisect (H2.4)"),
    ("transcripts/mcp/", "the MCP bridge (H7.1)"),
    (
        "transcripts/gpu/",
        "Vulpes's GPU broker answers these; the router only forwards",
    ),
    (
        "transcripts/modules/",
        "hideout.modules (index-only entries)",
    ),
    (
        "transcripts/lifecycle/rate-limited.json",
        "error 8 comes from the provider",
    ),
    (
        "transcripts/lifecycle/register-not-ready-in-30s.json",
        "the supervisor's ready timer",
    ),
    (
        "transcripts/ui/patch-rate.json",
        "the panel host coalesces patches",
    ),
    (
        "transcripts/ui/patch-event-asset.json",
        "the panel host's ui.event and assets",
    ),
    ("transcripts/ui/badge.json", "the panel host's tab badges"),
    (
        "transcripts/ui/visibility.json",
        "the panel host's tab visibility",
    ),
];

/// Hub-side steps the router does run although the folder is someone else's.
const ALSO: &[&str] = &[
    "transcripts/supervisor/provider-gone-error-2.json",
    "transcripts/hub/hub-restart-takes-back.json",
];

fn art() -> ModuleEntry {
    ModuleEntry {
        id: ART.into(),
        name: "Example Art".into(),
        holder: Holder::FirstParty,
        granted: [
            "facts.read:keyboard.*",
            "hub.notify",
            "provide:example.art.*",
            "channel.offer:previews",
        ]
        .map(String::from)
        .to_vec(),
        catalog_provide: Some(vec!["example.art.*".into()]),
        channel_risk: BTreeMap::from([("previews".into(), "normal".into())]),
        activate_on_call: false,
        panels: Some(vec!["art".into()]),
        settings_dir: format!("C:/example/settings/{ART}"),
        data_dir: format!("C:/example/data/{ART}"),
    }
}

fn other(id: &str, name: &str, provide: &str, channel: Option<(&str, &str)>) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: name.into(),
        holder: Holder::FirstParty,
        granted: vec![],
        catalog_provide: Some(vec![provide.into()]),
        channel_risk: channel
            .map(|(c, r)| BTreeMap::from([(c.into(), r.into())]))
            .unwrap_or_default(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
    }
}

struct Replay {
    router: Router,
    now: Instant,
    parties: HashMap<String, ConnId>,
    names: HashMap<ConnId, String>,
    inbox: HashMap<String, VecDeque<Value>>,
    closed: Vec<String>,
    events: Vec<Out>,
    /// Hub-picked values: expected (as text) to actual.
    bound: HashMap<String, Value>,
    /// Parties whose traffic the transcript leaves out.
    hidden: Vec<String>,
    tag: u64,
}

impl Replay {
    fn new(approve: Vec<&'static str>) -> Replay {
        let approved: Vec<Capability> = approve
            .iter()
            .map(|s| Capability::parse(s).unwrap())
            .collect();
        let approver = Box::new(move |_: &str, asked: &[Capability]| {
            asked
                .iter()
                .filter(|c| approved.is_empty() || approved.contains(c))
                .cloned()
                .collect()
        });
        let mut router = Router::new(Settings::default(), approver);
        router.add_module(art());
        router.set_token(ART, "tok-launch-1");
        Replay {
            router,
            now: Instant::now(),
            parties: HashMap::new(),
            names: HashMap::new(),
            inbox: HashMap::new(),
            closed: Vec::new(),
            events: Vec::new(),
            bound: HashMap::new(),
            hidden: Vec::new(),
            tag: 0,
        }
    }

    fn take(&mut self, out: Vec<Out>) {
        for o in out {
            match o {
                Out::Send(c, v) => {
                    let name = self.names[&c].clone();
                    self.inbox.entry(name).or_default().push_back(v);
                }
                Out::Close(c) => {
                    let name = self.names[&c].clone();
                    self.closed.push(name);
                    let more = self.router.disconnect(c);
                    self.take(more);
                }
                other => self.events.push(other),
            }
        }
    }

    fn conn(&mut self, party: &str) -> ConnId {
        if let Some(c) = self.parties.get(party) {
            return *c;
        }
        let c = self.router.connect(Origin::Pipe);
        self.parties.insert(party.into(), c);
        self.names.insert(c, party.into());
        c
    }

    fn send(&mut self, party: &str, msg: Value) {
        let c = self.conn(party);
        let out = self.router.receive(c, &msg.to_string(), self.now);
        self.take(out);
    }

    /// Sends and returns the answer, outside the transcript (set-up).
    fn call(&mut self, party: &str, msg: Value) -> Value {
        self.send(party, msg);
        self.inbox.get_mut(party).unwrap().pop_back().unwrap()
    }

    fn tool(&mut self, party: &str, name: &str, permissions: &[&str]) {
        let r = self.call(
            party,
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
                "client":{"name":name,"version":"0.1.0"},"hub":"1.0","api":{"foxglove":"1.13"},
                "permissions":permissions}}),
        );
        assert!(r.get("result").is_some(), "{r}");
    }

    /// A module party: initialized, then registered and ready if `methods` is given.
    fn module(&mut self, party: &str, id: &str, register: Option<Value>) {
        let token = format!("tok-{party}-{id}");
        self.router.set_token(id, &token);
        let r = self.call(
            party,
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
                "id":id,"version":"0.4.0","instance":"inst-1","token":token,"hub":"1.0",
                "api":{"foxglove":"1.13"}}}),
        );
        assert!(r.get("result").is_some(), "{r}");
        if let Some(reg) = register {
            let r = self.call(
                party,
                json!({"jsonrpc":"2.0","id":0,"method":"module.register","params":reg}),
            );
            assert_eq!(r["result"]["rejected"], json!([]), "{r}");
            self.inbox.remove(party);
            // Calls queued for this module (activation by call) arrive after ready.
            self.send(
                party,
                json!({"jsonrpc":"2.0","method":"module.ready","params":{"instance":"inst-1"}}),
            );
        } else {
            self.inbox.remove(party);
        }
    }

    /// Replaces hub-picked values the party echoes back with the real ones.
    fn bind_back(&self, msg: &mut Value) {
        if msg.get("method").is_none() {
            if let Some(v) = self.bound.get(&format!("id:{}", msg["id"])) {
                msg["id"] = v.clone();
            }
        }
        if let Some(s) = msg.pointer_mut("/params/subscription") {
            if let Some(v) = self.bound.get(&format!("sub:{s}")) {
                *s = v.clone();
            }
        }
    }

    fn matches(&mut self, path: &str, want: &Value, got: &Value) -> Result<(), String> {
        let bind = match path {
            "/id" if got.get("method").is_some() => Some("id"),
            _ if path.ends_with("/subscription") => Some("sub"),
            _ if path.ends_with("/nonce") => Some("nonce"),
            _ => None,
        };
        if let (Some(kind), Some(g)) = (bind, got.pointer(path)) {
            let key = format!("{kind}:{}", want.pointer(path).unwrap());
            let g = g.clone();
            return match self.bound.get(&key) {
                Some(v) if *v != g => Err(format!("{path}: bound to {v}, got {g}")),
                Some(_) => Ok(()),
                None => {
                    self.bound.insert(key, g);
                    Ok(())
                }
            };
        }
        let (w, g) = (want.pointer(path).unwrap(), got.pointer(path));
        match (w, g) {
            (Value::Object(wm), Some(Value::Object(gm))) => {
                // Message text and refusal reasons are free.
                let free = |k: &str| path.ends_with("/error") && (k == "message" || k == "data");
                for k in wm.keys().chain(gm.keys()) {
                    if free(k) {
                        continue;
                    }
                    let sub = format!("{path}/{k}");
                    if wm.contains_key(k) != gm.contains_key(k) {
                        return Err(format!("{sub}: want {:?}, got {:?}", wm.get(k), gm.get(k)));
                    }
                    self.matches(&sub, want, got)?;
                }
                Ok(())
            }
            (Value::Array(wa), Some(Value::Array(ga))) if path.ends_with("/granted") => {
                let sorted = |a: &Vec<Value>| {
                    let mut v: Vec<String> = a.iter().map(|x| x.to_string()).collect();
                    v.sort();
                    v
                };
                if sorted(wa) == sorted(ga) {
                    Ok(())
                } else {
                    Err(format!("{path}: want {w}, got {g:?}"))
                }
            }
            (Value::Array(wa), Some(Value::Array(ga))) if wa.len() == ga.len() => {
                for i in 0..wa.len() {
                    self.matches(&format!("{path}/{i}"), want, got)?;
                }
                Ok(())
            }
            (w, Some(g)) if w == g => Ok(()),
            _ => Err(format!("{path}: want {w}, got {g:?}")),
        }
    }

    fn expect(&mut self, party: &str, want: &Value) -> Result<(), String> {
        let got = self.inbox.get_mut(party).and_then(VecDeque::pop_front);
        let got = match got {
            Some(g) => g,
            // A call the hub itself makes (settings, actions, windows): the hub is the caller.
            None if want.get("method").is_some() && want.get("id").is_some() => {
                let module = match party {
                    "module" => ART,
                    p => p,
                };
                self.tag += 1;
                let out = self.router.call_module(
                    module,
                    want["method"].as_str().unwrap(),
                    want["params"].clone(),
                    self.tag,
                    self.now,
                );
                self.take(out);
                self.inbox
                    .get_mut(party)
                    .and_then(VecDeque::pop_front)
                    .ok_or("the hub's call was not sent")?
            }
            None => return Err(format!("nothing sent to {party}")),
        };
        // Compare from the root, so `/id` binding sees the whole message.
        let mut want_root = want.clone();
        if let Some(o) = want_root.as_object_mut() {
            o.retain(|k, _| k != "why");
        }
        self.matches("", &want_root, &got)
            .map_err(|e| format!("{e}\n  want {want}\n  got  {got}"))
    }
}

/// Starting state and the handling of each transcript's prose events.
fn setup(name: &str) -> Replay {
    match name {
        "initialize-tool" => Replay::new(vec!["facts.read:keyboard.*"]),
        "channel-open-denied" => {
            let mut r = Replay::new(vec![]);
            r.router.add_module(other(
                "hideout.example-lights",
                "Example Lights",
                "example.lights.*",
                Some(("keys-feed", "keys")),
            ));
            r.tool("tool", "example-cli", &[]);
            r.tool(
                "mcp:example-agent",
                "mcp:example-agent",
                &["channel.open:hideout.example-lights/keys-feed"],
            );
            r
        }
        "channel-open-tool" => {
            let mut r = Replay::new(vec![]);
            r.module(
                "module",
                ART,
                Some(json!({"methods":[],"facts":[],"channels":["previews"],"apis":{}})),
            );
            r.tool(
                "tool",
                "example-cli",
                &["channel.open:foxglove.example-art/previews"],
            );
            r
        }
        "panel-stream-grant" | "notify" | "action-invoke" | "window-open" | "settings-get-set" => {
            let mut r = Replay::new(vec![]);
            r.module("module", ART, Some(json!({"methods":["example.art.*"],"facts":[],"channels":["previews"],"apis":{}})));
            r
        }
        "duplicate-request-id" | "late-reply-after-timeout" | "call-timeout" => {
            let mut r = Replay::new(vec![]);
            r.module(
                "provider",
                ART,
                Some(json!({"methods":["example.art.*"],"facts":[],"channels":[],"apis":{}})),
            );
            r.hidden.push("provider".into());
            r.tool("tool", "example-cli", &[]);
            r
        }
        "oversized-line" => {
            let mut r = Replay::new(vec![]);
            r.module("peer", ART, None);
            r
        }
        "hub-state-facts" => {
            let mut r = Replay::new(vec![]);
            r.module(
                "provider",
                ART,
                Some(json!({"methods":[],"facts":[],"channels":[],"apis":{}})),
            );
            r.tool("tool", "example-cli", &["facts.read:*"]);
            r
        }
        "set-subscribe-changed" => {
            let mut r = Replay::new(vec![]);
            r.module(
                "module",
                ART,
                Some(json!({"methods":[],"facts":["example.art.*"],"channels":[],"apis":{}})),
            );
            r.tool("tool", "example-cli", &["facts.read:example.art.*"]);
            r
        }
        "set-denied" | "channel-offer-rejected" | "register-undeclared-panel" => {
            let mut r = Replay::new(vec![]);
            r.module("module", ART, None);
            r
        }
        "activation-by-call" => {
            let mut r = Replay::new(vec![]);
            let mut a = art();
            a.activate_on_call = true;
            r.router.add_module(a);
            r.tool("tool", "example-cli", &[]);
            r
        }
        "provider-gone-error-2" => {
            let mut r = Replay::new(vec![]);
            r.router.add_module(other(
                "foxglove.example-link",
                "Example Link",
                "example.link.*",
                None,
            ));
            r.module("module", ART, None);
            r
        }
        "re-registration-restart" => {
            let mut r = Replay::new(vec![]);
            r.router.set_token(ART, "tok-launch-2");
            r
        }
        _ => {
            let mut r = Replay::new(vec![]);
            r.tool("tool", "example-cli", &[]);
            r
        }
    }
}

fn event(r: &mut Replay, name: &str, text: &str) -> Result<(), String> {
    match (name, text) {
        (_, "hub closes the connection") => {
            if r.closed.is_empty() {
                return Err("the connection was not closed".into());
            }
        }
        (_, t) if t.starts_with("30 seconds pass") => {
            r.now += Duration::from_secs(30);
            let out = r.router.tick(r.now);
            r.take(out);
        }
        ("activation-by-call", t) if t.starts_with("hub launches") => {
            if !r.events.contains(&Out::Activate(ART.into())) {
                return Err("the module was not activated".into());
            }
            r.module(
                "module",
                ART,
                Some(json!({"methods":["example.art.*"],"facts":[],"channels":[],"apis":{}})),
            );
        }
        ("activation-by-call", _) => {}
        ("panel-stream-grant", t) if t.starts_with("panel visible") => {
            let out = r.router.open_host_channel(ART, "previews", r.now);
            r.take(out);
        }
        ("panel-stream-grant", t) if t.starts_with("hub returns") => {
            let ok = r.events.iter().any(|e| {
                matches!(e, Out::HostChannel { result: Ok(v), .. } if v["pipe"].is_string() && v["nonce"].is_string())
            });
            if !ok {
                return Err(format!("the panel host got no pipe: {:?}", r.events));
            }
        }
        ("notify", t) if t.starts_with("variant") => {
            let mut a = art();
            a.granted.retain(|g| g != "hub.notify");
            r.router.add_module(a);
            let c = r.parties.remove("module").unwrap();
            let out = r.router.disconnect(c);
            r.take(out);
            r.module("module", ART, None);
        }
        ("duplicate-request-id", "module foxglove.example-art answers the first call") => {
            answer_forwarded(r, json!({"status": "ok"}))?;
        }
        ("late-reply-after-timeout", t) if t.starts_with("the module sends its result") => {
            answer_forwarded(r, json!({"status": "ok"}))?;
        }
        ("malformed-json", t) if t.starts_with("the tool sends the line ") => {
            let line = &t["the tool sends the line ".len()..];
            let c = r.conn("tool");
            let out = r.router.receive(c, line, r.now);
            r.take(out);
        }
        ("oversized-line", t) if t.starts_with("a peer on module") => {
            let c = r.conn("peer");
            let out = r.router.receive(c, &"x".repeat(300 * 1024), r.now);
            r.take(out);
        }
        ("oversized-line", "hub closes the channel") if !r.closed.iter().any(|c| c == "peer") => {
            return Err("the channel was not closed".into());
        }
        _ => {}
    }
    Ok(())
}

/// The provider answers the call the hub forwarded to it (the oldest one it still holds).
fn answer_forwarded(r: &mut Replay, result: Value) -> Result<(), String> {
    let req = r
        .inbox
        .get_mut("provider")
        .and_then(VecDeque::pop_front)
        .ok_or("nothing was forwarded to the provider")?;
    r.send(
        "provider",
        json!({"jsonrpc":"2.0","id":req["id"],"result":result}),
    );
    Ok(())
}

fn replay(name: &str, doc: &Value) -> Result<usize, String> {
    let mut r = setup(name);
    let mut hub_steps = 0;
    for (i, step) in doc["steps"].as_array().unwrap().iter().enumerate() {
        let at = |e: String| format!("step {i}: {e}");
        if let Some(text) = step.get("event").and_then(Value::as_str) {
            event(&mut r, name, text).map_err(at)?;
            continue;
        }
        if let Some(e) = step.get("expect") {
            if let Some(n) = e["supervisor_crashes_counted"].as_u64() {
                let got = r.router.module_faults(ART);
                if u64::from(got) != n {
                    return Err(at(format!("crashes counted: want {n}, got {got}")));
                }
                if !r.events.contains(&Out::ModuleFault(ART.into())) {
                    return Err(at("no ModuleFault event".into()));
                }
            }
            continue;
        }
        let Some(msg) = step.get("msg") else { continue };
        let (from, to) = (step["from"].as_str().unwrap(), step["to"].as_str().unwrap());
        if to == "hub" {
            let mut m = msg.clone();
            r.bind_back(&mut m);
            r.send(from, m);
        } else {
            r.expect(to, msg).map_err(at)?;
            hub_steps += 1;
        }
    }
    for (party, left) in &r.inbox {
        if !left.is_empty() && !r.hidden.contains(party) {
            return Err(format!("unexpected messages to {party}: {left:?}"));
        }
    }
    Ok(hub_steps)
}

#[test]
fn hub_side_transcript_steps_pass() {
    let index: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("index.json")).unwrap()).unwrap();
    let mut passed = Vec::new();
    let mut skipped: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failed = Vec::new();
    let mut steps = 0;
    for key in index["vectors"].as_object().unwrap().keys() {
        if !key.starts_with("transcripts/") {
            continue;
        }
        let not_router = NOT_ROUTER.iter().find(|(p, _)| key.starts_with(p));
        if let (Some((_, why)), false) = (not_router, ALSO.contains(&key.as_str())) {
            *skipped.entry(why).or_default() += 1;
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root().join(key)) else {
            *skipped.entry("index-only entry").or_default() += 1;
            continue;
        };
        let doc: Value = serde_json::from_str(&text).unwrap();
        let name = doc["name"].as_str().unwrap();
        match replay(name, &doc) {
            Ok(n) => {
                steps += n;
                passed.push(key.clone());
            }
            Err(e) => failed.push(format!("{key}: {e}")),
        }
    }
    println!(
        "{} transcripts, {steps} hub-side steps passed",
        passed.len()
    );
    for p in &passed {
        println!("  ok {p}");
    }
    for (why, n) in &skipped {
        println!("skipped {n}: {why}");
    }
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
    assert!(passed.len() >= 20, "only {} transcripts ran", passed.len());
}
