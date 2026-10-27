//! The art flow's words (#518, #520): the badges, what happens after the keyboard restarts, and
//! the sending strip's sentences. Each comes from one table here, so the Screens page and the
//! tray panel say the same thing, and no relay state name or raw error reaches the UI.

use digitalis::protocol::{right_state, ArtStatus};

use super::art_service::{Job, JobKind, Snapshot};

/// How a badge or strip line is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Something is running.
    Busy,
    Ok,
    Warn,
    Error,
    /// No colour: a plain fact.
    Plain,
}

/// One badge per piece (the proposal's words table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    /// Kept in one of the keyboard's places.
    OnKeyboard,
    /// One of the firmware's own pieces.
    ComesWithKeyboard,
    /// Sent from this PC and not kept: gone after the keyboard restarts.
    UntilRestart,
    /// Only on this PC.
    ThisPc,
    /// Being sent now, in percent.
    Sending(u8),
    /// Bigger than the right screen takes.
    TooBig,
}

impl Badge {
    pub fn text(self) -> String {
        match self {
            Badge::OnKeyboard => "ON KEYBOARD".into(),
            Badge::ComesWithKeyboard => "COMES WITH KEYBOARD".into(),
            Badge::UntilRestart => "SHOWING UNTIL RESTART".into(),
            Badge::ThisPc => "THIS PC".into(),
            Badge::Sending(n) => format!("SENDING {n}%"),
            Badge::TooBig => "TOO BIG".into(),
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            Badge::OnKeyboard => Tone::Ok,
            Badge::ComesWithKeyboard | Badge::ThisPc => Tone::Plain,
            Badge::UntilRestart => Tone::Warn,
            Badge::Sending(_) => Tone::Busy,
            Badge::TooBig => Tone::Error,
        }
    }
}

/// What the right screen shows, from the keyboard's own report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Showing {
    /// Kept place n (0-based).
    Kept(u8),
    /// Built-in set n.
    BuiltIn(i8),
    /// A piece sent from this PC, not kept.
    Sent,
    /// The right half is in another state (receiving, a damaged piece, saving): the raw state.
    Other(u8),
}

pub fn showing(st: &ArtStatus) -> Showing {
    let count = i16::from(st.right_set_count);
    let set = i16::from(st.right_set);
    let kept = st.flash_slots > 0 && set >= count && set < count + i16::from(st.flash_slots);
    match st.right_state {
        // A kept place plays like a sent piece; its set number says which place.
        right_state::PLAYING | right_state::SAVED if kept => Showing::Kept((set - count) as u8),
        right_state::BUILT_IN if kept => Showing::Kept((set - count) as u8),
        right_state::BUILT_IN => Showing::BuiltIn(st.right_set),
        right_state::PLAYING | right_state::SAVED => Showing::Sent,
        other => Showing::Other(other),
    }
}

/// The badge for what shows now, if it has one.
pub fn showing_badge(showing: Showing) -> Option<Badge> {
    match showing {
        Showing::Kept(_) => Some(Badge::OnKeyboard),
        Showing::BuiltIn(_) => Some(Badge::ComesWithKeyboard),
        Showing::Sent => Some(Badge::UntilRestart),
        Showing::Other(_) => None,
    }
}

/// What happens to the piece on screen when the keyboard restarts. The right half starts on a
/// random piece of its own; Foxglove puts the remembered piece back, but only while it runs.
pub fn after_restart(showing: Showing, remembered: bool) -> Option<&'static str> {
    Some(match (showing, remembered) {
        (Showing::Kept(_), true) => {
            "Kept on the keyboard. After a restart, Foxglove shows it again while it runs on this PC."
        }
        (Showing::Kept(_), false) => {
            "Kept on the keyboard. After a restart, the keyboard picks one of its own pieces."
        }
        (Showing::BuiltIn(_), true) => {
            "Comes with the keyboard. After a restart, Foxglove shows it again while it runs on this PC."
        }
        (Showing::BuiltIn(_), false) => {
            "Comes with the keyboard. After a restart, the keyboard picks one of its own pieces."
        }
        (Showing::Sent, true) => {
            "Not kept on the keyboard. After a restart, Foxglove sends it again while it runs on this PC."
        }
        (Showing::Sent, false) => "Not kept on the keyboard: it's gone after a restart.",
        (Showing::Other(_), _) => return None,
    })
}

