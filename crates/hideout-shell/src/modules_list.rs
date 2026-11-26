// SPDX-License-Identifier: MIT
//! The Modules page's rows (docs/hideout-supervisor-ux.md, column **M**): one row per installed
//! module with its name, version, the state in plain words, one main button for the state and
//! **Open**. Generic: the hub hands in what is installed and reads facts; this file names no
//! module (§2.2).
//!
//! Layout (designer review of #1028, #1029): `[mark] Name  version` over the state words; the
//! red warning mark before a row that needs the user, so the warning doesn't rest on colour;
//! those rows first, then the others A–Z, Off last and muted. In safe mode the banner sits above
//! the rows with **Start normally** (§7.10) and **Find the module that breaks Hideout**. While a
//! search runs (#1033) the shell's bar takes the banner's place, the rows say whether each
//! module is on in the round, and Turn on and Turn off are locked.

use std::collections::HashMap;
use std::time::Instant;

use hideout_panels::state_words;
use hideout_supervisor::words;
use serde_json::Value;

use crate::egui;
use crate::theme::{Rgb, Tokens};

/// An installed module, as the registry has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    /// An `adopted` module (§3): an install the hub registered but did not make. Its row's
    /// version reads "1.2.0 · installed separately" (UX plan #1117: "adopted" is jargon).
    pub adopted: bool,
    /// Developer mode: the module runs from a dev folder and keeps its grants over rebuilds.
    /// Its row's version ends with the **Dev** marker.
    pub dev: bool,
    /// The dev folder it runs from, for the Dev marker's tooltip; None when not known.
    pub dev_folder: Option<String>,
}

/// The marker after an adopted module's version.
pub const ADOPTED: &str = "installed separately";
/// The marker after the version of a module that runs from a dev folder (developer mode).
pub const DEV: &str = "Dev";
/// What a screen reader says for [`DEV`].
pub const DEV_SAID: &str = "dev build";

/// What a row's main button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Does {
    /// The ladder (§7.8): Start again, Start with default settings, Go back, Turn off.
    StartAgain,
    StartDefaults,
    GoBack,
    TurnOff,
    /// Off: Turn on.
    TurnOn,
    /// Needs a newer Hideout: Update Hideout.
    UpdateHub,
    /// Couldn't start: Try again.
    TryAgain,
    /// Withdrawn by its publisher (#1026), on or off: Check for updates.
    CheckUpdates,
    /// Waiting for another module (§7.8): Go to <module>, the root named by
    /// `module.<id>.waiting`.
    GoTo,
    /// Update ready (`update-busy`, §7.6): Force.
    Force,
    /// Update waiting for you (`update-waiting`): Review, which opens the permission dialog
    /// (#1178) on the user's click.
    Review,
    /// A changed app (its file next to Hideout differs from the registered one): Update in
    /// Hideout registers the file again, then the Allow dialog opens.
    UpdateInHideout,
}

/// The row menu's button (its accessible name is "More actions for <module>").
pub const MORE: &str = "More actions";
/// The row menu's one item.
pub const REMOVE_MENU: &str = "Remove from Hideout\u{2026}";
/// The confirm's question, for `name`.
pub fn remove_question(name: &str) -> String {
    format!("Remove {name} from Hideout?")
}
/// What Remove does to a module Hideout only registered (an install made elsewhere).
pub const REMOVE_ADOPTED: &str =
    "Hideout forgets its permissions and stops it. Hideout also deletes any secrets you stored for it. Its own settings and files stay. You can add it again later.";
/// What Remove does to a module Hideout installed itself: its copy in Hideout's own
/// `modules` folder goes too; its settings stay.
pub const REMOVE_OWN: &str = "Hideout forgets its permissions, stops it and deletes the copy Hideout installed. Hideout also deletes any secrets you stored for it. Its settings stay. You can add it again later.";
/// The confirm's buttons.
pub const REMOVE: &str = "Remove";
pub const CANCEL: &str = "Cancel";
/// A row whose Remove waits for the module to stop: not clickable until it is gone.
pub const REMOVING: &str = "Removing\u{2026}";
/// The error under a row whose Remove did not work, for `name`. Never the raw error.
pub fn remove_failed(name: &str) -> String {
    format!("Couldn't remove {name}. It may still be running or a file is in use. Try again.")
}
/// The line under a row whose Turn on did not work after the user's Allow, for `name`. `why` is
/// the supervisor's raw error: it only picks the plain reason and is never shown.
pub fn turn_on_failed(name: &str, why: &str) -> String {
    if why.contains("trust.revoked") || why.contains("withdrawn") {
        format!(
            "Couldn't turn on {name}: its maker withdrew this version. Press {} to look for a new one.",
            words::CHECK_FOR_UPDATES
        )
    } else if why.contains("OK first") {
        format!(
            "Couldn't turn on {name}: it still needs your OK. Press {} to give it.",
            words::named(words::REVIEW_OK)
        )
    } else if why.contains("app-file-unreadable") {
        // The app file is there but could not be read (another program has it open). The
        // user's OK is kept, so the same button is the retry.
        format!(
            "Couldn't turn on {name}: Hideout couldn't read its app file. Try {} again.",
            words::TURN_ON
        )
    } else {
        format!(
            "Couldn't turn on {name}: the app didn't start. Try {} again.",
            words::TURN_ON
        )
    }
}
/// The error's choices: ask again (the confirm shows again), or put the message away.
pub const TRY_AGAIN: &str = "Try again";
pub const CLOSE: &str = "Close";

/// Where a Remove from Hideout stands for one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Stopping, then removed; the row stays, grey and not clickable, until the list drops it.
    Removing,
    /// It did not work: the row stays with the error under it.
    Failed,
}

/// The one-step confirm of Remove from Hideout: the menu only [`RemoveAsk::ask`]s, nothing is
/// removed until [`RemoveAsk::confirm`] answers for the same module.
#[derive(Debug, Default)]
pub struct RemoveAsk {
    asked: Option<String>,
    states: std::collections::BTreeMap<String, Removal>,
    /// A Turn on that did not work, by module id: the plain line for the row.
    turn_on: std::collections::BTreeMap<String, String>,
}

impl RemoveAsk {
    /// The menu item (or Try again): the confirm shows for `id` (replacing another's), and an
    /// earlier error of that row goes.
    pub fn ask(&mut self, id: &str) {
        self.states.remove(id);
        self.asked = Some(id.into());
    }
    /// A Remove that went to the supervisor, or did not work.
    pub fn set(&mut self, id: &str, r: Removal) {
        self.states.insert(id.into(), r);
    }
    /// Puts a row's Remove state away (the error's Close).
    pub fn clear(&mut self, id: &str) {
        self.states.remove(id);
    }
    pub fn state(&self, id: &str) -> Option<Removal> {
        self.states.get(id).copied()
    }
    /// Keeps the states of modules still listed (a removed module's row is gone).
    pub fn retain_listed(&mut self, listed: impl Fn(&str) -> bool) {
        self.states.retain(|id, _| listed(id));
        self.turn_on.retain(|id, _| listed(id));
    }
    /// A Turn on that did not work: the row says so until the next try.
    pub fn set_turn_on_failed(&mut self, id: &str, line: String) {
        self.turn_on.insert(id.into(), line);
    }
    /// Puts a row's Turn on line away (the next try, or it worked).
    pub fn clear_turn_on_failed(&mut self, id: &str) {
        self.turn_on.remove(id);
    }
    pub fn turn_on_line(&self, id: &str) -> Option<&str> {
        self.turn_on.get(id).map(String::as_str)
    }
    /// The module whose confirm shows.
    pub fn pending(&self) -> Option<&str> {
        self.asked.as_deref()
    }
    pub fn cancel(&mut self) {
        self.asked = None;
    }
    /// The confirm's Remove for `id`: `true` once, and only when that module was asked about.
    pub fn confirm(&mut self, id: &str) -> bool {
        if self.asked.as_deref() == Some(id) {
            self.asked = None;
            return true;
        }
        false
    }
}

/// A row's one main button (never two on a row).
#[derive(Clone, Debug, PartialEq)]
pub struct Primary {
    pub label: String,
    pub does: Does,
}

/// One drawn row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub version: String,
    /// What a screen reader says for the version: "version 0.1.0, installed separately" (some
    /// readers say "dot" for the visible middle dot).
    pub version_said: String,
    /// The Dev marker's tooltip and accessible description; None for an app not in a dev folder.
    pub dev_said: Option<String>,
    /// What the shell announces once when a rebuild restart finishes; None otherwise.
    pub rebuilt_said: Option<String>,
    /// What the shell announces once after a turn-off restarted the app to apply it, or
    /// stopped it because it couldn't be restarted; None otherwise. Never with `rebuilt_said`.
    pub restarted_said: Option<String>,
    /// The state in plain words (never an id).
    pub state: String,
    /// It needs the user (stopped, couldn't start or install, needs a newer Hideout): the red
    /// mark before the name, the state in the danger colour, and the top of the list.
    pub stopped: bool,
    /// Off: last in the list, muted.
    pub off: bool,
    /// The row's accessible name: "Art, stopped after 5 crashes".
    pub label: String,
    /// The main button for the state; Open is always there besides.
    pub primary: Option<Primary>,
    /// An install Hideout registered but did not make: Remove only forgets it (the confirm's
    /// words say so).
    pub adopted: bool,
    /// A Remove in progress or failed ([`Removal`]).
    pub removal: Option<Removal>,
    /// A Turn on that did not work: one plain line under the row.
    pub turn_on_note: Option<String>,
    /// A withdrawn row's Check for updates (`module.<id>.check`): the muted line under the
    /// row ("Checking Art's source\u{2026}", nothing newer, couldn't check); None before a check
    /// and once a new version moves the row on.
    pub check_note: Option<String>,
    /// The check is running: the button reads "Checking\u{2026}" and is disabled.
    pub checking: bool,
}

