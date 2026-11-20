// SPDX-License-Identifier: MIT
//! H5.2: the user's **Start with Windows** switch for the hub, on top of [`crate::autostart`].
//!
//! Rules (the user's, 11/18):
//! - It is OFF by default and nothing turns it on: not the install, not the first run, not a
//!   startup pass. [`crate::autostart::ensure`] is deliberately not called anywhere.
//! - Turning it on needs the user's click on **Turn on** in the dialog this module opens
//!   ([`Dialog`]). The only way to the on-write is [`UserOk`], which only
//!   [`Dialog::turn_on`] makes, so no other code path can write an enabled state.
//! - Writes happen only from an installed hub (its exe sits in the folder `HKCU\Software\Hideout\
//!   Installer` names) and never in a `HIDEOUT_CONFIG_DIR` / `HIDEOUT_HOME` run. Until the Hideout
//!   MSI exists (H5.6) no hub is "installed", so the switch is unavailable and writes nothing.
//! - Off-switch: **Turn off** (Windows' own disabled state). Rollback: **Remove shortcut**
//!   deletes `Hideout.lnk` and its approval value, exact target only; `hideout.exe
//!   --autostart-remove` does the same without a window.
//! - `Foxglove.lnk` and Foxglove's own autostart are never read or written.
//!
//! The Startup folder and registry are the [`StartupFolder`] / [`Approval`] traits, so every test
//! here uses fakes.

use std::path::PathBuf;

use crate::autostart::{self, Approval, Context, Outcome, StartupFolder, State};

// ---- The words (user-visible; UX review needed) --------------------------------------------

pub const HEADING: &str = "Start with Windows";
pub const STATUS_OFF: &str = "Hideout does not start when you sign in to Windows.";
pub const STATUS_ON: &str = "Hideout starts in the tray when you sign in to Windows.";
/// Shown instead of [`STATUS_ON`] when at least one app runs in Hideout.
pub const STATUS_ON_APPS: &str =
    "Hideout starts in the tray when you sign in, and starts your apps: Foxglove.";
pub const STATUS_OFF_IN_WINDOWS: &str = "Off. Hideout does not start when you sign in. You can \
turn it on here or in Windows Settings > Apps > Startup.";
pub const STATUS_FOREIGN: &str = "Another Hideout shortcut is set to start when you sign in.";
pub const STATUS_NOT_INSTALLED: &str =
    "Available once Hideout is installed. This copy does not start with Windows.";
pub const STATUS_TEST_RUN: &str = "Not available in a test run. Nothing is written.";
pub const TURN_ON: &str = "Turn on\u{2026}";
pub const TURN_OFF: &str = "Turn off";
pub const REMOVE: &str = "Remove shortcut";
pub const DIALOG_TITLE: &str = "Start Hideout when you sign in?";
pub const DIALOG_BODY: &str = "Hideout adds a shortcut to your Startup folder. It then starts in \
the tray each time you sign in to Windows. You can turn it off here, or in Windows Settings under \
Apps, then Startup.";
/// Shown in the dialog only when an app runs in Hideout.
pub const DIALOG_APPS: &str = "Hideout then starts Foxglove too. Foxglove's own startup entry \
stays as a backup: if Hideout does not start, Foxglove starts on its own.";
pub const DIALOG_REPLACES: &str =
    "The other copy of Hideout will no longer start when you sign in.";
pub const DIALOG_CONFIRM: &str = "Turn on";
pub const DIALOG_CANCEL: &str = "Not now";
pub const DONE_ON: &str = "Turned on. Hideout starts when you sign in.";
pub const DONE_OFF: &str = "Turned off. Hideout does not start when you sign in.";
pub const DONE_REMOVED: &str = "Removed the startup shortcut.";
pub const FAILED: &str =
    "That did not work. You can check Windows Settings under Apps, then Startup.";

// ---- The log lines (sign-in hand-over slice 6; not user-visible) ----------------------------

pub const LOG_TURNED_ON: &str = "Start with Windows turned on by the user (dialog)";
pub const LOG_TURNED_OFF: &str = "Start with Windows turned off by the user";
pub const LOG_REMOVED: &str = "Start with Windows: shortcut removed by the user";

