// SPDX-License-Identifier: MIT
//! The panels the hub holds (§5): registration, patches, facts for bindings, and input out as
//! `ui.event`. Window-free: the renderer draws from it and reports input back.

use crate::assets::Assets;
use crate::tree;
use crate::view::ViewState;
use hideout_contract::error::{Error, ErrorId};
use hideout_contract::messages::{
    Badge, PatchOp, UiBadge, UiEvent, UiPatch, UiVisibility, Visibility,
};
use hideout_contract::module::{Module, Slot};
use hideout_contract::panel::{self, Limits, Node, Panel, Register, Rejected, LIMITS};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Patches applied per panel at most this often (~20 a second, §5.3); the rest are coalesced.
pub const PATCH_INTERVAL: Duration = Duration::from_millis(50);
/// Patches a panel may hold back while it waits for its next slot. Past this the oldest go: a
/// module that floods is only coalesced harder, it is never answered with an error.
pub const MAX_QUEUED_PATCHES: usize = 256;
/// `ui.event` calls in flight at once; past this new input is dropped (with a log line).
pub const MAX_PENDING_EVENTS: usize = 1024;

/// What drawing one panel borrows from the host: the panel, its view state, the facts, the
/// assets and the bound keys.
pub(crate) type Parts<'a> = (
    &'a mut HostedPanel,
    &'a mut ViewState,
    &'a HashMap<String, Value>,
    &'a Assets,
    &'a HashMap<(String, String), String>,
    &'a crate::streams::Streams,
);

/// A panel's name in the hub: its module and its id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PanelKey {
    pub module: String,
    pub panel: String,
}

impl PanelKey {
    pub fn new(module: impl Into<String>, panel: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            panel: panel.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelState {
    Live,
    /// The module went away: the panel greys with its state ("restarting", "stopped",
    /// "crash-looped") and is not removed (§5.6, §7.8).
    Greyed(String),
    /// The module's last `ui.register` rejected this panel, with this error id (§5.5).
    Rejected(String),
    /// Drawing it panicked; greyed until the module registers again (§5.5).
    Broken,
    /// Started quarantined after repeated hub restarts on this module's panel (§5.5).
    Quarantined,
}

/// The words a person reads for a module state id (section 7.8). Unknown ids read "Not running".
pub fn state_words(state: &str) -> &'static str {
    match state {
        "starting" => "Starting",
        "updating" => "Updating",
        "restarting" => "Restarting",
        "crash-stopped" | "crash-looped" => "Stopped after 5 crashes",
        "waiting" => "Waiting for another module",
        "needs-hub" => "Needs a newer Hideout",
        "install-failed" => "Couldn't install or update",
        "off" => "Off",
        _ => "Not running",
    }
}

impl PanelState {
    /// The line a person reads above a panel that is not live, in plain words. No ids.
    pub fn banner(&self, module_name: &str) -> Option<String> {
        match self {
            PanelState::Live => None,
            PanelState::Greyed(state) => Some(format!("{module_name}: {}.", state_words(state))),
            PanelState::Rejected(_) | PanelState::Broken | PanelState::Quarantined => {
                Some(format!("{module_name}'s page couldn't be shown."))
            }
        }
    }

    /// The technical detail for a tooltip or "Copy diagnostics": the state or error id.
    pub fn details(&self) -> Option<String> {
        match self {
            PanelState::Live => None,
            PanelState::Greyed(state) => Some(format!("State: {state}")),
            PanelState::Rejected(id) => Some(format!("Error: {id}")),
            PanelState::Broken => Some("Drawing it failed".into()),
            PanelState::Quarantined => Some("Quarantined after repeated hub restarts".into()),
        }
    }
}

#[derive(Debug)]
pub struct HostedPanel {
    pub key: PanelKey,
    /// The module's name for people ("Art"), for "<Module>'s page couldn't be shown".
    pub module_name: String,
    pub instance: String,
    pub slot: Slot,
    pub title: String,
    pub icon: String,
    pub tree: Node,
    pub state: PanelState,
    /// The reason of the last refused `ui.event`, shown until the next input (§5.3).
    pub notice: Option<String>,
    /// The tab badge the module set with `ui.badge` (5.9); `none` until it does.
    pub badge: Badge,
    /// The count with `Badge::Count` (at least 1), else `None`.
    pub badge_count: Option<u32>,
    gate: PatchGate,
}

impl HostedPanel {
    pub fn live(&self) -> bool {
        self.state == PanelState::Live
    }
}

/// Coalesces patches to one application per [`PATCH_INTERVAL`] (§5.3).
#[derive(Debug, Default)]
struct PatchGate {
    last: Option<Instant>,
    queued: VecDeque<Vec<PatchOp>>,
}

impl PatchGate {
    fn due(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|l| now.duration_since(l) >= PATCH_INTERVAL)
    }

    fn next(&self) -> Option<Instant> {
        if self.queued.is_empty() {
            None
        } else {
            Some(self.last.map_or_else(Instant::now, |l| l + PATCH_INTERVAL))
        }
    }
}

