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
use hideout_contract::module::Slot;
use hideout_contract::panel::{self, Limits, Node, Panel, Register, Rejected, LIMITS};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Patches applied per panel at most this often (~20 a second, §5.3); the rest are coalesced.
pub const PATCH_INTERVAL: Duration = Duration::from_millis(50);
/// Patches a panel may hold back while it waits for its next slot. Past this the oldest go: a
/// module that floods is only coalesced harder, it is never answered with an error.
pub const MAX_QUEUED_PATCHES: usize = 256;
/// `ui.event` calls in flight at once; past this new input is dropped (with a log line).
pub const MAX_PENDING_EVENTS: usize = 1024;

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
        }
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
        let (ok, mut rejected) = panel::register(params, declared, &self.limits);
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
        if let Some(v) = self.views.get_mut(&key) {
            for node in tree::touched(ops) {
                v.optimistic.remove(node);
            }
        }
    }

    /// A module went away (§5.6, §7.8): its panels grey with `state`, keeping tree and view state.
    pub fn module_gone(&mut self, module: &str, state: &str) {
        for p in self.panels.iter_mut().filter(|p| p.key.module == module) {
            if p.state == PanelState::Live {
                p.state = PanelState::Greyed(state.to_string());
            }
            p.gate.queued.clear();
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
        let mut out = Vec::new();
        for p in self.panels.iter().filter(|p| p.slot == Slot::Page) {
            let want = if self.closed.contains(&p.key) {
                Visibility::Closed
            } else if shown.contains(&p.key) {
                Visibility::Visible
            } else {
                Visibility::Hidden
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
        if self.facts.get(name) == Some(&value) {
            return false;
        }
        self.facts.insert(name.to_string(), value);
        self.bound_facts().contains(name)
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
    pub(crate) fn parts(
        &mut self,
        key: &PanelKey,
    ) -> Option<(
        &mut HostedPanel,
        &mut ViewState,
        &HashMap<String, Value>,
        &Assets,
    )> {
        let i = self.index(key)?;
        let view = self.views.entry(key.clone()).or_default();
        Some((&mut self.panels[i], view, &self.facts, &self.assets))
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