/// The log line for a click that really changed something; `None` for everything else (no change,
/// a test run, a failure, which logs its own warning).
pub fn click_log(c: Click, outcome: &Result<Outcome, String>) -> Option<&'static str> {
    match (c, outcome) {
        (Click::Confirm, Ok(Outcome::Changed)) => Some(LOG_TURNED_ON),
        (Click::TurnOff, Ok(Outcome::Changed)) => Some(LOG_TURNED_OFF),
        (Click::Remove, Ok(Outcome::Changed)) => Some(LOG_REMOVED),
        _ => None,
    }
}

/// The first line of a `--tray` (sign-in) start: what Start with Windows says now.
pub fn sign_in_line(state: State, has_link: bool) -> String {
    let what = match state {
        State::On => "on (Hideout.lnk starts this exe)",
        State::Foreign => "another shortcut",
        State::Off if has_link => "off in Windows",
        _ => "off",
    };
    format!("sign-in start; Start with Windows: {what}")
}

// ---- Gate ----------------------------------------------------------------------------------

/// Whether the switch may write at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// This exe is not in the folder the Hideout installer recorded (dev build, no installer).
    NotInstalled,
    /// A `HIDEOUT_CONFIG_DIR` / `HIDEOUT_HOME` run.
    TestRun,
}

/// `installed`: the folder `HKCU\Software\Hideout\Installer` names, if any.
pub fn availability(ctx: &Context, installed: Option<&std::path::Path>) -> Availability {
    if ctx.overridden {
        return Availability::TestRun;
    }
    let inside = match (installed, ctx.exe.parent()) {
        (Some(dir), Some(exe_dir)) => {
            !dir.to_string_lossy().starts_with(r"\\") && autostart::same_path(dir, exe_dir)
        }
        _ => false,
    };
    if inside {
        Availability::Ready
    } else {
        Availability::NotInstalled
    }
}

// ---- The dialog and the user's OK ----------------------------------------------------------

/// Proof the user clicked **Turn on** in the dialog. Both fields are private to this module:
/// only [`Switch::ask`] makes a [`Dialog`], and only a `Click::Confirm` on it makes this.
pub struct UserOk(());

/// The open dialog. Dropping it is Cancel.
#[derive(Debug)]
pub struct Dialog {
    replaces_other: bool,
}

impl Dialog {
    pub fn replaces_other(&self) -> bool {
        self.replaces_other
    }

    /// The user's click on **Turn on**.
    fn turn_on(self) -> UserOk {
        UserOk(())
    }
}

// ---- What the page draws -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Row {
    pub status: String,
    pub turn_on: bool,
    pub turn_off: bool,
    pub remove: bool,
    pub message: Option<String>,
    /// At least one app runs in Hideout: the status and the dialog say that apps start too.
    pub apps: bool,
    /// `Some(replaces)` while the dialog is open.
    pub dialog: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Ask,
    Confirm,
    Cancel,
    TurnOff,
    Remove,
}

/// What the Settings page holds (object safe, so the page needs no type parameters).
pub trait StartupControl {
    fn row(&mut self) -> Row;
    fn click(&mut self, c: Click);
}

pub struct Switch<F: StartupFolder, A: Approval> {
    ctx: Context,
    installed: Option<PathBuf>,
    folder: F,
    approval: A,
    dialog: Option<Dialog>,
    message: Option<String>,
    /// Set by the hub while at least one app runs in Hideout (copy only; never a write gate).
    apps: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// (state, a shortcut exists, when read): the page draws many frames, the shell and COM
    /// are read at most every two seconds and after every click.
    snap: Option<(State, bool, std::time::Instant)>,
}

const SNAP_AGE: std::time::Duration = std::time::Duration::from_secs(2);

impl<F: StartupFolder, A: Approval> Switch<F, A> {
    pub fn new(ctx: Context, installed: Option<PathBuf>, folder: F, approval: A) -> Self {
        Self {
            ctx,
            installed,
            folder,
            approval,
            dialog: None,
            message: None,
            apps: Default::default(),
            snap: None,
        }
    }

    /// Gives the switch the flag the hub keeps while an app runs in Hideout.
    pub fn with_apps(mut self, apps: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.apps = apps;
        self
    }