/// One `ui.event` for the owning module (§5.3, §8.8). `seq` comes back with its result.
#[derive(Clone, Debug, PartialEq)]
pub struct Outgoing {
    pub seq: u64,
    pub module: String,
    pub event: UiEvent,
}

/// Input the renderer saw, before the host turns it into a `ui.event`.
#[derive(Clone, Debug, PartialEq)]
pub struct Interaction {
    pub key: PanelKey,
    pub node: String,
    pub kind: String,
    pub value: Value,
    /// The value drawn at once while the module decides (toggle, slider, choice, colour).
    pub optimistic: bool,
}

#[derive(Debug)]
struct Pending {
    key: PanelKey,
    node: String,
    value: Value,
    optimistic: bool,
}

/// Every panel in the hub, with its view state and the facts and assets it draws from.
#[derive(Debug)]
pub struct PanelHost {
    panels: Vec<HostedPanel>,
    views: HashMap<PanelKey, ViewState>,
    facts: HashMap<String, Value>,
    pub assets: Assets,
    limits: Limits,
    outbox: Vec<Outgoing>,
    pending: HashMap<u64, Pending>,
    /// The newest event per node: an older answer must not undo a newer optimistic value.
    latest: HashMap<(PanelKey, String), u64>,
    next_seq: u64,
    quarantined: HashSet<String>,
    pub(crate) breadcrumb: Option<PanelKey>,
    /// The last `ui.visibility` queued per page panel (sent on change only, 5.9).
    sent_visibility: HashMap<PanelKey, Visibility>,
    closed: HashSet<PanelKey>,
    visibility_out: Vec<(String, UiVisibility)>,
    /// The page panels on screen, from the last [`PanelHost::set_visible`].
    shown: Vec<PanelKey>,
    /// The tray panel is open: its tiles are `visible` (5.9, v1.4).
    tray_open: bool,
    /// Tiles the user hid (Edit tiles): `hidden` even while the tray panel is open.
    hidden_tiles: HashSet<PanelKey>,
    /// What each module's manifest declares (§4.7): panels with `when`, actions, settings.
    manifests: BTreeMap<String, Module>,
    /// Bound keys by (module, action), as people read them ("F13").
    keys: HashMap<(String, String), String>,
    /// When each module's panels last went from live to greyed ("Last seen", §7.8).
    gone_at: HashMap<String, Instant>,
    /// When the current `module.<id>.restart` fact arrived (the restart countdown, §7.8).
    restart_since: HashMap<String, Instant>,
    /// Settings values per module for the generated form (§4.7), from `settings.get`.
    settings: HashMap<String, Map<String, Value>>,
    settings_asked: HashSet<String>,
    /// The reason of the last refused setting, per module, shown under the form.
    pub settings_notice: HashMap<String, String>,
    hub_calls: Vec<HubCall>,
    /// Which secrets each module has saved, once asked.
    secrets_saved: HashMap<String, BTreeSet<String>>,
    /// The system file picker, when the hub has one.
    pub picker: Option<Picker>,
    /// The live streams of the page panels (§5.4, H2.9).
    pub streams: crate::streams::Streams,
}

/// A call the hub makes to a module for its own UI (not a `ui.event`).
#[derive(Clone, Debug, PartialEq)]
pub enum HubCall {
    /// A button with `action`, or a bound key: `action.invoke {action, source}` (§8.8).
    Invoke { module: String, action: String },
    /// The settings form needs the values: `settings.get`.
    SettingsGet { module: String },
    /// The user changed one setting: `settings.set {name, value}`.
    SettingsSet {
        module: String,
        name: String,
        value: Value,
    },
    /// Which of the module's `[[secrets]]` have a value saved (never the values).
    SecretsStatus { module: String },
    /// The user typed a secret: the hub stores it for the module (`None` clears it).
    SetSecret {
        module: String,
        name: String,
        value: Option<String>,
    },
}

/// Opens the system file picker for a `kind = "file"` setting (v1.4): given the manifest's
/// `filter`, the chosen path, or `None` when the user cancelled. The shell installs the
/// Windows one; tests install a fake, and with none the button is not drawn.
pub type FilePicker = Box<dyn FnMut(Option<&str>) -> Option<String>>;

/// [`FilePicker`] in the host (it has no `Debug`).
pub struct Picker(pub FilePicker);

impl std::fmt::Debug for Picker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Picker")
    }
}

/// A `when` clause (§4.6, grammar v1): one fact name, optionally prefixed by `!`; true when the
/// fact is set and is not `false`, `null`, `0` or `""`. No clause is true.
pub fn when_true(when: Option<&str>, facts: &HashMap<String, Value>) -> bool {
    let Some(w) = when.map(str::trim).filter(|w| !w.is_empty()) else {
        return true;
    };
    let (neg, name) = match w.strip_prefix('!') {
        Some(n) => (true, n.trim()),
        None => (false, w),
    };
    let set = match facts.get(name) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    };
    set != neg
}

