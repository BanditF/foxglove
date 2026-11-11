//! Replays the hub-side steps of the #906 transcripts (tests/hideout-vectors/transcripts)
//! against the router: every `party -> hub` message is fed in, every `hub -> party` message must
//! come out, in order. The router-free parts (supervisor, swaps, safe mode, the panel host's
//! drawing, MCP, Vulpes's own GPU broker) are listed with the reason they are not run here.
//!
//! The stream transcripts (H2.9) run the router together with the hub's panel host
//! (`hideout-panels`): it sends `ui.visibility`, opens and reopens streams through the router,
//! takes the module's patches and the frames the prose describes.
//!
//! Each transcript's prose `event`s and the state it starts in (who is connected, with which
//! grant) are set up below; the wire steps are the data. Message text and `data.reason` are free
//! (README); codes, results and params are compared. Ids and nonces the hub picks are bound on
//! first sight and must stay consistent.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hideout_contract::capability::{Capability, Holder};
use hideout_contract::frame::{encode, Frame};
use hideout_contract::messages::UiPatch;
use hideout_panels::streams::{Look, Phase, StreamAct, StreamCommand, StreamKey};
use hideout_panels::{PanelHost, PanelKey};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const ART: &str = "foxglove.example-art";
const SCENES: &str = "foxglove.example-scenes";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors")
}

