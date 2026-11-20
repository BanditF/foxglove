//! What the hub told this module about the sign-in hand-over (sign-in hand-over slice 2).
//!
//! The hub publishes the read-only fact `module.foxglove.starts_at_sign_in`: true while Hideout's
//! Start with Windows switch is on. Foxglove remembers it, and the hub's exe path, in its own
//! `[signin]` settings section so the next sign-in can decide, before any hub is up, whether
//! Hideout is the one that starts it. Nothing here starts, stops or writes anything outside that
//! section; it only remembers. The store is written on change only.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STORE_OWNER: &str = "core";
const SECTION: &str = "signin";

/// The `[signin]` section of the settings store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SigninSettings {
    /// Hideout starts Foxglove at sign-in, as the hub last said.
    pub hideout_starts_me: bool,
    /// The hub's exe, recorded with the fact; `None` whenever the flag is off.
    pub hideout_exe: Option<PathBuf>,
}

/// A local, absolute path to an `.exe` that exists: never a network (`\\server\...`) or
/// `\\?\` path, so a later sign-in never waits on, or runs, something off this PC.
pub fn usable_exe(p: &Path, exists: impl Fn(&Path) -> bool) -> bool {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    let drive =
        b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    drive && s.to_ascii_lowercase().ends_with(".exe") && exists(p)
}

/// What to remember after the hub said `fact`: `Some(new)` only when it differs from `prev`.
/// True with a usable hub exe sets both; false, or true without a usable exe, clears both.
pub fn learn(
    prev: &SigninSettings,
    fact: bool,
    hub_exe: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Option<SigninSettings> {
    let next = match hub_exe.filter(|p| fact && usable_exe(p, &exists)) {
        Some(exe) => SigninSettings {
            hideout_starts_me: true,
            hideout_exe: Some(exe.to_path_buf()),
        },
        None => SigninSettings::default(),
    };
    (next != *prev).then_some(next)
}

/// The log line for a changed value.
pub fn line(on: bool) -> String {
    format!(
        "Hideout starts Foxglove at sign-in: {} (remembered for the next sign-in)",
        if on { "yes" } else { "no" }
    )
}

/// Applies the hub's fact to `section`: writes and logs only on change. Returns whether it did.
pub fn apply(
    section: &super::store::Section,
    fact: bool,
    hub_exe: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
    log: impl FnOnce(&str),
) -> bool {
    let prev = section.try_load::<SigninSettings>().unwrap_or_default();
    let Some(next) = learn(&prev, fact, hub_exe, exists) else {
        return false;
    };
    if let Err(e) = section.save(&next) {
        eprintln!("Foxglove: sign-in hand-over: couldn't remember what the hub said: {e}");
        return false;
    }
    remember_hosted(next.hideout_starts_me);
    log(&line(next.hideout_starts_me));
    true
}

/// Foxglove's Start with Windows line when Hideout starts it, switch on (UX plan, slice 4).
pub const HOSTED_BACKUP_ON: &str = "Hideout starts Foxglove when you sign in. This switch is the backup: if Hideout does not start, Foxglove starts on its own.";
/// The same line with the switch off.
pub const HOSTED_BACKUP_OFF: &str =
    "Hideout starts Foxglove when you sign in. With this off there is no backup.";

/// The line under the Start with Windows switch: only while Hideout starts Foxglove.
pub fn hosted_line(hosted: bool, switch_on: bool) -> Option<&'static str> {
    hosted.then_some(if switch_on {
        HOSTED_BACKUP_ON
    } else {
        HOSTED_BACKUP_OFF
    })
}

/// Remembered `hideout_starts_me`: 0 = not read yet, 1 = no, 2 = yes.
static HOSTED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn remember_hosted(on: bool) {
    HOSTED.store(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
}

/// Whether Hideout starts Foxglove at sign-in, as remembered in `[signin]`.
pub fn hosted() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if HOSTED.load(Relaxed) == 0 {
        let s = super::store::section(STORE_OWNER, SECTION)
            .try_load::<SigninSettings>()
            .unwrap_or_default();
        remember_hosted(s.hideout_starts_me);
    }
    HOSTED.load(Relaxed) == 2
}

/// Draws the line under the switch; nothing when Hideout does not start Foxglove.
pub fn hosted_note(ui: &mut egui::Ui, hosted: bool, switch_on: bool) {
    if let Some(text) = hosted_line(hosted, switch_on) {
        ui.weak(text);
    }
}

