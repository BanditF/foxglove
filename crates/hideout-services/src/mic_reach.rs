// SPDX-License-Identifier: MIT
//! Whether this hub may touch the PC's real microphone (#1526).
//!
//! Only the installed hub, in the user's own settings folder, may read or set the default
//! communications microphone's mute. A hub that runs from a folder of its own (`HIDEOUT_CONFIG_DIR`
//! somewhere else, which every lab hub has), a hub in lab mode and a test build may not: they
//! share the PC with the user, and a mute they set is the user's microphone muted.
//!
//! [`start`](crate::start) says it once, from what the hub already knows of itself: its folder's
//! suffix (`hideout_router::server::Hub::folder_suffix`, empty only for the default folder) and
//! its lab mode. Until then, and in a run that never says, nothing is allowed: [`real`] fails
//! closed. `mic_win` asks [`guard`] before Core Audio is reached and again before the one mute
//! write, so a caller that forgets the first check still changes nothing.

use std::sync::OnceLock;

/// The refusal, for the log and for a caller: what is off and what to do instead. No paths.
pub const REFUSED: &str = "the microphone is off in this hub (it runs from its own settings folder, in lab mode, or is a test build); run the installed Hideout to read or mute the real microphone";

static REAL: OnceLock<bool> = OnceLock::new();

/// The rule: the real microphone only for a hub in the default folder (`folder_suffix` empty)
/// that isn't in lab mode, and never in a test build.
pub fn decide(folder_suffix: &str, lab: bool) -> bool {
    decide_as(cfg!(test), folder_suffix, lab)
}

fn decide_as(test_build: bool, folder_suffix: &str, lab: bool) -> bool {
    !test_build && folder_suffix.is_empty() && !lab
}

/// Says it for this process; the first call wins. Returns what holds.
pub fn set(real: bool) -> bool {
    *REAL.get_or_init(|| real)
}

/// Whether this hub may touch the real microphone. False until [`set`] said otherwise.
pub fn real() -> bool {
    REAL.get().copied().unwrap_or(false)
}

/// `Err(REFUSED)` unless this hub may touch the real microphone.
pub fn guard() -> Result<(), &'static str> {
    if real() {
        Ok(())
    } else {
        Err(REFUSED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_installed_hub_in_its_own_folder_reaches_the_microphone() {
        assert!(decide_as(false, "", false));
        // A folder of its own (every lab hub), lab mode, a test build: each alone is enough.
        assert!(!decide_as(false, ".0123456789abcdef", false));
        assert!(!decide_as(false, "", true));
        assert!(!decide_as(true, "", false));
        // This is a test build.
        assert!(!decide("", false));
    }

    #[test]
    fn a_hub_that_never_said_reaches_nothing() {
        // No test says "real": the default holds for the whole test binary, and saying "off"
        // keeps it.
        assert!(!real());
        assert_eq!(guard(), Err(REFUSED));
        assert!(!set(decide("", false)));
        assert!(!real());
    }

    #[test]
    fn the_refusal_says_what_to_do_and_names_no_folder() {
        assert!(REFUSED.contains("run the installed Hideout"));
        assert!(!REFUSED.contains('\\') && !REFUSED.contains('/') && !REFUSED.contains(':'));
    }

    /// The text of `fn name` in `src`, up to the next `fn ` at the same depth.
    fn body<'a>(src: &'a str, name: &str) -> &'a str {
        let at = src.find(name).unwrap_or_else(|| panic!("{name} not found"));
        let rest = &src[at + name.len()..];
        &rest[..rest.find("\n    fn ").unwrap_or(rest.len())]
    }

    #[test]
    fn core_audio_is_asked_only_after_the_guard() {
        let src = include_str!("mic_win.rs");
        // Reaching Core Audio at all (the bind and the mute read hang off this).
        let new = body(src, "pub fn new(");
        let guard = new
            .find("mic_reach::guard()")
            .expect("new() asks the guard");
        assert!(guard < new.find("CoInitializeEx").unwrap());
        assert!(guard < new.find("CoCreateInstance").unwrap());
        // The one write, and it is the only one in the file.
        let write = concat!(".Set", "Mute(");
        let set = body(src, "fn set_mute(");
        assert!(
            set.find("mic_reach::guard()")
                .expect("set_mute asks the guard")
                < set.find(write).unwrap()
        );
        assert_eq!(src.matches(write).count(), 1);
        // And `start` says it before the microphone starts.
        let lib = include_str!("lib.rs");
        let start = body(lib, "pub fn start(");
        assert!(start.find("mic_reach::set(").unwrap() < start.find("start_mic(").unwrap());
    }
}
