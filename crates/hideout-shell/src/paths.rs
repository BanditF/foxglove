// SPDX-License-Identifier: MIT
//! Where the hub keeps its files (contract §2.3).
//!
//! `HIDEOUT_CONFIG_DIR`, when set, is the whole hub folder: a lab or test hub runs on its own
//! folder and gets its own single-instance names (see `instance_suffix`). Otherwise
//! `HIDEOUT_HOME` (the contract's test override), otherwise `%LOCALAPPDATA%\Hideout`. The
//! installed build (no `lab-switches` feature) ignores both variables: it always uses the last.

use std::ffi::OsString;
use std::path::PathBuf;

/// `env(k)` as a path, none when unset or empty.
fn non_empty(env: &dyn Fn(&str) -> Option<OsString>, k: &str) -> Option<PathBuf> {
    env(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The hub folder the environment names: `HIDEOUT_CONFIG_DIR`, else `HIDEOUT_HOME`.
#[cfg(feature = "lab-switches")]
fn home_override(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    non_empty(env, "HIDEOUT_CONFIG_DIR").or_else(|| non_empty(env, "HIDEOUT_HOME"))
}

/// [`home_override`] in a build without `lab-switches` (the installed Hideout): never one.
#[cfg(not(feature = "lab-switches"))]
fn home_override(_env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    None
}

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
        let default = non_empty(&env, "LOCALAPPDATA").map(|base| base.join("Hideout"));
        if let Some(root) = home_override(&env) {
            // Naming the default folder is the default hub: same pipe and lock names (#1010).
            let overridden = default.as_ref().is_none_or(|d| {
                hideout_router::server::folder_key(d) != hideout_router::server::folder_key(&root)
            });
            return Some(Self { root, overridden });
        }
        default.map(|root| Self {
            root,
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

    /// The suffix of the hub's named kernel objects and pipes (router, alias, capture, lab
    /// control, lock, jobs; contract §8.2, #1010). Empty for the default folder, so its names
    /// are the old ones; for any other folder a stable hash of its canonical path, so a lab hub
    /// never meets the user's hub. See `hideout_router::server::folder_suffix`.
    pub fn instance_suffix(&self) -> String {
        if self.overridden {
            hideout_router::server::folder_suffix(&self.root)
        } else {
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::Path;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[cfg(feature = "lab-switches")]
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

    /// The installed build: both variables are ignored and the real default folder is used
    /// (the test only computes the path, it writes nothing).
    #[cfg(not(feature = "lab-switches"))]
    #[test]
    fn the_installed_build_ignores_the_home_variables() {
        let p = HubPaths::resolve(env(&[
            ("HIDEOUT_CONFIG_DIR", r"C:\lab\cfg"),
            ("HIDEOUT_HOME", r"C:\home"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ]))
        .unwrap();
        assert_eq!(
            p.root,
            PathBuf::from(r"C:\Users\u\AppData\Local").join("Hideout")
        );
        assert!(!p.overridden);
        assert_eq!(p.instance_suffix(), "");
        assert!(HubPaths::resolve(env(&[("HIDEOUT_CONFIG_DIR", r"C:\lab\cfg")])).is_none());
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
    fn an_override_naming_the_default_folder_is_the_default_hub() {
        let p = HubPaths::resolve(env(&[
            ("HIDEOUT_CONFIG_DIR", r"c:/l/HIDEOUT/"),
            ("LOCALAPPDATA", r"C:\L"),
        ]))
        .unwrap();
        assert!(!p.overridden);
        assert_eq!(p.instance_suffix(), "");
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
