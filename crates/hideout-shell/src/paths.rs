// SPDX-License-Identifier: MIT
//! Where the hub keeps its files (contract §2.3).
//!
//! `HIDEOUT_CONFIG_DIR`, when set, is the whole hub folder: a lab or test hub runs on its own
//! folder and gets its own single-instance names (see `instance_suffix`). Otherwise
//! `HIDEOUT_HOME` (the contract's test override), otherwise `%LOCALAPPDATA%\Hideout`.

use std::path::{Path, PathBuf};

/// The hub's folders, all under one root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubPaths {
    pub root: PathBuf,
    /// True when the root came from `HIDEOUT_CONFIG_DIR` or `HIDEOUT_HOME`, not the default.
    pub overridden: bool,
}

impl HubPaths {
    /// Resolves the root from an environment lookup (`std::env::var_os` in the hub, a map in tests).
    pub fn resolve(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<Self> {
        let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(root) = set("HIDEOUT_CONFIG_DIR").or_else(|| set("HIDEOUT_HOME")) {
            return Some(Self {
                root,
                overridden: true,
            });
        }
        set("LOCALAPPDATA").map(|base| Self {
            root: base.join("Hideout"),
            overridden: false,
        })
    }

    /// From the process environment.
    pub fn from_env() -> Option<Self> {
        Self::resolve(|k| std::env::var_os(k))
    }

    /// The hub's own preferences (§7.3).
    pub fn prefs_file(&self) -> PathBuf {
        self.root.join("hideout.toml")
    }

    /// The open tabs, their order, the active one and pop-outs (§5.9), owned by the hub.
    pub fn tabs_file(&self) -> PathBuf {
        self.root.join("tabs.toml")
    }

    /// Hub and module logs (§2.3).
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Installed modules (§2.3); the registry (H2.2) reads it.
    pub fn modules_dir(&self) -> PathBuf {
        self.root.join("modules")
    }

    /// A suffix for the hub's named kernel objects. Empty for the default folder; for any other
    /// folder a stable hash of its normalised path, so a lab hub never meets the user's hub.
    pub fn instance_suffix(&self) -> String {
        if self.overridden {
            format!(".{:016x}", fnv1a(&normalise(&self.root)))
        } else {
            String::new()
        }
    }
}

/// Lower case, forward slashes, no trailing slash: one spelling per folder on Windows.
fn normalise(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/").to_lowercase();
    s.trim_end_matches('/').to_string()
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn config_dir_wins_then_home_then_local_app_data() {
        let all = [
            ("HIDEOUT_CONFIG_DIR", r"C:\lab\cfg"),
            ("HIDEOUT_HOME", r"C:\home"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ];
        assert_eq!(
            HubPaths::resolve(env(&all)).unwrap().root,
            PathBuf::from(r"C:\lab\cfg")
        );
        assert_eq!(
            HubPaths::resolve(env(&all[1..])).unwrap().root,
            PathBuf::from(r"C:\home")
        );
        let d = HubPaths::resolve(env(&all[2..])).unwrap();
        assert_eq!(
            d.root,
            PathBuf::from(r"C:\Users\u\AppData\Local").join("Hideout")
        );
        assert!(!d.overridden);
        assert!(HubPaths::resolve(env(&[])).is_none());
    }

    #[test]
    fn an_empty_override_is_ignored() {
        let p = HubPaths::resolve(env(&[
            ("HIDEOUT_CONFIG_DIR", ""),
            ("LOCALAPPDATA", r"C:\L"),
        ]))
        .unwrap();
        assert!(!p.overridden);
    }

    #[test]
    fn files_live_under_the_root() {
        let p = HubPaths {
            root: PathBuf::from("r"),
            overridden: true,
        };
        assert_eq!(p.prefs_file(), Path::new("r").join("hideout.toml"));
        assert_eq!(p.logs_dir(), Path::new("r").join("logs"));
        assert_eq!(p.modules_dir(), Path::new("r").join("modules"));
    }

    #[test]
    fn instance_suffix_is_empty_for_the_default_and_stable_per_folder() {
        let def = HubPaths {
            root: PathBuf::from(r"C:\L\Hideout"),
            overridden: false,
        };
        assert_eq!(def.instance_suffix(), "");
        let a = HubPaths {
            root: PathBuf::from(r"C:\Lab\Cfg\"),
            overridden: true,
        };
        let b = HubPaths {
            root: PathBuf::from("c:/lab/cfg"),
            overridden: true,
        };
        let c = HubPaths {
            root: PathBuf::from(r"C:\Lab\Other"),
            overridden: true,
        };
        assert_eq!(a.instance_suffix(), b.instance_suffix());
        assert_ne!(a.instance_suffix(), c.instance_suffix());
        assert_eq!(a.instance_suffix().len(), 17);
    }
}
