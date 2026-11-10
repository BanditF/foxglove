// SPDX-License-Identifier: MIT
//! The tab strip's model (contract §5.9, `docs/hideout-tabs-ux.md`): which pages are open, in the
//! user's order, which one is active, which are popped out, and the last closed ones. The hub
//! owns all of it and saves it in its own folder (`tabs.toml`, §2.3); modules never see it.
//!
//! Pure: no egui, no panels. The window maps a [`TabId`] to a hub page or a module's page panel,
//! and draws the strip from [`Look`].

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// How many closed tabs Ctrl+Shift+T can bring back (this session only).
pub const REOPEN_DEPTH: usize = 10;

/// What a tab shows: one of the hub's own pages by its id, or a module's `page` panel.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TabId {
    Page { page: String },
    Panel { module: String, panel: String },
}

impl TabId {
    pub fn page(id: impl Into<String>) -> Self {
        TabId::Page { page: id.into() }
    }
    pub fn panel(module: impl Into<String>, panel: impl Into<String>) -> Self {
        TabId::Panel {
            module: module.into(),
            panel: panel.into(),
        }
    }
    pub fn module(&self) -> Option<&str> {
        match self {
            TabId::Panel { module, .. } => Some(module),
            TabId::Page { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    #[serde(flatten)]
    pub id: TabId,
    /// In its own window (§2.5): the tab stays in place, dimmed, as the way back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub popped: bool,
    /// The pop-out window is on screen (not minimised). Set by the window code (H2.11); a pop-out
    /// counts as `hidden` for `ui.visibility` until it says so.
    #[serde(skip)]
    pub popout_shown: bool,
    /// The title last seen, so a restored tab has a name before its module registers again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
}

/// What closing asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Close {
    /// Closed (or there was nothing to close).
    Done,
    /// These tabs hold unsaved work: ask "Close <Title>?" first, then [`Tabs::close_now`].
    Ask(Vec<TabId>),
}

/// The open tabs. Indices are strip positions, left to right.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: Option<usize>,
    /// Closed tabs with the place they had, newest last.
    closed: Vec<(TabId, usize)>,
    /// Something to save changed since [`Tabs::take_changed`].
    changed: bool,
}

/// The saved form, `tabs.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<usize>,
    #[serde(default)]
    tab: Vec<Tab>,
}

