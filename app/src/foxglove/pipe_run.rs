//! What an outside tool is granted on the app's own pipe (#1449), decided in one place
//! ([`grant`]) with two named arms: a normal run and a lab run.
//!
//! What must hold for the lab arm:
//!
//! - It comes only from how this process was started: the lab's launch marker
//!   (`FOXGLOVE_APP_OK=1`, [`super::away::LAB_ENV`], which labkit's `App` runs every test app
//!   under) together with a `FOXGLOVE_CONFIG_DIR` that is not the user's real settings folder.
//!   It is read once at start. Nothing a pipe client sends (its name, its `initialize`, any
//!   call) is looked at, so a client can't turn it on.
//! - It is off by default: a normal install has neither variable, and the marker alone (or on
//!   the real settings folder) is refused and said in the log.
//! - It never grants a sensitive permission (`Permission::sensitive`).
//! - When on, it is said once in the log at start, without a path, a name or a secret.
//!
//! Both arms grant the same today (everything not sensitive, the rule the pipe has had since
//! #1142). They are kept apart so the normal arm can move to the hub's free / ask / never
//! classes (`hideout_contract::capability::tool_class`) without the lab harness
//! (`scripts/lab`, which connects as an outside tool) losing what it asks for.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use foxglove_plugin_api::permission::Permission;

/// Which rule the pipe's approver follows in this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeRun {
    /// Every run but a lab run: the user's install.
    Normal,
    /// A lab run (see the module's words): a test app on its own settings folder.
    Lab,
}

/// What the start-up variables say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decided {
    /// No lab marker: a normal run, nothing to say.
    Normal,
    /// The marker and an own settings folder.
    Lab,
    /// The marker without what a lab run needs: a normal run, and the reason for the log.
    Refused(&'static str),
}

impl Decided {
    pub fn run(self) -> PipeRun {
        match self {
            Decided::Lab => PipeRun::Lab,
            Decided::Normal | Decided::Refused(_) => PipeRun::Normal,
        }
    }
}

/// `dir` is the real settings folder `real` or the folder that holds it.
pub fn real_settings_folder(dir: &Path, real: &Path) -> bool {
    let same = |a: &Path, b: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(a) == canon(b)
            || a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
    };
    same(dir, real) || real.parent().is_some_and(|p| same(dir, p))
}

/// The run, given the lab marker, `FOXGLOVE_CONFIG_DIR` and the user's real settings folder.
/// Anything short of all three reading as a lab run is a normal run.
pub fn decide(marker: Option<&OsStr>, config_dir: Option<&Path>, real: Option<&Path>) -> Decided {
    if marker != Some(OsStr::new("1")) {
        return Decided::Normal;
    }
    let Some(dir) = config_dir.filter(|d| !d.as_os_str().is_empty()) else {
        return Decided::Refused("FOXGLOVE_CONFIG_DIR isn't set");
    };
    let Some(real) = real else {
        return Decided::Refused("the real settings folder isn't known");
    };
    if real_settings_folder(dir, real) {
        return Decided::Refused("FOXGLOVE_CONFIG_DIR is the real settings folder");
    }
    Decided::Lab
}

/// [`decide`] from this process's own environment, said once in the log. Called once, when the
/// pipe starts; the answer is kept for the run.
pub fn at_start() -> PipeRun {
    let marker: Option<OsString> = std::env::var_os(super::away::LAB_ENV);
    let config_dir = std::env::var_os("FOXGLOVE_CONFIG_DIR").map(std::path::PathBuf::from);
    let real =
        directories::ProjectDirs::from("", "", "Foxglove").map(|d| d.config_dir().to_path_buf());
    let decided = decide(marker.as_deref(), config_dir.as_deref(), real.as_deref());
    if let Some(line) = start_line(decided) {
        eprintln!("{line}");
    }
    decided.run()
}

/// The log line of a run, if it has one. No path, no client name, nothing secret.
pub fn start_line(decided: Decided) -> Option<String> {
    let lab = super::away::LAB_ENV;
    match decided {
        Decided::Normal => None,
        Decided::Lab => Some(format!(
            "Foxglove API: lab run ({lab}=1 and its own settings folder): outside tools keep the lab rule, everything not sensitive is granted without asking"
        )),
        Decided::Refused(why) => Some(format!(
            "Foxglove API: {lab}=1 is set but this isn't a lab run ({why}); outside tools get the normal rule"
        )),
    }
}

/// What a tool that asked for `asked` is granted in `run`. The one place that decides it.
pub fn grant(run: PipeRun, asked: &[Permission]) -> Vec<Permission> {
    let allows: fn(&Permission) -> bool = match run {
        PipeRun::Normal => normal_allows,
        PipeRun::Lab => lab_allows,
    };
    asked.iter().filter(|p| allows(p)).cloned().collect()
}

/// The normal arm. No prompt yet (#1142): a tool is granted what it asks for when it connects,
/// except the sensitive permissions (key events, raw HID, any network), which stay off until a
/// per-tool permission dialog (contract §6.4) asks. The pipe admits this user only, not remote
/// clients and not their low-integrity (sandboxed) processes (`plugin-host` pipe tests).
fn normal_allows(p: &Permission) -> bool {
    !p.sensitive()
}