/// The restart countdown of a restarting module (§7.8): seconds left, which crash of how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restart {
    pub in_s: u64,
    pub crash: u64,
    pub of: u64,
}

impl Default for PanelHost {
    fn default() -> Self {
        Self::new(LIMITS)
    }
}

impl PanelHost {
    pub fn new(limits: Limits) -> Self {
        Self {
            panels: Vec::new(),
            views: HashMap::new(),
            facts: HashMap::new(),
            assets: Assets::default(),
            limits,
            outbox: Vec::new(),
            pending: HashMap::new(),
            latest: HashMap::new(),
            next_seq: 1,
            quarantined: HashSet::new(),
            breadcrumb: None,
            sent_visibility: HashMap::new(),
            closed: HashSet::new(),
            visibility_out: Vec::new(),
            shown: Vec::new(),
            tray_open: false,
            hidden_tiles: HashSet::new(),
            manifests: BTreeMap::new(),
            keys: HashMap::new(),
            gone_at: HashMap::new(),
            restart_since: HashMap::new(),
            settings: HashMap::new(),
            settings_asked: HashSet::new(),
            settings_notice: HashMap::new(),
            hub_calls: Vec::new(),
            secrets_saved: HashMap::new(),
            picker: None,
            streams: Default::default(),
        }
    }

    /// Whether the module's secret `name` is saved; the first call for a module asks the hub.
    pub fn secret_saved(&mut self, module: &str, name: &str) -> bool {
        match self.secrets_saved.get(module) {
            Some(s) => s.contains(name),
            None => {
                self.secrets_saved.insert(module.into(), BTreeSet::new());
                self.hub_calls.push(HubCall::SecretsStatus {
                    module: module.into(),
                });
                false
            }
        }
    }

    /// The hub's answer: the module's saved secret names.
    pub fn secrets_result(&mut self, module: &str, saved: BTreeSet<String>) {
        self.secrets_saved.insert(module.into(), saved);
    }

    /// The user saved a secret in the form. Shown as saved at once; a refusal shows the reason
    /// and asks again.
    pub fn set_secret(&mut self, module: &str, name: &str, value: String) {
        self.settings_notice.remove(module);
        self.secrets_saved
            .entry(module.into())
            .or_default()
            .insert(name.into());
        self.hub_calls.push(HubCall::SetSecret {
            module: module.into(),
            name: name.into(),
            value: Some(value),
        });
    }

    /// Saving a secret failed.
    pub fn secret_refused(&mut self, module: &str, reason: String) {
        self.settings_notice.insert(module.into(), reason);
        self.hub_calls.push(HubCall::SecretsStatus {
            module: module.into(),
        });
    }

    /// Opens the file picker, if the hub has one.
    pub fn pick_file(&mut self, filter: Option<&str>) -> Option<String> {
        self.picker.as_mut().and_then(|p| (p.0)(filter))
    }

    /// What `module`'s manifest declares (§4.7). The tray panel and the generated page sections
    /// read it; a module the hub has no manifest for still draws what it registers.
    pub fn set_manifest(&mut self, manifest: Module) {
        self.manifests.insert(manifest.id.clone(), manifest);
    }

    pub fn manifest(&self, module: &str) -> Option<&Module> {
        self.manifests.get(module)
    }

    pub fn manifests(&self) -> impl Iterator<Item = &Module> {
        self.manifests.values()
    }

    /// The module's name for people: its manifest's `name`, else what it registered with.
    pub fn module_name(&self, module: &str) -> String {
        self.manifests
            .get(module)
            .map(|m| m.name.clone())
            .or_else(|| {
                self.panels
                    .iter()
                    .find(|p| p.key.module == module)
                    .map(|p| p.module_name.clone())
            })
            .unwrap_or_else(|| module.to_string())
    }

    /// Binds (or with `None` unbinds) a key to one of the module's actions, as people read it.
    pub fn set_key(&mut self, module: &str, action: &str, key: Option<String>) {
        let k = (module.to_string(), action.to_string());
        match key {
            Some(v) => self.keys.insert(k, v),
            None => self.keys.remove(&k),
        };
    }

    pub fn key(&self, module: &str, action: &str) -> Option<&str> {
        self.keys
            .get(&(module.to_string(), action.to_string()))
            .map(String::as_str)
    }

    /// A `when` clause against the hub's facts (§4.6).
    pub fn when(&self, when: Option<&str>) -> bool {
        when_true(when, &self.facts)
    }

    /// `module.<id>.state` (§7.8), when the supervisor has published one.
    pub fn module_state(&self, module: &str) -> Option<&str> {
        self.facts
            .get(&format!("module.{module}.state"))
            .and_then(Value::as_str)
    }