impl Tabs {
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }
    pub fn len(&self) -> usize {
        self.tabs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
    pub fn active(&self) -> Option<usize> {
        self.active
    }
    pub fn active_id(&self) -> Option<&TabId> {
        self.active.and_then(|i| self.tabs.get(i)).map(|t| &t.id)
    }
    pub fn position(&self, id: &TabId) -> Option<usize> {
        self.tabs.iter().position(|t| &t.id == id)
    }
    pub fn is_open(&self, id: &TabId) -> bool {
        self.position(id).is_some()
    }

    /// Whether order, the active tab or a pop-out changed since the last call (then save).
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Opens `id` to the right of the active tab, or switches to it if it is open already.
    pub fn open(&mut self, id: TabId) -> usize {
        if let Some(i) = self.position(&id) {
            self.activate(i);
            return i;
        }
        let at = self.active.map(|a| a + 1).unwrap_or(self.tabs.len());
        self.insert(id, at)
    }

    fn insert(&mut self, id: TabId, at: usize) -> usize {
        let at = at.min(self.tabs.len());
        self.closed.retain(|(c, _)| c != &id);
        self.tabs.insert(
            at,
            Tab {
                id,
                popped: false,
                popout_shown: false,
                title: String::new(),
            },
        );
        self.active = Some(at);
        self.changed = true;
        at
    }

    pub fn activate(&mut self, i: usize) {
        if i < self.tabs.len() && self.active != Some(i) {
            self.active = Some(i);
            self.changed = true;
        }
    }

    /// Ctrl+Tab (`+1`) and Ctrl+Shift+Tab (`-1`): the next or previous tab in strip order, wrapping.
    pub fn cycle(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        if n == 0 {
            return;
        }
        let from = self.active.unwrap_or(0) as isize;
        self.activate((from + step).rem_euclid(n) as usize);
    }

    /// Ctrl+1 to Ctrl+8 pick tab N; Ctrl+9 the last tab, however many there are.
    pub fn select_number(&mut self, n: u8) {
        if self.tabs.is_empty() || n == 0 {
            return;
        }
        let i = if n >= 9 {
            self.tabs.len() - 1
        } else {
            n as usize - 1
        };
        if i < self.tabs.len() {
            self.activate(i);
        }
    }

    /// The tab at `from` moves to `to` (both strip positions); the active tab stays active.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || from == to {
            return;
        }
        let to = to.min(self.tabs.len() - 1);
        let active = self.active_id().cloned();
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        self.active = active.and_then(|a| self.position(&a));
        self.changed = true;
    }

    /// Asks to close tab `i`. `dirty` says whether a tab holds unsaved work.
    pub fn request_close(&mut self, i: usize, dirty: impl Fn(&TabId) -> bool) -> Close {
        let Some(t) = self.tabs.get(i) else {
            return Close::Done;
        };
        if dirty(&t.id) {
            return Close::Ask(vec![t.id.clone()]);
        }
        let id = t.id.clone();
        self.close_now(&[id]);
        Close::Done
    }

    /// Close others (`keep` stays) or Close to the right (`keep` and everything left of it stay).
    /// Asks once if any of the tabs to close is unsaved.
    pub fn request_close_many(
        &mut self,
        keep: usize,
        right_only: bool,
        dirty: impl Fn(&TabId) -> bool,
    ) -> Close {
        let ids: Vec<TabId> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, _)| if right_only { *i > keep } else { *i != keep })
            .map(|(_, t)| t.id.clone())
            .collect();
        let unsaved: Vec<TabId> = ids.iter().filter(|id| dirty(id)).cloned().collect();
        if !unsaved.is_empty() {
            return Close::Ask(ids);
        }
        self.activate(keep);
        self.close_now(&ids);
        Close::Done
    }

    /// Closes `ids` without asking. Closing the active tab activates the one to its right, else
    /// the one to its left (VS Code).
    pub fn close_now(&mut self, ids: &[TabId]) {
        for id in ids {
            let Some(i) = self.position(id) else { continue };
            let t = self.tabs.remove(i);
            self.closed.push((t.id, i));
            if self.closed.len() > REOPEN_DEPTH {
                self.closed.remove(0);
            }
            self.active = match self.active {
                _ if self.tabs.is_empty() => None,
                Some(a) if a == i => Some(i.min(self.tabs.len() - 1)),
                Some(a) if a > i => Some(a - 1),
                other => other,
            };
            self.changed = true;
        }
    }

    /// Ctrl+Shift+T: the last closed tab at its old place. `exists` drops tabs whose page or
    /// module is gone.
    pub fn reopen_closed(&mut self, exists: impl Fn(&TabId) -> bool) -> Option<TabId> {
        while let Some((id, at)) = self.closed.pop() {
            if exists(&id) && !self.is_open(&id) {
                self.insert(id.clone(), at);
                return Some(id);
            }
        }
        None
    }

    /// A module was removed: its tabs close and can't be reopened. Returns how many closed.
    pub fn drop_module(&mut self, module: &str) -> usize {
        let ids: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.id.module() == Some(module))
            .map(|t| t.id.clone())
            .collect();
        self.close_now(&ids);
        self.closed.retain(|(id, _)| id.module() != Some(module));
        ids.len()
    }

    /// Remembers tab `i`'s title for the next start (saved with the next change).
    pub fn set_title(&mut self, i: usize, title: &str) {
        if let Some(t) = self.tabs.get_mut(i) {
            if t.title != title {
                t.title = title.to_string();
            }
        }
    }

    /// The page moved to its own window: the tab stays, dimmed.
    pub fn pop_out(&mut self, i: usize) {
        if let Some(t) = self.tabs.get_mut(i) {
            if !t.popped {
                t.popped = true;
                t.popout_shown = false;
                self.changed = true;
            }
        }
    }

    /// Dock back: the page returns to its tab, in its old place, and becomes active.
    pub fn dock_back(&mut self, i: usize) {
        if let Some(t) = self.tabs.get_mut(i) {
            if t.popped {
                t.popped = false;
                t.popout_shown = false;
                self.changed = true;
            }
        }
        self.activate(i);
    }

    /// The pop-out window of tab `i` was shown or minimised (the window code, H2.11).
    pub fn set_popout_shown(&mut self, i: usize, shown: bool) {
        if let Some(t) = self.tabs.get_mut(i).filter(|t| t.popped) {
            t.popout_shown = shown;
        }
    }

    /// The tabs on screen for `ui.visibility`: the active tab when the hub window is shown (and
    /// the tab isn't popped out), plus every pop-out window that is shown.
    pub fn on_screen(&self, window_shown: bool) -> Vec<&TabId> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                if t.popped {
                    t.popout_shown
                } else {
                    window_shown && self.active == Some(*i)
                }
            })
            .map(|(_, t)| &t.id)
            .collect()
    }

    /// `tabs.toml` text.
    pub fn to_toml(&self) -> String {
        let saved = Saved {
            active: self.active,
            tab: self.tabs.clone(),
        };
        toml::to_string(&saved).unwrap_or_default()
    }

    /// Restores a saved strip. `exists` drops tabs whose page or module is gone (silently, as
    /// on a restart). A file that doesn't parse gives `Err` and an empty strip.
    pub fn parse(text: &str, exists: impl Fn(&TabId) -> bool) -> Result<Self, String> {
        let saved: Saved = toml::from_str(text).map_err(|e| e.to_string())?;
        let active_id = saved
            .active
            .and_then(|i| saved.tab.get(i))
            .map(|t| t.id.clone());
        let mut tabs: Vec<Tab> = Vec::new();
        for t in saved.tab {
            if exists(&t.id) && !tabs.iter().any(|o| o.id == t.id) {
                tabs.push(t);
            }
        }
        let active = active_id
            .and_then(|a| tabs.iter().position(|t| t.id == a))
            .or(if tabs.is_empty() { None } else { Some(0) });
        Ok(Self {
            tabs,
            active,
            closed: Vec::new(),
            changed: false,
        })
    }

    /// Reads `path`. Missing (first start) gives `Ok(None)`; unreadable or invalid gives `Err`.
    pub fn load(path: &Path, exists: impl Fn(&TabId) -> bool) -> Result<Option<Self>, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, exists)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes atomically: a temp file next to `path`, then a rename over it.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            hideout_acl::create_private_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, path)
    }
}