/// The right half's own states (`RIGHT_STATES`), as a sentence for the Showing-now line.
pub fn right_state_words(state: u8) -> &'static str {
    match state {
        0 => "Showing a piece that comes with the keyboard.",
        1 => "Receiving a piece from this PC…",
        2 => "Showing a piece sent from this PC.",
        3 | 4 => "The last piece arrived damaged, so the right screen kept what it had.",
        5 => "The last piece was too big for the right screen.",
        6 => "Keeping a piece on the keyboard…",
        7 => "Kept on the keyboard.",
        8 => "The keyboard couldn't keep the last piece.",
        _ => "The right screen is busy.",
    }
}

/// Why something went wrong, worked out from the error and the relay's last status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Cancelled,
    Paused,
    NoKeyboard,
    /// The right half doesn't answer (asleep, out of range, or the relay isn't up yet).
    RightAsleep,
    Busy,
    TooBig,
    Damaged,
    /// The keyboard stopped answering partway.
    LinkLost,
    KeepFailed,
    NotArt,
    FileMissing,
    /// It shows, but the choice couldn't be saved for after a restart.
    NotRemembered,
    ReadKept,
    Crashed,
    Other,
}

/// The relay's error codes (`ERRORS` in digitalis' protocol).
mod relay_error {
    pub const NOT_CONNECTED: u8 = 1;
    pub const TOO_BIG: u8 = 2;
    pub const BUSY: u8 = 3;
    pub const OUT_OF_RANGE: u8 = 4;
    pub const CRC_LEFT: u8 = 5;
    pub const BLE_WRITE: u8 = 6;
    pub const REJECTED: u8 = 7;
}

/// Sorts an art service error (the strings `art_service` and digitalis make) into a cause.
pub fn problem(error: &str, status: Option<&ArtStatus>) -> Problem {
    let e = error.to_ascii_lowercase();
    let has = |s: &str| e.contains(s);
    if has("cancelled") {
        return Problem::Cancelled;
    }
    if error == super::pause::REFUSED {
        return Problem::Paused;
    }
    if has("waiting for a keyboard") || has("no art status yet") {
        return Problem::NoKeyboard;
    }
    if has("panicked") {
        return Problem::Crashed;
    }
    if has("saving pinned art") {
        return Problem::NotRemembered;
    }
    if has("not an art blob") {
        return Problem::NotArt;
    }
    if has("reading ") && !has("flash slot") {
        return Problem::FileMissing;
    }
    // The relay's own report says most.
    if let Some(st) = status {
        match st.error {
            relay_error::NOT_CONNECTED => return Problem::RightAsleep,
            relay_error::TOO_BIG => return Problem::TooBig,
            relay_error::BUSY => return Problem::Busy,
            relay_error::OUT_OF_RANGE | relay_error::CRC_LEFT | relay_error::REJECTED => {
                return Problem::Damaged
            }
            relay_error::BLE_WRITE => return Problem::LinkLost,
            _ => {}
        }
        match st.right_state {
            3 | 4 => return Problem::Damaged,
            5 => return Problem::TooBig,
            right_state::SAVE_FAILED => return Problem::KeepFailed,
            _ => {}
        }
    }
    if has("failed on the right half") || (has("save failed") && !has("select")) {
        return Problem::KeepFailed;
    }
    if has("is more than") || has("the most is") || has("too big") {
        return Problem::TooBig;
    }
    if has("not connected") || has("no relay") || status.is_some_and(|st| st.link == 0) {
        return Problem::RightAsleep;
    }
    if has("busy") {
        return Problem::Busy;
    }
    if has("flash slot") || has("slot read") {
        return Problem::ReadKept;
    }
    if has("did not finish")
        || has("timed out")
        || has("timeout")
        || has("disconnect")
        || has("no reply")
        || has("push failed")
        || has("select failed")
        || has("refused")
    {
        return Problem::LinkLost;
    }
    Problem::Other
}

/// A strip button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Cancel,
    TryAgain,
}

/// One line of the sending strip, shared by the Screens page and the tray panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub tone: Tone,
    pub text: String,
    pub detail: Option<String>,
    /// 0..1 while a send reports progress.
    pub progress: Option<f32>,
    pub button: Option<Button>,
}

