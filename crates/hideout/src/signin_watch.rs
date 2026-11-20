// SPDX-License-Identifier: MIT
//! Keeps the router's `module.<id>.starts_at_sign_in` fact true when Start with Windows changes
//! outside the Settings page (Windows Settings > Apps > Startup flips the StartupApproved value;
//! someone deletes `Hideout.lnk`). One thread waits on two Windows change notifications, the
//! Startup folder and the StartupApproved registry key, and re-reads the switch on each wake.
//! No timer, no polling: with no change the thread sleeps for good. It only reads; it turns
//! nothing on and writes nothing.

/// Re-reads the switch on a wake and announces only a changed answer. A read that failed
/// (`None`) is skipped, never taken as "off".
pub struct Refresh<R, A> {
    read: R,
    announce: A,
    last: Option<bool>,
}

impl<R: FnMut() -> Option<bool>, A: FnMut(bool)> Refresh<R, A> {
    /// `known`: what was announced already (the switch's first read).
    pub fn new(read: R, announce: A, known: Option<bool>) -> Self {
        Self {
            read,
            announce,
            last: known,
        }
    }

    /// One wake. Returns whether an announcement went out.
    pub fn wake(&mut self) -> bool {
        match (self.read)() {
            Some(on) if self.last != Some(on) => {
                self.last = Some(on);
                (self.announce)(on);
                true
            }
            _ => false,
        }
    }
}

#[cfg(windows)]
pub use win::start;

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_ATTRIBUTES,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY,
        REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};

    struct Armed {
        folder: HANDLE,
        /// The registry key and the event its change signals; `None` when the key is not there
        /// (nothing was ever disabled in Windows Settings): the folder alone is watched.
        reg: Option<(HKEY, HANDLE)>,
    }

    // SAFETY: kernel handles, used by one thread after the hand-over.
    unsafe impl Send for Armed {}

    /// Arms the two notifications now (so a change after this call is never missed), then waits
    /// for them on a thread that lives as long as the process. `on_wake` runs after each one.
    /// `key` is under HKCU, watched with its subkeys.
    pub fn start(
        dir: &Path,
        key: &str,
        mut on_wake: Box<dyn FnMut() + Send>,
    ) -> std::io::Result<()> {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_ATTRIBUTES;
        // SAFETY: valid NUL-terminated path.
        let folder = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, filter) };
        if folder == INVALID_HANDLE_VALUE || folder.is_null() {
            return Err(std::io::Error::other(format!(
                "{}: {}",
                dir.display(),
                std::io::Error::last_os_error()
            )));
        }
        let reg = arm_key(key);
        let key = key.to_string();
        if reg.is_none() {
            log::info!("sign-in watch: the registry key is not there; the Startup folder only");
        }
        let armed = Armed { folder, reg };
        std::thread::Builder::new()
            .name("signin-watch".into())
            .spawn(move || {
                let mut armed = armed;
                loop {
                    let mut hs = vec![armed.folder];
                    if let Some((_, ev)) = armed.reg {
                        hs.push(ev);
                    }
                    // SAFETY: live handles owned by this thread for the process's life.
                    let r = unsafe {
                        WaitForMultipleObjects(hs.len() as u32, hs.as_ptr(), 0, INFINITE)
                    };
                    let i = r.wrapping_sub(WAIT_OBJECT_0) as usize;
                    if i >= hs.len() {
                        log::warn!("sign-in watch: wait failed ({r}); stopped");
                        return;
                    }
                    // SAFETY: the same live handles.
                    unsafe {
                        if i == 0 {
                            FindNextChangeNotification(armed.folder);
                        } else if let Some((k, ev)) = armed.reg {
                            RegNotifyChangeKeyValue(
                                k,
                                1,
                                REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME,
                                ev,
                                1,
                            );
                        }
                    }
                    // The key is made by the first disable in Windows Settings, which may be
                    // after the hub started: look for it again on each wake while it is missing.
                    if armed.reg.is_none() {
                        armed.reg = arm_key(&key);
                        if armed.reg.is_some() {
                            log::info!("sign-in watch: the registry key appeared; watching it");
                        }
                    }
                    on_wake();
                }
            })?;
        Ok(())
    }

    fn arm_key(key: &str) -> Option<(HKEY, HANDLE)> {
        let wide: Vec<u16> = key.encode_utf16().chain([0]).collect();
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated key; the handle is closed on every failure path below.
        unsafe {
            if RegOpenKeyExW(HKEY_CURRENT_USER, wide.as_ptr(), 0, KEY_NOTIFY, &mut h) != 0 {
                return None;
            }
            let ev = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
            if ev.is_null() {
                RegCloseKey(h);
                return None;
            }
            let r = RegNotifyChangeKeyValue(
                h,
                1,
                REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME,
                ev,
                1,
            );
            if r != 0 {
                RegCloseKey(h);
                return None;
            }
            Some((h, ev))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn announces_only_a_changed_answer() {
        let reads = RefCell::new(vec![Some(true), Some(true), Some(false), None, Some(false)]);
        let told = RefCell::new(Vec::new());
        let mut r = Refresh::new(
            || reads.borrow_mut().remove(0),
            |on| told.borrow_mut().push(on),
            Some(true),
        );
        assert!(!r.wake()); // still on
        assert!(!r.wake());
        assert!(r.wake()); // turned off in Windows Settings
        assert!(!r.wake()); // a failed read is not "off" and not a change
        assert!(!r.wake());
        assert_eq!(*told.borrow(), [false]);
    }

    #[test]
    fn an_unknown_start_announces_the_first_answer() {
        let told = RefCell::new(Vec::new());
        let mut r = Refresh::new(|| Some(false), |on| told.borrow_mut().push(on), None);
        assert!(r.wake());
        assert!(!r.wake());
        assert_eq!(*told.borrow(), [false]);
    }

    /// Window-free: a real folder notification on a temp folder wakes the callback; no registry
    /// key is touched (an absent key is the folder-only case).
    #[cfg(windows)]
    #[test]
    fn a_change_in_the_folder_wakes_the_watch() {
        use std::sync::mpsc;
        let dir = std::env::temp_dir().join(format!("hideout-signinwatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        start(
            &dir,
            r"Software\Hideout-no-such-key-for-tests",
            Box::new(move || {
                let _ = tx.send(());
            }),
        )
        .unwrap();
        std::fs::write(dir.join("Hideout.lnk"), b"x").unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("no wake after a file appeared");
        std::fs::remove_file(dir.join("Hideout.lnk")).unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("no wake after a file went");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
