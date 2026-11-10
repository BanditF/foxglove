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

/// The crash bar while `restarting`: (first line, second line). `secs` is the backoff step's
/// remaining seconds (never past 60), `k` the crashes in the current ten-minute window.
pub fn crash_bar(module: &str, secs: u64, k: usize) -> (String, String) {
    (
        format!(
            "{module} stopped unexpectedly. Restarting in {} s ({k} of 5).",
            secs.min(60)
        ),
        PLACE_KEPT.into(),
    )
}

/// The Modules page row and the tab tooltip while `restarting`.
pub fn restarting_row(secs: u64, k: usize) -> String {
    format!("Restarting in {} s ({k} of 5)", secs.min(60))
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
        vec![START_AGAIN.into(), format!("Open {module}")],
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
        format!("Couldn't start {module}"),
        FILES_DONT_MATCH.into(),
        vec![TRY_AGAIN.into(), OPEN_LOG.into()],
    )
}

/// Which step of the recovery ladder the stopped card offers first (UX plan, one action per
/// step). Published as the fact `module.<id>.recovery`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    StartAgain,
    Defaults,
    GoBack,
}

impl Recovery {
    /// The step for the `stops`-th crash stop this sign-in (1-based).
    pub fn for_stops(stops: u32, has_previous: bool) -> Recovery {
        match stops {
            0 | 1 => Recovery::StartAgain,
            2 => Recovery::Defaults,
            _ if has_previous => Recovery::GoBack,
            _ => Recovery::Defaults,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Recovery::StartAgain => "start-again",
            Recovery::Defaults => "defaults",
            Recovery::GoBack => "go-back",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Never shown" (UX plan): none of these may reach a drawn string.
    const NEVER: &[&str] = &[
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

    fn all() -> Vec<String> {
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
        ] {
            v.push(s.into());
        }
        v
    }

    #[test]
    fn no_ids_in_drawn_strings() {
        for s in all() {
            for n in NEVER {
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
    }

    #[test]
    fn ladder_steps() {
        assert_eq!(Recovery::for_stops(1, true), Recovery::StartAgain);
        assert_eq!(Recovery::for_stops(2, true), Recovery::Defaults);
        assert_eq!(Recovery::for_stops(3, true), Recovery::GoBack);
        assert_eq!(Recovery::for_stops(3, false), Recovery::Defaults);
    }
}
