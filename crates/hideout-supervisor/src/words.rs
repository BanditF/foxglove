// SPDX-License-Identifier: MIT
//! Every string the supervisor puts in front of a person (docs/hideout-supervisor-ux.md,
//! "Wording"). Plain words only: no state ids, error ids, exit codes, pids, hashes or Job Object
//! words ("Never shown"); those go to the log and Copy diagnostics.

/// The stopped card's title, the Modules row and the panel header for `crash-stopped`.
pub const STOPPED: &str = "Stopped after 5 crashes";
/// The crash bar's second line.
pub const PLACE_KEPT: &str = "Your place on this page is kept.";
pub const OPEN_LOG: &str = "Open log";
pub const START_AGAIN: &str = "Start again";
pub const START_DEFAULTS: &str = "Start with default settings";
pub const COPY_DIAGNOSTICS: &str = "Copy diagnostics";
pub const TRY_AGAIN: &str = "Try again";
/// The rollback note's dismiss button.
pub const OK: &str = "OK";
/// `module.<id>.reason` after a launch failure (the card's title already says "Couldn't start").
pub const DIDNT_START: &str = "It didn't start. Open log to see why.";
/// The ladder's second step line.
pub const DIDNT_HELP: &str = "Starting again didn't help last time.";
/// `install-failed` after a failed pid or exe-hash check at `initialize` (#959).
pub const FILES_DONT_MATCH: &str = "Its files don't match what was installed.";
/// A Modules row's `install-failed` from launching: the row already has the name (#1029).
pub const COULDNT_START: &str = "Couldn't start";
/// A Modules row's buttons (#1029): Open is the secondary one on every row.
pub const OPEN: &str = "Open";
pub const TURN_ON: &str = "Turn on";
pub const UPDATE_HIDEOUT: &str = "Update Hideout";

/// A version its publisher withdrew (state `revoked`, §7.8 v1.9, docs/hideout-revoked-ux.md).
/// The short name, for tooltips and the tray tile.
pub const WITHDRAWN: &str = "Withdrawn";
/// The withdrawn card's reason line and `module.<id>.reason` (no "for safety": Hideout doesn't
/// know why).
pub const WITHDRAWN_LINE: &str = "Its publisher withdrew this version, so Hideout stopped it.";
/// The label before the publisher's own words on the card (and nowhere else).
pub const PUBLISHERS_NOTE: &str = "Publisher's note:";
pub const CHECK_FOR_UPDATES: &str = "Check for updates";
/// The button while the check runs (disabled).
pub const CHECKING: &str = "Checking\u{2026}";
pub const NOTHING_NEWER: &str = "No new version yet. Checked just now.";
pub const COULDNT_CHECK: &str = "Couldn't check. Try again later.";
/// The tray tile's two lines.
pub const TILE_WITHDRAWN: (&str, &str) = ("Version withdrawn", "by its publisher");
/// The tab's state words (the strip's tooltip: "Art, withdrawn by its publisher").
pub const WITHDRAWN_TAB: &str = "Withdrawn by its publisher";
/// The longest publisher's note drawn on the card, in characters (decision 1, 11/10).
pub const NOTE_MAX: usize = 160;

/// The safe-mode banner on the Modules page (§7.10, UX plan "Safe mode and finding the
/// culprit"): title, line, main button, second action.
pub const SAFE_MODE: &str = "Safe mode";
pub const SAFE_MODE_LINE: &str = "Hideout closed 3 times in 5 minutes, so no modules were started.";
/// The line when the flag was set by hand (`safe_mode = true` in hideout.toml), not by the
/// closes: it doesn't claim a cause (#1033).
pub const SAFE_MODE_LINE_SET: &str = "Safe mode is on, so no modules were started.";
pub const START_NORMALLY: &str = "Start normally";
pub const FIND_CULPRIT: &str = "Find the module that breaks Hideout";
/// The tooltip of a step that isn't built yet (bisect).
pub const COMES_LATER: &str = "Comes in a later Hideout.";
/// The banner's line: the closes' cause only when they turned safe mode on.
pub fn safe_mode_line(by_closes: bool) -> &'static str {
    if by_closes {
        SAFE_MODE_LINE
    } else {
        SAFE_MODE_LINE_SET
    }
}

