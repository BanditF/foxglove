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
/// `module.<id>.reason` when the launch was refused only because the user's OK is missing (the
/// app file is new or moved since it was allowed). Never the crash wording (UX cut 2).
pub const WAITS_FOR_OK: &str = "Waits for your OK";
/// The row's button for it: opens the permission dialog.
pub const REVIEW_OK: &str = "Review\u{2026}";

/// A button's label as a sentence names it ("Press Review to give it."): without the trailing
/// ellipsis the button itself wears.
pub fn named(button: &str) -> &str {
    let b = button.trim_end();
    b.strip_suffix('\u{2026}')
        .or_else(|| b.strip_suffix("..."))
        .unwrap_or(b)
        .trim_end()
}
/// The ladder's second step line.
pub const DIDNT_HELP: &str = "Starting again didn't help last time.";
/// `install-failed` after a failed pid or exe-hash check at `initialize` (#959).
pub const FILES_DONT_MATCH: &str = "Its files don't match what was installed.";
/// An adopted module whose program isn't there (`module.<id>.failed` is `missing`): the row's
/// state words, the page's reason and `module.<id>.reason`. Module-agnostic.
pub fn not_installed(module: &str, path: &str) -> String {
    format!("{module} isn't installed on this PC \u{2014} it was registered from {path}")
}

/// An adopted module that says another version than it was registered as (`trust.mismatch`,
/// `module.<id>.failed` is `version`): one sentence, with what to do.
pub fn version_differs(module: &str, said: Option<&str>, registered: &str) -> String {
    match said {
        Some(v) => format!(
            "{module} is version {v} but was registered as {registered} \u{2014} register it again"
        ),
        None => format!(
            "{module} is a different version than the {registered} it was registered as \u{2014} register it again"
        ),
    }
}

/// A Modules row's `install-failed` from launching: the row already has the name (#1029).
pub const COULDNT_START: &str = "Couldn't start";
/// A Modules row's buttons (#1029): Open is the secondary one on every row.
pub const OPEN: &str = "Open";
pub const TURN_ON: &str = "Turn on";
pub const UPDATE_HIDEOUT: &str = "Update Hideout";
/// An app Hideout holds whose file next to Hideout now differs from the registered one (the Apps
/// row's state words, the Found card's line). Not a store update: never borrows those words.
pub const CHANGED_SINCE_ADDED: &str = "Changed since you added it";
/// The button for [`CHANGED_SINCE_ADDED`]: registers the changed file again.
pub const UPDATE_IN_HIDEOUT: &str = "Update in Hideout";
/// The row of a changed app that is off: it stays an Off row, and Update in Hideout replaces
/// Turn on (Turn on would only end in a refusal).
pub fn off_changed() -> String {
    format!("Off \u{b7} {}", CHANGED_SINCE_ADDED.to_lowercase())
}

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

/// The safe-mode banner on the Apps page (§7.10, UX plan "Safe mode and finding the
/// culprit"): title, line, main button, second action.
pub const SAFE_MODE: &str = "Safe mode";
pub const SAFE_MODE_LINE: &str = "Hideout closed 3 times in 5 minutes, so no apps were started.";
/// The line when the flag was set by hand (`safe_mode = true` in hideout.toml), not by the
/// closes: it doesn't claim a cause (#1033).
pub const SAFE_MODE_LINE_SET: &str = "Safe mode is on, so no apps were started.";
pub const START_NORMALLY: &str = "Start normally";
pub const FIND_CULPRIT: &str = "Find the app that breaks Hideout";
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

/// The crash retry line: "Restarting in <n> s, try <t> of 5", or "Restarting, try <t> of 5" once
/// the countdown is at 0. `k` is the crashes so far, so the run that follows is try k + 1 (the
/// first start was try 1). The app stops at the fifth crash, so the line reads "try 2 of 5" to
/// "try 5 of 5" and the next thing after "try 5 of 5" failing is "Stopped after 5 crashes".
pub fn restarting_try(secs: u64, k: usize) -> String {
    let t = k + 1;
    match secs.min(60) {
        0 => format!("Restarting, try {t} of 5"),
        n => format!("Restarting in {n} s, try {t} of 5"),
    }
}

