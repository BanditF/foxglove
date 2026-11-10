// SPDX-License-Identifier: MIT
//! Notices modules leaving and joining the inventory (contract §7.1: "it notices changes by a
//! file-change notification, never by polling"; #1012.4, #1054). One thread waits on Windows
//! change notifications for the hub folder (file and folder names: `inventory.json` is written
//! by a temp file and a rename) and, in lab mode, for `lab-modules\` (folder names). On each one
//! it reads the installed ids (`hideout_registry::installed`, nothing hashed) and reports what
//! changed.
//!
//! `lab-modules\` is armed when it exists at the start, and again on every change in the hub
//! folder (a `lab-modules\` made, removed or made again after the start is a folder-name change
//! there), so a lab folder made later is watched from then on (#1054.3).
//!
//! Waits: start = the hub's start, stop = the process's end; each wait ends on a change in one
//! of the folders, with no timeout (nothing is due without a change). A failed read is skipped,
//! never taken as a removal.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hideout_registry::installed::{installed_ids_in, removed};

/// One readable change in the watched folders: the ids removed and the ids added since the
/// last one (both may be empty: a module's version, `hideout.toml` or another file changed).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

/// What the watch reports, on every readable change.
pub type OnChange = Box<dyn Fn(&Change) + Send>;

/// Starts the watch on `home`, in lab mode when the environment asks for it (the registry's
/// rules). The ids installed now are the first "before".
pub fn start(home: &Path, on_change: OnChange) -> std::io::Result<()> {
    let lab = hideout_registry::lab::lab_mode_from_env(home).on();
    start_in(home, lab, on_change)
}

/// [`start`] with lab mode given (tests, where the environment is shared).
pub fn start_in(home: &Path, lab: bool, on_change: OnChange) -> std::io::Result<()> {
    let mut watch = win::Watch::new(home)?;
    let lab_dir = lab.then(|| hideout_registry::lab::LabMode::modules_dir(home));
    if let Some(d) = &lab_dir {
        watch.set_extra(d.is_dir().then_some(d.as_path()));
    }
    let home = home.to_path_buf();
    let before = installed_ids_in(&home, lab).unwrap_or_default();
    std::thread::Builder::new()
        .name("hideout-inventory-watch".into())
        .spawn(move || run(watch, home, lab_dir, before, on_change))?;
    Ok(())
}

