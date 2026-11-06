//! The focused keyboard (#799, C1; docs/design/device-focus-layouts-thumbnails-lights-declutter.md
//! section 1): one keyboard the whole app shows, picked from every keyboard the device layer
//! knows. Not `focus.rs`, which is window focus (#618). The model only: the switcher and the pages
//! are `focus_ui` (C2), the pins' cards `focus_pins` (C3). "Follow the keyboard I type on" (C4,
//! #802) feeds [`typed`] from `follow_typing`'s Raw Input listener: only which keyboard typed,
//! never a key.
//!
//! - **The keyboards:** the device layer reports what each source sees, on its own events: the
//!   host-link keyboard on its link's State events (`devices::start`), VIA/Vial keyboards on each
//!   scan (`via_host::rescan`), and LampArray keyboards on the lighting worker's scans
//!   (`pc_lights`). Nothing here asks a device or waits on a timer. A keyboard that goes stays
//!   listed, greyed (`connected: false`), until the app restarts, so a replug doesn't reorder
//!   the list. A LampArray that is also a host-link or VIA keyboard (the same VID/PID) is that
//!   keyboard, listed once.
//! - **Start rule:** the remembered keyboard (`[focus] keyboard`) when it connects, else the
//!   host-link keyboard, else the first one found. Until the user picks, the rule finishes as
//!   keyboards arrive: the remembered or a host-link keyboard may take an automatic focus held by
//!   one lower on that ladder, and an automatic focus whose keyboard went moves to the best
//!   connected one. Nothing else moves it.
//! - **A new keyboard never steals the focus**, except when nothing was focused.
//! - **A picked focus stays**, greyed when its keyboard goes, and a replug keeps it.
//! - **Follow the keyboard I type on** (off by default): with it on, a few keys on another
//!   keyboard move the focus there, at most once a second (`follow_typing::Follower`). A pick
//!   holds until the user types on a keyboard other than the one they typed on when they picked.
//! - **Pins:** the overlay and the wallpaper feed may each pin a keyboard (`[focus.pins]`); an
//!   unpinned view follows the focus. Their cards and the views' following are `focus_pins` (C3).
//! - **Events:** every change sets the `focus.*` facts on the app's bus ([`subscribe`]), which
//!   the pages and the local API's `facts.subscribe` get; `focus.get` / `focus.set` read and
//!   change it over the local API (API 1.12).
//!
//! With one keyboard nothing changes: it is the focus.

use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::time::Instant;

use foxglove_devices::facts::{Change, Value};
use serde::{Deserialize, Serialize};

use super::boards::Board;
use super::follow_typing::Follower;
use super::key_layout::KeyLayout;

/// The store's section.
pub const SECTION: &str = "focus";

#[cfg(test)]
thread_local! {
    /// Tests share one settings folder: one that picks through [`set`] keeps its `[focus]` in
    /// a section of its own, so it can't become another test's remembered keyboard.
    pub static TEST_SECTION: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// The store section the focus is kept in: [`SECTION`] (a test's own one in tests).
fn section() -> &'static str {
    #[cfg(test)]
    if let Some(s) = TEST_SECTION.get() {
        return s;
    }
    SECTION
}

/// Where a keyboard was found, which says what Foxglove can do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// Foxglove's own host link (ZMK firmware with the Foxglove module).
    HostLink,
    /// VIA or Vial, read-only (0.2).
    Via,
    /// A Windows Dynamic Lighting (HID LampArray) keyboard: its lights only.
    LampArray,
}

