// SPDX-License-Identifier: MIT
//! What the module permission dialog, its header lines and the Permissions section do (#1178,
//! docs/hideout-permission-dialog-ux.md). The words and rules are `hideout_shell::permission_dialog`
//! and the drawing `permission_dialog_ui`; this is the hub's side, shaped like `keyboard_actions`:
//! it reads `Registry::permission_view`, and the only thing that ever grants is the user's Allow
//! (`grant_listed_in`, with the list code the dialog showed). Turning off narrows
//! (`revoke_items_in`, router first). Nothing here runs from a background event: the dialog opens
//! only from a click of the user's on Turn on, Review… or Change….

use std::path::Path;

use hideout_registry::permission::{
    Group as RGroup, ItemState, View, REASON_NEVER, REASON_OVERLAP,
};
use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_shell::egui;
use hideout_shell::permission_dialog::{
    header, turned_off_note, Allowed, Group, HeaderKind, ItemWord, KeyboardChoice, ListItem,
    PermAnswer, PermDialog, PermHeader, Reason, Section, SectionItem, BUSY, NOT_SAVED,
};
use hideout_shell::permission_dialog_ui::{perm_dialog_ui, perm_header_ui, section_ui, SectionAsk};
use hideout_shell::theme::Tokens;

/// What the user pressed to open the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opener {
    /// Turn on… of the module: Allow then turns it on.
    TurnOn,
    /// Review… on "Update waiting for you": Allow then lets the swap run.
    Review,
    /// Review… on a header line: Allow then turns the module on.
    Header,
    /// Change… in the Permissions section: Allow only records.
    Change,
}

/// What follows a successful Allow, done by the page (it holds the supervisor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    TurnOn,
    Review,
    Nothing,
}

impl Opener {
    fn then(self) -> Then {
        match self {
            Opener::TurnOn | Opener::Header => Then::TurnOn,
            Opener::Review => Then::Review,
            Opener::Change => Then::Nothing,
        }
    }
}

/// The allow that was recorded and what to do after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub id: String,
    pub code: String,
    pub chosen: Vec<String>,
    pub declined: Vec<String>,
    pub then: Then,
}

fn group(g: RGroup) -> Group {
    match g {
        RGroup::Sensitive => Group::Sensitive,
        RGroup::MakeChanges => Group::MakeChanges,
        RGroup::AnswerQuestions => Group::AnswerQuestions,
        RGroup::ShareStatus => Group::ShareStatus,
        RGroup::CantBeAllowed => Group::CantBeAllowed,
    }
}

/// "2026-11-13T12:00:00Z" as "13 Nov 2026" (the input as it is when it doesn't parse).
pub fn date_words(at: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let d = at.get(..10).unwrap_or("");
    let p: Vec<&str> = d.split('-').collect();
    match p.as_slice() {
        [y, m, day] => match (m.parse::<usize>(), day.parse::<u32>()) {
            (Ok(m @ 1..=12), Ok(day)) => format!("{day} {} {y}", MONTHS[m - 1]),
            _ => at.into(),
        },
        _ => at.into(),
    }
}

/// The dialog for `id` as the registry lists it now. Normal-risk items start checked, sensitive
/// ones and items the user turned off start unchecked; after a void the last choice is the
/// default (still needing Allow).
pub fn build_dialog(home: &Path, id: &str, reason: Option<Reason>) -> Result<PermDialog, String> {
    let reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    let v = reg.permission_view(id).map_err(|e| format!("{e:?}"))?;
    let after_void = v.void.is_some() || !v.last_choice.is_empty();
    let items: Vec<ListItem> = v
        .listing
        .items
        .iter()
        .map(|i| {
            let allowable = i.state == ItemState::Asked;
            let checked = allowable
                && if after_void {
                    v.last_choice.contains(&i.raw)
                } else {
                    !i.sensitive && !v.declined.contains(&i.raw)
                };
            ListItem {
                raw: i.raw.clone(),
                line: i.line.clone(),
                group: if allowable {
                    group(i.group)
                } else {
                    Group::CantBeAllowed
                },
                allowable,
                sensitive: i.sensitive,
                reason: match i.state {
                    ItemState::Asked => None,
                    ItemState::Never => Some(REASON_NEVER.to_string()),
                    ItemState::Overlap => Some(REASON_OVERLAP.to_string()),
                },
                checked,
                confirmed: false,
            }
        })
        .collect();
    let keyboard = (id == crate::keyboard_actions::MODULE
        && matches!(
            reg.keyboard_state(id),
            hideout_registry::keyboard::KeyboardState::NotAsked
                | hideout_registry::keyboard::KeyboardState::Changed { .. }
        ))
    .then_some(KeyboardChoice {
        on: false,
        expanded: false,
    });
    let reason = reason.or(v.void.map(|_| Reason::Void));
    let mut items = items;
    items.sort_by_key(|i| (i.group, i.raw.clone()));
    Ok(PermDialog {
        id: id.into(),
        name: v.listing.name.clone(),
        code: v.listing.code().to_string(),
        path: v.listing.exe_path.clone(),
        items,
        reason,
        keyboard,
        error: None,
    })
}