/// The tray tooltip in safe mode.
pub const SAFE_MODE_TIP: &str = "Hideout \u{b7} safe mode";

/// A Modules row's accessible name: "<Module>, <state>" with the state's first letter
/// lowered ("Art, stopped after 5 crashes", the red mark's words).
pub fn row_label(module: &str, state: &str) -> String {
    let mut c = state.chars();
    let state = match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    };
    format!("{module}, {state}")
}

/// "Restarting in <n> s", or "Restarting…" once the countdown is at 0 (never "0 s", never past
/// 60 s).
fn restarting_in(secs: u64) -> String {
    match secs.min(60) {
        0 => "Restarting\u{2026}".into(),
        n => format!("Restarting in {n} s"),
    }
}

/// The crash bar while `restarting`: (first line, second line). `secs` is the backoff step's
/// remaining seconds (never past 60; 0 reads "Restarting…"), `k` the crashes in the current
/// ten-minute window.
pub fn crash_bar(module: &str, secs: u64, k: usize) -> (String, String) {
    (
        format!(
            "{module} stopped unexpectedly. {} ({k} of 5).",
            restarting_in(secs)
        ),
        PLACE_KEPT.into(),
    )
}

/// The Modules page row and the tab tooltip while `restarting`.
pub fn restarting_row(secs: u64, k: usize) -> String {
    format!("{} ({k} of 5)", restarting_in(secs))
}

/// What a screen reader says once per crash (the crash bar's polite live region). The countdown
/// is never announced.
pub fn crash_announcement(module: &str) -> String {
    format!("{module} stopped unexpectedly, restarting")
}

/// The red mark's accessible name for a stopped module.
pub fn stopped_mark(module: &str) -> String {
    format!("{module}, stopped after 5 crashes")
}

/// "Turn off <Module>": the ladder's last step.
pub fn turn_off(module: &str) -> String {
    format!("Turn off {module}")
}

/// "Open <Module>": the notification's second button.
pub fn open_module(module: &str) -> String {
    format!("Open {module}")
}

/// `install-failed` when the failure came from launching (the fact `module.<id>.failed` is
/// `launch`): the tab, the Modules row and the header.
pub fn couldnt_start_title(module: &str) -> String {
    format!("Couldn't start {module}")
}

/// The stopped card's body line.
pub fn stopped_body(module: &str) -> String {
    format!("{module} crashed 5 times in 10 minutes.")
}

/// The ladder's third step line.
pub fn worked_before(module: &str, version: &str) -> String {
    format!("{module} {version} worked before.")
}

/// "Go back to <previous>".
pub fn go_back(version: &str) -> String {
    format!("Go back to {version}")
}

/// The tray menu lines for a stopped module: (status, action).
pub fn tray_lines(module: &str) -> (String, String) {
    (
        format!("{module} stopped"),
        format!("Open {module} to start it again"),
    )
}

/// The notification for a stopped module (once per module per sign-in): title, body, buttons.
pub fn stopped_notification(module: &str) -> (String, String, Vec<String>) {
    (
        format!("{module} stopped"),
        "It crashed 5 times in 10 minutes.".into(),
        vec![START_AGAIN.into(), open_module(module)],
    )
}

/// The automatic rollback note and notification: (title, body). `old` is the version it went
/// back to, `new` the one that crashed.
pub fn rollback(module: &str, old: &str, new: &str) -> (String, String) {
    (
        format!("Went back to {module} {old}"),
        format!("{new} crashed 5 times after the update."),
    )
}