/// A tab's state as the strip shows it, from the module's state (§7.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabState {
    Live,
    /// Starting, updating or restarting: a ring round the mark. The words are the §7.8 ones.
    Busy(String),
    /// Crash-stopped, install failed, needs a newer hub, quarantined, couldn't be shown.
    Stopped(String),
    /// The module is turned off.
    Off,
}

/// What the module set with `ui.badge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mark {
    #[default]
    None,
    Attention,
    Dirty,
    Count(u32),
}

/// Everything the strip needs to draw one tab, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub title: String,
    pub state: TabState,
    pub mark: Mark,
    pub popped: bool,
}

/// The one badge a tab shows when several apply: stopped mark > unsaved dot > count > attention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    None,
    Stopped,
    Dirty,
    Count(String),
    Attention,
}

impl Look {
    pub fn shown(&self) -> Shown {
        if matches!(self.state, TabState::Stopped(_)) {
            return Shown::Stopped;
        }
        match self.mark {
            Mark::Dirty => Shown::Dirty,
            Mark::Count(n) => Shown::Count(count_text(n)),
            Mark::Attention => Shown::Attention,
            Mark::None => Shown::None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.state, TabState::Busy(_))
    }

    /// Popped out or off: the label is muted even on the active tab.
    pub fn muted(&self) -> bool {
        self.popped || self.state == TabState::Off
    }

