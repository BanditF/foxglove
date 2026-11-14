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
        format!("Let {} run in Hideout?", self.name)
    }

    /// "<name> asks for these <N> things. Each app that uses them still needs its own
    /// permission. Nothing leaves this PC." (the last sentence goes when the network is asked).
    pub fn intro(&self) -> String {
        let n = self.items.len();
        let mut s = format!(
            "{} asks for these {n} things. Each app that uses them still needs its own permission.",
            self.name
        );
        if !self
            .items
            .iter()
            .any(|i| i.raw == "net" || i.raw.starts_with("net."))
        {
            s.push_str(" Nothing leaves this PC.");
        }
        s
    }

    pub fn void_line(&self) -> String {
        format!(
            "{}'s program file is new or was moved since you allowed it.",
            self.name
        )
    }

    /// The reason lines on top.
    pub fn reason_lines(&self) -> Vec<String> {
        match self.reason {
            Some(Reason::ListChanged) => LIST_CHANGED.iter().map(|s| (*s).into()).collect(),
            Some(Reason::Void) => vec![self.void_line()],
            None => Vec::new(),
        }
    }

    pub fn binding_lines(&self) -> [String; 3] {
        [
            format!("Only for this copy of {}:", self.name),
            "If the program or this list changes, Hideout asks again.".into(),
            format!(
                "You can turn each one off any time in Modules, {}.",
                self.name
            ),
        ]
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

    pub fn all_label(&self) -> String {
        format!("{ALL_PREFIX} {}", self.allowable().count())
    }

    pub fn all_state(&self) -> All {
        let n = self.allowable().count();
        let on = self.allowable().filter(|i| i.checked).count();
        match on {
            0 => All::None,
            x if x == n => All::All,
            _ => All::Some,
        }
    }

    /// The All box: checks everything, or clears everything when it is already all.
    pub fn toggle_all(&mut self) {
        let to = self.all_state() != All::All;
        for i in self.items.iter_mut().filter(|i| i.allowable) {
            i.checked = to;
            if !to {
                i.confirmed = false;
            }
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
        }
    }

    /// The accessible description: intro plus the binding lines.
    pub fn said(&self) -> String {
        let [a, b, c] = self.binding_lines();
        format!("{} {a} {} {b} {c}", self.intro(), self.path)
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
}

impl ItemWord {
    pub fn word(self) -> &'static str {
        match self {
            ItemWord::Allowed => "Allowed",
            ItemWord::Off => "Off",
            ItemWord::CantBeAllowed => "Can't be allowed",
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

/// Which header line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HeaderKind {
    /// Adopted and started without the user's act, never asked.
    Ask,
    /// The program changed or moved since the allow.
    Changed,
    /// Allowed from the command line, not yet seen.
    Cli,
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
            format!("Until you allow it, {name} stays off."),
            NOT_NOW,
        ),
        HeaderKind::Changed => (
            format!("{name} changed since you allowed it"),
            "Its program file is new or was moved, so Hideout stopped it until you allow it again."
                .into(),
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
            error: None,
        }
    }

    #[test]
    fn the_words_follow_the_plan() {
        let d = dialog();
        assert_eq!(d.title(), "Let Foxglove run in Hideout?");
        assert_eq!(
            d.intro(),
            "Foxglove asks for these 4 things. Each app that uses them still needs its own \
             permission. Nothing leaves this PC."
        );
        assert_eq!(d.allow_label(), "Allow 2");
        assert_eq!(d.all_label(), "All 3");
        assert_eq!(
            d.binding_lines()[2],
            "You can turn each one off any time in Modules, Foxglove."
        );
        assert_eq!(
            keyboard_line("Foxglove", false),
            "Also let Foxglove handle your keyboard for other apps (off)"
        );
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
        d.toggle_all();
        assert_eq!(d.all_state(), All::None);
        assert_eq!(d.allow_label(), "Allow none");
        let a = d.allowed();
        assert!(a.chosen.is_empty());
        assert_eq!(a.declined.len(), 3);
        assert!(!a.declined.contains(&"modules.manage".to_string()));
    }

    #[test]
    fn the_all_box_is_tri_state() {
        let mut d = dialog();
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
    }

    #[test]
    fn reasons_and_header_lines_use_the_exact_words() {
        let mut d = dialog();
        d.reason = Some(Reason::ListChanged);
        assert_eq!(d.reason_lines(), LIST_CHANGED);
        d.reason = Some(Reason::Void);
        assert_eq!(
            d.reason_lines(),
            ["Foxglove's program file is new or was moved since you allowed it."]
        );
        let h = header(HeaderKind::Changed, "foxglove", "Foxglove", None);
        assert_eq!(h.title, "Foxglove changed since you allowed it");
        assert_eq!(
            h.body,
            "Its program file is new or was moved, so Hideout stopped it until you allow it again."
        );
        let h = header(HeaderKind::Ask, "foxglove", "Foxglove", None);
        assert_eq!(h.body, "Until you allow it, Foxglove stays off.");
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
}
