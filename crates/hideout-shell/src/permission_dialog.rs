// SPDX-License-Identifier: MIT
//! The module permission dialog's words and rules (#1178, `docs/hideout-permission-dialog-ux.md`):
//! what the dialog, the header lines and the Permissions section say and do. Pure and
//! window-free, so the wording, the counts and the defaults are tested without a window; the
//! drawing is `permission_dialog_ui.rs`. The shell has no registry: the hub fills these from
//! `Registry::permission_view`.
//!
//! Sentence case. The window never says "permission string", "provide", "method", "router",
//! "hash" or an id, except the selectable path and the list code.

pub const ALLOW_NONE: &str = "Allow none";
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
pub const NOT_SAVED: &str = "Hideout couldn't save your choice. Nothing changed.";
pub const SECTION_LABEL: &str = "Permissions";

/// "Keep my OK after updates in this folder" (slice 4 of the rebuild-trust note,
/// `review/ux-keep-ok-switch.md`). Plain words: "app", never "program", "hash", "grant",
/// "follow" or an id.
pub const KEEP_LABEL: &str = "Keep my OK after updates in this folder";
/// Why the box is off and disabled: a network, removable or unreadable folder.
pub const KEEP_REFUSED: &str = "Hideout keeps your OK only for a folder on this PC's own drive.";
/// Binding line 2 while the box is checked.
pub const KEEP_BINDING: &str = "If the app asks for more or moves, Hideout asks again.";
pub const KEEP_CONFIRM_TITLE: &str = "Keep your OK for this folder?";
pub const KEEP_CONFIRM_OK: &str = "Keep my OK";
pub const CANCEL: &str = "Cancel";
pub const TURN_ON_KEEP: &str = "Turn on\u{2026}";
/// The spoken names of the row's buttons (no ellipsis when spoken).
pub const KEEP_TURN_ON_SAID: &str = "Turn on keep my OK after updates, opens a confirm";
pub const KEEP_TURN_OFF_SAID: &str = "Turn off keep my OK after updates";
/// Shown when the choice could not be saved.
pub const KEEP_NOT_SAVED: &str = "Hideout couldn't save your choice. Nothing was changed.";
/// How many of the newest history lines show before "and N earlier".
pub const HISTORY_SHOWN: usize = 3;