    /// The restart countdown at `now`, while `module.<id>.restart` is set (§7.8).
    pub fn restart(&self, module: &str, now: Instant) -> Option<Restart> {
        let f = self.facts.get(&format!("module.{module}.restart"))?;
        let delay = f.get("delay_s")?.as_u64()?;
        let since = self.restart_since.get(module).copied().unwrap_or(now);
        let gone = now.saturating_duration_since(since);
        // Rounded up: "in 1 s" until it is due, never "in 0 s" while it still waits.
        let left = Duration::from_secs(delay).saturating_sub(gone);
        let in_s = left.as_secs() + u64::from(left.subsec_nanos() > 0);
        Some(Restart {
            in_s,
            crash: f.get("crash").and_then(Value::as_u64).unwrap_or(1),
            of: f.get("of").and_then(Value::as_u64).unwrap_or(5),
        })
    }

    /// When the restart countdown fact arrived (tests and review renders set it).
    pub fn set_restart_since(&mut self, module: &str, at: Instant) {
        self.restart_since.insert(module.to_string(), at);
    }

    /// How long ago the module's panels were last live, while they are greyed.
    pub fn last_seen(&self, module: &str, now: Instant) -> Option<Duration> {
        self.gone_at
            .get(module)
            .map(|t| now.saturating_duration_since(*t))
    }

    /// A button with `action` was pressed, or a bound key: queues `action.invoke`. Like a key
    /// press it is sent while the module restarts too (UX3).
    pub fn invoke(&mut self, module: &str, action: &str) {
        self.hub_calls.push(HubCall::Invoke {
            module: module.into(),
            action: action.into(),
        });
    }

    /// The settings form's values: every declared setting with the module's value, else its
    /// default. The first call for a module asks it (`settings.get`).
    pub fn settings_values(&mut self, module: &str) -> Map<String, Value> {
        if self.settings_asked.insert(module.to_string()) {
            self.hub_calls.push(HubCall::SettingsGet {
                module: module.into(),
            });
        }
        let known = self.settings.get(module);
        let mut out = Map::new();
        for d in self
            .manifests
            .get(module)
            .map(|m| &m.settings[..])
            .unwrap_or(&[])
        {
            let v = known
                .and_then(|k| k.get(&d.name))
                .cloned()
                .unwrap_or_else(|| d.default.clone());
            out.insert(d.name.clone(), v);
        }
        out
    }

    /// The module's `settings.get` answer.
    pub fn settings_result(&mut self, module: &str, values: Map<String, Value>) {
        self.settings.insert(module.into(), values);
    }

    /// The user changed one setting in the form: drawn at once, sent with `settings.set`.
    pub fn set_setting(&mut self, module: &str, name: &str, value: Value) {
        self.settings_notice.remove(module);
        self.settings
            .entry(module.into())
            .or_default()
            .insert(name.into(), value.clone());
        self.hub_calls.push(HubCall::SettingsSet {
            module: module.into(),
            name: name.into(),
            value,
        });
    }

    /// The module refused a setting: show why, and read the values back.
    pub fn setting_refused(&mut self, module: &str, reason: String) {
        self.settings_notice.insert(module.into(), reason);
        self.hub_calls.push(HubCall::SettingsGet {
            module: module.into(),
        });
    }

    /// The calls to make for the hub's own UI, oldest first.
    pub fn take_hub_calls(&mut self) -> Vec<HubCall> {
        std::mem::take(&mut self.hub_calls)
    }

    /// `ui.register` from `module` (§5.1, §5.6): validates each panel (§5.5) and replaces all of
    /// the module's panels in one step, in the place the first of them had. View state is kept
    /// by panel and node id. `declared` is the module's `[[ui.panels]]` ids when known.
    pub fn register(
        &mut self,
        module: &str,
        module_name: &str,
        declared: Option<&[String]>,
        params: &Register,
    ) -> Vec<Rejected> {
        self.register_with(module, module_name, declared, None, params)
    }