impl Problem {
    /// The sentence for this problem while doing `job`, a smaller line, and the way out.
    pub fn words(self, job: &Job) -> (String, Option<String>, Option<Button>) {
        let name = &job.name;
        let unchanged = Some("Nothing changed on the keyboard.".to_string());
        let again = Some(Button::TryAgain);
        match self {
            Problem::Cancelled => (format!("Stopped sending {name}."), unchanged, None),
            Problem::Paused => (
                "Foxglove is paused. Resume it to change the art.".into(),
                None,
                None,
            ),
            Problem::NoKeyboard => (
                "The keyboard isn't connected. Connect it, then:".into(),
                None,
                again,
            ),
            Problem::RightAsleep => (
                "The right half is asleep. Press any key on it, then:".into(),
                None,
                again,
            ),
            Problem::Busy => (
                "The keyboard is busy with another change. Wait a moment, then:".into(),
                None,
                again,
            ),
            Problem::TooBig => (
                format!("{name} is too big for the right screen."),
                Some("A piece can be at most 16 KiB.".into()),
                None,
            ),
            Problem::Damaged => (
                format!("{name} arrived damaged on the keyboard."),
                Some("Nothing changed on the keyboard; the right screen kept what it had.".into()),
                again,
            ),
            Problem::LinkLost => (
                match job.kind {
                    JobKind::Show => {
                        format!("Couldn't send {name}: the keyboard stopped answering.")
                    }
                    JobKind::Keep => {
                        format!("Couldn't keep {name}: the keyboard stopped answering.")
                    }
                    JobKind::Remove => {
                        format!("Couldn't remove {name}: the keyboard stopped answering.")
                    }
                    JobKind::Read => "Couldn't read the keyboard: it stopped answering.".into(),
                },
                unchanged,
                again,
            ),
            Problem::KeepFailed => (
                format!("The keyboard couldn't keep {name}."),
                Some(
                    "Its storage reported an error; the place it was saving to may now be empty."
                        .into(),
                ),
                again,
            ),
            Problem::NotArt => (
                format!("{name} isn't art for the right screen."),
                Some("Foxglove shows .bin files made for it (see the art guide).".into()),
                None,
            ),
            Problem::FileMissing => (
                format!("Couldn't open {name}'s file on this PC."),
                Some("It may have been moved or deleted.".into()),
                None,
            ),
            Problem::NotRemembered => (
                format!(
                    "{name} is showing, but Foxglove couldn't remember it for after a restart."
                ),
                None,
                None,
            ),
            Problem::ReadKept => ("Couldn't read what the keyboard keeps.".into(), None, again),
            Problem::Crashed => (
                "Foxglove's art helper stopped and restarted.".into(),
                None,
                again,
            ),
            Problem::Other => (
                match job.kind {
                    JobKind::Show => format!("Couldn't show {name}."),
                    JobKind::Keep => format!("Couldn't keep {name} on the keyboard."),
                    JobKind::Remove => format!("Couldn't remove {name}."),
                    JobKind::Read => "Couldn't read the keyboard.".into(),
                },
                None,
                again,
            ),
        }
    }
}

/// How long a success line stays.
pub const DONE_FOR: std::time::Duration = std::time::Duration::from_secs(5);