    /// The state line after the title ("3 new", "Not saved", ...), or `None`.
    pub fn state_line(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        match &self.state {
            TabState::Live => {}
            // "Restarting… (2 of 5)" already has its ellipsis.
            TabState::Busy(w) if w.contains('\u{2026}') => parts.push(w.clone()),
            TabState::Busy(w) => parts.push(format!("{w}\u{2026}")),
            TabState::Stopped(w) => parts.push(w.clone()),
            TabState::Off => parts.push("Off".into()),
        }
        match self.mark {
            Mark::Dirty => parts.push("Not saved".into()),
            Mark::Count(n) => parts.push(format!("{} new", count_text(n))),
            Mark::Attention => parts.push("Something new".into()),
            Mark::None => {}
        }
        if self.popped {
            parts.push("In its own window".into());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }

    /// The tooltip: the full title, then the state line.
    pub fn tooltip(&self) -> String {
        match self.state_line() {
            Some(s) => format!("{}\n{s}", self.title),
            None => self.title.clone(),
        }
    }

    /// The screen reader name: "Chat, 3 new", "Scenes, not saved", "Images, in its own window".
    pub fn accessible_name(&self) -> String {
        match self.state_line() {
            Some(s) => {
                let mut s = s;
                if let Some(first) = s.get(0..1) {
                    let lower = first.to_lowercase();
                    s.replace_range(0..1, &lower);
                }
                format!("{}, {s}", self.title)
            }
            None => self.title.clone(),
        }
    }
}

/// "99+" above 99.
pub fn count_text(n: u32) -> String {
    if n > 99 {
        "99+".into()
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str) -> TabId {
        TabId::page(id)
    }
    fn m(panel: &str) -> TabId {
        TabId::panel("mod.example", panel)
    }
    fn ids(t: &Tabs) -> Vec<String> {
        t.tabs()
            .iter()
            .map(|t| match &t.id {
                TabId::Page { page } => page.clone(),
                TabId::Panel { panel, .. } => panel.clone(),
            })
            .collect()
    }
    fn strip() -> Tabs {
        let mut t = Tabs::default();
        for id in ["a", "b", "c", "d"] {
            t.open(p(id));
        }
        t
    }

    #[test]
    fn open_goes_right_of_the_active_tab_and_switches_when_open() {
        let mut t = strip();
        assert_eq!(ids(&t), ["a", "b", "c", "d"]);
        t.activate(1);
        t.open(m("chat"));
        assert_eq!(ids(&t), ["a", "b", "chat", "c", "d"]);
        assert_eq!(t.active(), Some(2));
        assert_eq!(t.open(p("a")), 0);
        assert_eq!(t.len(), 5, "at most one tab per page");
        assert_eq!(t.active(), Some(0));
    }

    #[test]
    fn closing_the_active_tab_activates_the_right_one_else_the_left() {
        let mut t = strip();
        t.activate(1);
        assert_eq!(t.request_close(1, |_| false), Close::Done);
        assert_eq!(ids(&t), ["a", "c", "d"]);
        assert_eq!(t.active_id(), Some(&p("c")));
        t.activate(2);
        t.request_close(2, |_| false);
        assert_eq!(t.active_id(), Some(&p("c")));
        // Closing a tab left of the active one keeps the same tab active.
        t.request_close(0, |_| false);
        assert_eq!(t.active_id(), Some(&p("c")));
        t.request_close(0, |_| false);
        assert!(t.is_empty() && t.active().is_none());
    }

    #[test]
    fn a_dirty_tab_asks_before_closing() {
        let mut t = strip();
        let dirty = |id: &TabId| id == &p("b");
        assert_eq!(t.request_close(1, dirty), Close::Ask(vec![p("b")]));
        assert_eq!(t.len(), 4, "nothing closed until the user says so");
        assert_eq!(
            t.request_close_many(0, false, dirty),
            Close::Ask(vec![p("b"), p("c"), p("d")])
        );
        assert_eq!(t.request_close_many(1, true, dirty), Close::Done);
        assert_eq!(ids(&t), ["a", "b"]);
        t.close_now(&[p("b")]);
        assert_eq!(ids(&t), ["a"]);
    }

    #[test]
    fn reopen_brings_the_last_closed_back_at_its_place() {
        let mut t = strip();
        t.request_close(1, |_| false);
        t.request_close(2, |_| false);
        assert_eq!(ids(&t), ["a", "c"]);
        assert_eq!(t.reopen_closed(|_| true), Some(p("d")));
        assert_eq!(t.reopen_closed(|id| id != &p("x")), Some(p("b")));
        assert_eq!(ids(&t), ["a", "b", "c", "d"]);
        assert_eq!(t.active_id(), Some(&p("b")));
        assert_eq!(t.reopen_closed(|_| true), None);
        for i in 0..(REOPEN_DEPTH + 3) {
            t.open(p(&format!("x{i}")));
            t.close_now(&[p(&format!("x{i}"))]);
        }
        let mut n = 0;
        while t.reopen_closed(|_| true).is_some() {
            n += 1;
        }
        assert_eq!(n, REOPEN_DEPTH);
    }

    #[test]
    fn a_removed_module_closes_its_tabs_for_good() {
        let mut t = strip();
        t.open(m("chat"));
        t.open(m("art"));
        t.close_now(&[m("art")]);
        assert_eq!(t.drop_module("mod.example"), 1);
        assert_eq!(ids(&t), ["a", "b", "c", "d"]);
        assert_eq!(t.reopen_closed(|_| true), None);
    }

    #[test]
    fn reorder_keeps_the_active_tab() {
        let mut t = strip();
        t.activate(1);
        t.take_changed();
        t.move_tab(0, 3);
        assert_eq!(ids(&t), ["b", "c", "d", "a"]);
        assert_eq!(t.active_id(), Some(&p("b")));
        assert!(t.take_changed());
        t.move_tab(3, 99);
        assert_eq!(ids(&t), ["b", "c", "d", "a"]);
        t.move_tab(2, 0);
        assert_eq!(ids(&t), ["d", "b", "c", "a"]);
        assert_eq!(t.active(), Some(1));
    }

    #[test]
    fn keyboard_switching_follows_strip_order_and_wraps() {
        let mut t = strip();
        t.activate(3);
        t.cycle(1);
        assert_eq!(t.active(), Some(0));
        t.cycle(-1);
        assert_eq!(t.active(), Some(3));
        t.cycle(-1);
        assert_eq!(t.active(), Some(2));
        t.select_number(1);
        assert_eq!(t.active(), Some(0));
        t.select_number(4);
        assert_eq!(t.active(), Some(3));
        t.select_number(7);
        assert_eq!(t.active(), Some(3), "no tab 7: nothing changes");
        t.select_number(9);
        assert_eq!(t.active(), Some(3), "Ctrl+9 is the last tab");
        t.open(p("e"));
        t.select_number(2);
        t.select_number(9);
        assert_eq!(t.active_id(), Some(&p("e")));
        Tabs::default().cycle(1);
    }

    #[test]
    fn order_active_and_popouts_survive_a_restart() {
        let mut t = strip();
        t.open(m("chat"));
        t.move_tab(4, 0);
        t.pop_out(2);
        t.activate(3);
        let dir = std::env::temp_dir().join(format!("hideout-tabs-{}", std::process::id()));
        let f = dir.join("tabs.toml");
        assert_eq!(Tabs::load(&f, |_| true), Ok(None), "first start");
        t.save(&f).unwrap();
        let back = Tabs::load(&f, |_| true).unwrap().unwrap();
        assert_eq!(ids(&back), ids(&t));
        assert_eq!(back.active(), Some(3));
        assert!(back.tabs()[2].popped && !back.tabs()[1].popped);
        assert_eq!(back.tabs()[0].id, m("chat"));
        // A removed page is dropped silently; the active one keeps its tab.
        let gone = Tabs::load(&f, |id| id != &p("a")).unwrap().unwrap();
        assert_eq!(ids(&gone), ["chat", "b", "c", "d"]);
        assert_eq!(gone.active_id(), Some(&p("c")));
        std::fs::write(&f, "tab = 3").unwrap();
        assert!(Tabs::load(&f, |_| true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn popped_tabs_stay_in_place_and_count_on_screen_only_when_shown() {
        let mut t = strip();
        t.activate(1);
        t.pop_out(1);
        assert_eq!(ids(&t), ["a", "b", "c", "d"], "order doesn't shift");
        assert!(t.on_screen(true).is_empty(), "no window yet: hidden");
        t.set_popout_shown(1, true);
        assert_eq!(t.on_screen(false), [&p("b")]);
        t.activate(2);
        assert_eq!(t.on_screen(true), [&p("b"), &p("c")]);
        t.dock_back(1);
        assert!(!t.tabs()[1].popped);
        assert_eq!(t.on_screen(true), [&p("b")]);
        assert!(t.on_screen(false).is_empty(), "hub in the tray");
    }

    #[test]
    fn badges_follow_the_precedence_and_read_in_words() {
        let mut l = Look {
            title: "Chat".into(),
            state: TabState::Live,
            mark: Mark::Count(3),
            popped: false,
        };
        assert_eq!(l.shown(), Shown::Count("3".into()));
        assert_eq!(l.accessible_name(), "Chat, 3 new");
        l.mark = Mark::Count(140);
        assert_eq!(l.shown(), Shown::Count("99+".into()));
        l.mark = Mark::Dirty;
        assert_eq!(l.shown(), Shown::Dirty);
        assert_eq!(l.accessible_name(), "Chat, not saved");
        l.state = TabState::Stopped("Stopped after 5 crashes".into());
        assert_eq!(l.shown(), Shown::Stopped);
        assert_eq!(l.tooltip(), "Chat\nStopped after 5 crashes, Not saved");
        l.state = TabState::Busy("Updating".into());
        l.mark = Mark::Attention;
        assert!(l.busy());
        assert_eq!(l.shown(), Shown::Attention);
        assert_eq!(l.accessible_name(), "Chat, updating\u{2026}, Something new");
        l.state = TabState::Live;
        l.mark = Mark::None;
        l.popped = true;
        assert!(l.muted());
        assert_eq!(l.accessible_name(), "Chat, in its own window");
    }
}
