// SPDX-License-Identifier: MIT
//! The module permission dialog's words and rules (#1178, `docs/hideout-permission-dialog-ux.md`):
//! what the dialog, the header lines and the Permissions section say and do. Pure and
//! window-free, so the wording, the counts and the defaults are tested without a window; the
//! drawing is `permission_dialog_ui.rs`. The shell has no registry: the hub fills these from
//! `Registry::permission_view`.
//!
//! Sentence case. The window never says "permission string", "provide", "method", "router",
//! "hash" or an id, except the selectable path and the list code.

use hideout_contract::permission::OneRun;

pub const ALLOW_NONE: &str = "Allow none";
/// The button of an app that asks for nothing (there is nothing to count).
pub const ALLOW: &str = "Allow";
pub const ALLOW_AND_START: &str = "Allow and start";
pub const SOURCE_UNSIGNED: &str = "Not signed";
pub const SHOW: &str = "Show";
pub const HIDE: &str = "Hide";
pub const NOT_NOW: &str = "Not now";
pub const ALL_PREFIX: &str = "All";
pub const UNDERSTAND: &str = "I understand";
pub const REVIEW: &str = "Review\u{2026}";
pub const OK: &str = "OK";
pub const TURN_OFF: &str = "Turn off";
pub const TURN_ALL_OFF: &str = "Turn all off";
pub const CHANGE: &str = "Change\u{2026}";
pub const WAITS: &str = "Waits for your OK";
pub const LIST_CHANGED: [&str; 2] = [
    "The list changed while it was open. Nothing was allowed.",
    "Check the new list below, then choose again.",
];
pub const BUSY: &str = "Hideout is busy. Try again in a moment.";
pub const NOT_SAVED: &str = "Hideout couldn't save your choice. Nothing changed. Try again.";
pub const SECTION_LABEL: &str = "Permissions";

/// "Keep my OK after updates in this folder" (slice 4 of the rebuild-trust note,
/// `review/ux-keep-ok-switch.md`). Plain words: "app", never "program", "hash", "grant",
/// "follow" or an id.
pub const KEEP_LABEL: &str =
    "Keep my OK after updates in this folder, except Sensitive and keyboard";
/// Why the box is off and disabled: a network, removable or unreadable folder.
pub const KEEP_REFUSED: &str = "Hideout keeps your OK only for a folder on this PC's own drive.";
/// Why the box is off and disabled for a folder a cloud service keeps in step (#1465).
pub const KEEP_SYNCED: &str = "This folder syncs with the cloud, so Hideout asks again after each \
     update. To keep your OK, move the app to a folder that stays on this PC.";
/// Why the box is off when Hideout could not tell whether the folder syncs (#1465).
pub const KEEP_SYNC_UNKNOWN: &str = "Hideout couldn't check whether this folder syncs with the \
     cloud, so it asks again after each update. Try again later.";
/// The reason line of the Allow dialog, and the header body, for an app whose file changed under
/// "Keep my OK" (#1465): the app keeps running with what carried over.
pub const KEPT_CHANGED_BODY: &str = "Its app file changed. Sensitive items and the keyboard need \
     your OK again. It runs without them until then.";
/// Binding line 2 while the box is checked.
pub const KEEP_BINDING: &str = "If the app asks for more or moves, Hideout asks again.";
/// [`KEEP_BINDING`] for an app that asks for nothing.
pub const KEEP_BINDING_NOTHING: &str =
    "If the app asks for something or moves, Hideout asks again.";
pub const KEEP_CONFIRM_TITLE: &str = "Keep your OK for this folder?";
pub const KEEP_CONFIRM_OK: &str = "Keep my OK";
pub const CANCEL: &str = "Cancel";
pub const TURN_ON_KEEP: &str = "Turn on\u{2026}";
/// The spoken names of the row's buttons (no ellipsis when spoken).
pub const KEEP_TURN_ON_SAID: &str = "Turn on keep my OK after updates, opens a confirm";
pub const KEEP_TURN_OFF_SAID: &str = "Turn off keep my OK after updates";
/// Shown when the choice could not be saved.
pub const KEEP_NOT_SAVED: &str = "Hideout couldn't save your choice. Nothing changed. Try again.";
/// How many arguments show before "Show all N" (#1452).
pub const ARGS_SHOWN: usize = 4;
/// How many lines the opened argument list shows before it scrolls.
pub const ARGS_OPEN_LINES: usize = 8;
/// An argument longer than this is cut in the middle while the list is closed: its start, how
/// many characters are left out, its end. The open list shows it whole.
pub const ARG_MAX: usize = 80;
const ARG_HEAD: usize = 30;
/// What stands for an argument that is nothing but characters that are not drawn.
pub const ARG_UNSEEN: &str = "characters that can't be shown";
/// The link that opens the list when all arguments are listed and one is cut.
pub const SHOW_ALL: &str = "Show all";
/// Shown when an argument holds characters that are not drawn (or quote marks, drawn as `'`).
pub const ARGS_NOTE: &str = "Some characters can't be shown.";
pub const SHOW_FEWER: &str = "Show fewer";
/// How many of the newest history lines show before "and N earlier".
pub const HISTORY_SHOWN: usize = 3;

/// The last part of a path (`C:\Apps\Foxglove\foxglove.exe` gives `foxglove.exe`).
pub fn file_of(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

/// "Show all 6": the link that opens the whole argument list.
pub fn show_all(n: usize) -> String {
    format!("Show all {n}")
}

/// One argument as it is shown, whole: cleaned ([`hideout_contract::module::shown_argument`]:
/// nothing in it can end its quotes, break its line or turn its direction), and whether
/// cleaning changed it. Never cut: this is what is spoken, what the control pipe lists and what
/// the opened list draws.
pub fn shown_arg(raw: &str) -> (String, bool) {
    hideout_contract::module::shown_argument(raw)
}

/// A cleaned argument cut for the closed list, `None` when it fits ([`ARG_MAX`]): its start,
/// "…(123 more characters)…" with the number that is left out, and its end, so the end
/// stays visible and the reader sees how much is not.
fn cut_arg(clean: &str) -> Option<String> {
    let n = clean.chars().count();
    if n <= ARG_MAX {
        return None;
    }
    let mark = |left_out: usize| format!("\u{2026}({left_out} more characters)\u{2026}");
    // Room for the mark with its longest number, so the line never passes `ARG_MAX`.
    let tail = ARG_MAX.saturating_sub(ARG_HEAD + mark(n - ARG_HEAD).chars().count());
    let start: String = clean.chars().take(ARG_HEAD).collect();
    let end: String = clean.chars().skip(n - tail).collect();
    Some(format!("{start}{}{end}", mark(n - ARG_HEAD - tail)))
}

/// Whether the closed list cuts this argument (the list can then be opened to see it whole).
pub fn arg_is_cut(raw: &str) -> bool {
    shown_arg(raw).0.chars().count() > ARG_MAX
}

/// "Runs with 2 arguments:", the heading over the arguments; `None` for none.
pub fn args_heading(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("Runs with 1 argument:".into()),
        n => Some(format!("Runs with {n} arguments:")),
    }
}

/// One argument as its line, whole: in quotes, an empty one marked so that it is seen, and one
/// that is nothing but characters that are not drawn marked as that (it is not empty).
pub fn arg_line(raw: &str) -> String {
    match shown_arg(raw).0 {
        s if s.is_empty() && raw.is_empty() => "\u{201c}\u{201d} (empty)".to_string(),
        s if s.is_empty() => format!("\u{201c}\u{201d} ({ARG_UNSEEN})"),
        s => format!("\u{201c}{s}\u{201d}"),
    }
}

/// [`arg_line`] for the closed list: a long argument cut in the middle ([`ARG_MAX`]).
pub fn arg_line_cut(raw: &str) -> String {
    match cut_arg(&shown_arg(raw).0) {
        Some(cut) => format!("\u{201c}{cut}\u{201d}"),
        None => arg_line(raw),
    }
}

/// A program's file name inside a sentence: cleaned like an argument and cut to 40 characters.
pub fn shown_file(path: &str) -> String {
    let (clean, _) = hideout_contract::module::shown_argument(&file_of(path));
    hideout_contract::module::cut_middle(&clean, 40, 20)
}

/// What the control pipe answers when an Allow was stored for one run only (#1452): the same
/// facts as the dialog's lines.
pub fn one_run_reply(path: &str) -> String {
    format!(
        "allowed for this run only; {} runs whatever it is given, so Hideout asks again at the \
         next start",
        shown_file(path)
    )
}

/// The helper under the label.
pub fn keep_helper(name: &str, file: &str) -> String {
    format!(
        "A new build of {name} saved here as {file} keeps what you allowed, except Sensitive \
         items and the keyboard. Hideout asks again for those, and if the new build wants more \
         or the app moves."
    )
}

/// The helper under the label for an app that asks for nothing.
pub fn keep_helper_nothing(name: &str, file: &str) -> String {
    format!(
        "A new build of {name} saved here as {file} keeps your OK. Hideout asks again if the \
         new build asks for something, or if the app moves."
    )
}

/// After checking, in the attention colour.
pub fn keep_warning(name: &str, file: &str) -> String {
    format!(
        "Any app saved in this folder as {file} gets what you allowed {name}, except Sensitive \
         items and the keyboard. Only do this for a folder where only you put apps."
    )
}

/// The accessible name of the "I understand" under the box.
pub fn keep_understand_said(name: &str, file: &str) -> String {
    format!("{UNDERSTAND}: any app saved in this folder as {file} gets what you allowed {name}, except Sensitive items and the keyboard")
}

/// The polite note after Turn off.
pub fn keep_off_note(name: &str) -> String {
    format!("Off. A new build of {name} will need your OK again.")
}

/// The muted text instead of a button when Turn on is not offered yet.
pub fn keep_allow_first(name: &str) -> String {
    format!("Allow {name} first.")
}

/// Shown on the app's entry when its list changed while the confirm was open (the confirm has
/// closed by then).
pub fn keep_changed_open(name: &str) -> String {
    format!("{name} changed while you were choosing. Look at its list again, then choose.")
}

/// "New build on 17 Nov 2026, 20:40. Your OK stayed, except Sensitive and keyboard." and, when the
/// new build asks for more, "...; 2 things wait for it." `when` is the local time as
/// words ("17 Nov 2026, 20:40").
pub fn keep_history(when: &str, grew: usize) -> String {
    let mut line = format!("New build on {when}. Your OK stayed, except Sensitive and keyboard");
    if grew > 0 {
        line.push_str(&format!(
            "; {grew} thing{} wait{} for it",
            if grew == 1 { "" } else { "s" },
            if grew == 1 { "s" } else { "" }
        ));
    }
    line.push('.');
    line
}

/// The line after a kept binding went void: the app moved (or is gone), or starts in a new way.
pub fn keep_void_line(moved: bool, when: &str) -> String {
    if moved {
        format!("Moved or removed on {when}. Keeping your OK after updates is now off.")
    } else {
        format!("Starts in a new way since {when}. Keeping your OK after updates is now off.")
    }
}

/// "and 4 earlier", over the history when more than [`HISTORY_SHOWN`] lines exist.
pub fn earlier_label(n: usize) -> String {
    format!("and {n} earlier")
}

/// Whether the dialog offers the box or says why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeepOffer {
    Offered,
    Refused(String),
}

/// The dialog's "Keep my OK after updates in this folder" (None on `PermDialog` for a module with
/// no app path). Off by default; it starts on, with the understanding done, only when the
/// choice is already live (so an Allow that leaves the box alone does not turn it off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keep {
    pub offer: KeepOffer,
    pub on: bool,
    pub understood: bool,
}

impl Keep {
    /// The box on offer; `already`: the choice is live now.
    pub fn offered(already: bool) -> Keep {
        Keep {
            offer: KeepOffer::Offered,
            on: already,
            understood: already,
        }
    }