    /// [`PanelHost::register`], also refusing a button whose `action` names none of the module's
    /// `[[actions]]` ids (`actions`, when known; `ui.action`, spec v1.4).
    pub fn register_with(
        &mut self,
        module: &str,
        module_name: &str,
        declared: Option<&[String]>,
        actions: Option<&[String]>,
        params: &Register,
    ) -> Vec<Rejected> {
        let (ok, mut rejected) = panel::register_with(params, declared, actions, &self.limits);
        let mut seen = HashSet::new();
        let mut accepted: Vec<Panel> = Vec::new();
        for p in ok {
            let error = if !seen.insert(p.id.clone()) {
                Some(ErrorId::UiLimits)
            } else if let Some(id) = tree::duplicate_id(&p.tree) {
                log::warn!("{module}: panel {}: node id {id} used twice", p.id);
                Some(ErrorId::UiLimits)
            } else {
                None
            };
            match error {
                Some(e) => rejected.push(Rejected {
                    panel: p.id.clone(),
                    error: e.id().to_string(),
                }),
                None => accepted.push(p),
            }
        }
        let quarantined = self.quarantined.contains(module);
        let at = self
            .panels
            .iter()
            .position(|p| p.key.module == module)
            .unwrap_or(self.panels.len());
        let (old, rest): (Vec<HostedPanel>, Vec<HostedPanel>) = std::mem::take(&mut self.panels)
            .into_iter()
            .partition(|p| p.key.module == module);
        self.panels = rest;
        let mut fresh = Vec::new();
        // A new instance starts with no badge and a fresh `ui.visibility` (5.9).
        self.sent_visibility.retain(|k, _| k.module != module);
        self.closed.retain(|k| k.module != module);
        for p in accepted {
            let key = PanelKey::new(module, &p.id);
            let kept = old
                .iter()
                .find(|o| o.key == key && o.instance == params.instance && o.slot == p.slot);
            let (badge, badge_count) =
                kept.map_or((Badge::None, None), |o| (o.badge, o.badge_count));
            self.views.entry(key.clone()).or_default().rebuilt(&p.tree);
            fresh.push(HostedPanel {
                key,
                module_name: module_name.to_string(),
                instance: params.instance.clone(),
                slot: p.slot,
                title: p.title,
                icon: p.icon,
                tree: p.tree,
                state: if quarantined {
                    PanelState::Quarantined
                } else {
                    PanelState::Live
                },
                notice: None,
                badge,
                badge_count,
                gate: PatchGate::default(),
            });
        }
        // A rejected panel keeps its place (and its old tree, greyed) so the user sees why.
        for r in &rejected {
            let prior = old.iter().find(|o| o.key.panel == r.panel);
            let raw = params
                .panels
                .iter()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(r.panel.as_str()));
            let field = |k: &str| {
                raw.and_then(|v| v.get(k))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            let slot = raw
                .and_then(|v| v.get("slot"))
                .and_then(|s| serde_json::from_value(s.clone()).ok())
                .or(prior.map(|o| o.slot))
                .unwrap_or(Slot::Page);
            fresh.push(HostedPanel {
                key: PanelKey::new(module, &r.panel),
                module_name: module_name.to_string(),
                instance: params.instance.clone(),
                slot,
                title: field("title")
                    .or(prior.map(|o| o.title.clone()))
                    .unwrap_or_else(|| r.panel.clone()),
                icon: field("icon").unwrap_or_default(),
                tree: prior.map_or_else(empty_tree, |o| o.tree.clone()),
                state: PanelState::Rejected(r.error.clone()),
                notice: None,
                badge: Badge::None,
                badge_count: None,
                gate: PatchGate::default(),
            });
        }
        self.panels.splice(at..at, fresh);
        // Answers still owed for the old instance's events no longer apply.
        self.forget_pending(module);
        rejected
    }

    /// `ui.patch` from `module` (§5.3). Applied at once when the panel's slot is free, held and
    /// coalesced otherwise (never an error for the rate). A patch that breaks the tree or a cap
    /// is refused whole and the tree stays as it was.
    pub fn patch(&mut self, module: &str, patch: &UiPatch, now: Instant) -> Result<(), Error> {
        let limits = self.limits;
        let i = self
            .index(&PanelKey::new(module, &patch.panel))
            .ok_or_else(|| {
                Error::new(
                    ErrorId::UiUndeclared,
                    format!("{module} has no panel {}", patch.panel),
                )
            })?;
        self.streams.patch_received(&self.panels[i].key, &patch.ops);
        let p = &mut self.panels[i];
        if p.gate.due(now) && p.gate.queued.is_empty() {
            p.gate.last = Some(now);
            let r = apply_checked(p, &patch.ops, &limits);
            if r.is_ok() {
                self.after_patch(i, &patch.ops);
            }
            r
        } else {
            if p.gate.queued.len() >= MAX_QUEUED_PATCHES {
                p.gate.queued.pop_front();
            }
            p.gate.queued.push_back(patch.ops.clone());
            Ok(())
        }
    }

    /// Applies the patches held back whose slot has come. Returns when it next needs a call.
    pub fn flush(&mut self, now: Instant) -> Option<Instant> {
        let limits = self.limits;
        for i in 0..self.panels.len() {
            let p = &mut self.panels[i];
            if p.gate.queued.is_empty() || !p.gate.due(now) {
                continue;
            }
            p.gate.last = Some(now);
            let batch: Vec<Vec<PatchOp>> = p.gate.queued.drain(..).collect();
            let mut all = Vec::new();
            for ops in batch {
                if let Err(e) = apply_checked(p, &ops, &limits) {
                    log::warn!("{}/{}: patch refused: {e}", p.key.module, p.key.panel);
                }
                all.extend(ops);
            }
            self.after_patch(i, &all);
        }
        self.panels.iter().filter_map(|p| p.gate.next()).min()
    }

    fn after_patch(&mut self, i: usize, ops: &[PatchOp]) {
        let key = self.panels[i].key.clone();
        self.streams.patch_landed(&key, ops);
        if let Some(v) = self.views.get_mut(&key) {
            for node in tree::touched(ops) {
                v.optimistic.remove(node);
            }
        }
    }

    /// A module went away (§5.6, §7.8): its panels grey with `state`, keeping tree and view state.
    pub fn module_gone(&mut self, module: &str, state: &str) {
        self.module_gone_at(module, state, Instant::now());
    }