/// What a click on the page asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// **Open** on a row: the module's page.
    Open(String),
    /// A row's main button.
    Primary(String, Does),
    /// The row's **...** menu, **Remove from Hideout...**: only asks; nothing is removed.
    AskRemove(String),
    /// The confirm's **Remove**: the one click that removes.
    Remove(String),
    /// The confirm's **Cancel** (or Escape).
    CancelRemove,
    /// The error under a row: put it away.
    DismissRemove(String),
    /// The safe-mode banner's **Start normally**.
    StartNormally,
    /// The banner's **Find the module that breaks Hideout** (or "Check whether <Module> …").
    FindCulprit,
    /// The banner's **Copy diagnostics** (no module would start: it's Hideout itself).
    CopyDiagnostics,
}

/// The safe-mode banner's link to the search (#1033).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find<'a> {
    /// No search in this hub: the greyed link with "Comes in a later Hideout.".
    Later,
    /// The link's words.
    Link(&'a str),
    /// No module would start: no link; the line says it's Hideout itself, with Copy
    /// diagnostics · Start normally.
    Nothing,
}

/// The safe-mode banner: its line, a line after a search stopped, the search's link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Banner<'a> {
    pub line: &'a str,
    /// "Stopped finding. Nothing changed."
    pub note: Option<&'a str>,
    pub find: Find<'a>,
}

/// What sits above the rows, and whether the rows' Turn on / Turn off are locked (a search).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Top<'a> {
    pub banner: Option<Banner<'a>>,
    pub locked: bool,
    /// The module whose Remove confirm shows under its row ([`RemoveAsk`]).
    pub remove: Option<&'a str>,
    /// Whether this hub can remove a module (a supervisor runs); without one the menu is not
    /// offered.
    pub can_remove: bool,
}

/// A row's words during a search (#1033): `on` is `Some(true)` for a module on in this round,
/// `Some(false)` off for it, `None` not in the search. Rows that need the user keep theirs; an
/// Off row is not part of the search.
pub fn search_words(r: &mut Row, on: Option<bool>) {
    use hideout_supervisor::bisect::words as w;
    if r.stopped {
        return;
    }
    let state = match (r.off, on) {
        (true, _) => w::NOT_IN_SEARCH,
        (false, Some(true)) => w::ON_IN_ROUND,
        (false, Some(false)) => w::OFF_FOR_ROUND,
        (false, None) => return,
    };
    r.state = state.into();
    r.label = words::row_label(&r.name, state);
}

/// The row of an app that is on, whose file changed under "Keep my OK" (#1465): it runs with what
/// carried over, and Review... opens the Allow dialog for the rest. Only a plain On row changes;
/// anything that needs the user more (stopped, off, restarting) keeps its own words.
pub fn kept_changed_words(r: &mut Row) {
    if r.stopped || r.off || r.state != "On" {
        return;
    }
    r.state = words::ON_CHANGED_KEPT.into();
    r.label = words::row_label(&r.name, words::ON_CHANGED_KEPT);
    r.primary = Some(Primary {
        label: words::REVIEW_OK.into(),
        does: Does::Review,
    });
}

/// Turn on, Turn off and Update in Hideout: the buttons a search locks.
fn toggles(d: Does) -> bool {
    matches!(d, Does::TurnOn | Does::TurnOff | Does::UpdateInHideout)
}

/// Seconds left of a `restarting` module's backoff, counted from when its
/// `module.<id>.restart` fact was first seen (as the crash bar counts).
#[derive(Default)]
pub struct Restarts(HashMap<String, (Value, Instant)>);

impl Restarts {
    /// (seconds left, crash number) for the module's restart fact, or `None` without one.
    pub fn left(&mut self, id: &str, fact: Option<&Value>, now: Instant) -> Option<(u64, usize)> {
        let Some(v) = fact.filter(|v| !v.is_null()) else {
            self.0.remove(id);
            return None;
        };
        let e = self.0.entry(id.to_string()).or_insert((v.clone(), now));
        if e.0 != *v {
            *e = (v.clone(), now);
        }
        let delay = v.get("delay_s").and_then(Value::as_u64).unwrap_or(0);
        let k = v.get("crash").and_then(Value::as_u64).unwrap_or(1) as usize;
        let gone = now.saturating_duration_since(e.1).as_secs();
        Some((delay.saturating_sub(gone).min(60), k))
    }
}

/// The row for a module from its facts (`fact` takes the full fact name). `restart` is
/// [`Restarts::left`]'s answer for it. A waiting module's root isn't named without the list
/// ([`row_in`]).
pub fn row(
    m: &Installed,
    fact: &dyn Fn(&str) -> Option<Value>,
    restart: Option<(u64, usize)>,
) -> Row {
    row_in(m, &[], fact, restart)
}

/// [`row`] with the installed list, so "Waiting for <module>" names the root by its name (never
/// its id; a root not in the list reads "Waiting for another module").
pub fn row_in(
    m: &Installed,
    all: &[Installed],
    fact: &dyn Fn(&str) -> Option<Value>,
    restart: Option<(u64, usize)>,
) -> Row {
    row_changed(m, all, fact, restart, false)
}

