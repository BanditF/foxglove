// SPDX-License-Identifier: MIT
//! The keyboard grant's words and rules (#1155-B, the UX plan `ux-1155-legacy-grant.md`): what
//! the permission dialog, the Foxglove page's header lines and the Permissions row say and do in
//! each state. Pure and window-free, so every state of the plan's table and the dialog's keys are
//! tested without a window; the drawing is `keyboard_grant_ui.rs`.
//!
//! Sentence case. The UI never says "module mode", "method", "area", "router", "provide",
//! "grant", "hash" or an id.

/// Where the grant stands, as the hub reads it from `inventory.json` and `hideout.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Situation {
    /// Not installed or not adopted, or module mode off: Foxglove runs standalone. Nothing shown.
    Standalone,
    /// Adopted and never turned on. `started_without_act`: it started (autostart, an upgrade)
    /// with no click of the user's, so the page gets the header line.
    NotAsked { started_without_act: bool },
    /// Allowed on `at` (an RFC 3339 time).
    Granted { at: String },
    /// "Not now" in the dialog.
    Declined,
    /// The user turned it off.
    Revoked,
    /// The program file changed or moved since it was allowed (void).
    Changed { at: String, moved: bool },
}

/// The word on the Permissions row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    On,
    Off,
    AsksAgain,
}

impl RowState {
    pub fn word(self) -> &'static str {
        match self {
            RowState::On => "On",
            RowState::Off => "Off",
            RowState::AsksAgain => "Asks again",
        }
    }
}

/// What a button on the row or the header line asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// "Turn off": acts at once, no confirm.
    TurnOff,
    /// "Turn on…" or "Review…": opens the dialog.
    OpenDialog,
}

/// The Permissions row under Modules, Foxglove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub title: &'static str,
    pub state: RowState,
    pub line: String,
    pub button: &'static str,
    pub ask: Ask,
    /// The main (filled) button: only "Review…" after a change.
    pub main: bool,
    /// The row's accessible name, one group.
    pub said: String,
}

/// A hub-drawn header line over the Foxglove page, the frame of Update waiting for you.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub title: &'static str,
    pub body: &'static str,
    pub review: &'static str,
    pub not_now: &'static str,
}

pub const ROW_TITLE: &str = "Handle your keyboard for other apps";
pub const TURN_OFF: &str = "Turn off";
pub const TURN_ON: &str = "Turn on…";
pub const REVIEW: &str = "Review…";
pub const NOT_NOW: &str = "Not now";
pub const ALLOW: &str = "Allow";

pub const HEADER_ASK_TITLE: &str = "Foxglove needs your OK to handle your keyboard for other apps";
pub const HEADER_ASK_BODY: &str =
    "Until you allow it, apps that use your keyboard through Hideout can't reach it.";
pub const HEADER_CHANGED_TITLE: &str = "Foxglove changed since you allowed it";
pub const HEADER_CHANGED_BODY: &str =
    "Its program file is new or was moved, so Hideout turned keyboard sharing off.";

pub const DIALOG_TITLE: &str = "Let Foxglove handle your keyboard in Hideout?";
pub const DIALOG_INTRO: &str = "Apps you have allowed to use your keyboard will reach it through Foxglove. With this on, Foxglove can do these for them:";
pub const DIALOG_ITEMS: [&str; 4] = [
    "See which keyboard is connected, its battery, layer",
    "Change its colours, effects and brightness",
    "Change what the keyboard's screens show",
    "Run its key actions, switch Foxglove profiles, pick the keyboard in front (never reset or bootloader)",
];
pub const DIALOG_FOOTER: [&str; 2] = [
    "Each app still needs its own permission. Nothing leaves this PC.",
    "You can turn this off any time in Modules, Foxglove.",
];
pub const DIALOG_BINDING: [&str; 2] = [
    "Only for this copy of Foxglove:",
    "If the program file changes or moves, Hideout asks again.",
];
pub const DIALOG_REASON: &str = "Foxglove's program file is new or was moved since you allowed it.";