    /// The box refused for a folder that is not on this PC's own drive.
    pub fn refused() -> Keep {
        Keep::refused_with(KEEP_REFUSED)
    }

    /// The box off and disabled, with the reason said (a drive that is not local, a folder that
    /// syncs, a folder whose sync state could not be read).
    pub fn refused_with(reason: &str) -> Keep {
        Keep {
            offer: KeepOffer::Refused(reason.into()),
            on: false,
            understood: false,
        }
    }

    pub fn is_refused(&self) -> bool {
        matches!(self.offer, KeepOffer::Refused(_))
    }

    /// Allow sends it only when it is offered, checked and understood.
    pub fn active(&self) -> bool {
        !self.is_refused() && self.on && self.understood
    }

    /// The box: unchecking clears the understanding; a refused box stays off.
    pub fn set_on(&mut self, on: bool) {
        if self.is_refused() {
            return;
        }
        self.on = on;
        if !on {
            self.understood = false;
        }
    }

    /// The accessible name: the label and its state.
    pub fn said(&self) -> String {
        format!(
            "{KEEP_LABEL}, {}",
            if self.on { "checked" } else { "not checked" }
        )
    }

    /// The accessible description: the helper, or the reason when disabled.
    pub fn description(&self, name: &str, file: &str) -> String {
        match &self.offer {
            KeepOffer::Offered => keep_helper(name, file),
            KeepOffer::Refused(r) => r.clone(),
        }
    }
}

/// The small confirm of the Modules page's Turn on…: what it names and the list code it takes
/// when it opens (the code covers the app's file, so a build that lands meanwhile fails it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepConfirm {
    pub id: String,
    pub name: String,
    pub file: String,
    pub folder: String,
    pub code: String,
    /// An inline failure under the buttons.
    pub error: Option<String>,
}

impl KeepConfirm {
    /// Line 1 of the body.
    pub fn line(&self) -> String {
        keep_warning(&self.name, &self.file)
    }

    /// The dialog's description: line 1, then the folder.
    pub fn said(&self) -> String {
        format!("{} {}", self.line(), self.folder)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepAnswer {
    Keep,
    Cancel,
}

/// How the Modules page's row stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeepState {
    On,
    /// On, but the folder syncs or could not be read: nothing carries. The reason replaces the
    /// helper; Turn off stays so the user can end it here (#1465).
    OnHeld(String),
    /// Off, with Turn on… offered.
    Off,
    /// Off, no button: the muted reason (a drive that is not local, or "Allow {name} first.").
    Unavailable(String),
}

/// The Permissions section's row for the choice: label, state word, one button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepRow {
    pub state: KeepState,
    pub name: String,
    pub file: String,
    pub folder: String,
}

impl KeepRow {
    pub fn word(&self) -> &'static str {
        match self.state {
            KeepState::On | KeepState::OnHeld(_) => "On",
            _ => "Off",
        }
    }

    /// The button's text, where one is offered.
    pub fn button(&self) -> Option<&'static str> {
        match self.state {
            KeepState::On | KeepState::OnHeld(_) => Some(TURN_OFF),
            KeepState::Off => Some(TURN_ON_KEEP),
            KeepState::Unavailable(_) => None,
        }
    }

    pub fn button_said(&self) -> Option<&'static str> {
        match self.state {
            KeepState::On | KeepState::OnHeld(_) => Some(KEEP_TURN_OFF_SAID),
            KeepState::Off => Some(KEEP_TURN_ON_SAID),
            KeepState::Unavailable(_) => None,
        }
    }

    /// The muted line under the label.
    pub fn helper(&self) -> String {
        match &self.state {
            KeepState::Unavailable(r) | KeepState::OnHeld(r) => r.clone(),
            _ => keep_helper(&self.name, &self.file),
        }
    }

    /// The row's accessible name.
    pub fn said(&self) -> String {
        let line = match &self.state {
            KeepState::OnHeld(r) => format!(", {r}"),
            _ => String::new(),
        };
        format!(
            "{KEEP_LABEL}, {}, {}{line}",
            self.word().to_lowercase(),
            self.folder
        )
    }
}

/// Under the Apps page line for a command's request.
pub const QUEUED_BODY: &str =
    "Nothing is allowed yet. Review shows its whole list, checked as usual. You choose what to allow.";
/// The mark on an item checked from the user's earlier answer.
pub const BEFORE_MARK: &str = "Allowed before";
/// The mark on an unchecked item a command asked to allow.
pub const ASKED_MARK: &str = "Asked by a command";
/// Under the reason lines when a command asked for something that is not checked.
pub const ASKED_LINES: [&str; 2] = [
    "A command asked for the items marked \"Asked by a command\". Hideout did not check them.",
    "Check one only if you want it. Nothing is allowed until you press Allow.",
];
/// Under the reason lines when something is checked from before.
pub const BEFORE_LINES: [&str; 2] = [
    "Items marked \"Allowed before\" are checked from your last answer.",
    "You can uncheck any. Nothing is allowed until you press Allow.",
];

/// The groups, in the dialog's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Sensitive,
    MakeChanges,
    AnswerQuestions,
    ShareStatus,
    CantBeAllowed,
}

impl Group {
    pub fn heading(self) -> &'static str {
        match self {
            Group::Sensitive => "Sensitive",
            Group::MakeChanges => "Make changes when an app asks",
            Group::AnswerQuestions => "Answer questions from apps",
            Group::ShareStatus => "Share its status",
            Group::CantBeAllowed => "Can't be allowed",
        }
    }

    pub const ALL: [Group; 5] = [
        Group::Sensitive,
        Group::MakeChanges,
        Group::AnswerQuestions,
        Group::ShareStatus,
        Group::CantBeAllowed,
    ];
}

/// One line of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    /// The canonical permission string (what is sent back on Allow; never drawn).
    pub raw: String,
    pub line: String,
    pub group: Group,
    /// Can be allowed (a checkbox); false: plain text with its reason.
    pub allowable: bool,
    pub sensitive: bool,
    pub reason: Option<String>,
    pub checked: bool,
    /// The "I understand" under a sensitive item.
    pub confirmed: bool,
    /// Not in the list the user saw before it changed (marked "New").
    pub new: bool,
    /// Checked from the user's earlier answer for this same app file (display only; nothing is
    /// allowed until Allow). Never set for a sensitive item.
    pub before: bool,
    /// A command asked to allow this item and the dialog opened with it unchecked (display
    /// only: the command never checks anything). Set when the dialog opens and kept while the
    /// user checks or unchecks it; never set for an item that can't be allowed.
    pub asked: bool,
}

impl ListItem {
    /// Counts toward Allow: checked, and for a sensitive item also confirmed.
    pub fn counts(&self) -> bool {
        self.allowable && self.checked && (!self.sensitive || self.confirmed)
    }

    /// The sub-line under an item, where one helps.
    pub fn sub_line(&self) -> Option<&'static str> {
        (self.raw == "facts.write:keyboard.*").then_some("which keyboard, battery, layer, lights")
    }

    /// "<plain line>. This lets <name> <risk sentence>." under a sensitive item.
    pub fn confirm_line(&self, name: &str) -> String {
        format!(
            "{}. {}",
            self.line.trim_end_matches('.'),
            self.risk_line(name)
        )
    }

    /// "This lets <name> <risk>.": what the item lets the app do, in plain words.
    pub fn risk_line(&self, name: &str) -> String {
        let risk = match self.raw.as_str() {
            r if r.starts_with("provide:gpu.") => {
                "see every AI request from every app, \
             including what you type, and answer them"
            }
            "key.events" => "see every key you press",
            "hid.raw" => "talk to your devices directly",
            r if r.starts_with("device.hid:") => "talk to your devices directly",
            r if r == "net" || r.starts_with("net.") => "use the network",
            r if r.starts_with("ai.ask:") => "send questions to an AI service",
            r if r.starts_with("secrets.ref:") => "use a secret you stored",
            _ => "do something Hideout can't describe in a line",
        };
        format!("This lets {name} {risk}.")
    }

    /// In the group form the risk sentence stays on screen under every item the group note
    /// does not already say: devices and the network are in the note, anything else (every key
    /// press, AI requests, a stored secret, the GPU manager) is not. `None` for the former.
    pub fn group_risk_line(&self, name: &str) -> Option<String> {
        let in_note = self.raw.starts_with("device.hid:")
            || self.raw == "hid.raw"
            || self.raw == "net"
            || self.raw.starts_with("net.");
        (self.sensitive && !in_note).then(|| self.risk_line(name))
    }

    /// The accessible name: group verb, line, risk word and state ("..., checked").
    pub fn said(&self) -> String {
        let mut s = format!("{}: {}", self.group.heading(), self.line);
        if self.sensitive {
            s.push_str(", sensitive");
        }
        if self.new {
            s.push_str(", new");
        }
        if self.before {
            s.push_str(", allowed before");
        }
        if self.asked {
            s.push_str(", ");
            s.push_str(&ASKED_MARK.to_lowercase());
        }
        s.push_str(if self.checked {
            ", checked"
        } else {
            ", not checked"
        });
        s
    }
}

/// Why the dialog opened with a line on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// "The list changed while it was open."
    ListChanged,
    /// The program is new or was moved since the allow.
    Void,
    /// A kept grant whose signed update asks for more (#1238): only the new items are marked.
    More,
    /// The app file changed under "Keep my OK" (#1465): Sensitive items and the keyboard wait
    /// for the OK again, shown as allowed before.
    Kept,
}

/// The keyboard section (Foxglove only): off by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardChoice {
    pub on: bool,
    pub expanded: bool,
}

/// The dialog's state while it is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermDialog {
    pub id: String,
    pub name: String,
    /// The list code the dialog shows and sends back (16 hex digits).
    pub code: String,
    /// The program's canonical path (adopted modules); empty otherwise.
    pub path: String,
    /// The arguments the program is run with, as the manifest gives them (untrusted text: drawn
    /// only through [`shown_arg`]). Shown under the path; the list code covers them.
    pub args: Vec<String>,
    /// The program runs whatever it is given (a script runner, a part of Windows): Allow is
    /// for this run only and "Keep my OK" is not offered (#1452).
    pub one_run: Option<OneRun>,
    pub items: Vec<ListItem>,
    pub reason: Option<Reason>,
    pub keyboard: Option<KeyboardChoice>,
    /// "Keep my OK after updates in this folder"; `None` for a module with no app path.
    pub keep: Option<Keep>,
    /// After a changed list: the lines of items the old list asked for and this one doesn't.
    pub removed: Vec<String>,
    /// The app is at the very path the grant was bound to (so its file changed, not moved).
    pub same_place: bool,
    /// How the dialog is presented (first-run slice FR2); no effect on what Allow records.
    pub view: View,
    /// An inline failure under the buttons (busy, not saved).
    pub error: Option<String>,
}

/// Presentation only: nothing here changes what is allowed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// Allow also starts the app (the label says so).
    pub starts: bool,
    /// The everyday items are listed (they start collapsed, still ticked).
    pub everyday_open: bool,
    /// Every argument is listed (a list of more than [`ARGS_SHOWN`] starts with the first few).
    pub args_open: bool,
    /// One "I understand" for the whole sensitive group (first run): `None` when it is not
    /// offered (each sensitive item then has its own), else whether it is ticked. Ticking it
    /// ticks and confirms every sensitive item; each item can still be unticked by itself.
    pub group_ack: Option<bool>,
    /// The app is open on its own right now and Allow closes that copy first (FR5): the dialog
    /// says so in one line. Presentation only; the hand-over is the page's, after the Allow.
    pub handover: bool,
}

/// What Allow sends back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub code: String,
    pub chosen: Vec<String>,
    pub declined: Vec<String>,
    pub keyboard: bool,
    /// Keep the OK for a new build in this folder: offered, checked and understood.
    pub keep: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermAnswer {
    Allow(Allowed),
    NotNow,
}