impl Source {
    /// Its name in the local API and the facts.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::HostLink => "host_link",
            Source::Via => "via",
            Source::LampArray => "lamp_array",
        }
    }

    /// Why a keyboard found this way has fewer controls, for C2's muted reason chip; `None`
    /// for the host link, which has them all.
    pub fn limit(self) -> Option<&'static str> {
        match self {
            Source::HostLink => None,
            Source::Via => Some("read-only over VIA"),
            Source::LampArray => Some("lights only, through Windows Dynamic Lighting"),
        }
    }

    /// Its muted reason chip, the same words in the status strip, the switcher's list and the
    /// Lights card (#825): "read-only" over VIA, "lights only" for a LampArray keyboard; `None`
    /// for the host link. The chip's hover is `focus_ui::read_only_hover`.
    pub fn chip(self) -> Option<&'static str> {
        match self {
            Source::HostLink => None,
            Source::Via => Some(READ_ONLY_CHIP),
            Source::LampArray => Some(LIGHTS_ONLY_CHIP),
        }
    }
}

/// [`Source::chip`] for a keyboard Foxglove reads but doesn't change (VIA).
pub const READ_ONLY_CHIP: &str = "read-only";

/// [`Source::chip`] for a keyboard whose lights alone Foxglove sets (Windows Dynamic Lighting).
pub const LIGHTS_ONLY_CHIP: &str = "lights only";

/// One keyboard as a source sees it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Stable across replugs: the host link's device id, a VIA keyboard's saved-state key, a
    /// LampArray's `vvvv:pppp[serial]` key.
    pub key: String,
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    /// The device layer's id for calls (`devices.list`, `lighting.*`), when it has one.
    pub device: Option<String>,
}

/// A keyboard the focus knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    pub key: String,
    pub name: String,
    pub source: Source,
    pub vid: u16,
    pub pid: u16,
    pub device: Option<String>,
    /// `false`: it went this run; listed greyed until the app restarts.
    pub connected: bool,
}

impl Keyboard {
    /// Foxglove can't change its keymap or firmware (VIA, LampArray): its pages are muted.
    pub fn read_only(&self) -> bool {
        self.source != Source::HostLink
    }
}

/// A view that may pin its own keyboard instead of following the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum View {
    /// The layer overlay (KeyPeek).
    Overlay,
    /// The wallpaper feed (live_feed, wallpaper_sync).
    Feed,
}

impl View {
    pub const ALL: [View; 2] = [View::Overlay, View::Feed];
}

/// The views' pinned keyboards, by key; `None` follows the focus.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pins {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed: Option<String>,
}

impl Pins {
    pub fn get(&self, view: View) -> Option<&str> {
        match view {
            View::Overlay => self.overlay.as_deref(),
            View::Feed => self.feed.as_deref(),
        }
    }

    fn set(&mut self, view: View, key: Option<String>) {
        match view {
            View::Overlay => self.overlay = key,
            View::Feed => self.feed = key,
        }
    }
}

/// The `[focus]` section.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// The keyboard the user picked last, by key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<String>,
    /// "Follow the keyboard I type on" (C4, #802): off by default.
    pub follow_typing: bool,
    pub pins: Pins,
}

/// The focus and the keyboards it chooses from. Pure: the device layer's reports and the
/// user's picks go in, and nothing here touches a device, the store or the bus.
#[derive(Clone, Debug, Default)]
pub struct Model {
    /// In the order first found this run; gone ones stay, greyed.
    keyboards: Vec<Keyboard>,
    focus: Option<String>,
    /// The user picked the focus this run: the start rule no longer moves it.
    picked: bool,
    remembered: Option<String>,
    pins: Pins,
    follow_typing: bool,
    /// Which keyboard is being typed on, debounced (C4).
    follower: Follower,
}

impl Model {
    pub fn new(saved: &Saved) -> Model {
        Model {
            remembered: saved.keyboard.clone(),
            pins: saved.pins.clone(),
            follow_typing: saved.follow_typing,
            ..Model::default()
        }
    }

    /// Every keyboard known this run, gone ones greyed, in the order first found.
    pub fn keyboards(&self) -> &[Keyboard] {
        &self.keyboards
    }

    fn find(&self, key: &str) -> Option<&Keyboard> {
        self.keyboards.iter().find(|k| k.key == key)
    }

    /// The focused keyboard (maybe gone, greyed); `None` before any keyboard was found.
    pub fn focus(&self) -> Option<&Keyboard> {
        self.find(self.focus.as_deref()?)
    }