/// "12 Nov 2026" from `2026-11-12T10:00:00Z`; the text itself when it isn't a date.
pub fn date_words(at: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let d = at.get(..10).unwrap_or(at);
    let mut p = d.split('-');
    match (p.next(), p.next(), p.next()) {
        (Some(y), Some(m), Some(day)) => match (m.parse::<usize>(), day.parse::<u32>()) {
            (Ok(m @ 1..=12), Ok(day)) if y.len() == 4 => {
                format!("{day} {} {y}", MONTHS[m - 1])
            }
            _ => at.to_string(),
        },
        _ => at.to_string(),
    }
}

/// The header line over the Foxglove page for `s`, or none (plan §4).
pub fn header(s: &Situation) -> Option<Header> {
    match s {
        Situation::NotAsked {
            started_without_act: true,
        } => Some(Header {
            title: HEADER_ASK_TITLE,
            body: HEADER_ASK_BODY,
            review: REVIEW,
            not_now: NOT_NOW,
        }),
        Situation::Changed { .. } => Some(Header {
            title: HEADER_CHANGED_TITLE,
            body: HEADER_CHANGED_BODY,
            review: REVIEW,
            not_now: NOT_NOW,
        }),
        _ => None,
    }
}

/// The Permissions row for `s`, or none while Foxglove runs standalone (plan §4).
pub fn row(s: &Situation) -> Option<Row> {
    let (state, line, button, ask, main) = match s {
        Situation::Standalone => return None,
        Situation::NotAsked { .. } | Situation::Declined => (
            RowState::Off,
            "Not allowed yet.".to_string(),
            TURN_ON,
            Ask::OpenDialog,
            false,
        ),
        Situation::Granted { at } => (
            RowState::On,
            format!(
                "Keyboard, lights, art and screens, actions. Allowed {}.",
                date_words(at)
            ),
            TURN_OFF,
            Ask::TurnOff,
            false,
        ),
        Situation::Revoked => (
            RowState::Off,
            "You turned this off. Foxglove keeps running on its own.".to_string(),
            TURN_ON,
            Ask::OpenDialog,
            false,
        ),
        Situation::Changed { at, moved } => (
            RowState::AsksAgain,
            format!(
                "Foxglove's program file {} on {}.",
                if *moved { "was moved" } else { "changed" },
                date_words(at)
            ),
            REVIEW,
            Ask::OpenDialog,
            true,
        ),
    };
    let said = format!("{ROW_TITLE}, {}. {line} {button}, button.", state.word());
    Some(Row {
        title: ROW_TITLE,
        state,
        line,
        button,
        ask,
        main,
        said,
    })
}

/// Whether the areas are routed for `s` once `initialize` has checked path and hash (plan §4's
/// last column): only a standing grant.
pub fn routes(s: &Situation) -> bool {
    matches!(s, Situation::Granted { .. })
}

/// What the user answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    NotNow,
}

/// A key the dialog reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Tab,
    ShiftTab,
}

/// The dialog: the first section of the general permission dialog (§6.4), so "Update waiting for
/// you" can reuse it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    /// "Foxglove's program file is new or was moved ..." on top, after a change.
    pub reason: Option<&'static str>,
    /// The full path of this copy of Foxglove the answer binds to.
    pub path: String,
    /// Which button has focus: starts on Not now.
    pub focus: Answer,
}

impl Dialog {
    /// Opens for `path`; `after_change` puts the reason on top.
    pub fn open(path: &str, after_change: bool) -> Dialog {
        Dialog {
            reason: after_change.then_some(DIALOG_REASON),
            path: path.to_string(),
            focus: Answer::NotNow,
        }
    }

    /// Reading order for a screen reader: title, intro, the four items, footer, binding line with
    /// the path, Allow, Not now.
    pub fn reading_order(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(r) = self.reason {
            v.push(r.to_string());
        }
        v.push(DIALOG_TITLE.into());
        v.push(DIALOG_INTRO.into());
        v.extend(DIALOG_ITEMS.iter().map(|s| s.to_string()));
        v.push(DIALOG_FOOTER[0].into());
        v.push(DIALOG_FOOTER[1].into());
        v.push(DIALOG_BINDING[0].into());
        v.push(self.path.clone());
        v.push(DIALOG_BINDING[1].into());
        v.push(ALLOW.into());
        v.push(NOT_NOW.into());
        v
    }

