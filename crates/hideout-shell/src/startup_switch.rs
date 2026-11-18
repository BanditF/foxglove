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
pub const STATUS_OFF_IN_WINDOWS: &str = "Hideout is turned off in Windows' startup list, so it \
does not start when you sign in.";
pub const STATUS_FOREIGN: &str =
    "Another copy of Hideout is set to start when you sign in to Windows.";
pub const STATUS_NOT_INSTALLED: &str =
    "Available once Hideout is installed. This copy does not start with Windows.";
pub const STATUS_TEST_RUN: &str = "Not available in a test run. Nothing is written.";
pub const TURN_ON: &str = "Turn on\u{2026}";
pub const TURN_OFF: &str = "Turn off";
pub const REMOVE: &str = "Remove shortcut";
pub const DIALOG_TITLE: &str = "Start Hideout when you sign in?";
pub const DIALOG_BODY: &str = "Hideout adds a shortcut to your Startup folder. It then opens in \
the tray each time you sign in to Windows. You can turn it off here, or in Windows Settings under \
Apps, then Startup.";
pub const DIALOG_REPLACES: &str = "This replaces the other copy's startup shortcut.";
pub const DIALOG_CONFIRM: &str = "Turn on";
pub const DIALOG_CANCEL: &str = "Cancel";
pub const DONE_ON: &str = "Turned on. Hideout starts when you sign in.";
pub const DONE_OFF: &str = "Turned off. Hideout does not start when you sign in.";
pub const DONE_REMOVED: &str = "Removed the startup shortcut.";
pub const FAILED: &str = "That did not work. Nothing was changed that you need to undo.";

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

/// Proof the user clicked **Turn on** in the dialog. Cannot be made anywhere else.
pub struct UserOk(());

/// The open dialog. Dropping it is Cancel.
#[derive(Debug)]
pub struct Dialog {
    pub replaces_other: bool,
}

impl Dialog {
    /// The user's click on **Turn on**.
    pub fn turn_on(self) -> UserOk {
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
            snap: None,
        }
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

    /// The rollback: removes `Hideout.lnk` and its approval value, only when it starts this exe.
    pub fn remove_shortcut(&mut self) -> Result<Outcome, String> {
        autostart::remove(&self.ctx, &mut self.folder, &mut self.approval)
    }
}

impl<F: StartupFolder, A: Approval> StartupControl for Switch<F, A> {
    fn row(&mut self) -> Row {
        let avail = self.availability();
        let (state, has_link) = self.snapshot();
        let status = match (avail, state) {
            (Availability::TestRun, _) => STATUS_TEST_RUN,
            (Availability::NotInstalled, _) => STATUS_NOT_INSTALLED,
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
            dialog: self.dialog.as_ref().map(|d| d.replaces_other),
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
                    self.message = Some(done(r, DONE_ON));
                }
            }
            Click::TurnOff => {
                let r = self.turn_off();
                self.message = Some(done(r, DONE_OFF));
            }
            Click::Remove => {
                let r = self.remove_shortcut();
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
                    // The one allowed on-write is Switch::turn_on / turn_off here.
                    if bad_call && name != "startup_switch.rs" {
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
        assert!(!code.to_lowercase().contains("foxglove"));
        assert!(!code.contains("autostart::ensure("));
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
