// SPDX-License-Identifier: MIT
//! The tray menu's quick actions (contract §5.1 `action` slot, design UX8, #1234): one menu item
//! per `action` panel a module declares, in the frame's "pinned quick actions" place after Open
//! Hideout, between two separators. The panel's id names the `[[actions]]` entry the item
//! presses; its `title` is the item's words, its `when` (§4.6) whether it shows, evaluated from
//! the hub's facts, so a cold module's item still shows the right words. While the module runs,
//! the first `button` of its registered `action` tree may change the words and grey the item
//! (`label`, `disabled`). A bound key shows right-aligned (`Free GPU\tCtrl+Alt+G`).
//!
//! An item shows while its module is installed, not off or down (§7.8) and not held by safe
//! mode; Pause Hideout greys it, as a bound key does nothing while paused. Nothing but the
//! user's click presses it: no fact (game mode included) adds, hides or presses an item.
//!
//! Fed from the router's events thread like [`crate::marks`]; the UI thread applies a change
//! once (`take_change`). Window-free: the model is plain data, tested without a tray.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use hideout_contract::module::{fact_true, Module, Slot};
use serde_json::Value;

/// At most this many characters of an item's words; longer ones end in "…".
pub const LABEL_MAX: usize = 40;

/// An `action` panel from a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ActionPanel {
    id: String,
    title: String,
    when: Option<String>,
}

/// What a module's manifest says about its quick actions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Decl {
    name: String,
    panels: Vec<ActionPanel>,
    /// The declared `[[actions]]`: id and `when`.
    actions: Vec<(String, Option<String>)>,
}

/// The running module's words for an item, from its registered tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Live {
    /// The button's node id (a patch names it).
    node: String,
    label: Option<String>,
    disabled: bool,
}

/// One tray menu item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The item's number for its menu id; the same (module, action) keeps it for the run.
    pub n: u32,
    pub module: String,
    pub action: String,
    /// The words, with the bound key after a tab when there is one.
    pub text: String,
    pub enabled: bool,
}

/// Module states (§7.8) in which a module's items don't show: off, or down until the user acts.
const HIDDEN_STATES: &[&str] = &[
    "off",
    "crash-stopped",
    "install-failed",
    "revoked",
    "needs-hub",
    "quarantined",
];

/// The tray menu's quick actions.
#[derive(Debug, Default)]
pub struct QuickActions {
    modules: BTreeMap<String, Decl>,
    /// Every fact's truth by the `when` rule (§4.6): all a `when` needs.
    truthy: HashMap<String, bool>,
    /// `module.<id>.state` by module id.
    states: HashMap<String, String>,
    safe_mode: bool,
    paused: bool,
    /// Bound keys by (module, action).
    keys: HashMap<(String, String), String>,
    /// The running modules' words by (module, panel).
    live: HashMap<(String, String), Live>,
    /// Item numbers: index = n. Never reused in one run, so a stale menu pick finds nothing.
    targets: Vec<(String, String)>,
    /// What the menu has now (the last `take_change`).
    shown: Option<Vec<Item>>,
}

/// The words as a menu shows them: control and format characters dropped (no tab can fake a
/// key), whitespace folded, `&` doubled (a single one would underline the next letter), at
/// most [`LABEL_MAX`] characters.
pub fn menu_words(raw: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    let mut count = 0;
    for c in raw
        .chars()
        .filter(|c| !hideout_supervisor::words::hidden_char(*c))
    {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if count == LABEL_MAX {
            let trimmed = out.trim_end().len();
            out.truncate(trimmed);
            out.push('\u{2026}');
            return out.replace('&', "&&");
        }
        if space {
            out.push(' ');
            count += 1;
            space = false;
        }
        out.push(c);
        count += 1;
    }
    out.replace('&', "&&")
}

/// The first `button` node in a tree, depth first.
fn first_button(node: &Value) -> Option<&Value> {
    if node.get("type").and_then(Value::as_str) == Some("button") {
        return Some(node);
    }
    node.get("children")
        .and_then(Value::as_array)?
        .iter()
        .find_map(first_button)
}