    pub fn pins(&self) -> &Pins {
        &self.pins
    }

    pub fn follow_typing(&self) -> bool {
        self.follow_typing
    }

    /// Turns "Follow the keyboard I type on" on or off; off forgets who was typing.
    pub fn set_follow_typing(&mut self, on: bool) {
        self.follow_typing = on;
        if !on {
            self.follower = Follower::default();
        }
    }

    /// The known keyboard a typing device with these ids is: the focus when it is one, else the
    /// first connected one.
    fn typing_keyboard(&self, vid: u16, pid: u16) -> Option<String> {
        let is = |k: &&Keyboard| k.connected && (k.vid, k.pid) == (vid, pid);
        self.focus()
            .filter(is)
            .or_else(|| self.keyboards.iter().find(is))
            .map(|k| k.key.clone())
    }

    /// A keyboard with these ids typed at `at` (C4): only which device, never which key.
    /// Whether the focus moved: only with "Follow the keyboard I type on" on, only to a known
    /// keyboard, and only as the [`Follower`] allows (a few keys, at most once a second).
    pub fn typed(&mut self, vid: u16, pid: u16, at: Instant) -> bool {
        if !self.follow_typing {
            return false;
        }
        let Some(key) = self.typing_keyboard(vid, pid) else {
            return false;
        };
        if !self.follower.typed(&key, at) || self.focus.as_deref() == Some(key.as_str()) {
            return false;
        }
        self.focus = Some(key);
        // The user's own doing: the start rule no longer moves it.
        self.picked = true;
        true
    }

    /// The keyboard `view` shows: its pin when that keyboard is known this run (greyed when
    /// gone), else the focus. A pin to a keyboard not seen yet follows the focus until it is.
    pub fn for_view(&self, view: View) -> Option<&Keyboard> {
        self.pins
            .get(view)
            .and_then(|k| self.find(k))
            .or_else(|| self.focus())
    }

    /// `source` sees `seen` now (all of its keyboards): the rest of its keyboards went. Whether
    /// the list or the focus changed.
    pub fn report(&mut self, source: Source, seen: &[Seen]) -> bool {
        let before = (self.keyboards.clone(), self.focus.clone());
        for k in self.keyboards.iter_mut().filter(|k| k.source == source) {
            k.connected = seen.iter().any(|s| s.key == k.key);
        }
        for s in seen {
            match self
                .keyboards
                .iter_mut()
                .find(|k| k.key == s.key && k.source == source)
            {
                Some(k) => {
                    k.name.clone_from(&s.name);
                    k.device.clone_from(&s.device);
                    k.vid = s.vid;
                    k.pid = s.pid;
                }
                None => self.keyboards.push(Keyboard {
                    key: s.key.clone(),
                    name: s.name.clone(),
                    source,
                    vid: s.vid,
                    pid: s.pid,
                    device: s.device.clone(),
                    connected: true,
                }),
            }
        }
        self.merge_lamp_arrays();
        self.settle();
        before != (self.keyboards.clone(), self.focus.clone())
    }

    /// A LampArray with a host-link or VIA keyboard's VID/PID is that keyboard: it goes from the
    /// list, and a focus or pin on it moves to that keyboard.
    fn merge_lamp_arrays(&mut self) {
        let owner = |all: &[Keyboard], k: &Keyboard| {
            all.iter()
                .find(|o| o.source != Source::LampArray && (o.vid, o.pid) == (k.vid, k.pid))
                .map(|o| o.key.clone())
        };
        let moved: Vec<(String, String)> = self
            .keyboards
            .iter()
            .filter(|k| k.source == Source::LampArray)
            .filter_map(|k| Some((k.key.clone(), owner(&self.keyboards, k)?)))
            .collect();
        for (from, to) in moved {
            self.keyboards.retain(|k| k.key != from);
            if self.focus.as_deref() == Some(from.as_str()) {
                self.focus = Some(to.clone());
            }
            for view in View::ALL {
                if self.pins.get(view) == Some(from.as_str()) {
                    self.pins.set(view, Some(to.clone()));
                }
            }
        }
    }