/// The last part of a path (`C:\Apps\Foxglove\foxglove.exe` gives `foxglove.exe`).
pub fn file_of(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

/// The helper under the label.
pub fn keep_helper(name: &str, file: &str) -> String {
    format!(
        "A new build of {name} saved here as {file} keeps what you allowed. Hideout asks again \
         if the new build wants more, or if the app moves."
    )
}

/// After checking, in the attention colour.
pub fn keep_warning(name: &str, file: &str) -> String {
    format!(
        "Any app saved in this folder as {file} gets what you allowed {name}. Only do this for \
         a folder where only you put apps."
    )
}

/// The accessible name of the "I understand" under the box.
pub fn keep_understand_said(name: &str, file: &str) -> String {
    format!("{UNDERSTAND}: any app saved in this folder as {file} gets what you allowed {name}")
}

/// The polite note after Turn off.
pub fn keep_off_note(name: &str) -> String {
    format!("Off. A new build of {name} will need your OK again.")
}

/// The muted text instead of a button when Turn on is not offered yet.
pub fn keep_allow_first(name: &str) -> String {
    format!("Allow {name} first.")
}

/// Shown when the list changed while the confirm was open.
pub fn keep_changed_open(name: &str) -> String {
    format!("{name} changed while this was open. Nothing was changed.")
}

/// "New build on 17 Nov 2026, 20:40. Your OK stayed (kept for this folder)." and, when the
/// new build asks for more, "...; 2 new things wait for it." `when` is the local time as
/// words ("17 Nov 2026, 20:40").
pub fn keep_history(when: &str, grew: usize) -> String {
    let mut line = format!("New build on {when}. Your OK stayed (kept for this folder)");
    if grew > 0 {
        line.push_str(&format!(
            "; {grew} new thing{} wait{} for it",
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
        Keep {
            offer: KeepOffer::Refused(KEEP_REFUSED.into()),
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
            KeepState::On => "On",
            _ => "Off",
        }
    }

    /// The button's text, where one is offered.
    pub fn button(&self) -> Option<&'static str> {
        match self.state {
            KeepState::On => Some(TURN_OFF),
            KeepState::Off => Some(TURN_ON_KEEP),
            KeepState::Unavailable(_) => None,
        }
    }

    pub fn button_said(&self) -> Option<&'static str> {
        match self.state {
            KeepState::On => Some(KEEP_TURN_OFF_SAID),
            KeepState::Off => Some(KEEP_TURN_ON_SAID),
            KeepState::Unavailable(_) => None,
        }
    }

    /// The muted line under the label.
    pub fn helper(&self) -> String {
        match &self.state {
            KeepState::Unavailable(r) => r.clone(),
            _ => keep_helper(&self.name, &self.file),
        }
    }

    /// The row's accessible name.
    pub fn said(&self) -> String {
        format!(
            "{KEEP_LABEL}, {}, {}",
            self.word().to_lowercase(),
            self.folder
        )
    }
}

/// The mark on an item checked from the user's earlier answer.
pub const BEFORE_MARK: &str = "Allowed before";
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
        let risk = match self.raw.as_str() {
            r if r.starts_with("provide:gpu.") => {
                "see every AI request from every app, \
             including what you type, and answer them"
            }
            "key.events" => "see every key you press",
            "hid.raw" => "talk to your devices directly",
            r if r == "net" || r.starts_with("net.") => "use the network",
            r if r.starts_with("ai.ask:") => "send questions to an AI service",
            r if r.starts_with("secrets.ref:") => "use a secret you stored",
            _ => "do something Hideout can't describe in a line",
        };
        format!(
            "{}. This lets {name} {risk}.",
            self.line.trim_end_matches('.')
        )
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
    pub items: Vec<ListItem>,
    pub reason: Option<Reason>,
    pub keyboard: Option<KeyboardChoice>,
    /// "Keep my OK after updates in this folder"; `None` for a module with no app path.
    pub keep: Option<Keep>,
    /// After a changed list: the lines of items the old list asked for and this one doesn't.
    pub removed: Vec<String>,
    /// The app is at the very path the grant was bound to (so its file changed, not moved).
    pub same_place: bool,
    /// An inline failure under the buttons (busy, not saved).
    pub error: Option<String>,
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
        format!("Let {} run in Hideout?", self.name)
    }

    /// "<name> asks for these <N> things. Each app that uses them still needs its own
    /// permission. Nothing leaves this PC." (the last sentence goes when the network is asked).
    pub fn intro(&self) -> String {
        let n = self.items.len();
        let never = n - self.allowable().count();
        let mut s = format!("{} asks for these {n} things.", self.name);
        if never > 0 {
            s.push_str(&format!(" {never} can't be allowed."));
        }
        s.push_str(" Each app that uses them still needs its own permission.");
        if !self
            .items
            .iter()
            .any(|i| i.raw == "net" || i.raw.starts_with("net."))
        {
            s.push_str(" Nothing leaves this PC.");
        }
        s
    }

    /// The lines that say what is pre-checked from before; none when nothing is.
    pub fn before_lines(&self) -> Vec<String> {
        if self.items.iter().any(|i| i.before) {
            BEFORE_LINES.iter().map(|l| (*l).into()).collect()
        } else {
            Vec::new()
        }
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

    pub fn binding_lines(&self) -> [String; 3] {
        let keeping = self.keep.as_ref().is_some_and(|k| !k.is_refused() && k.on);
        [
            format!("Only for this copy of {}:", self.name),
            if keeping {
                KEEP_BINDING.into()
            } else {
                "If this file or this list changes, Hideout asks again.".into()
            },
            format!(
                "You can turn each one off any time in Modules, {}.",
                self.name
            ),
        ]
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

    /// "Allow 9", or "Allow none" with nothing counted.
    pub fn allow_label(&self) -> String {
        match self.count() {
            0 => ALLOW_NONE.into(),
            n => format!("Allow {n}"),
        }
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
        let [a, b, c] = self.binding_lines();
        let mut s = format!("{} {a} {} {b} {c}", self.intro(), self.path);
        if let Some(k) = &self.keep {
            s.push_str(&format!(
                " {KEEP_LABEL}: {}.",
                if k.on { "on" } else { "off" }
            ));
        }
        s
    }
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

    /// "Allow 9", or "Allow none" with nothing counted (the same rule as one dialog).
    pub fn allow_label(&self) -> String {
        match self.count() {
            0 => ALLOW_NONE.into(),
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
    /// Allowed from the command line, not yet seen.
    Cli,
    /// A command-line grant waits for the user's confirmation (#1183); nothing is allowed yet.
    Queued,
    /// Allowed, and a signed update asks for more (#1238); `cli` carries (count, first line).
    More,
    /// Two or more adopted apps wait for their first OK: one header, one dialog (UX cut 3).
    FirstRun,
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
            format!("Until you allow it, {name} stays off. Allow, and it starts."),
            NOT_NOW,
        ),
        HeaderKind::Changed => (
            format!("{name} changed since you allowed it"),
            "Its app file is new or was moved, so Hideout stopped it until you allow it again."
                .into(),
            NOT_NOW,
        ),
        HeaderKind::Queued => {
            let (n, _) = cli.unwrap_or((0, ""));
            (
                format!(
                    "A command asked to allow {n} permission{} for {name}",
                    if n == 1 { "" } else { "s" }
                ),
                "Nothing is allowed until you review it here.".into(),
                NOT_NOW,
            )
        }
        HeaderKind::More => {
            let (n, first) = cli.unwrap_or((0, ""));
            let body = if n == 1 {
                format!("Its update asks for one new thing: “{first}”. It stays off until you allow it, so apps that ask for it are told no.")
            } else {
                format!("Its update asks for {n} new things, like “{first}”. They stay off until you allow them, so apps that ask for them are told no.")
            };
            (format!("{name}'s update asks for more"), body, NOT_NOW)
        }
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
        }
    }

    fn dialog() -> PermDialog {
        PermDialog {
            id: "foxglove".into(),
            name: "Foxglove".into(),
            code: "7f3a91c20b4e55d1".into(),
            path: r"C:\Program Files\Foxglove\foxglove.exe".into(),
            items: vec![
                item("key.events", Group::Sensitive, true, false),
                item("provide:a.set", Group::MakeChanges, false, true),
                item("provide:b.read", Group::AnswerQuestions, false, true),
                ListItem {
                    allowable: false,
                    checked: false,
                    reason: Some("Hideout never allows this for this module.".into()),
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
            error: None,
        }
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
            "You can turn each one off any time in Modules, Foxglove."
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
            "Until you allow it, Foxglove stays off. Allow, and it starts."
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
        assert!(d
            .said()
            .ends_with("Keep my OK after updates in this folder: on."));
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
            "You can turn each one off any time in Modules, Foxglove."
        );
    }

    #[test]
    fn the_dialog_says_the_state_of_the_box() {
        let mut d = kept();
        let plain = dialog().said();
        assert_eq!(
            d.said(),
            format!("{plain} Keep my OK after updates in this folder: off.")
        );
        d.keep.as_mut().unwrap().set_on(true);
        assert!(d
            .said()
            .ends_with("Keep my OK after updates in this folder: on."));
        let k = d.keep.as_ref().unwrap();
        assert_eq!(k.said(), "Keep my OK after updates in this folder, checked");
        assert_eq!(
            Keep::offered(false).said(),
            "Keep my OK after updates in this folder, not checked"
        );
    }

    #[test]
    fn the_keep_words_use_the_exact_copy() {
        assert_eq!(file_of(r"C:\Apps\Fox\foxglove.exe"), "foxglove.exe");
        assert_eq!(
            keep_helper("Foxglove", "foxglove.exe"),
            "A new build of Foxglove saved here as foxglove.exe keeps what you allowed. Hideout \
             asks again if the new build wants more, or if the app moves."
        );
        assert_eq!(
            keep_warning("Foxglove", "foxglove.exe"),
            "Any app saved in this folder as foxglove.exe gets what you allowed Foxglove. Only \
             do this for a folder where only you put apps."
        );
        assert_eq!(
            keep_understand_said("Foxglove", "foxglove.exe"),
            "I understand: any app saved in this folder as foxglove.exe gets what you allowed \
             Foxglove"
        );
        assert_eq!(
            keep_off_note("Foxglove"),
            "Off. A new build of Foxglove will need your OK again."
        );
        assert_eq!(keep_allow_first("Foxglove"), "Allow Foxglove first.");
        assert_eq!(
            keep_changed_open("Foxglove"),
            "Foxglove changed while this was open. Nothing was changed."
        );
        assert_eq!(KEEP_CONFIRM_TITLE, "Keep your OK for this folder?");
        assert_eq!((KEEP_CONFIRM_OK, CANCEL), ("Keep my OK", "Cancel"));
        assert_eq!(earlier_label(4), "and 4 earlier");
    }

    #[test]
    fn the_history_says_the_time_and_what_waits() {
        assert_eq!(
            keep_history("17 Nov 2026, 20:40", 0),
            "New build on 17 Nov 2026, 20:40. Your OK stayed (kept for this folder)."
        );
        assert_eq!(
            keep_history("17 Nov 2026, 20:40", 2),
            "New build on 17 Nov 2026, 20:40. Your OK stayed (kept for this folder); 2 new \
             things wait for it."
        );
        assert_eq!(
            keep_history("17 Nov 2026, 20:40", 1),
            "New build on 17 Nov 2026, 20:40. Your OK stayed (kept for this folder); 1 new \
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
            r"Keep my OK after updates in this folder, on, C:\Apps\Fox"
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
}
