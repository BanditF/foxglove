// SPDX-License-Identifier: MIT
//! Start with Windows for the hub: a `Hideout.lnk` shortcut in the user's Startup folder that runs
//! `hideout.exe --tray`, and Windows' own on/off state for it, the `Hideout.lnk` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder`.
//!
//! Clean-room: written from the behaviour spec in `docs/packaging.md` (the Start with Windows
//! rules), not from any app code. H5.2 wires the user's switch ([`crate::startup_switch`]); the
//! startup pass [`ensure`] is NOT called by anything and the switch is off by default.
//!
//! Rules:
//! - Startup never turns the entry back on (#684). Disabled in Windows stays disabled, and a
//!   shortcut the user removed stays removed: the StartupApproved value is the "we made one once"
//!   mark, so no shortcut plus a value means removed on purpose.
//! - Only the user's switch ([`set_enabled`]) writes an enabled state.
//! - A shortcut starting another existing exe is left alone at startup; one whose file is gone is
//!   repaired. A shortcut for this exe without `--tray` is rewritten in place, approval untouched.
//! - Removal ([`remove`]) touches only a `Hideout.lnk` that starts this exe, and its approval value
//!   with it. `Foxglove.lnk` is never read or written.
//! - A run on a `HIDEOUT_CONFIG_DIR` / `HIDEOUT_HOME` folder writes nothing.
//! - Every call is idempotent: running it twice changes nothing the second time.
//!
//! The Startup folder and the registry sit behind two traits, so the tests use fakes and need no
//! window, no registry and no disk.

use std::path::{Path, PathBuf};

/// The shortcut's file name in the Startup folder, and its StartupApproved value name.
pub const LINK_NAME: &str = "Hideout.lnk";
/// The one argument the shortcut carries.
pub const TRAY_ARG: &str = "--tray";

/// What a shortcut starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub target: PathBuf,
    pub args: String,
}

/// The user's Startup folder, seen through `Hideout.lnk` only.
pub trait StartupFolder {
    /// The shortcut, or `None` when there is none.
    fn read(&self) -> Option<Link>;
    /// Creates or replaces the shortcut.
    fn write(&mut self, link: &Link) -> Result<(), String>;
    /// Deletes the shortcut; absent is success.
    fn delete(&mut self) -> Result<(), String>;
    /// Whether a file exists (decides repair against a second install).
    fn exists(&self, path: &Path) -> bool;
}

/// The StartupApproved value for `Hideout.lnk`.
pub trait Approval {
    /// `Some(true)` enabled, `Some(false)` disabled, `None` no value.
    fn read(&self) -> Option<bool>;
    fn write(&mut self, enabled: bool) -> Result<(), String>;
    /// Deletes the value; absent is success.
    fn delete(&mut self) -> Result<(), String>;
}

/// Who is asking: this install's exe, and whether this is a lab or test run.
#[derive(Clone, Debug)]
pub struct Context {
    pub exe: PathBuf,
    /// True for a `HIDEOUT_CONFIG_DIR` / `HIDEOUT_HOME` run (`HubPaths::overridden`): no writes.
    pub overridden: bool,
}

/// The entry as Windows and the shortcut say it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No shortcut and nothing marking one as ever made.
    Missing,
    /// A shortcut for this exe that Windows has not disabled.
    On,
    /// Disabled in Windows, or the shortcut was removed after it was made.
    Off,
    /// A shortcut that starts something else.
    Foreign,
}

/// What a call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing needed changing.
    Unchanged,
    /// Something was written or removed.
    Changed,
    /// A lab or test run: nothing was written.
    SkippedLabRun,
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn has_tray(args: &str) -> bool {
    args.split_whitespace()
        .any(|a| a.eq_ignore_ascii_case(TRAY_ARG))
}

fn wanted(ctx: &Context) -> Link {
    Link {
        target: ctx.exe.clone(),
        args: TRAY_ARG.to_string(),
    }
}

/// Reads the state. Never writes.
pub fn state(ctx: &Context, folder: &impl StartupFolder, approval: &impl Approval) -> State {
    let approved = approval.read();
    match folder.read() {
        None => {
            if approved.is_some() {
                State::Off
            } else {
                State::Missing
            }
        }
        Some(link) if !same_path(&link.target, &ctx.exe) => State::Foreign,
        Some(_) if approved == Some(false) => State::Off,
        Some(_) => State::On,
    }
}