    fn apps_run(&self) -> bool {
        self.apps.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn snapshot(&mut self) -> (State, bool) {
        if let Some((s, l, at)) = self.snap {
            if at.elapsed() < SNAP_AGE {
                return (s, l);
            }
        }
        let s = self.state();
        let l = self.folder.read().is_some();
        self.snap = Some((s, l, std::time::Instant::now()));
        (s, l)
    }

    pub fn availability(&self) -> Availability {
        availability(&self.ctx, self.installed.as_deref())
    }

    pub fn state(&self) -> State {
        autostart::state(&self.ctx, &self.folder, &self.approval)
    }

    /// Opens the dialog; `None` unless the switch may write.
    pub fn ask(&mut self) -> Option<&Dialog> {
        if self.availability() != Availability::Ready {
            return None;
        }
        self.message = None;
        self.dialog = Some(Dialog {
            replaces_other: self.state() == State::Foreign,
        });
        self.dialog.as_ref()
    }

    /// The only on-write. Needs the user's [`UserOk`] and an available switch.
    pub fn turn_on(&mut self, _ok: UserOk) -> Result<Outcome, String> {
        if self.availability() != Availability::Ready {
            return Ok(Outcome::SkippedLabRun);
        }
        autostart::set_enabled(&self.ctx, true, &mut self.folder, &mut self.approval)
    }

    /// The off-switch: Windows' disabled state, shortcut kept (like Windows' own toggle).
    pub fn turn_off(&mut self) -> Result<Outcome, String> {
        if self.ctx.overridden || self.state() != State::On {
            return Ok(Outcome::Unchanged);
        }
        autostart::set_enabled(&self.ctx, false, &mut self.folder, &mut self.approval)
    }

    /// The log line for a `--tray` start (reads the shell once; writes nothing).
    pub fn sign_in_line(&mut self) -> String {
        let (state, has_link) = self.snapshot();
        sign_in_line(state, has_link)
    }

    /// The rollback: removes `Hideout.lnk` and its approval value, only when it starts this exe.
    pub fn remove_shortcut(&mut self) -> Result<Outcome, String> {
        autostart::remove(&self.ctx, &mut self.folder, &mut self.approval)
    }
}

impl<F: StartupFolder, A: Approval> StartupControl for Switch<F, A> {
    fn row(&mut self) -> Row {
        let avail = self.availability();
        let (state, has_link) = self.snapshot();
        let apps = self.apps_run();
        let status = match (avail, state) {
            (Availability::TestRun, _) => STATUS_TEST_RUN,
            (Availability::NotInstalled, _) => STATUS_NOT_INSTALLED,
            (_, State::On) if apps => STATUS_ON_APPS,
            (_, State::On) => STATUS_ON,
            (_, State::Foreign) => STATUS_FOREIGN,
            (_, State::Off) if has_link => STATUS_OFF_IN_WINDOWS,
            (_, _) => STATUS_OFF,
        };
        let ready = avail == Availability::Ready;
        Row {
            status: status.to_string(),
            turn_on: ready && state != State::On && self.dialog.is_none(),
            turn_off: ready && state == State::On,
            remove: avail != Availability::TestRun
                && self.dialog.is_none()
                && matches!(state, State::On | State::Off)
                && has_link,
            message: self.message.clone(),
            apps,
            dialog: self.dialog.as_ref().map(Dialog::replaces_other),
        }
    }

