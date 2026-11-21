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
    file_of, first_run_header, header, keep_allow_first, keep_changed_open, keep_history,
    keep_off_note, keep_void_line, Allowed, FirstRun, Group, HeaderKind, ItemWord, Keep,
    KeepAnswer, KeepConfirm, KeepRow, KeepState, KeyboardChoice, ListItem, PermAnswer, PermDialog,
    PermHeader, Reason, Section, SectionItem, BUSY, KEEP_NOT_SAVED, KEEP_REFUSED, KEEP_SYNCED,
    KEEP_SYNC_UNKNOWN, NOT_SAVED,
};
use hideout_shell::permission_dialog_ui::{
    first_run_dialog_ui, keep_confirm_ui, perm_dialog_ui, perm_header_ui, section_ui,
    FirstRunAnswer, SectionAsk,
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
    /// The dialog said a standalone copy would be closed (FR5): the page asks the copy to quit
    /// before it starts the app.
    pub handover: bool,
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

/// First-run presentation (FR2): when Allow also starts the app, the button says so and the
/// keyboard-sharing section stays out of the dialog (it is still offered from the app's
/// Permissions, and nothing is recorded for it, so it is asked later, not declined now).
fn present_first_run(d: &mut PermDialog, how: Opener, standalone: bool, starts: bool) {
    if how.then() == Then::TurnOn {
        d.view.starts = starts;
        d.view.handover = standalone;
        d.keyboard = None;
        if d.reason.is_none() && d.sensitive_count() > 0 && d.items.iter().all(|i| !i.before) {
            // One "I understand" for the whole sensitive group.
            d.view.group_ack = Some(false);
        }
    }
}

/// The dialog for `id` as the registry lists it now. Normal-risk items start checked, sensitive
/// ones and items the user turned off start unchecked. After a void (a re-ask) only the
/// non-sensitive items the user allowed before, for the same file path on a local drive, start
/// checked and marked "Allowed before"; all else starts unchecked. Display only: Allow decides.
pub fn build_dialog(home: &Path, id: &str, reason: Option<Reason>) -> Result<PermDialog, String> {
    let reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    let v = reg.permission_view(id).map_err(|e| format!("{e:?}"))?;
    let after_void = v.void.is_some() || !v.last_choice.is_empty();
    // #1465: the app file changed under "Keep my OK": the Sensitive items that waited are shown
    // pre-checked as allowed before (still unconfirmed, so Allow counts them only after the user
    // confirms them).
    let kept = matches!(reason, None | Some(Reason::Kept)) && kept_changed(&v);
    let items: Vec<ListItem> = v
        .listing
        .items
        .iter()
        .map(|i| {
            let allowable = i.state == ItemState::Asked;
            // A re-ask (after a void) is display-only help: an item is pre-checked only when the
            // user allowed it before, it is not sensitive, and the app is at exactly the same
            // place on this PC's own drive. Everything else (new items, sensitive items, a
            // moved or network path) starts unchecked. Allow is still the user's click.
            let before = after_void
                && allowable
                && (!i.sensitive || kept)
                && v.same_place
                && v.last_choice.contains(&i.raw)
                && !v.declined.contains(&i.raw);
            let checked = if after_void {
                before
            } else {
                allowable && !i.sensitive && !v.declined.contains(&i.raw)
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
                before,
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
    let more =
        reason.is_none() && v.void.is_none() && v.bound.is_some() && !asked_pending(&v).is_empty();
    let reason = reason
        .or(v.void.map(|_| Reason::Void))
        .or(kept.then_some(Reason::Kept))
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
        same_place: v.same_place,
        view: Default::default(),
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
        if !hideout_registry::permission::on_local_fixed_drive(&v.listing.exe_path) {
            Keep::refused()
        } else {
            match sync_line(v) {
                Some(why) => Keep::refused_with(why),
                None => Keep::offered(keep_is_on(v)),
            }
        },
    )
}

/// Why "Keep my OK" is off for a folder a cloud service keeps in step, or one Hideout could not
/// read (#1465); `None` for a folder that stays on this PC.
fn sync_line(v: &View) -> Option<&'static str> {
    use hideout_registry::sync_root::SyncAnswer;
    match v.folder_sync {
        SyncAnswer::Local => None,
        SyncAnswer::Synced => Some(KEEP_SYNCED),
        SyncAnswer::Unknown => Some(KEEP_SYNC_UNKNOWN),
    }
}

/// The app file changed under "Keep my OK" (#1465): what carried over still runs, and Sensitive
/// items wait for the OK again. The newest kept step is the folder's, and a Sensitive item waits.
pub fn kept_changed(v: &View) -> bool {
    v.void.is_none()
        && v.bound.as_ref().is_some_and(|b| {
            !b.void
                && b.follow.is_some()
                && b.attested
                    .last()
                    .is_some_and(|a| a.by == hideout_registry::permission::FOLLOW_ATTESTED_BY)
        })
        && asked_pending(v)
            .iter()
            .any(|i| hideout_registry::permission::held_on_change(&i.raw))
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
    // Oldest first; the page shows the last few and folds the rest under "and N earlier".
    let history = v
        .bound
        .as_ref()
        .map(|b| {
            let last = b.attested.len().saturating_sub(1);
            b.attested
                .iter()
                .enumerate()
                .map(|(n, a)| {
                    let grew = if n == last { waiting } else { 0 };
                    if a.by == hideout_registry::permission::FOLLOW_ATTESTED_BY {
                        return keep_history(&time_words(&a.at), grew);
                    }
                    if a.by == hideout_registry::dev_folder::DEV_ATTESTED_BY {
                        use hideout_supervisor::words::{new_build_line, NewBuild};
                        // TODO(S2): with the diff, `AsksMore(new_only)` when the build grew.
                        let what = if grew > 0 {
                            NewBuild::KeptWaiting(grew)
                        } else {
                            NewBuild::Kept
                        };
                        return new_build_line(&time_words(&a.at), what);
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
                .collect()
        })
        .unwrap_or_default();
    // A kept binding that went void says when and why (and that the choice is now off).
    // Only once the void is saved (then `at` is the time it went void); moved or changed comes
    // from where the app runs now against where it was allowed, never from `binding_void`.
    let kept_void = v
        .bound
        .as_ref()
        .filter(|b| b.follow.is_some() && b.void)
        .map(|b| {
            let moved = !b.exe_path.is_empty()
                && (v.listing.exe_path.is_empty()
                    || !hideout_registry::keyboard::same_path(&b.exe_path, &v.listing.exe_path)
                    || !Path::new(&v.listing.exe_path).exists());
            keep_void_line(moved, &time_words(&b.at))
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

/// Whether the choice can be turned on for this fresh view: the OK is live (not void, grants
/// stay), the app is an adopted one on this PC's own drive.
fn keep_can_turn_on(v: &View) -> bool {
    v.adopted
        && v.void.is_none()
        && !v.granted.is_empty()
        && v.bound.as_ref().is_some_and(|b| !b.void)
        && hideout_registry::permission::on_local_fixed_drive(&v.listing.exe_path)
        && sync_line(v).is_none()
}

/// The Permissions section's row for the choice; `None` for a module with no app path.
fn keep_row(name: &str, v: &View) -> Option<KeepRow> {
    if !v.adopted || v.listing.exe_path.is_empty() {
        return None;
    }
    let path = &v.listing.exe_path;
    let state = if keep_is_on(v) {
        match sync_line(v) {
            Some(why) => KeepState::OnHeld(why.into()),
            None => KeepState::On,
        }
    } else if v.granted.is_empty() || v.bound.is_none() || v.void.is_some() {
        KeepState::Unavailable(keep_allow_first(name))
    } else if !hideout_registry::permission::on_local_fixed_drive(path) {
        KeepState::Unavailable(KEEP_REFUSED.into())
    } else if let Some(why) = sync_line(v) {
        KeepState::Unavailable(why.into())
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

/// Whether the newest program change the grant kept was a dev-folder rebuild (rebuild R6): the
/// header then speaks of the new build, not of an update.
fn dev_build(v: &View) -> bool {
    v.bound
        .as_ref()
        .and_then(|b| b.attested.last())
        .is_some_and(|a| a.by == hideout_registry::dev_folder::DEV_ATTESTED_BY)
}

/// Which header line a module gets, if any. `enabled`: the module is on in the hub's settings.
pub fn header_kind(v: &View, enabled: bool, cli_seen: bool) -> Option<HeaderKind> {
    if v.void.is_some() {
        Some(HeaderKind::Changed)
    } else if v.bound.is_none() && v.adopted && !v.pending.is_empty() {
        // Slice 2 (allow means on): an Off adopted app asks too, because Allow turns it on.
        Some(HeaderKind::Ask)
    } else if v.bound.is_some() && enabled && kept_changed(v) {
        Some(HeaderKind::ChangedKept)
    } else if v.bound.is_some() && enabled && !asked_pending(v).is_empty() {
        Some(if dev_build(v) {
            HeaderKind::MoreBuild
        } else {
            HeaderKind::More
        })
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
    /// The first-run review: one dialog, a section per app (UX cut 3).
    pub first_run: Option<FirstRun>,
    first_run_drawn: bool,
    /// The first-run header the user answered Not now, by (app, list code), until a list changes.
    first_hidden: Vec<(String, String)>,
    /// What the hub can see of standalone copies (FR5); none in tests that do not need it.
    standalone: Option<std::sync::Arc<dyn crate::handover::Standalone>>,
    /// The note after a hand-over that did not happen, with Start again.
    pub(crate) retry: Option<String>,
    /// The app whose standalone copy is being closed, and the worker's answer when it comes.
    handing: Vec<(String, std::sync::mpsc::Receiver<Handed>)>,
    /// The window's context, to ask for a redraw when the worker is done.
    ctx: Option<egui::Context>,
    /// One note per app whose copy stayed (two apps can each have one).
    pub(crate) handover_notes: Vec<hideout_shell::handover::Note>,
}

/// What a hand-over worker found. `started` is the app's start, done on the worker itself when
/// the page gave it a way to (so it happens even when the Apps page is not on screen).
#[derive(Debug)]
pub struct Handed {
    pub went: bool,
    pub started: Option<Result<(), String>>,
}

/// Starts one app from any thread (the supervisor's Turn on).
pub type Starter = std::sync::Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

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
            first_run: None,
            first_run_drawn: false,
            first_hidden: Vec::new(),
            standalone: None,
            handover_notes: Vec::new(),
            retry: None,
            handing: Vec::new(),
            ctx: None,
        };
        p.refresh();
        p
    }

    /// Lets the dialog offer to close a standalone copy of an app (first-run FR5).
    pub fn with_standalone(mut self, s: std::sync::Arc<dyn crate::handover::Standalone>) -> PermUi {
        self.standalone = Some(s);
        self
    }

    /// Whether a verified standalone copy of `id` is open now (an unsure case is not one).
    pub(crate) fn copy_open(&self, id: &str) -> bool {
        self.standalone
            .as_ref()
            .is_some_and(|s| s.seen(id) == crate::handover::Seen::Standalone)
    }

    /// Starts closing the standalone copy of `id` on a worker (the wait can take seconds, so the
    /// window never blocks on it). True: a hand-over is under way (or already was) and the page
    /// starts the app only when [`PermUi::finished_hand_over`] says yes. False: nothing to hand
    /// over to (no standalone support here), so the app starts at once.
    pub fn start_hand_over(&mut self, id: &str, starter: Option<Starter>) -> bool {
        self.start_worker(id, starter, |s, app| crate::handover::hand_over(s, app))
    }

    /// Allow without a promised hand-over: looks once more, on a worker, for a copy that opened
    /// after the dialog. Same contract as [`PermUi::start_hand_over`]: the page starts the app
    /// only when [`PermUi::finished_hand_over`] says yes (no copy now); a copy, or one that cannot be
    /// verified (`Unsure`, as `hand_over` reads it), gives the note.
    /// Nothing is ever closed by this.
    pub fn start_late_check(&mut self, id: &str, starter: Option<Starter>) -> bool {
        self.start_worker(id, starter, |s, app| {
            s.seen(app) == crate::handover::Seen::No
        })
    }

    fn start_worker(
        &mut self,
        id: &str,
        starter: Option<Starter>,
        work: impl FnOnce(&dyn crate::handover::Standalone, &str) -> bool + Send + 'static,
    ) -> bool {
        let Some(s) = self.standalone.clone() else {
            return false;
        };
        // One slot per app: a second app's Allow never waits behind (or is dropped by) the first.
        if self.handing.iter().any(|(h, _)| h == id) {
            return true;
        }
        self.handover_notes.retain(|n| n.id != id);
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let app = id.to_string();
        let spawned = std::thread::Builder::new()
            .name("hand-over-ui".into())
            .spawn(move || {
                let went = work(s.as_ref(), &app);
                let started = if went { starter.map(|f| f(&app)) } else { None };
                let _ = tx.send(Handed { went, started });
                // The hub runs a frame at once, even with its window hidden in the tray, so
                // `background` settles the result (an event, no timer).
                if let Some(c) = ctx {
                    hideout_shell::wake_for_background(&c);
                }
            })
            .is_ok();
        if spawned {
            self.handing.push((id.to_string(), rx));
        } else {
            self.stays(id);
        }
        true
    }

    /// The copy stays: a note with Start again, nothing started.
    pub(crate) fn stays(&mut self, id: &str) {
        let name = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.view.listing.name.clone())
            .unwrap_or_else(|| id.to_string());
        log::warn!("hand-over: {id}: the standalone copy stays open; not starting");
        self.handover_notes.retain(|n| n.id != id);
        self.handover_notes.push(hideout_shell::handover::Note {
            id: id.to_string(),
            name,
        });
    }

    /// A finished hand-over, once: the app's id and what the worker found. `wait` blocks for the
    /// oldest worker (tests); the page passes false and takes whatever is done. Call it until it
    /// answers `None`: every app that was allowed gets its turn.
    pub fn finished_hand_over(&mut self, wait: bool) -> Option<(String, Handed)> {
        use std::sync::mpsc::TryRecvError;
        let gone = || Handed {
            went: false,
            started: None,
        };
        let (id, handed) = if wait {
            if self.handing.is_empty() {
                return None;
            }
            let (id, rx) = self.handing.remove(0);
            (id, rx.recv().unwrap_or_else(|_| gone()))
        } else {
            let mut found = None;
            for (i, (_, rx)) in self.handing.iter().enumerate() {
                match rx.try_recv() {
                    Ok(v) => {
                        found = Some((i, v));
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        found = Some((i, gone()));
                        break;
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }
            let (i, v) = found?;
            (self.handing.remove(i).0, v)
        };
        if !handed.went {
            self.stays(&id);
        }
        Some((id, handed))
    }

    /// The app whose Start again was pressed, once.
    pub fn take_retry(&mut self) -> Option<String> {
        self.retry.take()
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

    /// Whether `id` is on and its file changed under "Keep my OK": Sensitive items wait (#1465).
    pub fn kept_changed(&self, id: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.id == id && e.enabled && kept_changed(&e.view))
    }

    /// Opens the dialog for `id` (only from the user's click).
    /// The opener the header line of `id` would use (More when it asks for more, Header when it
    /// waits for its first or a voided OK), else the Modules page's own Review...: Settings >
    /// Developer's Review... then has the same Allow as the Modules page for that app.
    pub fn review_opener(&self, id: &str) -> Opener {
        let kind = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| header_kind(&e.view, e.enabled, false));
        match kind {
            Some(HeaderKind::More | HeaderKind::MoreBuild | HeaderKind::ChangedKept) => {
                Opener::More
            }
            Some(_) => Opener::Header,
            None => Opener::Review,
        }
    }

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
                let copy = how.then() == Then::TurnOn && self.copy_open(id);
                let starts = self.starts_at_enable(id);
                present_first_run(&mut d, how, copy, starts);
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
        let handover = d.view.handover;
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
                    handover,
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

    /// The apps that wait for their first OK: adopted, never allowed, with something to allow.
    fn first_run_ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| {
                e.view.adopted
                    && e.view.bound.is_none()
                    && e.view.void.is_none()
                    && !e.view.pending.is_empty()
            })
            .map(|e| e.id.clone())
            .collect()
    }

    /// Whether the one first-run header shows: two or more apps wait and the user has not
    /// answered Not now for this very set of lists.
    fn first_run_shown(&self) -> bool {
        let ids = self.first_run_ids();
        ids.len() >= 2 && self.first_run_key(&ids) != self.first_hidden
    }

    fn first_run_key(&self, ids: &[String]) -> Vec<(String, String)> {
        ids.iter()
            .filter_map(|i| self.entries.iter().find(|e| &e.id == i))
            .map(|e| (e.id.clone(), e.view.listing.code().to_string()))
            .collect()
    }

    /// Whether turning the app on launches it (activation `enable`, the default), not only on
    /// demand. Unreadable manifest: true, as before.
    fn starts_at_enable(&self, id: &str) -> bool {
        Registry::open_from_env(&self.home)
            .ok()
            .and_then(|r| r.manifest(id))
            .is_none_or(|m| {
                m.activation_on.is_empty() || m.activation_on.iter().any(|a| a == "enable")
            })
    }

    /// Opens the first-run review (only from the user's click). Each section is the app's own
    /// dialog, checked exactly as the single dialog would check it.
    pub fn open_first_run(&mut self) {
        let ids = self.first_run_ids();
        let sections: Vec<PermDialog> = ids
            .iter()
            .filter_map(|id| build_dialog(&self.home, id, None).ok())
            .map(|mut d| {
                let copy = self.copy_open(&d.id);
                let starts = self.starts_at_enable(&d.id);
                present_first_run(&mut d, Opener::Header, copy, starts);
                d
            })
            .collect();
        if sections.is_empty() {
            return;
        }
        log::info!(
            "permissions: first run: dialog shown for {} apps",
            sections.len()
        );
        self.first_run = Some(FirstRun {
            sections,
            error: None,
        });
        self.first_run_drawn = false;
    }

    /// The answer to the first-run review. Not now grants nothing. Allow records each section
    /// with that section's own list code and counted items, one by one; a section whose list
    /// changed is not granted and stays with the reason, the others are recorded and returned.
    pub fn answer_first_run(&mut self, a: FirstRunAnswer) -> Vec<Done> {
        let FirstRunAnswer::Allow(each) = a else {
            if let Some(f) = self.first_run.take() {
                let ids: Vec<String> = f.sections.iter().map(|d| d.id.clone()).collect();
                self.first_hidden = self.first_run_key(&ids);
            }
            return Vec::new();
        };
        let Some(mut f) = self.first_run.take() else {
            return Vec::new();
        };
        f.error = None;
        let mut done = Vec::new();
        let mut left: Vec<PermDialog> = Vec::new();
        for d in std::mem::take(&mut f.sections) {
            let Some((_, al)) = each.iter().find(|(i, _)| i == &d.id) else {
                left.push(d);
                continue;
            };
            let Allowed {
                code,
                chosen,
                declined,
                keyboard,
                keep,
            } = al.clone();
            self.queue.clear(&self.home, &d.id);
            let r = hideout_supervisor::driver::grant_listed_dialog_in(
                &self.home, &self.hub, &d.id, &code, &chosen, &declined, keep,
            );
            match r {
                Ok(_) => {
                    if d.keyboard.is_some() {
                        self.keyboard(&d.id, keyboard);
                    }
                    done.push(Done {
                        id: d.id,
                        code,
                        chosen,
                        declined,
                        then: Then::TurnOn,
                        handover: d.view.handover,
                    });
                }
                Err(e) if e.contains("list_changed") => {
                    log::warn!(
                        "permissions: {}: not allowed: list changed (shown {code})",
                        d.id
                    );
                    match build_dialog(&self.home, &d.id, Some(Reason::ListChanged)) {
                        Ok(mut n) => {
                            n.mark_changes(&d);
                            left.push(n);
                        }
                        Err(_) => left.push(d),
                    }
                }
                Err(e) => {
                    log::warn!("permissions: {}: allow failed ({e})", d.id);
                    f.error = Some(
                        if e.contains("lock") || e.contains("busy") {
                            BUSY
                        } else {
                            NOT_SAVED
                        }
                        .into(),
                    );
                    left.push(d);
                }
            }
        }
        self.refresh();
        if !left.is_empty() {
            f.sections = left;
            self.first_run = Some(f);
            self.first_run_drawn = false;
        }
        done
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
        // A rebuilt, never-checked app must not get the old OK bound to it by this switch.
        if !keep_can_turn_on(&v) {
            self.refresh();
            return self.note(id, keep_allow_first(&v.listing.name));
        }
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
        // Looked at again: the app may have been rebuilt while the confirm was open.
        let fresh = Registry::open_from_env(&self.home)
            .ok()
            .and_then(|r| r.permission_view(&k.id).ok());
        if !fresh.as_ref().is_some_and(keep_can_turn_on) {
            self.refresh();
            self.note(&k.id, keep_allow_first(&k.name));
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
        self.ctx = Some(ui.ctx().clone());
        let mut open: Option<(String, Opener, egui::Id)> = None;
        let mut hide: Option<(String, HeaderKind, String)> = None;
        let mut off: Option<(String, Option<String>)> = None;
        let mut open_queued: Option<(String, egui::Id)> = None;
        let mut keep_on: Option<(String, egui::Id)> = None;
        let mut keep_stop: Option<String> = None;
        let mut open_first: Option<egui::Id> = None;
        for (id, _) in &self.handing {
            let name = self
                .entries
                .iter()
                .find(|e| &e.id == id)
                .map(|e| e.view.listing.name.clone())
                .unwrap_or_else(|| id.clone());
            hideout_shell::handover::closing_ui(ui, t, &name);
        }
        let mut again = None;
        for n in &self.handover_notes {
            if hideout_shell::handover::note_ui(ui, t, n) {
                again = Some(n.id.clone());
            }
            ui.add_space(8.0);
        }
        if let Some(id) = again {
            self.handover_notes.retain(|n| n.id != id);
            self.retry = Some(id);
        }
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
        let first_ids = if self.first_run_shown() {
            self.first_run_ids()
        } else {
            Vec::new()
        };
        if !first_ids.is_empty() {
            let h = first_run_header(first_ids.len());
            match perm_header_ui(ui, t, &h) {
                Some((true, id)) => open_first = Some(id),
                Some((false, _)) => {
                    self.first_hidden = self.first_run_key(&first_ids);
                }
                None => {}
            }
            ui.add_space(8.0);
        }
        for e in &self.entries {
            if first_ids.contains(&e.id) {
                continue;
            }
            let kind = header_kind(&e.view, e.enabled, false).filter(|k| {
                let code = e.view.listing.code();
                !self
                    .hidden
                    .iter()
                    .any(|(i, h, c)| i == &e.id && h == k && c == code)
            });
            if let Some(k) = kind {
                let name = &e.view.listing.name;
                let cli = if matches!(k, HeaderKind::More | HeaderKind::MoreBuild) {
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
                        let how = if matches!(
                            k,
                            HeaderKind::More | HeaderKind::MoreBuild | HeaderKind::ChangedKept
                        ) {
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
        if let Some(btn) = open_first {
            self.opener_id = Some(btn);
            self.open_first_run();
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
    pub fn dialog(&mut self, ctx: &egui::Context, t: &Tokens) -> Vec<Done> {
        self.ctx = Some(ctx.clone());
        if self.first_run.is_some() {
            let first = !self.first_run_drawn;
            self.first_run_drawn = true;
            let a = self
                .first_run
                .as_mut()
                .and_then(|d| first_run_dialog_ui(ctx, t, d, first));
            let done = a.map(|a| self.answer_first_run(a)).unwrap_or_default();
            if self.first_run.is_none() {
                if let Some(id) = self.opener_id.take() {
                    ctx.memory_mut(|m| m.request_focus(id));
                }
            }
            return done;
        }
        if self.dialog.is_none() {
            return self.keep_dialog(ctx, t).into_iter().collect();
        }
        let first = !self.drawn;
        self.drawn = true;
        let Some(a) = self
            .dialog
            .as_mut()
            .and_then(|d| perm_dialog_ui(ctx, t, d, first))
        else {
            return Vec::new();
        };
        let done = self.answer(a);
        if self.dialog.is_none() {
            if let Some(id) = self.opener_id.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        done.into_iter().collect()
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
        // Off or on, still unanswered: the first-run Ask header (Allow turns the app on).
        assert_eq!(header_kind(&v, false, true), Some(HeaderKind::Ask));
        assert_eq!(header_kind(&v, true, true), Some(HeaderKind::Ask));
        // Turn on opens the dialog, listing every ask; the user's Allow records them all.
        f.ui.open(FX, Opener::TurnOn);
        let d = f.ui.dialog.clone().expect("Turn on opens the dialog");
        assert_eq!(d.items.iter().filter(|i| i.allowable).count(), asked);
        // What Foxglove does itself (HID classes, `net`) is drawn red and starts unchecked: the
        // user ticks each one.
        let red = [
            "device.hid:keyboard",
            "device.hid:lighting",
            "device.hid:vendor",
            "net",
        ];
        for r in red {
            let i = d.items.iter().find(|i| i.raw == r).unwrap();
            assert!(i.sensitive && !i.checked, "{r}");
        }
        let done = {
            f.ui.open(FX, Opener::TurnOn);
            let dlg = f.ui.dialog.as_mut().unwrap();
            for r in red {
                dlg.set_checked(r, true);
                dlg.items.iter_mut().find(|i| i.raw == r).unwrap().confirmed = true;
            }
            let a = dlg.allowed();
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

    /// Slice 2, allow means on: an adopted app that is Off gets the Ask header, its Allow (from
    /// the header) says turn on, Not now says nothing and grants nothing, and a re-Allow of an
    /// app that is already allowed never asks again (no header, nothing pending).
    #[test]
    fn allow_on_an_adopted_off_app_means_turn_on_and_not_now_does_not() {
        let mut f = fx("allowon", &["hub.notify", "ui.window"]);
        let v = view(&f);
        assert!(!Registry::open_from_env(&f.home).unwrap().hub.enabled(ID));
        assert_eq!(header_kind(&v, false, true), Some(HeaderKind::Ask));
        // Not now: nothing is allowed and nothing says turn on.
        f.ui.open(ID, Opener::Header);
        assert!(f.ui.answer(PermAnswer::NotNow).is_none());
        assert!(view(&f).granted.is_empty());
        assert!(f.ui.needs_ok(ID));
        // Allow: recorded, and the page is told to turn it on.
        let done = allow_all(&mut f, Opener::Header).unwrap();
        assert_eq!(done.then, Then::TurnOn);
        assert!(!f.ui.needs_ok(ID));
        let v = view(&f);
        assert_eq!(header_kind(&v, false, true), None);
        // Opened again from Change (the section): records only, never turns on.
        let again = allow_all(&mut f, Opener::Change).unwrap();
        assert_eq!(again.then, Then::Nothing);
    }

    /// A list that changed while the dialog was open grants nothing and says nothing to turn on.
    #[test]
    fn a_refused_allow_does_not_turn_anything_on() {
        let mut f = fx("allowrefused", &["hub.notify", "ui.window"]);
        f.ui.open(ID, Opener::Header);
        let mut a = f.ui.dialog.as_ref().unwrap().allowed();
        a.code = "0".repeat(16);
        assert!(f.ui.answer(PermAnswer::Allow(a)).is_none());
        assert!(view(&f).granted.is_empty());
        assert!(f.ui.needs_ok(ID));
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

    /// The first-run group "I understand" covers the items on screen when it is ticked, and
    /// nothing else: an item added by a changed list comes back unticked and unconfirmed, with
    /// its own "I understand" and no group box.
    #[test]
    fn the_group_i_understand_never_covers_items_added_later() {
        let mut f = fx("grpadded", &["hub.notify", "net"]);
        f.ui.open(ID, Opener::TurnOn);
        let mut shown = f.ui.dialog.clone().unwrap();
        assert_eq!(
            shown.view.group_ack,
            Some(false),
            "fresh first run offers it"
        );
        shown.set_group_ack(true);
        assert!(item(&shown, "net").confirmed);
        // The app now also asks for a second sensitive thing.
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&f.exe, &["hub.notify", "net", "key.events"]),
        )
        .unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
        assert!(f.ui.answer(PermAnswer::Allow(shown.allowed())).is_none());
        assert!(view(&f).granted.is_empty(), "nothing was allowed");
        let d = f.ui.dialog.clone().expect("reopened");
        assert_eq!(d.reason, Some(Reason::ListChanged));
        assert_eq!(d.view.group_ack, None, "no group box on a changed list");
        for raw in ["net", "key.events"] {
            let i = item(&d, raw);
            assert!(i.sensitive && !i.checked && !i.confirmed, "{raw}");
        }
        assert!(item(&d, "key.events").new);
        let chosen = d.allowed().chosen;
        assert!(
            !chosen.iter().any(|r| r == "net" || r == "key.events"),
            "{chosen:?}"
        );
        // Ticking the group box on a dialog without it does nothing.
        let mut d = d;
        d.set_group_ack(true);
        assert_eq!(d.count(), 1);
    }

    /// An update or a move never carries the group "I understand": a rebuild (void) and an
    /// update that asks for more both open with every sensitive item unticked, unconfirmed and
    /// with its own "I understand".
    #[test]
    fn an_update_or_move_never_carries_the_group_i_understand() {
        let mut f = fx("grpgone", &["hub.notify", "net"]);
        f.ui.open(ID, Opener::TurnOn);
        let mut d = f.ui.dialog.clone().unwrap();
        d.set_group_ack(true);
        f.ui.answer(PermAnswer::Allow(d.allowed())).unwrap();
        assert!(view(&f).granted.iter().any(|g| g.raw == "net"));
        // A rebuilt file at the same place, then the same app moved to a new place.
        std::fs::write(&f.exe, b"program v2").unwrap();
        Registry::open_from_env(&f.home)
            .unwrap()
            .void_if_changed(ID, NOW)
            .unwrap();
        f.ui.refresh();
        for how in [Opener::Header, Opener::TurnOn] {
            f.ui.open(ID, how);
            let d = f.ui.dialog.clone().unwrap();
            assert_eq!(d.reason, Some(Reason::Void));
            assert_eq!(d.view.group_ack, None, "{how:?}");
            let net = item(&d, "net");
            assert!(net.sensitive && !net.checked && !net.confirmed, "{how:?}");
            assert!(!d.allowed().chosen.iter().any(|r| r == "net"));
        }
        let mut m = fx("grpmoved", &["hub.notify", "net"]);
        m.ui.open(ID, Opener::TurnOn);
        let mut d = m.ui.dialog.clone().unwrap();
        d.set_group_ack(true);
        m.ui.answer(PermAnswer::Allow(d.allowed())).unwrap();
        // Same app, different path on this PC: its file is new for the grant.
        let moved = m.exe.with_file_name("moved").join("vulpes.exe");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::write(&moved, b"program v1").unwrap();
        std::fs::write(
            m.folder.join("module.toml"),
            manifest(&moved, &["hub.notify", "net"]),
        )
        .unwrap();
        let mut reg = Registry::open(&m.home, false).unwrap();
        reg.register_adopted(&m.folder, NOW).unwrap();
        reg.void_if_changed(ID, NOW).unwrap();
        m.ui.refresh();
        m.ui.open(ID, Opener::TurnOn);
        let d = m.ui.dialog.clone().unwrap();
        assert_eq!(d.reason, Some(Reason::Void), "the move voided the OK");
        assert_eq!(d.view.group_ack, None, "a moved app gets no group box");
        let net = item(&d, "net");
        assert!(!net.checked && !net.confirmed);
        // An update that asks for more: only the new items, with no group box.
        let mut u = updated_with_more("grpmore", &["net"]);
        u.ui.open(ID, Opener::More);
        let d = u.ui.dialog.clone().unwrap();
        assert_eq!(d.reason, Some(Reason::More));
        assert_eq!(d.view.group_ack, None);
        assert!(d.items.iter().filter(|i| i.sensitive).all(|i| !i.confirmed));
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
        assert_eq!(d.allow_label(), "Allow and start");
        assert_eq!(d.all_state(), hideout_shell::permission_dialog::All::Some);
        let done = f.ui.answer(PermAnswer::Allow(d.allowed())).unwrap();
        assert_eq!(done.then, Then::TurnOn);
        let v = view(&f);
        assert!(v.void.is_none());
        assert_eq!(v.granted.len(), 1);
        assert!(!f.ui.needs_ok(ID));
    }

    /// Allows `hub.notify` + `lighting.lease` + sensitive `net` (understood), then changes the
    /// program file so the next launch voids the grant.
    fn allowed_then_rebuilt(tag: &str) -> Fx {
        let mut f = fx(tag, &["hub.notify", "lighting.lease", "net"]);
        f.ui.open(ID, Opener::TurnOn);
        {
            let d = f.ui.dialog.as_mut().unwrap();
            for i in d.items.iter_mut().filter(|i| i.raw == "net") {
                i.checked = true;
                i.confirmed = true;
            }
        }
        let a = f.ui.dialog.as_ref().unwrap().allowed();
        assert_eq!(a.chosen.len(), 3);
        f.ui.answer(PermAnswer::Allow(a)).unwrap();
        std::fs::write(&f.exe, b"program v2").unwrap();
        Registry::open_from_env(&f.home)
            .unwrap()
            .void_if_changed(ID, NOW)
            .unwrap();
        f.ui.refresh();
        f
    }

    fn item<'a>(d: &'a PermDialog, raw: &str) -> &'a ListItem {
        d.items.iter().find(|i| i.raw == raw).unwrap()
    }

    #[test]
    fn a_rebuild_prechecks_only_non_sensitive_items_allowed_before() {
        let mut f = allowed_then_rebuilt("cut4-same");
        assert!(view(&f).same_place);
        f.ui.open(ID, Opener::Header);
        let d = f.ui.dialog.clone().unwrap();
        for raw in ["hub.notify", "lighting.lease"] {
            let i = item(&d, raw);
            assert!(i.checked && i.before && !i.sensitive, "{raw}");
        }
        // Sensitive: unchecked, unconfirmed, not marked, though it was allowed before.
        let net = item(&d, "net");
        assert!(net.sensitive && !net.checked && !net.confirmed && !net.before);
        assert!(d.void_line().contains("file changed since"));
        assert_eq!(d.allow_label(), "Allow 2");
        assert!(d.before_lines()[0].contains("Allowed before"));
        // Allow grants the two, never the sensitive one.
        let a = d.allowed();
        assert!(!a.chosen.contains(&"net".to_string()));
        assert!(a.declined.contains(&"net".to_string()));
    }

    #[test]
    fn a_new_item_after_a_rebuild_starts_unchecked() {
        let mut f = allowed_then_rebuilt("cut4-new");
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(
                &f.exe,
                &["hub.notify", "lighting.lease", "net", "ui.window"],
            ),
        )
        .unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
        f.ui.refresh();
        f.ui.open(ID, Opener::Header);
        let d = f.ui.dialog.clone().unwrap();
        let w = item(&d, "ui.window");
        assert!(!w.checked && !w.before);
        assert!(item(&d, "hub.notify").checked && item(&d, "hub.notify").before);
    }

    #[test]
    fn a_moved_app_gets_the_full_ask_with_nothing_prechecked() {
        let mut f = allowed_then_rebuilt("cut4-moved");
        let moved = f.exe.parent().unwrap().parent().unwrap().join("other");
        std::fs::create_dir_all(&moved).unwrap();
        let exe2 = moved.join("vulpes.exe");
        std::fs::write(&exe2, b"program v2").unwrap();
        std::fs::write(
            f.folder.join("module.toml"),
            manifest(&exe2, &["hub.notify", "lighting.lease", "net"]),
        )
        .unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
        f.ui.refresh();
        assert!(!view(&f).same_place);
        f.ui.open(ID, Opener::Header);
        let d = f.ui.dialog.clone().unwrap();
        assert!(d.items.iter().all(|i| !i.checked && !i.before));
        assert_eq!(d.allow_label(), "Allow none");
        assert!(d.before_lines().is_empty());
    }

    #[test]
    fn a_network_or_other_host_path_is_never_the_same_place() {
        use hideout_registry::permission::reask_same_place as same;
        assert!(!same(
            r"\\server\share\a\vulpes.exe",
            r"\\server\share\a\vulpes.exe"
        ));
        assert!(!same(
            r"\\?\UNC\server\share\vulpes.exe",
            r"\\?\UNC\server\share\vulpes.exe"
        ));
        assert!(!same("", ""));
        assert!(!same(r"C:\a\vulpes.exe", r"C:\b\vulpes.exe"));
        assert!(!same(r"C:\a\vulpes.exe", ""));
    }

    #[test]
    fn not_now_after_a_rebuild_grants_nothing_and_keep_stays_off() {
        let mut f = allowed_then_rebuilt("cut4-cancel");
        f.ui.open(ID, Opener::Header);
        let d = f.ui.dialog.clone().unwrap();
        // Keep is never pre-set, and sending Allow does not turn it on.
        assert_eq!(d.keep, Some(Keep::offered(false)));
        assert!(!d.allowed().keep);
        assert!(f.ui.answer(PermAnswer::NotNow).is_none());
        let v = view(&f);
        assert!(v.granted.is_empty(), "Not now grants nothing");
        assert!(v.void.is_some());
        assert!(f.ui.needs_ok(ID));
    }

    fn rebuilt_dialog(f: &mut Fx) -> PermDialog {
        std::fs::write(&f.exe, b"program v2").unwrap();
        Registry::open_from_env(&f.home)
            .unwrap()
            .void_if_changed(ID, NOW)
            .unwrap();
        f.ui.refresh();
        f.ui.open(ID, Opener::Header);
        f.ui.dialog.clone().unwrap()
    }

    fn with_args(exe: &Path, args: &str) -> String {
        manifest(exe, &["hub.notify", "lighting.lease"]).replace(
            &format!("run = ['{}']", exe.display().to_string().replace('\\', "/")),
            &format!(
                "run = ['{}'{args}]",
                exe.display().to_string().replace('\\', "/")
            ),
        )
    }

    fn readopt(f: &Fx, text: String) {
        std::fs::write(f.folder.join("module.toml"), text).unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&f.folder, NOW)
            .unwrap();
    }

    #[test]
    fn changed_run_args_after_the_void_get_the_full_ask_bound_empty_or_not() {
        // Bound with no args, then the manifest adds one.
        let mut f = fx("cut4-args-empty", &["hub.notify", "lighting.lease"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        readopt(&f, with_args(&f.exe, ", '--new-flag'"));
        let d = rebuilt_dialog(&mut f);
        assert!(!view(&f).same_place);
        assert!(d.items.iter().all(|i| !i.checked && !i.before));
        // Bound with args, then they change.
        let mut f = fx("cut4-args-set", &["hub.notify", "lighting.lease"]);
        readopt(&f, with_args(&f.exe, ", '--a'"));
        allow_all(&mut f, Opener::TurnOn).unwrap();
        readopt(&f, with_args(&f.exe, ", '--b'"));
        let d = rebuilt_dialog(&mut f);
        assert!(d.items.iter().all(|i| !i.checked && !i.before));
    }

    #[test]
    fn an_item_turned_off_before_the_rebuild_is_not_prechecked_or_marked() {
        let mut f = fx("cut4-revoked", &["hub.notify", "lighting.lease"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        f.ui.turn_off(ID, "lighting.lease");
        let d = rebuilt_dialog(&mut f);
        let off = item(&d, "lighting.lease");
        assert!(!off.checked && !off.before);
        assert!(item(&d, "hub.notify").before);
    }

    #[test]
    fn an_unknown_item_kind_counts_as_sensitive_so_it_is_never_prechecked() {
        let (_, _, sensitive) = hideout_registry::permission::describe("mystery.kind:x");
        assert!(sensitive);
        // The pre-check rule needs `!sensitive`, so such an item stays unchecked and unmarked.
    }

    #[test]
    fn another_module_at_the_same_exe_path_inherits_no_earlier_answer() {
        let mut f = fx("cut4-other", &["hub.notify", "lighting.lease"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        // A second module, its own id and folder, adopted with the very same exe.
        let other = f.folder.parent().unwrap().join("manifest2");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("module.toml"),
            manifest(&f.exe, &["hub.notify", "lighting.lease"])
                .replace(&format!("id = \"{ID}\""), "id = \"fennec\""),
        )
        .unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&other, NOW)
            .unwrap();
        f.ui.refresh();
        f.ui.open("fennec", Opener::TurnOn);
        let d = f.ui.dialog.clone().unwrap();
        assert!(d.items.iter().all(|i| !i.before));
        assert!(!view_of(&f, "fennec").same_place);
        assert!(view_of(&f, "fennec").last_choice.is_empty());
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

    /// An app that only starts on demand goes Idle after Allow, so its button is plain "Allow".
    #[test]
    fn an_on_demand_app_says_plain_allow() {
        let mut f = fx_from("ondemand", |exe| {
            format!(
                "{}[activation]
on = [\"open\", \"call\"]
",
                manifest(exe, &["ui.window"])
            )
        });
        f.ui.open(ID, Opener::TurnOn);
        let d = f.ui.dialog.clone().unwrap();
        assert!(!d.view.starts);
        assert_eq!(d.allow_label(), "Allow 1");
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
        assert_eq!(d.allow_label(), "Allow and start");
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

    /// Rebuild R6: a dev-folder rebuild that asks for more reads as a new build, counts the
    /// new things, and says the app runs with what was allowed until then.
    #[test]
    fn a_dev_rebuild_that_asks_for_more_gets_the_new_build_header_and_history_line() {
        let f = updated_with_more("more-dev", &["provide:a.read", "provide:a.set"]);
        let mut v = view(&f);
        // An update by the installer still reads as an update.
        assert_eq!(header_kind(&v, true, true), Some(HeaderKind::More));
        v.bound.as_mut().unwrap().attested.last_mut().unwrap().by =
            hideout_registry::dev_folder::DEV_ATTESTED_BY.into();
        assert_eq!(header_kind(&v, true, true), Some(HeaderKind::MoreBuild));
        // Off: no header, as for an update. A void still wins.
        assert_eq!(header_kind(&v, false, true), None);
        let p = asked_pending(&v);
        let h = header(
            HeaderKind::MoreBuild,
            ID,
            "Vulpes",
            Some((p.len(), &p[0].line)),
        );
        assert_eq!(h.title, "Vulpes has 2 things waiting for your OK");
        assert_eq!(h.body, "It runs with what you allowed until then.");
        assert_eq!(h.review, "Review\u{2026}");
        let one = header(HeaderKind::MoreBuild, ID, "Vulpes", Some((1, "x")));
        assert_eq!(one.title, "Vulpes has 1 thing waiting for your OK");
        assert!(!one.title.contains("new build asks"));
        // The history says what the plan's Details line says.
        let s = section_for(ID, &v).unwrap();
        let last = s.history.last().unwrap();
        assert!(
            last.starts_with("New build on ")
                && last.ends_with("What you allowed stayed; 2 things still wait for you."),
            "{:?}",
            s.history
        );
    }

    /// An item left unanswered earlier plus a plain rebuild (no growth) must not read as growth.
    #[test]
    fn a_dev_rebuild_with_only_leftover_pending_items_does_not_claim_growth() {
        // Allow only `hub.notify`; `ui.window` stays pending (neither chosen nor declined).
        let mut f = fx("leftover-dev", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        let mut reg = Registry::open(&f.home, false).unwrap();
        let _ = &mut reg;
        f.ui.refresh();
        let mut v = view(&f);
        // Leave one item pending, as an earlier partial Allow would have.
        // Leave one item unanswered, as an earlier partial Allow would have.
        let mut left = v.granted.remove(0);
        left.state = ItemState::Asked;
        let item = left.raw.clone();
        v.listing.items.push(left);
        v.pending = vec![item];
        let at = v.bound.as_ref().unwrap().at.clone();
        v.bound
            .as_mut()
            .unwrap()
            .attested
            .push(hideout_registry::inventory::Attestation {
                from_sha256: "a".into(),
                to_sha256: "b".into(),
                from_path: String::new(),
                to_path: String::new(),
                version: "1.0.0".into(),
                at,
                by: hideout_registry::dev_folder::DEV_ATTESTED_BY.into(),
            });
        assert_eq!(header_kind(&v, true, true), Some(HeaderKind::MoreBuild));
        let p = asked_pending(&v);
        assert_eq!(p.len(), 1);
        let h = header(
            HeaderKind::MoreBuild,
            ID,
            "Vulpes",
            Some((p.len(), &p[0].line)),
        );
        assert_eq!(h.title, "Vulpes has 1 thing waiting for your OK");
        assert!(!h.title.contains("new build asks"));
        let s = section_for(ID, &v).unwrap();
        let last = s.history.last().unwrap();
        assert!(last.ends_with("1 thing still waits for you."), "{last}");
        assert!(!last.contains("asks for"), "{last}");
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
            same_place: false,
            view: Default::default(),
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
            before: false,
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

    /// #1465: a changed file under "Keep my OK" keeps the app running, asks again for the
    /// Sensitive items only, and shows them pre-checked as allowed before.
    #[test]
    fn a_changed_kept_build_asks_again_for_sensitive_items_and_keeps_the_rest() {
        let mut f = fx("keep-changed", &["hub.notify", "net"]);
        f.ui.open(ID, Opener::TurnOn);
        let mut d = f.ui.dialog.clone().unwrap();
        for i in d.items.iter_mut().filter(|i| i.allowable) {
            i.checked = true;
            i.confirmed = true;
        }
        let k = d.keep.as_mut().expect("the keep box is offered");
        k.set_on(true);
        k.understood = true;
        assert!(f.ui.answer(PermAnswer::Allow(d.allowed())).is_some());
        assert_eq!(view(&f).granted.len(), 2);
        std::fs::write(&f.exe, b"program v2").unwrap();
        let mut reg = Registry::open(&f.home, false).unwrap();
        assert!(reg.keep_followed(ID, NOW).unwrap());
        let v = view(&f);
        assert!(v.void.is_none());
        assert_eq!(v.granted.len(), 1);
        assert!(v.granted.iter().all(|i| i.raw == "hub.notify"));
        assert!(kept_changed(&v));
        assert_eq!(header_kind(&v, true, true), Some(HeaderKind::ChangedKept));
        // Off: no header, as for any app that is not running.
        assert_eq!(header_kind(&v, false, true), None);
        let h = header(HeaderKind::ChangedKept, ID, "Vulpes", None);
        assert_eq!(h.title, "Vulpes changed since you allowed it");
        // The dialog shows the waiting item pre-checked as allowed before, not yet confirmed.
        let d = build_dialog(&f.home, ID, None).unwrap();
        assert_eq!(d.reason, Some(Reason::Kept));
        let net = d.items.iter().find(|i| i.raw == "net").unwrap();
        assert!(net.checked && net.before && !net.confirmed);
        assert!(
            d.allowed().chosen.is_empty(),
            "nothing is allowed by itself"
        );
    }

    /// #1465: Keep is on but the folder syncs or cannot be read: the row stays On with Turn off
    /// and shows why nothing carries, in place of the helper.
    #[test]
    fn a_kept_row_in_a_synced_folder_says_why_and_keeps_turn_off() {
        use hideout_registry::dev_folder::DevProbes;
        use hideout_registry::drive::SystemDrives;
        use hideout_registry::sync_root::{SyncAnswer, SyncProbe};
        #[derive(Clone, Copy)]
        struct Fake(SyncAnswer);
        impl SyncProbe for Fake {
            fn check(&self, _: &str) -> SyncAnswer {
                self.0
            }
        }
        let mut f = fx("keep-synced", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        allow_keeping(&mut f, Opener::Change, true);
        for (answer, line) in [
            (SyncAnswer::Synced, KEEP_SYNCED),
            (SyncAnswer::Unknown, KEEP_SYNC_UNKNOWN),
        ] {
            let mut reg = Registry::open(&f.home, false).unwrap();
            reg.set_dev_probes(DevProbes {
                drives: std::sync::Arc::new(SystemDrives),
                sync: std::sync::Arc::new(Fake(answer)),
            });
            let v = reg.permission_view(ID).unwrap();
            assert!(keep_is_on(&v));
            let row = section_for(ID, &v).unwrap().keep.unwrap();
            assert_eq!(row.state, KeepState::OnHeld(line.into()));
            assert_eq!(row.word(), "On");
            assert_eq!(
                row.button(),
                Some(hideout_shell::permission_dialog::TURN_OFF)
            );
            assert_eq!(row.helper(), line);
            assert!(row.said().ends_with(line));
        }
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
                && s.history[0].ends_with("Your OK stayed, except Sensitive and keyboard."),
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
        // Not saved yet: no line (the time would be the Allow time).
        assert!(section_for(ID, &v).unwrap().kept_void.is_none());
        reg.void_if_changed(ID, NOW).unwrap();
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

    #[test]
    fn a_rebuilt_app_never_gets_the_old_ok_bound_by_the_switch() {
        let mut f = fx("keep-rebuilt", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        let before = view(&f).bound.unwrap();
        // A new build lands while the choice is off: Turn on refuses and says Allow first.
        std::fs::write(&f.exe, b"program v2 never checked").unwrap();
        f.ui.open_keep_confirm(ID);
        assert!(f.ui.keep_confirm.is_none());
        let name = view(&f).listing.name;
        let note =
            f.ui.entries
                .iter()
                .find(|e| e.id == ID)
                .unwrap()
                .note
                .clone();
        assert_eq!(note, Some(keep_allow_first(&name)));
        assert!(follow_of(&f).is_none());
        assert_eq!(view(&f).bound.unwrap().exe_sha256, before.exe_sha256);
        // And one that lands while the confirm is open: Keep my OK changes nothing.
        let mut g = fx("keep-rebuilt-open", &["hub.notify", "ui.window"]);
        allow_all(&mut g, Opener::TurnOn).unwrap();
        let before = view(&g).bound.unwrap();
        g.ui.open_keep_confirm(ID);
        assert!(g.ui.keep_confirm.is_some());
        std::fs::write(&g.exe, b"program v2 never checked").unwrap();
        g.ui.answer_keep(KeepAnswer::Keep);
        assert!(follow_of(&g).is_none());
        let after = view(&g).bound.unwrap();
        assert_eq!(after.exe_sha256, before.exe_sha256);
        assert!(!after.void || after.exe_sha256 == before.exe_sha256);
    }

    #[test]
    fn the_void_line_needs_a_saved_void_and_tells_moved_from_changed() {
        let mut f = fx("keep-void-line", &["hub.notify", "ui.window"]);
        allow_all(&mut f, Opener::TurnOn).unwrap();
        allow_keeping(&mut f, Opener::Change, true);
        let v = view(&f);
        let line = |v: &View| section_for(ID, v).unwrap().kept_void;
        assert!(line(&v).is_none());
        let mut saved = v.clone();
        saved.bound.as_mut().unwrap().void = true;
        // Void, same place: starts in a new way.
        assert!(line(&saved)
            .unwrap()
            .starts_with("Starts in a new way since "));
        // Void, runs from elsewhere now: moved.
        saved.bound.as_mut().unwrap().exe_path = r"C:\Elsewhereulpes.exe".into();
        assert!(line(&saved).unwrap().starts_with("Moved or removed on "));
        // Not saved (the Allow time would show): no line.
        let mut unsaved = v;
        unsaved.void = Some(hideout_registry::keyboard::VoidReason::Moved);
        assert!(line(&unsaved).is_none());
    }
    const FOX: &str = "fennec";

    /// A second adopted app next to vulpes, in the same home.
    fn add_second(f: &mut Fx, perms: &[&str]) {
        let install = f.exe.parent().unwrap().to_path_buf();
        let exe = install.join("fennec.exe");
        std::fs::write(&exe, b"program v1").unwrap();
        let folder = f.folder.parent().unwrap().join("manifest2");
        std::fs::create_dir_all(&folder).unwrap();
        let text = manifest(&exe, perms)
            .replace("id = \"vulpes\"", "id = \"fennec\"")
            .replace("name = \"Vulpes\"", "name = \"Fennec\"");
        std::fs::write(folder.join("module.toml"), text).unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&folder, NOW)
            .unwrap();
        f.ui.refresh();
    }

    fn two(tag: &str) -> Fx {
        let mut f = fx(tag, &["hub.notify", "net"]);
        add_second(&mut f, &["ui.window", "device.hid:lighting"]);
        f
    }

    #[test]
    fn two_apps_waiting_get_one_header_and_one_dialog_with_a_section_each() {
        let mut f = two("fr-open");
        assert_eq!(f.ui.first_run_ids().len(), 2);
        assert!(f.ui.first_run_shown());
        f.ui.open_first_run();
        let d = f.ui.first_run.clone().unwrap();
        assert_eq!(d.sections.len(), 2);
        assert_eq!(d.title(), "Let 2 apps run in Hideout?");
        // Opening grants nothing.
        assert!(view(&f).bound.is_none() && view_of(&f, FOX).bound.is_none());
    }

    #[test]
    fn one_app_waiting_keeps_its_own_header_and_no_first_run_header() {
        let f = fx("fr-one", &["hub.notify"]);
        assert!(!f.ui.first_run_shown());
    }

    #[test]
    fn first_run_not_now_grants_and_starts_nothing_and_hides_the_header() {
        let mut f = two("fr-notnow");
        f.ui.open_first_run();
        let done = f.ui.answer_first_run(FirstRunAnswer::NotNow);
        assert!(done.is_empty());
        assert!(f.ui.first_run.is_none());
        assert!(view(&f).bound.is_none() && view_of(&f, FOX).bound.is_none());
        assert!(view(&f).granted.is_empty() && view_of(&f, FOX).granted.is_empty());
        assert!(!f.ui.first_run_shown());
    }

    #[test]
    fn first_run_allow_grants_each_section_its_own_counted_items_only() {
        let mut f = two("fr-allow");
        f.ui.open_first_run();
        {
            let d = f.ui.first_run.as_mut().unwrap();
            // Vulpes: its sensitive item is ticked AND understood. Fennec: ticked, not understood.
            let v = d.sections.iter_mut().find(|s| s.id == ID).unwrap();
            v.set_checked("net", true);
            v.items
                .iter_mut()
                .find(|i| i.raw == "net")
                .unwrap()
                .confirmed = true;
            let n = d.sections.iter_mut().find(|s| s.id == FOX).unwrap();
            n.set_checked("device.hid:lighting", true);
        }
        let a = f.ui.first_run.as_ref().unwrap().allowed();
        let done = f.ui.answer_first_run(FirstRunAnswer::Allow(a));
        assert_eq!(done.len(), 2);
        assert!(done.iter().all(|d| d.then == Then::TurnOn));
        let v = view(&f);
        assert!(v.granted.iter().any(|g| g.raw == "net"));
        assert!(v.granted.iter().any(|g| g.raw == "hub.notify"));
        let n = view_of(&f, FOX);
        assert!(n.granted.iter().any(|g| g.raw == "ui.window"));
        // Not understood means not counted, so not granted.
        assert!(!n.granted.iter().any(|g| g.raw == "device.hid:lighting"));
        assert!(f.ui.first_run.is_none());
    }

    #[test]
    fn a_sensitive_item_in_first_run_starts_unchecked_and_unconfirmed() {
        let mut f = two("fr-sens");
        f.ui.open_first_run();
        for s in &f.ui.first_run.as_ref().unwrap().sections {
            for i in s.items.iter().filter(|i| i.sensitive) {
                assert!(!i.checked && !i.confirmed, "{}", i.raw);
            }
        }
    }

    #[test]
    fn a_list_that_changed_in_one_section_is_not_granted_and_the_other_is() {
        let mut f = two("fr-changed");
        f.ui.open_first_run();
        // Fennec's list changes while the dialog is open.
        let folder = f.folder.parent().unwrap().join("manifest2");
        let exe = f.exe.parent().unwrap().join("fennec.exe");
        let text = manifest(
            &exe,
            &["ui.window", "device.hid:lighting", "lighting.lease"],
        )
        .replace("id = \"vulpes\"", "id = \"fennec\"")
        .replace("name = \"Vulpes\"", "name = \"Fennec\"");
        std::fs::write(folder.join("module.toml"), text).unwrap();
        Registry::open(&f.home, false)
            .unwrap()
            .register_adopted(&folder, NOW)
            .unwrap();
        let a = f.ui.first_run.as_ref().unwrap().allowed();
        let done = f.ui.answer_first_run(FirstRunAnswer::Allow(a));
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].id, ID);
        assert!(view_of(&f, FOX).granted.is_empty());
        assert!(view_of(&f, FOX).bound.is_none());
        let d = f.ui.first_run.clone().expect("stays open for the app left");
        assert_eq!(d.sections.len(), 1);
        assert_eq!(d.sections[0].id, FOX);
        assert_eq!(d.sections[0].reason, Some(Reason::ListChanged));
    }

    #[test]
    fn the_first_run_words_avoid_the_banned_ones() {
        let f = two("fr-words");
        let d = FirstRun {
            sections: Vec::new(),
            error: None,
        };
        let h = first_run_header(2);
        let all = [
            h.title,
            h.body,
            d.title(),
            hideout_shell::permission_dialog::FIRST_RUN_INTRO.to_string(),
            f.ui.first_run_ids().len().to_string(),
        ];
        for t in all {
            let t = t.to_lowercase();
            for bad in ["program", "provide", "method", "palette", "look ", "effect"] {
                assert!(!t.contains(bad), "{t} says {bad}");
            }
        }
    }

    #[test]
    fn first_run_allow_label_counts_items_not_apps() {
        let mut f = two("fr-label");
        f.ui.open_first_run();
        let d = f.ui.first_run.as_ref().unwrap();
        assert!(d.allow_label().ends_with(" items"), "{}", d.allow_label());
    }

    #[test]
    fn a_section_that_cannot_be_saved_stays_with_its_error_and_the_other_is_granted() {
        let mut f = two("fr-savefail");
        f.ui.open_first_run();
        // Fennec's save fails (the module is not known any more), vulpes's goes through.
        f.ui.first_run
            .as_mut()
            .unwrap()
            .sections
            .iter_mut()
            .find(|s| s.id == FOX)
            .unwrap()
            .id = "ghost".into();
        let a = f.ui.first_run.as_ref().unwrap().allowed();
        let done = f.ui.answer_first_run(FirstRunAnswer::Allow(a));
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].id, ID);
        assert!(!view(&f).granted.is_empty());
        assert!(view_of(&f, FOX).granted.is_empty());
        let d = f.ui.first_run.clone().expect("stays open");
        assert_eq!(d.sections.len(), 1);
        assert!(d.error.is_some());
    }

    #[test]
    fn one_apps_allow_never_grants_or_returns_another() {
        let mut f = two("fr-isolated");
        f.ui.open_first_run();
        let a: Vec<_> =
            f.ui.first_run
                .as_ref()
                .unwrap()
                .allowed()
                .into_iter()
                .filter(|(i, _)| i == ID)
                .collect();
        let done = f.ui.answer_first_run(FirstRunAnswer::Allow(a));
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].id, ID);
        assert!(view_of(&f, FOX).granted.is_empty() && view_of(&f, FOX).bound.is_none());
        assert_eq!(f.ui.first_run.as_ref().unwrap().sections[0].id, FOX);
    }

    /// A standalone source that says "no copy" for every app, and a copy of `slow` that takes a
    /// moment: no windows, no pipe.
    struct NoCopy;
    impl crate::handover::Standalone for NoCopy {
        fn seen(&self, _: &str) -> crate::handover::Seen {
            crate::handover::Seen::No
        }
        fn ask_to_quit(&self, _: &str) -> bool {
            true
        }
    }

    fn starter_log() -> (Starter, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let l = log.clone();
        (
            std::sync::Arc::new(move |id: &str| {
                l.lock().unwrap().push(id.to_string());
                Ok(())
            }),
            log,
        )
    }

    /// First-run Allow for two apps at once: each has its own slot, so both start (before, the
    /// second was dropped with no note).
    #[test]
    fn two_apps_allowed_together_each_get_their_own_hand_over() {
        let mut f = fx("two-slots", &[]);
        f.ui = PermUi::new(&f.home, f.ui.hub.clone()).with_standalone(std::sync::Arc::new(NoCopy));
        let (st, log) = starter_log();
        assert!(f.ui.start_late_check("app-a", Some(st.clone())));
        assert!(f.ui.start_late_check("app-b", Some(st.clone())));
        // The same app twice is still one slot.
        assert!(f.ui.start_late_check("app-b", Some(st)));
        let mut got = Vec::new();
        while let Some((id, h)) = f.ui.finished_hand_over(true) {
            assert!(h.went && matches!(h.started, Some(Ok(()))), "{id}");
            got.push(id);
        }
        got.sort();
        assert_eq!(got, ["app-a", "app-b"]);
        let mut started = log.lock().unwrap().clone();
        started.sort();
        assert_eq!(started, ["app-a", "app-b"], "both started, once each");
        assert!(f.ui.handover_notes.is_empty());
    }

    /// The worker starts the app itself: it is started although nothing ever draws the page or
    /// reads the result (the user left the Apps page or closed to the tray).
    #[test]
    fn the_app_starts_without_the_page_being_drawn() {
        let mut f = fx("undrawn", &[]);
        f.ui = PermUi::new(&f.home, f.ui.hub.clone()).with_standalone(std::sync::Arc::new(NoCopy));
        let (st, log) = starter_log();
        assert!(f.ui.start_hand_over("app-a", Some(st)));
        let t = std::time::Instant::now();
        while log.lock().unwrap().is_empty() {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(5),
                "never started"
            );
            std::thread::yield_now();
        }
        assert_eq!(log.lock().unwrap().as_slice(), ["app-a"]);
    }

    /// A copy that stays gives one note per app: two apps that waited show two notes.
    #[test]
    fn each_app_whose_copy_stays_gets_its_own_note() {
        struct Stays;
        impl crate::handover::Standalone for Stays {
            fn seen(&self, _: &str) -> crate::handover::Seen {
                crate::handover::Seen::Standalone
            }
            fn ask_to_quit(&self, _: &str) -> bool {
                false
            }
        }
        let mut f = fx("two-notes", &[]);
        f.ui = PermUi::new(&f.home, f.ui.hub.clone()).with_standalone(std::sync::Arc::new(Stays));
        let (st, log) = starter_log();
        f.ui.start_hand_over("app-a", Some(st.clone()));
        f.ui.start_late_check("app-b", Some(st));
        while f.ui.finished_hand_over(true).is_some() {}
        let mut ids: Vec<_> = f.ui.handover_notes.iter().map(|n| n.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, ["app-a", "app-b"]);
        assert!(
            log.lock().unwrap().is_empty(),
            "nothing started beside a copy"
        );
    }

    /// A holder that cannot be verified (`Unsure`) is read as `hand_over` reads it: the late
    /// check starts nothing beside it and the note shows.
    #[test]
    fn the_late_check_refuses_an_unverified_holder() {
        struct Unverified;
        impl crate::handover::Standalone for Unverified {
            fn seen(&self, _: &str) -> crate::handover::Seen {
                crate::handover::Seen::Unsure
            }
            fn ask_to_quit(&self, _: &str) -> bool {
                false
            }
        }
        let mut f = fx("late-unsure", &[]);
        f.ui =
            PermUi::new(&f.home, f.ui.hub.clone()).with_standalone(std::sync::Arc::new(Unverified));
        let (st, log) = starter_log();
        f.ui.start_late_check("app-a", Some(st));
        while f.ui.finished_hand_over(true).is_some() {}
        assert!(
            log.lock().unwrap().is_empty(),
            "started beside an unsure holder"
        );
        assert_eq!(f.ui.handover_notes.len(), 1);
    }
}