    /// A key. Escape answers Not now from anywhere; Enter answers the focused button (Not now
    /// when the dialog opened); Tab and Shift+Tab cycle Allow and Not now (focus is trapped).
    pub fn key(&mut self, k: Key) -> Option<Answer> {
        match k {
            Key::Escape => Some(Answer::NotNow),
            Key::Enter => Some(self.focus),
            Key::Tab | Key::ShiftTab => {
                self.focus = match self.focus {
                    Answer::Allow => Answer::NotNow,
                    Answer::NotNow => Answer::Allow,
                };
                None
            }
        }
    }

    /// Closed any other way (the window's close, a click outside, the app going away) is Not now.
    pub fn close(&self) -> Answer {
        Answer::NotNow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: &str = "2026-11-12T10:00:00Z";

    fn all() -> Vec<Situation> {
        vec![
            Situation::Standalone,
            Situation::NotAsked {
                started_without_act: false,
            },
            Situation::NotAsked {
                started_without_act: true,
            },
            Situation::Granted { at: AT.into() },
            Situation::Declined,
            Situation::Revoked,
            Situation::Changed {
                at: AT.into(),
                moved: false,
            },
            Situation::Changed {
                at: AT.into(),
                moved: true,
            },
        ]
    }

    #[test]
    fn standalone_shows_nothing() {
        assert_eq!(header(&Situation::Standalone), None);
        assert_eq!(row(&Situation::Standalone), None);
        assert!(!routes(&Situation::Standalone));
    }

    #[test]
    fn not_asked_has_a_row_and_a_header_only_when_started_without_the_users_act() {
        let quiet = Situation::NotAsked {
            started_without_act: false,
        };
        assert_eq!(header(&quiet), None);
        let r = row(&quiet).unwrap();
        assert_eq!(
            (r.state, r.line.as_str(), r.button, r.ask, r.main),
            (
                RowState::Off,
                "Not allowed yet.",
                "Turn on…",
                Ask::OpenDialog,
                false
            )
        );
        let h = header(&Situation::NotAsked {
            started_without_act: true,
        })
        .unwrap();
        assert_eq!(
            h.title,
            "Foxglove needs your OK to handle your keyboard for other apps"
        );
        assert_eq!(
            h.body,
            "Until you allow it, apps that use your keyboard through Hideout can't reach it."
        );
        assert_eq!((h.review, h.not_now), ("Review…", "Not now"));
        assert_eq!(
            row(&Situation::NotAsked {
                started_without_act: true
            })
            .unwrap(),
            r
        );
    }

    #[test]
    fn granted_is_on_with_turn_off_and_the_only_state_that_routes() {
        let g = Situation::Granted { at: AT.into() };
        let r = row(&g).unwrap();
        assert_eq!(r.state.word(), "On");
        assert_eq!(
            r.line,
            "Keyboard, lights, art and screens, actions. Allowed 12 Nov 2026."
        );
        assert_eq!((r.button, r.ask), ("Turn off", Ask::TurnOff));
        assert_eq!(header(&g), None);
        for s in all() {
            assert_eq!(routes(&s), matches!(s, Situation::Granted { .. }), "{s:?}");
        }
    }

    #[test]
    fn declined_does_not_nag() {
        assert_eq!(header(&Situation::Declined), None);
        let r = row(&Situation::Declined).unwrap();
        assert_eq!(
            (r.state, r.line.as_str(), r.button),
            (RowState::Off, "Not allowed yet.", "Turn on…")
        );
    }

    #[test]
    fn revoked_says_foxglove_keeps_running() {
        assert_eq!(header(&Situation::Revoked), None);
        let r = row(&Situation::Revoked).unwrap();
        assert_eq!(r.state, RowState::Off);
        assert_eq!(
            r.line,
            "You turned this off. Foxglove keeps running on its own."
        );
        assert_eq!(r.button, "Turn on…");
    }

    #[test]
    fn a_changed_program_asks_again_with_the_reason_and_a_main_review_button() {
        for (moved, word) in [(false, "changed"), (true, "was moved")] {
            let s = Situation::Changed {
                at: AT.into(),
                moved,
            };
            let h = header(&s).unwrap();
            assert_eq!(h.title, "Foxglove changed since you allowed it");
            assert_eq!(
                h.body,
                "Its program file is new or was moved, so Hideout turned keyboard sharing off."
            );
            let r = row(&s).unwrap();
            assert_eq!(r.state.word(), "Asks again");
            assert_eq!(
                r.line,
                format!("Foxglove's program file {word} on 12 Nov 2026.")
            );
            assert_eq!(
                (r.button, r.main, r.ask),
                ("Review…", true, Ask::OpenDialog)
            );
        }
    }

    #[test]
    fn the_row_is_one_group_with_the_state_in_words() {
        let r = row(&Situation::Granted { at: AT.into() }).unwrap();
        assert!(r
            .said
            .starts_with("Handle your keyboard for other apps, On. "));
        assert!(r.said.ends_with("Turn off, button."));
    }

    #[test]
    fn no_state_says_a_forbidden_word() {
        let bad = [
            "module mode",
            "method",
            "area",
            "router",
            "provide",
            "grant",
            "hash",
            "foxglove.exe",
        ];
        let mut text = Vec::new();
        for s in all() {
            if let Some(h) = header(&s) {
                text.extend([h.title.to_string(), h.body.to_string()]);
            }
            if let Some(r) = row(&s) {
                text.extend([r.title.to_string(), r.line, r.button.to_string()]);
            }
        }
        text.extend(Dialog::open("C:\\x", true).reading_order()[..2].to_vec());
        text.extend(DIALOG_ITEMS.iter().map(|s| s.to_string()));
        text.extend(DIALOG_FOOTER.iter().map(|s| s.to_string()));
        text.extend(DIALOG_BINDING.iter().map(|s| s.to_string()));
        for t in text {
            for w in bad {
                assert!(!t.to_lowercase().contains(w), "{w:?} in {t:?}");
            }
        }
    }

    #[test]
    fn dates() {
        assert_eq!(date_words("2026-01-05T00:00:00Z"), "5 Jan 2026");
        assert_eq!(date_words("soon"), "soon");
    }

    #[test]
    fn the_dialog_opens_on_not_now_and_enter_esc_and_close_all_answer_it() {
        let mut d = Dialog::open("C:\\Program Files\\Foxglove\\foxglove.exe", false);
        assert_eq!(d.focus, Answer::NotNow);
        assert_eq!(d.reason, None);
        assert_eq!(d.clone().key(Key::Enter), Some(Answer::NotNow));
        assert_eq!(d.clone().key(Key::Escape), Some(Answer::NotNow));
        assert_eq!(d.close(), Answer::NotNow);
        // Tab moves to Allow and back; Esc is still Not now from there; only Enter on Allow allows.
        assert_eq!(d.key(Key::Tab), None);
        assert_eq!(d.focus, Answer::Allow);
        assert_eq!(d.clone().key(Key::Escape), Some(Answer::NotNow));
        assert_eq!(d.clone().key(Key::Enter), Some(Answer::Allow));
        assert_eq!(d.key(Key::ShiftTab), None);
        assert_eq!(d.key(Key::Enter), Some(Answer::NotNow));
    }

    #[test]
    fn the_dialog_reads_in_the_planned_order_with_the_path_and_the_reason_on_top() {
        let path = "C:\\Program Files\\Foxglove\\foxglove.exe";
        let d = Dialog::open(path, true);
        let o = d.reading_order();
        assert_eq!(o[0], DIALOG_REASON);
        assert_eq!(o[1], "Let Foxglove handle your keyboard in Hideout?");
        assert_eq!(o.len(), 1 + 1 + 1 + 4 + 2 + 3 + 2);
        assert!(o.contains(&path.to_string()));
        assert_eq!(&o[o.len() - 2..], ["Allow", "Not now"]);
        let plain = Dialog::open(path, false).reading_order();
        assert_eq!(plain[0], DIALOG_TITLE);
    }
}