    fn click(&mut self, c: Click) {
        self.snap = None;
        let done = |r: Result<Outcome, String>, ok: &str| match r {
            Ok(_) => ok.to_string(),
            Err(e) => {
                log::warn!("start with Windows: {e}");
                FAILED.to_string()
            }
        };
        match c {
            Click::Ask => {
                self.ask();
            }
            Click::Cancel => self.dialog = None,
            Click::Confirm => {
                if let Some(d) = self.dialog.take() {
                    let r = self.turn_on(d.turn_on());
                    if let Some(l) = click_log(c, &r) {
                        log::info!("{l}");
                    }
                    self.message = Some(done(r, DONE_ON));
                }
            }
            Click::TurnOff => {
                let r = self.turn_off();
                if let Some(l) = click_log(c, &r) {
                    log::info!("{l}");
                }
                self.message = Some(done(r, DONE_OFF));
            }
            Click::Remove => {
                let r = self.remove_shortcut();
                if let Some(l) = click_log(c, &r) {
                    log::info!("{l}");
                }
                self.message = Some(done(r, DONE_REMOVED));
            }
        }
    }
}

// ---- Windows' StartupApproved bytes --------------------------------------------------------

/// A StartupApproved value as Windows writes it: 12 bytes, the first 02 (enabled) or 03
/// (disabled, then the time as a FILETIME).
pub fn approved_bytes(enabled: bool, filetime: u64) -> [u8; 12] {
    let mut b = [0u8; 12];
    if enabled {
        b[0] = 2;
    } else {
        b[0] = 3;
        b[4..].copy_from_slice(&filetime.to_le_bytes());
    }
    b
}

/// Windows writes 02 or 06 for on and 01, 03 or 07 for off.
pub fn approved_enabled(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|b| b & 1 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autostart::Link;
    use std::path::Path;

    #[derive(Default)]
    struct Folder {
        link: Option<Link>,
        writes: usize,
    }
    impl StartupFolder for Folder {
        fn read(&self) -> Option<Link> {
            self.link.clone()
        }
        fn write(&mut self, l: &Link) -> Result<(), String> {
            self.link = Some(l.clone());
            self.writes += 1;
            Ok(())
        }
        fn delete(&mut self) -> Result<(), String> {
            self.link = None;
            self.writes += 1;
            Ok(())
        }
        fn exists(&self, _: &Path) -> bool {
            false
        }
    }
    #[derive(Default)]
    struct Reg {
        v: Option<bool>,
        writes: usize,
    }
    impl Approval for Reg {
        fn read(&self) -> Option<bool> {
            self.v
        }
        fn write(&mut self, e: bool) -> Result<(), String> {
            self.v = Some(e);
            self.writes += 1;
            Ok(())
        }
        fn delete(&mut self) -> Result<(), String> {
            self.v = None;
            self.writes += 1;
            Ok(())
        }
    }

    const DIR: &str = r"C:\Users\u\AppData\Local\Programs\Hideout";
    const EXE: &str = r"C:\Users\u\AppData\Local\Programs\Hideout\hideout.exe";

    fn ctx(overridden: bool) -> Context {
        Context {
            exe: PathBuf::from(EXE),
            overridden,
        }
    }
    fn sw(installed: bool, overridden: bool) -> Switch<Folder, Reg> {
        Switch::new(
            ctx(overridden),
            installed.then(|| PathBuf::from(DIR)),
            Folder::default(),
            Reg::default(),
        )
    }
    fn writes(s: &Switch<Folder, Reg>) -> usize {
        s.folder.writes + s.approval.writes
    }

    #[test]
    fn off_by_default_and_a_fresh_switch_writes_nothing() {
        let mut s = sw(true, false);
        let row = s.row();
        assert_eq!(row.status, STATUS_OFF);
        assert!(row.turn_on && !row.turn_off && !row.remove && row.dialog.is_none());
        assert_eq!(s.state(), State::Missing);
        assert_eq!(writes(&s), 0, "building and reading the row write nothing");
    }

    #[test]
    fn turning_on_needs_the_dialog_and_its_turn_on() {
        let mut s = sw(true, false);
        s.click(Click::Ask);
        assert_eq!(writes(&s), 0, "opening the dialog writes nothing");
        assert_eq!(s.row().dialog, Some(false));
        s.click(Click::Cancel);
        assert_eq!((writes(&s), s.row().dialog), (0, None));
        // Confirm with no dialog open does nothing.
        s.click(Click::Confirm);
        assert_eq!(writes(&s), 0);
        s.click(Click::Ask);
        s.click(Click::Confirm);
        assert_eq!(s.folder.link.as_ref().unwrap().args, "--tray");
        assert_eq!(s.approval.v, Some(true));
        assert_eq!(s.state(), State::On);
        assert_eq!(s.row().message.as_deref(), Some(DONE_ON));
        assert_eq!(s.row().status, STATUS_ON);
    }

    #[test]
    fn not_installed_and_test_runs_cannot_ask_or_write() {
        for (installed, overridden, want) in [
            (false, false, Availability::NotInstalled),
            (true, true, Availability::TestRun),
            (false, true, Availability::TestRun),
        ] {
            let mut s = sw(installed, overridden);
            assert_eq!(s.availability(), want);
            s.click(Click::Ask);
            s.click(Click::Confirm);
            s.click(Click::TurnOff);
            s.click(Click::Remove);
            assert_eq!(writes(&s), 0, "{want:?}");
            let row = s.row();
            assert!(!row.turn_on && !row.turn_off && row.dialog.is_none());
        }
        // A UserOk cannot get around the gate either.
        let mut s = sw(false, false);
        let ok = Dialog {
            replaces_other: false,
        }
        .turn_on();
        assert_eq!(s.turn_on(ok), Ok(Outcome::SkippedLabRun));
        assert_eq!(writes(&s), 0);
    }

    #[test]
    fn installed_folder_must_hold_this_exe() {
        let c = ctx(false);
        assert_eq!(availability(&c, Some(Path::new(DIR))), Availability::Ready);
        assert_eq!(
            availability(&c, Some(Path::new(&DIR.to_uppercase()))),
            Availability::Ready
        );
        assert_eq!(
            availability(&c, Some(Path::new(r"D:\other"))),
            Availability::NotInstalled
        );
        assert_eq!(
            availability(&c, Some(Path::new(r"\\host\share\Hideout"))),
            Availability::NotInstalled
        );
        assert_eq!(availability(&c, None), Availability::NotInstalled);
    }

    #[test]
    fn off_switch_keeps_the_shortcut_and_survives_a_restart() {
        let mut s = sw(true, false);
        s.click(Click::Ask);
        s.click(Click::Confirm);
        s.click(Click::TurnOff);
        assert_eq!(s.state(), State::Off);
        assert_eq!(s.row().status, STATUS_OFF_IN_WINDOWS);
        assert!(s.row().remove && s.row().turn_on && !s.row().turn_off);
        // Nothing in the wiring is an automatic turn-on: a fresh Switch over the same files
        // (the next start) reads Off and writes nothing.
        let Switch {
            folder, approval, ..
        } = s;
        let mut again = Switch::new(ctx(false), Some(PathBuf::from(DIR)), folder, approval);
        let before = writes(&again);
        assert_eq!(again.row().status, STATUS_OFF_IN_WINDOWS);
        assert_eq!(writes(&again), before);
    }

    #[test]
    fn remove_shortcut_is_the_rollback() {
        let mut s = sw(true, false);
        s.click(Click::Ask);
        s.click(Click::Confirm);
        s.click(Click::Remove);
        assert!(s.folder.link.is_none() && s.approval.v.is_none());
        assert_eq!(s.state(), State::Missing);
        assert_eq!(s.row().message.as_deref(), Some(DONE_REMOVED));
        // Twice is a no-op.
        let n = writes(&s);
        s.click(Click::Remove);
        assert_eq!(writes(&s), n);
    }

    #[test]
    fn a_foreign_shortcut_is_named_in_the_dialog_and_never_removed() {
        let mut s = sw(true, false);
        s.folder.link = Some(Link {
            target: PathBuf::from(r"D:\other\hideout.exe"),
            args: "--tray".into(),
        });
        s.approval.v = Some(true);
        assert_eq!(s.row().status, STATUS_FOREIGN);
        assert!(!s.row().remove, "not this exe's shortcut");
        s.click(Click::Remove);
        assert_eq!(writes(&s), 0);
        s.click(Click::Ask);
        assert_eq!(s.row().dialog, Some(true));
    }

    #[test]
    fn foxglove_is_never_touched() {
        // The fakes model Hideout.lnk only: the traits have no way to name another file. The
        // names the module writes are fixed.
        assert_eq!(autostart::LINK_NAME, "Hideout.lnk");
        assert!(!autostart::LINK_NAME.to_lowercase().contains("foxglove"));
    }

    /// Guard: nothing may run the startup pass or the on-write outside the dialog path, and
    /// nothing here names Foxglove's shortcut.
    #[test]
    fn nothing_else_turns_it_on() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut hits = Vec::new();
        for c in std::fs::read_dir(&crates).unwrap().flatten() {
            let Ok(rd) = std::fs::read_dir(c.path().join("src")) else {
                continue;
            };
            for f in rd.flatten() {
                let p = f.path();
                if p.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                for (i, l) in text.lines().enumerate() {
                    let code = l.trim_start();
                    if code.starts_with("//") {
                        continue;
                    }
                    let bad_call = code.contains("autostart::ensure(")
                        || code.contains("autostart::set_enabled(");
                    // The one allowed on-write is Switch::turn_on / turn_off here; only the
                    // switch and the page's drawing name the dialog's OK.
                    // (Other dialogs have their own Click and Dialog types: only files that
                    // use this module count.)
                    let ok_path = text.contains("startup_switch")
                        && (code.contains("Click::Confirm")
                            || code.contains("Dialog {")
                            || code.contains("UserOk("));
                    let allowed_ok = ["startup_switch.rs", "settings_view.rs", "shell_tests.rs"]
                        .contains(&name.as_str());
                    if (bad_call && name != "startup_switch.rs") || (ok_path && !allowed_ok) {
                        hits.push(format!("{name}:{}", i + 1));
                    }
                }
            }
        }
        assert!(hits.is_empty(), "{hits:?}");
        let me = include_str!("startup_switch.rs");
        let code: String = me
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .take_while(|l| !l.contains("mod tests"))
            .collect::<Vec<_>>()
            .join(" ");
        // The copy may say "Foxglove"; the code never names its shortcut or startup entry.
        assert!(!code.to_lowercase().contains("foxglove.lnk"));
        assert!(!code.contains("autostart::ensure("));
    }