/// The tri-state of the All box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum All {
    All,
    Some,
    None,
}

impl PermDialog {
    pub fn title(&self) -> String {
        if self.reason == Some(Reason::More) {
            return format!("Let {} do more?", self.name);
        }
        if self.reason == Some(Reason::Kept) {
            return format!("{} changed since you allowed it", self.name);
        }
        format!("Let {} run in Hideout?", self.name)
    }

    /// "<name> asks for these <N> things. Each app that uses them still needs its own
    /// permission. Nothing leaves this PC." (the last sentence goes when the network is asked).
    /// One item reads "asks for 1 thing"; none, "<name> doesn't ask for anything."
    pub fn intro(&self) -> String {
        let n = self.items.len();
        if n == 0 {
            return format!(
                "{} doesn't ask for anything. Nothing leaves this PC.",
                self.name
            );
        }
        let never = n - self.allowable().count();
        let mut s = if n == 1 {
            format!("{} asks for 1 thing.", self.name)
        } else {
            format!("{} asks for these {n} things.", self.name)
        };
        if never > 0 {
            s.push_str(&format!(" {never} can't be allowed."));
        }
        s.push_str(if n == 1 {
            " Each app that uses it still needs its own permission."
        } else {
            " Each app that uses them still needs its own permission."
        });
        if !self
            .items
            .iter()
            .any(|i| i.raw == "net" || i.raw.starts_with("net."))
        {
            s.push_str(" Nothing leaves this PC.");
        }
        s
    }

    /// The lines that explain the marks: what is pre-checked from before, and what a command
    /// asked for that is not checked. None when no item has either mark.
    pub fn before_lines(&self) -> Vec<String> {
        let before = self.items.iter().any(|i| i.before);
        let asked = self.items.iter().any(|i| i.asked);
        let lines: &[&str] = match (before, asked) {
            (true, true) => &[BEFORE_LINES[0], ASKED_LINES[0], ASKED_LINES[1]],
            (true, false) => &BEFORE_LINES,
            (false, true) => &ASKED_LINES,
            (false, false) => &[],
        };
        lines.iter().map(|l| (*l).into()).collect()
    }

    pub fn void_line(&self) -> String {
        if self.same_place {
            return format!("{}'s app file changed since you allowed it.", self.name);
        }
        format!(
            "{}'s app file is new or was moved since you allowed it.",
            self.name
        )
    }

    /// The reason lines on top.
    pub fn reason_lines(&self) -> Vec<String> {
        let mut v = self.reason_lines_base();
        v.extend(self.before_lines());
        v
    }

    fn reason_lines_base(&self) -> Vec<String> {
        match self.reason {
            Some(Reason::ListChanged) => {
                let mut v: Vec<String> = LIST_CHANGED.iter().map(|s| (*s).into()).collect();
                if !self.removed.is_empty() {
                    v.push(format!("No longer asked: {}", self.removed.join(", ")));
                }
                v
            }
            Some(Reason::Void) => vec![self.void_line()],
            Some(Reason::Kept) => vec![KEPT_CHANGED_BODY.into()],
            Some(Reason::More) => vec![
                format!("Since its update, {} also asks for these.", self.name),
                "What you allowed before stays allowed.".into(),
            ],
            None => Vec::new(),
        }
    }

    /// The app's file name (last part of `path`).
    pub fn file(&self) -> String {
        file_of(&self.path)
    }

    /// The three lines under the list. An app that asks for nothing has no list and nothing
    /// to turn off one by one, so its lines speak of the OK and of the app.
    pub fn binding_lines(&self) -> [String; 3] {
        let keeping = self.keep.as_ref().is_some_and(|k| !k.is_refused() && k.on);
        let above = if self.path.is_empty() {
            ""
        } else if self.args.is_empty() {
            " (the file above)"
        } else {
            " (the file and arguments above)"
        };
        if let Some(why) = self.one_run {
            let (file, name) = (shown_file(&self.path), &self.name);
            return [
                format!("Your OK is for this run of {name} only:"),
                match why {
                    OneRun::Scripts => format!(
                        "{file} runs whatever it is given, so Hideout asks again each time \
                         {name} starts."
                    ),
                    OneRun::Windows => format!(
                        "{file} is part of Windows and runs whatever it is given, so Hideout \
                         asks again each time {name} starts."
                    ),
                },
                match self.items.len() {
                    0 => format!("You can turn {name} off any time in Apps."),
                    1 => format!("You can turn it off any time in Apps, {name}."),
                    _ => format!("You can turn each one off any time in Apps, {name}."),
                },
            ];
        }
        match self.items.len() {
            0 => [
                format!("Your OK is only for this copy of {}{above}:", self.name),
                if keeping {
                    KEEP_BINDING_NOTHING.into()
                } else {
                    "If this file changes or it asks for something, Hideout asks again.".into()
                },
                format!("You can turn {} off any time in Apps.", self.name),
            ],
            n => [
                format!("Only for this copy of {}{above}:", self.name),
                if keeping {
                    KEEP_BINDING.into()
                } else {
                    "If this file or this list changes, Hideout asks again.".into()
                },
                if n == 1 {
                    format!("You can turn it off any time in Apps, {}.", self.name)
                } else {
                    format!("You can turn each one off any time in Apps, {}.", self.name)
                },
            ],
        }
    }

    /// Every line under the list, in order: [`PermDialog::binding_lines`], and for an app whose
    /// OK is for one run the next step, after the reason.
    pub fn binding_text(&self) -> Vec<String> {
        let [a, b, c] = self.binding_lines();
        match self.one_run {
            Some(_) => vec![a, b, self.one_run_next(), c],
            None => vec![a, b, c],
        }
    }

    /// What the user can do to be asked only once (one-run apps).
    pub fn one_run_next(&self) -> String {
        format!(
            "To be asked only once, use a version of {} that has its own program file.",
            self.name
        )
    }

    /// "Runs with 2 arguments:", the heading over the arguments; `None` when there are none
    /// (nothing is then added to the dialog).
    pub fn args_heading(&self) -> Option<String> {
        args_heading(self.args.len())
    }

    /// Every argument, whole, as one line in quotes, in order. An empty one is marked, so it
    /// is seen.
    pub fn arg_lines(&self) -> Vec<String> {
        self.args.iter().map(|a| arg_line(a)).collect()
    }

    /// Whether an argument holds something that is not drawn as written ([`ARGS_NOTE`]).
    pub fn args_note(&self) -> bool {
        self.args.iter().any(|a| shown_arg(a).1)
    }

    /// The argument list can be opened and closed: it has more than [`ARGS_SHOWN`] arguments,
    /// or one of them is cut while it is closed. So nothing of an argument is out of reach.
    pub fn args_fold(&self) -> bool {
        self.args.len() > ARGS_SHOWN || self.args.iter().any(|a| arg_is_cut(a))
    }

    /// The link under the arguments, `None` when everything is drawn anyway: "Show all 6" for
    /// a long list, "Show all" when only a cut argument is opened, "Show fewer" to close.
    pub fn args_link(&self) -> Option<String> {
        if !self.args_fold() {
            return None;
        }
        Some(if self.view.args_open {
            SHOW_FEWER.to_string()
        } else if self.args.len() > ARGS_SHOWN {
            show_all(self.args.len())
        } else {
            SHOW_ALL.to_string()
        })
    }

    /// The argument lines drawn now. Opened: every argument, whole. Closed: the first
    /// [`ARGS_SHOWN`], a long one cut in the middle with the number of characters left out.
    pub fn args_drawn(&self) -> Vec<String> {
        if !self.args_fold() || self.view.args_open {
            return self.arg_lines();
        }
        self.args
            .iter()
            .take(ARGS_SHOWN)
            .map(|a| arg_line_cut(a))
            .collect()
    }

    /// The arguments as they are spoken: the heading and every argument in order, whole (never
    /// only the first few, never cut), an empty one as "empty", then the note.
    pub fn args_said(&self) -> Option<String> {
        let mut s = self.args_heading()?;
        for a in &self.args {
            match shown_arg(a).0 {
                x if x.is_empty() && a.is_empty() => s.push_str(" empty"),
                x if x.is_empty() => s.push_str(&format!(" {ARG_UNSEEN}")),
                x => s.push_str(&format!(" \u{201c}{x}\u{201d}")),
            }
        }
        if self.args_note() {
            s.push_str(&format!(" {ARGS_NOTE}"));
        }
        Some(s)
    }

    /// Marks what a changed list added (`new`) and records what it dropped (`removed`),
    /// comparing with the list `old` that was on screen.
    pub fn mark_changes(&mut self, old: &PermDialog) {
        for i in self.items.iter_mut() {
            i.new = !old.items.iter().any(|o| o.raw == i.raw);
        }
        self.removed = old
            .items
            .iter()
            .filter(|o| !self.items.iter().any(|i| i.raw == o.raw))
            .map(|o| o.line.clone())
            .collect();
    }

    /// The items that have a checkbox.
    pub fn allowable(&self) -> impl Iterator<Item = &ListItem> {
        self.items.iter().filter(|i| i.allowable)
    }

    /// How many Allow would grant.
    pub fn count(&self) -> usize {
        self.items.iter().filter(|i| i.counts()).count()
    }

    /// "Allow 9", "Allow and start" when Allow also starts the app, or "Allow none" with
    /// nothing counted. An app that asks for nothing has nothing to count: "Allow" (or
    /// "Allow and start").
    pub fn allow_label(&self) -> String {
        if self.items.is_empty() {
            return if self.view.starts {
                ALLOW_AND_START
            } else {
                ALLOW
            }
            .into();
        }
        match self.count() {
            0 => ALLOW_NONE.into(),
            _ if self.view.starts && self.sensitive_decided() => ALLOW_AND_START.into(),
            n => format!("Allow {n}"),
        }
    }

    /// Ticks or clears the group "I understand": every sensitive item the user can allow is
    /// ticked and confirmed, or cleared. A no-op when the group box is not offered.
    pub fn set_group_ack(&mut self, on: bool) {
        if self.view.group_ack.is_none() {
            return;
        }
        self.view.group_ack = Some(on);
        for i in self.items.iter_mut().filter(|i| i.allowable && i.sensitive) {
            i.checked = on;
            i.confirmed = on;
        }
    }

    /// The group box follows the items: with none of them still ticked it clears itself.
    pub fn sync_group_ack(&mut self) {
        if self.view.group_ack == Some(true)
            && !self
                .items
                .iter()
                .any(|i| i.allowable && i.sensitive && i.checked)
        {
            self.view.group_ack = Some(false);
        }
    }

    /// Whether the group "I understand" is offered and ticked.
    pub fn group_understood(&self) -> bool {
        self.view.group_ack == Some(true)
    }

    /// Whether every sensitive item the user can allow counts (ticked and understood). Until then Allow says how
    /// many it grants: a button that says "and start" would imply the app starts limited.
    pub fn sensitive_decided(&self) -> bool {
        self.items
            .iter()
            .filter(|i| i.allowable && i.sensitive)
            .all(|i| i.counts())
    }

    /// How many everyday items there are: can be allowed and not sensitive (what the All box
    /// covered).
    pub fn everyday(&self) -> usize {
        self.others().count()
    }

    /// Whether the everyday items are listed: the user opened them, or something in them needs
    /// to be seen (a new item, one checked from an earlier answer, or one a command asked for).
    pub fn everyday_shown(&self) -> bool {
        self.view.everyday_open || self.items.iter().any(|i| i.new || i.before || i.asked)
    }

    /// "17 everyday items".
    pub fn everyday_label(&self) -> String {
        everyday_label(self.everyday())
    }