    /// The start rule's ladder: the remembered keyboard, then a host-link one, then the rest.
    fn rank(&self, k: &Keyboard) -> u8 {
        if self.remembered.as_deref() == Some(k.key.as_str()) {
            0
        } else if k.source == Source::HostLink {
            1
        } else {
            2
        }
    }

    /// Applies the start rule to an automatic focus (see the module docs).
    fn settle(&mut self) {
        let best = self
            .keyboards
            .iter()
            .filter(|k| k.connected)
            .min_by_key(|k| self.rank(k))
            .map(|k| (k.key.clone(), self.rank(k)));
        let Some((best, best_rank)) = best else {
            return;
        };
        let now = self.focus().map(|k| (k.connected, self.rank(k)));
        let take = match now {
            None => true,
            Some(_) if self.picked => false,
            Some((true, rank)) => best_rank < rank,
            Some((false, _)) => true,
        };
        if take {
            self.focus = Some(best);
        }
    }

    /// The user picks `key` (a keyboard known this run, also a greyed one): it stays the focus
    /// until the user picks again, and is remembered.
    pub fn pick(&mut self, key: &str) -> Result<&Keyboard, String> {
        if self.find(key).is_none() {
            return Err(format!("no keyboard {key}"));
        }
        self.focus = Some(key.to_string());
        self.remembered = Some(key.to_string());
        self.picked = true;
        Ok(self.find(key).expect("checked above"))
    }

    /// Pins `view` to `key` (a keyboard known this run), or back to following the focus.
    pub fn pin(&mut self, view: View, key: Option<&str>) -> Result<(), String> {
        if let Some(k) = key.filter(|k| self.find(k).is_none()) {
            return Err(format!("no keyboard {k}"));
        }
        self.pins.set(view, key.map(String::from));
        Ok(())
    }

    /// The `focus.*` facts this state shows: each key with its value, or `None` to remove it.
    pub fn facts(&self) -> Vec<(&'static str, Option<Value>)> {
        let focus = self.focus();
        let text = |s: Option<&str>| s.map(|s| Value::Text(s.to_string()));
        vec![
            ("focus.keyboard", text(focus.map(|k| k.key.as_str()))),
            ("focus.name", text(focus.map(|k| k.name.as_str()))),
            ("focus.source", text(focus.map(|k| k.source.as_str()))),
            ("focus.connected", focus.map(|k| Value::Bool(k.connected))),
            (
                "focus.keyboards",
                Some(Value::Int(self.keyboards.len() as i64)),
            ),
            (
                "focus.connected_keyboards",
                Some(Value::Int(
                    self.keyboards.iter().filter(|k| k.connected).count() as i64,
                )),
            ),
            (
                "focus.overlay",
                text(self.for_view(View::Overlay).map(|k| k.key.as_str())),
            ),
            (
                "focus.feed",
                text(self.for_view(View::Feed).map(|k| k.key.as_str())),
            ),
        ]
    }
}

/// The keyboards among a lighting scan's devices: Dynamic Lighting devices whose LampArray kind
/// is a keyboard, and keyboards' own LampArrays (#754). Pure, for the worker and the tests.
pub fn lamp_array_keyboards(
    snapshot: &super::pc_lights::Snapshot,
    layouts: &std::collections::BTreeMap<(u16, u16), super::pc_lights::LampLayout>,
) -> Vec<Seen> {
    /// Windows' `LampArrayKind::Keyboard`.
    const KEYBOARD: u32 = 1;
    let ids = |key: &str| -> Option<(u16, u16)> {
        let (vid, rest) = key.split_once(':')?;
        Some((
            u16::from_str_radix(vid, 16).ok()?,
            u16::from_str_radix(rest.get(..4)?, 16).ok()?,
        ))
    };
    let mut out: Vec<Seen> = Vec::new();
    for row in &snapshot.devices {
        if row.control != foxglove_devices::pc_lighting::Control::LampArray {
            continue;
        }
        let Some((vid, pid)) = ids(&row.key) else {
            continue;
        };
        if layouts.get(&(vid, pid)).is_some_and(|l| l.kind == KEYBOARD) {
            out.push(Seen {
                key: row.key.clone(),
                name: row.name.clone(),
                vid,
                pid,
                device: None,
            });
        }
    }
    for k in &snapshot.keyboard_lamp_arrays {
        // Over USB and Bluetooth one keyboard lists two interfaces: one entry per keyboard.
        if out.iter().any(|o| (o.vid, o.pid) == (k.vid, k.pid)) {
            continue;
        }
        out.push(Seen {
            key: k.key.clone(),
            name: k.name.clone(),
            vid: k.vid,
            pid: k.pid,
            device: None,
        });
    }
    out
}

