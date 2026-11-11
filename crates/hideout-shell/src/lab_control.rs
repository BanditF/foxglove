// SPDX-License-Identifier: MIT
//! Lab control of a lab hub (#1003): drive the tray, the keyboard and the pages from a script,
//! and read back what the hub drew, so the lab checks of #995, #997 and #1004 run without hands.
//!
//! How: `hideout.exe --lab-do <verb> [args]` (see [`request`]) connects to the lab hub's control
//! pipe and sends one line; the hub's pipe thread answers it ([`answer`]). The verbs:
//!
//! - **Tray** (`tray-click`, `tray-double-click`, `tray-menu`, `tray-key-select`): the mouse
//!   messages Windows sends a notification-area icon are posted to tray-icon's own message
//!   window as its callback message (`WM_USER_TRAYICON`, 6002). tray-icon's window procedure
//!   turns them into its `TrayIconEvent`s (and opens the menu for a right button up), and the
//!   hub's handler gets them as from a real click: the same code past the library's handler.
//!   `tray-key-select` posts what the shell sends for Enter or Space on the icon with this hub's
//!   notify version (tray-icon sets none, so version 0: `WM_RBUTTONDOWN` + `WM_RBUTTONUP`).
//! - **Tray menu** (`menu-key <key>`, `menu-pick <text>`): keys posted to the open menu window
//!   (`#32768`, the menu's own keyboard loop), or the picked item's `WM_COMMAND` posted to the
//!   tray window, which muda's subclass turns into the `MenuEvent` the hub handles.
//! - **Window** (`key <chord>`, `text <text>`, `click <name>`, `focus <name>`, `open-flyout`,
//!   `close-flyout`, `close-window`): keys and text go into the next egui pass of the target
//!   window as input events (an egui plugin's input hook, before the hub's frame reads input);
//!   `click` and `focus` are AccessKit action requests on the node with that accessible name, what
//!   a screen reader or UI Automation sends. `close-window` posts `WM_CLOSE` to the main window.
//! - **Read back** (`state`, `frames <ms> [<from_t>]`): the accessible tree of each window as
//!   the hub last drew it (names, roles, selected, focus), the tabs and the active page, the
//!   flyout, other viewports (pop-outs), the foreground window, the tray menu, the tray events
//!   received and the loop-wake and frame counters; and a frame log, one row per pass of each
//!   window with what was on screen, for the double-click flicker check.
//! - **Supervisor states** (#1096: `update-ready`, `update-waiting`, `updating`, `quarantine`):
//!   the update verbs go to the hub's own handler ([`set_hub_verbs`], the supervisor's update
//!   step for a lab module); `quarantine` puts a module's panels in the panel host's quarantine
//!   in the next frame, which hands it to the supervisor as a broken panel does. A hub that set
//!   no handler (not a lab hub) refuses them.
//!
//! What it does not prove (each stays one hand check, docs/hideout-lab.md): that Windows
//! delivers a real click, double click or key to the icon (the shell's own messages, its
//! double-click timing and `Shell_NotifyIcon` focus handling), that the keyboard reaches the
//! hub window through winit (keys enter at egui's input), foreground rights a real click grants
//! (`SetForegroundWindow` from a posted message may be refused), or how anything looks.
//!
//! Cost: only a lab hub serves it: lab mode on (`HIDEOUT_LAB=1` with a `HIDEOUT_CONFIG_DIR` that
//! is not the user's folder, decided by the registry's `lab_mode` and passed in with [`allow`]),
//! and never on the default folder. The user's hub starts no thread, no pipe and no plugin. A
//! lab hub has one thread blocked in `ConnectNamedPipe` and a plugin that counts passes; the
//! accessible tree is built and kept only after the first request ("armed").

use crate::paths::HubPaths;
use eframe::egui::{self, accesskit, Event, Key, Modifiers, ViewportId};
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Set by `hideout.exe` once the registry has decided lab mode is on for this hub's folder.
static ALLOWED: AtomicBool = AtomicBool::new(false);

/// Event-loop wakes since start: `wake::Gate` runs `shell::tray_sync` once per wake (at
/// `about_to_wait`), which counts here.
static WAKES: AtomicU64 = AtomicU64::new(0);

/// One event-loop wake (from `shell::tray_sync`); one relaxed add.
pub fn note_wake() {
    WAKES.fetch_add(1, Ordering::Relaxed);
}

/// The lab hub's control state, while lab control runs.
static LAB: OnceLock<Arc<Lab>> = OnceLock::new();

/// How long an action waits for the pass that takes it (fallback only: a pass comes within
/// milliseconds of the wake).
pub const PASS_TIMEOUT: Duration = Duration::from_secs(2);
/// How long an action waits for the frame after it (the one that shows its result).
pub const SETTLE_TIMEOUT: Duration = Duration::from_millis(500);
/// The longest frame log one `frames` asks for.
pub const MAX_FRAMES_MS: u64 = 10_000;
/// Rows kept in the frame log.
const LOG_ROWS: usize = 4096;
/// Nodes kept per window.
const MAX_NODES: usize = 600;
/// The longest request line read.
const MAX_LINE: u64 = 4096;

/// tray-icon 0.24's private callback message (`WM_USER_TRAYICON`); `tray::tests` pins the
/// version the constant was read from.
pub const WM_USER_TRAYICON: u32 = 6002;

/// Lab mode was decided on for this hub (`hideout.exe`, from `hideout_registry::lab`).
pub fn allow(on: bool) {
    ALLOWED.store(on, Ordering::Release);
}

/// Whether this hub serves lab control: lab mode allowed and a folder of its own. Never for the
/// default folder, whatever the environment or [`allow`] says.
pub fn enabled(paths: &HubPaths) -> bool {
    paths.overridden && ALLOWED.load(Ordering::Acquire)
}

/// `\\.\pipe\hideout-lab-<user><suffix>`: one per user and lab folder, the router pipe's rule
/// (contract §8.2, #1010).
pub fn pipe_name(paths: &HubPaths) -> String {
    format!(
        r"\\.\pipe\hideout-lab-{}{}",
        hideout_router::server::user(),
        paths.instance_suffix()
    )
}

/// A window of the hub, as the verbs name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Win {
    Main,
    Flyout,
}

impl Win {
    pub fn name(self) -> &'static str {
        match self {
            Win::Main => "main",
            Win::Flyout => "flyout",
        }
    }
}

/// One accessible node as drawn.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Node {
    #[serde(skip)]
    pub id: accesskit::NodeId,
    pub role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
}

impl Node {
    /// What it says: its name, or a label's text (egui puts a label's words in its value).
    pub fn text(&self) -> &str {
        if self.name.is_empty() {
            &self.value
        } else {
            &self.name
        }
    }
}

/// A window's last drawn tree.
#[derive(Clone, Debug, Default)]
struct Tree {
    nodes: Vec<Node>,
    at_ms: u64,
    title: String,
}

/// One pass of one window.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FrameRow {
    pub t: u64,
    pub w: String,
    /// The main window was on screen (shown, not minimised).
    pub main_on_screen: bool,
    /// The flyout's window was visible.
    pub flyout_visible: bool,
}

#[derive(Default)]
struct Inner {
    /// Input for the next pass of a window: (window, events, ticket).
    queue: Vec<(Win, Vec<Event>, Modifiers, u64)>,
    next_ticket: u64,
    /// The last ticket a pass took, and the root passes finished when it was taken.
    taken: u64,
    taken_pass: u64,
    /// Root passes finished (every flyout pass runs inside one).
    root_passes: u64,
    frames: VecDeque<FrameRow>,
    counts: HashMap<String, u64>,
    trees: HashMap<String, Tree>,
    /// The viewports whose passes are running (an immediate viewport's pass runs inside the
    /// root's).
    stack: Vec<(ViewportId, String)>,
    last_tray: String,
    /// Settings to change in the hub's next frame (`set-setting`): (module, name, value, ticket).
    settings: Vec<(String, String, serde_json::Value, u64)>,
    /// Modules whose panels the hub's next frame quarantines (`quarantine`, #1096): (module,
    /// ticket).
    quarantines: Vec<(String, u64)>,
}