    /// The sensitive items the user can allow.
    pub fn sensitive_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.allowable && i.sensitive)
            .count()
    }

    /// "Foxglove can: use your keyboard and PC lights, read and save its own files, and go
    /// online." Built from the asks, sensitive ones first, never written per app. `None` when
    /// nothing in the list says what the app can do, and for a re-ask of new items only.
    pub fn capability_line(&self) -> Option<String> {
        if self.reason == Some(Reason::More) {
            return None;
        }
        let mut phrases: Vec<String> = Vec::new();
        let asks = |raw: &str| self.items.iter().any(|i| i.allowable && i.raw == raw);
        match (asks("device.hid:keyboard"), asks("device.hid:lighting")) {
            (true, true) => phrases.push("use your keyboard and PC lights".into()),
            (true, false) => phrases.push("use your keyboard".into()),
            (false, true) => phrases.push("use your PC lights".into()),
            _ => {}
        }
        for i in self.items.iter().filter(|i| i.allowable && i.sensitive) {
            if i.raw == "device.hid:keyboard" || i.raw == "device.hid:lighting" {
                continue;
            }
            let p = can_phrase(&i.raw);
            if !phrases.iter().any(|x| x == p) {
                phrases.push(p.into());
            }
        }
        if asks("files") {
            phrases.push(can_phrase("files").into());
        }
        let list = match phrases.len() {
            0 => return None,
            1 => phrases[0].clone(),
            2 => format!("{} and {}", phrases[0], phrases[1]),
            n => format!("{}, and {}", phrases[..n - 1].join(", "), phrases[n - 1]),
        };
        Some(format!("{} can: {list}.", self.name))
    }

    /// "Not signed · C:\path\app.exe": where the app comes from. Hideout does not check an
    /// app it adopted from a folder, so it says so. `None` for an app with no path.
    // TODO: branch on the app's identity class once signed or catalog apps exist; every adopted
    // app is unsigned today.
    pub fn source_line(&self) -> Option<String> {
        (!self.path.is_empty()).then(|| format!("{SOURCE_UNSIGNED} \u{b7} {}", self.path))
    }

    /// The hand-over line (FR5), only while a standalone copy of the app is open.
    pub fn handover_line(&self) -> Option<String> {
        self.view
            .handover
            .then(|| crate::handover::dialog_line(&self.name))
    }

    /// The line under the sensitive group.
    pub fn sensitive_note(&self) -> String {
        let asks = |p: &dyn Fn(&str) -> bool| self.items.iter().any(|i| i.allowable && p(&i.raw));
        let hw = asks(&|r| r.starts_with("device.hid:") || r == "hid.raw");
        let net = asks(&|r| r == "net" || r.starts_with("net."));
        let what = match (hw, net) {
            (true, true) => "your devices and the internet directly",
            (true, false) => "your devices directly",
            (false, true) => "the internet directly",
            (false, false) => "things beyond what Hideout can limit",
        };
        format!(
            "These let {} reach {what}. Hideout can't limit what it does with them.",
            self.name
        )
    }

    /// The items the All box covers: can be allowed and not sensitive. A sensitive item is
    /// only ever set by hand, and it keeps its own "I understand".
    fn others(&self) -> impl Iterator<Item = &ListItem> {
        self.items.iter().filter(|i| i.allowable && !i.sensitive)
    }

    /// Whether the All box has anything to cover (it is hidden when not).
    pub fn has_all(&self) -> bool {
        self.others().next().is_some()
    }

    /// "All 9", or "All 8 other items" when sensitive items sit outside it.
    pub fn all_label(&self) -> String {
        let n = self.others().count();
        if self.allowable().any(|i| i.sensitive) {
            let w = if n == 1 { "item" } else { "items" };
            format!("{ALL_PREFIX} {n} other {w}")
        } else {
            format!("{ALL_PREFIX} {n}")
        }
    }

    pub fn all_state(&self) -> All {
        let n = self.others().count();
        let on = self.others().filter(|i| i.checked).count();
        match on {
            0 => All::None,
            x if x == n => All::All,
            _ => All::Some,
        }
    }

    /// The All box: checks every non-sensitive item, or clears them when they are all checked.
    /// Sensitive items are left exactly as the user set them.
    pub fn toggle_all(&mut self) {
        let to = self.all_state() != All::All;
        for i in self
            .items
            .iter_mut()
            .filter(|i| i.allowable && !i.sensitive)
        {
            i.checked = to;
        }
    }

    /// Unchecking a sensitive item also clears its "I understand".
    pub fn set_checked(&mut self, raw: &str, on: bool) {
        if let Some(i) = self.items.iter_mut().find(|i| i.raw == raw && i.allowable) {
            i.checked = on;
            if !on {
                i.confirmed = false;
            }
        }
    }

    /// The items of a group, in order.
    pub fn group(&self, g: Group) -> impl Iterator<Item = &ListItem> {
        self.items.iter().filter(move |i| i.group == g)
    }

    /// What Allow sends: the counted items, and every other item of the list that can be
    /// allowed as declined (so the module can run with nothing, §4). Never-listed items are the
    /// registry's to record.
    pub fn allowed(&self) -> Allowed {
        let chosen: Vec<String> = self
            .items
            .iter()
            .filter(|i| i.counts())
            .map(|i| i.raw.clone())
            .collect();
        let declined = self
            .items
            .iter()
            .filter(|i| i.allowable && !i.counts())
            .map(|i| i.raw.clone())
            .collect();
        Allowed {
            code: self.code.clone(),
            chosen,
            declined,
            keyboard: self.keyboard.as_ref().is_some_and(|k| k.on),
            keep: self.keep.as_ref().is_some_and(Keep::active),
        }
    }

    /// The accessible description: intro plus the binding lines, and the state of the keep box.
    pub fn said(&self) -> String {
        let lines = self.binding_text();
        let mut s = format!("{} {} {}", self.intro(), lines[0], self.path);
        if let Some(args) = self.args_said() {
            s.push_str(&format!(" {args}"));
        }
        for line in &lines[1..] {
            s.push_str(&format!(" {line}"));
        }
        if let Some(k) = &self.keep {
            s.push_str(&format!(
                " {KEEP_LABEL}: {}.",
                if k.on { "on" } else { "off" }
            ));
        }
        s
    }
}

/// What a permission lets the app do, in the words of "<name> can: ...". Every sensitive ask
/// has its own phrase; one this table does not know says so rather than hiding it.
pub fn can_phrase(raw: &str) -> &'static str {
    match raw {
        "device.hid:keyboard" => "use your keyboard",
        "device.hid:lighting" => "use your PC lights",
        "device.hid:vendor" => "talk to other devices from the same maker",
        "hid.raw" => "talk to your devices directly",
        "key.events" => "see every key you press",
        "files" => "read and save its own files",
        r if r == "net" || r.starts_with("net.") => "go online",
        r if r.starts_with("provide:gpu.") => "answer every app's AI requests",
        r if r.starts_with("ai.ask:") => "send questions to an AI service",
        r if r.starts_with("secrets.ref:") => "use a secret you stored",
        _ => "do something Hideout can't describe in a line",
    }
}

/// "17 everyday items" (the heading of the collapsed list).
pub fn everyday_label(n: usize) -> String {
    format!("{n} everyday item{}", if n == 1 { "" } else { "s" })
}

/// "These need your OK (4)": the heading of the sensitive group in the dialog.
pub fn needs_ok_heading(n: usize) -> String {
    format!("These need your OK ({n})")
}

/// The keyboard section's words.
pub fn keyboard_line(name: &str, on: bool) -> String {
    format!(
        "Also let {name} handle your keyboard for other apps ({})",
        if on { "on" } else { "off" }
    )
}

/// How an item stands in the Permissions section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemWord {
    Allowed,
    Off,
    CantBeAllowed,
    /// Asked (by an update, say) and never answered: not the same as turned off (#1238).
    Waits,
}

impl ItemWord {
    pub fn word(self) -> &'static str {
        match self {
            ItemWord::Allowed => "Allowed",
            ItemWord::Off => "Off",
            ItemWord::CantBeAllowed => "Can't be allowed",
            ItemWord::Waits => WAITS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionItem {
    pub raw: String,
    pub line: String,
    pub group: Group,
    pub word: ItemWord,
}

/// The Permissions section of one module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub items: Vec<SectionItem>,
    /// "Allowed in Hideout on 12 Nov 2026" / "Allowed from the command line on ...".
    pub allowed_on: Option<String>,
    /// "Updated to 0.3.1 on 12 Nov 2026. Your OK stayed." per attested update, oldest first.
    pub history: Vec<String>,
    /// "Moved or removed on ..." while a kept binding is void; only then.
    pub kept_void: Option<String>,
    /// The "Keep my OK after updates in this folder" row; `None` for a module with no app path.
    pub keep: Option<KeepRow>,
    /// Something Off or waiting can be turned on in the dialog.
    pub can_change: bool,
    /// A line under the section (a turn-off's consequence, or a failure).
    pub note: Option<String>,
}

impl Section {
    pub fn allowed(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.word == ItemWord::Allowed)
            .count()
    }

    pub fn heading(&self) -> String {
        format!("{} \u{b7} {SECTION_LABEL}", self.name)
    }

    /// The group's accessible name ("Make changes when an app asks, 4 items").
    pub fn group_said(&self, g: Group) -> String {
        let n = self.items.iter().filter(|i| i.group == g).count();
        format!("{}, {n} item{}", g.heading(), if n == 1 { "" } else { "s" })
    }
}

/// First run (UX cut 3): several adopted apps wait for their first OK. One header on the
/// Modules page, one dialog with a section per app. Plain words ("app"); nothing here
/// pre-confirms or allows anything.
pub const FIRST_RUN_BODY: &str =
    "Each app stays off until you allow it, and starts when you do. You review each app's permissions one by one.";
pub const FIRST_RUN_INTRO: &str = "Each app has its own list. Nothing is allowed until you press Allow, and each sensitive item needs its own “I understand”.";

/// "2 apps wait for your OK".
pub fn first_run_title(n: usize) -> String {
    format!("{n} apps wait for your OK")
}

/// The one header line for apps that wait for their first OK.
pub fn first_run_header(n: usize) -> PermHeader {
    header(HeaderKind::FirstRun, "", "", Some((n, "")))
}

/// The first-run dialog: one section per app, each the app's own permission dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstRun {
    pub sections: Vec<PermDialog>,
    /// An inline failure under the buttons.
    pub error: Option<String>,
}

impl FirstRun {
    pub fn title(&self) -> String {
        let n = self.sections.len();
        format!("Let {n} apps run in Hideout?")
    }

    /// How many items Allow would grant across the sections.
    pub fn count(&self) -> usize {
        self.sections.iter().map(PermDialog::count).sum()
    }

    /// "Allow 9 items", or "Allow none" with nothing counted (the same rule as one dialog);
    /// "Allow" when no app asks for anything.
    pub fn allow_label(&self) -> String {
        match self.count() {
            0 if self.sections.iter().all(|s| s.items.is_empty()) => ALLOW.into(),
            0 => ALLOW_NONE.into(),
            1 => "Allow 1 item".into(),
            n => format!("Allow {n} items"),
        }
    }

    /// What Allow sends, per section: each section's own list code and its own counted items.
    pub fn allowed(&self) -> Vec<(String, Allowed)> {
        self.sections
            .iter()
            .map(|d| (d.id.clone(), d.allowed()))
            .collect()
    }

    /// The accessible description: the intro and the names of the apps.
    pub fn said(&self) -> String {
        let names: Vec<&str> = self.sections.iter().map(|d| d.name.as_str()).collect();
        format!("{FIRST_RUN_INTRO} {}.", names.join(", "))
    }
}