/// What the strip says now, if anything: the running job, else how the last one ended.
pub fn strip(s: &Snapshot) -> Option<Strip> {
    if s.busy {
        let job = s.job.as_ref()?;
        let name = &job.name;
        let sending = matches!(job.kind, JobKind::Show | JobKind::Keep);
        let (text, progress) = match (job.kind, s.push_progress) {
            (JobKind::Show | JobKind::Keep, Some((done, total))) => (
                format!("Sending {name} to the right screen…"),
                Some(if total == 0 {
                    0.0
                } else {
                    f32::from(done) / f32::from(total)
                }),
            ),
            (JobKind::Show, None) => (format!("Showing {name}…"), None),
            (JobKind::Keep, None) => (format!("Keeping {name} on the keyboard…"), None),
            (JobKind::Remove, _) => (format!("Removing {name} from the keyboard…"), None),
            (JobKind::Read, _) => ("Reading what the keyboard keeps…".into(), None),
        };
        let saving = matches!(job.kind, JobKind::Remove)
            || (job.kind == JobKind::Keep && s.push_progress.is_none());
        return Some(Strip {
            tone: Tone::Busy,
            text,
            detail: saving.then(|| "Saving: don't unplug the keyboard.".into()),
            progress,
            button: (sending && s.push_progress.is_some()).then_some(Button::Cancel),
        });
    }
    let outcome = s.outcome.as_ref()?;
    match &outcome.result {
        Ok(()) => {
            if outcome.at.elapsed() > DONE_FOR || outcome.job.quiet {
                return None;
            }
            let name = &outcome.job.name;
            let text = match outcome.job.kind {
                JobKind::Show => match s.status.as_ref().map(showing) {
                    Some(Showing::Sent) => {
                        format!("{name} is showing until the keyboard restarts (not kept).")
                    }
                    _ => format!("{name} is on the right screen."),
                },
                JobKind::Keep => format!("{name} is on the right screen and kept on the keyboard."),
                JobKind::Remove => format!("{name} is no longer kept on the keyboard."),
                JobKind::Read => return None,
            };
            Some(Strip {
                tone: if text.contains("not kept") {
                    Tone::Warn
                } else {
                    Tone::Ok
                },
                text,
                detail: None,
                progress: None,
                button: None,
            })
        }
        Err(error) => {
            let cause = problem(error, s.status.as_ref());
            let (text, detail, button) = cause.words(&outcome.job);
            Some(Strip {
                tone: match cause {
                    Problem::Cancelled | Problem::Paused | Problem::NotRemembered => Tone::Warn,
                    Problem::RightAsleep | Problem::Busy | Problem::NoKeyboard => Tone::Warn,
                    _ => Tone::Error,
                },
                text,
                detail,
                progress: None,
                button,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::art_service::{Choice, Outcome, Retry};
    use digitalis::protocol::{ERRORS, RIGHT_STATES};
    use std::time::{Duration, Instant};

    /// Relay words that mean nothing to a user (the plain ones, like "too big", may stay).
    const TECHNICAL: [&str; 8] = [
        "bad CRC",
        "bad blob",
        "playing pushed set",
        "save failed",
        "CRC",
        "data out of range",
        "no relay",
        "Bluetooth write failed",
    ];

    fn job(kind: JobKind) -> Job {
        Job {
            kind,
            name: "Uplink".into(),
            quiet: false,
            again: Some(Retry::Choose(Choice::BuiltIn(1))),
        }
    }

    fn st(state: u8, set: i8) -> ArtStatus {
        ArtStatus {
            link: 2,
            right_state: state,
            right_set: set,
            right_set_count: 17,
            flash_slots: 8,
            flash_used: 0b11,
            ..Default::default()
        }
    }

    #[test]
    fn showing_reads_the_keyboard_report() {
        assert_eq!(showing(&st(0, 3)), Showing::BuiltIn(3));
        assert_eq!(showing(&st(2, 3)), Showing::Sent);
        assert_eq!(showing(&st(2, 18)), Showing::Kept(1));
        assert_eq!(showing(&st(0, 17)), Showing::Kept(0));
        assert_eq!(
            showing(&st(7, 3)),
            Showing::Sent,
            "saved, still the sent piece"
        );
        assert_eq!(showing(&st(3, 3)), Showing::Other(3));
        let old = ArtStatus {
            flash_slots: 0,
            ..st(2, 18)
        };
        assert_eq!(showing(&old), Showing::Sent, "firmware without places");
    }

    /// The restart line never promises what doesn't happen: only kept pieces stay on the
    /// keyboard, and Foxglove's put-back needs Foxglove running.
    #[test]
    fn the_restart_line_tells_the_truth() {
        for remembered in [true, false] {
            let kept = after_restart(Showing::Kept(0), remembered).unwrap();
            assert!(kept.starts_with("Kept on the keyboard"));
            let sent = after_restart(Showing::Sent, remembered).unwrap();
            assert!(sent.starts_with("Not kept"));
            let built_in = after_restart(Showing::BuiltIn(2), remembered).unwrap();
            assert!(built_in.starts_with("Comes with the keyboard"));
            for line in [kept, sent, built_in] {
                assert!(
                    !line.contains("Comes back"),
                    "the old false promise: {line}"
                );
                assert_eq!(
                    line.contains("while it runs on this PC"),
                    remembered,
                    "Foxglove's put-back is only named when it will happen: {line}"
                );
            }
        }
        for state in 3..=8 {
            assert_eq!(after_restart(Showing::Other(state), true), None);
        }
    }

    #[test]
    fn every_right_state_has_plain_words() {
        for state in 0..RIGHT_STATES.len() {
            let words = right_state_words(state as u8);
            assert!(words.ends_with('.') || words.ends_with('…'), "{words}");
            for raw in TECHNICAL {
                assert!(!words.contains(raw), "{raw} leaks into {words}");
            }
        }
        assert_eq!(right_state_words(200), "The right screen is busy.");
    }

    #[test]
    fn badges_use_the_new_words() {
        let words: Vec<String> = [
            Badge::OnKeyboard,
            Badge::ComesWithKeyboard,
            Badge::UntilRestart,
            Badge::ThisPc,
            Badge::Sending(60),
            Badge::TooBig,
        ]
        .into_iter()
        .map(Badge::text)
        .collect();
        assert_eq!(
            words,
            [
                "ON KEYBOARD",
                "COMES WITH KEYBOARD",
                "SHOWING UNTIL RESTART",
                "THIS PC",
                "SENDING 60%",
                "TOO BIG"
            ]
        );
    }

    /// Each relay error code and right state, one by one, to a cause.
    #[test]
    fn relay_errors_map_to_causes() {
        let with_error = |code: u8| ArtStatus {
            error: code,
            ..st(0, 1)
        };
        let expected = [
            Problem::Other, // "none": the string decides
            Problem::RightAsleep,
            Problem::TooBig,
            Problem::Busy,
            Problem::Damaged,
            Problem::Damaged,
            Problem::LinkLost,
            Problem::Damaged,
        ];
        assert_eq!(expected.len(), ERRORS.len());
        for (code, want) in expected.into_iter().enumerate() {
            let got = problem("push failed: left: failed", Some(&with_error(code as u8)));
            let want = if code == 0 { Problem::LinkLost } else { want };
            assert_eq!(got, want, "relay error {code} ({})", ERRORS[code]);
        }
        let right = |state: u8| problem("push failed: x", Some(&st(state, 1)));
        assert_eq!(right(3), Problem::Damaged, "bad CRC");
        assert_eq!(right(4), Problem::Damaged, "bad blob");
        assert_eq!(right(5), Problem::TooBig);
        assert_eq!(right(8), Problem::KeepFailed);
    }

    /// Each error string the art service and digitalis make, one by one.
    #[test]
    fn service_errors_map_to_causes() {
        let cases = [
            ("push cancelled", Problem::Cancelled),
            ("cancelled", Problem::Cancelled),
            (crate::foxglove::pause::REFUSED, Problem::Paused),
            (
                "Waiting for a keyboard with the host link",
                Problem::NoKeyboard,
            ),
            ("No art status yet", Problem::NoKeyboard),
            ("Art worker panicked; restarting", Problem::Crashed),
            ("saving pinned art: access denied", Problem::NotRemembered),
            (r"C:\art\x.bin is not an art blob", Problem::NotArt),
            (
                r"reading C:\art\x.bin: The system cannot find the file",
                Problem::FileMissing,
            ),
            (
                "save failed on the right half: left: done",
                Problem::KeepFailed,
            ),
            ("blob is 70000 bytes; the most is 65535", Problem::TooBig),
            ("the right half is not connected", Problem::RightAsleep),
            ("the relay is busy", Problem::Busy),
            ("reading flash slot 3: slot read error 2", Problem::ReadKept),
            (
                "select did not finish in 10 s: left: pushing",
                Problem::LinkLost,
            ),
            ("request timed out", Problem::LinkLost),
            (
                "the left half refused the blob: left: idle",
                Problem::LinkLost,
            ),
            ("device disconnected", Problem::LinkLost),
            ("something new", Problem::Other),
        ];
        for (error, want) in cases {
            assert_eq!(problem(error, None), want, "{error}");
        }
        let asleep = ArtStatus {
            link: 0,
            ..Default::default()
        };
        assert_eq!(
            problem("something new", Some(&asleep)),
            Problem::RightAsleep
        );
    }

    /// No cause shows a raw state name, a `{:?}`, or the old byte counter.
    #[test]
    fn problem_words_are_plain() {
        let all = [
            Problem::Cancelled,
            Problem::Paused,
            Problem::NoKeyboard,
            Problem::RightAsleep,
            Problem::Busy,
            Problem::TooBig,
            Problem::Damaged,
            Problem::LinkLost,
            Problem::KeepFailed,
            Problem::NotArt,
            Problem::FileMissing,
            Problem::NotRemembered,
            Problem::ReadKept,
            Problem::Crashed,
            Problem::Other,
        ];
        for kind in [JobKind::Show, JobKind::Keep, JobKind::Remove, JobKind::Read] {
            for p in all {
                let (text, detail, _) = p.words(&job(kind));
                let all_text = format!("{text} {}", detail.unwrap_or_default());
                for bad in TECHNICAL
                    .iter()
                    .chain(&["slot", "NVA1", "D6", "manifest", "{", "bytes"])
                {
                    assert!(!all_text.contains(bad), "{p:?}: {bad} in {all_text}");
                }
            }
        }
        let (text, _, button) = Problem::RightAsleep.words(&job(JobKind::Show));
        assert_eq!(text, "The right half is asleep. Press any key on it, then:");
        assert_eq!(button, Some(Button::TryAgain));
    }

    #[test]
    fn the_strip_follows_the_job() {
        let mut s = Snapshot {
            busy: true,
            job: Some(job(JobKind::Show)),
            push_progress: Some((1200, 2000)),
            ..Default::default()
        };
        let sending = strip(&s).unwrap();
        assert_eq!(sending.text, "Sending Uplink to the right screen…");
        assert_eq!(sending.progress, Some(0.6));
        assert_eq!(sending.button, Some(Button::Cancel));
        s.push_progress = None;
        assert_eq!(strip(&s).unwrap().text, "Showing Uplink…");
        assert_eq!(
            strip(&s).unwrap().button,
            None,
            "a built-in can't be cancelled"
        );

        s.busy = false;
        s.status = Some(st(2, 3));
        s.outcome = Some(Outcome {
            job: job(JobKind::Show),
            result: Ok(()),
            at: Instant::now(),
        });
        let done = strip(&s).unwrap();
        assert_eq!(
            done.text,
            "Uplink is showing until the keyboard restarts (not kept)."
        );
        assert_eq!(done.tone, Tone::Warn);
        s.status = Some(st(0, 3));
        assert_eq!(strip(&s).unwrap().text, "Uplink is on the right screen.");
        if let Some(old) = Instant::now().checked_sub(DONE_FOR + Duration::from_secs(1)) {
            s.outcome.as_mut().unwrap().at = old;
            assert_eq!(strip(&s), None, "a success line fades");
        }

        s.outcome = Some(Outcome {
            job: job(JobKind::Show),
            result: Err("the right half is not connected".into()),
            at: Instant::now(),
        });
        let asleep = strip(&s).unwrap();
        assert_eq!(asleep.button, Some(Button::TryAgain));
        assert_eq!(asleep.tone, Tone::Warn);

        // Keep: the send, then the save, then the line that says both.
        s.busy = true;
        s.job = Some(job(JobKind::Keep));
        s.push_progress = Some((500, 1000));
        assert_eq!(
            strip(&s).unwrap().text,
            "Sending Uplink to the right screen…"
        );
        s.push_progress = None;
        let saving = strip(&s).unwrap();
        assert_eq!(saving.text, "Keeping Uplink on the keyboard…");
        assert_eq!(
            saving.detail.as_deref(),
            Some("Saving: don't unplug the keyboard.")
        );
        assert_eq!(saving.button, None, "a save can't be stopped halfway");
        s.busy = false;
        s.status = Some(st(0, 19));
        s.outcome = Some(Outcome {
            job: job(JobKind::Keep),
            result: Ok(()),
            at: Instant::now(),
        });
        let kept = strip(&s).unwrap();
        assert_eq!(
            kept.text,
            "Uplink is on the right screen and kept on the keyboard."
        );
        assert_eq!(kept.tone, Tone::Ok);

        let mut quiet = job(JobKind::Show);
        quiet.quiet = true;
        s.outcome = Some(Outcome {
            job: quiet,
            result: Ok(()),
            at: Instant::now(),
        });
        assert_eq!(
            strip(&s),
            None,
            "rotation and put-backs don't announce themselves"
        );
    }
}