/// The crash bar while `restarting`: (first line, second line). `secs` is the backoff step's
/// remaining seconds (never past 60; 0 reads "Restarting, try t of 5"), `k` the crashes in the current
/// ten-minute window.
pub fn crash_bar(module: &str, secs: u64, k: usize) -> (String, String) {
    (
        format!(
            "{module} stopped unexpectedly. {}.",
            restarting_try(secs, k)
        ),
        PLACE_KEPT.into(),
    )
}

/// The Modules page row and the tab tooltip while a planned exit restarts the module (a GPU
/// reset, an update install): no crash happened, so no count (#1159).
pub const RESTARTING_PLANNED: &str = "Restarting\u{2026}";

/// The Modules page row and the tab tooltip while `restarting` after a crash.
pub fn restarting_row(secs: u64, k: usize) -> String {
    restarting_try(secs, k)
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

/// `waiting` (§7.8): the UI name, its way out and the reason line, naming the root once.
pub fn waiting_for(root: &str) -> String {
    format!("Waiting for {root}")
}
pub fn go_to(root: &str) -> String {
    format!("Go to {root}")
}
pub fn waiting_reason(root: &str) -> String {
    format!("{root} is not running.")
}

/// `module.<id>.reason` when sandboxing is on and the module can't run in one (§7.7).
pub const NO_SANDBOX: &str = "It can't run in a sandbox, and Hideout is set to sandbox apps.";

/// `needs-hub` (§7.8).
pub const NEEDS_HUB: &str = "Needs a newer Hideout";

/// `quarantined` (§5.5): the UI name and `module.<id>.reason`.
pub fn quarantined_title(module: &str) -> String {
    format!("{module}'s page couldn't be shown")
}
pub const QUARANTINED_LINE: &str = "Hideout stopped twice while drawing it.";

/// The hot swap's states (§7.6, §7.8).
pub const UPDATING: &str = "Updating\u{2026}";
pub const UPDATE_READY: &str = "Update ready";
pub const UPDATE_WAITING: &str = "Update waiting for you";
pub const FORCE: &str = "Force";
pub const NOT_NOW: &str = "Not now";
pub const REVIEW: &str = "Review";
/// `module.<id>.reason` for `update-busy`; `reason` is the module's own drain reason ("a flash
/// running").
pub fn update_busy(reason: &str) -> String {
    format!(
        "Waits until you finish {}",
        reason.trim().trim_end_matches('.')
    )
}
/// `module.<id>.reason` for `update-waiting`: the first new permission in plain words (a
/// [`hideout_contract::permission::Permission::describe`] line, "see every key you press"),
/// and whether more come with it. `None`: no plain words for it.
pub fn update_asks(first: Option<&str>, more: bool) -> String {
    match (first, more) {
        (Some(p), false) => format!("The new version wants to {p}."),
        (Some(p), true) => format!("The new version wants to {p}, and more."),
        (None, _) => "The new version asks for more permissions.".into(),
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
        not_installed("Art", "C:/Apps/Art/art.exe"),
        version_differs("Art", Some("0.5.0"), "0.4.0"),
        version_differs("Art", None, "0.4.0"),
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
    v.extend([
        waiting_for("Link"),
        go_to("Link"),
        waiting_reason("Link"),
        quarantined_title("Art"),
        update_busy("a flash running"),
        update_asks(Some("see every key you press"), false),
        update_asks(Some("see every key you press"), true),
        update_asks(None, false),
    ]);
    // The consent lines for fact permissions reach this surface through `update_asks`.
    v.extend(
        hideout_contract::permission::fact_lines()
            .iter()
            .map(|l| update_asks(Some(l), false)),
    );
    for s in [
        NEEDS_HUB,
        NO_SANDBOX,
        QUARANTINED_LINE,
        UPDATING,
        UPDATE_READY,
        UPDATE_WAITING,
        FORCE,
        NOT_NOW,
        REVIEW,
    ] {
        v.push(s.into());
    }
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
        WAITS_FOR_OK,
        REVIEW_OK,
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
    // Rebuilds of an app in a dev folder (the folder lines need a path, which the rule below
    // keeps out of drawn text; their path-free reasons are here).
    v.extend([
        rebuilt_row("20:41"),
        rebuilt_said("Art"),
        DEV_CHIP_SAID.into(),
        NEW_BUILD_HELD_FOR_GAME.into(),
        NEW_BUILD_READY.into(),
        REBUILD_STARTS_IT.into(),
        restart_with_new_build("Art"),
        build_unfinished_running("Art"),
        build_unfinished_stopped("Art"),
        build_unreadable("Art"),
        build_needs_ok("Art"),
        build_asks_for_more("Art", 1),
        build_asks_for_more("Art", 3),
        waiting_for_ok("Art", 1),
        waiting_for_ok("Art", 3),
        RUNS_WITH_WHAT_YOU_ALLOWED.into(),
        not_carried(NotCarried::DevModeOff, ""),
        not_carried(NotCarried::AllowedFromCommandLine, ""),
        not_carried(NotCarried::OutsideDevFolders, ""),
    ]);
    for w in [
        NewBuild::Kept,
        NewBuild::AsksMore(1),
        NewBuild::AsksMore(2),
        NewBuild::KeptWaiting(1),
        NewBuild::KeptWaiting(2),
        NewBuild::Unfinished,
        NewBuild::Unreadable,
        NewBuild::HeldForOk,
        NewBuild::StoppedAfterCrashes,
    ] {
        v.push(new_build_line("19 Nov 2026, 20:41", w));
    }
    // Finding the module that breaks Hideout (#1033).
    v.extend(crate::bisect::words::every_string());
    v
}

// ---- Rebuilds of an app in a dev folder (UX plan: 0-click rebuild, section 3) ------------------
// The noun is "app". Never "module", "hash", "enrol" or "grant" in these lines.

/// The Dev chip's tooltip and accessible description, for the dev folder `folder`.
pub fn dev_chip_said(folder: &str) -> String {
    format!("Dev build from {folder}. Rebuilds keep what you allowed.")
}

/// The Dev chip's description when the folder is not known.
pub const DEV_CHIP_SAID: &str = "Dev build. Rebuilds keep what you allowed.";

/// The row's state line after a rebuild restart, until the next state change. `at` is the local
/// time of day, "20:41".
pub fn rebuilt_row(at: &str) -> String {
    format!("On \u{b7} Rebuilt and restarted at {at}")
}

/// What a screen reader is told, once per rebuild restart (polite, only while the window is
/// open).
pub fn rebuilt_said(name: &str) -> String {
    format!("{name} rebuilt and restarted")
}

/// A rebuild is carried but waits for the end of a game.
pub const NEW_BUILD_HELD_FOR_GAME: &str = "New build ready. Restarts after your game.";
/// A rebuild is carried, and the "Restart apps after a rebuild" switch is off.
pub const NEW_BUILD_READY: &str = "New build ready";
/// The extra line under a dev app that stopped after crashes.
pub const REBUILD_STARTS_IT: &str = "Rebuild it and Hideout starts the new build.";
/// The accessible name of the Restart button of a [`NEW_BUILD_READY`] row.
pub fn restart_with_new_build(name: &str) -> String {
    format!("Restart {name} with the new build")
}

/// The new build was written but looks unfinished, and an earlier copy runs.
pub fn build_unfinished_running(name: &str) -> String {
    format!("The new build of {name} looks unfinished. The earlier one keeps running.")
}
/// The same, with nothing running.
pub fn build_unfinished_stopped(name: &str) -> String {
    format!("The new build of {name} looks unfinished. Hideout starts it when the build is done.")
}
/// The new build could not be read at all; the row's button is [`TRY_AGAIN`].
pub fn build_unreadable(name: &str) -> String {
    format!("Hideout couldn't read the new build of {name}. Build it again, or Try again.")
}
/// The new build is not carried (a [`not_carried`] line follows in Details).
pub fn build_needs_ok(name: &str) -> String {
    format!("The new build of {name} needs your OK")
}

fn more_things(n: usize) -> String {
    if n == 1 {
        "1 more thing".into()
    } else {
        format!("{n} more things")
    }
}

/// The header when a rebuilt app asks for more than it was allowed: `n` new items.
pub fn build_asks_for_more(name: &str, n: usize) -> String {
    format!("{name}'s new build asks for {}", more_things(n))
}
/// The header while the new items' origin is not known (before S2's diff): true whether the
/// build grew or items were left unanswered earlier.
pub fn waiting_for_ok(name: &str, n: usize) -> String {
    let things = if n == 1 {
        "1 thing".to_string()
    } else {
        format!("{n} things")
    };
    format!("{name} has {things} waiting for your OK")
}
/// Its second line.
pub const RUNS_WITH_WHAT_YOU_ALLOWED: &str = "It runs with what you allowed until then.";

/// What happened to a new build, for the Details history line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewBuild {
    /// Carried, nothing new asked.
    Kept,
    /// Carried; `n` new items wait for the user.
    AsksMore(usize),
    /// Carried; `n` items (new or left over) still wait for the user.
    KeptWaiting(usize),
    Unfinished,
    Unreadable,
    HeldForOk,
    StoppedAfterCrashes,
}

/// The Details history line for a new build at `when` ("19 Nov 2026, 20:41").
pub fn new_build_line(when: &str, what: NewBuild) -> String {
    let head = format!("New build on {when}");
    match what {
        NewBuild::Kept => format!("{head}. What you allowed stayed."),
        NewBuild::AsksMore(n) => {
            let wait = if n == 1 { "that waits" } else { "those wait" };
            format!("{head}. It asks for {}; {wait} for you.", more_things(n))
        }
        NewBuild::KeptWaiting(n) => {
            let (things, wait) = if n == 1 {
                ("1 thing".to_string(), "waits")
            } else {
                (format!("{n} things"), "wait")
            };
            format!("{head}. What you allowed stayed; {things} still {wait} for you.")
        }
        NewBuild::Unfinished => format!("{head}, not used: it looked unfinished."),
        NewBuild::Unreadable => format!("{head}, not used: Hideout couldn't read it."),
        NewBuild::HeldForOk => format!("{head}, held for your OK."),
        NewBuild::StoppedAfterCrashes => format!(
            "{head} stopped after {} crashes.",
            crate::policy::CRASH_LIMIT
        ),
    }
}

/// Why a new build is not carried (the row 9 reason lines).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotCarried {
    DevModeOff,
    FolderGone,
    OthersCanWrite,
    AllowedFromCommandLine,
    OutsideDevFolders,
}

