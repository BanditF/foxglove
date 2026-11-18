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
    file_of, header, keep_allow_first, keep_changed_open, keep_history, keep_off_note,
    keep_void_line, Allowed, Group, HeaderKind, ItemWord, Keep, KeepAnswer, KeepConfirm, KeepRow,
    KeepState, KeyboardChoice, ListItem, PermAnswer, PermDialog, PermHeader, Reason, Section,
    SectionItem, BUSY, KEEP_NOT_SAVED, KEEP_REFUSED, NOT_SAVED,
};
use hideout_shell::permission_dialog_ui::{
    keep_confirm_ui, perm_dialog_ui, perm_header_ui, section_ui, SectionAsk,
};
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
    /// Review… on "asks for more": the module already runs, Allow only adds (#1238).
    More,
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
            Opener::Change | Opener::More => Then::Nothing,
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A UTC time as the PC's local time: (year, month, day, hour, minute).
#[cfg(windows)]
fn to_local(t: (u16, u16, u16, u16, u16)) -> (u16, u16, u16, u16, u16) {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
    let utc = SYSTEMTIME {
        wYear: t.0,
        wMonth: t.1,
        wDayOfWeek: 0,
        wDay: t.2,
        wHour: t.3,
        wMinute: t.4,
        wSecond: 0,
        wMilliseconds: 0,
    };
    let mut local = utc;
    // SAFETY: both pointers are to live SYSTEMTIMEs; a null zone means the current one.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    if ok == 0 {
        t
    } else {
        (
            local.wYear,
            local.wMonth,
            local.wDay,
            local.wHour,
            local.wMinute,
        )
    }
}

#[cfg(not(windows))]
fn to_local(t: (u16, u16, u16, u16, u16)) -> (u16, u16, u16, u16, u16) {
    t
}

/// "2026-11-13T12:00:00Z" as local time in words, "13 Nov 2026, 20:40" (the date alone when the
/// time doesn't parse).
pub fn time_words(at: &str) -> String {
    let parsed = || -> Option<(u16, u16, u16, u16, u16)> {
        let mut d = at.get(..10)?.split('-');
        let mut t = at.get(11..16)?.split(':');
        Some((
            d.next()?.parse().ok()?,
            d.next()?.parse().ok()?,
            d.next()?.parse().ok()?,
            t.next()?.parse().ok()?,
            t.next()?.parse().ok()?,
        ))
    };
    match parsed() {
        Some(p) if (1..=12).contains(&p.1) => {
            let (y, mo, d, h, mi) = to_local(p);
            format!(
                "{d} {} {y}, {h:02}:{mi:02}",
                MONTHS[usize::from(mo.clamp(1, 12)) - 1]
            )
        }
        _ => date_words(at),
    }
}

/// The folder of the app's file (`C:\Apps\Fox\foxglove.exe` gives `C:\Apps\Fox`).
fn folder_of(path: &str) -> String {
    path.rsplit_once(['\\', '/'])
        .map_or_else(String::new, |(f, _)| f.to_string())
}