    /// Every user-visible string, exact (the PR body lists the same).
    #[test]
    fn the_copy_is_exact() {
        assert_eq!(
            STATUS_OFF,
            "Hideout does not start when you sign in to Windows."
        );
        assert_eq!(
            STATUS_ON,
            "Hideout starts in the tray when you sign in to Windows."
        );
        assert_eq!(
            STATUS_ON_APPS,
            "Hideout starts in the tray when you sign in, and starts your apps: Foxglove."
        );
        assert_eq!(
            STATUS_OFF_IN_WINDOWS,
            "Off. Hideout does not start when you sign in. You can turn it on here or in Windows Settings > Apps > Startup."
        );
        assert_eq!(
            STATUS_FOREIGN,
            "Another Hideout shortcut is set to start when you sign in."
        );
        assert_eq!(DIALOG_CANCEL, "Not now");
        assert_eq!(
            DIALOG_BODY,
            "Hideout adds a shortcut to your Startup folder. It then starts in the tray each time you sign in to Windows. You can turn it off here, or in Windows Settings under Apps, then Startup."
        );
        assert_eq!(
            DIALOG_APPS,
            "Hideout then starts Foxglove too. Foxglove's own startup entry stays as a backup: if Hideout does not start, Foxglove starts on its own."
        );
    }