fn run(
    mut watch: win::Watch,
    home: PathBuf,
    lab_dir: Option<PathBuf>,
    mut before: BTreeSet<String>,
    on_change: OnChange,
) {
    loop {
        let woke = match watch.wait() {
            Ok(w) => w,
            Err(e) => {
                log::warn!(
                    "inventory watch stopped: {e}; installs and removals show at the next start"
                );
                return;
            }
        };
        // Re-armed before the read, so a module folder made meanwhile is read now.
        if let Some(d) = &lab_dir {
            match woke {
                win::Woke::Home => {
                    let was = watch.has_extra();
                    watch.set_extra(d.is_dir().then_some(d.as_path()));
                    if !was && watch.has_extra() {
                        log::info!("inventory watch: watching {}", d.display());
                    }
                }
                win::Woke::Extra if !d.is_dir() => watch.set_extra(None),
                win::Woke::Extra | win::Woke::ExtraLost => {}
            }
        }
        let Some(now) = installed_ids_in(&home, lab_dir.is_some()) else {
            log::info!("inventory watch: the installed modules couldn't be read; next change");
            continue;
        };
        let change = Change {
            removed: removed(&before, &now),
            added: now.difference(&before).cloned().collect(),
        };
        before = now;
        if !change.removed.is_empty() {
            log::info!("inventory watch: removed {}", change.removed.join(", "));
        }
        if !change.added.is_empty() {
            log::info!("inventory watch: installed {}", change.added.join(", "));
        }
        on_change(&change);
    }
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    };
    use windows_sys::Win32::System::Threading::{WaitForMultipleObjects, INFINITE};

    /// Which notification ended a wait.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Woke {
        /// The hub folder.
        Home,
        /// The extra folder (`lab-modules\`), re-armed.
        Extra,
        /// The extra folder's notification couldn't be re-armed (the folder went): dropped.
        ExtraLost,
    }

    /// Change notifications on the hub folder and at most one extra folder (not their
    /// subfolders).
    pub struct Watch {
        home: (HANDLE, PathBuf),
        extra: Option<(HANDLE, PathBuf)>,
    }

    // The handles are used by the watch thread only, after it is moved there.
    unsafe impl Send for Watch {}

    impl Watch {
        pub fn new(home: &Path) -> std::io::Result<Watch> {
            Ok(Watch {
                home: (first(home)?, home.to_path_buf()),
                extra: None,
            })
        }

        /// Watches `dir` as the extra folder, freshly armed, in place of the last one; `None`
        /// stops watching it. A folder that can't be watched is logged and left out.
        pub fn set_extra(&mut self, dir: Option<&Path>) {
            if let Some((h, _)) = self.extra.take() {
                unsafe { FindCloseChangeNotification(h) };
            }
            let Some(dir) = dir else {
                return;
            };
            match first(dir) {
                Ok(h) => self.extra = Some((h, dir.to_path_buf())),
                Err(e) => log::warn!("inventory watch: {e}; not watched until it changes again"),
            }
        }

        pub fn has_extra(&self) -> bool {
            self.extra.is_some()
        }

        /// Blocks until a file or folder name changes in one of the folders, then re-arms that
        /// folder's notification (before the caller reads, so no change is missed). Only the
        /// hub folder's failure ends the watch.
        pub fn wait(&mut self) -> std::io::Result<Woke> {
            let mut handles = vec![self.home.0];
            handles.extend(self.extra.as_ref().map(|e| e.0));
            let n = handles.len() as u32;
            let r = unsafe { WaitForMultipleObjects(n, handles.as_ptr(), 0, INFINITE) };
            let i = r.wrapping_sub(WAIT_OBJECT_0) as usize;
            if i >= handles.len() {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { FindNextChangeNotification(handles[i]) } != 0 {
                return Ok(if i == 0 { Woke::Home } else { Woke::Extra });
            }
            let e = std::io::Error::last_os_error();
            if i == 0 {
                return Err(std::io::Error::other(format!(
                    "{}: {e}",
                    self.home.1.display()
                )));
            }
            if let Some((_, d)) = &self.extra {
                log::info!("inventory watch: {} not watched now: {e}", d.display());
            }
            self.set_extra(None);
            Ok(Woke::ExtraLost)
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            self.set_extra(None);
            unsafe { FindCloseChangeNotification(self.home.0) };
        }
    }

    fn first(dir: &Path) -> std::io::Result<HANDLE> {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME;
        let h = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, filter) };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            let e = std::io::Error::last_os_error();
            return Err(std::io::Error::other(format!("{}: {e}", dir.display())));
        }
        Ok(h)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn inventory(ids: &[&str]) -> String {
        let modules: Vec<String> = ids
            .iter()
            .map(|id| {
                format!(
                    r#""{id}": {{"version": "1.0.0", "kind": "service", "family": "foxglove",
                       "visibility": "public", "source": "s", "channel": "stable",
                       "installed": "2026-11-10T00:00:00Z", "files": {{}}}}"#
                )
            })
            .collect();
        format!(
            r#"{{"format": "hideout-inventory-v1", "modules": {{{}}}}}"#,
            modules.join(",")
        )
    }

    /// Written as `hideout-core` does: a temp file, then a rename over the old one.
    fn save(dir: &Path, ids: &[&str]) {
        let tmp = dir.join("inventory.json.tmp");
        std::fs::write(&tmp, inventory(ids)).unwrap();
        std::fs::rename(&tmp, dir.join("inventory.json")).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hideout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The reports with ids in them, in order, until `want` is seen (10 s fallback so a broken
    /// watch fails rather than hangs).
    fn until(rx: &mpsc::Receiver<Change>, want: &Change) -> Vec<Change> {
        let mut seen = Vec::new();
        while seen.last() != Some(want) {
            let c = rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("no {want:?}; saw {seen:?}"));
            if c != Change::default() {
                seen.push(c);
            }
        }
        seen
    }

    fn change(removed: &[&str], added: &[&str]) -> Change {
        Change {
            removed: removed.iter().map(|s| s.to_string()).collect(),
            added: added.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Window-free: a real change notification on a temp folder. An install reports its id
    /// as added; a removal reports its id once; a broken write reports nothing.
    #[test]
    fn installs_and_removals_are_reported() {
        let dir = temp("invwatch");
        save(&dir, &["a.art", "c.chat"]);
        let (tx, rx) = mpsc::channel();
        start_in(
            &dir,
            false,
            Box::new(move |c| {
                let _ = tx.send(c.clone());
            }),
        )
        .unwrap();
        save(&dir, &["a.art", "c.chat", "d.new"]);
        assert_eq!(
            until(&rx, &change(&[], &["d.new"])),
            [change(&[], &["d.new"])]
        );
        std::fs::write(dir.join("inventory.json.tmp"), "{not json").unwrap();
        std::fs::rename(dir.join("inventory.json.tmp"), dir.join("broken.json")).unwrap();
        save(&dir, &["a.art", "d.new"]);
        assert_eq!(
            until(&rx, &change(&["c.chat"], &[])),
            [change(&["c.chat"], &[])]
        );
        let more: Vec<_> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(300)).ok())
            .filter(|c| *c != Change::default())
            .collect();
        assert!(more.is_empty(), "reported twice: {more:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #1054.3, window-free: in lab mode, a `lab-modules\` made after the start is watched from
    /// then on (a module folder made in it later is reported), and again after it is removed
    /// and made anew.
    #[test]
    fn a_lab_folder_made_after_the_start_is_watched() {
        let dir = temp("invwatch-lab");
        let (tx, rx) = mpsc::channel();
        start_in(
            &dir,
            true,
            Box::new(move |c| {
                let _ = tx.send(c.clone());
            }),
        )
        .unwrap();
        let lab = dir.join(hideout_registry::lab::LAB_MODULES_DIR);
        // The hub folder's change arms `lab-modules\`; its report comes after the arming.
        std::fs::create_dir(&lab).unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("making lab-modules wasn't noticed");
        // Only `lab-modules\`'s own notification can see this one.
        std::fs::create_dir(lab.join("m.lab")).unwrap();
        until(&rx, &change(&[], &["m.lab"]));
        std::fs::remove_dir(lab.join("m.lab")).unwrap();
        until(&rx, &change(&["m.lab"], &[]));
        // Removed and made again: watched anew.
        std::fs::remove_dir(&lab).unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("removing lab-modules wasn't noticed");
        std::fs::create_dir(&lab).unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("making lab-modules again wasn't noticed");
        std::fs::create_dir(lab.join("m.two")).unwrap();
        until(&rx, &change(&[], &["m.two"]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
