// SPDX-License-Identifier: MIT
//! Notices a module leaving the inventory (contract §7.1: "it notices changes by a file-change
//! notification, never by polling"; #1012.4). One thread waits on Windows change notifications
//! for the hub folder (file names: `inventory.json` is written by a temp file and a rename) and,
//! in lab mode, for `lab-modules\` (folder names). On each one it reads the installed ids
//! (`hideout_registry::installed`, nothing hashed) and reports the ids that are gone.
//!
//! Waits: start = the hub's start, stop = the process's end; each wait ends on a change in one
//! of the folders, with no timeout (nothing is due without a change). A failed read is skipped,
//! never taken as a removal. A `lab-modules\` folder made after the start isn't watched.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hideout_registry::installed::{installed_ids, removed};

/// What the watch reports: the ids removed since the last change, never empty.
pub type OnRemoved = Box<dyn Fn(Vec<String>) + Send>;

/// Starts the watch on `home`. The ids installed now are the first "before".
pub fn start(home: &Path, on_removed: OnRemoved) -> std::io::Result<()> {
    let mut dirs = vec![home.to_path_buf()];
    if hideout_registry::lab::lab_mode_from_env(home).on() {
        let lab = hideout_registry::lab::LabMode::modules_dir(home);
        if lab.is_dir() {
            dirs.push(lab);
        }
    }
    let watch = win::Watch::new(&dirs)?;
    let home = home.to_path_buf();
    let before = installed_ids(&home).unwrap_or_default();
    std::thread::Builder::new()
        .name("hideout-inventory-watch".into())
        .spawn(move || run(watch, home, before, on_removed))?;
    Ok(())
}

fn run(watch: win::Watch, home: PathBuf, mut before: BTreeSet<String>, on_removed: OnRemoved) {
    loop {
        if let Err(e) = watch.wait() {
            log::warn!("inventory watch stopped: {e}; a removed module's windows stay open");
            return;
        }
        let Some(now) = installed_ids(&home) else {
            log::info!("inventory watch: the installed modules couldn't be read; next change");
            continue;
        };
        let gone = removed(&before, &now);
        before = now;
        if !gone.is_empty() {
            log::info!("inventory watch: removed {}", gone.join(", "));
            on_removed(gone);
        }
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

    /// Change notifications on a few folders (not their subfolders).
    pub struct Watch {
        handles: Vec<HANDLE>,
        dirs: Vec<PathBuf>,
    }

    // The handles are used by the watch thread only, after it is moved there.
    unsafe impl Send for Watch {}

    impl Watch {
        pub fn new(dirs: &[PathBuf]) -> std::io::Result<Watch> {
            let mut w = Watch {
                handles: Vec::new(),
                dirs: dirs.to_vec(),
            };
            for d in dirs {
                w.handles.push(first(d)?);
            }
            Ok(w)
        }

        /// Blocks until a file or folder name changes in one of the folders, then re-arms that
        /// folder's notification (before the caller reads, so no change is missed).
        pub fn wait(&self) -> std::io::Result<()> {
            let n = self.handles.len() as u32;
            let r = unsafe { WaitForMultipleObjects(n, self.handles.as_ptr(), 0, INFINITE) };
            let i = r.wrapping_sub(WAIT_OBJECT_0) as usize;
            if i >= self.handles.len() {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { FindNextChangeNotification(self.handles[i]) } == 0 {
                return Err(std::io::Error::other(format!(
                    "{}: {}",
                    self.dirs[i].display(),
                    std::io::Error::last_os_error()
                )));
            }
            Ok(())
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            for &h in &self.handles {
                unsafe { FindCloseChangeNotification(h) };
            }
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

    /// Window-free: a real change notification on a temp folder. An install reports nothing;
    /// a removal reports its id once; a broken write reports nothing. The 10 s wait is only the
    /// fallback for a test that would otherwise hang.
    #[test]
    fn a_module_leaving_the_inventory_is_reported() {
        let dir = std::env::temp_dir().join(format!("hideout-invwatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        save(&dir, &["a.art", "c.chat"]);
        let (tx, rx) = mpsc::channel();
        start(
            &dir,
            Box::new(move |ids| {
                let _ = tx.send(ids);
            }),
        )
        .unwrap();
        save(&dir, &["a.art", "c.chat", "d.new"]);
        std::fs::write(dir.join("inventory.json.tmp"), "{not json").unwrap();
        std::fs::rename(dir.join("inventory.json.tmp"), dir.join("broken.json")).unwrap();
        save(&dir, &["a.art", "d.new"]);
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("no removal reported");
        assert_eq!(got, ["c.chat"]);
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "reported twice"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