    /// [`PanelHost::module_gone`] at a given time ("Last seen" counts from it). A greyed panel
    /// takes the newer state.
    pub fn module_gone_at(&mut self, module: &str, state: &str, now: Instant) {
        let mut was_live = false;
        for p in self.panels.iter_mut().filter(|p| p.key.module == module) {
            match &p.state {
                PanelState::Live => {
                    was_live = true;
                    p.state = PanelState::Greyed(state.to_string());
                }
                PanelState::Greyed(_) => p.state = PanelState::Greyed(state.to_string()),
                _ => {}
            }
            p.gate.queued.clear();
        }
        if was_live {
            self.gone_at.insert(module.to_string(), now);
        }
        self.forget_pending(module);
    }

    /// `ui.badge` from `module` (5.9). A badge for a panel the module didn't register, or for a
    /// slot other than `page`, is ignored and logged.
    pub fn set_badge(&mut self, module: &str, b: &UiBadge) {
        let Some(p) = self
            .panels
            .iter_mut()
            .find(|p| p.key.module == module && p.key.panel == b.panel && p.slot == Slot::Page)
        else {
            log::warn!(
                "{module}: ui.badge for {} ignored: not a page panel",
                b.panel
            );
            return;
        };
        match (b.badge, b.count) {
            (Badge::Count, Some(n)) if n >= 1 => {
                p.badge = Badge::Count;
                p.badge_count = Some(n);
            }
            (Badge::Count, _) => log::warn!("{module}: ui.badge count without a count ignored"),
            (other, _) => {
                p.badge = other;
                p.badge_count = None;
            }
        }
    }

    /// The content area changed: `shown` is the page panel on screen, if any. Every other page
    /// panel is `hidden` (5.9). Queues a `ui.visibility` for each panel whose state changed.
    pub fn set_shown(&mut self, shown: Option<&PanelKey>) {
        self.set_visible(shown.map(std::slice::from_ref).unwrap_or_default());
    }

    /// Like [`PanelHost::set_shown`] for a hub with tabs and pop-outs: every panel in `shown` is
    /// on screen (the active tab, pop-outs that aren't minimised); every other open page panel is
    /// `hidden`, and closed ones `closed`. An empty slice is the hub minimised or in the tray.
    pub fn set_visible(&mut self, shown: &[PanelKey]) {
        self.shown = shown.to_vec();
        self.sync_visibility();
    }

    /// The tray panel opened or closed: its tiles turn `visible` or `hidden` (5.9, v1.4).
    pub fn set_tray_open(&mut self, open: bool) {
        self.tray_open = open;
        self.sync_visibility();
    }

    /// The user hid or showed a tile (Edit tiles). A hidden tile stays `hidden`.
    pub fn set_tile_hidden(&mut self, key: &PanelKey, hidden: bool) {
        if hidden {
            self.hidden_tiles.insert(key.clone());
        } else {
            self.hidden_tiles.remove(key);
        }
        self.sync_visibility();
    }

    /// Whether the user hid the tile (or the tray panel shows it not).
    pub fn tile_hidden(&self, key: &PanelKey) -> bool {
        self.hidden_tiles.contains(key)
    }

    /// A button's `opens` (v1.4) or a tile click: the module's first `page` panel to open, with
    /// the node to bring into view (`page#<node>`; `settings` is the generated Settings
    /// section). `None` when the module has no page.
    pub fn navigate(&mut self, module: &str, target: &str) -> Option<PanelKey> {
        let page = self
            .manifests
            .get(module)
            .and_then(|m| m.panels.iter().find(|p| p.slot == Slot::Page))
            .map(|p| PanelKey::new(module, &p.id))
            .or_else(|| {
                self.panels
                    .iter()
                    .find(|p| p.key.module == module && p.slot == Slot::Page)
                    .map(|p| p.key.clone())
            })?;
        let focus = match target {
            "settings" => Some(crate::sections::SETTINGS_ID.to_string()),
            t => t.strip_prefix("page#").map(String::from),
        };
        self.view_mut(&page).focus = focus;
        Some(page)
    }

    /// Queues a `ui.visibility` for each page or tile panel whose state changed. A tile is never
    /// `closed`.
    fn sync_visibility(&mut self) {
        let mut out = Vec::new();
        let shown = &self.shown;
        for p in &self.panels {
            let want = match p.slot {
                Slot::Page if self.closed.contains(&p.key) => Visibility::Closed,
                Slot::Page if shown.contains(&p.key) => Visibility::Visible,
                Slot::Tile if self.tray_open && !self.hidden_tiles.contains(&p.key) => {
                    Visibility::Visible
                }
                // A tile's first word is `visible`, when the tray panel first opens (the
                // ui/visibility transcript); until then the module hears nothing.
                Slot::Tile if !self.sent_visibility.contains_key(&p.key) => continue,
                Slot::Page | Slot::Tile => Visibility::Hidden,
                _ => continue,
            };
            if self.sent_visibility.get(&p.key) != Some(&want) {
                self.sent_visibility.insert(p.key.clone(), want);
                out.push((
                    p.key.module.clone(),
                    UiVisibility {
                        panel: p.key.panel.clone(),
                        state: want,
                    },
                ));
            }
        }
        self.visibility_out.extend(out);
    }