/// Which header line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HeaderKind {
    /// Adopted and started without the user's act, never asked.
    Ask,
    /// The program changed or moved since the allow.
    Changed,
    /// The app file changed under "Keep my OK" (#1465): same heading as [`HeaderKind::Changed`],
    /// but the app runs with what carried over.
    ChangedKept,
    /// Allowed from the command line, not yet seen.
    Cli,
    /// A command-line grant waits for the user's confirmation (#1183); nothing is allowed yet.
    Queued,
    /// Allowed, and a signed update asks for more (#1238); `cli` carries (count, first line).
    More,
    /// A rebuilt dev app asks for more (rebuild R6): `cli` carries (count, first line). The app
    /// keeps running with what was allowed; the title counts the new things.
    MoreBuild,
    /// Two or more adopted apps wait for their first OK: one header, one dialog (UX cut 3).
    FirstRun,
    /// A program outside Hideout asks for more than it may have (S1): the hub's own line, from
    /// the router's queue; `name` is the program's own word for itself, quoted when drawn.
    Tool,
}

/// A hub-drawn header line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermHeader {
    pub id: String,
    pub kind: HeaderKind,
    pub title: String,
    pub body: String,
    pub review: &'static str,
    pub dismiss: &'static str,
}

pub fn header(kind: HeaderKind, id: &str, name: &str, cli: Option<(usize, &str)>) -> PermHeader {
    let (title, body, dismiss) = match kind {
        HeaderKind::Ask => (
            format!("{name} needs your OK to run in Hideout"),
            format!("Until you allow it, {name} stays off. Allow turns it on."),
            NOT_NOW,
        ),
        HeaderKind::Changed => (
            format!("{name} changed since you allowed it"),
            "Its app file is new or was moved, so Hideout stopped it until you allow it again."
                .into(),
            NOT_NOW,
        ),
        HeaderKind::ChangedKept => (
            format!("{name} changed since you allowed it"),
            KEPT_CHANGED_BODY.into(),
            NOT_NOW,
        ),
        // No count: Review opens the whole list checked as usual, so what Allow covers is
        // the dialog's own count, not how many items the command named.
        HeaderKind::Queued => (
            format!("A command asked for permissions for {name}"),
            QUEUED_BODY.into(),
            NOT_NOW,
        ),
        HeaderKind::More => {
            let (n, first) = cli.unwrap_or((0, ""));
            let body = if n == 1 {
                format!("Its update asks for one new thing: “{first}”. It stays off until you allow it, so apps that ask for it are told no.")
            } else {
                format!("Its update asks for {n} new things, like “{first}”. They stay off until you allow them, so apps that ask for them are told no.")
            };
            (format!("{name}'s update asks for more"), body, NOT_NOW)
        }
        HeaderKind::MoreBuild => {
            let (n, _) = cli.unwrap_or((0, ""));
            (
                // TODO(S2): once the registry diff counts only the build's new items, switch
                // back to `build_asks_for_more(name, new_only)`, and keep this wording when
                // `new_only == 0` and leftovers remain. Until then the origin is unknown.
                hideout_supervisor::words::waiting_for_ok(name, n),
                hideout_supervisor::words::RUNS_WITH_WHAT_YOU_ALLOWED.into(),
                NOT_NOW,
            )
        }
        HeaderKind::Tool => (
            crate::tool_dialog::header_title(cli.map_or(0, |c| c.0)),
            crate::tool_dialog::header_body(name),
            NOT_NOW,
        ),
        HeaderKind::FirstRun => (
            first_run_title(cli.map_or(0, |c| c.0)),
            FIRST_RUN_BODY.into(),
            NOT_NOW,
        ),
        HeaderKind::Cli => {
            let (n, when) = cli.unwrap_or((0, ""));
            (
                format!("{name} was allowed from the command line"),
                format!("{n} permissions, {when}."),
                OK,
            )
        }
    };
    PermHeader {
        id: id.into(),
        kind,
        title,
        body,
        review: REVIEW,
        dismiss,
    }
}

/// The note after turning one item off, where the consequence is worth a line.
pub fn turned_off_note(name: &str, raw: &str) -> Option<String> {
    (raw == "facts.write:keyboard.*")
        .then(|| format!("{name} can't share its status. Its tiles stay empty."))
}

