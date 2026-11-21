// SPDX-License-Identifier: MIT
//! R3 of the 0-click rebuild plan: starts a rebuild watch ([`crate::rebuild_watch`]) for each
//! app enrolled under a dev folder and drops it when the app stops qualifying. A settled new
//! program goes to [`Supervisor::rebuilt`], whose gate decides; nothing here grants anything.
//!
//! The watch target's manifest is the program file itself: the hub's own manifest copy is
//! hub-controlled, so a change to it is never a rebuild signal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hideout_registry::Registry;
use hideout_supervisor::driver::{program_of, Supervisor};

use crate::rebuild_watch::{self as watch, Outcome};

/// The running watches, by app id. Dropping one stops its thread. `H` is the watch handle
/// (another type only in tests).
pub struct Watches<H = watch::Handle> {
    by_id: HashMap<String, (PathBuf, H)>,
}

impl<H> Default for Watches<H> {
    fn default() -> Self {
        Watches {
            by_id: HashMap::new(),
        }
    }
}

/// The program each enrolled app is watched at; empty while dev folders are off.
fn wanted(reg: &Registry) -> HashMap<String, PathBuf> {
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
    want
}

impl Watches {
    /// Brings the watches in line with the registry: one per enrolled app, none for the rest.
    /// Cheap to repeat; a watch whose program path is unchanged is kept.
    pub fn sync(&mut self, reg: &Registry, sup: &Supervisor) {
        self.reconcile(wanted(reg), |id, exe| {
            let target = watch::Target {
                id: id.to_string(),
                exe: exe.to_path_buf(),
                manifest: exe.to_path_buf(),
            };
            let (s, name) = (sup.clone(), id.to_string());
            watch::spawn(
                target,
                Box::new(move |o| match o {
                    Outcome::Settled { exe_sha256, .. } => s.rebuilt(&name, &exe_sha256),
                    Outcome::GaveUp(why) => {
                        log::info!("rebuild {name}: not taken up: {why:?}");
                    }
                }),
            )
        });
    }
}

impl<H> Watches<H> {
    /// Keeps the watches whose app is still wanted at the same program, drops the rest (before
    /// any start, so a moved program never has two watches), and starts the missing ones. A
    /// start that fails is not recorded: the next call tries it again.
    fn reconcile(
        &mut self,
        want: HashMap<String, PathBuf>,
        mut start: impl FnMut(&str, &Path) -> std::io::Result<H>,
    ) {
        self.by_id.retain(|id, (exe, _)| want.get(id) == Some(exe));
        for (id, exe) in want {
            if self.by_id.contains_key(&id) {
                continue;
            }
            match start(&id, &exe) {
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

    /// A watch handle that says when it is dropped (a real one stops its thread then).
    struct Fake(String, Log);
    type Log = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    impl Drop for Fake {
        fn drop(&mut self) {
            self.1.borrow_mut().push(format!("stop {}", self.0));
        }
    }

    /// Reconciles with a starter that records each start as "start <id> <exe>" and refuses the
    /// ids in `refuse`.
    fn run(w: &mut Watches<Fake>, log: &Log, list: &[(&str, &str)], refuse: &[&str]) {
        let want = list
            .iter()
            .map(|(id, exe)| (id.to_string(), PathBuf::from(exe)))
            .collect();
        w.reconcile(want, |id, exe| {
            if refuse.contains(&id) {
                return Err(std::io::Error::other("no handle"));
            }
            let tag = format!("{id} {}", exe.display());
            log.borrow_mut().push(format!("start {tag}"));
            Ok(Fake(tag, log.clone()))
        });
    }

    /// What happened since the last look, sorted (apps are visited in no fixed order).
    fn taken(log: &Log) -> Vec<String> {
        let mut v = std::mem::take(&mut *log.borrow_mut());
        v.sort();
        v
    }

    /// Repeating the same list starts nothing and stops nothing: a running watch (and the quiet
    /// time it may be counting) is never restarted by an unrelated inventory change.
    #[test]
    fn an_unchanged_app_keeps_its_watch() {
        let log = Log::default();
        let mut w = Watches::<Fake>::default();
        run(&mut w, &log, &[("a", "x/a.exe"), ("b", "x/b.exe")], &[]);
        assert_eq!(taken(&log), ["start a x/a.exe", "start b x/b.exe"]);
        run(&mut w, &log, &[("a", "x/a.exe"), ("b", "x/b.exe")], &[]);
        assert!(taken(&log).is_empty());
    }

    /// An app that stops qualifying (removed, moved out of a dev folder, dev folders turned
    /// off) loses its watch at once, and only its own.
    #[test]
    fn an_app_that_no_longer_qualifies_loses_its_watch() {
        let log = Log::default();
        let mut w = Watches::<Fake>::default();
        run(&mut w, &log, &[("a", "x/a.exe"), ("b", "x/b.exe")], &[]);
        taken(&log);
        run(&mut w, &log, &[("b", "x/b.exe")], &[]);
        assert_eq!(taken(&log), ["stop a x/a.exe"]);
        // Dev folders off: nothing is wanted.
        run(&mut w, &log, &[], &[]);
        assert_eq!(taken(&log), ["stop b x/b.exe"]);
        assert!(w.by_id.is_empty());
    }

    /// A program that moved: the old path's watch is stopped before the new one starts, so the
    /// old file can never report a rebuild for the app again.
    #[test]
    fn a_moved_program_swaps_its_watch_old_one_first() {
        let log = Log::default();
        let mut w = Watches::<Fake>::default();
        run(&mut w, &log, &[("a", "old/a.exe")], &[]);
        taken(&log);
        run(&mut w, &log, &[("a", "new/a.exe")], &[]);
        assert_eq!(*log.borrow(), ["stop a old/a.exe", "start a new/a.exe"]);
        assert_eq!(w.by_id["a"].0, PathBuf::from("new/a.exe"));
    }

    /// A watch that could not start is not remembered as running: the next sync tries again,
    /// and the failure of one app does not cost another its watch.
    #[test]
    fn a_failed_start_is_tried_again_and_spares_the_others() {
        let log = Log::default();
        let mut w = Watches::<Fake>::default();
        run(&mut w, &log, &[("a", "x/a.exe"), ("b", "x/b.exe")], &["a"]);
        assert_eq!(taken(&log), ["start b x/b.exe"]);
        assert!(!w.by_id.contains_key("a"));
        run(&mut w, &log, &[("a", "x/a.exe"), ("b", "x/b.exe")], &[]);
        assert_eq!(taken(&log), ["start a x/a.exe"]);
        // A moved program whose new watch fails leaves no watch on the old path either.
        run(&mut w, &log, &[("a", "y/a.exe"), ("b", "x/b.exe")], &["a"]);
        assert_eq!(taken(&log), ["stop a x/a.exe"]);
        assert!(!w.by_id.contains_key("a"));
    }

    /// Dropping the set stops every watch (the hub's exit).
    #[test]
    fn dropping_the_set_stops_every_watch() {
        let log = Log::default();
        let mut w = Watches::<Fake>::default();
        run(&mut w, &log, &[("a", "x/a.exe"), ("b", "x/b.exe")], &[]);
        taken(&log);
        drop(w);
        assert_eq!(taken(&log), ["stop a x/a.exe", "stop b x/b.exe"]);
    }
}