/// `install-failed` from the launch check: (title, body, buttons).
pub fn couldnt_start(module: &str) -> (String, String, Vec<String>) {
    (
        couldnt_start_title(module),
        FILES_DONT_MATCH.into(),
        vec![TRY_AGAIN.into(), OPEN_LOG.into()],
    )
}

/// The withdrawn card's title: "<Module> <version> was withdrawn" (the version is in it: only
/// that version is).
pub fn withdrawn_title(module: &str, version: &str) -> String {
    format!("{module} {version} was withdrawn")
}

/// The Modules row's state words: "Withdrawn · 0.5.0".
pub fn withdrawn_row(version: &str) -> String {
    format!("{WITHDRAWN} \u{b7} {version}")
}

/// A withdrawn module after Turn off: "Off · 0.5.0 was withdrawn" (row and Off card).
pub fn off_withdrawn(version: &str) -> String {
    format!("Off \u{b7} {version} was withdrawn")
}

/// The red mark's tooltip and accessible name: "Art, withdrawn by its publisher".
pub fn withdrawn_mark(module: &str) -> String {
    row_label(module, WITHDRAWN_TAB)
}

/// The card's status line while Check for updates runs.
pub fn checking_source(module: &str) -> String {
    format!("Checking {module}'s source\u{2026}")
}

/// The one polite announcement when the state arrives with the page on screen. The note is
/// never in it.
pub fn withdrawn_announcement(module: &str, version: &str) -> String {
    format!("{module} {version} was withdrawn by its publisher and stopped")
}

/// The tray menu lines for a withdrawn module: (status, action).
pub fn withdrawn_tray_lines(module: &str) -> (String, String) {
    (
        format!("{module} was withdrawn"),
        format!("Open {module} to check for updates"),
    )
}

/// The notification, once per withdrawn version: title, body, buttons. The publisher's note is
/// never in it (outside the window, no label, no cap).
pub fn withdrawn_notification(module: &str, version: &str) -> (String, String, Vec<String>) {
    (
        withdrawn_title(module, version),
        "Its publisher withdrew it, so Hideout stopped it.".into(),
        vec![CHECK_FOR_UPDATES.into(), open_module(module)],
    )
}

/// Characters dropped from a publisher's note: C0 and C1 controls, bidi marks, overrides and
/// isolates, zero-width characters and other invisible format characters. Whitespace (tabs,
/// newlines) is not dropped here: [`clean_note`] folds it to one space.
pub fn hidden_char(c: char) -> bool {
    if c.is_whitespace() {
        return false;
    }
    matches!(c as u32,
        0x00..=0x1F | 0x7F..=0x9F // C0, DEL, C1
        | 0x00AD // soft hyphen
        | 0x061C // Arabic letter mark
        | 0x180E // Mongolian vowel separator
        | 0x200B..=0x200F // zero-width space and joiners, LRM, RLM
        | 0x202A..=0x202E // bidi embeddings and overrides
        | 0x2060..=0x2064 // word joiner, invisible operators
        | 0x2066..=0x206F // bidi isolates, deprecated format characters
        | 0xFEFF // zero-width no-break space
        | 0xFFF9..=0xFFFB // interlinear annotation
        | 0xE0000..=0xE007F) // tags
}