/// The line under the section: only while the status item is Off, never while it is Allowed.
pub fn status_note(name: &str, items: &[SectionItem]) -> Option<String> {
    items
        .iter()
        .find(|i| i.raw == "facts.write:keyboard.*" && i.word == ItemWord::Off)
        .and_then(|i| turned_off_note(name, &i.raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(raw: &str, g: Group, sensitive: bool, checked: bool) -> ListItem {
        ListItem {
            raw: raw.into(),
            line: format!("line of {raw}"),
            group: g,
            allowable: true,
            sensitive,
            reason: None,
            checked,
            confirmed: false,
            new: false,
            before: false,
            asked: false,
        }
    }

    fn dialog() -> PermDialog {
        PermDialog {
            id: "foxglove".into(),
            name: "Foxglove".into(),
            code: "7f3a91c20b4e55d1".into(),
            path: r"C:\Program Files\Foxglove\foxglove.exe".into(),
            args: Vec::new(),
            one_run: None,
            items: vec![
                item("key.events", Group::Sensitive, true, false),
                item("provide:a.set", Group::MakeChanges, false, true),
                item("provide:b.read", Group::AnswerQuestions, false, true),
                ListItem {
                    allowable: false,
                    checked: false,
                    reason: Some("Hideout never allows this for this app.".into()),
                    ..item("modules.manage", Group::CantBeAllowed, false, false)
                },
            ],
            reason: None,
            keyboard: Some(KeyboardChoice {
                on: false,
                expanded: false,
            }),
            keep: None,
            removed: Vec::new(),
            same_place: false,
            view: Default::default(),
            error: None,
        }
    }

    /// An app that asks for nothing: no "these 0 things", no "Allow none", no "each one".
    #[test]
    fn an_app_that_asks_for_nothing_reads_plainly() {
        let mut d = dialog();
        d.items.clear();
        assert_eq!(
            d.intro(),
            "Foxglove doesn't ask for anything. Nothing leaves this PC."
        );
        assert_eq!(d.allow_label(), "Allow");
        assert_eq!(
            d.binding_lines(),
            [
                "Your OK is only for this copy of Foxglove (the file above):",
                "If this file changes or it asks for something, Hideout asks again.",
                "You can turn Foxglove off any time in Apps.",
            ]
        );
        assert!(!d.has_all() && !d.everyday_shown());
        d.keep = Some(Keep::offered(true));
        assert_eq!(
            d.binding_lines()[1],
            "If the app asks for something or moves, Hideout asks again."
        );
        d.path.clear();
        assert_eq!(
            d.binding_lines()[0],
            "Your OK is only for this copy of Foxglove:"
        );
        d.view.starts = true;
        assert_eq!(d.allow_label(), "Allow and start");
        assert_eq!(
            keep_helper_nothing("Foxglove", "foxglove.exe"),
            "A new build of Foxglove saved here as foxglove.exe keeps your OK. Hideout asks \
             again if the new build asks for something, or if the app moves."
        );
    }

    /// One item is "1 thing" and "it"; a list with nothing ticked still says "Allow none".
    #[test]
    fn an_app_that_asks_for_one_thing_says_it_in_the_singular() {
        let mut d = dialog();
        d.items.truncate(2);
        d.items.remove(0);
        assert_eq!(
            d.intro(),
            "Foxglove asks for 1 thing. Each app that uses it still needs its own permission. \
             Nothing leaves this PC."
        );
        assert_eq!(d.allow_label(), "Allow 1");
        assert_eq!(
            d.binding_lines()[2],
            "You can turn it off any time in Apps, Foxglove."
        );
        d.items[0].checked = false;
        assert_eq!(d.allow_label(), "Allow none");
    }

    #[test]
    fn the_words_follow_the_plan() {
        let d = dialog();
        assert_eq!(d.title(), "Let Foxglove run in Hideout?");
        assert_eq!(
            d.intro(),
            "Foxglove asks for these 4 things. 1 can't be allowed. Each app that uses them \
             still needs its own permission. Nothing leaves this PC."
        );
        assert_eq!(d.allow_label(), "Allow 2");
        assert_eq!(d.all_label(), "All 2 other items");
        assert_eq!(
            d.binding_lines()[2],
            "You can turn each one off any time in Apps, Foxglove."
        );
        assert_eq!(
            keyboard_line("Foxglove", false),
            "Also let Foxglove handle your keyboard for other apps (off)"
        );
        // The counts agree: total, can't be allowed, the All box and the button.
        assert_eq!(d.items.len() - d.allowable().count(), 1);
        let mut n = dialog();
        n.items.push(item("net", Group::Sensitive, true, false));
        assert!(!n.intro().contains("Nothing leaves this PC."));
    }

    #[test]
    fn a_sensitive_item_counts_only_when_checked_and_understood() {
        let mut d = dialog();
        d.set_checked("key.events", true);
        assert_eq!(d.count(), 2, "checked but not understood");
        d.items[0].confirmed = true;
        assert_eq!(d.count(), 3);
        d.set_checked("key.events", false);
        assert!(!d.items[0].confirmed, "unchecking clears the understanding");
        let a = d.allowed();
        assert_eq!(a.chosen, ["provide:a.set", "provide:b.read"]);
        assert_eq!(a.declined, ["key.events"]);
        assert!(!a.keyboard);
    }

    #[test]
    fn nothing_checked_reads_allow_none_and_declines_everything_asked() {
        let mut d = dialog();
        d.toggle_all();
        assert_eq!(d.all_state(), All::None);
        assert_eq!(d.allow_label(), "Allow none");
        let a = d.allowed();
        assert!(a.chosen.is_empty());
        assert_eq!(a.declined.len(), 3);
        assert!(!a.declined.contains(&"modules.manage".to_string()));
    }

    #[test]
    fn the_all_box_skips_sensitive_items() {
        let mut d = dialog();
        d.toggle_all(); // both others were on: clears them
        assert!(!d.items[1].checked && !d.items[2].checked);
        d.toggle_all();
        assert!(!d.items[0].checked, "key.events stays off");
        assert!(d.items[1].checked && d.items[2].checked);
        d.toggle_all();
        assert!(!d.items[0].checked);
        // The user's own choice on a sensitive item is left alone, both ways.
        d.set_checked("key.events", true);
        d.items[0].confirmed = true;
        d.toggle_all();
        assert!(d.items[0].checked && d.items[0].confirmed);
        d.toggle_all();
        assert!(d.items[0].checked && d.items[0].confirmed);
        d.set_checked("key.events", false);
        d.toggle_all();
        assert!(!d.items[0].checked);
        // Unknown kinds arrive sensitive (describe) and stay off.
        d.items
            .push(item("what.is.this", Group::Sensitive, true, false));
        d.toggle_all();
        assert!(!d.items.last().unwrap().checked);
        d.toggle_all();
        assert!(!d.items.last().unwrap().checked);
        assert_eq!(d.all_label(), "All 2 other items");
        assert_eq!(d.all_state(), All::All);
    }

    #[test]
    fn allow_with_all_checked_grants_no_sensitive_without_understanding() {
        let mut d = dialog();
        d.toggle_all();
        d.toggle_all();
        d.items[0].checked = true; // checked by hand, not understood
        d.toggle_all();
        d.toggle_all();
        let a = d.allowed();
        assert_eq!(a.chosen, ["provide:a.set", "provide:b.read"]);
        assert_eq!(a.declined, ["key.events"]);
    }

    #[test]
    fn the_all_box_hides_with_no_other_items_and_reads_plain_without_sensitive() {
        let mut d = dialog();
        assert!(d.has_all());
        d.items.retain(|i| i.sensitive || !i.allowable);
        assert!(!d.has_all());
        d.toggle_all();
        assert!(!d.items[0].checked);
        let mut e = dialog();
        e.items.retain(|i| !i.sensitive);
        assert_eq!(e.all_label(), "All 2");
    }

    #[test]
    fn re_ask_before_items_stay_checked_and_the_all_box_counts_them() {
        let mut d = dialog();
        d.items[1].before = true;
        d.items[2].checked = false;
        assert!(d.items[1].checked && d.items[1].before);
        assert_eq!(d.all_state(), All::Some);
        d.toggle_all();
        assert!(d.items[1].checked && d.items[2].checked && !d.items[0].checked);
    }

    #[test]
    fn the_all_box_is_tri_state() {
        let mut d = dialog();
        assert_eq!(d.all_state(), All::All);
        d.set_checked("provide:b.read", false);
        assert_eq!(d.all_state(), All::Some);
        d.toggle_all();
        assert_eq!(d.all_state(), All::All);
        d.toggle_all();
        assert_eq!(d.all_state(), All::None);
    }

    /// An item a command asked for says so, after any other mark, in the words drawn on it.
    #[test]
    fn an_asked_item_says_asked_by_the_command() {
        let mut i = item("key.events", Group::Sensitive, true, false);
        assert!(!i.said().contains("asked by a command"));
        i.asked = true;
        assert_eq!(
            i.said(),
            "Sensitive: line of key.events, sensitive, asked by a command, not checked"
        );
        assert!(i.said().contains(&ASKED_MARK.to_lowercase()));
        i.new = true;
        assert!(i.said().contains(", new, asked by a command"));
    }

    /// An everyday item a command asked for is never left behind "Show", and the lines under
    /// the reason say what the mark means.
    #[test]
    fn an_asked_everyday_item_is_listed_and_explained() {
        let mut d = dialog();
        d.items = vec![
            item("hub.notify", Group::ShareStatus, false, true),
            item("ui.window", Group::ShareStatus, false, false),
        ];
        assert!(!d.everyday_shown() && d.before_lines().is_empty());
        d.items[1].asked = true;
        assert!(d.everyday_shown());
        assert_eq!(d.before_lines(), ASKED_LINES.map(String::from).to_vec());
        assert!(ASKED_LINES[0].contains(ASKED_MARK));
        d.items[0].before = true;
        assert_eq!(
            d.before_lines(),
            vec![BEFORE_LINES[0], ASKED_LINES[0], ASKED_LINES[1]]
        );
    }

    /// The Apps page line for a command's request gives no count: the dialog's own count is
    /// the true one.
    #[test]
    fn a_commands_request_line_gives_no_count() {
        let h = header(HeaderKind::Queued, "m", "Sample", Some((3, "")));
        assert_eq!(h.title, "A command asked for permissions for Sample");
        assert_eq!(h.body, QUEUED_BODY);
        assert!(!h.title.contains('3') && !h.body.contains('3'));
    }

    #[test]
    fn items_say_group_line_risk_and_state() {
        let mut d = dialog();
        assert_eq!(
            d.items[1].said(),
            "Make changes when an app asks: line of provide:a.set, checked"
        );
        d.items[0].checked = true;
        assert_eq!(
            d.items[0].said(),
            "Sensitive: line of key.events, sensitive, checked"
        );
        assert!(d.items[0]
            .confirm_line("Foxglove")
            .contains("see every key you press"));
        let gpu = item("provide:gpu.*", Group::Sensitive, true, false);
        assert_eq!(
            gpu.confirm_line("Gpu Helper"),
            "line of provide:gpu.*. This lets Gpu Helper see every AI request from every app, including what you type, and answer them."
        );
    }

    #[test]
    fn reasons_and_header_lines_use_the_exact_words() {
        let mut d = dialog();
        d.reason = Some(Reason::ListChanged);
        assert_eq!(d.reason_lines(), LIST_CHANGED);
        d.reason = Some(Reason::Void);
        assert_eq!(
            d.reason_lines(),
            ["Foxglove's app file is new or was moved since you allowed it."]
        );
        let h = header(HeaderKind::Changed, "foxglove", "Foxglove", None);
        assert_eq!(h.title, "Foxglove changed since you allowed it");
        assert_eq!(
            h.body,
            "Its app file is new or was moved, so Hideout stopped it until you allow it again."
        );
        let h = header(HeaderKind::Ask, "foxglove", "Foxglove", None);
        assert_eq!(
            h.body,
            "Until you allow it, Foxglove stays off. Allow turns it on."
        );
        let h = header(
            HeaderKind::Cli,
            "foxglove",
            "Foxglove",
            Some((13, "12 Nov 2026")),
        );
        assert_eq!(h.title, "Foxglove was allowed from the command line");
        assert_eq!(h.body, "13 permissions, 12 Nov 2026.");
        assert_eq!((h.review, h.dismiss), ("Review\u{2026}", "OK"));
    }

    #[test]
    fn a_changed_list_marks_new_items_and_names_the_dropped_ones() {
        let old = dialog();
        let mut now = dialog();
        now.items.retain(|i| i.raw != "provide:a.set");
        now.items
            .push(item("provide:c.set", Group::MakeChanges, false, true));
        now.reason = Some(Reason::ListChanged);
        now.mark_changes(&old);
        let new: Vec<_> = now
            .items
            .iter()
            .filter(|i| i.new)
            .map(|i| &i.raw[..])
            .collect();
        assert_eq!(new, ["provide:c.set"]);
        assert_eq!(now.removed, ["line of provide:a.set"]);
        let lines = now.reason_lines();
        assert_eq!(
            lines.last().unwrap(),
            "No longer asked: line of provide:a.set"
        );
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn the_status_line_shows_only_while_the_item_is_off() {
        let mk = |word| {
            vec![SectionItem {
                raw: "facts.write:keyboard.*".into(),
                line: "Share status".into(),
                group: Group::ShareStatus,
                word,
            }]
        };
        assert!(status_note("Foxglove", &mk(ItemWord::Allowed)).is_none());
        assert_eq!(
            status_note("Foxglove", &mk(ItemWord::Off)).as_deref(),
            Some("Foxglove can't share its status. Its tiles stay empty.")
        );
    }

    fn kept() -> PermDialog {
        PermDialog {
            keep: Some(Keep::offered(false)),
            ..dialog()
        }
    }

    #[test]
    fn keep_is_off_by_default_and_sent_only_when_checked_and_understood() {
        let mut d = kept();
        let k = d.keep.as_ref().unwrap();
        assert!(!k.on && !k.understood && !k.active());
        assert!(!d.allowed().keep);
        d.keep.as_mut().unwrap().set_on(true);
        assert!(!d.allowed().keep, "checked but not understood");
        d.keep.as_mut().unwrap().understood = true;
        assert!(d.allowed().keep);
        d.keep.as_mut().unwrap().set_on(false);
        let k = d.keep.as_ref().unwrap();
        assert!(!k.understood, "unchecking clears the understanding");
        assert!(!d.allowed().keep);
        // No keep section at all (a module with no app path): never sent.
        assert!(!dialog().allowed().keep);
    }

    #[test]
    fn a_refused_keep_never_sends_and_stays_off() {
        let mut d = PermDialog {
            keep: Some(Keep::refused()),
            ..dialog()
        };
        let k = d.keep.as_mut().unwrap();
        k.set_on(true);
        k.understood = true;
        assert!(!k.on, "a refused box cannot be checked");
        assert!(!d.allowed().keep);
        assert_eq!(
            d.keep
                .as_ref()
                .unwrap()
                .description("Foxglove", "foxglove.exe"),
            "Hideout keeps your OK only for a folder on this PC's own drive."
        );
        assert_eq!(
            d.binding_lines()[1],
            "If this file or this list changes, Hideout asks again."
        );
    }

    #[test]
    fn a_live_choice_starts_checked_and_understood() {
        let d = PermDialog {
            keep: Some(Keep::offered(true)),
            ..dialog()
        };
        assert!(
            d.allowed().keep,
            "an Allow that leaves the box alone keeps it"
        );
        assert!(d.said().ends_with(
            "Keep my OK after updates in this folder, except Sensitive and keyboard: on."
        ));
    }

    #[test]
    fn the_binding_line_changes_while_the_box_is_checked() {
        let mut d = kept();
        assert_eq!(
            d.binding_lines()[1],
            "If this file or this list changes, Hideout asks again."
        );
        d.keep.as_mut().unwrap().set_on(true);
        assert_eq!(
            d.binding_lines()[1],
            "If the app asks for more or moves, Hideout asks again."
        );
        assert_eq!(
            d.binding_lines()[2],
            "You can turn each one off any time in Apps, Foxglove."
        );
    }

    #[test]
    fn the_dialog_says_the_state_of_the_box() {
        let mut d = kept();
        let plain = dialog().said();
        assert_eq!(
            d.said(),
            format!("{plain} Keep my OK after updates in this folder, except Sensitive and keyboard: off.")
        );
        d.keep.as_mut().unwrap().set_on(true);
        assert!(d.said().ends_with(
            "Keep my OK after updates in this folder, except Sensitive and keyboard: on."
        ));
        let k = d.keep.as_ref().unwrap();
        assert_eq!(
            k.said(),
            "Keep my OK after updates in this folder, except Sensitive and keyboard, checked"
        );
        assert_eq!(
            Keep::offered(false).said(),
            "Keep my OK after updates in this folder, except Sensitive and keyboard, not checked"
        );
    }

    #[test]
    fn the_keep_words_use_the_exact_copy() {
        assert_eq!(file_of(r"C:\Apps\Fox\foxglove.exe"), "foxglove.exe");
        assert_eq!(
            keep_helper("Foxglove", "foxglove.exe"),
            "A new build of Foxglove saved here as foxglove.exe keeps what you allowed, except \
             Sensitive items and the keyboard. Hideout asks again for those, and if the new \
             build wants more or the app moves."
        );
        assert_eq!(
            keep_warning("Foxglove", "foxglove.exe"),
            "Any app saved in this folder as foxglove.exe gets what you allowed Foxglove, \
             except Sensitive items and the keyboard. Only do this for a folder where only you \
             put apps."
        );
        assert_eq!(
            keep_understand_said("Foxglove", "foxglove.exe"),
            "I understand: any app saved in this folder as foxglove.exe gets what you allowed \
             Foxglove, except Sensitive items and the keyboard"
        );
        assert_eq!(
            keep_off_note("Foxglove"),
            "Off. A new build of Foxglove will need your OK again."
        );
        assert_eq!(keep_allow_first("Foxglove"), "Allow Foxglove first.");
        assert_eq!(
            keep_changed_open("Foxglove"),
            "Foxglove changed while you were choosing. Look at its list again, then choose."
        );
        assert_eq!(KEEP_CONFIRM_TITLE, "Keep your OK for this folder?");
        assert_eq!((KEEP_CONFIRM_OK, CANCEL), ("Keep my OK", "Cancel"));
        assert_eq!(earlier_label(4), "and 4 earlier");
    }

    #[test]
    fn the_history_says_the_time_and_what_waits() {
        assert_eq!(
            keep_history("17 Nov 2026, 20:40", 0),
            "New build on 17 Nov 2026, 20:40. Your OK stayed, except Sensitive and keyboard."
        );
        assert_eq!(
            keep_history("17 Nov 2026, 20:40", 2),
            "New build on 17 Nov 2026, 20:40. Your OK stayed, except Sensitive and keyboard; 2 \
             things wait for it."
        );
        assert_eq!(
            keep_history("17 Nov 2026, 20:40", 1),
            "New build on 17 Nov 2026, 20:40. Your OK stayed, except Sensitive and keyboard; 1 \
             thing waits for it."
        );
        assert_eq!(
            keep_void_line(true, "17 Nov 2026, 20:40"),
            "Moved or removed on 17 Nov 2026, 20:40. Keeping your OK after updates is now off."
        );
        assert_eq!(
            keep_void_line(false, "17 Nov 2026, 20:40"),
            "Starts in a new way since 17 Nov 2026, 20:40. Keeping your OK after updates is now \
             off."
        );
    }

    #[test]
    fn the_keep_row_says_state_button_and_folder() {
        let row = |state| KeepRow {
            state,
            name: "Foxglove".into(),
            file: "foxglove.exe".into(),
            folder: r"C:\Apps\Fox".into(),
        };
        let on = row(KeepState::On);
        assert_eq!((on.word(), on.button()), ("On", Some("Turn off")));
        assert_eq!(
            on.said(),
            r"Keep my OK after updates in this folder, except Sensitive and keyboard, on, C:\Apps\Fox"
        );
        assert_eq!(on.button_said(), Some("Turn off keep my OK after updates"));
        let off = row(KeepState::Off);
        assert_eq!((off.word(), off.button()), ("Off", Some("Turn on\u{2026}")));
        assert_eq!(
            off.button_said(),
            Some("Turn on keep my OK after updates, opens a confirm")
        );
        assert!(!off.button_said().unwrap().contains('\u{2026}'));
        let no = row(KeepState::Unavailable(keep_allow_first("Foxglove")));
        assert_eq!((no.word(), no.button()), ("Off", None));
        assert_eq!(no.helper(), "Allow Foxglove first.");
        assert_eq!(off.helper(), keep_helper("Foxglove", "foxglove.exe"));
    }

    #[test]
    fn the_keep_confirm_names_the_folder_and_what_it_means() {
        let c = KeepConfirm {
            id: "foxglove".into(),
            name: "Foxglove".into(),
            file: "foxglove.exe".into(),
            folder: r"C:\Apps\Fox".into(),
            code: "7f3a91c20b4e55d1".into(),
            error: None,
        };
        assert_eq!(c.line(), keep_warning("Foxglove", "foxglove.exe"));
        assert_eq!(c.said(), format!("{} C:\\Apps\\Fox", c.line()));
    }

    /// The new strings use the plain words: never "program", "hash", "grant", "follow", "bound"
    /// or an id.
    #[test]
    fn the_keep_strings_keep_to_plain_words() {
        let all = [
            KEEP_LABEL.to_string(),
            KEEP_REFUSED.into(),
            KEEP_BINDING.into(),
            KEEP_CONFIRM_TITLE.into(),
            KEEP_CONFIRM_OK.into(),
            CANCEL.into(),
            KEEP_TURN_ON_SAID.into(),
            KEEP_TURN_OFF_SAID.into(),
            KEEP_NOT_SAVED.into(),
            keep_helper("Foxglove", "foxglove.exe"),
            keep_warning("Foxglove", "foxglove.exe"),
            keep_understand_said("Foxglove", "foxglove.exe"),
            keep_off_note("Foxglove"),
            keep_allow_first("Foxglove"),
            keep_changed_open("Foxglove"),
            keep_history("17 Nov 2026, 20:40", 0),
            keep_history("17 Nov 2026, 20:40", 3),
            keep_void_line(true, "17 Nov 2026, 20:40"),
            keep_void_line(false, "17 Nov 2026, 20:40"),
            earlier_label(3),
        ];
        for t in all {
            let l = t.to_lowercase();
            for bad in ["program", "hash", "grant", "follow", "bound", "sha"] {
                assert!(!l.contains(bad), "{bad:?} in {t:?}");
            }
        }
    }

    fn fr2_dialog(raws: &[(&str, bool)]) -> PermDialog {
        let mut d = dialog();
        d.items = raws
            .iter()
            .map(|(r, sens)| {
                let g = if *sens {
                    Group::Sensitive
                } else {
                    Group::MakeChanges
                };
                item(r, g, *sens, !*sens)
            })
            .collect();
        d
    }

    #[test]
    fn the_capability_line_is_built_from_the_asks_sensitive_first() {
        let d = fr2_dialog(&[
            ("files", false),
            ("net", true),
            ("device.hid:lighting", true),
            ("device.hid:keyboard", true),
        ]);
        assert_eq!(
            d.capability_line().unwrap(),
            "Foxglove can: use your keyboard and PC lights, go online, and read and save its own files."
        );
        let one = fr2_dialog(&[("net", true)]);
        assert_eq!(one.capability_line().unwrap(), "Foxglove can: go online.");
        let two = fr2_dialog(&[("net", true), ("device.hid:keyboard", true)]);
        assert_eq!(
            two.capability_line().unwrap(),
            "Foxglove can: use your keyboard and go online."
        );
        // Nothing to say, and a re-ask of new items only: no line.
        assert_eq!(fr2_dialog(&[("ui.window", false)]).capability_line(), None);
        let mut more = d.clone();
        more.reason = Some(Reason::More);
        assert_eq!(more.capability_line(), None);
    }

    /// Every sensitive ask is in the line, whatever it is (no product list behind it).
    #[test]
    fn the_capability_line_covers_every_sensitive_ask() {
        for raw in [
            "device.hid:keyboard",
            "device.hid:lighting",
            "device.hid:vendor",
            "hid.raw",
            "key.events",
            "net",
            "net.example",
            "ai.ask:chat",
            "secrets.ref:token",
            "provide:gpu.*",
            "device.hid:something-new",
        ] {
            let d = fr2_dialog(&[(raw, true)]);
            let line = d.capability_line().unwrap_or_default();
            let want = match raw {
                "device.hid:keyboard" | "device.hid:lighting" => can_phrase(raw),
                _ => can_phrase(raw),
            };
            assert!(line.contains(want), "{raw}: {line}");
        }
        let d = fr2_dialog(&[
            ("device.hid:keyboard", true),
            ("device.hid:lighting", true),
            ("device.hid:vendor", true),
        ]);
        let line = d.capability_line().unwrap();
        assert!(
            line.contains("keyboard") && line.contains("PC lights") && line.contains("same maker")
        );
    }

    #[test]
    fn the_fr2_words_are_plain_and_name_no_product() {
        let mut d = fr2_dialog(&[
            ("device.hid:keyboard", true),
            ("device.hid:lighting", true),
            ("device.hid:vendor", true),
            ("net", true),
            ("files", false),
            ("facts.write:keyboard.*", false),
        ]);
        d.name = "Gizmo Keys".into();
        d.path = r"C:\Apps\Gizmo\gizmo.exe".into();
        let mut all = vec![
            d.capability_line().unwrap(),
            d.sensitive_note(),
            d.source_line().unwrap(),
            d.everyday_label(),
            d.title(),
            d.intro(),
            "Hideout never allows this for this app.".to_string(),
            needs_ok_heading(4),
            ALLOW_AND_START.to_string(),
            SHOW.to_string(),
            HIDE.to_string(),
        ];
        for r in [
            "device.hid:keyboard",
            "device.hid:lighting",
            "device.hid:vendor",
            "net",
            "files",
        ] {
            all.push(can_phrase(r).to_string());
        }
        all.extend(d.binding_lines().iter().map(|l| l.to_string()));
        for text in &all {
            let l = text.to_lowercase();
            for bad in [
                "module",
                "manifest",
                "grant",
                "hash",
                "installed separately",
                "capability",
                "foxglove",
                "logitech",
                "vulpes",
            ] {
                assert!(!l.contains(bad), "{bad:?} in {text:?}");
            }
        }
        assert_eq!(
            d.source_line().unwrap(),
            "Not signed \u{b7} C:\\Apps\\Gizmo\\gizmo.exe"
        );
        assert_eq!(
            d.sensitive_note(),
            "These let Gizmo Keys reach your devices and the internet directly. \
             Hideout can't limit what it does with them."
        );
        assert_eq!(needs_ok_heading(4), "These need your OK (4)");
        assert_eq!(everyday_label(17), "17 everyday items");
        assert_eq!(everyday_label(1), "1 everyday item");
    }

    #[test]
    fn allow_and_start_replaces_the_count_only_when_allow_starts_the_app() {
        let mut d = dialog();
        assert!(d.allow_label().starts_with("Allow "));
        d.view.starts = true;
        for i in d.items.iter_mut().filter(|i| i.sensitive) {
            i.checked = false;
        }
        assert!(d.allow_label().starts_with("Allow "));
        assert_ne!(
            d.allow_label(),
            "Allow and start",
            "a sensitive item is unticked"
        );
        for i in d.items.iter_mut().filter(|i| i.sensitive) {
            i.checked = true;
            i.confirmed = true;
        }
        assert_eq!(d.allow_label(), "Allow and start");
        for i in d.items.iter_mut() {
            i.checked = false;
        }
        assert_eq!(d.allow_label(), "Allow none");
    }

    #[test]
    fn the_everyday_items_start_collapsed_unless_something_in_them_must_be_seen() {
        let mut d = fr2_dialog(&[("ui.window", false), ("hub.notify", false)]);
        assert_eq!((d.everyday(), d.everyday_shown()), (2, false));
        d.view.everyday_open = true;
        assert!(d.everyday_shown());
        d.view.everyday_open = false;
        d.items[0].new = true;
        assert!(d.everyday_shown(), "a new item is never hidden");
    }
    // The whole command in the Allow dialog (#1452). Numbers are the UX plan's checklist.

    fn with_args(args: &[&str]) -> PermDialog {
        PermDialog {
            args: args.iter().map(|a| a.to_string()).collect(),
            ..dialog()
        }
    }

    /// The dialog's spoken text as it was before the arguments were part of it.
    fn said_before(d: &PermDialog) -> String {
        let [a, b, c] = d.binding_lines();
        format!("{} {a} {} {b} {c}", d.intro(), d.path)
    }

    /// 1: an app without arguments has the dialog and the spoken text it had.
    #[test]
    fn an_app_without_arguments_adds_nothing_to_the_dialog() {
        let d = dialog();
        assert_eq!(d.args_heading(), None);
        assert!(d.args_drawn().is_empty() && !d.args_fold() && !d.args_note());
        assert_eq!(d.args_said(), None);
        assert_eq!(d.binding_text(), d.binding_lines().to_vec());
        assert_eq!(
            d.binding_lines()[0],
            "Only for this copy of Foxglove (the file above):"
        );
        assert_eq!(d.said(), said_before(&d));
        let nothing = PermDialog {
            items: Vec::new(),
            ..dialog()
        };
        assert_eq!(nothing.said(), said_before(&nothing));
    }

    #[test]
    fn the_arguments_are_listed_under_the_path_one_per_line_in_quotes() {
        let d = with_args(&["--profile", "work files"]);
        assert_eq!(d.args_heading().as_deref(), Some("Runs with 2 arguments:"));
        assert_eq!(
            d.args_drawn(),
            ["\u{201c}--profile\u{201d}", "\u{201c}work files\u{201d}"]
        );
        assert!(!d.args_fold() && !d.args_note());
        assert_eq!(
            d.binding_lines()[0],
            "Only for this copy of Foxglove (the file and arguments above):"
        );
        let nothing = PermDialog {
            items: Vec::new(),
            ..with_args(&["-v"])
        };
        assert_eq!(
            nothing.args_heading().as_deref(),
            Some("Runs with 1 argument:")
        );
        assert_eq!(
            nothing.binding_lines()[0],
            "Your OK is only for this copy of Foxglove (the file and arguments above):"
        );
        // Spoken after the path, before the rest of the lines.
        let [a, b, c] = d.binding_lines();
        assert_eq!(
            d.said(),
            format!(
                "{} {a} {} Runs with 2 arguments: \u{201c}--profile\u{201d} \
                 \u{201c}work files\u{201d} {b} {c}",
                d.intro(),
                d.path
            )
        );
    }

    /// 2: every argument can be reached, and all of them are always spoken.
    #[test]
    fn a_long_argument_list_shows_its_first_four_and_opens_to_all() {
        let mut d = with_args(&["a1", "a2", "a3", "a4", "a5", "a6"]);
        assert_eq!(d.args_heading().as_deref(), Some("Runs with 6 arguments:"));
        assert!(d.args_fold());
        assert_eq!(d.args_drawn().len(), ARGS_SHOWN);
        assert_eq!(d.args_drawn()[3], "\u{201c}a4\u{201d}");
        assert_eq!(show_all(d.args.len()), "Show all 6");
        let said = d.said();
        for a in &d.args {
            assert!(said.contains(&format!("\u{201c}{a}\u{201d}")), "{said}");
        }
        d.view.args_open = true;
        assert_eq!(d.args_drawn(), d.arg_lines());
        assert_eq!(d.args_drawn().len(), 6);
        assert_eq!(d.said(), said, "spoken the same, open or not");
        assert_eq!(SHOW_FEWER, "Show fewer");
        assert_eq!(d.args_link().as_deref(), Some("Show fewer"));
        d.view.args_open = false;
        assert_eq!(d.args_link().as_deref(), Some("Show all 6"));
        // Four fit without a link.
        let four = with_args(&["1", "2", "3", "4"]);
        assert!(!four.args_fold());
        assert_eq!(four.args_link(), None);
    }

    /// 3: nothing in an argument ends its quotes, breaks its line or turns its direction.
    #[test]
    fn an_argument_stays_inside_its_quotes_on_one_line() {
        for bad in [
            "a\u{201d} \u{201c}b",
            "a\" \"b",
            "line one\nline two\r\n\tthree",
            "abc\u{202e}exe.txt",
            "a\u{200b}b\u{2066}c\u{2069}",
            "x\u{0}y\u{1b}[31mz",
        ] {
            let d = with_args(&["ok", bad]);
            let line = d.arg_lines()[1].clone();
            let inner: String = {
                let mut c = line.chars();
                assert_eq!(c.next(), Some('\u{201c}'), "{line:?}");
                assert_eq!(c.next_back(), Some('\u{201d}'), "{line:?}");
                c.collect()
            };
            assert!(
                !inner.chars().any(|c| c.is_control()
                    || matches!(c, '"' | '\u{201c}' | '\u{201d}')
                    || hideout_contract::module::hidden_in_name(c)),
                "{bad:?} gives {line:?}"
            );
            assert!(d.args_note(), "{bad:?}");
            assert!(d.said().ends_with(&said_tail(&d)), "{bad:?}");
            assert!(d.args_said().unwrap().ends_with(ARGS_NOTE));
        }
        assert_eq!(arg_line("a\u{201d} \u{201c}b"), "\u{201c}a' 'b\u{201d}");
        assert_eq!(ARGS_NOTE, "Some characters can't be shown.");
        // Plain text, spaces included, is shown as written, with no note.
        let d = with_args(&["C:\\My Files\\a b.txt", "--x=1"]);
        assert_eq!(d.arg_lines()[0], "\u{201c}C:\\My Files\\a b.txt\u{201d}");
        assert!(!d.args_note());
        assert!(!d.said().contains(ARGS_NOTE));
    }

    fn said_tail(d: &PermDialog) -> String {
        d.binding_text()[1..].join(" ")
    }

    /// 4: a long argument is cut in the middle while the list is closed, so its end is seen;
    /// the cut says how many characters it leaves out, and the list opens to all of it.
    #[test]
    fn a_long_argument_is_cut_in_the_middle_and_can_be_seen_whole() {
        let long = format!(
            "{}{}",
            "a".repeat(40),
            "tail-of-41-characters-that-must-be-seen!!"
        );
        assert_eq!(long.chars().count(), 81);
        assert_eq!(shown_arg(&long), (long.clone(), false), "never cut here");
        assert!(arg_is_cut(&long));
        let head: String = long.chars().take(30).collect();
        let tail: String = long.chars().skip(81 - 28).collect();
        assert_eq!(
            arg_line_cut(&long),
            format!("\u{201c}{head}\u{2026}(23 more characters)\u{2026}{tail}\u{201d}")
        );
        assert_eq!(arg_line_cut(&long).chars().count(), ARG_MAX + 2);
        assert!(arg_line_cut(&long).ends_with("must-be-seen!!\u{201d}"));
        // What is shown and what is left out add up to the whole argument, at any length.
        for n in [81, 82, 100, 999, 1_000, 12_345, 400_000] {
            let arg: String = ('a'..='z').cycle().take(n).collect();
            let line = arg_line_cut(&arg);
            assert!(line.chars().count() <= ARG_MAX + 2, "{n}");
            let (start, rest) = line.split_once("\u{2026}(").expect("the cut");
            let (count, end) = rest
                .split_once(" more characters)\u{2026}")
                .expect("the count");
            let left_out: usize = count.parse().unwrap();
            let shown = start.chars().count() - 1 + end.chars().count() - 1;
            assert_eq!(shown + left_out, n, "{line}");
            assert!(arg.starts_with(&start[3..]) && arg.ends_with(&end[..end.len() - 3]));
        }
        let exact = "b".repeat(80);
        assert!(!arg_is_cut(&exact));
        assert_eq!(arg_line_cut(&exact), arg_line(&exact));

        // One cut argument is enough for the link, with fewer than five arguments.
        let mut d = with_args(&["--config", &long]);
        assert!(d.args_fold());
        assert_eq!(d.args_link().as_deref(), Some("Show all"));
        assert_eq!(
            d.args_drawn(),
            ["\u{201c}--config\u{201d}".to_string(), arg_line_cut(&long)]
        );
        // Spoken whole, open or not.
        let said = d.said();
        assert!(said.contains(&format!("\u{201c}{long}\u{201d}")), "{said}");
        assert!(!said.contains("more characters"), "{said}");
        d.view.args_open = true;
        assert_eq!(d.args_link().as_deref(), Some("Show fewer"));
        assert_eq!(
            d.args_drawn(),
            [
                "\u{201c}--config\u{201d}".to_string(),
                format!("\u{201c}{long}\u{201d}")
            ]
        );
        assert_eq!(d.said(), said);
        assert!(!d.args_note());

        // In a long list the opened view shows every argument whole.
        let mut many = with_args(&["1", "2", &long, "4", "5", &long]);
        assert_eq!(many.args_link().as_deref(), Some("Show all 6"));
        assert_eq!(many.args_drawn().len(), ARGS_SHOWN);
        assert_eq!(many.args_drawn()[2], arg_line_cut(&long));
        many.view.args_open = true;
        assert_eq!(many.args_drawn(), many.arg_lines());
        assert_eq!(many.args_drawn()[5], format!("\u{201c}{long}\u{201d}"));
    }

    /// An argument that is nothing but characters that are not drawn is not "empty".
    #[test]
    fn an_argument_of_unseen_characters_is_not_called_empty() {
        for unseen in [
            "\u{3164}",
            "\u{2800}\u{2800}",
            "\u{200b}\u{fe0f}",
            "\u{115f}\u{1160}\u{ffa0}",
        ] {
            let d = with_args(&["--name", unseen]);
            assert_eq!(
                d.arg_lines()[1],
                "\u{201c}\u{201d} (characters that can't be shown)",
                "{unseen:?}"
            );
            assert_eq!(d.args_drawn()[1], d.arg_lines()[1]);
            assert!(d.args_note(), "{unseen:?}");
            assert_eq!(
                d.args_said().as_deref(),
                Some(
                    "Runs with 2 arguments: \u{201c}--name\u{201d} characters that can't be \
                     shown Some characters can't be shown."
                ),
                "{unseen:?}"
            );
            assert!(!d.args_said().unwrap().contains("empty"));
        }
        // Inside other text they are dropped and the note says so.
        let d = with_args(&["a\u{3164}b"]);
        assert_eq!(d.arg_lines()[0], "\u{201c}ab\u{201d}");
        assert!(d.args_note());
    }

    /// 5: an empty argument is seen and spoken.
    #[test]
    fn an_empty_argument_is_shown_and_spoken() {
        let d = with_args(&["--name", "", "x"]);
        assert_eq!(d.arg_lines()[1], "\u{201c}\u{201d} (empty)");
        assert_eq!(
            d.args_said().as_deref(),
            Some("Runs with 3 arguments: \u{201c}--name\u{201d} empty \u{201c}x\u{201d}")
        );
        assert!(!d.args_note());
    }

    fn one_run(why: OneRun, path: &str, args: &[&str]) -> PermDialog {
        PermDialog {
            path: path.into(),
            one_run: Some(why),
            keep: None,
            ..with_args(args)
        }
    }

    /// 6 and 7: an app whose program runs whatever it is given is allowed for this run only;
    /// the lines name the file, say why and give the next step.
    #[test]
    fn a_one_run_app_says_its_ok_is_for_this_run_and_why() {
        let d = one_run(OneRun::Scripts, r"C:\Python312\python.exe", &["-m", "fox"]);
        assert_eq!(
            d.binding_text(),
            [
                "Your OK is for this run of Foxglove only:",
                "python.exe runs whatever it is given, so Hideout asks again each time \
                 Foxglove starts.",
                "To be asked only once, use a version of Foxglove that has its own program file.",
                "You can turn each one off any time in Apps, Foxglove.",
            ]
        );
        assert_eq!(
            d.said(),
            format!(
                "{} Your OK is for this run of Foxglove only: C:\\Python312\\python.exe Runs \
                 with 2 arguments: \u{201c}-m\u{201d} \u{201c}fox\u{201d} python.exe runs \
                 whatever it is given, so Hideout asks again each time Foxglove starts. To be \
                 asked only once, use a version of Foxglove that has its own program file. You \
                 can turn each one off any time in Apps, Foxglove.",
                d.intro()
            )
        );
        let w = one_run(
            OneRun::Windows,
            r"C:\Windows\System32\rundll32.exe",
            &["x.dll,Go"],
        );
        assert_eq!(
            w.binding_text()[1],
            "rundll32.exe is part of Windows and runs whatever it is given, so Hideout asks \
             again each time Foxglove starts."
        );
        for d in [&d, &w] {
            let said = d.said();
            assert!(
                !said.contains("this copy") && !said.contains(KEEP_LABEL),
                "{said}"
            );
            assert_eq!(
                d.binding_text()[0],
                "Your OK is for this run of Foxglove only:"
            );
            assert!(!d.allowed().keep);
        }
        // The same without arguments and for an app that asks for nothing.
        let bare = PermDialog {
            items: Vec::new(),
            ..one_run(OneRun::Scripts, r"C:\Tools\node.exe", &[])
        };
        assert_eq!(bare.binding_text().len(), 4);
        assert_eq!(
            bare.binding_text()[3],
            "You can turn Foxglove off any time in Apps."
        );
        // The file's name is untrusted text too: cleaned, and cut to 40 characters.
        let long = format!(r"C:\T\{}\u{{202e}}python.exe", "p".repeat(60))
            .replace("\\u{202e}", "\u{202e}");
        let file = shown_file(&long);
        assert_eq!(file.chars().count(), 40);
        assert!(file.ends_with("python.exe") && !file.contains('\u{202e}'));
    }

    /// 9: the control pipe's answer carries the dialog's facts.
    #[test]
    fn the_pipe_reply_for_a_one_run_app_says_what_the_dialog_says() {
        assert_eq!(
            one_run_reply(r"C:\Python312\python.exe"),
            "allowed for this run only; python.exe runs whatever it is given, so Hideout asks \
             again at the next start"
        );
    }

    /// 10: the new words keep to the window's vocabulary.
    #[test]
    fn the_new_lines_use_app_and_program_only() {
        let d = one_run(OneRun::Scripts, r"C:\Python312\python.exe", &["a\u{0}", ""]);
        let w = one_run(
            OneRun::Windows,
            r"C:\Windows\System32\cmd.exe",
            &["/c", "x"],
        );
        let plain = with_args(&["1", "2", "3", "4", "5"]);
        let mut all = vec![
            ARGS_NOTE.to_string(),
            SHOW_FEWER.to_string(),
            show_all(5),
            one_run_reply(&d.path),
            d.args_said().unwrap(),
            plain.args_said().unwrap(),
        ];
        all.extend(d.binding_text());
        all.extend(w.binding_text());
        all.extend(plain.binding_text());
        for line in &all {
            let low = line.to_lowercase();
            for word in ["module", "hash", "grant", "microsoft", "python ", "sha"] {
                assert!(!low.contains(word), "{line:?} says {word:?}");
            }
        }
    }
}