    #[test]
    fn the_apps_line_shows_only_when_an_app_runs_in_hideout() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut s = sw(true, false).with_apps(flag.clone());
        s.click(Click::Ask);
        s.click(Click::Confirm);
        assert_eq!(s.row().status, STATUS_ON);
        assert!(!s.row().apps);
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        s.snap = None;
        assert_eq!(s.row().status, STATUS_ON_APPS);
        assert!(s.row().apps);
        // Off states never claim apps start.
        s.click(Click::TurnOff);
        assert_eq!(s.row().status, STATUS_OFF_IN_WINDOWS);
    }

    #[test]
    fn the_log_lines_are_exact_and_only_for_real_changes() {
        assert_eq!(
            click_log(Click::Confirm, &Ok(Outcome::Changed)),
            Some("Start with Windows turned on by the user (dialog)")
        );
        assert_eq!(
            click_log(Click::TurnOff, &Ok(Outcome::Changed)),
            Some("Start with Windows turned off by the user")
        );
        assert_eq!(
            click_log(Click::Remove, &Ok(Outcome::Changed)),
            Some("Start with Windows: shortcut removed by the user")
        );
        for o in [
            Ok(Outcome::Unchanged),
            Ok(Outcome::SkippedLabRun),
            Err("x".to_string()),
        ] {
            for c in [Click::Confirm, Click::TurnOff, Click::Remove] {
                assert_eq!(click_log(c, &o), None);
            }
        }
        assert_eq!(click_log(Click::Ask, &Ok(Outcome::Changed)), None);
        assert_eq!(click_log(Click::Cancel, &Ok(Outcome::Changed)), None);
    }

    #[test]
    fn the_sign_in_line_names_the_switch_state() {
        assert_eq!(
            sign_in_line(State::On, true),
            "sign-in start; Start with Windows: on (Hideout.lnk starts this exe)"
        );
        assert_eq!(
            sign_in_line(State::Off, true),
            "sign-in start; Start with Windows: off in Windows"
        );
        assert_eq!(
            sign_in_line(State::Off, false),
            "sign-in start; Start with Windows: off"
        );
        assert_eq!(
            sign_in_line(State::Foreign, true),
            "sign-in start; Start with Windows: another shortcut"
        );
        let mut s = sw(true, false);
        assert_eq!(s.sign_in_line(), "sign-in start; Start with Windows: off");
    }

    #[test]
    fn approved_bytes_match_windows() {
        assert_eq!(approved_bytes(true, 5)[0], 2);
        let off = approved_bytes(false, 0x0102_0304_0506_0708);
        assert_eq!((off[0], off[4], off[11]), (3, 8, 1));
        assert!(approved_enabled(&[2, 0, 0]) && approved_enabled(&[6]));
        for b in [1u8, 3, 7] {
            assert!(!approved_enabled(&[b]));
        }
        assert!(!approved_enabled(&[]));
    }
}
