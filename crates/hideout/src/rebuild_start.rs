// SPDX-License-Identifier: MIT
//! R3 of the 0-click rebuild plan: starts a rebuild watch ([`crate::rebuild_watch`]) for each
//! app enrolled under a dev folder and drops it when the app stops qualifying. A settled new
//! program goes to [`Supervisor::rebuilt`], whose gate decides; nothing here grants anything.
//!
//! The watch target's manifest is the program file itself: the hub's own manifest copy is
//! hub-controlled, so a change to it is never a rebuild signal.

use std::collections::HashMap;
use std::path::PathBuf;

use hideout_registry::Registry;
use hideout_supervisor::driver::{program_of, Supervisor};

use crate::rebuild_watch::{self as watch, Outcome};

/// The running watches, by app id. Dropping one stops its thread.
#[derive(Default)]
pub struct Watches {
    by_id: HashMap<String, (PathBuf, watch::Handle)>,
}

impl Watches {
    /// Brings the watches in line with the registry: one per enrolled app, none for the rest.
    /// Cheap to repeat; a watch whose program path is unchanged is kept.
    pub fn sync(&mut self, reg: &Registry, sup: &Supervisor) {
        let mut want: HashMap<String, PathBuf> = HashMap::new();
        if reg.dev_folders_on() {
            for id in reg.inventory.modules.keys() {
                if reg.dev_folder_of(id).is_none() {
                    continue;
                }
                let run0 = reg
                    .manifest(id)
                    .and_then(|m| m.run.and_then(|r| r.into_iter().next()));
                let version = reg.inventory.modules.get(id).map(|m| m.version.clone());
                if let (Some(run0), Some(version)) = (run0, version) {
                    // The same file the driver launches: a relative run[0] is the module
                    // folder's, not the hub's working directory's.
                    let exe = program_of(&reg.module_dir(id, &version), &run0);
                    want.insert(id.clone(), exe);
                }
            }
        }
        self.by_id.retain(|id, (exe, _)| want.get(id) == Some(exe));
        for (id, exe) in want {
            if self.by_id.contains_key(&id) {
                continue;
            }
            let target = watch::Target {
                id: id.clone(),
                exe: exe.clone(),
                manifest: exe.clone(),
            };
            let (s, name) = (sup.clone(), id.clone());
            let started = watch::spawn(
                target,
                Box::new(move |o| match o {
                    Outcome::Settled { exe_sha256, .. } => s.rebuilt(&name, &exe_sha256),
                    Outcome::GaveUp(why) => {
                        log::info!("rebuild {name}: not taken up: {why:?}");
                    }
                }),
            );
            match started {
                Ok(h) => {
                    self.by_id.insert(id, (exe, h));
                }
                Err(e) => log::warn!("rebuild {id}: no watch: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relative `run[0]` is watched where the driver launches it, in the module's folder.
    #[test]
    fn a_relative_program_is_watched_in_the_module_folder() {
        let dir = std::path::Path::new("modules").join("app").join("1.0");
        assert_eq!(program_of(&dir, "app.exe"), dir.join("app.exe"));
        let abs = std::env::current_dir().unwrap().join("x").join("app.exe");
        assert_eq!(program_of(&dir, abs.to_str().unwrap()), abs);
    }
}
