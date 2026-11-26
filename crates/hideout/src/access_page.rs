// SPDX-License-Identifier: MIT
//! The **Access** page: the apps' permissions grouped by the thing they touch
//! (`hideout_shell::access_view`). A second view of the Apps page's Permissions sections, with
//! no permission logic of its own: it reads the same `Registry::permission_view`, builds the
//! same sections (`permission_actions::section_for`), turns an item off with the same call
//! (`revoke_items_in`, router first), and never grants. **Change…** leaves a request for the Apps
//! page (`review_ask`), which opens that app's hub-drawn dialog; Allow there is the user's click.
//!
//! The two pages tell each other of a change through two flags here (no timer): the Apps page
//! re-reads after a turn-off made here, and this page re-reads after anything changed there.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_shell::access_view::{self, AccessAsk, ThingGroup};
use hideout_shell::permission_dialog::Section;

/// Counts the Apps page's re-reads of the permissions (a grant, a turn-off, a module's start).
static PERMS_READ: AtomicU64 = AtomicU64::new(0);
/// This page turned something off since the Apps page last looked.
static TURNED_OFF: AtomicBool = AtomicBool::new(false);

/// The Apps page read the permissions again.
pub fn perms_read() {
    PERMS_READ.fetch_add(1, Ordering::Relaxed);
}

/// Whether this page turned something off since the last call. One use.
pub fn take_turned_off() -> bool {
    TURNED_OFF.swap(false, Ordering::Relaxed)
}

pub const NOT_SAVED: &str = "Turned off, but the change couldn't be saved.";

/// What a turn-off says: the note when it wasn't saved, else "Turned off", with the restart
/// when one was started for `restarting` (the app's name).
fn turned_off_said(note: Option<&str>, restarting: Option<&str>) -> String {
    match (note, restarting) {
        (Some(n), _) => n.to_string(),
        (None, Some(name)) => hideout_supervisor::words::turned_off_restarts(name),
        (None, None) => "Turned off".to_string(),
    }
}

pub struct AccessPage {
    home: PathBuf,
    hub: Hub,
    /// Every app with permissions, by id (their state facts are the event that re-reads).
    ids: Vec<String>,
    groups: Vec<ThingGroup>,
    seen: Vec<Option<serde_json::Value>>,
    stamp: u64,
    note: Option<String>,
    said: Vec<String>,
}

/// The sections of every app that has permissions, as the Apps page builds them.
fn sections(home: &std::path::Path) -> Vec<Section> {
    let Ok(reg) = Registry::open_from_env(home) else {
        return Vec::new();
    };
    reg.inventory
        .modules
        .keys()
        .filter_map(|id| {
            let view = reg.permission_view(id).ok()?;
            crate::permission_actions::section_for(id, &view)
        })
        .collect()
}

impl AccessPage {
    pub fn new(home: PathBuf, hub: Hub) -> Self {
        let mut p = Self {
            home,
            hub,
            ids: Vec::new(),
            groups: Vec::new(),
            seen: Vec::new(),
            stamp: PERMS_READ.load(Ordering::Relaxed),
            note: None,
            said: Vec::new(),
        };
        p.refresh();
        p
    }

    /// Reads the permissions again (after an action and on a change; never per frame).
    fn refresh(&mut self) {
        let s = sections(&self.home);
        self.ids = s.iter().map(|s| s.id.clone()).collect();
        self.groups = access_view::group(&s);
    }

    /// Re-reads when the Apps page did, or when an app's state fact changed (its start can void
    /// a grant).
    fn sync(&mut self) {
        let stamp = PERMS_READ.load(Ordering::Relaxed);
        let states: Vec<Option<serde_json::Value>> = self.hub.read(|r| {
            self.ids
                .iter()
                .map(|id| r.fact(&format!("module.{id}.state")).cloned())
                .collect()
        });
        if stamp != self.stamp || states != self.seen {
            self.stamp = stamp;
            self.seen = states;
            self.refresh();
        }
    }

    /// Turn off one allowed item: the router first, then the file. Never asks, never grants.
    fn turn_off(&mut self, app: &str, raw: &str) {
        let r = hideout_supervisor::driver::revoke_items_in(
            &self.home,
            &self.hub,
            app,
            Some(std::slice::from_ref(&raw.to_string())),
        );
        self.note = None;
        // The app restarts when the turn-off changes how it is contained (#1451): said with
        // the turn-off. How the restart ended is the Apps page's row to say.
        let restarts = r.as_ref().is_ok_and(|off| off.applies.restarts());
        let name = self
            .groups
            .iter()
            .flat_map(|g| &g.rows)
            .find(|row| row.app == app)
            .map(|row| row.name.clone());
        if let Err(e) = r {
            log::warn!("permissions: {app}: turn off not saved ({e})");
            self.note = Some(NOT_SAVED.to_string());
        }
        self.refresh();
        TURNED_OFF.store(true, Ordering::Relaxed);
        self.said.push(turned_off_said(
            self.note.as_deref(),
            name.as_deref().filter(|_| restarts),
        ));
    }
}

#[cfg(windows)]
impl hideout_shell::Page for AccessPage {
    fn id(&self) -> &str {
        access_view::ID
    }

    fn title(&self) -> &str {
        access_view::TITLE
    }

    fn icon(&self) -> &str {
        access_view::ICON
    }

    /// At the launcher's foot, above Settings.
    fn at_bottom(&self) -> bool {
        true
    }

    fn take_said(&mut self) -> Vec<String> {
        std::mem::take(&mut self.said)
    }

    fn ui(&mut self, ui: &mut hideout_shell::egui::Ui, t: &hideout_shell::theme::Tokens) {
        self.sync();
        let ask = hideout_shell::egui::ScrollArea::vertical()
            .id_salt("hub.access.scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                access_view::access_ui(ui, t, &self.groups, self.note.as_deref())
            })
            .inner;
        match ask {
            Some(AccessAsk::TurnOff { app, raw }) => self.turn_off(&app, &raw),
            Some(AccessAsk::Change { app }) => {
                // The dialog is the Apps page's: leave the request and show that page.
                crate::review_ask::push(&app, false);
                hideout_shell::open_module(crate::modules_page::ID);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1451: a turn-off that restarts the app says so, by the app's name.
    #[test]
    fn a_turn_off_says_when_the_app_restarts_to_apply() {
        assert_eq!(turned_off_said(None, None), "Turned off");
        assert_eq!(
            turned_off_said(None, Some("Art")),
            "Turned off. Art restarts to apply."
        );
        assert_eq!(turned_off_said(Some(NOT_SAVED), Some("Art")), NOT_SAVED);
    }

    /// One test: the flags are process-wide.
    #[test]
    fn a_turn_off_is_told_once() {
        take_turned_off();
        assert!(!take_turned_off());
        TURNED_OFF.store(true, Ordering::Relaxed);
        assert!(take_turned_off());
        assert!(!take_turned_off());
        let n = PERMS_READ.load(Ordering::Relaxed);
        perms_read();
        assert!(PERMS_READ.load(Ordering::Relaxed) > n);
    }
}