fn live_of(button: &Value) -> Option<Live> {
    let props = button.get("props");
    Some(Live {
        node: button.get("id")?.as_str()?.to_string(),
        label: props
            .and_then(|p| p.get("label"))
            .and_then(Value::as_str)
            .map(String::from),
        disabled: props
            .and_then(|p| p.get("disabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

impl QuickActions {
    /// A module's manifest (each time it is read): its `action` panels and `[[actions]]`.
    pub fn manifest(&mut self, m: &Module) {
        let decl = Decl {
            name: m.name.clone(),
            panels: m
                .panels
                .iter()
                .filter(|p| p.slot == Slot::Action)
                .map(|p| ActionPanel {
                    id: p.id.clone(),
                    title: p.title.clone(),
                    when: p.when.clone(),
                })
                .collect(),
            actions: m
                .actions
                .iter()
                .map(|a| (a.decl.id.clone(), a.when.clone()))
                .collect(),
        };
        for p in &decl.panels {
            if !decl.actions.iter().any(|(a, _)| a == &p.id) {
                log::warn!(
                    "{}: action panel {:?} names no [[actions]] entry; no tray item",
                    m.id,
                    p.id
                );
            }
        }
        self.modules.insert(m.id.clone(), decl);
    }

    /// The module left the inventory.
    pub fn removed(&mut self, module: &str) {
        self.modules.remove(module);
        self.live.retain(|(m, _), _| m != module);
    }

    /// A hub fact.
    pub fn fact(&mut self, key: &str, value: &Value) {
        if key == "hub.safe_mode" {
            self.safe_mode = value.as_bool() == Some(true);
        }
        if let Some(id) = key
            .strip_prefix("module.")
            .and_then(|r| r.strip_suffix(".state"))
        {
            match value.as_str() {
                Some(s) => {
                    self.states.insert(id.into(), s.into());
                }
                None => {
                    self.states.remove(id);
                }
            }
            // Its words are the running module's only while it runs.
            if value.as_str() != Some("running") {
                self.live.retain(|(m, _), _| m != id);
            }
        }
        if fact_true(value) {
            self.truthy.insert(key.into(), true);
        } else {
            self.truthy.remove(key);
        }
    }

    /// A module's accepted `ui.register` panels (the router checked them against its manifest):
    /// the words of its `action` panels' first button.
    pub fn panels(&mut self, module: &str, panels: &Value) {
        let Some(decl) = self.modules.get(module) else {
            return;
        };
        let ids: Vec<String> = decl.panels.iter().map(|p| p.id.clone()).collect();
        self.live.retain(|(m, _), _| m != module);
        for p in panels.as_array().into_iter().flatten() {
            let Some(id) = p.get("id").and_then(Value::as_str) else {
                continue;
            };
            if !ids.iter().any(|i| i == id) {
                continue;
            }
            if let Some(live) = p.get("tree").and_then(first_button).and_then(live_of) {
                self.live.insert((module.into(), id.into()), live);
            }
        }
    }

    /// A module's `ui.patch`: a `set` on an action panel's button changes its words or grey.
    pub fn patch(&mut self, module: &str, params: &Value) {
        let Some(panel) = params.get("panel").and_then(Value::as_str) else {
            return;
        };
        let Some(live) = self.live.get_mut(&(module.to_string(), panel.to_string())) else {
            return;
        };
        for op in params
            .get("ops")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let set = op.get("op").and_then(Value::as_str) == Some("set");
            if !set || op.get("node").and_then(Value::as_str) != Some(live.node.as_str()) {
                continue;
            }
            let Some(props) = op.get("props") else {
                continue;
            };
            if let Some(l) = props.get("label").and_then(Value::as_str) {
                live.label = Some(l.to_string());
            }
            if let Some(d) = props.get("disabled") {
                live.disabled = d.as_bool().unwrap_or(false);
            }
        }
    }

    /// The bound keys (module, action, chord), all of them.
    pub fn set_keys(&mut self, keys: &[(String, String, String)]) {
        self.keys = keys
            .iter()
            .map(|(m, a, k)| ((m.clone(), a.clone()), k.clone()))
            .collect();
    }

    pub fn set_paused(&mut self, on: bool) {
        self.paused = on;
    }

    fn when(&self, w: Option<&str>) -> bool {
        let Some(w) = w.map(str::trim).filter(|w| !w.is_empty()) else {
            return true;
        };
        match w.strip_prefix('!') {
            Some(name) => !self.truthy.contains_key(name.trim()),
            None => self.truthy.contains_key(w),
        }
    }

    /// The number of (module, action), made on first use.
    fn number(&mut self, module: &str, action: &str) -> u32 {
        match self
            .targets
            .iter()
            .position(|(m, a)| m == module && a == action)
        {
            Some(i) => i as u32,
            None => {
                self.targets.push((module.into(), action.into()));
                (self.targets.len() - 1) as u32
            }
        }
    }

    /// The menu's items now, in module id order, then the manifest's order.
    pub fn items(&mut self) -> Vec<Item> {
        if self.safe_mode {
            return Vec::new();
        }
        let mut wanted = Vec::new();
        for (id, decl) in &self.modules {
            let state = self.states.get(id).map(String::as_str);
            if state.is_some_and(|s| HIDDEN_STATES.contains(&s)) {
                continue;
            }
            for p in &decl.panels {
                let Some((_, action_when)) = decl.actions.iter().find(|(a, _)| a == &p.id) else {
                    continue;
                };
                if !self.when(p.when.as_deref()) || !self.when(action_when.as_deref()) {
                    continue;
                }
                let live = self.live.get(&(id.clone(), p.id.clone()));
                let words = live
                    .and_then(|l| l.label.as_deref())
                    .filter(|l| !menu_words(l).is_empty())
                    .unwrap_or(&p.title);
                let mut text = menu_words(words);
                if let Some(k) = self.keys.get(&(id.clone(), p.id.clone())) {
                    text = format!("{text}\t{}", menu_words(k));
                }
                let enabled = !self.paused && !live.is_some_and(|l| l.disabled);
                wanted.push((id.clone(), p.id.clone(), text, enabled));
            }
        }
        wanted
            .into_iter()
            .map(|(module, action, text, enabled)| Item {
                n: self.number(&module, &action),
                module,
                action,
                text,
                enabled,
            })
            .collect()
    }

    /// The items, when they differ from what the menu has; they then count as shown.
    pub fn take_change(&mut self) -> Option<Vec<Item>> {
        let items = self.items();
        if self.shown.as_ref() == Some(&items) {
            return None;
        }
        self.shown = Some(items.clone());
        Some(items)
    }

    /// Whether the items differ from what the menu has (a wake is worth it).
    pub fn changed(&mut self) -> bool {
        let items = self.items();
        self.shown.as_ref() != Some(&items)
    }

    /// What item `n` presses: (module, action), only while it is in the menu and not grey. A
    /// pick of an item that has since gone or greyed does nothing.
    pub fn target(&self, n: u32) -> Option<(String, String)> {
        self.shown
            .as_ref()?
            .iter()
            .find(|i| i.n == n && i.enabled)
            .map(|i| (i.module.clone(), i.action.clone()))
    }

    /// A module's name for people ("AI"), its id when no manifest gave one.
    pub fn name(&self, module: &str) -> String {
        self.modules
            .get(module)
            .map(|d| d.name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| module.to_string())
    }

    /// An item's words as the manifest says them ("Free GPU"), the action id without one.
    pub fn title(&self, module: &str, action: &str) -> String {
        self.modules
            .get(module)
            .and_then(|d| d.panels.iter().find(|p| p.id == action))
            .map(|p| p.title.clone())
            .unwrap_or_else(|| action.to_string())
    }
}

/// A notification about a tray press that didn't happen: title, body, the module to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub module: String,
    /// "Open AI".
    pub button: String,
}

/// The longest module reason a notification shows, in bytes (cut on a whole character).
pub const REASON_MAX: usize = 64;

/// The words when a tray press didn't happen. `not_started`: the module didn't start in time;
/// otherwise it refused with `reason` (its `data.reason`, cleaned, at most [`REASON_MAX`] bytes).
pub fn failure_notice(
    name: &str,
    title: &str,
    module: &str,
    not_started: bool,
    reason: &str,
) -> Notice {
    let clean = |s: &str| hideout_supervisor::words::clean_note(s);
    let (name, title) = (clean(name), clean(title));
    let (t, body) = if not_started {
        (
            format!("{name} didn't start"),
            format!("So \u{201c}{title}\u{201d} didn't happen. Try again from the tray menu."),
        )
    } else {
        let mut r = clean(reason);
        if r.len() > REASON_MAX {
            let mut cut = REASON_MAX - '\u{2026}'.len_utf8();
            while !r.is_char_boundary(cut) {
                cut -= 1;
            }
            r.truncate(cut);
            r.push('\u{2026}');
        }
        if r.is_empty() {
            r = format!("{name} said no.");
        }
        (format!("Couldn't {}", lower_first(&title)), r)
    };
    Notice {
        title: t,
        body,
        module: module.to_string(),
        button: hideout_supervisor::words::open_module(&name),
    }
}

/// "Free GPU" → "free GPU" (an all-caps word stays as it is).
fn lower_first(s: &str) -> String {
    let mut words = s.splitn(2, ' ');
    let first = words.next().unwrap_or_default();
    let rest = words.next();
    let first = if first.chars().skip(1).any(|c| c.is_uppercase()) {
        first.to_string()
    } else {
        first.to_lowercase()
    };
    match rest {
        Some(r) => format!("{first} {r}"),
        None => first,
    }
}

static QA: Mutex<Option<QuickActions>> = Mutex::new(None);
type Wake = Arc<dyn Fn() + Send + Sync>;
/// (module, action) to `action.invoke {source: "tray"}` (the feed's tray invoker).
pub type Invoker = Arc<dyn Fn(&str, &str) + Send + Sync>;
static WAKER: Mutex<Option<Wake>> = Mutex::new(None);
/// What a tray press calls: (module, action) → `action.invoke {source: "tray"}`.
static INVOKER: Mutex<Option<Invoker>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut QuickActions) -> R) -> R {
    let mut g = QA.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(QuickActions::default))
}

