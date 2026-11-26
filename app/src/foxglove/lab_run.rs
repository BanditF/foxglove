//! Whether this run is a lab or test run, and what such a run may not reach (#1519, #1526).
//!
//! A Foxglove whose settings folder is set (`FOXGLOVE_CONFIG_DIR`) and isn't the real one, and
//! every test build, is a lab or test run: it is somebody's experiment beside the user's own
//! Foxglove, on the user's PC. How the process was started decides, once, at start; nothing a
//! script has to remember. Such a run keeps its hands off what belongs to the user:
//! - Wallpaper Engine (`wallpaper_engine::reach`, #1519);
//! - **the PC's audio (#1526): no volume or mute of a real device or app is changed**, master
//!   volume is never engaged and no journal claims the user's sessions ([`start`], which tells
//!   `foxglove_panel::audio_reach`, the guard at the Core Audio calls themselves);
//! - smaller things asked here with [`lab_run`]: no fresh copy after a GPU reset, the real
//!   update folder isn't read, the notifications' icon isn't pointed at the lab's folder.
//!
//! A normal run (no `FOXGLOVE_CONFIG_DIR`, or it names the real folder) is unchanged.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use foxglove_panel::audio_reach::{self, Reach};

/// On-call's explicit switch: with `1`, a lab run may set the real default communications
/// microphone's mute (the lab cases that exercise exactly that: `scripts/lab/mic_mute.py`,
/// `scripts/lab/api_tools.py`). Never an output volume; means nothing in a normal run.
pub const REAL_MIC: &str = "FOXGLOVE_LAB_REAL_MIC";

/// Whether this process was started as a lab or test run: its settings folder is set and isn't
/// the real one. The same rule as the plugin auto-install hook (`plugin_install`), asked there.
/// Read once.
pub fn lab_run() -> bool {
    static LAB: OnceLock<bool> = OnceLock::new();
    *LAB.get_or_init(|| {
        let config_dir = match super::store::test_sandbox() {
            Some(dir) => Some(dir.to_path_buf()),
            None => std::env::var_os("FOXGLOVE_CONFIG_DIR").map(PathBuf::from),
        };
        let real = directories::ProjectDirs::from("", "", "Foxglove")
            .map(|d| d.config_dir().to_path_buf());
        lab_run_as(config_dir.as_deref(), real.as_deref())
    })
}

/// [`lab_run`]'s rule: `config_dir` is the folder the run was pointed at (if any), `real` the
/// user's own.
fn lab_run_as(config_dir: Option<&Path>, real: Option<&Path>) -> bool {
    super::plugin_install::autoinstall_allowed(true, config_dir, real).is_ok()
}

/// Says, once and before anything follows the audio, what this run may do with the PC's real
/// volumes and mutes; one log line when that isn't everything. Until this is called nothing is
/// allowed (`foxglove_panel::audio_reach`).
pub fn start() -> Reach {
    let real_mic = real_mic_asked(cfg!(test), std::env::var_os(REAL_MIC).as_deref());
    let reach = audio_reach::set(audio_reach::decide(lab_run(), real_mic));
    #[cfg(not(test))]
    match reach {
        Reach::Real => {}
        Reach::Mic => eprintln!(
            "Foxglove: audio: off for this run (a lab or test run); only the microphone's mute may be set (the lab's microphone switch)"
        ),
        Reach::Off => eprintln!(
            "Foxglove: audio: off for this run (a lab or test run); no volume or mute is changed"
        ),
    }
    reach
}

/// Whether the lab's microphone switch counts: only its value `1`, and never in a test build,
/// whatever the environment of the shell that runs the tests says.
fn real_mic_asked(test_build: bool, value: Option<&std::ffi::OsStr>) -> bool {
    !test_build && value.is_some_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_panel::audio_reach::Target;

    /// A normal run stays a normal run: no folder named, or the real one (itself, in another
    /// case, or its parent, which is what `FOXGLOVE_CONFIG_DIR` names).
    #[test]
    fn only_a_run_pointed_at_another_folder_is_a_lab_run() {
        let real = Path::new(r"C:\Users\someone\AppData\Roaming\Foxglove\config");
        assert!(!lab_run_as(None, Some(real)));
        assert!(!lab_run_as(None, None));
        assert!(!lab_run_as(Some(real), Some(real)));
        assert!(!lab_run_as(
            Some(Path::new(
                r"c:\users\SOMEONE\appdata\roaming\foxglove\CONFIG"
            )),
            Some(real)
        ));
        assert!(!lab_run_as(real.parent(), Some(real)));
        assert!(lab_run_as(
            Some(Path::new(r"C:\Users\someone\AppData\Local\Temp\lab-1")),
            Some(real)
        ));
        // And what follows for the audio: everything in a normal run, nothing in a lab run.
        let normal = audio_reach::decide(lab_run_as(None, Some(real)), false);
        assert_eq!(normal, Reach::Real);
        assert!(normal.allows(Target::Output) && normal.allows(Target::Mic));
        let lab = audio_reach::decide(lab_run_as(Some(Path::new(r"C:\lab")), Some(real)), false);
        assert_eq!(lab, Reach::Off);
    }

    /// Every test build is a lab run, and saying so at start leaves the real audio out of reach:
    /// the local API's microphone mute is refused before any device is looked for.
    #[test]
    fn a_test_build_never_reaches_the_real_audio() {
        assert!(lab_run());
        let reach = start();
        assert_ne!(reach, Reach::Real);
        assert!(!reach.allows(Target::Output));
        assert_eq!(reach, Reach::Off);
        assert_eq!(audio_reach::reach(), reach);
        assert_eq!(
            foxglove_panel::mic::set_mute(true),
            Err(audio_reach::REFUSED.to_string())
        );
        assert!(foxglove_panel::sessions::output().is_err());
    }

    /// The lab's microphone switch is for a lab copy On-call starts, never for a test build:
    /// tests run in whatever shell, and that shell's environment must not arm them.
    #[test]
    fn a_test_build_ignores_the_microphone_switch() {
        use std::ffi::OsStr;
        let one = Some(OsStr::new("1"));
        assert!(real_mic_asked(false, one));
        assert!(!real_mic_asked(true, one));
        assert!(!real_mic_asked(false, None));
        assert!(!real_mic_asked(false, Some(OsStr::new("true"))));
        // And `start` asks with this build's own answer, not a constant.
        let src = include_str!("lab_run.rs");
        assert!(src.contains("real_mic_asked(cfg!(test), std::env::var_os(REAL_MIC)"));
    }

    /// The run says what it may reach before any feature starts, and the knob (the one that
    /// follows the audio sessions and engages master volume) asks before it starts.
    #[test]
    fn the_reach_is_said_before_anything_starts() {
        let src = |name: &str| {
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name))
                .unwrap()
        };
        let main = src("main.rs");
        let said = main
            .find("foxglove::lab_run::start();")
            .expect("main says the reach");
        for later in [
            "foxglove::features::start_all()",
            "foxglove::hub_link::start()",
            "foxglove::gpu_reset::start()",
            "foxglove::update::start()",
            "platform::run(",
        ] {
            assert!(said < main.find(later).expect(later), "{later} comes first");
        }
        let knob = src("foxglove/knob.rs");
        let start = knob.find("\npub fn start() {").expect("knob::start");
        let asked = knob[start..]
            .find("audio_reach::reach()")
            .expect("the knob asks");
        let follows = knob[start..]
            .find("sessions::start(")
            .expect("the knob follows");
        assert!(asked < follows);
    }
}