// The app's one model, loaded from the store on first use. Tests run side by side: each keeps
// its own.
#[cfg(not(test))]
static STATE: Mutex<Option<Model>> = Mutex::new(None);

#[cfg(not(test))]
fn with<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(state.get_or_insert_with(loaded))
}

#[cfg(test)]
thread_local! {
    static STATE: std::cell::RefCell<Option<Model>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn with<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    STATE.with_borrow_mut(|s| f(s.get_or_insert_with(loaded)))
}

/// The saved `[focus]`, or the defaults when there is none or the store can't be read now
/// (nothing is saved until the user picks).
fn loaded() -> Model {
    Model::new(&super::store::try_load::<Saved>(section()).unwrap_or_default())
}

/// Each keyboard's board profile, looked up once when it is first reported (on the reporting
/// thread, never the UI's), for [`layout`].
static BOARDS: Mutex<Vec<(String, Option<Board>)>> = Mutex::new(Vec::new());

fn note_board(source: Source, seen: &[Seen]) {
    let mut boards = BOARDS.lock().unwrap_or_else(|e| e.into_inner());
    for s in seen {
        if source != Source::HostLink && boards.iter().any(|(k, _)| *k == s.key) {
            continue;
        }
        // The host link's profile was just read on its connect.
        let board = match source {
            Source::HostLink => super::boards::current(),
            _ => super::boards::find(s.vid, s.pid, &s.name),
        };
        boards.retain(|(k, _)| *k != s.key);
        boards.push((s.key.clone(), board));
    }
}

/// `source` sees `seen` now (all its keyboards). Called by the device layer on its own events.
pub fn report(source: Source, seen: Vec<Seen>) {
    note_board(source, &seen);
    let changed = with(|m| {
        let before = m.focus().map(|k| (k.key.clone(), k.name.clone()));
        let changed = m.report(source, &seen);
        if let Some(after) = m
            .focus()
            .filter(|k| Some(&k.key) != before.as_ref().map(|b| &b.0))
        {
            eprintln!(
                "Foxglove: keyboard focus: {} \u{2192} {} (start rule)",
                before.as_ref().map_or("none", |b| b.1.as_str()),
                after.name
            );
        }
        changed.then(|| m.facts())
    });
    if let Some(facts) = changed {
        publish(facts);
    }
}

/// The host-link keyboard connected (`Some`) or its link dropped (`None`): from its link's
/// State events (`devices::start`).
pub fn host_link(device: Option<&foxglove_link::devices::FoxgloveLinkDevice>) {
    use foxglove_devices::Device as _;
    let seen = device
        .map(|d| {
            let info = d.info();
            Seen {
                key: info.id.to_string(),
                name: info.name.clone(),
                vid: info.id.vid,
                pid: info.id.pid,
                device: Some(info.id.to_string()),
            }
        })
        .into_iter()
        .collect();
    report(Source::HostLink, seen);
}