/// [`row_in`] for an app whose file next to Hideout differs from the registered one
/// (`changed`, from the page's scan; only an installed-separately app can be changed). The
/// changed state wins over On, Off, "Waits for your OK" and the missing-program and
/// other-version sentences, and its one button is **Update in Hideout**. Every other state
/// keeps its own words (withdrawn, safe mode, starting, waiting, updating, quarantined,
/// crashed, couldn't start, needs a newer Hideout). The words are never the store update's.
pub fn row_changed(
    m: &Installed,
    all: &[Installed],
    fact: &dyn Fn(&str) -> Option<Value>,
    restart: Option<(u64, usize)>,
    changed: bool,
) -> Row {
    let changed = changed && m.adopted;
    let get =
        |k: &str| fact(&format!("module.{}.{k}", m.id)).and_then(|v| v.as_str().map(String::from));
    let state = get("state");
    // The version its publisher withdrew, while the router's fact says so (#1026).
    let withdrawn = fact(&format!("module.{}.revoked", m.id))
        .and_then(|v| v.get("version").and_then(Value::as_str).map(String::from));
    let safe_mode = fact("hub.safe_mode").and_then(|v| v.as_bool()) == Some(true);
    let primary = |label: &str, does| {
        Some(Primary {
            label: label.into(),
            does,
        })
    };
    // The adopted failures with their one sentence (missing program, another version).
    let sentence = state.as_deref() == Some("install-failed")
        && matches!(get("failed").as_deref(), Some("missing" | "version"))
        && get("reason").is_some();
    // A restart that applied a turn-off finished (`module.<id>.restarted`): the clock time, or
    // no time when it is not known (the fact is then `true`; never shown as 00:00 or blank).
    let restarted = fact(&format!("module.{}.restarted", m.id))
        .filter(|v| !v.is_null())
        .map(|v| v.as_str().filter(|t| !t.is_empty()).map(String::from));
    let restarted_line = restarted
        .as_ref()
        .map(|at| words::restarted_row(at.as_deref()));
    // A withdrawn row's check, said on the row itself (the same words as the module's page).
    let check = get("check");
    let checking = check.as_deref() == Some("checking");
    let check_button = || {
        let label = if checking {
            words::CHECKING
        } else {
            words::CHECK_FOR_UPDATES
        };
        primary(label, Does::CheckUpdates)
    };
    let (words, stopped, main) = match state.as_deref() {
        Some("revoked") => (
            words::withdrawn_row(withdrawn.as_deref().unwrap_or(&m.version)),
            true,
            check_button(),
        ),
        // Turned off after the withdrawal: no Turn on, Check for updates instead.
        None | Some("off") if !m.enabled && withdrawn.is_some() => (
            words::off_withdrawn(withdrawn.as_deref().unwrap_or(&m.version)),
            false,
            check_button(),
        ),
        // Off but changed: still an Off row; Turn on would only end in a refusal.
        None | Some("running") | Some("off") if changed && !m.enabled => (
            words::off_changed(),
            false,
            primary(words::UPDATE_IN_HIDEOUT, Does::UpdateInHideout),
        ),
        None | Some("running") | Some("off") if !m.enabled => (
            "Off".to_string(),
            false,
            primary(words::TURN_ON, Does::TurnOn),
        ),
        None if safe_mode => ("On \u{b7} not started in safe mode".into(), false, None),
        None | Some("running") if changed => (
            words::CHANGED_SINCE_ADDED.into(),
            false,
            primary(words::UPDATE_IN_HIDEOUT, Does::UpdateInHideout),
        ),
        // A rebuild restart finished (`module.<id>.rebuilt` is the clock time) until the next
        // state change. So for a restart that applied a turn-off; the two never show together.
        None | Some("running") => (
            match (&restarted_line, get("rebuilt").filter(|t| !t.is_empty())) {
                (Some(line), _) => line.clone(),
                (None, Some(at)) => words::rebuilt_row(&at),
                (None, None) => "On".into(),
            },
            false,
            None,
        ),
        Some("starting") => ("Starting\u{2026}".into(), false, None),
        Some("updating") => (words::UPDATING.into(), false, None),
        // Root cause once (§7.8): the root's name and Go to it; never red, never first.
        Some("waiting") => {
            let root = get("waiting")
                .and_then(|r| all.iter().find(|i| i.id == r))
                .map(|i| i.name.clone());
            match root {
                Some(r) => (
                    words::waiting_for(&r),
                    false,
                    primary(&words::go_to(&r), Does::GoTo),
                ),
                None => (state_words("waiting").to_string(), false, None),
            }
        }
        Some("update-busy") => (
            words::UPDATE_READY.into(),
            false,
            primary(words::FORCE, Does::Force),
        ),
        Some("update-waiting") => (
            words::UPDATE_WAITING.into(),
            false,
            primary(words::REVIEW, Does::Review),
        ),
        Some("quarantined") => (
            words::quarantined_title(&m.name),
            true,
            primary(words::START_AGAIN, Does::StartAgain),
        ),
        Some("restarting") => {
            // A restart fact means a crash restart (counted); a planned exit publishes none.
            let text = match restart {
                Some((secs, k)) => words::restarting_row(secs, k),
                None => words::RESTARTING_PLANNED.into(),
            };
            (text, false, None)
        }
        Some("crash-stopped") | Some("crash-looped") => {
            // The ladder's step, worded as the stopped card's main button.
            let step = words::Recovery::from_id(get("recovery").as_deref());
            let previous = get("previous");
            let does = match (step, previous.is_some()) {
                (words::Recovery::StartAgain, _) => Does::StartAgain,
                (words::Recovery::Defaults, _) => Does::StartDefaults,
                (words::Recovery::GoBack, true) => Does::GoBack,
                (words::Recovery::GoBack, false) | (words::Recovery::Last, _) => Does::TurnOff,
            };
            let label = step.button(&m.name, previous.as_deref());
            (words::STOPPED.into(), true, primary(&label, does))
        }
        // Refused (program missing, another version, or only the OK) and changed: the update is
        // what fixes it. Still red and first, the app is not running.
        Some("install-failed")
            if changed && (sentence || get("failed").as_deref() == Some("ok")) =>
        {
            (
                words::CHANGED_SINCE_ADDED.into(),
                true,
                primary(words::UPDATE_IN_HIDEOUT, Does::UpdateInHideout),
            )
        }
        // An adopted module whose program is missing, or that says another version than it was
        // registered as: the supervisor's one sentence (`module.<id>.reason`), not a bare
        // "Couldn't start". Rows that need the user keep Try again, as for any launch failure.
        Some("install-failed") if sentence => (
            get("reason").unwrap_or_default(),
            true,
            primary(words::TRY_AGAIN, Does::TryAgain),
        ),
        // Refused only because the user's OK is missing: not a crash. Review… opens the dialog
        // (a Turn on that waits for the OK), and Allow then starts it.
        Some("install-failed") if get("failed").as_deref() == Some("ok") => (
            words::WAITS_FOR_OK.into(),
            true,
            primary(words::REVIEW_OK, Does::TurnOn),
        ),
        // It was ended to apply a turn-off and couldn't be started again: stopped, never
        // left running as it was allowed before. Start again starts it as it is allowed now.
        Some("install-failed") if get("failed").as_deref() == Some("apply") => (
            words::stopped_to_apply_row(),
            true,
            primary(words::START_AGAIN, Does::StartAgain),
        ),
        // Stopped because a turn-off couldn't be saved: the same row, its own words.
        Some("install-failed") if get("failed").as_deref() == Some("unsaved") => (
            words::stopped_unsaved_row(),
            true,
            primary(words::START_AGAIN, Does::StartAgain),
        ),
        Some("install-failed") if get("failed").as_deref() == Some("launch") => (
            words::COULDNT_START.into(),
            true,
            primary(words::TRY_AGAIN, Does::TryAgain),
        ),
        Some("needs-hub") => (
            state_words("needs-hub").to_string(),
            true,
            primary(words::UPDATE_HIDEOUT, Does::UpdateHub),
        ),
        Some("off") => ("Off".into(), false, primary(words::TURN_ON, Does::TurnOn)),
        Some(s) => (state_words(s).to_string(), s == "install-failed", None),
    };
    let shows_changed = matches!(&main, Some(p) if p.does == Does::UpdateInHideout);
    let checks = matches!(&main, Some(p) if p.does == Does::CheckUpdates);
    let check_note = checks
        .then(|| match check.as_deref() {
            Some("checking") => Some(words::checking_source(&m.name)),
            Some("none") => Some(words::NOTHING_NEWER.to_string()),
            Some("failed") => Some(words::COULDNT_CHECK.to_string()),
            _ => None,
        })
        .flatten();
    Row {
        removal: None,
        turn_on_note: None,
        check_note,
        checking: checks && checking,
        adopted: m.adopted,
        id: m.id.clone(),
        name: m.name.clone(),
        version: {
            let mut v = m.version.clone();
            if m.adopted {
                v = format!("{v} \u{b7} {ADOPTED}");
            }
            if m.dev {
                v = format!("{v} \u{b7} {DEV}");
            }
            v
        },
        dev_said: m.dev.then(|| match &m.dev_folder {
            Some(f) => words::dev_chip_said(f),
            None => words::DEV_CHIP_SAID.to_string(),
        }),
        rebuilt_said: (state.as_deref().is_none_or(|s| s == "running") && m.enabled)
            .then(|| get("rebuilt").filter(|t| !t.is_empty()))
            .flatten()
            .filter(|_| restarted.is_none())
            .map(|_| words::rebuilt_said(&m.name)),
        // Only with its line on the row: what is announced is what can be read.
        restarted_said: if restarted_line.as_ref() == Some(&words) {
            Some(words::restarted_said(&m.name))
        } else if state.as_deref() == Some("install-failed")
            && get("failed").as_deref() == Some("apply")
        {
            Some(words::stopped_to_apply_said(&m.name))
        } else if state.as_deref() == Some("install-failed")
            && get("failed").as_deref() == Some("unsaved")
        {
            Some(words::stopped_unsaved_said(&m.name))
        } else {
            None
        },
        version_said: {
            let mut v = format!("version {}", m.version);
            if m.adopted {
                v = format!("{v}, {ADOPTED}");
            }
            if m.dev {
                v = format!("{v}, {DEV_SAID}");
            }
            v
        },
        // "Art's page couldn't be shown" and the adopted failures' sentences already name the
        // module. An adopted module's name carries the marker, so a screen reader says
        // "Example AI, installed separately, on" (the visible version text is a separate node).
        label: if state.as_deref() == Some("quarantined") || (sentence && !shows_changed) {
            words.clone()
        } else if m.adopted {
            // "Off · changed since you added it" is said with a comma, not "dot".
            let said = if shows_changed {
                words.replacen(" \u{b7} ", ", ", 1)
            } else {
                words.clone()
            };
            words::row_label(&format!("{}, {ADOPTED}", m.name), &said)
        } else {
            words::row_label(&m.name, &words)
        },
        off: words == "Off" || words.starts_with("Off \u{b7}"),
        state: words,
        stopped,
        primary: main,
    }
}

/// The list's order: rows that need the user first, then the others A–Z, Off last. No
/// headings (fewer than about 8 modules).
pub fn order(rows: &mut [Row]) {
    rows.sort_by_cached_key(|r| {
        let group = if r.stopped {
            0
        } else if r.off {
            2
        } else {
            1
        };
        (group, r.name.to_lowercase())
    });
}

/// The page's empty state.
/// The page's name: the heading, the sidebar label, the window title and the palette entry.
pub const TITLE: &str = "Apps";
pub const EMPTY_HEADING: &str = "No apps yet";
pub const EMPTY_BODY: &str = "Apps add pages, tray tiles, status items and commands to \
Hideout. Installed apps will be listed here, with their state and version.";
/// The rows' secondary button.
pub const OPEN: &str = words::OPEN;

fn c(x: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(x.0, x.1, x.2)
}

/// The safe-mode banner (§7.10): "Safe mode" / the line / **Start normally** · Find the module
/// that breaks Hideout. `line` is [`words::safe_mode_line`]'s. Bisect isn't built: its link is
/// greyed with "Comes in a later Hideout." as its tooltip and its accessible description (a
/// screen reader reads it without hovering). Amber, like the tray's dot. True when Start
/// normally was clicked.
pub fn safe_mode_banner(ui: &mut egui::Ui, t: &Tokens, line: &str) -> bool {
    let b = Banner {
        line,
        note: None,
        find: Find::Later,
    };
    banner_ui(ui, t, &b) == Some(Click::StartNormally)
}

/// The safe-mode banner with its search parts (#1033): **Start normally** · the link; with no
/// module to search, **Copy diagnostics** · Start normally. Returns what was clicked.
pub fn banner_ui(ui: &mut egui::Ui, t: &Tokens, b: &Banner) -> Option<Click> {
    use egui::RichText;
    let mut click = None;
    egui::Frame::new()
        .fill(c(crate::theme::warn_fill(t)))
        .stroke(egui::Stroke::new(1.0_f32, c(t.warn)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(words::SAFE_MODE)
                    .size(16.0)
                    .strong()
                    .color(c(t.text)),
            );
            ui.add_space(2.0);
            let line = match b.find {
                Find::Nothing => hideout_supervisor::bisect::words::NONE_ON,
                _ => b.line,
            };
            ui.label(RichText::new(line).color(c(t.text)));
            if let Some(note) = b.note {
                ui.label(RichText::new(note).color(c(t.text)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let primary = |ui: &mut egui::Ui, text: &str| {
                    let b = egui::Button::new(RichText::new(text).color(c(t.on_accent)))
                        .fill(c(t.accent));
                    ui.add(b).clicked()
                };
                let dot = |ui: &mut egui::Ui| {
                    ui.label(RichText::new("\u{b7}").color(c(t.text_muted)));
                };
                match b.find {
                    Find::Nothing => {
                        if primary(ui, words::COPY_DIAGNOSTICS) {
                            click = Some(Click::CopyDiagnostics);
                        }
                        dot(ui);
                        let link = RichText::new(words::START_NORMALLY).color(c(t.text));
                        if ui.link(link).clicked() {
                            click = Some(Click::StartNormally);
                        }
                    }
                    Find::Link(words_) => {
                        if primary(ui, words::START_NORMALLY) {
                            click = Some(Click::StartNormally);
                        }
                        dot(ui);
                        if ui.link(RichText::new(words_).color(c(t.text))).clicked() {
                            click = Some(Click::FindCulprit);
                        }
                    }
                    Find::Later => {
                        if primary(ui, words::START_NORMALLY) {
                            click = Some(Click::StartNormally);
                        }
                        dot(ui);
                        let later = ui
                            .add_enabled(
                                false,
                                egui::Link::new(
                                    RichText::new(words::FIND_CULPRIT).color(c(t.text_muted)),
                                ),
                            )
                            .on_disabled_hover_text(words::COMES_LATER);
                        ui.ctx().accesskit_node_builder(later.id, |n| {
                            n.set_description(words::COMES_LATER);
                        });
                    }
                }
            });
        });
    ui.add_space(16.0);
    click
}

