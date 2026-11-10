// SPDX-License-Identifier: MIT
//! Lab mode (#1001, docs/hideout-lab.md): a lab hub on its own `HIDEOUT_CONFIG_DIR` loads the
//! modules in `<config>\lab-modules\<id>\` as they ship, first-party ids included, the way a
//! dev link would (contract §10). Nothing about trust changes:
//!
//! - lab modules are never written to `inventory.json` and never touch the trust store (the
//!   root, catalogs, anchors and `seen`);
//! - they hold permissions as a developer-mode sideload does (`Source::Sideload`), never as a
//!   first-party catalog module, so the never-list (§6.6) applies as for any sideload;
//! - the supervisor still checks the pid and exe hash of what it launched at `initialize`;
//! - lab mode is refused on the user's real hub folder.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The environment switch (`hideout.exe --lab` sets it).
pub const LAB_ENV: &str = "HIDEOUT_LAB";
/// The folder under the lab hub's config dir that holds one folder per lab module.
pub const LAB_MODULES_DIR: &str = "lab-modules";
/// The lab's stand-in for a catalog revocation (#1026): `<config>\lab-withdrawn.json`, read
/// in lab mode only, `{"<id>": {"version": "<v>", "reason": "<text>"}}`. A lab module whose
/// loaded version it names reads as withdrawn by its publisher, so the withdrawn card, the
/// tray and the notification can be tried without a catalog. Never read outside lab mode.
pub const LAB_WITHDRAWN_FILE: &str = "lab-withdrawn.json";

/// Whether this hub runs in lab mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabMode {
    /// Not asked for.
    Off,
    /// Asked for and allowed: lab modules load from [`LabMode::modules_dir`].
    On,
    /// Asked for and refused; the hub runs as a normal hub. The reason is for the log.
    Refused(String),
}

impl LabMode {
    pub fn on(&self) -> bool {
        *self == LabMode::On
    }

    /// `<home>\lab-modules`.
    pub fn modules_dir(home: &Path) -> PathBuf {
        home.join(LAB_MODULES_DIR)
    }
}

/// One entry of [`LAB_WITHDRAWN_FILE`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct LabWithdrawn {
    pub version: String,
    #[serde(default)]
    pub reason: String,
}

/// [`LAB_WITHDRAWN_FILE`]'s entries; none when it is missing or unreadable (logged by the
/// caller through `lab_report`).
pub fn load_withdrawn(
    home: &Path,
) -> Result<std::collections::BTreeMap<String, LabWithdrawn>, String> {
    let path = home.join(LAB_WITHDRAWN_FILE);
    match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// A path compared case-insensitively, `/` as `\`, without trailing separators.
fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// `a` is `b` or inside it.
fn at_or_under(a: &Path, b: &Path) -> bool {
    let (a, b) = (norm(a), norm(b));
    !b.is_empty() && (a == b || a.starts_with(&format!("{b}\\")))
}

/// Decides lab mode for the hub folder `home` from an environment lookup. Lab mode needs
/// `HIDEOUT_LAB=1` and `HIDEOUT_CONFIG_DIR` set to `home`, and `home` must not be (or be inside)
/// the user's real hub folders, `%LOCALAPPDATA%\Hideout` and `%APPDATA%\Hideout`.
pub fn lab_mode(home: &Path, env: impl Fn(&str) -> Option<OsString>) -> LabMode {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    if set(LAB_ENV).is_none_or(|v| v != "1") {
        return LabMode::Off;
    }
    let Some(config) = set("HIDEOUT_CONFIG_DIR").map(PathBuf::from) else {
        return LabMode::Refused(
            "lab mode needs HIDEOUT_CONFIG_DIR set to a lab folder, not the user's hub folder"
                .into(),
        );
    };
    if norm(&config) != norm(home) {
        return LabMode::Refused(format!(
            "the hub folder {} is not HIDEOUT_CONFIG_DIR {}",
            home.display(),
            config.display()
        ));
    }
    for base in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(b) = set(base) {
            let real = PathBuf::from(b).join("Hideout");
            if at_or_under(home, &real) {
                return LabMode::Refused(format!(
                    "{} is the user's real hub folder ({})",
                    home.display(),
                    real.display()
                ));
            }
        }
    }
    LabMode::On
}

/// [`lab_mode`] from this process's environment.
pub fn lab_mode_from_env(home: &Path) -> LabMode {
    lab_mode(home, |k| std::env::var_os(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: BTreeMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k| m.get(k).cloned()
    }

    const USER: [(&str, &str); 2] = [
        ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ("APPDATA", r"C:\Users\u\AppData\Roaming"),
    ];

    #[test]
    fn off_unless_asked() {
        let home = Path::new(r"C:\lab\hub");
        assert_eq!(lab_mode(home, env(&USER)), LabMode::Off);
        let e = env(&[("HIDEOUT_LAB", "0"), ("HIDEOUT_CONFIG_DIR", r"C:\lab\hub")]);
        assert_eq!(lab_mode(home, e), LabMode::Off);
    }

    #[test]
    fn on_for_a_lab_config_dir() {
        let mut p = USER.to_vec();
        p.extend([("HIDEOUT_LAB", "1"), ("HIDEOUT_CONFIG_DIR", r"C:/Lab/Hub/")]);
        assert_eq!(lab_mode(Path::new(r"c:\lab\hub"), env(&p)), LabMode::On);
    }

    #[test]
    fn refused_without_config_dir_or_on_the_real_folder() {
        let mut p = USER.to_vec();
        p.push(("HIDEOUT_LAB", "1"));
        let real = Path::new(r"C:\Users\u\AppData\Local\Hideout");
        assert!(matches!(
            lab_mode(real, env(&p)),
            LabMode::Refused(r) if r.contains("HIDEOUT_CONFIG_DIR")
        ));
        // HIDEOUT_HOME alone is not a lab folder.
        let mut h = p.clone();
        h.push(("HIDEOUT_HOME", r"C:\lab\hub"));
        assert!(matches!(
            lab_mode(Path::new(r"C:\lab\hub"), env(&h)),
            LabMode::Refused(_)
        ));
        for dir in [
            r"C:\Users\u\AppData\Local\Hideout",
            r"C:\Users\u\AppData\Local\hideout\",
            r"C:\Users\u\AppData\Local\Hideout\lab",
            r"C:\Users\u\AppData\Roaming\Hideout",
        ] {
            let mut c = p.clone();
            c.push(("HIDEOUT_CONFIG_DIR", dir));
            assert!(
                matches!(lab_mode(Path::new(dir), env(&c)), LabMode::Refused(r) if r.contains("real hub folder")),
                "{dir}"
            );
        }
        // A sibling whose name only starts the same is fine.
        let mut c = p.clone();
        c.push((
            "HIDEOUT_CONFIG_DIR",
            r"C:\Users\u\AppData\Local\Hideout-lab",
        ));
        assert_eq!(
            lab_mode(Path::new(r"C:\Users\u\AppData\Local\Hideout-lab"), env(&c)),
            LabMode::On
        );
    }

    #[test]
    fn refused_when_home_is_not_the_config_dir() {
        let mut p = USER.to_vec();
        p.extend([("HIDEOUT_LAB", "1"), ("HIDEOUT_CONFIG_DIR", r"C:\lab\a")]);
        assert!(matches!(
            lab_mode(Path::new(r"C:\lab\b"), env(&p)),
            LabMode::Refused(_)
        ));
    }
}