    /// The user closed the panel's tab: it reads `closed` until it is opened again.
    pub fn close_panel(&mut self, key: &PanelKey) {
        self.closed.insert(key.clone());
    }

    /// Opening a panel again (from the sidebar) clears `closed`.
    pub fn reopen_panel(&mut self, key: &PanelKey) {
        self.closed.remove(key);
    }

    /// The `ui.visibility` notifications to send: the owning module and the message.
    pub fn take_visibility(&mut self) -> Vec<(String, UiVisibility)> {
        std::mem::take(&mut self.visibility_out)
    }

    /// Opens and closes the page panels' streams to match what is on screen (§5.4). Call after
    /// sending [`PanelHost::take_visibility`]: `visible` goes out before a panel's opens.
    pub fn sync_streams(&mut self, now: std::time::Instant) {
        self.streams.sync(&self.panels, &self.shown, now);
    }

    /// Starts `module`'s panels quarantined (§5.5) until [`PanelHost::release`].
    pub fn quarantine(&mut self, module: &str) {
        self.quarantined.insert(module.to_string());
        for p in self.panels.iter_mut().filter(|p| p.key.module == module) {
            p.state = PanelState::Quarantined;
        }
    }

    /// "Start again" on a quarantined module.
    pub fn release(&mut self, module: &str) {
        self.quarantined.remove(module);
        for p in self.panels.iter_mut().filter(|p| p.key.module == module) {
            if p.state == PanelState::Quarantined {
                p.state = PanelState::Live;
            }
        }
    }

    /// The panel being drawn right now, if any (§5.5). The hub persists it to tell a restart
    /// loop caused by one panel.
    pub fn breadcrumb(&self) -> Option<&PanelKey> {
        self.breadcrumb.as_ref()
    }

    /// A fact for bindings changed. Returns whether a panel binds it (and needs a redraw).
    pub fn set_fact(&mut self, name: &str, value: Value) -> bool {
        if self.facts.get(name) == Some(&value)
            || (value.is_null() && !self.facts.contains_key(name))
        {
            return false;
        }
        let module_fact = name
            .strip_prefix("module.")
            .and_then(|r| r.rsplit_once('.'))
            .map(|(m, k)| (m.to_string(), k.to_string()));
        if value.is_null() {
            self.facts.remove(name);
        } else {
            self.facts.insert(name.to_string(), value.clone());
        }
        match module_fact.as_ref().map(|(m, k)| (m.as_str(), k.as_str())) {
            // A module that isn't running greys its panels (§7.8); `running` and `starting`
            // wait for its `ui.register`, which makes them live again.
            Some((m, "state")) => {
                match value.as_str() {
                    Some("running") | None => {}
                    Some("starting") => {
                        for p in self.panels.iter_mut().filter(|p| p.key.module == m) {
                            if matches!(p.state, PanelState::Greyed(_)) {
                                p.state = PanelState::Greyed("starting".into());
                            }
                        }
                    }
                    Some(s) => {
                        let s = s.to_string();
                        self.module_gone(m, &s);
                    }
                }
                true
            }
            Some((m, "restart")) => {
                if value.is_null() {
                    self.restart_since.remove(m);
                } else {
                    self.restart_since.insert(m.to_string(), Instant::now());
                }
                true
            }
            Some(_) => true,
            // `when` clauses (`apps.seen.<name>` and others) decide which tiles show.
            None => {
                self.bound_facts().contains(name)
                    || self.manifests.values().any(|m| {
                        m.panels.iter().any(|p| {
                            p.when
                                .as_deref()
                                .is_some_and(|w| w.trim_start_matches('!') == name)
                        })
                    })
            }
        }
    }

    pub fn fact(&self, name: &str) -> Option<&Value> {
        self.facts.get(name)
    }

    /// The facts the panels bind (`fact:<name>`), for the hub to subscribe to.
    pub fn bound_facts(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for p in &self.panels {
            let mut stack = vec![&p.tree];
            while let Some(n) = stack.pop() {
                if let Some(f) = binding(n) {
                    out.insert(f.to_string());
                }
                stack.extend(n.children.iter());
            }
        }
        out
    }

    /// Input from the renderer becomes a `ui.event` for the owning module only (§5.3). An
    /// optimistic value draws at once and is reverted if the module refuses.
    pub fn emit(&mut self, i: Interaction) {
        let Some(idx) = self.index(&i.key) else {
            return;
        };
        if !self.panels[idx].live() {
            return;
        }
        if self.pending.len() >= MAX_PENDING_EVENTS {
            log::warn!(
                "{}: too many ui.event calls in flight; input dropped",
                i.key.module
            );
            return;
        }
        self.panels[idx].notice = None;
        let seq = self.next_seq;
        self.next_seq += 1;
        if i.optimistic {
            self.views
                .entry(i.key.clone())
                .or_default()
                .optimistic
                .insert(i.node.clone(), i.value.clone());
        }
        self.latest.insert((i.key.clone(), i.node.clone()), seq);
        self.outbox.push(Outgoing {
            seq,
            module: i.key.module.clone(),
            event: UiEvent {
                panel: i.key.panel.clone(),
                node: i.node.clone(),
                kind: i.kind,
                value: i.value.clone(),
            },
        });
        self.pending.insert(
            seq,
            Pending {
                key: i.key,
                node: i.node,
                value: i.value,
                optimistic: i.optimistic,
            },
        );
    }