/// Sets the `focus.*` facts on the app's bus.
fn publish(facts: Vec<(&'static str, Option<Value>)>) {
    let store = super::devices::bus().facts();
    for (key, value) in facts {
        match value {
            Some(v) => store.set(key, v),
            None => {
                store.remove(key);
            }
        }
    }
}

/// Every change to the focus, its keyboards or the pins, as `focus.*` fact changes.
pub fn subscribe() -> Receiver<Change> {
    super::devices::bus().facts().subscribe("focus.*")
}

/// The focused keyboard (maybe gone, greyed); `None` before any keyboard was found. Never
/// blocks on a device, so the UI thread may ask.
pub fn current() -> Option<Keyboard> {
    with(|m| m.focus().cloned())
}

/// Every keyboard known this run, gone ones greyed, in the order first found.
pub fn keyboards() -> Vec<Keyboard> {
    with(|m| m.keyboards().to_vec())
}

/// The keyboard `view` shows: its pin, else the focus.
pub fn for_view(view: View) -> Option<Keyboard> {
    with(|m| m.for_view(view).cloned())
}

/// The views' pins as saved.
pub fn pins() -> Pins {
    with(|m| m.pins().clone())
}

/// "Follow the keyboard I type on" (C4): off by default.
pub fn follow_typing() -> bool {
    with(|m| m.follow_typing())
}

/// Turns "Follow the keyboard I type on" on or off, saved in `[focus] follow_typing`. The
/// listener itself is `follow_typing::set`'s.
pub fn set_follow_typing(on: bool) {
    with(|m| m.set_follow_typing(on));
    save(|s| s.follow_typing = on);
}

/// A keyboard with these ids typed at `at` (from `follow_typing`'s source): moves the focus as
/// [`Model::typed`] says and announces it. Whether it moved. Never told which key.
pub fn typed(vid: u16, pid: u16, at: Instant) -> bool {
    let facts = with(|m| {
        let before = m.focus().map(|k| k.name.clone());
        if !m.typed(vid, pid, at) {
            return None;
        }
        eprintln!(
            "Foxglove: keyboard focus: {} \u{2192} {} (typed on)",
            before.as_deref().unwrap_or("none"),
            m.focus().map_or("none", |k| k.name.as_str())
        );
        Some(m.facts())
    });
    let moved = facts.is_some();
    if let Some(facts) = facts {
        publish(facts);
    }
    moved
}

/// The user picks `key`: it becomes the focus, is remembered in `[focus] keyboard`, and the
/// change is announced.
pub fn set(key: &str) -> Result<Keyboard, String> {
    let (picked, facts) = with(|m| {
        let before = m.focus().map(|k| k.name.clone());
        let picked = m.pick(key)?.clone();
        if before.as_deref() != Some(picked.name.as_str()) {
            eprintln!(
                "Foxglove: keyboard focus: {} \u{2192} {} (picked)",
                before.as_deref().unwrap_or("none"),
                picked.name
            );
        }
        Ok::<_, String>((picked, m.facts()))
    })?;
    save(|s| s.keyboard = Some(key.to_string()));
    publish(facts);
    Ok(picked)
}

/// Pins `view` to `key`, or back to the focus (`None`); saved in `[focus.pins]`.
pub fn pin(view: View, key: Option<&str>) -> Result<(), String> {
    let facts = with(|m| {
        m.pin(view, key)?;
        Ok::<_, String>(m.facts())
    })?;
    save(|s| s.pins.set(view, key.map(String::from)));
    publish(facts);
    Ok(())
}

/// Read-modify-write of `[focus]`, so a pick and a pin can't undo each other.
fn save(change: impl FnOnce(&mut Saved)) {
    if let Err(e) = super::store::update(section(), change) {
        eprintln!("Foxglove: can't save the keyboard focus: {e}");
    }
}

/// `keyboard`'s keys, from B1's ladder (`key_layout`): what its sources handed over for it, else
/// its board profile (looked up when it was first reported), else a guess. Asks it nothing.
pub fn layout(keyboard: &Keyboard) -> KeyLayout {
    let board = BOARDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(k, _)| *k == keyboard.key)
        .and_then(|(_, b)| b.clone());
    super::key_layout::of_keyboard(keyboard.vid, keyboard.pid, board.as_ref())
}

#[cfg(test)]
#[path = "keyboard_focus_tests.rs"]
mod tests;
