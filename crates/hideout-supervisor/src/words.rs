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

/// The safe-mode banner on the Modules page (§7.10, UX plan "Safe mode and finding the
/// culprit"): title, line, main button, second action.
pub const SAFE_MODE: &str = "Safe mode";
pub const SAFE_MODE_LINE: &str = "Hideout closed 3 times in 5 minutes, so no modules were started.";
pub const START_NORMALLY: &str = "Start normally";
pub const FIND_CULPRIT: &str = "Find the module that breaks Hideout";
/// The tooltip of a step that isn't built yet (bisect).
pub const COMES_LATER: &str = "Comes in a later Hideout.";
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
    let mut v = vec![
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
        START_NORMALLY,
        FIND_CULPRIT,
        COMES_LATER,
        SAFE_MODE_TIP,
    ] {
        v.push(s.into());
    }
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