/// The hub said `fact` (its exe is `hub_exe` while its process is alive).
pub fn on_fact(fact: bool, hub_exe: Option<PathBuf>) {
    apply(
        &super::store::section(STORE_OWNER, SECTION),
        fact,
        hub_exe.as_deref(),
        |p| p.try_exists().unwrap_or(false),
        super::app_log::line,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const HUB: &str = r"C:\Users\x\AppData\Local\Hideout\hideout.exe";
    fn yes(_: &Path) -> bool {
        true
    }
    fn on() -> SigninSettings {
        SigninSettings {
            hideout_starts_me: true,
            hideout_exe: Some(HUB.into()),
        }
    }

    #[test]
    fn true_with_a_usable_exe_remembers_both() {
        let got = learn(&SigninSettings::default(), true, Some(Path::new(HUB)), yes);
        assert_eq!(got, Some(on()));
    }

    #[test]
    fn false_or_a_bad_exe_clears_both() {
        assert_eq!(
            learn(&on(), false, Some(Path::new(HUB)), yes),
            Some(SigninSettings::default())
        );
        for bad in [
            None,
            Some(r"\\server\share\hideout.exe"),
            Some(r"\\?\C:\Hideout\hideout.exe"),
            Some(r"hideout.exe"),
            Some(r"C:\Hideout\hideout.dll"),
        ] {
            assert_eq!(
                learn(&on(), true, bad.map(Path::new), yes),
                Some(SigninSettings::default()),
                "{bad:?}"
            );
        }
        // A missing file is not usable.
        assert_eq!(
            learn(&on(), true, Some(Path::new(HUB)), |_| false),
            Some(SigninSettings::default())
        );
    }

    #[test]
    fn the_same_value_changes_nothing() {
        assert_eq!(learn(&on(), true, Some(Path::new(HUB)), yes), None);
        assert_eq!(learn(&SigninSettings::default(), false, None, yes), None);
        assert_eq!(
            learn(&SigninSettings::default(), true, Some(Path::new("x")), yes),
            None
        );
    }

    #[test]
    fn the_line_shows_only_when_hosted_and_follows_the_switch() {
        assert_eq!(hosted_line(false, true), None);
        assert_eq!(hosted_line(false, false), None);
        assert_eq!(hosted_line(true, true), Some(HOSTED_BACKUP_ON));
        assert_eq!(hosted_line(true, false), Some(HOSTED_BACKUP_OFF));
        assert_eq!(
            HOSTED_BACKUP_ON,
            "Hideout starts Foxglove when you sign in. This switch is the backup: if Hideout does not start, Foxglove starts on its own."
        );
        assert_eq!(
            HOSTED_BACKUP_OFF,
            "Hideout starts Foxglove when you sign in. With this off there is no backup."
        );
    }

    #[test]
    fn the_note_renders_only_when_hosted() {
        let height = |hosted, on| {
            let ctx = egui::Context::default();
            let mut h = 0.0;
            let out = ctx.run_ui(Default::default(), |ui| {
                hosted_note(ui, hosted, on);
                h = ui.min_rect().height();
            });
            out.drop_without_applying_deltas();
            h
        };
        assert_eq!(height(false, true), 0.0);
        assert_eq!(height(false, false), 0.0);
        assert!(height(true, true) > 0.0);
        assert!(height(true, false) > 0.0);
    }

    #[test]
    fn the_log_line_is_exact() {
        assert_eq!(
            line(true),
            "Hideout starts Foxglove at sign-in: yes (remembered for the next sign-in)"
        );
        assert_eq!(
            line(false),
            "Hideout starts Foxglove at sign-in: no (remembered for the next sign-in)"
        );
    }

    #[test]
    fn the_store_is_written_and_logged_on_change_only() {
        let section = crate::foxglove::store::test_section("signin");
        let lines = RefCell::new(Vec::new());
        let hub = Some(Path::new(HUB));
        let run = |fact| {
            apply(&section, fact, hub, yes, |l| {
                lines.borrow_mut().push(l.to_string())
            })
        };
        assert!(run(true));
        assert_eq!(section.try_load::<SigninSettings>(), Some(on()));
        assert!(!run(true), "same value, no write");
        assert!(run(false));
        assert_eq!(
            section.try_load::<SigninSettings>(),
            Some(SigninSettings::default())
        );
        assert!(!run(false));
        assert_eq!(lines.borrow().len(), 2, "{:?}", lines.borrow());
        assert!(lines.borrow()[0].contains(": yes ("));
        assert!(lines.borrow()[1].contains(": no ("));
    }
}