/// "2026-11-13T12:00:00Z" as "13 Nov 2026" (the input as it is when it doesn't parse).
pub fn date_words(at: &str) -> String {
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
                && if after_void && v.last_choice.contains(&i.raw) {
                    true
                } else if after_void && v.declined.contains(&i.raw) {
                    false
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
                new: false,
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
    let keep = keep_choice(&v);
    // A kept grant whose update asked for more: only what was never answered is New.
    let more = reason.is_none() && v.void.is_none() && !asked_pending(&v).is_empty();
    let reason = reason
        .or(v.void.map(|_| Reason::Void))
        .or(more.then_some(Reason::More));
    let mut items = items;
    if reason == Some(Reason::More) {
        for i in items.iter_mut() {
            i.new = i.allowable && v.pending.contains(&i.raw);
        }
    }
    items.sort_by_key(|i| (i.group, i.raw.clone()));
    Ok(PermDialog {
        id: id.into(),
        name: v.listing.name.clone(),
        code: v.listing.code().to_string(),
        path: v.listing.exe_path.clone(),
        items,
        reason,
        keyboard,
        keep,
        removed: Vec::new(),
        error: None,
    })
}

/// Whether the choice is live now: set, and the binding still holds.
fn keep_is_on(v: &View) -> bool {
    v.bound
        .as_ref()
        .is_some_and(|b| b.follow.is_some() && !b.void)
        && v.void.is_none()
}

/// The dialog's "Keep my OK after updates in this folder": only for an adopted app with a path;
/// refused for a folder that is not on this PC's own drive; checked when already on.
fn keep_choice(v: &View) -> Option<Keep> {
    if !v.adopted || v.listing.exe_path.is_empty() {
        return None;
    }
    Some(
        if hideout_registry::permission::on_local_fixed_drive(&v.listing.exe_path) {
            Keep::offered(keep_is_on(v))
        } else {
            Keep::refused()
        },
    )
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
            word: if allowable && v.pending.contains(&i.raw) {
                ItemWord::Waits
            } else if allowable {
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
    let note = hideout_shell::permission_dialog::status_note(&name, &items);
    let waiting = asked_pending(v).len();
    // Newest first, so the page can fold the older ones under "and N earlier".
    let history = v
        .bound
        .as_ref()
        .map(|b| {
            let last = b.attested.len().saturating_sub(1);
            let mut lines: Vec<String> = b
                .attested
                .iter()
                .enumerate()
                .map(|(n, a)| {
                    let grew = if n == last { waiting } else { 0 };
                    if a.by == hideout_registry::permission::FOLLOW_ATTESTED_BY {
                        return keep_history(&time_words(&a.at), grew);
                    }
                    let mut line = format!(
                        "Updated to {} on {}. Your OK stayed",
                        a.version,
                        date_words(&a.at)
                    );
                    if n == last && waiting > 0 {
                        line.push_str(&format!(
                            "; {waiting} new thing{} wait{} for it",
                            if waiting == 1 { "" } else { "s" },
                            if waiting == 1 { "s" } else { "" }
                        ));
                    }
                    line.push('.');
                    line
                })
                .collect();
            lines.reverse();
            lines
        })
        .unwrap_or_default();
    // A kept binding that went void says when and why (and that the choice is now off).
    let kept_void = v
        .bound
        .as_ref()
        .filter(|b| b.follow.is_some())
        .zip(v.void)
        .map(|(b, why)| {
            keep_void_line(
                why == hideout_registry::keyboard::VoidReason::Moved,
                &time_words(&b.at),
            )
        });
    let keep = keep_row(&name, v);
    Some(Section {
        id: id.into(),
        name,
        items,
        allowed_on,
        history,
        kept_void,
        keep,
        can_change,
        note,
    })
}

/// The Permissions section's row for the choice; `None` for a module with no app path.
fn keep_row(name: &str, v: &View) -> Option<KeepRow> {
    if !v.adopted || v.listing.exe_path.is_empty() {
        return None;
    }
    let path = &v.listing.exe_path;
    let state = if keep_is_on(v) {
        KeepState::On
    } else if v.granted.is_empty() || v.bound.is_none() || v.void.is_some() {
        KeepState::Unavailable(keep_allow_first(name))
    } else if !hideout_registry::permission::on_local_fixed_drive(path) {
        KeepState::Unavailable(KEEP_REFUSED.into())
    } else {
        KeepState::Off
    };
    Some(KeepRow {
        state,
        name: name.into(),
        file: file_of(path),
        folder: folder_of(path),
    })
}

/// The pending items that can be allowed, in the dialog's order (group, then raw).
fn asked_pending(v: &View) -> Vec<&hideout_registry::permission::Item> {
    let mut items: Vec<_> = v
        .listing
        .items
        .iter()
        .filter(|i| i.state == ItemState::Asked && v.pending.contains(&i.raw))
        .collect();
    items.sort_by_key(|i| (group(i.group), i.raw.clone()));
    items
}

/// Which header line a module gets, if any. `enabled`: the module is on in the hub's settings.
pub fn header_kind(v: &View, enabled: bool, cli_seen: bool) -> Option<HeaderKind> {
    if v.void.is_some() {
        Some(HeaderKind::Changed)
    } else if v.bound.is_none() && v.adopted && enabled && !v.pending.is_empty() {
        Some(HeaderKind::Ask)
    } else if v.bound.is_some() && enabled && !asked_pending(v).is_empty() {
        Some(HeaderKind::More)
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
    hidden: Vec<(String, HeaderKind, String)>,
    seen: Vec<Option<serde_json::Value>>,
    /// CLI grant requests waiting for the user (#1183).
    queue: crate::grant_queue::GrantQueue,
    /// The small confirm of Turn on for "Keep my OK after updates in this folder".
    pub keep_confirm: Option<KeepConfirm>,
    keep_drawn: bool,
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
            queue: crate::grant_queue::GrantQueue::default(),
            keep_confirm: None,
            keep_drawn: false,
        };
        p.refresh();
        p
    }

    /// Uses the hub's shared CLI grant queue.
    pub fn with_queue(mut self, queue: crate::grant_queue::GrantQueue) -> PermUi {
        self.queue = queue;
        self
    }

    /// Opens the dialog for a queued CLI grant (only from the user's click). The items the
    /// command named start checked and every other item unchecked, for the list code the
    /// command named; Allow then records exactly what the dialog shows. A list that changed
    /// since the command drops the request and opens the normal dialog with the reason.
    pub fn open_queued(&mut self, id: &str) {
        let Some(q) = self.queue.get(&self.home, id) else {
            return self.open(id, Opener::Change);
        };
        self.open_with(id, Opener::Change, None, None);
        let Some(d) = self.dialog.as_mut() else {
            return;
        };
        if d.code != q.code {
            self.queue.clear(&self.home, id);
            self.open_with(id, Opener::Change, Some(Reason::ListChanged), None);
            return;
        }
        for i in d.items.iter_mut() {
            i.checked = i.allowable && q.items.contains(&i.raw);
        }
    }

    /// The module ids with a CLI grant waiting.
    #[cfg(test)]
    pub fn queued(&self) -> Vec<String> {
        self.queue
            .for_home(&self.home)
            .into_iter()
            .map(|q| q.id)
            .collect()
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
        self.open_with(id, how, None, None);
    }

    fn open_with(
        &mut self,
        id: &str,
        how: Opener,
        reason: Option<Reason>,
        prev: Option<&PermDialog>,
    ) {
        match build_dialog(&self.home, id, reason) {
            Ok(mut d) => {
                if let Some(p) = prev {
                    d.mark_changes(p);
                }
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
            keep,
        }) = a
        else {
            return None;
        };
        // Whatever the dialog decides answers a command waiting for this module.
        self.queue.clear(&self.home, &d.id);
        // Only this path can set the choice; every Allow restates it (unchecked clears it).
        let r = hideout_supervisor::driver::grant_listed_dialog_in(
            &self.home, &self.hub, &d.id, &code, &chosen, &declined, keep,
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
                self.open_with(&d.id, how, Some(Reason::ListChanged), Some(&d));
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

    /// Opens the small confirm of Turn on for "Keep my OK after updates in this folder" (only
    /// from the user's click). It takes the list code as it is now, which covers the app's file.
    pub fn open_keep_confirm(&mut self, id: &str) {
        let Ok(reg) = Registry::open_from_env(&self.home) else {
            return;
        };
        let Ok(v) = reg.permission_view(id) else {
            return self.note(id, KEEP_NOT_SAVED.into());
        };
        let path = v.listing.exe_path.clone();
        self.keep_confirm = Some(KeepConfirm {
            id: id.into(),
            name: v.listing.name.clone(),
            file: file_of(&path),
            folder: folder_of(&path),
            code: v.listing.code().to_string(),
            error: None,
        });
        self.keep_drawn = false;
    }

    /// The answer of the confirm. Keep my OK records the choice for the list the confirm took
    /// (the grants stay as they are); a list that changed meanwhile changes nothing and says so.
    pub fn answer_keep(&mut self, a: KeepAnswer) {
        let Some(mut k) = self.keep_confirm.take() else {
            return;
        };
        if a == KeepAnswer::Cancel {
            return;
        }
        let r = hideout_supervisor::driver::grant_listed_dialog_in(
            &self.home,
            &self.hub,
            &k.id,
            &k.code,
            &[],
            &[],
            true,
        );
        match r {
            Ok(_) => self.refresh(),
            Err(e) if e.contains("list_changed") => {
                log::warn!("permissions: {}: keep not saved: list changed", k.id);
                self.refresh();
                self.note(&k.id, keep_changed_open(&k.name));
            }
            Err(e) => {
                log::warn!("permissions: {}: keep not saved ({e})", k.id);
                k.error = Some(KEEP_NOT_SAVED.into());
                self.keep_confirm = Some(k);
            }
        }
    }

    /// Turn off "Keep my OK after updates in this folder". Narrows, so it never asks.
    pub fn keep_off(&mut self, id: &str) {
        let r = hideout_supervisor::driver::stop_keeping_in(&self.home, id);
        self.refresh();
        let name = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.view.listing.name.clone())
            .unwrap_or_else(|| id.to_string());
        match r {
            Ok(_) => self.note(id, keep_off_note(&name)),
            Err(e) => {
                log::warn!("permissions: {id}: turning off keep not saved ({e})");
                self.note(id, KEEP_NOT_SAVED.into());
            }
        }
    }

    /// Turn off one allowed item: the router first, then the file. Never asks.
    pub fn turn_off(&mut self, id: &str, raw: &str) {
        self.narrow(id, Some(std::slice::from_ref(&raw.to_string())));
    }

    pub fn turn_all_off(&mut self, id: &str) {
        self.narrow(id, None);
    }

    fn narrow(&mut self, id: &str, items: Option<&[String]>) {
        let r = hideout_supervisor::driver::revoke_items_in(&self.home, &self.hub, id, items);
        // Cleared first: only a failure leaves a note; the status line comes from the state.
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.note = None;
        }
        self.refresh();
        // The router already closed it: the section says Off whatever the file says.
        if let Err(e) = r {
            log::warn!("permissions: {id}: turn off not saved ({e})");
            self.note(
                id,
                "Turned off, but the change couldn't be saved.".to_string(),
            );
        }
    }

    /// The header lines (at most one per module) and the Permissions sections, at the top of the
    /// Modules page.
    pub fn top(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let mut open: Option<(String, Opener, egui::Id)> = None;
        let mut hide: Option<(String, HeaderKind, String)> = None;
        let mut off: Option<(String, Option<String>)> = None;
        let mut open_queued: Option<(String, egui::Id)> = None;
        let mut keep_on: Option<(String, egui::Id)> = None;
        let mut keep_stop: Option<String> = None;
        for q in self.queue.for_home(&self.home) {
            let Some(e) = self.entries.iter().find(|e| e.id == q.id) else {
                continue;
            };
            let h = header(
                HeaderKind::Queued,
                &q.id,
                &e.view.listing.name,
                Some((q.items.len(), "")),
            );
            match perm_header_ui(ui, t, &h) {
                Some((true, id)) => open_queued = Some((q.id.clone(), id)),
                // Not now: the command's request is dropped; nothing was allowed.
                Some((false, _)) => self.queue.clear(&self.home, &q.id),
                None => {}
            }
            ui.add_space(8.0);
        }
        for e in &self.entries {
            let kind = header_kind(&e.view, e.enabled, false).filter(|k| {
                let code = e.view.listing.code();
                !self
                    .hidden
                    .iter()
                    .any(|(i, h, c)| i == &e.id && h == k && c == code)
            });
            if let Some(k) = kind {
                let name = &e.view.listing.name;
                let cli = if k == HeaderKind::More {
                    let p = asked_pending(&e.view);
                    p.first().map(|f| (p.len(), f.line.clone()))
                } else {
                    e.view
                        .bound
                        .as_ref()
                        .map(|b| (e.view.granted.len(), date_words(&b.at)))
                };
                let h: PermHeader =
                    header(k, &e.id, name, cli.as_ref().map(|(n, w)| (*n, w.as_str())));
                match perm_header_ui(ui, t, &h) {
                    Some((true, id)) => {
                        let how = if k == HeaderKind::More {
                            Opener::More
                        } else {
                            Opener::Header
                        };
                        open = Some((e.id.clone(), how, id));
                    }
                    Some((false, _)) => {
                        hide = Some((e.id.clone(), k, e.view.listing.code().to_string()))
                    }
                    None => {}
                }
                ui.add_space(8.0);
            }
        }
        for e in &self.entries {
            let Some(mut s) = section_for(&e.id, &e.view) else {
                continue;
            };
            s.note = e.note.clone().or(s.note);
            match section_ui(ui, t, &s, false) {
                Some((SectionAsk::TurnOff(raw), _)) => off = Some((e.id.clone(), Some(raw))),
                Some((SectionAsk::TurnAllOff, _)) => off = Some((e.id.clone(), None)),
                Some((SectionAsk::Change, id)) => open = Some((e.id.clone(), Opener::Change, id)),
                Some((SectionAsk::KeepOn, id)) => keep_on = Some((e.id.clone(), id)),
                Some((SectionAsk::KeepOff, _)) => keep_stop = Some(e.id.clone()),
                None => {}
            }
            ui.add_space(12.0);
        }
        if let Some((id, btn)) = open_queued {
            self.opener_id = Some(btn);
            self.open_queued(&id);
        }
        if let Some((id, how, btn)) = open {
            self.opener_id = Some(btn);
            self.open(&id, how);
        }
        if let Some((id, btn)) = keep_on {
            self.opener_id = Some(btn);
            self.open_keep_confirm(&id);
        }
        if let Some(id) = keep_stop {
            self.keep_off(&id);
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
        if self.dialog.is_none() {
            return self.keep_dialog(ctx, t);
        }
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

    /// Draws the Turn on confirm while it is open; the focus goes back to Turn on when it closes.
    fn keep_dialog(&mut self, ctx: &egui::Context, t: &Tokens) -> Option<Done> {
        let k = self.keep_confirm.as_ref()?;
        let first = !self.keep_drawn;
        self.keep_drawn = true;
        let a = keep_confirm_ui(ctx, t, k, first)?;
        self.answer_keep(a);
        if self.keep_confirm.is_none() {
            if let Some(id) = self.opener_id.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_router::{Router, Settings};
    use std::sync::mpsc;

    const NOW: &str = "2026-11-13T12:00:00Z";
    const ID: &str = "vulpes";

    fn manifest(exe: &Path, perms: &[&str]) -> String {
        let run = exe.display().to_string().replace('\\', "/");
        let perms: Vec<String> = perms.iter().map(|p| format!("\"{p}\"")).collect();
        format!(
            "id = \"{ID}\"\nname = \"Vulpes\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"arete\"\nhub = \"1.0\"\nrun = ['{run}']\napi = {{ arete = \"1.0\" }}\n\
             permissions = [{}]\n",
            perms.join(", ")
        )
    }

    struct Fx {
        home: std::path::PathBuf,
        exe: std::path::PathBuf,
        folder: std::path::PathBuf,
        ui: PermUi,
    }

    fn fx(tag: &str, perms: &[&str]) -> Fx {
        fx_from(tag, |exe| manifest(exe, perms))
    }

    fn fx_from(tag: &str, text: impl Fn(&Path) -> String) -> Fx {
        let base = std::env::temp_dir().join(format!("hideout-pact-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let install = base.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let exe = install.join("vulpes.exe");
        std::fs::write(&exe, b"program v1").unwrap();
        let folder = base.join("manifest");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("module.toml"), text(&exe)).unwrap();
        let home = base.join("home");
        let mut reg = Registry::open(&home, false).unwrap();
        reg.register_adopted(&folder, NOW).unwrap();
        let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
        let (events, _rx) = mpsc::channel();
        let ui = PermUi::new(&home, Hub::new(router, events));
        Fx {
            home,
            exe,
            folder,
            ui,
        }
    }

    fn view(f: &Fx) -> View {
        Registry::open_from_env(&f.home)
            .unwrap()
            .permission_view(ID)
            .unwrap()
    }

    fn view_of(f: &Fx, id: &str) -> View {
        Registry::open_from_env(&f.home)
            .unwrap()
            .permission_view(id)
            .unwrap()
    }

    fn allow_all(f: &mut Fx, how: Opener) -> Option<Done> {
        f.ui.open(ID, how);
        let a = f.ui.dialog.as_ref().unwrap().allowed();
        f.ui.answer(PermAnswer::Allow(a))
    }

    /// #1178 as the supervised run saw it: foxglove's own shipped manifest adopted, nothing
    /// granted, everything pending, module off. Turn on must lead to the dialog (never a dead
    /// end), the user's Allow records the shown list, and only then does Turn on go through.
    #[test]
    fn the_shipped_foxglove_manifest_adopted_is_started_through_the_dialog() {
        const FX: &str = "foxglove";
        let mut f = fx_from("foxglove", |exe| {
            include_str!("../../../app/src/foxglove/hub_link/module.toml")
                .replace("@VERSION@", "0.3.0")
                .replace("@EXE@", &exe.display().to_string().replace('\\', "/"))
        });
        let v = view_of(&f, FX);
        let asked = v.pending.len();
        assert!(asked >= 13, "the manifest asks for {asked}");
        assert!(v.granted.is_empty(), "adopt grants nothing");
        assert!(v.bound.is_none() && v.adopted);
        // What Supervisor::turn_on refuses on, and what routes Turn on to the dialog.
        assert!(f.ui.needs_ok(FX));
        // Off: no header line yet; on (enabled, still unanswered): the first-run Ask header.
        assert_eq!(header_kind(&v, false, true), None);
        assert_eq!(header_kind(&v, true, true), Some(HeaderKind::Ask));
        // Turn on opens the dialog, listing every ask; the user's Allow records them all.
        f.ui.open(FX, Opener::TurnOn);
        let d = f.ui.dialog.clone().expect("Turn on opens the dialog");
        assert_eq!(d.items.iter().filter(|i| i.allowable).count(), asked);
        let done = {
            f.ui.open(FX, Opener::TurnOn);
            let a = f.ui.dialog.as_ref().unwrap().allowed();
            f.ui.answer(PermAnswer::Allow(a))
        }
        .unwrap();
        assert_eq!(done.then, Then::TurnOn);
        let v = view_of(&f, FX);
        assert_eq!(v.granted.len(), asked);
        assert!(v.pending.is_empty());
        // turn_on's gate is open now, and the header is gone.
        assert!(!f.ui.needs_ok(FX));
        assert_eq!(header_kind(&v, true, true), None);
    }

    #[test]
    fn allow_records_the_shown_list_and_turn_on_follows_it() {
        let mut f = fx("allow", &["hub.notify", "ui.window", "provide:a.read"]);
        assert!(f.ui.needs_ok(ID));
        f.ui.open(ID, Opener::TurnOn);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.items.len(), 3);
        assert!(
            d.items.iter().all(|i| i.checked),
            "normal items start checked"
        );
        // Not now writes nothing.
        f.ui.answer(PermAnswer::NotNow);
        assert!(f.ui.dialog.is_none());
        assert!(view(&f).granted.is_empty());
        let done = allow_all(&mut f, Opener::TurnOn).unwrap();
        assert_eq!(done.then, Then::TurnOn);
        assert_eq!(done.chosen.len(), 3);
        assert_eq!(view(&f).granted.len(), 3);
        assert!(!f.ui.needs_ok(ID));
        // Review on an update waiting for the user lets the swap run; Change only records.
        assert_eq!(Opener::Review.then(), Then::Review);
        assert_eq!(Opener::Change.then(), Then::Nothing);
    }

    fn queue_for(f: &mut Fx, items: &[&str]) -> crate::grant_queue::GrantQueue {
        let q = crate::grant_queue::GrantQueue::default();
        let code = view(f).listing.code().to_string();
        q.submit(crate::grant_queue::Queued {
            home: f.home.clone(),
            id: ID.into(),
            code,
            items: items.iter().map(|s| s.to_string()).collect(),
        });
        let ui = PermUi::new(&f.home, f.ui.hub.clone()).with_queue(q.clone());
        f.ui = ui;
        q
    }

    #[test]
    fn confirming_a_queued_cli_grant_applies_exactly_the_listed_items() {
        let mut f = fx("queued", &["hub.notify", "ui.window", "provide:a.read"]);
        let q = queue_for(&mut f, &["hub.notify", "ui.window"]);
        assert_eq!(f.ui.queued(), [ID]);
        // Queued is not allowed.
        assert!(view(&f).granted.is_empty());
        f.ui.open_queued(ID);
        let d = f.ui.dialog.clone().unwrap();
        let checked: Vec<&str> = d
            .items
            .iter()
            .filter(|i| i.checked)
            .map(|i| i.raw.as_str())
            .collect();
        assert_eq!(checked.len(), 2);
        assert!(!checked.contains(&"provide:a.read"));
        // Allow records exactly those two, for the list code the command named.
        let a = d.allowed();
        assert_eq!(a.code, q.get(&f.home, ID).unwrap().code);
        let done = f.ui.answer(PermAnswer::Allow(a)).expect("allowed");
        assert_eq!(done.then, Then::Nothing);
        let mut granted = view(&f)
            .granted
            .iter()
            .map(|g| g.raw.clone())
            .collect::<Vec<_>>();
        granted.sort();
        assert_eq!(granted, ["hub.notify", "ui.window"]);
        assert!(
            q.for_home(&f.home).is_empty(),
            "the answered request stays queued"
        );
    }

    #[test]
    fn a_queued_request_for_an_old_list_is_dropped_and_grants_nothing() {
        let mut f = fx("queuedstale", &["hub.notify", "ui.window"]);
        let q = queue_for(&mut f, &["hub.notify"]);
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&f.exe, &["hub.notify", "ui.window", "lighting.lease"]),
        )
        .unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
        f.ui.open_queued(ID);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.reason, Some(Reason::ListChanged));
        assert!(q.for_home(&f.home).is_empty());
        assert!(view(&f).granted.is_empty());
    }

    #[test]
    fn a_list_that_changed_while_the_dialog_was_open_is_refused_and_reopens() {
        let mut f = fx("changed", &["hub.notify", "ui.window"]);
        f.ui.open(ID, Opener::TurnOn);
        let shown = f.ui.dialog.clone().unwrap();
        // The manifest asks for more while the dialog is open (a re-adopt).
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&f.exe, &["hub.notify", "ui.window", "lighting.lease"]),
        )
        .unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
        let a = shown.allowed();
        assert!(f.ui.answer(PermAnswer::Allow(a)).is_none());
        // Nothing was allowed; the dialog is back with the new list and the reason on top.
        assert!(view(&f).granted.is_empty());
        let d = f.ui.dialog.clone().expect("reopened");
        assert_eq!(d.reason, Some(Reason::ListChanged));
        assert_eq!(d.items.len(), 3);
        assert_ne!(d.code, shown.code);
        assert_eq!(f.ui.opener, Some(Opener::TurnOn));
        // Choosing again on the new list works.
        let a = d.allowed();
        assert!(f.ui.answer(PermAnswer::Allow(a)).is_some());
        assert_eq!(view(&f).granted.len(), 3);
    }

    #[test]
    fn a_voided_grant_goes_back_through_review_then_allow() {
        let mut f = fx("void", &["hub.notify", "ui.window"]);
        // Allow only one of the two.
        f.ui.open(ID, Opener::TurnOn);
        f.ui.dialog
            .as_mut()
            .unwrap()
            .set_checked("ui.window", false);
        let a = f.ui.dialog.as_ref().unwrap().allowed();
        f.ui.answer(PermAnswer::Allow(a)).unwrap();
        assert_eq!(view(&f).granted.len(), 1);
        // The program file changes: the next launch voids the grant.
        std::fs::write(&f.exe, b"program v2").unwrap();
        Registry::open_from_env(&f.home)
            .unwrap()
            .void_if_changed(ID, NOW)
            .unwrap();
        f.ui.refresh();
        let v = view(&f);
        assert!(v.void.is_some() && v.granted.is_empty());
        assert_eq!(header_kind(&v, false, false), Some(HeaderKind::Changed));
        assert!(f.ui.needs_ok(ID));
        // Review opens with the void line and the old choice as the default.
        f.ui.open(ID, Opener::Header);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.reason, Some(Reason::Void));
        let checked: Vec<_> = d
            .items
            .iter()
            .filter(|i| i.checked)
            .map(|i| &i.raw[..])
            .collect();
        assert_eq!(checked, ["hub.notify"]);
        // Declined items start unchecked and the button count follows.
        assert!(
            !d.items
                .iter()
                .find(|i| i.raw == "ui.window")
                .unwrap()
                .checked
        );
        assert_eq!(d.allow_label(), "Allow 1");
        assert_eq!(d.all_state(), hideout_shell::permission_dialog::All::Some);
        let done = f.ui.answer(PermAnswer::Allow(d.allowed())).unwrap();
        assert_eq!(done.then, Then::TurnOn);
        let v = view(&f);
        assert!(v.void.is_none());
        assert_eq!(v.granted.len(), 1);
        assert!(!f.ui.needs_ok(ID));
    }

    #[test]
    fn turning_off_only_narrows_and_change_offers_it_again_through_the_full_dialog() {
        let mut f = fx("narrow", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        f.ui.turn_off(ID, "ui.window");
        let v = view(&f);
        assert_eq!(
            v.granted.iter().map(|i| &i.raw[..]).collect::<Vec<_>>(),
            ["hub.notify"]
        );
        let s = section_for(ID, &v).unwrap();
        let word = |raw: &str| s.items.iter().find(|i| i.raw == raw).unwrap().word;
        assert_eq!(word("hub.notify"), ItemWord::Allowed);
        assert_eq!(word("ui.window"), ItemWord::Off);
        assert!(s.can_change);
        assert!(s
            .allowed_on
            .as_deref()
            .is_some_and(|l| l.starts_with("Allowed in Hideout on ")));
        // Change… lists only what is off, unchecked; nothing widens without Allow.
        f.ui.open(ID, Opener::Change);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.items.len(), 1);
        assert!(!d.items[0].checked);
        assert!(f.ui.answer(PermAnswer::NotNow).is_none());
        assert_eq!(view(&f).granted.len(), 1);
        // An item that is not allowed can't be turned off; Turn all off empties the grant.
        f.ui.turn_off(ID, "ui.window");
        assert_eq!(view(&f).granted.len(), 1);
        f.ui.turn_all_off(ID);
        assert!(view(&f).granted.is_empty());
    }

    #[test]
    fn counts_agree_and_the_status_line_follows_the_item() {
        let mut f = fx(
            "counts",
            &["facts.write:keyboard.*", "ui.window", "modules.manage"],
        );
        f.ui.open(ID, Opener::TurnOn);
        let d = f.ui.dialog.clone().unwrap();
        let never = d.items.iter().filter(|i| !i.allowable).count();
        assert_eq!(never, 1);
        assert_eq!(d.items.len(), 3);
        assert!(d
            .intro()
            .starts_with("Vulpes asks for these 3 things. 1 can't be allowed."));
        assert_eq!(d.all_label(), "All 2");
        assert_eq!(d.allow_label(), "Allow 2");
        allow_all(&mut f, Opener::TurnOn).unwrap();
        // Allowed: no line about empty tiles.
        let s = section_for(ID, &view(&f)).unwrap();
        assert!(s.note.is_none());
        // Off: the line shows, and it goes again when the item is allowed through Change.
        f.ui.turn_off(ID, "facts.write:keyboard.*");
        let s = section_for(ID, &view(&f)).unwrap();
        assert_eq!(
            s.note.as_deref(),
            Some("Vulpes can't share its status. Its tiles stay empty.")
        );
        f.ui.open(ID, Opener::Change);
        let a = f.ui.dialog.as_mut().unwrap();
        a.set_checked("facts.write:keyboard.*", true);
        let a = a.allowed();
        f.ui.answer(PermAnswer::Allow(a)).unwrap();
        assert!(section_for(ID, &view(&f)).unwrap().note.is_none());
    }

    #[test]
    fn the_allowed_line_follows_who_allowed_it() {
        let f = fx("by", &["hub.notify"]);
        let v = view(&f);
        let list = v.listing.code().to_string();
        let mut reg = Registry::open_from_env(&f.home).unwrap();
        reg.approve_listed(
            ID,
            &list,
            &["hub.notify".to_string()],
            &[],
            NOW,
            "cli",
            false,
        )
        .unwrap();
        let v = view(&f);
        assert_eq!(header_kind(&v, false, false), Some(HeaderKind::Cli));
        assert_eq!(
            section_for(ID, &v).unwrap().allowed_on.as_deref(),
            Some("Allowed from the command line on 13 Nov 2026")
        );
    }

    #[test]
    fn a_changed_list_marks_the_new_item_and_names_the_dropped_one() {
        let mut f = fx("marks", &["hub.notify", "ui.window"]);
        f.ui.open(ID, Opener::TurnOn);
        let shown = f.ui.dialog.clone().unwrap();
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&f.exe, &["hub.notify", "lighting.lease"]),
        )
        .unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
        assert!(f.ui.answer(PermAnswer::Allow(shown.allowed())).is_none());
        let d = f.ui.dialog.clone().unwrap();
        assert_ne!(d.code, shown.code);
        let new: Vec<_> = d
            .items
            .iter()
            .filter(|i| i.new)
            .map(|i| &i.raw[..])
            .collect();
        assert_eq!(new, ["lighting.lease"]);
        let gone = shown.items.iter().find(|i| i.raw == "ui.window").unwrap();
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0], gone.line);
        assert!(d
            .reason_lines()
            .contains(&format!("No longer asked: {}", gone.line)));
    }

    /// Allows two items, then a signed update (same folder, attested) brings a bigger manifest.
    fn updated_with_more(tag: &str, more: &[&str]) -> Fx {
        let mut f = fx(tag, &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        std::fs::write(&f.exe, b"program v2").unwrap();
        let mut perms = vec!["hub.notify", "ui.window"];
        perms.extend_from_slice(more);
        std::fs::write(f.folder.join("module.toml"), manifest(&f.exe, &perms)).unwrap();
        let mut reg = Registry::open(&f.home, false).unwrap();
        reg.register_adopted(&f.folder, NOW).unwrap();
        reg.attest_update(ID, "1.1.0", "updater", NOW).unwrap();
        f.ui.refresh();
        f
    }

    fn more_header(f: &Fx) -> Option<HeaderKind> {
        header_kind(&view(f), true, true)
    }

    #[test]
    fn an_update_that_asks_for_more_gets_the_more_header_and_allow_only_adds() {
        let mut f = updated_with_more("more", &["provide:a.read", "provide:a.set"]);
        let v = view(&f);
        assert!(v.void.is_none());
        assert_eq!(v.granted.len(), 2, "the kept grant stays");
        assert_eq!(more_header(&f), Some(HeaderKind::More));
        // The module being off shows no header (Turn on goes through the dialog instead).
        assert_eq!(header_kind(&v, false, true), None);
        // The header counts the new things and quotes the first one in dialog order.
        let p = asked_pending(&v);
        assert_eq!(p.len(), 2);
        let h = header(HeaderKind::More, ID, "Vulpes", Some((p.len(), &p[0].line)));
        assert_eq!(h.title, "Vulpes's update asks for more");
        assert!(h
            .body
            .starts_with("Its update asks for 2 new things, like \u{201c}"));
        // Section: pending says Waits, the kept grant says Allowed; the history says so.
        let s = section_for(ID, &v).unwrap();
        let word = |raw: &str| s.items.iter().find(|i| i.raw == raw).unwrap().word;
        assert_eq!(word("provide:a.read"), ItemWord::Waits);
        assert_eq!(word("hub.notify"), ItemWord::Allowed);
        assert_eq!(
            s.history,
            ["Updated to 1.1.0 on 13 Nov 2026. Your OK stayed; 2 new things wait for it."]
        );
        // Review: only the unanswered items, all New, with the More reason.
        f.ui.open(ID, Opener::More);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.reason, Some(Reason::More));
        assert_eq!(d.title(), "Let Vulpes do more?");
        assert_eq!(d.items.len(), 2);
        assert!(d.items.iter().all(|i| i.new && i.checked));
        assert!(d.items[0].said().ends_with(", new, checked"));
        assert_eq!(
            d.reason_lines()[1],
            "What you allowed before stays allowed."
        );
        // Allow one of the two: nothing is revoked, the other is declined, the header is gone.
        let mut d2 = d.clone();
        d2.set_checked("provide:a.set", false);
        let done = f.ui.answer(PermAnswer::Allow(d2.allowed())).unwrap();
        assert_eq!(done.then, Then::Nothing);
        let v = view(&f);
        let mut granted: Vec<_> = v.granted.iter().map(|g| g.raw.clone()).collect();
        granted.sort();
        assert_eq!(
            granted,
            ["hub.notify", "provide:a.read", "ui.window"],
            "grew by exactly the chosen item"
        );
        assert_eq!(more_header(&f), None);
        // The old grants never went back to pending, and the binding stayed (growth-only).
        let reg = Registry::open_from_env(&f.home).unwrap();
        let m = &reg.inventory.modules[ID];
        assert!(m.pending_permissions.is_empty());
        assert!(m.bound.as_ref().is_some_and(|b| !b.void));
        // The declined one says Off now, and the history is back to plain.
        let s = section_for(ID, &v).unwrap();
        assert_eq!(
            s.items
                .iter()
                .find(|i| i.raw == "provide:a.set")
                .unwrap()
                .word,
            ItemWord::Off
        );
        assert_eq!(
            s.history,
            ["Updated to 1.1.0 on 13 Nov 2026. Your OK stayed."]
        );
        // A declined item never brings the header back; Change opens without New marks.
        f.ui.open(ID, Opener::Change);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.reason, None);
        assert!(d.items.iter().all(|i| !i.new));
    }

    #[test]
    fn one_new_thing_reads_in_the_singular_and_a_void_update_says_changed() {
        let f = updated_with_more("one", &["provide:a.read"]);
        let v = view(&f);
        let p = asked_pending(&v);
        let h = header(HeaderKind::More, ID, "Vulpes", Some((1, &p[0].line)));
        assert!(h.body.starts_with("Its update asks for one new thing: "));
        assert_eq!(
            section_for(ID, &v).unwrap().history[0],
            "Updated to 1.1.0 on 13 Nov 2026. Your OK stayed; 1 new thing waits for it."
        );
        // The program then changes without a signed update: Changed wins over More.
        std::fs::write(&f.exe, b"another program").unwrap();
        Registry::open_from_env(&f.home)
            .unwrap()
            .void_if_changed(ID, NOW)
            .unwrap();
        assert_eq!(more_header(&f), Some(HeaderKind::Changed));
    }

    #[test]
    fn a_smaller_update_drops_only_the_item_no_longer_asked_and_shows_no_header() {
        let mut f = fx("less", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        std::fs::write(&f.exe, b"program v2").unwrap();
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&f.exe, &["hub.notify"]),
        )
        .unwrap();
        let mut reg = Registry::open(&f.home, false).unwrap();
        reg.register_adopted(&f.folder, NOW).unwrap();
        reg.attest_update(ID, "1.1.0", "updater", NOW).unwrap();
        f.ui.refresh();
        let v = view(&f);
        assert!(v.void.is_none());
        let granted: Vec<_> = v.granted.iter().map(|g| g.raw.clone()).collect();
        assert_eq!(granted, ["hub.notify"], "the remaining grant stays");
        assert!(asked_pending(&v).is_empty());
        assert_eq!(more_header(&f), None);
    }

    #[test]
    fn nothing_asked_more_means_no_header() {
        let f = updated_with_more("quiet", &[]);
        assert_eq!(more_header(&f), None);
        let s = section_for(ID, &view(&f)).unwrap();
        assert_eq!(
            s.history,
            ["Updated to 1.1.0 on 13 Nov 2026. Your OK stayed."]
        );
    }

    #[test]
    fn not_now_hides_the_header_until_the_list_changes() {
        let mut f = updated_with_more("notnow", &["provide:a.read"]);
        let code = view(&f).listing.code().to_string();
        f.ui.hidden
            .push((ID.into(), HeaderKind::More, code.clone()));
        let shown = |f: &Fx| {
            let v = view(f);
            header_kind(&v, true, true).filter(|k| {
                !f.ui
                    .hidden
                    .iter()
                    .any(|(i, h, c)| i == ID && h == k && c == v.listing.code())
            })
        };
        assert_eq!(shown(&f), None);
        // The next update asks for one more: a new list code, the header is back.
        let perms = ["hub.notify", "ui.window", "provide:a.read", "provide:a.set"];
        std::fs::write(f.folder.join("module.toml"), manifest(&f.exe, &perms)).unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
        assert_ne!(view(&f).listing.code(), code);
        assert_eq!(shown(&f), Some(HeaderKind::More));
        // Not now in the dialog writes nothing and leaves Waits.
        f.ui.open(ID, Opener::More);
        f.ui.answer(PermAnswer::NotNow);
        assert!(f.ui.dialog.is_none());
        assert_eq!(view(&f).pending.len(), 2);
    }

    #[test]
    fn the_more_words_avoid_the_banned_ones() {
        let h = header(
            HeaderKind::More,
            ID,
            "Foxglove",
            Some((3, "Pause your live wallpaper")),
        );
        let d = PermDialog {
            id: ID.into(),
            name: "Foxglove".into(),
            code: "0".into(),
            path: String::new(),
            items: Vec::new(),
            reason: Some(Reason::More),
            keyboard: None,
            keep: None,
            removed: Vec::new(),
            error: None,
        };
        let mut all = vec![h.title, h.body, d.title()];
        all.extend(d.reason_lines());
        all.push(hideout_shell::permission_dialog::WAITS.to_string());
        for t in all {
            let t = t.to_lowercase();
            for bad in ["program", "provide", "method", "palette", "look ", "effect"] {
                assert!(!t.contains(bad), "{t} says {bad}");
            }
        }
    }

    #[test]
    fn the_gpu_manager_words_avoid_the_banned_ones() {
        let (g, line, sensitive) = hideout_registry::permission::describe("provide:gpu.*");
        assert_eq!(g, hideout_registry::permission::Group::Sensitive);
        let item = ListItem {
            raw: "provide:gpu.*".into(),
            line,
            group: Group::Sensitive,
            allowable: true,
            sensitive,
            reason: None,
            checked: false,
            confirmed: false,
            new: false,
        };
        assert!(item.sensitive);
        let said = item.said();
        let confirm = item.confirm_line("Gpu Helper");
        for t in [item.line.clone(), said, confirm] {
            let t = t.to_lowercase();
            for bad in [
                "program", "provide", "method", "palette", "look ", "effect", "broker",
            ] {
                assert!(!t.contains(bad), "{t} says {bad}");
            }
        }
    }

    #[test]
    fn dates_read_in_words() {
        assert_eq!(date_words("2026-11-13T12:00:00Z"), "13 Nov 2026");
        assert_eq!(date_words("later"), "later");
    }

    /// Checks the dialog's keep box (understanding included) and presses Allow.
    fn allow_keeping(f: &mut Fx, how: Opener, keep: bool) {
        f.ui.open(ID, how);
        let mut d = f.ui.dialog.clone().expect("the dialog opens");
        let k = d.keep.as_mut().expect("the keep box is offered");
        k.set_on(keep);
        k.understood = keep;
        assert!(f.ui.answer(PermAnswer::Allow(d.allowed())).is_some());
    }

    fn follow_of(f: &Fx) -> Option<hideout_registry::inventory::FollowFolder> {
        view(f).bound.and_then(|b| b.follow)
    }

    #[test]
    fn the_choice_is_set_only_by_a_checked_and_understood_allow_in_the_dialog() {
        let mut f = fx("keep-dialog", &["hub.notify", "ui.window"]);
        // Offered, off by default; a plain Allow leaves no follow.
        f.ui.open(ID, Opener::TurnOn);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.keep, Some(Keep::offered(false)));
        assert!(f.ui.answer(PermAnswer::Allow(d.allowed())).is_some());
        assert!(follow_of(&f).is_none());
        // Checked but not understood: not sent.
        f.ui.open(ID, Opener::Change);
        let mut d = f.ui.dialog.clone().unwrap();
        d.keep.as_mut().unwrap().set_on(true);
        assert!(!d.allowed().keep);
        f.ui.answer(PermAnswer::NotNow);
        assert!(follow_of(&f).is_none());
        // Checked and understood: set, with the folder and file of the app, by the dialog.
        allow_keeping(&mut f, Opener::Change, true);
        let fo = follow_of(&f).expect("kept");
        assert_eq!(fo.by, "dialog");
        assert_eq!(fo.file_name, "vulpes.exe");
        // The dialog now opens with the box checked; an Allow that leaves it alone keeps it.
        f.ui.open(ID, Opener::Change);
        let d = f.ui.dialog.clone().unwrap();
        assert_eq!(d.keep, Some(Keep::offered(true)));
        assert!(f.ui.answer(PermAnswer::Allow(d.allowed())).is_some());
        assert!(follow_of(&f).is_some());
        // Unchecking in the dialog and pressing Allow clears it; the grants stay.
        let granted = view(&f).granted.len();
        allow_keeping(&mut f, Opener::Change, false);
        assert!(follow_of(&f).is_none());
        assert_eq!(view(&f).granted.len(), granted);
    }

    #[test]
    fn the_section_row_follows_the_state_and_turn_on_asks_first() {
        let mut f = fx("keep-row", &["hub.notify", "ui.window"]);
        let row = |f: &Fx| section_for(ID, &view(f)).unwrap().keep.unwrap();
        // Nothing allowed yet: no button, the muted words.
        assert_eq!(
            row(&f).state,
            KeepState::Unavailable(keep_allow_first(&view(&f).listing.name))
        );
        allow_all(&mut f, Opener::TurnOn).unwrap();
        assert_eq!(row(&f).state, KeepState::Off);
        let granted = view(&f).granted.clone();
        // Turn on… opens the confirm, which writes nothing; Cancel leaves it off.
        f.ui.open_keep_confirm(ID);
        let k = f.ui.keep_confirm.clone().expect("the confirm opens");
        assert_eq!(k.file, "vulpes.exe");
        assert_eq!(k.folder, f.exe.parent().unwrap().display().to_string());
        assert!(follow_of(&f).is_none());
        f.ui.answer_keep(KeepAnswer::Cancel);
        assert!(f.ui.keep_confirm.is_none() && follow_of(&f).is_none());
        // Keep my OK sets it and leaves every grant as it was.
        f.ui.open_keep_confirm(ID);
        f.ui.answer_keep(KeepAnswer::Keep);
        assert!(f.ui.keep_confirm.is_none());
        assert_eq!(follow_of(&f).unwrap().by, "dialog");
        assert_eq!(view(&f).granted, granted);
        assert_eq!(row(&f).state, KeepState::On);
        // Turn off clears only the choice and says so.
        f.ui.keep_off(ID);
        assert!(follow_of(&f).is_none());
        assert_eq!(view(&f).granted, granted);
        assert_eq!(row(&f).state, KeepState::Off);
        let note =
            f.ui.entries
                .iter()
                .find(|e| e.id == ID)
                .unwrap()
                .note
                .clone();
        assert_eq!(note, Some(keep_off_note(&view(&f).listing.name)));
    }

    #[test]
    fn a_list_that_changed_while_the_confirm_was_open_changes_nothing() {
        let mut f = fx("keep-stale", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        f.ui.open_keep_confirm(ID);
        // The app asks for more while the confirm is open.
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&f.exe, &["hub.notify", "ui.window", "lighting.lease"]),
        )
        .unwrap();
        let mut reg = Registry::open(&f.home, false).unwrap();
        reg.register_adopted(&f.folder, NOW).unwrap();
        f.ui.answer_keep(KeepAnswer::Keep);
        assert!(f.ui.keep_confirm.is_none());
        assert!(follow_of(&f).is_none(), "nothing was changed");
        let note =
            f.ui.entries
                .iter()
                .find(|e| e.id == ID)
                .unwrap()
                .note
                .clone();
        assert_eq!(note, Some(keep_changed_open(&view(&f).listing.name)));
    }

    #[test]
    fn a_kept_new_build_reads_in_the_history_and_a_moved_app_turns_the_choice_off() {
        let mut f = fx("keep-history", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        allow_keeping(&mut f, Opener::Change, true);
        std::fs::write(&f.exe, b"program v2").unwrap();
        let mut reg = Registry::open(&f.home, false).unwrap();
        assert!(reg.keep_followed(ID, NOW).unwrap());
        let v = view(&f);
        assert!(v.void.is_none());
        let s = section_for(ID, &v).unwrap();
        assert_eq!(s.history.len(), 1);
        assert!(
            s.history[0].starts_with("New build on ")
                && s.history[0].ends_with("Your OK stayed (kept for this folder)."),
            "{:?}",
            s.history
        );
        assert_eq!(s.keep.unwrap().state, KeepState::On);
        assert!(s.kept_void.is_none());
        // The app moves: the grant is void, the line says so and Turn on is gone.
        let moved = f.exe.with_file_name("vulpes2.exe");
        std::fs::copy(&f.exe, &moved).unwrap();
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&moved, &["hub.notify", "ui.window"]),
        )
        .unwrap();
        reg.register_adopted(&f.folder, NOW).unwrap();
        let v = view(&f);
        assert!(v.void.is_some());
        let s = section_for(ID, &v).unwrap();
        assert!(s
            .kept_void
            .as_deref()
            .is_some_and(|l| l.starts_with("Moved or removed on ")
                && l.ends_with("Keeping your OK after updates is now off.")));
        assert!(matches!(s.keep.unwrap().state, KeepState::Unavailable(_)));
    }

    #[test]
    fn local_time_words_read_the_date_and_the_clock() {
        let w = time_words("2026-11-17T12:30:00Z");
        assert!(
            w.starts_with("17 Nov 2026, ")
                || w.starts_with("16 Nov 2026, ")
                || w.starts_with("18 Nov 2026, "),
            "{w}"
        );
        assert_eq!(w.len(), "17 Nov 2026, 12:30".len());
        assert_eq!(time_words("2026-11-17"), "17 Nov 2026");
        assert_eq!(time_words("soon"), "soon");
    }
}