/// The lab arm: what the lab harness needs to drive a test app without a window to answer in.
/// Never a sensitive permission.
fn lab_allows(p: &Permission) -> bool {
    !p.sensitive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(texts: &[&str]) -> Vec<Permission> {
        texts
            .iter()
            .map(|t| Permission::try_from(t.to_string()).unwrap_or_else(|e| panic!("{t}: {e}")))
            .collect()
    }

    fn texts(granted: &[Permission]) -> Vec<String> {
        granted.iter().cloned().map(String::from).collect()
    }

    /// One of every kind of permission a tool can ask for.
    const EVERY: &[&str] = &[
        "facts.read:*",
        "facts.read:mic.*",
        "facts.write:media.*",
        "devices.read",
        "lighting.write",
        "behaviors.invoke",
        "screens.write",
        "art.write",
        "key.events",
        "hid.raw",
        "profiles.write",
        "focus.write",
        "net",
        "net.connect:example.com:443",
        "net.listen:udp:127.0.0.1:9000",
        "net.listen:tcp:127.0.0.1:9000",
        "files",
        "audio.inputs.read",
        "audio.inputs.mute",
        "secrets",
        "app.windows",
        "wallpaper.control",
        "apps.watch:example.exe",
    ];

    const SENSITIVE: &[&str] = &["key.events", "hid.raw", "net"];

    /// Today's rule for a normal run, pinned (#1449): everything asked for but the sensitive
    /// ones, in the order asked. The change to the hub's classes edits this test on purpose.
    #[test]
    fn a_normal_run_grants_everything_not_sensitive() {
        let granted = texts(&grant(PipeRun::Normal, &perms(EVERY)));
        let want: Vec<String> = EVERY
            .iter()
            .filter(|t| !SENSITIVE.contains(t))
            .map(|t| t.to_string())
            .collect();
        assert_eq!(granted, want);
        assert!(grant(PipeRun::Normal, &[]).is_empty());
    }

    /// What the lab harness asks for (`scripts/lab` through labkit's `connect`, `foxglove-cli`
    /// and `foxglove-lab`'s app API): a lab run grants all of it.
    #[test]
    fn a_lab_run_grants_what_the_harness_asks_for() {
        let harness = [
            "devices.read",
            "facts.read:*",
            "facts.read:mic.*",
            "lighting.write",
            "art.write",
            "profiles.write",
            "app.windows",
            "audio.inputs.read",
            "audio.inputs.mute",
        ];
        assert_eq!(
            texts(&grant(PipeRun::Lab, &perms(&harness))),
            harness.map(String::from)
        );
    }

    /// No run grants a sensitive permission, a lab run included, whatever else is asked with it.
    #[test]
    fn no_run_grants_a_sensitive_permission() {
        for run in [PipeRun::Normal, PipeRun::Lab] {
            assert!(grant(run, &perms(SENSITIVE)).is_empty(), "{run:?}");
            let granted = grant(run, &perms(EVERY));
            assert!(granted.iter().all(|p| !p.sensitive()), "{run:?}");
        }
        // The list above is the whole sensitive set.
        for p in perms(EVERY) {
            let text = String::from(p.clone());
            assert_eq!(p.sensitive(), SENSITIVE.contains(&text.as_str()), "{text}");
        }
    }

    /// A lab run needs the marker as exactly `1` and a settings folder of its own; anything else
    /// is a normal run.
    #[test]
    fn a_lab_run_needs_the_marker_and_its_own_folder() {
        let s = OsStr::new;
        let real = Path::new("C:/Users/someone/AppData/Roaming/Foxglove/config");
        let own = Path::new("C:/Temp/foxglove-lab-x");
        // Off by default: nothing set, or the folder alone (a test run that isn't a lab run).
        assert_eq!(decide(None, None, Some(real)), Decided::Normal);
        assert_eq!(decide(None, Some(own), Some(real)), Decided::Normal);
        for marker in ["", "0", "true", "yes", "11", " 1"] {
            let d = decide(Some(s(marker)), Some(own), Some(real));
            assert_eq!(d, Decided::Normal, "{marker:?}");
        }
        // The marker alone, or on the real folder (or the folder that holds it), is refused.
        for dir in [
            None,
            Some(Path::new("")),
            Some(real),
            Some(Path::new(
                "c:/users/someone/appdata/roaming/foxglove/CONFIG",
            )),
            Some(Path::new("C:/Users/someone/AppData/Roaming/Foxglove")),
        ] {
            let d = decide(Some(s("1")), dir, Some(real));
            assert!(matches!(d, Decided::Refused(_)), "{dir:?}");
            assert_eq!(d.run(), PipeRun::Normal, "{dir:?}");
        }
        assert!(matches!(
            decide(Some(s("1")), Some(own), None),
            Decided::Refused(_)
        ));
        // Both together.
        let d = decide(Some(s("1")), Some(own), Some(real));
        assert_eq!(d, Decided::Lab);
        assert_eq!(d.run(), PipeRun::Lab);
        assert_eq!(Decided::Normal.run(), PipeRun::Normal);
    }

    /// A normal run says nothing; a lab run and a refused marker say one line each, and no line
    /// carries a path.
    #[test]
    fn the_start_line_says_a_lab_run_once_and_names_no_path() {
        assert_eq!(start_line(Decided::Normal), None);
        let lab = start_line(Decided::Lab).unwrap();
        assert!(lab.contains("lab run") && lab.contains("FOXGLOVE_APP_OK=1"));
        let refused = start_line(Decided::Refused("FOXGLOVE_CONFIG_DIR isn't set")).unwrap();
        assert!(refused.contains("isn't a lab run") && refused.contains("normal rule"));
        for line in [lab, refused] {
            assert!(!line.contains('\\') && !line.contains(":/"), "{line}");
            assert_eq!(line.lines().count(), 1);
        }
    }
}