/// The one Details line for [`NotCarried`]; `folder` is the dev folder where it applies.
pub fn not_carried(why: NotCarried, folder: &str) -> String {
    match why {
        NotCarried::DevModeOff => "Developer mode is off, so Hideout asks again.".into(),
        NotCarried::FolderGone => {
            format!("{folder} is no longer a dev folder, so Hideout asks again.")
        }
        NotCarried::OthersCanWrite => format!(
            "Other accounts on this PC can change {folder} or this app, so Hideout no longer trusts its builds."
        ),
        NotCarried::AllowedFromCommandLine => "This app was allowed from the command line. Allow it here once to keep its OK over rebuilds.".into(),
        NotCarried::OutsideDevFolders => {
            "This build is outside your dev folders, so Hideout asks again.".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_named_in_a_sentence_loses_its_ellipsis() {
        assert_eq!(named(REVIEW_OK), "Review");
        assert_eq!(named("Turn on..."), "Turn on");
        assert_eq!(named(TURN_ON), "Turn on");
        assert_eq!(named("Wait\u{2026} what"), "Wait\u{2026} what");
    }

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
            "Art stopped unexpectedly. Restarting in 4 s, try 3 of 5."
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
                "Hideout closed 3 times in 5 minutes, so no apps were started.",
                "Start normally",
                "Find the app that breaks Hideout"
            )
        );
    }

    #[test]
    fn restart_lines_say_what_is_happening_in_plain_words() {
        let mut all = vec![RESTARTING_PLANNED.to_string()];
        for secs in [0, 4, 90] {
            // The crash counts that can show: the fifth crash stops the app instead.
            for k in 1..=4 {
                all.push(restarting_row(secs, k));
                all.push(crash_bar("Art", secs, k).0);
            }
        }
        for s in &all {
            // No bare "(2 of 5)": say "try 3 of 5". A planned restart has no count at all.
            assert!(
                !s.contains('(') && !s.contains(')'),
                "bracket count in {s:?}"
            );
            assert!(
                s.starts_with("Art stopped") || s.starts_with("Restarting"),
                "{s:?}"
            );
            assert!(!s.to_lowercase().contains("module"), "module in {s:?}");
        }
        assert_eq!(RESTARTING_PLANNED, "Restarting\u{2026}");
        assert_eq!(restarting_row(0, 1), "Restarting, try 2 of 5");
        assert_eq!(restarting_row(4, 4), "Restarting in 4 s, try 5 of 5");
        assert_eq!(
            row_label("Art", &restarting_row(4, 2)),
            "Art, restarting in 4 s, try 3 of 5"
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
            "Art stopped unexpectedly. Restarting, try 3 of 5."
        );
        assert_eq!(restarting_row(0, 2), "Restarting, try 3 of 5");
        assert_eq!(restarting_row(90, 2), "Restarting in 60 s, try 3 of 5");
        for secs in 0..200 {
            assert!(!crash_bar("Art", secs, 1).0.contains(" 0 s"));
        }
    }

    #[test]
    fn the_rebuild_lines_read_as_the_plan_words_them() {
        assert_eq!(
            dev_chip_said("C:\\dev\\out"),
            "Dev build from C:\\dev\\out. Rebuilds keep what you allowed."
        );
        assert_eq!(
            rebuilt_row("20:41"),
            "On \u{b7} Rebuilt and restarted at 20:41"
        );
        assert_eq!(rebuilt_said("Foxglove"), "Foxglove rebuilt and restarted");
        assert_eq!(
            build_asks_for_more("Foxglove", 1),
            "Foxglove's new build asks for 1 more thing"
        );
        assert_eq!(
            build_asks_for_more("Foxglove", 2),
            "Foxglove's new build asks for 2 more things"
        );
        let at = "19 Nov 2026, 20:41";
        assert_eq!(
            new_build_line(at, NewBuild::Kept),
            "New build on 19 Nov 2026, 20:41. What you allowed stayed."
        );
        assert_eq!(
            new_build_line(at, NewBuild::AsksMore(2)),
            "New build on 19 Nov 2026, 20:41. It asks for 2 more things; those wait for you."
        );
        assert_eq!(
            new_build_line(at, NewBuild::AsksMore(1)),
            "New build on 19 Nov 2026, 20:41. It asks for 1 more thing; that waits for you."
        );
        assert_eq!(
            new_build_line(at, NewBuild::StoppedAfterCrashes),
            "New build on 19 Nov 2026, 20:41 stopped after 5 crashes."
        );
        assert_eq!(
            new_build_line(at, NewBuild::Unfinished),
            "New build on 19 Nov 2026, 20:41, not used: it looked unfinished."
        );
        assert_eq!(
            new_build_line(at, NewBuild::Unreadable),
            "New build on 19 Nov 2026, 20:41, not used: Hideout couldn't read it."
        );
        assert_eq!(
            new_build_line(at, NewBuild::HeldForOk),
            "New build on 19 Nov 2026, 20:41, held for your OK."
        );
        assert_eq!(
            NEW_BUILD_HELD_FOR_GAME,
            "New build ready. Restarts after your game."
        );
        assert_eq!(NEW_BUILD_READY, "New build ready");
        assert_eq!(
            REBUILD_STARTS_IT,
            "Rebuild it and Hideout starts the new build."
        );
        assert_eq!(
            restart_with_new_build("Foxglove"),
            "Restart Foxglove with the new build"
        );
        assert_eq!(
            build_unfinished_running("Foxglove"),
            "The new build of Foxglove looks unfinished. The earlier one keeps running."
        );
        assert_eq!(
            build_unfinished_stopped("Foxglove"),
            "The new build of Foxglove looks unfinished. Hideout starts it when the build is done."
        );
        assert_eq!(
            build_unreadable("Foxglove"),
            "Hideout couldn't read the new build of Foxglove. Build it again, or Try again."
        );
        assert_eq!(
            build_needs_ok("Foxglove"),
            "The new build of Foxglove needs your OK"
        );
        assert_eq!(
            RUNS_WITH_WHAT_YOU_ALLOWED,
            "It runs with what you allowed until then."
        );
        let d = "D:/out";
        assert_eq!(
            not_carried(NotCarried::DevModeOff, d),
            "Developer mode is off, so Hideout asks again."
        );
        assert_eq!(
            not_carried(NotCarried::FolderGone, d),
            "D:/out is no longer a dev folder, so Hideout asks again."
        );
        assert_eq!(
            not_carried(NotCarried::OthersCanWrite, d),
            "Other accounts on this PC can change D:/out or this app, so Hideout no longer trusts its builds."
        );
        assert_eq!(
            not_carried(NotCarried::AllowedFromCommandLine, d),
            "This app was allowed from the command line. Allow it here once to keep its OK over rebuilds."
        );
        assert_eq!(
            not_carried(NotCarried::OutsideDevFolders, d),
            "This build is outside your dev folders, so Hideout asks again."
        );
    }

    #[test]
    fn the_rebuild_lines_use_the_house_noun_and_no_jargon() {
        let mut all = vec![
            dev_chip_said("D:\\out"),
            DEV_CHIP_SAID.into(),
            rebuilt_row("20:41"),
            rebuilt_said("Foxglove"),
            NEW_BUILD_HELD_FOR_GAME.into(),
            NEW_BUILD_READY.into(),
            REBUILD_STARTS_IT.into(),
            restart_with_new_build("Foxglove"),
            build_unfinished_running("Foxglove"),
            build_unfinished_stopped("Foxglove"),
            build_unreadable("Foxglove"),
            build_needs_ok("Foxglove"),
            build_asks_for_more("Foxglove", 3),
            RUNS_WITH_WHAT_YOU_ALLOWED.into(),
            SAFE_MODE_LINE.into(),
            SAFE_MODE_LINE_SET.into(),
            FIND_CULPRIT.into(),
            NO_SANDBOX.into(),
        ];
        for w in [
            NewBuild::Kept,
            NewBuild::AsksMore(2),
            NewBuild::Unfinished,
            NewBuild::Unreadable,
            NewBuild::HeldForOk,
            NewBuild::StoppedAfterCrashes,
        ] {
            all.push(new_build_line("19 Nov 2026, 20:41", w));
        }
        for w in [
            NotCarried::DevModeOff,
            NotCarried::FolderGone,
            NotCarried::OthersCanWrite,
            NotCarried::AllowedFromCommandLine,
            NotCarried::OutsideDevFolders,
        ] {
            all.push(not_carried(w, "D:\\out"));
        }
        for s in crate::bisect::words::every_string() {
            assert!(!s.to_lowercase().contains("module"), "module in {s:?}");
        }
        for line in &all {
            // The Apps page's rows and the permission dialog's copy has the same guard in
            // hideout-shell (modules_list.rs, permission_dialog.rs): this crate can't see them.
            for bad in ["module", "hash", "enrol", "grant", "sha", "bound"] {
                assert!(!line.to_lowercase().contains(bad), "{bad:?} in {line:?}");
            }
        }
    }
}