/// The publisher's note as the card draws it (decision 1, 11/10): plain text, control and
/// format characters dropped, every run of whitespace one space, the ends trimmed, and at most
/// [`NOTE_MAX`] characters, cut on a whole character with "…". Empty: the card leaves the note
/// lines out.
pub fn clean_note(raw: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in raw.chars().filter(|c| !hidden_char(*c)) {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    if out.chars().count() <= NOTE_MAX {
        return out;
    }
    let mut cut: String = out.chars().take(NOTE_MAX - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('\u{2026}');
    cut
}

/// The full note for Copy diagnostics: as written, with control and format characters (and
/// whitespace other than a space) escaped as `\u{202e}`, so the text can't reorder or hide
/// itself.
pub fn escape_note(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if hidden_char(c) || (c.is_whitespace() && c != ' ') {
                format!("\\u{{{:x}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Which step of the recovery ladder the stopped card offers (UX plan, one action per step).
/// Published as the fact `module.<id>.recovery`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    StartAgain,
    Defaults,
    GoBack,
    /// No previous version to go back to: Copy diagnostics and Turn off <Module>.
    Last,
}

impl Recovery {
    /// The step for the `stops`-th crash stop this sign-in (1-based). Without a previous version
    /// the third stop goes to the last step (step 3 is hidden).
    pub fn for_stops(stops: u32, has_previous: bool) -> Recovery {
        match stops {
            0 | 1 => Recovery::StartAgain,
            2 => Recovery::Defaults,
            _ if has_previous => Recovery::GoBack,
            _ => Recovery::Last,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Recovery::StartAgain => "start-again",
            Recovery::Defaults => "defaults",
            Recovery::GoBack => "go-back",
            Recovery::Last => "last",
        }
    }

    /// The step a `module.<id>.recovery` value names; anything else is the first step.
    pub fn from_id(id: Option<&str>) -> Recovery {
        match id {
            Some("defaults") => Recovery::Defaults,
            Some("go-back") => Recovery::GoBack,
            Some("last") => Recovery::Last,
            _ => Recovery::StartAgain,
        }
    }

    /// The card's main button for this step. `previous` is the version to go back to; a
    /// `go-back` without one reads like the last step.
    pub fn button(self, module: &str, previous: Option<&str>) -> String {
        match (self, previous) {
            (Recovery::StartAgain, _) => START_AGAIN.into(),
            (Recovery::Defaults, _) => START_DEFAULTS.into(),
            (Recovery::GoBack, Some(v)) => go_back(v),
            (Recovery::GoBack, None) | (Recovery::Last, _) => turn_off(module),
        }
    }

    /// The line under the card's body for this step, if any.
    pub fn line(self, module: &str, previous: Option<&str>) -> Option<String> {
        match (self, previous) {
            (Recovery::Defaults, _) => Some(DIDNT_HELP.into()),
            (Recovery::GoBack, Some(v)) => Some(worked_before(module, v)),
            _ => None,
        }
    }
}

/// "Never shown" (UX plan): none of these may reach a drawn string.
pub const NEVER_SHOWN: &[&str] = &[
    "crash-stopped",
    "revoked",
    "trust.",
    "lease",
    "catalog",
    "seq",
    "signature",
    "install-failed",
    "rpc.",
    "frame-too-large",
    "exit code",
    "pid",
    "hash",
    "Job Object",
    "AppContainer",
    "initialize",
    "backoff",
];

/// Every string this file makes, for the "Never shown" tests (here and in the shell).
pub fn every_string() -> Vec<String> {
    let (a, b) = crash_bar("Art", 4, 2);
    let (t, m) = tray_lines("Art");
    let (nt, nb, na) = stopped_notification("Art");
    let (rt, rb) = rollback("Art", "0.4.2", "0.5.0");
    let (ct, cb, ca) = couldnt_start("Art");
    let (wt, wm) = withdrawn_tray_lines("Art");
    let (wnt, wnb, wna) = withdrawn_notification("Art", "0.5.0");
    let mut v = vec![
        withdrawn_title("Art", "0.5.0"),
        withdrawn_row("0.5.0"),
        off_withdrawn("0.5.0"),
        withdrawn_mark("Art"),
        checking_source("Art"),
        withdrawn_announcement("Art", "0.5.0"),
        wt,
        wm,
        wnt,
        wnb,
        a,
        b,
        restarting_row(4, 2),
        stopped_body("Art"),
        worked_before("Art", "0.4.2"),
        go_back("0.4.2"),
        crash_bar("Art", 0, 4).0,
        restarting_row(0, 4),
        crash_announcement("Art"),
        stopped_mark("Art"),
        turn_off("Art"),
        open_module("Art"),
        couldnt_start_title("Art"),
        t,
        m,
        nt,
        nb,
        rt,
        rb,
        ct,
        cb,
    ];
    v.extend(na);
    v.extend(ca);
    v.extend(wna);
    for r in [
        Recovery::StartAgain,
        Recovery::Defaults,
        Recovery::GoBack,
        Recovery::Last,
    ] {
        for prev in [Some("0.4.2"), None] {
            v.push(r.button("Art", prev));
            v.extend(r.line("Art", prev));
        }
    }
    for s in [
        STOPPED,
        PLACE_KEPT,
        OPEN_LOG,
        START_AGAIN,
        START_DEFAULTS,
        COPY_DIAGNOSTICS,
        TRY_AGAIN,
        DIDNT_HELP,
        OK,
        DIDNT_START,
        COULDNT_START,
        OPEN,
        TURN_ON,
        UPDATE_HIDEOUT,
        SAFE_MODE,
        SAFE_MODE_LINE,
        SAFE_MODE_LINE_SET,
        START_NORMALLY,
        FIND_CULPRIT,
        COMES_LATER,
        SAFE_MODE_TIP,
        WITHDRAWN,
        WITHDRAWN_LINE,
        PUBLISHERS_NOTE,
        CHECK_FOR_UPDATES,
        CHECKING,
        NOTHING_NEWER,
        COULDNT_CHECK,
        TILE_WITHDRAWN.0,
        TILE_WITHDRAWN.1,
        WITHDRAWN_TAB,
    ] {
        v.push(s.into());
    }
    // Finding the module that breaks Hideout (#1033).
    v.extend(crate::bisect::words::every_string());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ids_in_drawn_strings() {
        for s in every_string() {
            for n in NEVER_SHOWN {
                assert!(
                    !s.to_lowercase().contains(&n.to_lowercase()),
                    "{s:?} shows {n:?}"
                );
            }
        }
    }

    #[test]
    fn wording_matches_the_ux_plan() {
        assert_eq!(
            crash_bar("Art", 4, 2).0,
            "Art stopped unexpectedly. Restarting in 4 s (2 of 5)."
        );
        assert!(crash_bar("Art", 300, 1).0.contains("60 s"));
        assert_eq!(
            rollback("Art", "0.4.2", "0.5.0"),
            (
                "Went back to Art 0.4.2".to_string(),
                "0.5.0 crashed 5 times after the update.".to_string()
            )
        );
        assert_eq!(STOPPED, "Stopped after 5 crashes");
        assert_eq!(stopped_body("Art"), "Art crashed 5 times in 10 minutes.");
        assert_eq!(
            stopped_notification("Art").2,
            vec!["Start again".to_string(), "Open Art".into()]
        );
        assert_eq!(tray_lines("Art").1, "Open Art to start it again");
        assert_eq!(row_label("Art", STOPPED), stopped_mark("Art"));
        assert_eq!(row_label("Dock", COULDNT_START), "Dock, couldn't start");
        assert_eq!(
            (SAFE_MODE, SAFE_MODE_LINE, START_NORMALLY, FIND_CULPRIT),
            (
                "Safe mode",
                "Hideout closed 3 times in 5 minutes, so no modules were started.",
                "Start normally",
                "Find the module that breaks Hideout"
            )
        );
    }

    #[test]
    fn ladder_steps() {
        assert_eq!(Recovery::for_stops(1, true), Recovery::StartAgain);
        assert_eq!(Recovery::for_stops(2, true), Recovery::Defaults);
        assert_eq!(Recovery::for_stops(3, true), Recovery::GoBack);
        assert_eq!(Recovery::for_stops(3, false), Recovery::Last);
        assert_eq!(Recovery::for_stops(7, false), Recovery::Last);
        for r in [
            Recovery::StartAgain,
            Recovery::Defaults,
            Recovery::GoBack,
            Recovery::Last,
        ] {
            assert_eq!(Recovery::from_id(Some(r.id())), r);
        }
        assert_eq!(Recovery::from_id(None), Recovery::StartAgain);
        assert_eq!(
            Recovery::GoBack.button("Art", Some("0.4.2")),
            "Go back to 0.4.2"
        );
        assert_eq!(
            Recovery::GoBack.line("Art", Some("0.4.2")).as_deref(),
            Some("Art 0.4.2 worked before.")
        );
        assert_eq!(Recovery::Last.button("Art", None), "Turn off Art");
        assert_eq!(Recovery::Last.line("Art", None), None);
    }

    /// The withdrawn strings (docs/hideout-revoked-ux.md, decided 11/10).
    #[test]
    fn withdrawn_wording_matches_the_ux_plan() {
        assert_eq!(withdrawn_title("Art", "0.5.0"), "Art 0.5.0 was withdrawn");
        assert_eq!(
            WITHDRAWN_LINE,
            "Its publisher withdrew this version, so Hideout stopped it."
        );
        assert!(!WITHDRAWN_LINE.contains("safety"));
        assert_eq!(withdrawn_row("0.5.0"), "Withdrawn \u{b7} 0.5.0");
        assert_eq!(off_withdrawn("0.5.0"), "Off \u{b7} 0.5.0 was withdrawn");
        assert_eq!(withdrawn_mark("Art"), "Art, withdrawn by its publisher");
        assert_eq!(
            withdrawn_tray_lines("Art"),
            (
                "Art was withdrawn".to_string(),
                "Open Art to check for updates".to_string()
            )
        );
        assert_eq!(
            withdrawn_notification("Art", "0.5.0"),
            (
                "Art 0.5.0 was withdrawn".to_string(),
                "Its publisher withdrew it, so Hideout stopped it.".to_string(),
                vec!["Check for updates".to_string(), "Open Art".into()]
            )
        );
        assert_eq!(checking_source("Art"), "Checking Art's source\u{2026}");
    }

    /// Decision 1: the note is cleaned and capped at 160 characters.
    #[test]
    fn the_publishers_note_is_cleaned_and_capped() {
        // A bidi override, a newline, a tab, zero-width characters, a C1 control.
        let raw = "  Please\u{202e}draw\u{202c} update\n\tto 0.5.1.\u{200b}\u{feff} \u{85}See https://x.test  ";
        assert_eq!(
            clean_note(raw),
            "Pleasedraw update to 0.5.1. See https://x.test"
        );
        assert_eq!(clean_note(""), "");
        assert_eq!(clean_note("\u{2066}\u{2069}\n \u{200d}"), "");
        // 300 characters: at most 160, ending in the ellipsis, cut on a whole character.
        let long: String = "\u{e9}a".repeat(150);
        let c = clean_note(&long);
        assert_eq!(c.chars().count(), NOTE_MAX);
        assert!(c.ends_with('\u{2026}'));
        let words = "word ".repeat(60);
        let c = clean_note(&words);
        assert!(
            c.chars().count() <= NOTE_MAX && c.ends_with("word\u{2026}"),
            "{c:?}"
        );
        // Exactly 160 stays whole.
        let exact = "x".repeat(NOTE_MAX);
        assert_eq!(clean_note(&exact), exact);
        // Copy diagnostics keeps every character, escaped.
        assert_eq!(escape_note("a\u{202e}b\nc d"), "a\\u{202e}b\\u{a}c d");
    }

    #[test]
    fn the_countdown_never_reads_0_s() {
        assert_eq!(
            crash_bar("Art", 0, 2).0,
            "Art stopped unexpectedly. Restarting\u{2026} (2 of 5)."
        );
        assert_eq!(restarting_row(0, 2), "Restarting\u{2026} (2 of 5)");
        assert_eq!(restarting_row(90, 2), "Restarting in 60 s (2 of 5)");
        for secs in 0..200 {
            assert!(!crash_bar("Art", secs, 1).0.contains(" 0 s"));
        }
    }
}