/// The Permissions section of one module, or `None` when it has nothing to say.
pub fn section_for(id: &str, v: &View) -> Option<Section> {
    let name = v.listing.name.clone();
    let mut items: Vec<SectionItem> = v
        .granted
        .iter()
        .map(|i| SectionItem {
            raw: i.raw.clone(),
            line: i.line.clone(),
            group: group(i.group),
            word: ItemWord::Allowed,
        })
        .collect();
    let mut can_change = v.void.is_some();
    for i in &v.listing.items {
        let allowable = i.state == ItemState::Asked;
        can_change |= allowable;
        items.push(SectionItem {
            raw: i.raw.clone(),
            line: i.line.clone(),
            group: if allowable {
                group(i.group)
            } else {
                Group::CantBeAllowed
            },
            word: if allowable {
                ItemWord::Off
            } else {
                ItemWord::CantBeAllowed
            },
        });
    }
    if items.is_empty() {
        return None;
    }
    items.sort_by_key(|i| (i.group, i.raw.clone()));
    let allowed_on = v.bound.as_ref().filter(|b| !b.void).map(|b| {
        let when = date_words(&b.at);
        if b.by == "cli" {
            format!("Allowed from the command line on {when}")
        } else {
            format!("Allowed in Hideout on {when}")
        }
    });
    let history = v
        .bound
        .as_ref()
        .map(|b| {
            b.attested
                .iter()
                .map(|a| {
                    format!(
                        "Updated to {} on {}. Your OK stayed.",
                        a.version,
                        date_words(&a.at)
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Section {
        id: id.into(),
        name,
        items,
        allowed_on,
        history,
        can_change,
        note: None,
    })
}

/// Which header line a module gets, if any. `enabled`: the module is on in the hub's settings.
pub fn header_kind(v: &View, enabled: bool, cli_seen: bool) -> Option<HeaderKind> {
    if v.void.is_some() {
        Some(HeaderKind::Changed)
    } else if v.bound.is_none() && v.adopted && enabled && !v.pending.is_empty() {
        Some(HeaderKind::Ask)
    } else if v.bound.as_ref().is_some_and(|b| b.by == "cli") && !cli_seen {
        Some(HeaderKind::Cli)
    } else {
        None
    }
}

/// Whether Turn on must go through the dialog first.
pub fn needs_ok(home: &Path, id: &str) -> bool {
    Registry::open_from_env(home)
        .ok()
        .and_then(|r| r.permission_view(id).ok())
        .is_some_and(|v| !v.pending.is_empty() || v.void.is_some())
}

/// What the page keeps for one module with something to show.
struct Entry {
    id: String,
    view: View,
    enabled: bool,
    note: Option<String>,
}

/// The dialog, header lines and Permissions sections of the Modules page.
pub struct PermUi {
    home: std::path::PathBuf,
    hub: Hub,
    entries: Vec<Entry>,
    pub dialog: Option<PermDialog>,
    opener: Option<Opener>,
    opener_id: Option<egui::Id>,
    drawn: bool,
    /// Header lines the user answered with Not now / OK, until the next hub start.
    hidden: Vec<(String, HeaderKind)>,
    seen: Vec<Option<serde_json::Value>>,
}

impl PermUi {
    pub fn new(home: &Path, hub: Hub) -> PermUi {
        let mut p = PermUi {
            home: home.to_path_buf(),
            hub,
            entries: Vec::new(),
            dialog: None,
            opener: None,
            opener_id: None,
            drawn: false,
            hidden: Vec::new(),
            seen: Vec::new(),
        };
        p.refresh();
        p
    }

    /// Reads every module's permissions from the registry again (on a change of the inventory
    /// and after each action; never per frame).
    pub fn refresh(&mut self) {
        let Ok(reg) = Registry::open_from_env(&self.home) else {
            self.entries.clear();
            return;
        };
        let notes: Vec<(String, Option<String>)> = self
            .entries
            .iter()
            .map(|e| (e.id.clone(), e.note.clone()))
            .collect();
        self.entries = reg
            .inventory
            .modules
            .keys()
            .filter_map(|id| {
                let view = reg.permission_view(id).ok()?;
                let any = !view.granted.is_empty()
                    || !view.listing.items.is_empty()
                    || view.void.is_some();
                any.then(|| Entry {
                    id: id.clone(),
                    enabled: reg.hub.enabled(id),
                    note: notes
                        .iter()
                        .find(|(n, _)| n == id)
                        .and_then(|(_, n)| n.clone()),
                    view,
                })
            })
            .collect();
    }

    /// Whether the module still waits for the user's OK (pending items or a void binding).
    pub fn needs_ok(&self, id: &str) -> bool {
        needs_ok(&self.home, id)
    }

    /// Re-reads when a module's state fact changed (no timer).
    pub fn sync(&mut self, states: Vec<Option<serde_json::Value>>) {
        if self.seen != states {
            self.seen = states;
            self.refresh();
        }
    }

    /// Opens the dialog for `id` (only from the user's click).
    pub fn open(&mut self, id: &str, how: Opener) {
        self.open_with(id, how, None);
    }

    fn open_with(&mut self, id: &str, how: Opener, reason: Option<Reason>) {
        match build_dialog(&self.home, id, reason) {
            Ok(d) => {
                let asked = d.items.iter().filter(|i| i.allowable).count();
                log::info!(
                    "permissions: {id}: dialog shown, list {}, {asked} asked, {} can't be allowed",
                    d.code,
                    d.items.len() - asked
                );
                self.dialog = Some(d);
                self.opener = Some(how);
                self.drawn = false;
            }
            Err(e) => {
                log::warn!("permissions: {id}: the dialog can't open ({e})");
                self.note(id, format!("{id}'s permissions couldn't be read."));
            }
        }
    }

    fn note(&mut self, id: &str, text: String) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.note = Some(text);
        }
    }

    /// The user's answer. Not now writes nothing. Allow records the grant for the list the
    /// dialog showed; on success the dialog closes and the page does what `then` says; a list
    /// that changed reopens the dialog with the new list and the reason on top, nothing allowed.
    pub fn answer(&mut self, a: PermAnswer) -> Option<Done> {
        let d = self.dialog.take()?;
        let how = self.opener.take().unwrap_or(Opener::Change);
        let PermAnswer::Allow(Allowed {
            code,
            chosen,
            declined,
            keyboard,
        }) = a
        else {
            return None;
        };
        let by = "dialog";
        let r = hideout_supervisor::driver::grant_listed_in(
            &self.home, &self.hub, &d.id, &code, &chosen, &declined, by,
        );
        match r {
            Ok(_) => {
                if d.keyboard.is_some() {
                    self.keyboard(&d.id, keyboard);
                }
                self.refresh();
                Some(Done {
                    id: d.id,
                    code,
                    chosen,
                    declined,
                    then: how.then(),
                })
            }
            Err(e) if e.contains("list_changed") => {
                log::warn!(
                    "permissions: {}: not allowed: list changed (shown {code})",
                    d.id
                );
                self.open_with(&d.id, how, Some(Reason::ListChanged));
                None
            }
            Err(e) => {
                log::warn!("permissions: {}: allow failed ({e})", d.id);
                let mut d = d;
                d.error = Some(
                    if e.contains("lock") || e.contains("busy") {
                        BUSY
                    } else {
                        NOT_SAVED
                    }
                    .into(),
                );
                self.dialog = Some(d);
                self.opener = Some(how);
                None
            }
        }
    }

    /// The keyboard section: ticked is the #1155-B Allow for the same program; left off is its
    /// Not now, so the keyboard dialog does not follow.
    fn keyboard(&self, id: &str, on: bool) {
        let r = if on {
            crate::keyboard_actions::dialog_path(&self.home, id)
                .and_then(|p| crate::keyboard_actions::allow(&self.home, &self.hub, id, &p))
        } else {
            crate::keyboard_actions::decline(&self.home, id)
        };
        if let Err(e) = r {
            log::warn!("permissions: {id}: keyboard choice not saved ({e})");
        }
    }

    /// Turn off one allowed item: the router first, then the file. Never asks.
    pub fn turn_off(&mut self, id: &str, raw: &str) {
        self.narrow(id, Some(std::slice::from_ref(&raw.to_string())), Some(raw));
    }

    pub fn turn_all_off(&mut self, id: &str) {
        self.narrow(id, None, None);
    }

    fn narrow(&mut self, id: &str, items: Option<&[String]>, one: Option<&str>) {
        let r = hideout_supervisor::driver::revoke_items_in(&self.home, &self.hub, id, items);
        let name = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .map_or_else(|| id.to_string(), |e| e.view.listing.name.clone());
        self.refresh();
        let note = match r {
            // The router already closed it: the section says Off whatever the file says.
            Err(e) => {
                log::warn!("permissions: {id}: turn off not saved ({e})");
                Some("Turned off, but the change couldn't be saved.".to_string())
            }
            Ok(_) => one.and_then(|raw| turned_off_note(&name, raw)),
        };
        if let Some(n) = note {
            self.note(id, n);
        }
    }

    /// The header lines (at most one per module) and the Permissions sections, at the top of the
    /// Modules page.
    pub fn top(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let mut open: Option<(String, Opener, egui::Id)> = None;
        let mut hide: Option<(String, HeaderKind)> = None;
        let mut off: Option<(String, Option<String>)> = None;
        for e in &self.entries {
            let kind = header_kind(&e.view, e.enabled, false)
                .filter(|k| !self.hidden.iter().any(|(i, h)| i == &e.id && h == k));
            if let Some(k) = kind {
                let name = &e.view.listing.name;
                let cli = e
                    .view
                    .bound
                    .as_ref()
                    .map(|b| (e.view.granted.len(), date_words(&b.at)));
                let h: PermHeader =
                    header(k, &e.id, name, cli.as_ref().map(|(n, w)| (*n, w.as_str())));
                match perm_header_ui(ui, t, &h) {
                    Some((true, id)) => open = Some((e.id.clone(), Opener::Header, id)),
                    Some((false, _)) => hide = Some((e.id.clone(), k)),
                    None => {}
                }
                ui.add_space(8.0);
            }
        }
        for e in &self.entries {
            let Some(mut s) = section_for(&e.id, &e.view) else {
                continue;
            };
            s.note = e.note.clone();
            match section_ui(ui, t, &s, false) {
                Some((SectionAsk::TurnOff(raw), _)) => off = Some((e.id.clone(), Some(raw))),
                Some((SectionAsk::TurnAllOff, _)) => off = Some((e.id.clone(), None)),
                Some((SectionAsk::Change, id)) => open = Some((e.id.clone(), Opener::Change, id)),
                None => {}
            }
            ui.add_space(12.0);
        }
        if let Some((id, how, btn)) = open {
            self.opener_id = Some(btn);
            self.open(&id, how);
        }
        if let Some(h) = hide {
            self.hidden.push(h);
        }
        if let Some((id, raw)) = off {
            match raw {
                Some(r) => self.turn_off(&id, &r),
                None => self.turn_all_off(&id),
            }
        }
    }

    /// Draws the dialog while it is open. An Allow that was recorded comes back so the page can
    /// turn the module on; the focus goes back to the button that opened it.
    pub fn dialog(&mut self, ctx: &egui::Context, t: &Tokens) -> Option<Done> {
        let first = !self.drawn;
        self.drawn = true;
        let a = perm_dialog_ui(ctx, t, self.dialog.as_mut()?, first)?;
        let done = self.answer(a);
        if self.dialog.is_none() {
            if let Some(id) = self.opener_id.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        done
    }
}