/// The startup pass, for a switch that is on by default. Makes the shortcut only when none was
/// ever made, repairs one whose file is gone, adds `--tray` to this exe's own. Never turns a
/// disabled or removed entry back on.
pub fn ensure(
    ctx: &Context,
    folder: &mut impl StartupFolder,
    approval: &mut impl Approval,
) -> Result<Outcome, String> {
    if ctx.overridden {
        return Ok(Outcome::SkippedLabRun);
    }
    let approved = approval.read();
    match folder.read() {
        None => {
            if approved.is_some() {
                return Ok(Outcome::Unchanged); // removed on purpose
            }
            folder.write(&wanted(ctx))?;
            approval.write(true)?;
            Ok(Outcome::Changed)
        }
        Some(link) if same_path(&link.target, &ctx.exe) => {
            if has_tray(&link.args) {
                Ok(Outcome::Unchanged)
            } else {
                folder.write(&wanted(ctx))?; // in place, approval untouched
                Ok(Outcome::Changed)
            }
        }
        Some(link) => {
            if folder.exists(&link.target) {
                Ok(Outcome::Unchanged) // a second install's entry
            } else {
                folder.write(&wanted(ctx))?;
                Ok(Outcome::Changed)
            }
        }
    }
}

/// The narrow startup repair (sign-in hand-over slice 3): a `Hideout.lnk` that starts this exe,
/// is enabled in Windows, and lacks `--tray` gets its arguments rewritten in place (the approval
/// is not touched). It never creates a missing shortcut, never turns a disabled or removed entry
/// back on, never changes another exe's shortcut, and writes nothing in a lab or test run.
pub fn repair_tray_arg(
    ctx: &Context,
    folder: &mut impl StartupFolder,
    approval: &impl Approval,
) -> Result<Outcome, String> {
    if ctx.overridden {
        return Ok(Outcome::SkippedLabRun);
    }
    match folder.read() {
        Some(link)
            if same_path(&link.target, &ctx.exe)
                && !has_tray(&link.args)
                && approval.read() == Some(true) =>
        {
            folder.write(&wanted(ctx))?;
            Ok(Outcome::Changed)
        }
        _ => Ok(Outcome::Unchanged),
    }
}

/// The user's switch. Writes what Windows' own toggle writes; turning on also makes the shortcut
/// when it is missing, starts another exe, or lacks `--tray`.
pub fn set_enabled(
    ctx: &Context,
    on: bool,
    folder: &mut impl StartupFolder,
    approval: &mut impl Approval,
) -> Result<Outcome, String> {
    if ctx.overridden {
        return Ok(Outcome::SkippedLabRun);
    }
    let mut changed = false;
    let before = approval.read();
    if before != Some(on) {
        // Off with no shortcut still records the value: that is how "removed" stays removed.
        // Written first: a shortcut with no value reads as On, a value with no shortcut as Off,
        // so a failure between the two writes can never leave autostart on.
        approval.write(on)?;
        changed = true;
    }
    if on {
        let want = wanted(ctx);
        if folder.read().as_ref() != Some(&want) {
            if let Err(e) = folder.write(&want) {
                // Put the value back as it was.
                let _ = match before {
                    Some(v) => approval.write(v),
                    None => approval.delete(),
                };
                return Err(e);
            }
            changed = true;
        }
    }
    Ok(if changed {
        Outcome::Changed
    } else {
        Outcome::Unchanged
    })
}