    /// The `ui.event` calls to send, oldest first.
    pub fn take_events(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.outbox)
    }

    /// The module's answer to event `seq`: `Err(reason)` means refused, and the hub reverts the
    /// optimistic value and shows the reason (§5.3).
    pub fn event_result(&mut self, seq: u64, result: Result<(), String>) {
        let Some(p) = self.pending.remove(&seq) else {
            return;
        };
        let slot = (p.key.clone(), p.node.clone());
        let newest = self.latest.get(&slot) == Some(&seq);
        if newest {
            self.latest.remove(&slot);
        }
        let idx = self.index(&p.key);
        match result {
            Ok(()) if p.optimistic && newest => {
                // Accepted: the value stands until the module patches it.
                if let Some(idx) = idx {
                    if let Some(n) = tree::find_mut(&mut self.panels[idx].tree, &p.node) {
                        n.props.insert("value".into(), p.value);
                    }
                }
                if let Some(v) = self.views.get_mut(&p.key) {
                    v.optimistic.remove(&p.node);
                }
            }
            Ok(()) => {}
            Err(reason) => {
                if newest {
                    if let Some(v) = self.views.get_mut(&p.key) {
                        v.optimistic.remove(&p.node);
                    }
                }
                if let Some(idx) = idx {
                    self.panels[idx].notice = Some(reason);
                }
            }
        }
    }

    fn forget_pending(&mut self, module: &str) {
        let keys: Vec<PanelKey> = self
            .pending
            .values()
            .filter(|p| p.key.module == module)
            .map(|p| p.key.clone())
            .collect();
        self.pending.retain(|_, p| p.key.module != module);
        self.latest.retain(|(k, _), _| k.module != module);
        self.outbox.retain(|o| o.module != module);
        for k in keys {
            if let Some(v) = self.views.get_mut(&k) {
                v.optimistic.clear();
            }
        }
    }

    fn index(&self, key: &PanelKey) -> Option<usize> {
        self.panels.iter().position(|p| &p.key == key)
    }

    pub fn panel(&self, key: &PanelKey) -> Option<&HostedPanel> {
        self.panels.iter().find(|p| &p.key == key)
    }

    pub fn panels(&self) -> &[HostedPanel] {
        &self.panels
    }

    /// The panels in one slot, in registration order (`page` for the sidebar).
    pub fn in_slot(&self, slot: Slot) -> impl Iterator<Item = &HostedPanel> {
        self.panels.iter().filter(move |p| p.slot == slot)
    }

    pub fn view(&self, key: &PanelKey) -> Option<&ViewState> {
        self.views.get(key)
    }

    pub fn view_mut(&mut self, key: &PanelKey) -> &mut ViewState {
        self.views.entry(key.clone()).or_default()
    }

    /// Split borrows for the renderer.
    pub(crate) fn parts(&mut self, key: &PanelKey) -> Option<Parts<'_>> {
        let i = self.index(key)?;
        let view = self.views.entry(key.clone()).or_default();
        Some((
            &mut self.panels[i],
            view,
            &self.facts,
            &self.assets,
            &self.keys,
            &self.streams,
        ))
    }

    /// The hub's facts (for `when` clauses).
    pub fn facts(&self) -> &HashMap<String, Value> {
        &self.facts
    }
}

fn empty_tree() -> Node {
    Node {
        kind: "column".into(),
        id: "root".into(),
        props: Default::default(),
        bind: None,
        stream: None,
        children: Vec::new(),
    }
}

/// Applies `ops` to a copy of the panel's tree, checks the caps and ids, and keeps it on success.
fn apply_checked(p: &mut HostedPanel, ops: &[PatchOp], limits: &Limits) -> Result<(), Error> {
    let mut next = p.tree.clone();
    tree::apply(&mut next, ops)?;
    let candidate = Panel {
        id: p.key.panel.clone(),
        slot: p.slot,
        title: p.title.clone(),
        icon: p.icon.clone(),
        tree: next,
    };
    panel::validate(&candidate, limits)?;
    p.tree = candidate.tree;
    Ok(())
}

/// The fact a node binds (`fact:<name>`), from its `bind` field or a `bind` prop (§5.2).
pub fn binding(n: &Node) -> Option<&str> {
    n.bind
        .as_deref()
        .or_else(|| n.props.get("bind").and_then(Value::as_str))
        .and_then(|b| b.strip_prefix("fact:"))
}