/// The control state the plugin, the tray hook and the pipe thread share.
pub struct Lab {
    start: Instant,
    armed: AtomicBool,
    tray_events: AtomicU64,
    flyout: ViewportId,
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl Lab {
    pub fn new(flyout: ViewportId) -> Self {
        Self {
            start: Instant::now(),
            armed: AtomicBool::new(false),
            tray_events: AtomicU64::new(0),
            flyout,
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Milliseconds since lab control started (the clock every answer uses).
    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn window_of(&self, id: ViewportId, title: &str) -> String {
        if id == ViewportId::ROOT {
            "main".into()
        } else if id == self.flyout {
            "flyout".into()
        } else {
            format!("viewport:{title}")
        }
    }

    /// Queues input for `win`'s next pass; returns its ticket.
    pub fn queue(&self, win: Win, events: Vec<Event>, modifiers: Modifiers) -> u64 {
        let mut i = self.lock();
        i.next_ticket += 1;
        let t = i.next_ticket;
        i.queue.push((win, events, modifiers, t));
        t
    }

    /// Queues a settings change for the hub's next frame (`set-setting`); returns its ticket.
    pub fn queue_setting(&self, module: &str, name: &str, value: serde_json::Value) -> u64 {
        let mut i = self.lock();
        i.next_ticket += 1;
        let t = i.next_ticket;
        i.settings.push((module.into(), name.into(), value, t));
        t
    }

    /// The queued settings changes, for the hub's frame to make as the Settings form does.
    pub fn take_settings(&self) -> Vec<(String, String, serde_json::Value)> {
        let mut i = self.lock();
        let all = std::mem::take(&mut i.settings);
        let Some(took) = all.iter().map(|s| s.3).max() else {
            return Vec::new();
        };
        i.taken = i.taken.max(took);
        i.taken_pass = i.root_passes;
        drop(i);
        self.cv.notify_all();
        all.into_iter().map(|(m, n, v, _)| (m, n, v)).collect()
    }

    /// Queues a panel quarantine for the hub's next frame (`quarantine`, #1096); its ticket.
    pub fn queue_quarantine(&self, module: &str) -> u64 {
        let mut i = self.lock();
        i.next_ticket += 1;
        let t = i.next_ticket;
        i.quarantines.push((module.into(), t));
        t
    }

    /// The queued quarantines, for the hub's frame to hand to the panel host as its own
    /// quarantine does (`PanelHost::quarantine`, then the supervisor through the hand-off).
    pub fn take_quarantines(&self) -> Vec<String> {
        let mut i = self.lock();
        let all = std::mem::take(&mut i.quarantines);
        let Some(took) = all.iter().map(|q| q.1).max() else {
            return Vec::new();
        };
        i.taken = i.taken.max(took);
        i.taken_pass = i.root_passes;
        drop(i);
        self.cv.notify_all();
        all.into_iter().map(|(m, _)| m).collect()
    }

    /// Waits until a pass took `ticket` and the root pass around it ended (at most `timeout`);
    /// returns the root passes so far. An action no pass took is dropped, so a later pass
    /// doesn't act on it.
    pub fn wait_taken(&self, ticket: u64, timeout: Duration) -> Result<u64, String> {
        let deadline = Instant::now() + timeout;
        let mut i = self.lock();
        let mut taken_at = None;
        loop {
            if taken_at.is_none() && i.taken >= ticket {
                // The root pass that was running when it was taken ends past this count.
                taken_at = Some(i.taken_pass);
            }
            if let Some(at) = taken_at {
                if i.stack.is_empty() && i.root_passes > at {
                    return Ok(i.root_passes);
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                if taken_at.is_some() {
                    return Ok(i.root_passes);
                }
                i.queue.retain(|q| q.3 != ticket);
                i.settings.retain(|q| q.3 != ticket);
                i.quarantines.retain(|q| q.1 != ticket);
                return Err(format!(
                    "no pass of the window within {} s (is it on screen?)",
                    timeout.as_secs()
                ));
            }
            i = self
                .cv
                .wait_timeout(i, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Waits for a root pass after `passes` (the frame that shows an action's result), at most
    /// `timeout`; false when none came (a hidden window with the flyout closed draws nothing).
    pub fn wait_pass_after(&self, passes: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut i = self.lock();
        while !(i.stack.is_empty() && i.root_passes > passes) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            i = self
                .cv
                .wait_timeout(i, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        true
    }

    /// A tray event reached the hub's handler.
    pub fn note_tray(&self, what: &str) {
        self.tray_events.fetch_add(1, Ordering::AcqRel);
        let t = self.now_ms();
        self.lock().last_tray = format!("{what} at {t}");
    }

    /// Turns the accessible tree on; from now on each pass keeps its window's tree.
    pub fn arm(&self) -> bool {
        !self.armed.swap(true, Ordering::AcqRel)
    }

    pub fn armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }

    /// The nodes `win` last drew.
    pub fn nodes(&self, win: Win) -> Vec<Node> {
        self.lock()
            .trees
            .get(win.name())
            .map(|t| t.nodes.clone())
            .unwrap_or_default()
    }

    /// The frame log rows from `from` to `to` (ms) and the passes per window in it.
    pub fn frames(&self, from: u64, to: u64) -> (Vec<FrameRow>, HashMap<String, u64>) {
        let i = self.lock();
        let rows: Vec<FrameRow> = i
            .frames
            .iter()
            .filter(|r| r.t >= from && r.t <= to)
            .cloned()
            .collect();
        let mut counts = HashMap::new();
        for r in &rows {
            *counts.entry(r.w.clone()).or_insert(0) += 1;
        }
        (rows, counts)
    }

    fn begin(&self, input: &mut egui::RawInput) {
        let id = input.viewport_id;
        let title = input
            .viewports
            .get(&id)
            .and_then(|v| v.title.clone())
            .unwrap_or_default();
        let mut i = self.lock();
        let name = self.window_of(id, &title);
        i.stack.push((id, title));
        let win = match name.as_str() {
            "main" => Win::Main,
            "flyout" => Win::Flyout,
            _ => return,
        };
        let mut took = 0;
        let mut rest = Vec::new();
        for (w, events, mods, ticket) in std::mem::take(&mut i.queue) {
            if w == win {
                if mods != Modifiers::NONE {
                    input.modifiers = mods;
                }
                input.events.extend(events);
                took = took.max(ticket);
            } else {
                rest.push((w, events, mods, ticket));
            }
        }
        i.queue = rest;
        if took > 0 {
            i.taken = i.taken.max(took);
            i.taken_pass = i.root_passes;
            self.cv.notify_all();
        }
    }

    fn end(&self, output: &egui::FullOutput, on_screen: (bool, bool)) {
        let t = self.now_ms();
        let mut i = self.lock();
        let Some((id, title)) = i.stack.pop() else {
            return;
        };
        let name = self.window_of(id, &title);
        *i.counts.entry(name.clone()).or_insert(0) += 1;
        if i.frames.len() >= LOG_ROWS {
            i.frames.pop_front();
        }
        i.frames.push_back(FrameRow {
            t,
            w: name.clone(),
            main_on_screen: on_screen.0,
            flyout_visible: on_screen.1,
        });
        if self.armed() {
            if let Some(u) = output.platform_output.accesskit_update.as_ref() {
                let nodes = nodes_of(u);
                i.trees.insert(
                    name,
                    Tree {
                        nodes,
                        at_ms: t,
                        title,
                    },
                );
            }
        }
        if id == ViewportId::ROOT {
            i.root_passes += 1;
        }
        drop(i);
        self.cv.notify_all();
    }
}

/// The named nodes of a tree update, in tree order (parents before children).
fn nodes_of(u: &accesskit::TreeUpdate) -> Vec<Node> {
    let by_id: HashMap<accesskit::NodeId, &accesskit::Node> =
        u.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let root = u.tree.as_ref().map(|t| t.root);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut todo: Vec<accesskit::NodeId> = root.into_iter().collect();
    // Depth-first in child order; nodes no parent names come after, in update order.
    let push = |id: accesskit::NodeId, n: &accesskit::Node, out: &mut Vec<Node>| {
        let name = n.label().unwrap_or_default().to_string();
        let value = n.value().unwrap_or_default().to_string();
        if (name.is_empty() && value.is_empty()) || out.len() >= MAX_NODES {
            return;
        }
        out.push(Node {
            id,
            role: format!("{:?}", n.role()),
            name,
            value,
            selected: n.is_selected(),
            disabled: n.is_disabled(),
            focused: id == u.focus,
        });
    };
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(n) = by_id.get(&id) {
            push(id, n, &mut out);
            todo.extend(n.children().iter().rev().copied());
        }
    }
    for (id, n) in &u.nodes {
        if seen.insert(*id) {
            push(*id, n, &mut out);
        }
    }
    out
}

/// The egui plugin: takes queued input into a window's pass, counts passes, keeps the tree.
pub struct Plugin {
    lab: Arc<Lab>,
    on_screen: fn() -> (bool, bool),
}

impl Plugin {
    pub fn new(lab: Arc<Lab>, on_screen: fn() -> (bool, bool)) -> Self {
        Self { lab, on_screen }
    }
}

impl egui::Plugin for Plugin {
    fn debug_name(&self) -> &'static str {
        "hideout.lab-control"
    }

    fn input_hook(&mut self, input: &mut egui::RawInput) {
        self.lab.begin(input);
    }

    fn output_hook(&mut self, output: &mut egui::FullOutput) {
        self.lab.end(output, (self.on_screen)());
    }
}

/// A tray event reached the hub's tray handler (the hook in `shell::run`); free while lab control
/// is off.
/// Settings changes `set-setting` queued for this frame (none on the user's hub: no lab).
pub fn take_settings() -> Vec<(String, String, serde_json::Value)> {
    LAB.get().map(|l| l.take_settings()).unwrap_or_default()
}

/// Panel quarantines `quarantine` queued for this frame (none on the user's hub: no lab).
pub fn take_quarantines() -> Vec<String> {
    LAB.get().map(|l| l.take_quarantines()).unwrap_or_default()
}

/// The hub's own lab verbs (#1096): the supervisor's update steps and its state, which this
/// crate can't reach. `hideout.exe` sets them in a lab hub only ([`set_hub_verbs`]); a hub
/// without them refuses those verbs.
pub type HubVerbs =
    Box<dyn Fn(&str, &str) -> Result<serde_json::Value, String> + Send + Sync + 'static>;

static HUB_VERBS: OnceLock<HubVerbs> = OnceLock::new();

/// Why a hub verb was refused when the hub set none (not a lab hub).
pub const NO_HUB_VERBS: &str = "lab hub only: this hub has no supervisor verbs";

/// Sets the hub's lab verbs, once; `hideout.exe` calls it only when [`enabled`].
pub fn set_hub_verbs(f: HubVerbs) {
    let _ = HUB_VERBS.set(f);
}

pub fn note_tray(what: &dyn std::fmt::Debug) {
    if let Some(lab) = LAB.get() {
        lab.note_tray(&format!("{what:?}"));
    }
}

/// A key chord: `ctrl+k`, `shift+tab`, `esc`, `enter`, `down`, `f6`, `alt+left`.
pub fn parse_chord(chord: &str) -> Result<(Key, Modifiers), String> {
    let mut mods = Modifiers::NONE;
    let parts: Vec<&str> = chord.split('+').map(str::trim).collect();
    let (last, rest) = parts.split_last().ok_or("empty chord")?;
    for m in rest {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                mods.ctrl = true;
                mods.command = true;
            }
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            other => return Err(format!("unknown modifier `{other}` in `{chord}`")),
        }
    }
    let alias = match last.to_ascii_lowercase().as_str() {
        "esc" | "escape" => "Escape",
        "enter" | "return" => "Enter",
        "space" => "Space",
        "tab" => "Tab",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        _ => "",
    };
    let name = if alias.is_empty() {
        last.to_ascii_uppercase()
    } else {
        alias.to_string()
    };
    let key = Key::from_name(&name).ok_or_else(|| format!("unknown key `{last}` in `{chord}`"))?;
    Ok((key, mods))
}

/// The press and release of a chord, as egui-winit would send them.
pub fn chord_events(key: Key, modifiers: Modifiers) -> Vec<Event> {
    [true, false]
        .into_iter()
        .map(|pressed| Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

/// The node named `name`: an exact name first, then one whose name starts with `name, ` (a tab
/// or sidebar entry with its state, "Art, stopped after 5 crashes"). `role` prefers that role.
pub fn find_node<'a>(nodes: &'a [Node], name: &str, role: Option<&str>) -> Option<&'a Node> {
    let exact = |n: &&Node| n.text() == name;
    let prefixed = |n: &&Node| n.text().starts_with(&format!("{name}, "));
    let roled = |n: &&Node| role.is_none_or(|r| n.role == r);
    nodes
        .iter()
        .filter(roled)
        .find(exact)
        .or_else(|| nodes.iter().filter(roled).find(prefixed))
        .or_else(|| nodes.iter().find(exact))
        .or_else(|| nodes.iter().find(prefixed))
}

/// The `nth` (from 1) node with exactly `name`, or [`find_node`] for the first.
pub fn find_nth<'a>(
    nodes: &'a [Node],
    name: &str,
    role: Option<&str>,
    nth: usize,
) -> Option<&'a Node> {
    if nth <= 1 {
        return find_node(nodes, name, role);
    }
    nodes.iter().filter(|n| n.text() == name).nth(nth - 1)
}

/// What the hub's platform side does for the pipe thread (the real Win32 calls in the hub; fakes
/// in tests).
pub trait Host: Send + 'static {
    /// Wakes the hub for a pass (the root, which also runs an open flyout's).
    fn wake(&self);
    /// Turns egui's accessible tree on.
    fn arm(&self);
    /// (main window on screen, flyout open, flyout window visible).
    fn windows(&self) -> (bool, bool, bool);
    /// Posts tray-icon's callback message with `mouse` (a `WM_*BUTTON*` id) to its window.
    fn post_tray(&self, mouse: u32) -> Result<(), String>;
    /// Posts `WM_CLOSE` to the main window.
    fn close_main(&self) -> Result<(), String>;
    /// The foreground window: main, flyout, menu, other or none.
    fn foreground(&self) -> &'static str;
    /// The flyout window's rectangle, its monitor's and that monitor's work area (physical
    /// pixels, `[left, top, right, bottom]`), while it is open.
    fn flyout_geometry(&self) -> Option<serde_json::Value> {
        None
    }
    /// The open tray menu's items, if one is open.
    fn menu(&self) -> Option<Vec<MenuItem>>;
    /// Posts a key (a virtual-key code) to the open tray menu.
    fn menu_key(&self, vk: u16) -> Result<(), String>;
    /// Chooses the open tray menu's item `id`: closes the menu and posts its `WM_COMMAND`.
    fn menu_pick(&self, id: u32) -> Result<(), String>;
    /// One of the hub's own lab verbs (#1096: `update-ready`, `update-waiting`, `updating`,
    /// `quarantine` (the check before it is queued) and `module-state`), from
    /// [`set_hub_verbs`]. Refused when the hub set none.
    fn hub_verb(&self, verb: &str, rest: &str) -> Result<serde_json::Value, String> {
        match HUB_VERBS.get() {
            Some(f) => f(verb, rest),
            None => Err(format!("{NO_HUB_VERBS} ({verb})")),
        }
    }
}

/// One item of the tray menu.
#[derive(Clone, Debug, serde::Serialize)]
pub struct MenuItem {
    pub text: String,
    #[serde(skip)]
    pub id: u32,
    pub enabled: bool,
    pub checked: bool,
    pub highlighted: bool,
}

const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;

/// The verbs and their arguments, for `--lab-do help` and errors.
pub const HELP: &str = "verbs: state | frames <ms> [<from_t>] | tray-click | tray-double-click [<gap_ms>] | \
tray-menu | tray-key-select | menu-key <down|up|left|right|enter|esc|home|end> | menu-pick <text> | \
key <chord> [main|flyout] | text <text> | click <name>[#n]|@<i> [main|flyout] | focus <tab or page> | \
open-flyout | close-flyout | close-window | set-setting <module> <name> <value> | \
update-ready <module> [<reason>] | update-waiting <module> <permission>... | updating <module> | \
quarantine <module>";

/// Answers one request line. Every answer is one JSON object with `ok` (and `error` when not).
pub fn answer(line: &str, lab: &Lab, host: &dyn Host) -> String {
    if lab.arm() {
        host.arm();
        // A first pass with the tree on, so `state` has something to read.
        host.wake();
    }
    let line = line.trim_end_matches(['\r', '\n']);
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    let reply = match run_verb(verb, rest, lab, host) {
        Ok(mut v) => {
            v["ok"] = true.into();
            v
        }
        Err(e) => serde_json::json!({"ok": false, "error": e}),
    };
    let mut s = reply.to_string();
    s.push('\n');
    s
}

fn done(lab: &Lab, verb: &str) -> serde_json::Value {
    serde_json::json!({"verb": verb, "t": lab.now_ms()})
}

fn run_verb(
    verb: &str,
    rest: &str,
    lab: &Lab,
    host: &dyn Host,
) -> Result<serde_json::Value, String> {
    let (main_on, fly_open, _) = host.windows();
    match verb {
        "state" => Ok(state(lab, host)),
        "frames" => {
            let mut a = rest.split_whitespace();
            let ms: u64 = a
                .next()
                .ok_or("frames <ms> [<from_t>]")?
                .parse()
                .map_err(|_| "frames: <ms> is a number")?;
            let ms = ms.min(MAX_FRAMES_MS);
            let from = match a.next() {
                Some(f) => f.parse().map_err(|_| "frames: <from_t> is a number")?,
                None => lab.now_ms(),
            };
            // A bounded wait for the window asked for (it ends at `from + ms`, at most 10 s).
            let until = from + ms;
            let now = lab.now_ms();
            if until > now {
                std::thread::sleep(Duration::from_millis((until - now).min(MAX_FRAMES_MS)));
            }
            let (rows, counts) = lab.frames(from, until);
            let flyout_visible: Vec<u64> = rows
                .iter()
                .filter(|r| r.flyout_visible)
                .map(|r| r.t)
                .collect();
            Ok(serde_json::json!({
                "from": from, "to": until, "counts": counts, "rows": rows,
                "flyout_visible_first": flyout_visible.first(),
                "flyout_visible_last": flyout_visible.last(),
            }))
        }
        "tray-click" => {
            host.post_tray(WM_LBUTTONDOWN)?;
            host.post_tray(WM_LBUTTONUP)?;
            Ok(done(lab, verb))
        }
        "tray-double-click" => {
            // Windows' order for a double click on an icon; the gap is the time between the two
            // clicks, so the first click's frame can run as it would for a person.
            let gap: u64 = if rest.is_empty() {
                150
            } else {
                rest.parse().map_err(|_| "tray-double-click [<gap_ms>]")?
            };
            let t = lab.now_ms();
            host.post_tray(WM_LBUTTONDOWN)?;
            host.post_tray(WM_LBUTTONUP)?;
            std::thread::sleep(Duration::from_millis(gap.min(1000)));
            host.post_tray(WM_LBUTTONDBLCLK)?;
            host.post_tray(WM_LBUTTONUP)?;
            Ok(serde_json::json!({"verb": verb, "t": t, "gap_ms": gap}))
        }
        // Enter or Space on the icon (Win+B, arrows): with notify version 0 the shell sends the
        // right-button pair, so the keyboard path is the menu's.
        "tray-menu" | "tray-key-select" => {
            host.post_tray(WM_RBUTTONDOWN)?;
            host.post_tray(WM_RBUTTONUP)?;
            Ok(done(lab, verb))
        }
        "menu-key" => {
            let vk = match rest.to_ascii_lowercase().as_str() {
                "down" => 0x28,
                "up" => 0x26,
                "left" => 0x25,
                "right" => 0x27,
                "enter" | "return" => 0x0D,
                "esc" | "escape" => 0x1B,
                "home" => 0x24,
                "end" => 0x23,
                other => return Err(format!("menu-key: unknown key `{other}`")),
            };
            host.menu_key(vk)?;
            Ok(done(lab, verb))
        }
        "menu-pick" => {
            let items = host
                .menu()
                .ok_or("no tray menu is open (tray-menu first)")?;
            let want = rest.to_lowercase();
            let item = items
                .iter()
                .find(|i| i.text.to_lowercase() == want)
                .ok_or_else(|| format!("the tray menu has no `{rest}`"))?;
            if !item.enabled {
                return Err(format!("`{}` is greyed out", item.text));
            }
            host.menu_pick(item.id)?;
            Ok(done(lab, verb))
        }
        "key" | "text" | "click" | "focus" | "close-flyout" => {
            let (arg, target) = split_target(rest);
            let win = match target {
                Some(w) => w,
                None if verb == "close-flyout" => Win::Flyout,
                None if verb == "focus" => Win::Main,
                None => default_window(host, main_on, fly_open),
            };
            let shown = match win {
                Win::Main => main_on,
                Win::Flyout => fly_open,
            };
            if !shown {
                if verb == "close-flyout" {
                    return Ok(
                        serde_json::json!({"verb": verb, "t": lab.now_ms(), "already": true}),
                    );
                }
                return Err(format!(
                    "the {} window is not on screen (main: tray-double-click or menu-pick Open Hideout; flyout: open-flyout)",
                    win.name()
                ));
            }
            let (events, mods) = match verb {
                "key" => {
                    let (k, m) = parse_chord(arg)?;
                    (chord_events(k, m), m)
                }
                "close-flyout" => (chord_events(Key::Escape, Modifiers::NONE), Modifiers::NONE),
                "text" => {
                    if arg.is_empty() {
                        return Err("text <text>".into());
                    }
                    (vec![Event::Text(arg.to_string())], Modifiers::NONE)
                }
                _ => {
                    let nodes = lab.nodes(win);
                    let role = (verb == "focus").then_some("Tab");
                    // `<name>#<n>`: the n-th node with that name (tiles share "···").
                    let (name, nth) = match arg.rsplit_once('#') {
                        Some((a, n)) if n.parse::<usize>().is_ok() => (a, n.parse().unwrap_or(1)),
                        _ => (arg, 1),
                    };
                    // `@<i>`: the i-th node of `state` (a text field has no name of its own).
                    let at = arg.strip_prefix('@').and_then(|i| i.parse::<usize>().ok());
                    let found = match at {
                        Some(i) => nodes.get(i),
                        None => find_nth(&nodes, name, role, nth),
                    };
                    let node = found.ok_or_else(|| {
                        format!("no node named `{arg}` in the {} window", win.name())
                    })?;
                    let mut ev = vec![Event::AccessKitActionRequest(accesskit::ActionRequest {
                        action: accesskit::Action::Focus,
                        target: node.id,
                        data: None,
                    })];
                    ev.push(Event::AccessKitActionRequest(accesskit::ActionRequest {
                        action: accesskit::Action::Click,
                        target: node.id,
                        data: None,
                    }));
                    (ev, Modifiers::NONE)
                }
            };
            let ticket = lab.queue(win, events, mods);
            host.wake();
            let t = lab.now_ms();
            let passes = lab.wait_taken(ticket, PASS_TIMEOUT)?;
            host.wake();
            let settled = lab.wait_pass_after(passes, SETTLE_TIMEOUT);
            Ok(serde_json::json!({"verb": verb, "t": t, "window": win.name(), "settled": settled}))
        }
        "set-setting" => {
            // The Settings form's own path (`PanelHost::set_setting`, then `settings.set` to the
            // module), for a field a script can't type into (a file picker, #995.3).
            let mut a = rest.splitn(3, ' ');
            let (Some(module), Some(name), Some(raw)) = (a.next(), a.next(), a.next()) else {
                return Err("set-setting <module> <name> <value>".into());
            };
            if !main_on {
                return Err(
                    "the main window is not on screen (tray-double-click or menu-pick Open Hideout)"
                        .into(),
                );
            }
            let value = setting_value(raw);
            let ticket = lab.queue_setting(module, name, value.clone());
            host.wake();
            let t = lab.now_ms();
            let passes = lab.wait_taken(ticket, PASS_TIMEOUT)?;
            host.wake();
            let settled = lab.wait_pass_after(passes, SETTLE_TIMEOUT);
            Ok(
                serde_json::json!({"verb": verb, "t": t, "module": module, "name": name,
                "value": value, "settled": settled}),
            )
        }
        // The supervisor's update states (#1096), lab modules of a lab hub only: through its
        // update step and the module's real drain call (driver_lab.rs in hideout-supervisor).
        "update-ready" | "update-waiting" | "updating" => {
            if rest.is_empty() {
                return Err(format!("{verb} <module> ..."));
            }
            let t = lab.now_ms();
            let mut v = host.hub_verb(verb, rest)?;
            v["verb"] = verb.into();
            v["t"] = t.into();
            Ok(v)
        }
        // A panel quarantine (#1096): the panel host's own quarantine in the hub's next frame,
        // which hands the module to the supervisor (§5.5, v1.10), as when a panel breaks.
        "quarantine" => {
            let module = rest;
            if module.is_empty() || module.contains(' ') {
                return Err("quarantine <module>".into());
            }
            // Lab hub and lab module only, before anything is queued.
            host.hub_verb("quarantine", module)?;
            if !main_on {
                return Err(
                    "the main window is not on screen (tray-double-click or menu-pick Open Hideout)"
                        .into(),
                );
            }
            let ticket = lab.queue_quarantine(module);
            host.wake();
            let t = lab.now_ms();
            let passes = lab.wait_taken(ticket, PASS_TIMEOUT)?;
            host.wake();
            let settled = lab.wait_pass_after(passes, SETTLE_TIMEOUT);
            let after = host.hub_verb("module-state", &format!("{module} quarantined"))?;
            Ok(serde_json::json!({"verb": verb, "t": t, "module": module,
                "settled": settled, "state": after["state"]}))
        }
        "open-flyout" => {
            if fly_open {
                return Ok(serde_json::json!({"verb": verb, "t": lab.now_ms(), "already": true}));
            }
            host.post_tray(WM_LBUTTONDOWN)?;
            host.post_tray(WM_LBUTTONUP)?;
            Ok(done(lab, verb))
        }
        "close-window" => {
            host.close_main()?;
            Ok(done(lab, verb))
        }
        "help" | "" => Ok(serde_json::json!({"help": HELP})),
        other => Err(format!("unknown verb `{other}`; {HELP}")),
    }
}

/// A `set-setting` value: JSON when it parses as JSON (`10`, `true`, `"x"`), else the text as
/// a string (a Windows path).
fn setting_value(raw: &str) -> serde_json::Value {
    let raw = raw.trim();
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

/// `<arg> [main|flyout]`.
fn split_target(rest: &str) -> (&str, Option<Win>) {
    for (suffix, w) in [(" main", Win::Main), (" flyout", Win::Flyout)] {
        if let Some(a) = rest.strip_suffix(suffix) {
            return (a.trim(), Some(w));
        }
    }
    (rest, None)
}

/// The window keys go to when none is named: the foreground one of the two, else the flyout if it
/// is open, else the main window.
fn default_window(host: &dyn Host, main_on: bool, fly_open: bool) -> Win {
    match host.foreground() {
        "flyout" if fly_open => Win::Flyout,
        "main" if main_on => Win::Main,
        _ if fly_open => Win::Flyout,
        _ => Win::Main,
    }
}

fn state(lab: &Lab, host: &dyn Host) -> serde_json::Value {
    let (main_on, fly_open, fly_visible) = host.windows();
    let i = lab.lock();
    let tree = |w: &str| -> serde_json::Value {
        match i.trees.get(w) {
            Some(t) => serde_json::json!({
                "title": t.title,
                "tree_t": t.at_ms,
                "focus": t.nodes.iter().find(|n| n.focused).map(|n| n.text().to_string()),
                "nodes": t.nodes,
            }),
            None => serde_json::json!({"nodes": []}),
        }
    };
    let main_tree = i.trees.get("main");
    let tabs: Vec<serde_json::Value> = main_tree
        .map(|t| {
            t.nodes
                .iter()
                .filter(|n| n.role == "Tab")
                .map(|n| serde_json::json!({"name": n.name, "selected": n.selected == Some(true)}))
                .collect()
        })
        .unwrap_or_default();
    let active = main_tree.and_then(|t| {
        t.nodes
            .iter()
            .find(|n| n.role == "Tab" && n.selected == Some(true))
            .map(|n| n.name.clone())
    });
    let mut main = tree("main");
    main["on_screen"] = main_on.into();
    main["passes"] = i.counts.get("main").copied().unwrap_or(0).into();
    let mut flyout = tree("flyout");
    flyout["open"] = fly_open.into();
    flyout["visible"] = fly_visible.into();
    flyout["passes"] = i.counts.get("flyout").copied().unwrap_or(0).into();
    if let Some(g) = host.flyout_geometry() {
        flyout["geometry"] = g;
    }
    let others: Vec<serde_json::Value> = i
        .counts
        .iter()
        .filter(|(k, _)| k.starts_with("viewport:"))
        .map(|(k, n)| {
            let mut v = tree(k);
            v["window"] = k.trim_start_matches("viewport:").into();
            v["passes"] = (*n).into();
            v
        })
        .collect();
    serde_json::json!({
        "t": lab.now_ms(),
        "tabs": tabs,
        "active_page": active,
        "main": main,
        "flyout": flyout,
        "popouts": others,
        "foreground": host.foreground(),
        "tray": {
            "events": lab.tray_events.load(Ordering::Acquire),
            "last": i.last_tray,
            "menu": host.menu(),
        },
        "loop": {"wakes": WAKES.load(Ordering::Relaxed), "root_passes": i.root_passes},
    })
}

/// Serves lab control on `name` from a thread of its own: one request per connection, one at a
/// time. Blocks in `ConnectNamedPipe` between requests (no polling). The pipe is current-user
/// only and refuses remote clients (`ServerPipe`).
pub fn serve(name: &str, lab: Arc<Lab>, host: impl Host) -> io::Result<()> {
    use foxglove_plugin::channel::ServerPipe;
    let first = ServerPipe::create(name, true)?;
    let name = name.to_string();
    std::thread::Builder::new()
        .name("hideout-lab".into())
        .spawn(move || {
            let mut next = Some(first);
            loop {
                let pipe = match next.take() {
                    Some(p) => p,
                    None => match ServerPipe::create(&name, false) {
                        Ok(p) => p,
                        Err(e) => {
                            log::error!("lab control: {name}: {e}");
                            return;
                        }
                    },
                };
                let accepted = pipe.accept();
                // The next instance listens before this answer goes out, so a client that asks
                // again at once always finds the pipe.
                next = ServerPipe::create(&name, false).ok();
                let mut conn = match accepted {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("lab control: {name}: accept: {e}");
                        continue;
                    }
                };
                let mut line = String::new();
                let read = BufReader::new(conn.clone())
                    .take(MAX_LINE)
                    .read_line(&mut line);
                let reply = match read {
                    Ok(_) => {
                        log::info!("lab control: {}", line.trim_end());
                        answer(&line, &lab, &host)
                    }
                    Err(e) => format!(
                        "{}\n",
                        serde_json::json!({"ok": false, "error": format!("reading the request: {e}")})
                    ),
                };
                let _ = conn.write_all(reply.as_bytes());
            }
        })?;
    Ok(())
}

/// The client side (`hideout.exe --lab-do <verb> [args]`): sends one line, returns the answer.
pub fn request(pipe: &str, line: &str) -> io::Result<String> {
    let mut conn = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)?;
    conn.write_all(format!("{line}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(conn).read_to_string(&mut reply)?;
    Ok(reply)
}

/// Whether an answer says `ok`.
pub fn answer_ok(reply: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(reply.trim())
        .ok()
        .and_then(|v| v.get("ok").and_then(|o| o.as_bool()))
        == Some(true)
}

/// Starts lab control in the hub (`shell::run`, the one hook): the plugin on `ctx`, the pipe
/// thread with the hub's Win32 side. `None` (nothing started) unless [`enabled`].
pub fn start(paths: &HubPaths, ctx: &egui::Context, flyout: ViewportId) -> Option<()> {
    if !enabled(paths) {
        return None;
    }
    let lab = Arc::new(Lab::new(flyout));
    let name = pipe_name(paths);
    let host = win32::Hub { ctx: ctx.clone() };
    if let Err(e) = serve(&name, lab.clone(), host) {
        log::warn!("lab control: {name}: {e}");
        return None;
    }
    ctx.add_plugin(Plugin::new(lab.clone(), win32::on_screen));
    let _ = LAB.set(lab);
    log::info!("lab control: on, {name}");
    Some(())
}

/// The hub's real Win32 side of [`Host`].
mod win32 {
    use super::{Host, MenuItem};
    use crate::platform;
    use eframe::egui;
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetForegroundWindow, GetMenuItemCount, GetMenuItemID, GetMenuState,
        GetMenuStringW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SendMessageW,
        HMENU, MF_BYPOSITION, MF_CHECKED, MF_DISABLED, MF_GRAYED, MF_HILITE, MF_SEPARATOR,
        WM_CLOSE, WM_COMMAND, WM_KEYDOWN, WM_KEYUP,
    };

    /// `MN_GETHMENU`: the menu a `#32768` window shows.
    const MN_GETHMENU: u32 = 0x01E1;

    pub struct Hub {
        pub ctx: egui::Context,
    }

    fn hwnd(h: isize) -> Option<HWND> {
        (h != 0).then_some(HWND(h as *mut _))
    }

    pub fn on_screen() -> (bool, bool) {
        let fly =
            hwnd(platform::flyout_hwnd()).is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() });
        (platform::main_window_on_screen(), fly)
    }

    /// This process's visible menu window (`#32768`), if a menu is open.
    fn menu_window() -> Option<HWND> {
        let me = std::process::id();
        let mut after: Option<HWND> = None;
        loop {
            let h = unsafe { FindWindowExW(None, after, w!("#32768"), None) }.ok()?;
            if h.0.is_null() {
                return None;
            }
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            if pid == me && unsafe { IsWindowVisible(h).as_bool() } {
                return Some(h);
            }
            after = Some(h);
        }
    }

    impl Host for Hub {
        fn wake(&self) {
            self.ctx.request_repaint();
        }

        fn arm(&self) {
            self.ctx.enable_accesskit();
        }

        fn windows(&self) -> (bool, bool, bool) {
            let (main, fly) = on_screen();
            (main, crate::flyout::live(), fly)
        }

        fn post_tray(&self, mouse: u32) -> Result<(), String> {
            let h = hwnd(crate::tray::message_window()).ok_or("the tray icon isn't registered")?;
            unsafe {
                PostMessageW(
                    Some(h),
                    super::WM_USER_TRAYICON,
                    WPARAM(0),
                    LPARAM(mouse as isize),
                )
            }
            .map_err(|e| e.to_string())
        }

        fn close_main(&self) -> Result<(), String> {
            let h = hwnd(platform::main_hwnd()).ok_or("no main window")?;
            unsafe { PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)) }
                .map_err(|e| e.to_string())
        }

        fn foreground(&self) -> &'static str {
            let fg = unsafe { GetForegroundWindow() };
            if fg.0.is_null() {
                return "none";
            }
            let fg = fg.0 as isize;
            if fg == platform::main_hwnd() {
                "main"
            } else if fg == platform::flyout_hwnd() && fg != 0 {
                "flyout"
            } else if menu_window().is_some_and(|m| m.0 as isize == fg) {
                "menu"
            } else if fg == crate::tray::message_window() {
                // tray-icon makes its window the foreground one while its menu is open.
                "tray"
            } else {
                "other"
            }
        }

        fn flyout_geometry(&self) -> Option<serde_json::Value> {
            use windows::Win32::Foundation::RECT;
            use windows::Win32::Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            };
            use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
            let h = hwnd(platform::flyout_hwnd())?;
            let mut r = RECT::default();
            unsafe { GetWindowRect(h, &mut r) }.ok()?;
            let mon = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !unsafe { GetMonitorInfoW(mon, &mut info) }.as_bool() {
                return None;
            }
            let a = |r: RECT| [r.left, r.top, r.right, r.bottom];
            Some(serde_json::json!({
                "rect": a(r), "monitor": a(info.rcMonitor), "work": a(info.rcWork)
            }))
        }

        fn menu(&self) -> Option<Vec<MenuItem>> {
            let m = menu_window()?;
            let hm = unsafe { SendMessageW(m, MN_GETHMENU, Some(WPARAM(0)), Some(LPARAM(0))) };
            let hm = HMENU(hm.0 as *mut _);
            let n = unsafe { GetMenuItemCount(Some(hm)) };
            let mut items = Vec::new();
            for i in 0..n.max(0) as u32 {
                let st = unsafe { GetMenuState(hm, i, MF_BYPOSITION) };
                if st & MF_SEPARATOR.0 != 0 {
                    continue;
                }
                let mut buf = [0u16; 256];
                let len = unsafe { GetMenuStringW(hm, i, Some(&mut buf), MF_BYPOSITION) };
                items.push(MenuItem {
                    text: String::from_utf16_lossy(&buf[..len.max(0) as usize]).replace('&', ""),
                    id: unsafe { GetMenuItemID(hm, i as i32) },
                    enabled: st & (MF_GRAYED.0 | MF_DISABLED.0) == 0,
                    checked: st & MF_CHECKED.0 != 0,
                    highlighted: st & MF_HILITE.0 != 0,
                });
            }
            Some(items)
        }

        fn menu_key(&self, vk: u16) -> Result<(), String> {
            let m = menu_window().ok_or("no tray menu is open (tray-menu first)")?;
            let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as isize;
            let down = 1 | (scan << 16);
            let up = down | (0b11 << 30);
            unsafe {
                PostMessageW(Some(m), WM_KEYDOWN, WPARAM(vk as usize), LPARAM(down))
                    .and_then(|()| PostMessageW(Some(m), WM_KEYUP, WPARAM(vk as usize), LPARAM(up)))
            }
            .map_err(|e| e.to_string())
        }

        fn menu_pick(&self, id: u32) -> Result<(), String> {
            let tray =
                hwnd(crate::tray::message_window()).ok_or("the tray icon isn't registered")?;
            // Esc ends the menu's loop; then the item's command, as the menu sends it on a pick.
            self.menu_key(0x1B)?;
            unsafe { PostMessageW(Some(tray), WM_COMMAND, WPARAM(id as usize), LPARAM(0)) }
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn lab_paths() -> HubPaths {
        HubPaths::resolve(|k| (k == "HIDEOUT_CONFIG_DIR").then(|| OsString::from(r"C:\lab\h")))
            .unwrap()
    }

    #[test]
    fn only_an_allowed_lab_folder_serves_lab_control() {
        let user =
            HubPaths::resolve(|k| (k == "LOCALAPPDATA").then(|| OsString::from(r"C:\u"))).unwrap();
        allow(false);
        assert!(!enabled(&lab_paths()));
        allow(true);
        assert!(enabled(&lab_paths()));
        // The user's own hub never does, whatever was allowed.
        assert!(!enabled(&user));
        allow(false);
        assert!(pipe_name(&lab_paths()).ends_with(&lab_paths().instance_suffix()));
        assert!(pipe_name(&lab_paths()).starts_with(r"\\.\pipe\hideout-lab-"));
    }

    #[test]
    fn chords_parse() {
        let (k, m) = parse_chord("ctrl+k").unwrap();
        assert_eq!(k, Key::K);
        assert!(m.ctrl && m.command && !m.shift);
        assert_eq!(parse_chord("esc").unwrap(), (Key::Escape, Modifiers::NONE));
        assert_eq!(parse_chord("Shift+Tab").unwrap().0, Key::Tab);
        assert!(parse_chord("shift+tab").unwrap().1.shift);
        assert_eq!(parse_chord("down").unwrap().0, Key::ArrowDown);
        assert_eq!(parse_chord("f6").unwrap().0, Key::F6);
        assert!(parse_chord("hyper+k").is_err());
        assert!(parse_chord("ctrl+nokey").is_err());
        assert_eq!(split_target("ctrl+k flyout"), ("ctrl+k", Some(Win::Flyout)));
        assert_eq!(split_target("Open Hideout"), ("Open Hideout", None));
    }

    /// A fake platform side: the windows it reports and the tray messages posted.
    #[derive(Clone, Default)]
    struct Fake {
        posted: Arc<Mutex<Vec<u32>>>,
        woke: Arc<AtomicU64>,
        main_on: bool,
        fly_open: bool,
        menu: Option<Vec<MenuItem>>,
        picked: Arc<Mutex<Vec<u32>>>,
    }

    impl Host for Fake {
        fn wake(&self) {
            self.woke.fetch_add(1, Ordering::AcqRel);
        }
        fn arm(&self) {}
        fn windows(&self) -> (bool, bool, bool) {
            (self.main_on, self.fly_open, self.fly_open)
        }
        fn post_tray(&self, mouse: u32) -> Result<(), String> {
            self.posted.lock().unwrap().push(mouse);
            Ok(())
        }
        fn close_main(&self) -> Result<(), String> {
            Ok(())
        }
        fn foreground(&self) -> &'static str {
            "other"
        }
        fn menu(&self) -> Option<Vec<MenuItem>> {
            self.menu.clone()
        }
        fn menu_key(&self, _vk: u16) -> Result<(), String> {
            Ok(())
        }
        fn menu_pick(&self, id: u32) -> Result<(), String> {
            self.picked.lock().unwrap().push(id);
            Ok(())
        }
    }

    fn json(s: &str) -> serde_json::Value {
        serde_json::from_str(s.trim()).unwrap()
    }

    /// The tray verbs post the mouse messages Windows sends an icon, in its order, and nothing
    /// else; the menu verbs find items by text.
    #[test]
    fn tray_verbs_post_windows_messages_in_order() {
        let lab = Lab::new(ViewportId::from_hash_of("fly"));
        let fake = Fake {
            menu: Some(vec![
                MenuItem {
                    text: "Open Hideout".into(),
                    id: 7,
                    enabled: true,
                    checked: false,
                    highlighted: false,
                },
                MenuItem {
                    text: "Hideout".into(),
                    id: 3,
                    enabled: false,
                    checked: false,
                    highlighted: false,
                },
            ]),
            ..Fake::default()
        };
        let take = || std::mem::take(&mut *fake.posted.lock().unwrap());
        assert!(answer_ok(&answer("tray-click", &lab, &fake)));
        assert_eq!(take(), vec![WM_LBUTTONDOWN, WM_LBUTTONUP]);
        let r = json(&answer("tray-double-click 10\n", &lab, &fake));
        assert_eq!(r["gap_ms"], 10);
        assert_eq!(
            take(),
            vec![WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_LBUTTONUP]
        );
        for v in ["tray-menu", "tray-key-select"] {
            assert!(answer_ok(&answer(v, &lab, &fake)));
            assert_eq!(take(), vec![WM_RBUTTONDOWN, WM_RBUTTONUP]);
        }
        assert!(answer_ok(&answer("menu-pick open hideout", &lab, &fake)));
        assert_eq!(*fake.picked.lock().unwrap(), vec![7]);
        assert!(!answer_ok(&answer("menu-pick Hideout", &lab, &fake)));
        assert!(!answer_ok(&answer("menu-pick Nope", &lab, &fake)));
        assert!(!answer_ok(&answer("menu-key sideways", &lab, &fake)));
        assert!(!answer_ok(&answer("bogus", &lab, &fake)));
        // Window verbs refuse a window that isn't on screen, and post nothing.
        let r = json(&answer("key ctrl+k", &lab, &fake));
        assert!(
            r["error"].as_str().unwrap().contains("not on screen"),
            "{r}"
        );
        assert!(answer_ok(&answer("close-flyout", &lab, &fake)));
        assert!(take().is_empty());
    }

    /// Window-free: a headless context with the plugin. A queued chord and an AccessKit click
    /// reach the pass as input, the frame log counts the passes, and `state` lists the drawn
    /// nodes and the focus.
    #[test]
    fn input_reaches_the_pass_and_state_reads_the_tree() {
        let lab = Arc::new(Lab::new(ViewportId::from_hash_of("fly")));
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.add_plugin(Plugin::new(lab.clone(), || (true, false)));
        let fake = Fake {
            main_on: true,
            ..Fake::default()
        };
        let clicks = Arc::new(AtomicU64::new(0));
        let chords = Arc::new(AtomicU64::new(0));
        let pass = {
            let (clicks, chords) = (clicks.clone(), chords.clone());
            move |ctx: &egui::Context| {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
                        chords.fetch_add(1, Ordering::AcqRel);
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if ui.button("Open Hideout").clicked() {
                            clicks.fetch_add(1, Ordering::AcqRel);
                        }
                        ui.label("Focus timer");
                    });
                });
            }
        };
        pass(&ctx);
        // `state` arms the tree; the next pass keeps it.
        assert!(answer_ok(&answer("state", &lab, &fake)));
        pass(&ctx);
        let s = json(&answer("state", &lab, &fake));
        let names: Vec<&str> = s["main"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str().or(n["value"].as_str()))
            .collect();
        assert!(
            names.contains(&"Open Hideout") && names.contains(&"Focus timer"),
            "{s}"
        );
        // A chord and a click from the pipe thread, answered after the passes that take them.
        let hub = {
            let ctx = ctx.clone();
            let lab = lab.clone();
            let pass = pass.clone();
            std::thread::spawn(move || {
                // The hub side: passes until both actions are answered.
                for _ in 0..400 {
                    pass(&ctx);
                    std::thread::sleep(Duration::from_millis(5));
                    if lab.lock().taken >= 2 && lab.lock().queue.is_empty() {
                        pass(&ctx);
                        pass(&ctx);
                        break;
                    }
                }
            })
        };
        let r = json(&answer("key ctrl+k", &lab, &fake));
        assert_eq!(r["ok"], true, "{r}");
        let r = json(&answer("click Open Hideout", &lab, &fake));
        assert_eq!(r["ok"], true, "{r}");
        hub.join().unwrap();
        assert_eq!(chords.load(Ordering::Acquire), 1);
        assert_eq!(clicks.load(Ordering::Acquire), 1);
        let s = json(&answer("state", &lab, &fake));
        assert_eq!(s["main"]["focus"], "Open Hideout", "{s}");
        assert!(s["main"]["passes"].as_u64().unwrap() >= 4);
        let r = json(&answer("click Nothing here", &lab, &fake));
        assert!(r["error"].as_str().unwrap().contains("no node named"));
        let f = json(&answer(&format!("frames {} 0", lab.now_ms()), &lab, &fake));
        assert!(f["counts"]["main"].as_u64().unwrap() >= 1, "{f}");
        assert!(f["flyout_visible_first"].is_null());
    }

    /// An action no pass takes (the window went away) times out, and is dropped so a later pass
    /// doesn't act on it.
    #[test]
    fn an_untaken_action_times_out_and_is_dropped() {
        let lab = Lab::new(ViewportId::from_hash_of("fly"));
        let t = lab.queue(Win::Main, vec![], Modifiers::NONE);
        assert!(lab.wait_taken(t, Duration::from_millis(30)).is_err());
        assert!(lab.lock().queue.is_empty());
        assert!(!lab.wait_pass_after(0, Duration::from_millis(10)));
    }

    /// `set-setting` (#995.3): the value as JSON or as text, taken by the hub's next frame.
    #[test]
    fn a_queued_setting_is_taken_once_by_the_frame() {
        assert_eq!(setting_value("10"), serde_json::json!(10));
        assert_eq!(setting_value("true"), serde_json::json!(true));
        assert_eq!(
            setting_value(r"C:\lab dir\vendor sdk.dll"),
            serde_json::json!(r"C:\lab dir\vendor sdk.dll")
        );
        let lab = Lab::new(ViewportId::from_hash_of("fly"));
        let t = lab.queue_setting("org.example.module", "sdk-file", serde_json::json!("x.dll"));
        assert_eq!(
            lab.take_settings(),
            vec![(
                "org.example.module".into(),
                "sdk-file".into(),
                serde_json::json!("x.dll")
            )]
        );
        assert!(lab.take_settings().is_empty());
        assert!(lab.lock().taken >= t);
        // Untaken: dropped on the timeout, as other actions are.
        let t = lab.queue_setting("m", "n", serde_json::json!(1));
        assert!(lab.wait_taken(t, Duration::from_millis(30)).is_err());
        assert!(lab.take_settings().is_empty());
    }

    /// A host with the hub's verbs (#1096): records what reached them and answers a state.
    struct WithHub {
        fake: Fake,
        asked: Arc<Mutex<Vec<(String, String)>>>,
        refuse: bool,
    }

    impl Host for WithHub {
        fn wake(&self) {
            self.fake.wake()
        }
        fn arm(&self) {}
        fn windows(&self) -> (bool, bool, bool) {
            self.fake.windows()
        }
        fn post_tray(&self, mouse: u32) -> Result<(), String> {
            self.fake.post_tray(mouse)
        }
        fn close_main(&self) -> Result<(), String> {
            Ok(())
        }
        fn foreground(&self) -> &'static str {
            "other"
        }
        fn menu(&self) -> Option<Vec<MenuItem>> {
            None
        }
        fn menu_key(&self, _vk: u16) -> Result<(), String> {
            Ok(())
        }
        fn menu_pick(&self, _id: u32) -> Result<(), String> {
            Ok(())
        }
        fn hub_verb(&self, verb: &str, rest: &str) -> Result<serde_json::Value, String> {
            self.asked.lock().unwrap().push((verb.into(), rest.into()));
            if self.refuse {
                return Err("org.x is not a lab module".into());
            }
            let state = match verb {
                "update-ready" => "update-busy",
                "update-waiting" => "update-waiting",
                "updating" => "updating",
                _ => "quarantined",
            };
            Ok(serde_json::json!({"module": rest.split(' ').next(), "state": state}))
        }
    }

    /// #1096: without the hub's verbs (not a lab hub) the supervisor verbs are refused and
    /// nothing is queued; with them, the update verbs reach the hub's handler as asked.
    #[test]
    fn supervisor_verbs_are_refused_without_the_hubs_handler_and_passed_with_it() {
        let lab = Lab::new(ViewportId::from_hash_of("fly"));
        let fake = Fake {
            main_on: true,
            ..Fake::default()
        };
        for line in [
            "update-ready org.x a flash",
            "update-waiting org.x devices.read",
            "updating org.x",
            "quarantine org.x",
        ] {
            let r = json(&answer(line, &lab, &fake));
            assert!(
                r["error"].as_str().unwrap().starts_with(NO_HUB_VERBS),
                "{line}: {r}"
            );
        }
        assert!(lab.lock().quarantines.is_empty());

        let asked = Arc::new(Mutex::new(Vec::new()));
        let hub = WithHub {
            fake: fake.clone(),
            asked: asked.clone(),
            refuse: false,
        };
        let r = json(&answer("update-ready org.x a flash running", &lab, &hub));
        assert_eq!(
            (r["ok"].as_bool(), r["state"].as_str()),
            (Some(true), Some("update-busy"))
        );
        assert_eq!(r["verb"], "update-ready");
        assert!(answer_ok(&answer(
            "update-waiting org.x devices.read",
            &lab,
            &hub
        )));
        assert!(answer_ok(&answer("updating org.x", &lab, &hub)));
        assert!(!answer_ok(&answer("updating", &lab, &hub)));
        assert_eq!(
            *asked.lock().unwrap(),
            vec![
                ("update-ready".into(), "org.x a flash running".into()),
                ("update-waiting".into(), "org.x devices.read".into()),
                ("updating".into(), "org.x".into()),
            ]
        );
        // The hub's check refuses (not a lab module): nothing is queued.
        let refusing = WithHub {
            fake,
            asked,
            refuse: true,
        };
        assert!(!answer_ok(&answer("quarantine org.x", &lab, &refusing)));
        assert!(lab.lock().quarantines.is_empty());
    }

    /// #1096: `quarantine` is taken by the hub's next frame (the panel host's quarantine) and
    /// answers the supervisor's state after it; a window not on screen is refused.
    #[test]
    fn a_quarantine_is_taken_by_the_next_frame() {
        let lab = Arc::new(Lab::new(ViewportId::from_hash_of("fly")));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let hidden = WithHub {
            fake: Fake::default(),
            asked: asked.clone(),
            refuse: false,
        };
        let r = json(&answer("quarantine org.x", &lab, &hidden));
        assert!(
            r["error"].as_str().unwrap().contains("not on screen"),
            "{r}"
        );
        assert!(lab.lock().quarantines.is_empty());

        let ctx = egui::Context::default();
        ctx.add_plugin(Plugin::new(lab.clone(), || (true, false)));
        let taken = Arc::new(Mutex::new(Vec::new()));
        let frames = {
            let (ctx, lab, taken) = (ctx.clone(), lab.clone(), taken.clone());
            std::thread::spawn(move || {
                // The hub side: frames until the quarantine is taken, then two more.
                for _ in 0..400 {
                    let _ = ctx.run(egui::RawInput::default(), |_| {
                        taken.lock().unwrap().extend(lab.take_quarantines());
                    });
                    if !taken.lock().unwrap().is_empty() {
                        for _ in 0..2 {
                            let _ = ctx.run(egui::RawInput::default(), |_| {});
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        let shown = WithHub {
            fake: Fake {
                main_on: true,
                ..Fake::default()
            },
            asked: asked.clone(),
            refuse: false,
        };
        let r = json(&answer("quarantine org.x", &lab, &shown));
        frames.join().unwrap();
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["state"], "quarantined");
        assert_eq!(*taken.lock().unwrap(), vec!["org.x".to_string()]);
        let asked = asked.lock().unwrap();
        assert_eq!(asked.last().unwrap().0, "module-state");
        assert_eq!(asked.last().unwrap().1, "org.x quarantined");
    }

    /// The whole path over the current-user pipe, window-free.
    #[test]
    fn a_request_round_trips_over_the_pipe() {
        let pipe = format!(r"\\.\pipe\hideout-lab-test-{}", std::process::id());
        let lab = Arc::new(Lab::new(ViewportId::from_hash_of("fly")));
        let fake = Fake::default();
        let posted = fake.posted.clone();
        serve(&pipe, lab.clone(), fake).unwrap();
        let r = request(&pipe, "tray-click").unwrap();
        assert!(answer_ok(&r), "{r}");
        assert_eq!(*posted.lock().unwrap(), vec![WM_LBUTTONDOWN, WM_LBUTTONUP]);
        lab.note_tray("click");
        let s = json(&request(&pipe, "state").unwrap());
        assert_eq!(s["tray"]["events"], 1);
        assert!(s["tray"]["last"].as_str().unwrap().starts_with("click at "));
        assert!(!answer_ok(&request(&pipe, "nope").unwrap()));
    }
}