/// Draws the page: title, the safe-mode banner with its line when `banner` is set, then the rows
/// (or the empty state). Returns what was clicked.
pub fn page_ui(ui: &mut egui::Ui, t: &Tokens, rows: &[Row], banner: Option<&str>) -> Option<Click> {
    let top = Top {
        banner: banner.map(|line| Banner {
            line,
            note: None,
            find: Find::Later,
        }),
        locked: false,
        remove: None,
        can_remove: true,
    };
    page_ui_with(ui, t, rows, top)
}

/// [`page_ui_with_head`] with nothing above the banner.
pub fn page_ui_with(ui: &mut egui::Ui, t: &Tokens, rows: &[Row], top: Top) -> Option<Click> {
    page_ui_with_head(ui, t, rows, top, &mut |_| {})
}

/// [`page_ui`] with the banner's search parts and, during a search, Turn on and Turn off
/// locked: they keep the focus, say the lock line as their description and do nothing, and
/// the line shows under the list (#1033).
/// `head` draws between the title and the banner (the keyboard grant's header line and
/// Permissions block, #1155): above the list, so a long list can't push it off the screen.
pub fn page_ui_with_head(
    ui: &mut egui::Ui,
    t: &Tokens,
    rows: &[Row],
    top: Top,
    head: &mut dyn FnMut(&mut egui::Ui),
) -> Option<Click> {
    use egui::RichText;
    let lock_line = hideout_supervisor::bisect::words::LOCKED;
    ui.label(RichText::new(TITLE).size(24.0).strong().color(c(t.text)));
    ui.add_space(20.0);
    head(ui);
    let mut click = None;
    if let Some(b) = &top.banner {
        click = banner_ui(ui, t, b);
    }
    if rows.is_empty() {
        ui.label(RichText::new(EMPTY_HEADING).size(17.0).color(c(t.text)));
        ui.add_space(4.0);
        ui.label(RichText::new(EMPTY_BODY).color(c(t.text_muted)));
        return click;
    }
    for r in rows {
        let (name_col, state_col) = match (r.stopped, r.off) {
            (true, _) => (t.text, t.danger),
            (false, true) => (t.text_muted, t.text_muted),
            (false, false) => (t.text, t.text_muted),
        };
        ui.push_id(("hub.modules.row", &r.id), |ui| {
            egui::Frame::new()
                .fill(c(t.surface))
                .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if r.stopped {
                                    ui.label(
                                        RichText::new("\u{26A0}").size(15.0).color(c(t.danger)),
                                    );
                                }
                                let name = ui.label(
                                    RichText::new(&r.name)
                                        .size(15.0)
                                        .strong()
                                        .color(c(name_col)),
                                );
                                let said = r.label.clone();
                                ui.ctx()
                                    .accesskit_node_builder(name.id, |n| n.set_label(said));
                                let version =
                                    ui.label(RichText::new(&r.version).color(c(t.text_muted)));
                                let said = r.version_said.clone();
                                let desc = r.dev_said.clone();
                                ui.ctx().accesskit_node_builder(version.id, |n| {
                                    n.set_label(said);
                                    if let Some(d) = desc {
                                        n.set_description(d);
                                    }
                                });
                                if let Some(tip) = &r.dev_said {
                                    version.on_hover_text(tip);
                                }
                            });
                            if r.removal == Some(Removal::Removing) {
                                ui.label(RichText::new(REMOVING).color(c(t.text_muted)));
                            } else {
                                // The visible words are the accessible label; a finished rebuild
                                // restart is told once through the shell's announcement.
                                ui.label(RichText::new(&r.state).color(c(state_col)));
                            }
                        });
                        let busy = r.removal == Some(Removal::Removing);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if busy {
                                return;
                            }
                            // No menu in a hub that can't remove (no supervisor).
                            if top.can_remove {
                                let said = format!("{MORE} for {}", r.name);
                                let more =
                                    ui.menu_button(RichText::new("\u{2026}").size(16.0), |ui| {
                                        // A search locks it like Turn on / Turn off.
                                        if ui
                                            .add_enabled(
                                                !top.locked,
                                                egui::Button::new(REMOVE_MENU),
                                            )
                                            .clicked()
                                        {
                                            click = Some(Click::AskRemove(r.id.clone()));
                                            ui.close();
                                        }
                                    });
                                more.response.widget_info(move || {
                                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &said)
                                });
                            }
                            let b = ui.button(OPEN).on_hover_text(words::open_module(&r.name));
                            if b.clicked() {
                                click = Some(Click::Open(r.id.clone()));
                            }
                            if let Some(p) = &r.primary {
                                let locked = top.locked && toggles(p.does);
                                let (fg, bg) = if locked || r.checking {
                                    (t.text_muted, t.surface)
                                } else {
                                    (t.on_accent, t.accent)
                                };
                                let b = egui::Button::new(RichText::new(&p.label).color(c(fg)))
                                    .fill(c(bg));
                                // A running check: the button waits, the line under the row
                                // says what is happening.
                                let resp = ui.add_enabled(!r.checking, b);
                                if locked {
                                    // Focusable, so its description is read (#1033 follow-up 5).
                                    ui.ctx().accesskit_node_builder(resp.id, |n| {
                                        n.set_description(lock_line);
                                    });
                                } else if resp.clicked() {
                                    click = Some(Click::Primary(r.id.clone(), p.does));
                                }
                            }
                        });
                    });
                    if top.remove == Some(r.id.as_str()) {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.label(
                            RichText::new(remove_question(&r.name))
                                .strong()
                                .color(c(t.text)),
                        );
                        let line = if r.adopted {
                            REMOVE_ADOPTED
                        } else {
                            REMOVE_OWN
                        };
                        ui.label(RichText::new(line).color(c(t.text_muted)));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            // A search that started after the ask locks it too.
                            let go = ui.add_enabled(
                                !top.locked,
                                egui::Button::new(RichText::new(REMOVE).color(c(t.on_accent)))
                                    .fill(c(t.danger)),
                            );
                            if go.clicked() {
                                click = Some(Click::Remove(r.id.clone()));
                            }
                            let cancel = ui.button(CANCEL);
                            // The safe choice has the focus when the confirm appears.
                            let seen = egui::Id::new("hub.modules.remove.focus");
                            let first = ui.data_mut(|d| {
                                let was = d.get_temp::<String>(seen);
                                d.insert_temp(seen, r.id.clone());
                                was.as_deref() != Some(r.id.as_str())
                            });
                            if first {
                                cancel.request_focus();
                            }
                            // Escape is taken here, so it closes nothing else; the window's
                            // close button cancels too.
                            if cancel.clicked() || crate::dismiss::dismissed_taking_esc(ui.ctx()) {
                                click = Some(Click::CancelRemove);
                            }
                        });
                    }
                    if let Some(line) = &r.turn_on_note {
                        ui.add_space(8.0);
                        ui.label(RichText::new(line).color(c(t.danger)));
                    }
                    // The check's result, a polite live region so a screen reader hears it
                    // without the focus moving.
                    if let Some(line) = &r.check_note {
                        ui.add_space(8.0);
                        let note = ui.label(RichText::new(line).color(c(t.text_muted)));
                        let said = line.clone();
                        ui.ctx().accesskit_node_builder(note.id, |n| {
                            n.set_live(egui::accesskit::Live::Polite);
                            n.set_label(said);
                        });
                    }
                    if r.removal == Some(Removal::Failed) {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.label(RichText::new(remove_failed(&r.name)).color(c(t.danger)));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button(TRY_AGAIN).clicked() {
                                click = Some(Click::AskRemove(r.id.clone()));
                            }
                            if ui.button(CLOSE).clicked() {
                                click = Some(Click::DismissRemove(r.id.clone()));
                            }
                        });
                    }
                });
        });
        ui.add_space(8.0);
    }
    if top.remove.is_none() {
        ui.data_mut(|d| d.remove::<String>(egui::Id::new("hub.modules.remove.focus")));
    }
    if top.locked {
        ui.label(RichText::new(lock_line).color(c(t.text_muted)));
    }
    click
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_turn_on_has_one_plain_line_and_never_the_raw_error() {
        let line = turn_on_failed("Art", "trust.revoked: a@1 was withdrawn");
        assert_eq!(
            line,
            "Couldn't turn on Art: its maker withdrew this version. Press Check for updates to look for a new one."
        );
        let other = turn_on_failed("Art", "Io(Os { code: 5 })");
        assert_eq!(
            other,
            "Couldn't turn on Art: the app didn't start. Try Turn on again."
        );
        assert!(!other.contains("Os"));
        // A button named in a sentence has no ellipsis.
        assert_eq!(
            turn_on_failed(
                "Art",
                "Hideout needs your OK first. Press Turn on\u{2026} to review."
            ),
            "Couldn't turn on Art: it still needs your OK. Press Review to give it."
        );
        assert_eq!(
            turn_on_failed("Art", "app-file-unreadable: the app file could not be read"),
            "Couldn't turn on Art: Hideout couldn't read its app file. Try Turn on again."
        );
        let mut a = RemoveAsk::default();
        a.set_turn_on_failed("m", other.clone());
        assert_eq!(a.turn_on_line("m"), Some(other.as_str()));
        a.clear_turn_on_failed("m");
        assert_eq!(a.turn_on_line("m"), None);
    }

    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn m(id: &str, enabled: bool) -> Installed {
        Installed {
            adopted: false,
            dev: false,
            dev_folder: None,
            id: id.into(),
            name: "Art".into(),
            version: "0.5.0".into(),
            enabled,
        }
    }

    fn with(facts: &[(&str, Value)]) -> impl Fn(&str) -> Option<Value> {
        let f: HashMap<String, Value> = facts
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        move |k| f.get(k).cloned()
    }

    type Case<'a> = (
        bool,
        Vec<(&'a str, Value)>,
        Option<(u64, usize)>,
        &'a str,
        bool,
    );

    #[test]
    fn each_state_reads_the_row_words_and_never_an_id() {
        let cases: Vec<Case> = vec![
            (true, vec![], None, "On", false),
            (
                true,
                vec![("module.m.art.state", json!("running"))],
                None,
                "On",
                false,
            ),
            (false, vec![], None, "Off", false),
            (
                false,
                vec![("module.m.art.state", json!("off"))],
                None,
                "Off",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("starting"))],
                None,
                "Starting\u{2026}",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("restarting"))],
                Some((4, 2)),
                "Restarting in 4 s, try 3 of 5",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("restarting"))],
                None,
                "Restarting\u{2026}",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("restarting"))],
                Some((0, 2)),
                "Restarting, try 3 of 5",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("crash-stopped"))],
                None,
                "Stopped after 5 crashes",
                true,
            ),
            (
                true,
                vec![
                    ("module.m.art.state", json!("install-failed")),
                    ("module.m.art.failed", json!("launch")),
                ],
                None,
                "Couldn't start",
                true,
            ),
            (
                true,
                vec![
                    ("module.m.art.state", json!("install-failed")),
                    ("module.m.art.failed", json!("ok")),
                ],
                None,
                "Waits for your OK",
                true,
            ),
            (
                true,
                vec![("module.m.art.state", json!("install-failed"))],
                None,
                "Couldn't install or update",
                true,
            ),
            (
                true,
                vec![("module.m.art.state", json!("needs-hub"))],
                None,
                "Needs a newer Hideout",
                true,
            ),
            (
                true,
                vec![("hub.safe_mode", json!(true))],
                None,
                "On \u{b7} not started in safe mode",
                false,
            ),
        ];
        for (enabled, facts, restart, want, stopped) in cases {
            let r = row(&m("m.art", enabled), &with(&facts), restart);
            assert_eq!((r.state.as_str(), r.stopped), (want, stopped), "{facts:?}");
            let mut drawn = vec![r.state.clone(), r.label.clone()];
            drawn.extend(r.primary.iter().map(|p| p.label.clone()));
            for s in &drawn {
                for never in words::NEVER_SHOWN {
                    assert!(!s.contains(never), "{never} in {s}");
                }
                assert!(!s.contains("m.art"), "{s}");
            }
        }
    }

    type ButtonCase<'a> = (bool, Vec<(&'a str, Value)>, Option<(&'a str, Does)>);

    /// One main button per state (designer review of #1028): the ladder's wording for a
    /// stopped row, Turn on, Update Hideout, Try again; none for the other states.
    #[test]
    fn each_state_has_at_most_one_main_button() {
        let stopped = |extra: Vec<(&'static str, Value)>| {
            let mut f = vec![("module.m.art.state", json!("crash-stopped"))];
            f.extend(extra);
            f
        };
        let cases: Vec<ButtonCase> = vec![
            (
                true,
                stopped(vec![]),
                Some(("Start again", Does::StartAgain)),
            ),
            (
                true,
                stopped(vec![("module.m.art.recovery", json!("defaults"))]),
                Some(("Start with default settings", Does::StartDefaults)),
            ),
            (
                true,
                stopped(vec![
                    ("module.m.art.recovery", json!("go-back")),
                    ("module.m.art.previous", json!("0.4.2")),
                ]),
                Some(("Go back to 0.4.2", Does::GoBack)),
            ),
            (
                true,
                stopped(vec![("module.m.art.recovery", json!("last"))]),
                Some(("Turn off Art", Does::TurnOff)),
            ),
            (false, vec![], Some(("Turn on", Does::TurnOn))),
            (
                true,
                vec![("module.m.art.state", json!("off"))],
                Some(("Turn on", Does::TurnOn)),
            ),
            (
                true,
                vec![("module.m.art.state", json!("needs-hub"))],
                Some(("Update Hideout", Does::UpdateHub)),
            ),
            (
                true,
                vec![
                    ("module.m.art.state", json!("install-failed")),
                    ("module.m.art.failed", json!("launch")),
                ],
                Some(("Try again", Does::TryAgain)),
            ),
            (
                true,
                vec![
                    ("module.m.art.state", json!("install-failed")),
                    ("module.m.art.failed", json!("ok")),
                ],
                Some((words::REVIEW_OK, Does::TurnOn)),
            ),
            (
                true,
                vec![("module.m.art.state", json!("install-failed"))],
                None,
            ),
            (true, vec![], None),
            (true, vec![("module.m.art.state", json!("starting"))], None),
            (
                true,
                vec![("module.m.art.state", json!("restarting"))],
                None,
            ),
            (true, vec![("hub.safe_mode", json!(true))], None),
        ];
        for (enabled, facts, want) in cases {
            let r = row(&m("m.art", enabled), &with(&facts), None);
            let got = r.primary.as_ref().map(|p| (p.label.as_str(), p.does));
            assert_eq!(got, want, "{facts:?}");
        }
        let r = row(&m("m.art", true), &with(&stopped(vec![])), None);
        assert_eq!(r.label, "Art, stopped after 5 crashes");
    }

    /// #1465: an On row whose file changed under "Keep my OK" says so and offers Review...;
    /// a row that is off or stopped keeps its own words.
    #[test]
    fn a_kept_changed_row_says_so_and_offers_review() {
        let on = with(&[("module.m.art.state", json!("running"))]);
        let mut r = row(&m("m.art", true), &on, None);
        assert_eq!(r.state, "On");
        kept_changed_words(&mut r);
        assert_eq!(r.state, "On \u{b7} changed since you allowed it");
        assert_eq!(r.label, "Art, on \u{b7} changed since you allowed it");
        let p = r.primary.unwrap();
        assert_eq!((p.label.as_str(), p.does), ("Review\u{2026}", Does::Review));
        let mut off = row(&m("m.art", false), &on, None);
        kept_changed_words(&mut off);
        assert_eq!(off.state, "Off");
    }

    /// Rows that need the user first, then A–Z, Off last.
    /// Check for updates on a withdrawn row says its result under the row (on or turned off):
    /// the button waits while the check runs, then nothing newer or couldn't check; a row
    /// that is not withdrawn never shows a check line.
    #[test]
    fn a_withdrawn_rows_check_shows_under_the_row() {
        let m = |enabled: bool| Installed {
            adopted: false,
            dev: false,
            dev_folder: None,
            id: "m.a".into(),
            name: "Art".into(),
            version: "0.5.0".into(),
            enabled,
        };
        let row_with = |enabled: bool, state: &'static str, check: Option<&'static str>| {
            row(
                &m(enabled),
                &|k: &str| match k {
                    "module.m.a.state" => Some(json!(state)),
                    "module.m.a.revoked" => Some(json!({"version": "0.5.0", "reason": "x"})),
                    "module.m.a.check" => check.map(|c| json!(c)),
                    _ => None,
                },
                None,
            )
        };
        for (enabled, state) in [(true, "revoked"), (false, "off")] {
            let r = row_with(enabled, state, None);
            assert_eq!((r.check_note.as_deref(), r.checking), (None, false));
            assert_eq!(r.primary.unwrap().label, "Check for updates");
            let r = row_with(enabled, state, Some("checking"));
            assert_eq!(
                r.check_note.as_deref(),
                Some("Checking Art's source\u{2026}")
            );
            assert!(r.checking);
            let p = r.primary.unwrap();
            assert_eq!(
                (p.label.as_str(), p.does),
                ("Checking\u{2026}", Does::CheckUpdates)
            );
            let r = row_with(enabled, state, Some("none"));
            assert_eq!(
                r.check_note.as_deref(),
                Some("No new version yet. Checked just now.")
            );
            assert!(!r.checking);
            assert_eq!(r.primary.unwrap().label, "Check for updates");
            let r = row_with(enabled, state, Some("failed"));
            assert_eq!(
                r.check_note.as_deref(),
                Some("Couldn't check. Try again later.")
            );
            assert!(!r.checking);
        }
        // Not withdrawn any more (the new version arrived): no line, whatever the fact says.
        let r = row(
            &m(true),
            &|k: &str| match k {
                "module.m.a.state" => Some(json!("running")),
                "module.m.a.check" => Some(json!("none")),
                _ => None,
            },
            None,
        );
        assert_eq!((r.check_note, r.checking), (None, false));
    }

    #[test]
    fn needs_you_first_then_a_to_z_then_off() {
        let mk = |name: &str, stopped: bool, off: bool| Row {
            id: name.to_lowercase(),
            name: name.into(),
            version: "1".into(),
            version_said: String::new(),
            dev_said: None,
            rebuilt_said: None,
            restarted_said: None,
            state: String::new(),
            stopped,
            off,
            label: String::new(),
            primary: None,
            adopted: false,
            removal: None,
            turn_on_note: None,
            check_note: None,
            checking: false,
        };
        let mut rows = vec![
            mk("Beat", false, false),
            mk("Alpha", false, true),
            mk("Zed", true, false),
            mk("art", false, false),
            mk("Dock", true, false),
        ];
        order(&mut rows);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Dock", "Zed", "art", "Beat", "Alpha"]);
    }

    /// The banner's words and the tray item's are the UX plan's, and none is an id.
    #[test]
    fn safe_mode_words_never_show_an_id() {
        for s in [
            words::SAFE_MODE,
            words::SAFE_MODE_LINE,
            words::SAFE_MODE_LINE_SET,
            words::START_NORMALLY,
            words::FIND_CULPRIT,
            words::COMES_LATER,
            crate::tray::START_NORMALLY_LABEL,
        ] {
            for never in words::NEVER_SHOWN {
                assert!(!s.to_lowercase().contains(&never.to_lowercase()), "{s}");
            }
        }
        assert_eq!(crate::tray::START_NORMALLY_LABEL, "Start normally");
    }

    #[test]
    fn the_restart_countdown_runs_from_when_the_fact_was_seen() {
        let mut r = Restarts::default();
        let t0 = Instant::now();
        let f = json!({"delay_s": 8, "crash": 2, "of": 5});
        assert_eq!(r.left("m", Some(&f), t0), Some((8, 2)));
        assert_eq!(
            r.left("m", Some(&f), t0 + Duration::from_millis(2500)),
            Some((6, 2))
        );
        assert_eq!(
            r.left("m", Some(&f), t0 + Duration::from_secs(20)),
            Some((0, 2))
        );
        let g = json!({"delay_s": 16, "crash": 3, "of": 5});
        assert_eq!(
            r.left("m", Some(&g), t0 + Duration::from_secs(21)),
            Some((16, 3))
        );
        assert_eq!(r.left("m", None, t0), None);
    }

    /// A module that runs from a dev folder (developer mode) shows the Dev marker after its
    /// version, and says "dev build" to a screen reader.
    #[test]
    fn a_dev_folder_module_shows_the_dev_marker() {
        let fact = |_: &str| None;
        let mut a = m("vulpes", true);
        a.adopted = true;
        a.dev = true;
        let r = row(&a, &fact, None);
        assert_eq!(r.version, "0.5.0 \u{b7} installed separately \u{b7} Dev");
        assert_eq!(
            r.version_said,
            "version 0.5.0, installed separately, dev build"
        );
        a.adopted = false;
        assert_eq!(row(&a, &fact, None).version, "0.5.0 \u{b7} Dev");
    }

    /// The Dev marker's tooltip and accessible description name the folder.
    #[test]
    fn the_dev_marker_says_where_the_build_comes_from() {
        let fact = |_: &str| None;
        let mut a = m("vulpes", true);
        assert_eq!(row(&a, &fact, None).dev_said, None);
        a.dev = true;
        assert_eq!(
            row(&a, &fact, None).dev_said.as_deref(),
            Some("Dev build. Rebuilds keep what you allowed.")
        );
        a.dev_folder = Some("D:\\dev\\out".into());
        assert_eq!(
            row(&a, &fact, None).dev_said.as_deref(),
            Some("Dev build from D:\\dev\\out. Rebuilds keep what you allowed.")
        );
    }

    /// After a rebuild restart the row says when, and a screen reader hears it once; any other
    /// state drops the line.
    #[test]
    fn a_rebuild_restart_shows_its_time_and_is_told_to_a_screen_reader() {
        let a = m("vulpes", true);
        let id = format!("module.{}.", a.id);
        let with = |state: &'static str| {
            let id = id.clone();
            move |k: &str| -> Option<Value> {
                match k.strip_prefix(&id)? {
                    "state" => Some(Value::String(state.into())),
                    "rebuilt" => Some(Value::String("20:41".into())),
                    _ => None,
                }
            }
        };
        let r = row(&a, &with("running"), None);
        assert_eq!(r.state, "On \u{b7} Rebuilt and restarted at 20:41");
        assert_eq!(
            r.rebuilt_said.as_deref(),
            Some(format!("{} rebuilt and restarted", a.name).as_str())
        );
        assert!(!r.off && !r.stopped);
        let r = row(&a, &with("restarting"), None);
        assert_eq!(r.state, "Restarting\u{2026}");
        assert_eq!(r.rebuilt_said, None);
        assert_eq!(row(&a, &|_: &str| None, None).state, "On");
    }

    /// #1451: after a restart that applied a turn-off the row says so, with the time when it
    /// is known and without one when it is not (checklist 2: never 00:00 or blank); a rebuild
    /// line never shows with it (6); any other state drops the line.
    #[test]
    fn a_restart_to_apply_shows_its_line_and_is_told_to_a_screen_reader() {
        let a = m("vulpes", true);
        let id = format!("module.{}.", a.id);
        let with = |state: &'static str, at: Value, rebuilt: bool| {
            let id = id.clone();
            move |k: &str| -> Option<Value> {
                match k.strip_prefix(&id)? {
                    "state" => Some(Value::String(state.into())),
                    "restarted" => Some(at.clone()),
                    "rebuilt" if rebuilt => Some(Value::String("20:30".into())),
                    _ => None,
                }
            }
        };
        let said = format!("{} restarted to apply your change", a.name);
        let r = row(&a, &with("running", "20:41".into(), false), None);
        assert_eq!(r.state, "On \u{b7} Restarted at 20:41 to apply your change");
        assert_eq!(r.restarted_said.as_deref(), Some(said.as_str()));
        assert!(!r.off && !r.stopped && r.primary.is_none());
        // Checklist 7: the accessible label is the visible line, after the app's name.
        assert_eq!(r.label, words::row_label(&a.name, &r.state));
        // The time is not known: the line names none.
        for unknown in [Value::Bool(true), Value::String(String::new())] {
            let r = row(&a, &with("running", unknown, false), None);
            assert_eq!(r.state, "On \u{b7} Restarted to apply your change");
            assert!(!r.state.contains("00:00") && !r.state.contains("at "));
            assert_eq!(r.restarted_said.as_deref(), Some(said.as_str()));
            assert_eq!(r.label, words::row_label(&a.name, &r.state));
        }
        // Both facts set: one line, the restart's, and one announcement.
        let r = row(&a, &with("running", "20:41".into(), true), None);
        assert_eq!(r.state, "On \u{b7} Restarted at 20:41 to apply your change");
        assert_eq!(r.rebuilt_said, None);
        assert!(r.restarted_said.is_some());
        // Not while it restarts, and not once the fact is gone.
        let r = row(&a, &with("restarting", "20:41".into(), false), None);
        assert_eq!(r.state, "Restarting\u{2026}");
        assert_eq!(r.restarted_said, None);
        let r = row(&a, &with("running", Value::Null, false), None);
        assert_eq!(r.state, "On");
        assert_eq!(r.restarted_said, None);
    }

    /// #1451, checklist 3: an app that couldn't be restarted to apply a turn-off is stopped:
    /// red-marked, at the top of the list, with Start again.
    #[test]
    fn an_app_that_couldnt_restart_to_apply_is_a_stopped_row_with_start_again() {
        let a = m("vulpes", true);
        let b = m("aardvark", true);
        let id = format!("module.{}.", a.id);
        let fact = move |k: &str| -> Option<Value> {
            match k.strip_prefix(&id)? {
                "state" => Some("install-failed".into()),
                "failed" => Some("apply".into()),
                "reason" => Some(words::COULDNT_RESTART_TO_APPLY.into()),
                _ => None,
            }
        };
        let r = row(&a, &fact, None);
        assert_eq!(
            r.state,
            "Stopped \u{b7} Couldn't restart to apply your change. Start it again."
        );
        assert!(r.stopped && !r.off);
        assert_eq!(
            r.primary,
            Some(Primary {
                label: words::START_AGAIN.into(),
                does: Does::StartAgain,
            })
        );
        assert_eq!(
            r.restarted_said.as_deref(),
            Some(format!("{} stopped. Couldn't restart to apply your change.", a.name).as_str())
        );
        // Checklist 7.
        assert_eq!(r.label, words::row_label(&a.name, &r.state));
        // Above a running app that sorts before it by name.
        let mut rows = vec![row(&b, &|_: &str| None, None), r];
        order(&mut rows);
        assert_eq!(rows[0].id, a.id);

        // Stopped because the turn-off couldn't be saved: the same row with its own words.
        let id = format!("module.{}.", a.id);
        let fact = move |k: &str| -> Option<Value> {
            match k.strip_prefix(&id)? {
                "state" => Some("install-failed".into()),
                "failed" => Some("unsaved".into()),
                "reason" => Some(words::STOPPED_UNSAVED.into()),
                _ => None,
            }
        };
        let r = row(&a, &fact, None);
        assert_eq!(
            r.state,
            "Stopped \u{b7} Your change couldn't be saved. Start it again to try again."
        );
        assert!(r.stopped && !r.off);
        assert_eq!(
            r.primary,
            Some(Primary {
                label: words::START_AGAIN.into(),
                does: Does::StartAgain,
            })
        );
        assert_eq!(
            r.restarted_said.as_deref(),
            Some(format!("{} stopped. Your change couldn't be saved.", a.name).as_str())
        );
        assert_eq!(r.label, words::row_label(&a.name, &r.state));
        let mut rows = vec![row(&b, &|_: &str| None, None), r];
        order(&mut rows);
        assert_eq!(rows[0].id, a.id);
    }

    fn changed_app(enabled: bool) -> Installed {
        let mut a = m("vulpes", enabled);
        a.name = "PC sensors".into();
        a.version = "0.1.0".into();
        a.adopted = true;
        a
    }

    fn facts_of(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<Value> {
        let pairs: Vec<(String, &'static str)> = pairs
            .iter()
            .map(|(k, v)| (format!("module.vulpes.{k}"), *v))
            .collect();
        move |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| json!(v))
    }

    /// An installed-separately app whose file changed: the changed words and **Update in
    /// Hideout** win over On, Off, "Waits for your OK" and the missing/version sentences; a row
    /// that is refused keeps the red mark.
    #[test]
    fn a_changed_row_says_so_and_offers_update_in_hideout() {
        let update = Some(("Update in Hideout".to_string(), Does::UpdateInHideout));
        let main = |r: &Row| r.primary.as_ref().map(|p| (p.label.clone(), p.does));
        // On.
        let r = row_changed(&changed_app(true), &[], &|_: &str| None, None, true);
        assert_eq!(r.state, "Changed since you added it");
        assert_eq!(main(&r), update);
        assert!(!r.stopped && !r.off);
        assert_eq!(
            r.label,
            "PC sensors, installed separately, changed since you added it"
        );
        let r = row_changed(
            &changed_app(true),
            &[],
            &facts_of(&[("state", "running")]),
            None,
            true,
        );
        assert_eq!(r.state, "Changed since you added it");
        assert_eq!(
            r.label,
            "PC sensors, installed separately, changed since you added it"
        );
        assert_eq!(main(&r), update);
        // Off: stays an Off row, Update in Hideout instead of Turn on.
        let r = row_changed(&changed_app(false), &[], &|_: &str| None, None, true);
        assert_eq!(r.state, "Off \u{b7} changed since you added it");
        assert!(r.off && !r.stopped);
        assert_eq!(main(&r), update);
        assert_eq!(
            r.label,
            "PC sensors, installed separately, off, changed since you added it"
        );
        // Waits for your OK, missing program, another version: the changed words, red mark.
        for facts in [
            vec![("state", "install-failed"), ("failed", "ok")],
            vec![
                ("state", "install-failed"),
                ("failed", "missing"),
                ("reason", "PC sensors isn't installed on this PC"),
            ],
            vec![
                ("state", "install-failed"),
                ("failed", "version"),
                ("reason", "PC sensors is another version"),
            ],
        ] {
            let r = row_changed(&changed_app(true), &[], &facts_of(&facts), None, true);
            assert_eq!(r.state, "Changed since you added it", "{facts:?}");
            assert_eq!(main(&r), update);
            assert!(r.stopped, "red mark kept: {facts:?}");
            assert_eq!(
                r.label,
                "PC sensors, installed separately, changed since you added it"
            );
        }
    }

    /// A search locks Update in Hideout with Turn on and Turn off, and nothing else.
    #[test]
    fn a_search_locks_update_in_hideout() {
        assert!(toggles(Does::UpdateInHideout));
        assert!(toggles(Does::TurnOn) && toggles(Does::TurnOff));
        assert!(!toggles(Does::Force));
    }

    /// The states that are not this one keep their own words and button, changed or not.
    #[test]
    fn a_changed_row_leaves_the_other_states_alone() {
        for facts in [
            vec![("state", "revoked")],
            vec![("state", "starting")],
            vec![("state", "updating")],
            vec![("state", "update-waiting")],
            vec![("state", "update-busy")],
            vec![("state", "quarantined")],
            vec![("state", "crash-stopped")],
            vec![("state", "needs-hub")],
            vec![("state", "install-failed"), ("failed", "launch")],
        ] {
            let f = facts_of(&facts);
            let a = changed_app(true);
            let plain = row_in(&a, &[], &f, None);
            let with = row_changed(&a, &[], &f, None, true);
            assert_eq!(with, plain, "{facts:?}");
            assert_ne!(with.state, "Changed since you added it");
        }
        // Withdrawn and then turned off, and safe mode.
        let off = changed_app(false);
        let f = facts_of(&[("state", "off")]);
        let withdrawn = |k: &str| match k {
            "module.vulpes.revoked" => Some(json!({"version": "0.1.0"})),
            _ => f(k),
        };
        assert_eq!(
            row_changed(&off, &[], &withdrawn, None, true).state,
            "Off \u{b7} 0.1.0 was withdrawn"
        );
        let safe = |k: &str| (k == "hub.safe_mode").then_some(json!(true));
        let r = row_changed(&changed_app(true), &[], &safe, None, true);
        assert_eq!(r.state, "On \u{b7} not started in safe mode");
        assert!(r.primary.is_none());
    }

    /// Not changed, or not installed separately: every row is what `row_in` gave before.
    #[test]
    fn a_row_that_is_not_changed_is_the_same_as_before() {
        for (enabled, facts) in [
            (true, vec![]),
            (false, vec![]),
            (false, vec![("state", "off")]),
            (true, vec![("state", "running")]),
            (true, vec![("state", "install-failed"), ("failed", "ok")]),
            (
                true,
                vec![
                    ("state", "install-failed"),
                    ("failed", "version"),
                    ("reason", "x"),
                ],
            ),
        ] {
            let f = facts_of(&facts);
            let a = changed_app(enabled);
            assert_eq!(
                row_changed(&a, &[], &f, None, false),
                row_in(&a, &[], &f, None)
            );
            // Not installed separately: the flag does nothing.
            let mut own = changed_app(enabled);
            own.adopted = false;
            assert_eq!(
                row_changed(&own, &[], &f, None, true),
                row_in(&own, &[], &f, None)
            );
        }
        let r = row_in(&changed_app(true), &[], &|_: &str| None, None);
        assert_eq!(r.state, "On");
        let r = row_in(&changed_app(false), &[], &|_: &str| None, None);
        assert_eq!(
            (r.state.as_str(), r.primary.unwrap().label.as_str()),
            ("Off", "Turn on")
        );
    }

    /// During a search (#1033): rows say on or off for the round, Off rows aren't part of it,
    /// rows that need the user keep their words; a locked Turn on keeps its focus, says the
    /// lock line as its description and does nothing.
    #[test]
    fn an_adopted_module_reads_as_a_row_with_its_version_and_the_marker() {
        let fact = |_: &str| None;
        let mut a = m("vulpes", true);
        a.name = "Example AI".into();
        a.version = "0.4.0".into();
        a.adopted = true;
        let r = row(&a, &fact, None);
        assert_eq!(r.name, "Example AI");
        assert_eq!(r.version, "0.4.0 \u{b7} installed separately");
        assert_eq!(r.version_said, "version 0.4.0, installed separately");
        assert_eq!(r.state, "On");
        // Any other row keeps its bare version.
        assert_eq!(row(&m("x", true), &fact, None).version, "0.5.0");
        assert_eq!(
            row(&m("x", true), &fact, None).version_said,
            "version 0.5.0"
        );
        // A path that isn't there is the launch's failure, worded as for any module.
        let failed = |k: &str| match k {
            "module.vulpes.state" => Some(json!("install-failed")),
            "module.vulpes.failed" => Some(json!("launch")),
            _ => None,
        };
        let r = row(&a, &failed, None);
        assert!(r.stopped);
        assert_eq!(r.state, words::COULDNT_START);
    }

    /// The row's accessible name says "installed separately" (the visible version text is a
    /// separate node a reader may skip), and the two adopted failures read as one sentence.
    #[test]
    fn an_adopted_row_is_named_and_its_failures_are_one_sentence() {
        let mut a = m("vulpes", true);
        a.name = "Example AI".into();
        a.version = "0.4.0".into();
        a.adopted = true;
        let r = row(&a, &|_: &str| None, None);
        assert_eq!(r.label, "Example AI, installed separately, on");
        // In the accessibility tree, too.
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = crate::theme::tokens(true);
        let rows = [r];
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = page_ui(ui, &t, &rows, None);
                });
            },
        );
        let nodes = out.platform_output.accesskit_update.unwrap().nodes;
        assert!(
            nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Example AI, installed separately, on")),
            "no row name with the marker"
        );
        // The program is gone.
        let missing = |k: &str| match k {
            "module.vulpes.state" => Some(json!("install-failed")),
            "module.vulpes.failed" => Some(json!("missing")),
            "module.vulpes.reason" => {
                Some(json!(words::not_installed("Example AI", "C:/x/ai.exe")))
            }
            _ => None,
        };
        let r = row(&a, &missing, None);
        assert!(r.stopped);
        assert_eq!(
            r.state,
            "Example AI isn't installed on this PC \u{2014} it was registered from C:/x/ai.exe"
        );
        assert_eq!(r.label, r.state, "the sentence already names the module");
        // Another version than it was registered as.
        let other = |k: &str| match k {
            "module.vulpes.state" => Some(json!("install-failed")),
            "module.vulpes.failed" => Some(json!("version")),
            "module.vulpes.reason" => Some(json!(words::version_differs(
                "Example AI",
                Some("0.5.0"),
                "0.4.0"
            ))),
            _ => None,
        };
        let r = row(&a, &other, None);
        assert!(r.stopped);
        assert_eq!(
            r.state,
            "Example AI is version 0.5.0 but was registered as 0.4.0 \u{2014} register it again"
        );
    }

    #[test]
    fn a_search_words_the_rows_and_locks_the_toggles() {
        use hideout_supervisor::bisect::words as w;
        let fact = |k: &str| match k {
            "hub.safe_mode" => Some(json!(true)),
            "module.m.c.state" => Some(json!("crash-stopped")),
            _ => None,
        };
        let m = |id: &str, name: &str, enabled| Installed {
            adopted: false,
            dev: false,
            dev_folder: None,
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            enabled,
        };
        let mut a = row(&m("m.a", "Art", true), &fact, None);
        let mut b = row(&m("m.b", "Beat", true), &fact, None);
        let mut c_ = row(&m("m.c", "Chat", true), &fact, None);
        let mut o = row(&m("m.o", "Oak", false), &fact, None);
        search_words(&mut a, Some(true));
        search_words(&mut b, Some(false));
        search_words(&mut c_, Some(false));
        search_words(&mut o, None);
        assert_eq!(a.state, w::ON_IN_ROUND);
        assert_eq!(b.state, w::OFF_FOR_ROUND);
        assert_eq!(b.label, "Beat, off for this round");
        assert_eq!(c_.state, words::STOPPED, "a stopped row keeps its words");
        assert_eq!(o.state, w::NOT_IN_SEARCH);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = crate::theme::tokens(false);
        let rows = vec![a, o];
        let mut clicked = None;
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let top = Top {
                        banner: None,
                        locked: true,
                        remove: None,
                        can_remove: true,
                    };
                    if let Some(c) = page_ui_with(ui, &t, &rows, top) {
                        clicked = Some(c);
                    }
                });
            })
        };
        let out = frame(Vec::new());
        let nodes = out.platform_output.accesskit_update.unwrap().nodes;
        let turn_on = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(words::TURN_ON))
            .expect("no Turn on node");
        assert!(!turn_on.is_disabled(), "locked, but still focusable");
        assert_eq!(turn_on.description(), Some(w::LOCKED));
        let labels: Vec<_> = nodes
            .iter()
            .filter_map(|(_, n)| n.label().or(n.value()))
            .collect();
        assert!(labels.contains(&w::LOCKED), "the lock line under the list");
        // Tab to it and press Enter: nothing.
        for _ in 0..4 {
            let key = |pressed| egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![key(true), key(false)]);
            let enter = |pressed| egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![enter(true), enter(false)]);
        }
        assert!(
            !matches!(clicked, Some(Click::Primary(_, Does::TurnOn))),
            "{clicked:?}"
        );
    }

    /// The banner's link starts the search; with nothing to search the line says it's Hideout
    /// itself and Copy diagnostics leads (#1033).
    #[test]
    fn the_banner_offers_the_search_or_says_its_hideout_itself() {
        use hideout_supervisor::bisect::words as w;
        let t = crate::theme::tokens(false);
        let labels = |b: Banner| {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let out = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    banner_ui(ui, &t, &b);
                });
            });
            let nodes = out.platform_output.accesskit_update.unwrap().nodes;
            nodes
                .iter()
                .filter_map(|(_, n)| n.label().or(n.value()).map(String::from))
                .collect::<Vec<_>>()
        };
        let one = w::check_one("Art");
        let l = labels(Banner {
            line: words::SAFE_MODE_LINE,
            note: Some(w::STOPPED),
            find: Find::Link(&one),
        });
        for s in [
            words::SAFE_MODE_LINE,
            w::STOPPED,
            words::START_NORMALLY,
            one.as_str(),
        ] {
            assert!(l.iter().any(|x| x == s), "{s} not in {l:?}");
        }
        let l = labels(Banner {
            line: words::SAFE_MODE_LINE,
            note: None,
            find: Find::Nothing,
        });
        assert!(l.iter().any(|x| x == w::NONE_ON), "{l:?}");
        assert!(!l.iter().any(|x| x == words::SAFE_MODE_LINE), "{l:?}");
        assert!(!l.iter().any(|x| x == words::FIND_CULPRIT), "{l:?}");
        assert!(l.iter().any(|x| x == words::COPY_DIAGNOSTICS), "{l:?}");
    }

    /// The banner says the line it's given, and the greyed bisect link carries "Comes in a later
    /// Hideout." as its accessible description, not only as a hover tooltip (#1033).
    #[test]
    fn the_banner_says_its_line_and_the_greyed_link_why() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = crate::theme::tokens(false);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                page_ui(ui, &t, &[], Some(words::SAFE_MODE_LINE_SET));
            });
        });
        let nodes = out.platform_output.accesskit_update.unwrap().nodes;
        let link = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(words::FIND_CULPRIT))
            .expect("no bisect link node");
        assert!(link.is_disabled());
        assert_eq!(link.description(), Some(words::COMES_LATER));
        // Labels carry their text as the value, buttons and links as the label.
        let labels: Vec<_> = nodes
            .iter()
            .filter_map(|(_, n)| n.label().or(n.value()))
            .collect();
        assert!(labels.contains(&words::SAFE_MODE_LINE_SET), "{labels:?}");
        assert!(!labels.contains(&words::SAFE_MODE_LINE), "{labels:?}");
    }

    /// The menu only asks; the confirm answers once, for the module it asked about.
    #[test]
    fn remove_needs_the_confirm_for_that_module_and_answers_once() {
        let mut a = RemoveAsk::default();
        assert!(!a.confirm("art"), "nothing was asked");
        a.ask("art");
        assert_eq!(a.pending(), Some("art"));
        assert!(!a.confirm("beat"), "another module's confirm");
        assert_eq!(a.pending(), Some("art"));
        assert!(a.confirm("art"));
        assert!(!a.confirm("art"), "answers once");
        a.ask("art");
        a.cancel();
        assert!(!a.confirm("art"), "cancelled");
        a.ask("art");
        a.ask("beat");
        assert!(!a.confirm("art"), "a newer ask replaces the older");
    }

    fn labels_of(rows: &[Row], remove: Option<&str>) -> (Vec<String>, Option<Click>) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = crate::theme::tokens(false);
        let mut clicked = None;
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let top = Top {
                    remove,
                    can_remove: true,
                    ..Top::default()
                };
                clicked = page_ui_with(ui, &t, rows, top);
            });
        });
        let nodes = out.platform_output.accesskit_update.unwrap().nodes;
        let labels = nodes
            .iter()
            .filter_map(|(_, n)| n.label().or(n.value()).map(String::from))
            .collect();
        (labels, clicked)
    }

    /// The window's close button cancels an open Remove confirm, like Esc; a frame without it
    /// answers nothing.
    #[test]
    fn the_windows_close_button_cancels_the_remove_confirm() {
        let rows = [row_of("art", "Art", false)];
        let t = crate::theme::tokens(false);
        let ctx = egui::Context::default();
        let mut got = Vec::new();
        for close in [false, false, true] {
            let mut raw = egui::RawInput::default();
            if close {
                raw.viewports
                    .entry(egui::ViewportId::ROOT)
                    .or_default()
                    .events
                    .push(egui::ViewportEvent::Close);
            }
            let mut clicked = None;
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let top = Top {
                        remove: Some("art"),
                        can_remove: true,
                        ..Top::default()
                    };
                    clicked = page_ui_with(ui, &t, &rows, top);
                });
            });
            got.push(clicked);
        }
        assert_eq!(got, [None, None, Some(Click::CancelRemove)]);
    }

    fn row_of(id: &str, name: &str, adopted: bool) -> Row {
        row(
            &Installed {
                id: id.into(),
                name: name.into(),
                version: "1.0.0".into(),
                enabled: true,
                dev: false,
                dev_folder: None,
                adopted,
            },
            &|_| None,
            None,
        )
    }

    /// Removing reads "Removing…" with no buttons; a failure says so plainly under the row with
    /// Try again and Close; a hub that can't remove has no menu; a search locks the confirm.
    #[test]
    fn removing_and_failed_rows_say_so_and_a_hub_without_a_supervisor_has_no_menu() {
        let mut one = row_of("m.one", "One", false);
        let mut two = row_of("m.two", "Two", false);
        one.removal = Some(Removal::Removing);
        two.removal = Some(Removal::Failed);
        let (l, _) = labels_of(&[one, two], None);
        assert!(l.iter().any(|x| x == REMOVING), "{l:?}");
        assert!(!l.iter().any(|x| x == "More actions for One"), "{l:?}");
        assert!(l.iter().any(|x| x == "More actions for Two"), "{l:?}");
        assert!(l.iter().any(|x| x == &remove_failed("Two")), "{l:?}");
        assert!(l.iter().any(|x| x == TRY_AGAIN) && l.iter().any(|x| x == CLOSE));
        for never in words::NEVER_SHOWN {
            assert!(!remove_failed("x")
                .to_lowercase()
                .contains(&never.to_lowercase()));
        }

        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = crate::theme::tokens(false);
        let rows = [row_of("m.one", "One", false)];
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let top = Top {
                    remove: Some("m.one"),
                    can_remove: false,
                    locked: true,
                    ..Top::default()
                };
                page_ui_with(ui, &t, &rows, top);
            });
        });
        let nodes = out.platform_output.accesskit_update.unwrap().nodes;
        let find = |s: &str| nodes.iter().find(|(_, n)| n.label() == Some(s));
        assert!(
            find("More actions for One").is_none(),
            "no menu without a supervisor"
        );
        let remove = find(REMOVE).expect("the confirm shows");
        assert!(remove.1.is_disabled(), "a search locks Remove");
        let cancel = find(CANCEL).expect("Cancel");
        assert!(!cancel.1.is_disabled());
    }

    /// The confirm shows only under the row that was asked about, says plainly what happens
    /// (never an id), and drawing a page clicks nothing.
    #[test]
    fn the_confirm_says_what_happens_and_only_shows_when_asked() {
        let rows = [row_of("m.one", "One", true), row_of("m.two", "Two", false)];
        let (l, c) = labels_of(&rows, None);
        assert_eq!(c, None);
        for s in [REMOVE_ADOPTED, REMOVE_OWN, REMOVE, CANCEL] {
            assert!(!l.iter().any(|x| x == s), "{s} shown unasked: {l:?}");
        }
        assert!(l.iter().any(|x| x == "More actions for One"), "{l:?}");

        let (l, c) = labels_of(&rows, Some("m.one"));
        assert_eq!(c, None, "the confirm never clicks itself");
        assert!(l.iter().any(|x| x == REMOVE_ADOPTED), "{l:?}");
        assert!(!l.iter().any(|x| x == REMOVE_OWN), "{l:?}");
        assert!(l.iter().any(|x| x == &remove_question("One")), "{l:?}");
        assert!(l.iter().any(|x| x == REMOVE) && l.iter().any(|x| x == CANCEL));

        let (l, _) = labels_of(&rows, Some("m.two"));
        assert!(l.iter().any(|x| x == REMOVE_OWN), "{l:?}");
        assert!(!l.iter().any(|x| x == REMOVE_ADOPTED), "{l:?}");

        for s in [
            REMOVE_ADOPTED,
            REMOVE_OWN,
            REMOVE_MENU,
            &remove_question("x"),
        ] {
            for never in words::NEVER_SHOWN {
                assert!(!s.to_lowercase().contains(&never.to_lowercase()), "{s}");
            }
        }
    }

    #[test]
    fn the_page_copy_says_app_never_module() {
        // Same rule as the words.rs guard: one noun, "app", in what the user reads.
        let all = [
            remove_question("Art"),
            REMOVE_MENU.to_string(),
            REMOVE_ADOPTED.to_string(),
            REMOVE_OWN.to_string(),
            REMOVING.to_string(),
            remove_failed("Art"),
            turn_on_failed("Art", "x"),
            EMPTY_HEADING.to_string(),
            EMPTY_BODY.to_string(),
            MORE.to_string(),
            crate::settings_view::PRIVACY_LINE.to_string(),
            TITLE.to_string(),
        ];
        for line in &all {
            assert!(
                !line.to_lowercase().contains("module"),
                "module in {line:?}"
            );
        }
        assert_eq!(EMPTY_HEADING, "No apps yet");
    }
}
