//! What this run may do with the PC's real audio (#1526).
//!
//! **A lab or test run never changes a real volume or mute.** A copy of Foxglove started for a
//! test (its settings folder isn't the real one) once engaged master volume on the user's real
//! output device and scaled every app's level; the user's own Foxglove then took the scaled
//! levels for the apps' own. So the program that links this crate says once, at start, what its
//! run may reach ([`set`], decided by [`decide`]), and every write of a volume or mute asks here
//! last, at the Core Audio call itself (the `write` module: the only place those calls are
//! made).
//!
//! Until [`set`] is called the reach is [`Reach::Off`]: a program that never says so, and every
//! test build of this crate, writes nothing.
//!
//! - [`Reach::Real`]: a normal run. Everything as before.
//! - [`Reach::Off`]: nothing is written, the audio sessions are not followed
//!   ([`crate::sessions::start`] starts no thread, so master volume is never engaged and no
//!   journal claims a session) and the microphone reads as unavailable.
//! - [`Reach::Mic`]: a lab run started with the explicit microphone switch (On-call's): as
//!   `Off`, but the default communications microphone is read and its mute may be set. No
//!   output volume or mute, ever.

use std::sync::OnceLock;

/// What a refused write answers.
pub const REFUSED: &str = "audio: off for this run (a lab or test run)";

/// What this run may do with the PC's real audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// A normal run: the real devices, as always.
    Real,
    /// A lab run with the explicit microphone switch: only the microphone's mute.
    Mic,
    /// A lab or test run: nothing.
    Off,
}

/// Which side of the PC's audio a call touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The output device and the apps' sessions on it: levels and mutes.
    Output,
    /// The default communications microphone's mute.
    Mic,
}

impl Reach {
    /// Whether this run may touch `target`.
    pub fn allows(self, target: Target) -> bool {
        match (self, target) {
            (Reach::Real, _) => true,
            (Reach::Mic, Target::Mic) => true,
            (Reach::Mic, Target::Output) | (Reach::Off, _) => false,
        }
    }
}

/// The rule. `lab`: the run is a lab or test run. `real_mic`: the explicit switch that lets a
/// lab run set the real microphone's mute. A normal run is `Real` whatever the switch says.
pub fn decide(lab: bool, real_mic: bool) -> Reach {
    match (lab, real_mic) {
        (false, _) => Reach::Real,
        (true, true) => Reach::Mic,
        (true, false) => Reach::Off,
    }
}

static REACH: OnceLock<Reach> = OnceLock::new();

/// Says what this run may reach: once, at start, before anything follows the audio. The first
/// call decides; what holds is returned.
pub fn set(reach: Reach) -> Reach {
    *REACH.get_or_init(|| reach)
}

/// This run's reach; `Off` until [`set`] said otherwise.
pub fn reach() -> Reach {
    REACH.get().copied().unwrap_or(Reach::Off)
}

/// `Ok` when this run may touch `target`; [`REFUSED`] otherwise.
pub fn guard(target: Target) -> Result<(), String> {
    guard_as(reach(), target)
}

/// [`guard`] under `reach`.
pub fn guard_as(reach: Reach, target: Target) -> Result<(), String> {
    if reach.allows(target) {
        Ok(())
    } else {
        Err(REFUSED.into())
    }
}

/// The Core Audio calls that change a volume or a mute: the only place in this crate that makes
/// them (a test reads the sources to hold that), each behind [`guard`](super::guard).
#[cfg(windows)]
pub(crate) mod write {
    use windows::core::{Error, Result, GUID};
    use windows::Win32::Foundation::E_ACCESSDENIED;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::ISimpleAudioVolume;

    use super::Target;

    fn guard(target: Target) -> Result<()> {
        super::guard(target).map_err(|why| Error::new(E_ACCESSDENIED, why))
    }

    /// The output device's level (0.0-1.0).
    pub fn endpoint_level(e: &IAudioEndpointVolume, level: f32, context: &GUID) -> Result<()> {
        guard(Target::Output)?;
        // SAFETY: a COM call on a live endpoint volume, on its caller's COM thread; the context
        // outlives the call.
        unsafe { e.SetMasterVolumeLevelScalar(level, context) }
    }

    /// The output device's mute.
    pub fn endpoint_mute(e: &IAudioEndpointVolume, muted: bool, context: &GUID) -> Result<()> {
        guard(Target::Output)?;
        // SAFETY: as above.
        unsafe { e.SetMute(muted, context) }
    }

    /// One app session's level (0.0-1.0).
    pub fn session_level(v: &ISimpleAudioVolume, level: f32, context: &GUID) -> Result<()> {
        guard(Target::Output)?;
        // SAFETY: a COM call on a live session volume, on its caller's COM thread.
        unsafe { v.SetMasterVolume(level, context) }
    }

    /// One app session's mute.
    pub fn session_mute(v: &ISimpleAudioVolume, muted: bool, context: &GUID) -> Result<()> {
        guard(Target::Output)?;
        // SAFETY: as above.
        unsafe { v.SetMute(muted, context) }
    }