/// Transcripts that are not router behaviour, with the reason.
const NOT_ROUTER: &[(&str, &str)] = &[
    (
        "transcripts/supervisor/",
        "supervisor (H2.5): run by crates/hideout-supervisor/tests/transcripts.rs",
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
        "the supervisor's ready timer: run by crates/hideout-supervisor/tests/transcripts.rs",
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
        "transcripts/net/stream-refused.json",
        "the network broker answers net.stream: run by crates/hideout-services/tests/net.rs",
    ),
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
    /// The hub's panel host, for the stream transcripts.
    host: Option<PanelHost>,
    /// The channel generation each stream node was granted last.
    granted: HashMap<String, u64>,
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
            host: None,
            granted: HashMap::new(),
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
                Out::HostChannel {
                    module,
                    channel,
                    result,
                } if self.host.is_some() => {
                    let host = self.host.as_mut().unwrap();
                    if let Some((key, gen, _, _)) =
                        host.streams.granted(&module, &channel, result.clone())
                    {
                        self.granted.insert(key.node, gen);
                    }
                    self.events.push(Out::HostChannel {
                        module,
                        channel,
                        result,
                    });
                }
                Out::Patch { module, params } if self.host.is_some() => {
                    let patch: UiPatch = serde_json::from_value(params).unwrap();
                    let host = self.host.as_mut().unwrap();
                    host.patch(&module, &patch, self.now).unwrap();
                    // A patch that removes a stream closes it (no pipe here to close).
                    host.sync_streams(self.now);
                    self.host_opens();
                }
                Out::AssetPut {
                    module,
                    call,
                    params,
                } if self.host.is_some() => {
                    let put = serde_json::from_value(params).unwrap();
                    let host = self.host.as_mut().unwrap();
                    let result = host.assets.put(&put).map(|()| json!({}));
                    assert!(module == ART);
                    assert_eq!(result.is_ok(), host.assets.get(&put.hash).is_some());
                    let more = self.router.host_answer(call, result);
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

    /// The panel host registers one page panel of the module with `tree`.
    fn host_page(&mut self, panel: &str, tree: Value) {
        let mut host = PanelHost::default();
        let reg = serde_json::from_value(json!({"instance": "inst-1", "panels": [
            {"id": panel, "slot": "page", "title": "Page", "icon": "x", "tree": tree}]}))
        .unwrap();
        assert!(host.register(ART, "Example Art", None, &reg).is_empty());
        self.host = Some(host);
    }

    /// The panel's tab turns `visible` or `hidden`: the panel host's `ui.visibility` goes to
    /// the module (when `tell`), then its stream opens or closes.
    fn host_show(&mut self, panel: &str, visible: bool, tell: bool) {
        let host = self.host.as_mut().unwrap();
        let key = PanelKey::new(ART, panel);
        host.set_visible(if visible {
            std::slice::from_ref(&key)
        } else {
            &[]
        });
        let notes = host.take_visibility();
        host.sync_streams(self.now);
        if tell {
            let c = self.conn("module");
            let out = notes
                .into_iter()
                .map(|(_, v)| {
                    Out::Send(
                        c,
                        json!({"jsonrpc": "2.0", "method": "ui.visibility",
                               "params": serde_json::to_value(v).unwrap()}),
                    )
                })
                .collect();
            self.take(out);
        }
        self.host_opens();
    }

    /// The panel host's stream opens go through the router (closes need no router).
    fn host_opens(&mut self) {
        let cmds = self.host.as_mut().unwrap().streams.take_commands();
        for c in cmds {
            if let StreamCommand::Open {
                module, channel, ..
            } = c
            {
                let out = self.router.open_host_channel(&module, &channel, self.now);
                self.take(out);
            }
        }
    }

    /// A frame on the stream of `node`, as its pipe would deliver it.
    fn host_frame(&mut self, panel: &str, node: &str, frame: Frame) {
        let gen = self.granted[node];
        let host = self.host.as_mut().unwrap();
        let key = StreamKey::new(PanelKey::new(ART, panel), node);
        let wire = encode(&frame);
        assert!(host
            .streams
            .frame(&key, gen, &wire[4..], self.now)
            .is_none());
    }

    /// What a streamed text node shows: its `text` and what the stream appended.
    fn host_text(&self, panel: &str, node: &str) -> String {
        let host = self.host.as_ref().unwrap();
        let key = PanelKey::new(ART, panel);
        let tree = &host.panel(&key).unwrap().tree;
        let n = tree.children.iter().find(|n| n.id == node).unwrap();
        let mut t = n
            .props
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        t.push_str(&host.streams.get(&StreamKey::new(key, node)).unwrap().text);
        t
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
            // The panel host's process id (v1.10, #1092): the hub's, not the transcript's.
            _ if path.ends_with("/consumer_pid") => Some("pid"),
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
                // Message text and refusal reasons are free; a refusal's §15 id (`data.id`) is
                // checked when the transcript names one (v1.12).
                let free = |k: &str| path.ends_with("/error") && (k == "message" || k == "data");
                for k in wm.keys().chain(gm.keys()) {
                    if free(k) {
                        if k == "data" {
                            let id = format!("{path}/data/id");
                            if let Some(w) = want.pointer(&id) {
                                if got.pointer(&id) != Some(w) {
                                    return Err(format!(
                                        "{id}: want {w}, got {:?}",
                                        got.pointer(&id)
                                    ));
                                }
                            }
                        }
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
            // The panel host's `ui.visibility`: the tab the transcript implies changed.
            None if want["method"] == "ui.visibility" && self.host.is_some() => {
                let panel = want["params"]["panel"].as_str().unwrap();
                self.host_show(panel, want["params"]["state"] == "visible", true);
                self.inbox
                    .get_mut(party)
                    .and_then(VecDeque::pop_front)
                    .ok_or("the panel host sent no ui.visibility")?
            }
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
        // The host services' refusals (§8.10, §8.11, v1.12): the router checks the grant.
        "open-refused" | "set-mute-refused" => {
            let mut r = Replay::new(vec![]);
            let mut a = art();
            a.granted.extend(
                [
                    "net.connect:127.0.0.1:4455",
                    "net.connect:*.example.test:*",
                    "net.listen:udp:127.0.0.1:5568",
                    "facts.write:mic.*",
                ]
                .map(String::from),
            );
            r.router.add_module(a);
            r.router.serve(&hideout_router::host_methods());
            r.module("module", ART, None);
            r
        }
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
        "channel-open-tool" | "channel-create-bad-pipe" => {
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
        "text-restore-on-visible" | "reopen-then-stopped" => {
            let mut r = Replay::new(vec![]);
            let mut a = art();
            a.granted.push("channel.offer:log".into());
            a.channel_risk.insert("log".into(), "normal".into());
            r.router.add_module(a);
            r.module(
                "module",
                ART,
                Some(json!({"methods":[],"facts":[],"channels":["previews","log"],"apis":{}})),
            );
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
        "lease-closed-disconnect" | "lease-closed-released" | "lease-addressed-event" => {
            let mut r = Replay::new(vec![]);
            r.module(
                "module",
                ART,
                Some(json!({"methods":["example.art.*"],"facts":[],"channels":[],"apis":{}})),
            );
            r.tool("tool", "example-cli", &[]);
            r.tool("other", "second-cli", &[]);
            r
        }
        "lease-closed-revoked" => {
            let mut r = Replay::new(vec![]);
            r.router
                .add_module(other(SCENES, "Example Scenes", "example.scenes.*", None));
            r.module(
                "module",
                ART,
                Some(json!({"methods":["example.art.*"],"facts":[],"channels":[],"apis":{}})),
            );
            r.module(
                "consumer",
                SCENES,
                Some(json!({"methods":[],"facts":[],"channels":[],"apis":{}})),
            );
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
        // The panel host answers `ui.asset.put` (v1.7): its answer goes back to the module.
        "asset-put-refused" => {
            let mut r = Replay::new(vec![]);
            r.module("module", ART, None);
            r.host_page(
                "art",
                json!({"type": "column", "id": "root", "children": [
                    {"type": "image", "id": "shot", "props": {"hash": "a".repeat(64), "label": "Art"}}]}),
            );
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
        (_, "the tool's connection ends") => {
            let c = r.parties.remove("tool").unwrap();
            let out = r.router.disconnect(c);
            r.take(out);
        }
        ("lease-closed-revoked", t) if t.starts_with("an accepted catalog revokes") => {
            let out = r.router.revoke(SCENES, "0.4.0", "placeholder reason");
            r.take(out);
            let key = format!("module.{SCENES}.revoked");
            let want = json!({"version": "0.4.0", "reason": "placeholder reason"});
            if r.router.fact(&key) != Some(&want) {
                return Err(format!("{key} is {:?}", r.router.fact(&key)));
            }
        }
        ("lease-closed-revoked", "the consumer's connection ends") => {
            let c = r.parties.remove("consumer").unwrap();
            let out = r.router.disconnect(c);
            r.take(out);
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
        ("text-restore-on-visible", t) if t.starts_with("panel 'activity' registered") => {
            r.host_page(
                "activity",
                json!({"type": "column", "id": "root", "children": [
                    {"type": "text", "id": "log", "props": {"stream": "log", "mono": true}}]}),
            );
        }
        ("text-restore-on-visible", t) if t.starts_with("frames append 'line 1") => {
            for line in ["line 1\n", "line 2\n"] {
                r.host_frame(
                    "activity",
                    "log",
                    Frame::Text {
                        seq: 0,
                        append: line.into(),
                    },
                );
            }
        }
        ("text-restore-on-visible", t) if t.starts_with("hub closes the channel") => {
            let key = StreamKey::new(PanelKey::new(ART, "activity"), "log");
            let phase = r.host.as_ref().unwrap().streams.get(&key).unwrap().phase;
            if phase != Phase::Closed {
                return Err(format!("the stream is {phase:?}, not closed"));
            }
            let shown = r.host_text("activity", "log");
            if shown != "line 1\nline 2\n" {
                return Err(format!("the node shows {shown:?} while hidden"));
            }
        }
        ("text-restore-on-visible", t) if t.starts_with("frames append 'line 4") => {
            r.host_frame(
                "activity",
                "log",
                Frame::Text {
                    seq: 0,
                    append: "line 4\n".into(),
                },
            );
            let shown = r.host_text("activity", "log");
            if shown != "line 1\nline 2\nline 3\nline 4\n" {
                return Err(format!("the node shows {shown:?}"));
            }
        }
        ("reopen-then-stopped", t) if t.starts_with("panel 'room' visible") => {
            r.host_page(
                "room",
                json!({"type": "column", "id": "root", "children": [
                    {"type": "canvas", "id": "map", "props": {"stream": "previews"},
                     "children": [{"type": "rect", "id": "a", "props": {}}]}]}),
            );
            // Visible before the transcript starts: its ui.visibility is not part of it.
            r.host_show("room", true, false);
        }
        ("reopen-then-stopped", t) if t.starts_with("the provider closes the channel") => {
            let key = StreamKey::new(PanelKey::new(ART, "room"), "map");
            let gen = r.granted["map"];
            r.host.as_mut().unwrap().streams.ended(&key, gen, r.now);
        }
        ("reopen-then-stopped", t) if t.ends_with("s later the hub reopens") => {
            let secs: u64 = t[..1].parse().unwrap();
            let host = r.host.as_mut().unwrap();
            // Not a moment early.
            host.streams
                .tick(r.now + Duration::from_secs(secs) - Duration::from_millis(1));
            if !host.streams.take_commands().is_empty() {
                return Err(format!("reopened before {secs} s"));
            }
            r.now += Duration::from_secs(secs);
            host.streams.tick(r.now);
            r.host_opens();
        }
        ("reopen-then-stopped", t) if t.starts_with("no more reopens") => {
            let key = StreamKey::new(PanelKey::new(ART, "room"), "map");
            let host = r.host.as_mut().unwrap();
            let look = host.streams.look(&key, true, r.now);
            if look != Look::Stopped {
                return Err(format!("the node looks {look:?}"));
            }
            host.streams.tick(r.now + Duration::from_secs(3600));
            if !host.streams.take_commands().is_empty() {
                return Err("a fourth reopen".into());
            }
        }
        ("reopen-then-stopped", "the user presses Try again") => {
            let key = StreamKey::new(PanelKey::new(ART, "room"), "map");
            r.host
                .as_mut()
                .unwrap()
                .streams
                .act(&StreamAct::TryAgain(key), r.now);
            r.host_opens();
        }
        ("reopen-then-stopped", t) if t.starts_with("hub closes the channel") => {
            let key = StreamKey::new(PanelKey::new(ART, "room"), "map");
            let host = r.host.as_mut().unwrap();
            if host.streams.get(&key).is_some() {
                return Err("the removed stream kept its state".into());
            }
            host.streams.tick(r.now + Duration::from_secs(3600));
            if !host.streams.take_commands().is_empty() {
                return Err("a removed stream reopened".into());
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

#[test]
fn a_revocation_notice_ends_when_another_version_connects() {
    // #959: `module.<id>.revoked` stays while only the revoked version is around.
    let key = format!("module.{ART}.revoked");
    for (revoked, kept) in [("0.4.0", true), ("0.3.0", false)] {
        let mut r = Replay::new(vec![]);
        let out = r.router.revoke(ART, revoked, "");
        r.take(out);
        assert_eq!(
            r.router.fact(&key),
            Some(&json!({"version": revoked, "reason": ""}))
        );
        // `module` initializes as 0.4.0.
        r.module("module", ART, None);
        assert_eq!(r.router.fact(&key).is_some(), kept, "revoked {revoked}");
    }
}