/// Wakes the UI thread when the menu would change. The model's lock is released first: the
/// waker never runs under it.
fn wake_if_changed() {
    if !with(|q| q.changed()) {
        return;
    }
    let w = WAKER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(w) = w {
        w();
    }
}

/// A module's manifest (the hub calls it for each manifest it reads).
pub fn note_manifest(m: &Module) {
    with(|q| q.manifest(m));
    wake_if_changed();
}

/// The module left the inventory.
pub fn note_removed(module: &str) {
    with(|q| q.removed(module));
    wake_if_changed();
}

/// A hub fact (the hub calls it for every fact).
pub fn note_fact(key: &str, value: &Value) {
    with(|q| q.fact(key, value));
    wake_if_changed();
}

/// A module's accepted panels.
pub fn note_panels(module: &str, panels: &Value) {
    with(|q| q.panels(module, panels));
    wake_if_changed();
}

/// A module's `ui.patch`.
pub fn note_patch(module: &str, params: &Value) {
    with(|q| q.patch(module, params));
    wake_if_changed();
}

/// The bound keys, all of them (the shell calls it at start and when they change).
pub(crate) fn note_keys(keys: &[(String, String, String)]) {
    with(|q| q.set_keys(keys));
    wake_if_changed();
}

/// Pause Hideout from the tray menu.
pub(crate) fn note_paused(on: bool) {
    with(|q| q.set_paused(on));
    wake_if_changed();
}

/// How the UI thread gets woken for a change.
pub(crate) fn set_waker(w: Wake) {
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
}

/// What a tray press calls (the feed's tray invoker).
pub(crate) fn set_invoker(i: Option<Invoker>) {
    *INVOKER.lock().unwrap_or_else(|e| e.into_inner()) = i;
}

/// The change for the UI thread to apply to the menu, if any.
pub(crate) fn take_change() -> Option<Vec<Item>> {
    with(|q| q.take_change())
}

/// The user picked item `n` in the tray menu: its action goes to the module with source
/// `tray`. Neither lock is held while the invoker runs (it takes the router's).
pub(crate) fn press(n: u32) {
    let Some((module, action)) = with(|q| q.target(n)) else {
        log::info!("tray: quick action {n} is no longer in the menu; nothing pressed");
        return;
    };
    let invoke = INVOKER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match invoke {
        Some(i) => {
            log::info!("tray: {module}/{action} pressed");
            i(&module, &action);
        }
        None => log::warn!("tray: {module}/{action}: no router in this hub"),
    }
}

/// The module's name and the item's words, for a notification.
pub fn words_for(module: &str, action: &str) -> (String, String) {
    with(|q| (q.name(module), q.title(module, action)))
}

#[cfg(test)]
mod tests;