    /// The microphone's mute.
    pub fn mic_mute(e: &IAudioEndpointVolume, muted: bool, context: &GUID) -> Result<()> {
        guard(Target::Mic)?;
        // SAFETY: a COM call on a live endpoint volume, on its caller's COM thread.
        unsafe { e.SetMute(muted, context) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_run_reaches_everything_and_a_lab_run_nothing_or_the_microphone() {
        // A normal run: unchanged, whatever the switch says.
        for real_mic in [false, true] {
            let r = decide(false, real_mic);
            assert_eq!(r, Reach::Real);
            assert!(r.allows(Target::Output) && r.allows(Target::Mic));
            assert_eq!(guard_as(r, Target::Output), Ok(()));
            assert_eq!(guard_as(r, Target::Mic), Ok(()));
        }
        // A lab run: nothing.
        let off = decide(true, false);
        assert_eq!(off, Reach::Off);
        assert_eq!(guard_as(off, Target::Output), Err(REFUSED.to_string()));
        assert_eq!(guard_as(off, Target::Mic), Err(REFUSED.to_string()));
        // With the explicit switch: the microphone's mute, never an output.
        let mic = decide(true, true);
        assert_eq!(mic, Reach::Mic);
        assert_eq!(guard_as(mic, Target::Mic), Ok(()));
        assert_eq!(guard_as(mic, Target::Output), Err(REFUSED.to_string()));
    }

    /// Nothing in this crate's tests calls [`set`], so a test build writes nothing.
    #[test]
    fn a_run_that_never_said_reaches_nothing() {
        assert_eq!(reach(), Reach::Off);
        assert!(guard(Target::Output).is_err());
        assert!(guard(Target::Mic).is_err());
        // No thread, no device, no journal: the sessions are not followed at all.
        struct Never;
        impl crate::master::Host for Never {
            fn journal(&self) -> Result<Option<crate::master::Journal>, String> {
                panic!("a lab run read the journal")
            }
            fn write_journal(&mut self, _: &crate::master::Journal) -> Result<(), String> {
                panic!("a lab run wrote the journal")
            }
            fn choice(&self, _: &str) -> Option<bool> {
                panic!("a lab run read the device's choice")
            }
            fn listed(&self, _: &[String]) -> bool {
                panic!("a lab run looked the device up")
            }
            fn saved(&self, _: &str) -> Option<(f32, bool)> {
                panic!("a lab run read the saved level")
            }
            fn remember(&mut self, _: &str, _: &str, _: f32, _: bool) {
                panic!("a lab run saved a level")
            }
        }
        let sessions = crate::sessions::start(
            |_| panic!("a lab run heard a session change"),
            Some(Box::new(Never)),
        );
        assert!(sessions.is_none());
        assert!(crate::sessions::output().is_err());
        crate::sessions::recheck();
        crate::sessions::stop(std::time::Duration::from_millis(1));
        // The microphone: unavailable, read from nowhere, and its mute refused.
        let (tx, rx) = std::sync::mpsc::channel();
        crate::mic::watch(move |m| {
            let _ = tx.send(m);
        });
        let mic = rx.try_recv().expect("said at once, on the caller's thread");
        assert_eq!(mic.state, crate::mic::MicState::Unavailable);
        assert!(mic.id.is_empty() && mic.name.is_empty());
        assert_eq!(crate::mic::set_mute(true), Err(REFUSED.to_string()));
        assert_eq!(crate::mic::set_mute(false), Err(REFUSED.to_string()));
    }

    /// The words that change a real volume or mute through Core Audio.
    const WRITES: &[&str] = &[
        ".SetMasterVolumeLevelScalar(",
        ".SetMasterVolumeLevel(",
        ".SetMasterVolume(",
        ".SetMute(",
        ".SetChannelVolumeLevelScalar(",
        ".SetChannelVolumeLevel(",
        ".SetChannelVolume(",
        ".SetAllVolumes(",
        ".VolumeStepUp(",
        ".VolumeStepDown(",
    ];

    /// Every Core Audio write in this crate is one of the `write` module's functions, and each
    /// of those asks the guard before its call.
    #[test]
    fn every_core_audio_write_is_behind_the_guard() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let text = std::fs::read_to_string(&path).unwrap();
            if name != "audio_reach.rs" {
                for w in WRITES {
                    assert!(
                        !text.contains(w),
                        "{name} calls {w}..) itself; go through audio_reach::write"
                    );
                }
                continue;
            }
            // This file: only the `write` module, up to the tests.
            let from = text.find("pub(crate) mod write {").unwrap();
            let to = text.find("#[cfg(test)]").unwrap();
            assert!(
                WRITES.iter().all(|w| !text[..from].contains(w)),
                "a write outside the write module"
            );
            for body in text[from..to].split("\n    pub fn ").skip(1) {
                let Some(at) = WRITES.iter().filter_map(|w| body.find(w)).min() else {
                    continue;
                };
                let guard = body.find("guard(Target::").expect("a write with no guard");
                assert!(guard < at, "the guard comes after the write: {body}");
                assert!(
                    body[guard..at].contains(")?;"),
                    "the guard's refusal isn't returned: {body}"
                );
                seen += 1;
            }
        }
        assert_eq!(seen, 5, "every function of the write module is checked");
    }
}