/// Uninstall cleanup: removes `Hideout.lnk` and its approval value, only when the shortcut starts
/// this exe. A foreign or missing shortcut leaves both alone.
pub fn remove(
    ctx: &Context,
    folder: &mut impl StartupFolder,
    approval: &mut impl Approval,
) -> Result<Outcome, String> {
    if ctx.overridden {
        return Ok(Outcome::SkippedLabRun);
    }
    match folder.read() {
        Some(link) if same_path(&link.target, &ctx.exe) => {
            folder.delete()?;
            approval.delete()?;
            Ok(Outcome::Changed)
        }
        _ => Ok(Outcome::Unchanged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Folder {
        link: Option<Link>,
        files: Vec<PathBuf>,
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
        fn exists(&self, p: &Path) -> bool {
            self.files.iter().any(|f| same_path(f, p))
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

    const EXE: &str = r"C:\Users\u\AppData\Local\Programs\Hideout\hideout.exe";
    const OTHER: &str = r"D:\other\hideout.exe";

    fn ctx() -> Context {
        Context {
            exe: PathBuf::from(EXE),
            overridden: false,
        }
    }
    fn link(t: &str, a: &str) -> Link {
        Link {
            target: PathBuf::from(t),
            args: a.into(),
        }
    }
    fn world(l: Option<Link>, v: Option<bool>) -> (Folder, Reg) {
        (
            Folder {
                link: l,
                ..Default::default()
            },
            Reg {
                v,
                ..Default::default()
            },
        )
    }

    #[test]
    fn state_table() {
        let c = ctx();
        let cases = [
            (None, None, State::Missing),
            (None, Some(true), State::Off),
            (None, Some(false), State::Off),
            (Some(link(EXE, "--tray")), None, State::On),
            (Some(link(EXE, "--tray")), Some(true), State::On),
            (Some(link(EXE, "--tray")), Some(false), State::Off),
            (Some(link(OTHER, "--tray")), Some(true), State::Foreign),
            (Some(link(OTHER, "")), None, State::Foreign),
        ];
        for (l, v, want) in cases {
            let (f, r) = world(l.clone(), v);
            assert_eq!(state(&c, &f, &r), want, "{l:?} {v:?}");
        }
    }

    #[test]
    fn ensure_makes_it_once_when_never_made() {
        let (mut f, mut r) = world(None, None);
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Changed));
        assert_eq!(f.link, Some(link(EXE, "--tray")));
        assert_eq!(r.v, Some(true));
        let (fw, rw) = (f.writes, r.writes);
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        assert_eq!((f.writes, r.writes), (fw, rw));
    }

    #[test]
    fn ensure_never_re_enables() {
        // Disabled in Windows.
        let (mut f, mut r) = world(Some(link(EXE, "--tray")), Some(false));
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        assert_eq!((f.writes, r.writes, r.v), (0, 0, Some(false)));
        // Disabled and a stale shortcut without --tray: fixed in place, still disabled.
        let (mut f, mut r) = world(Some(link(EXE, "")), Some(false));
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Changed));
        assert_eq!(f.link, Some(link(EXE, "--tray")));
        assert_eq!((r.writes, r.v), (0, Some(false)));
        // Shortcut removed by the user: stays removed.
        let (mut f, mut r) = world(None, Some(true));
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        assert!(f.link.is_none());
    }

    #[test]
    fn ensure_foreign_target() {
        // Another install that exists: left alone.
        let (mut f, mut r) = world(Some(link(OTHER, "--tray")), Some(true));
        f.files.push(PathBuf::from(OTHER));
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        assert_eq!(f.link, Some(link(OTHER, "--tray")));
        // Its file is gone: repaired to this exe, approval untouched.
        let (mut f, mut r) = world(Some(link(OTHER, "--tray")), Some(true));
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Changed));
        assert_eq!(f.link, Some(link(EXE, "--tray")));
        assert_eq!(r.writes, 0);
    }

    #[test]
    fn switch_on_and_off_twice() {
        let (mut f, mut r) = world(None, None);
        assert_eq!(
            set_enabled(&ctx(), true, &mut f, &mut r),
            Ok(Outcome::Changed)
        );
        assert_eq!(
            (f.link.clone(), r.v),
            (Some(link(EXE, "--tray")), Some(true))
        );
        assert_eq!(
            set_enabled(&ctx(), true, &mut f, &mut r),
            Ok(Outcome::Unchanged)
        );
        assert_eq!(
            set_enabled(&ctx(), false, &mut f, &mut r),
            Ok(Outcome::Changed)
        );
        assert_eq!(r.v, Some(false));
        assert!(
            f.link.is_some(),
            "off keeps the shortcut, like Windows' toggle"
        );
        assert_eq!(
            set_enabled(&ctx(), false, &mut f, &mut r),
            Ok(Outcome::Unchanged)
        );
        assert_eq!(state(&ctx(), &f, &r), State::Off);
        // Startup after the user turned it off does not undo it (#684).
        assert_eq!(ensure(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        assert_eq!(r.v, Some(false));
    }

    #[test]
    fn switch_on_takes_over_a_foreign_shortcut() {
        let (mut f, mut r) = world(Some(link(OTHER, "--tray")), Some(false));
        assert_eq!(
            set_enabled(&ctx(), true, &mut f, &mut r),
            Ok(Outcome::Changed)
        );
        assert_eq!(f.link, Some(link(EXE, "--tray")));
        assert_eq!(r.v, Some(true));
    }

    #[test]
    fn remove_is_exact_target_and_idempotent() {
        let (mut f, mut r) = world(Some(link(EXE, "--tray")), Some(true));
        assert_eq!(remove(&ctx(), &mut f, &mut r), Ok(Outcome::Changed));
        assert!(f.link.is_none() && r.v.is_none());
        assert_eq!(remove(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        // Foreign: both stay.
        let (mut f, mut r) = world(Some(link(OTHER, "--tray")), Some(true));
        assert_eq!(remove(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        assert_eq!((f.link.is_some(), r.v), (true, Some(true)));
        // Missing shortcut: a leftover value is not ours to delete.
        let (mut f, mut r) = world(None, Some(false));
        assert_eq!(remove(&ctx(), &mut f, &mut r), Ok(Outcome::Unchanged));
        assert_eq!(r.v, Some(false));
    }

    #[test]
    fn path_compare_ignores_case_and_slashes() {
        let (mut f, mut r) = world(
            Some(link(&EXE.to_uppercase().replace('\\', "/"), "--TRAY")),
            Some(true),
        );
        assert_eq!(state(&ctx(), &f, &r), State::On);
        assert_eq!(remove(&ctx(), &mut f, &mut r), Ok(Outcome::Changed));
    }

    #[test]
    fn a_failed_write_never_leaves_it_on() {
        struct BadReg(Reg);
        impl Approval for BadReg {
            fn read(&self) -> Option<bool> {
                self.0.read()
            }
            fn write(&mut self, _: bool) -> Result<(), String> {
                Err("registry".into())
            }
            fn delete(&mut self) -> Result<(), String> {
                self.0.delete()
            }
        }
        struct BadFolder(Folder);
        impl StartupFolder for BadFolder {
            fn read(&self) -> Option<Link> {
                self.0.read()
            }
            fn write(&mut self, _: &Link) -> Result<(), String> {
                Err("disk".into())
            }
            fn delete(&mut self) -> Result<(), String> {
                self.0.delete()
            }
            fn exists(&self, p: &Path) -> bool {
                self.0.exists(p)
            }
        }
        // The value cannot be written: no shortcut is made.
        let (f, r) = world(None, None);
        let (mut f, mut r) = (f, BadReg(r));
        assert!(set_enabled(&ctx(), true, &mut f, &mut r).is_err());
        assert!(f.link.is_none());
        assert_ne!(state(&ctx(), &f, &r), State::On);
        // The shortcut cannot be written: the value goes back to what it was.
        for before in [None, Some(false)] {
            let (f, r) = world(None, before);
            let (mut f, mut r) = (BadFolder(f), r);
            assert!(set_enabled(&ctx(), true, &mut f, &mut r).is_err());
            assert_eq!(r.v, before);
            assert_ne!(state(&ctx(), &f, &r), State::On);
        }
    }

    #[test]
    fn lab_run_writes_nothing() {
        let c = Context {
            overridden: true,
            ..ctx()
        };
        let (mut f, mut r) = world(Some(link(EXE, "")), None);
        assert_eq!(ensure(&c, &mut f, &mut r), Ok(Outcome::SkippedLabRun));
        assert_eq!(
            set_enabled(&c, true, &mut f, &mut r),
            Ok(Outcome::SkippedLabRun)
        );
        assert_eq!(
            set_enabled(&c, false, &mut f, &mut r),
            Ok(Outcome::SkippedLabRun)
        );
        assert_eq!(remove(&c, &mut f, &mut r), Ok(Outcome::SkippedLabRun));
        assert_eq!((f.writes, r.writes), (0, 0));
        assert_eq!(state(&c, &f, &r), State::On);
    }

    #[test]
    fn repair_adds_the_tray_arg_to_an_enabled_shortcut_in_place() {
        let mut f = Folder {
            link: Some(link(EXE, "")),
            ..Default::default()
        };
        let a = Reg {
            v: Some(true),
            ..Default::default()
        };
        assert_eq!(repair_tray_arg(&ctx(), &mut f, &a), Ok(Outcome::Changed));
        assert_eq!(f.link, Some(link(EXE, TRAY_ARG)));
        assert_eq!((f.writes, a.writes), (1, 0));
        // Idempotent.
        assert_eq!(repair_tray_arg(&ctx(), &mut f, &a), Ok(Outcome::Unchanged));
        assert_eq!(f.writes, 1);
    }

    #[test]
    fn repair_leaves_disabled_removed_missing_and_foreign_alone() {
        for (l, v) in [
            (Some(link(EXE, "")), Some(false)),  // disabled
            (Some(link(EXE, "")), None),         // no approval value
            (None, Some(true)),                  // removed
            (None, None),                        // missing
            (Some(link(OTHER, "")), Some(true)), // foreign target
        ] {
            let mut f = Folder {
                link: l.clone(),
                ..Default::default()
            };
            let a = Reg {
                v,
                ..Default::default()
            };
            assert_eq!(repair_tray_arg(&ctx(), &mut f, &a), Ok(Outcome::Unchanged));
            assert_eq!((f.link, f.writes, a.writes), (l, 0, 0));
        }
    }

    #[test]
    fn repair_skips_a_lab_run() {
        let mut f = Folder {
            link: Some(link(EXE, "")),
            ..Default::default()
        };
        let a = Reg {
            v: Some(true),
            ..Default::default()
        };
        let lab = Context {
            overridden: true,
            ..ctx()
        };
        assert_eq!(
            repair_tray_arg(&lab, &mut f, &a),
            Ok(Outcome::SkippedLabRun)
        );
        assert_eq!(f.writes, 0);
    }
}
