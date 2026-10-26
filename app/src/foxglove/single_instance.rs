//! One Foxglove per user (#345). The first instance takes a named mutex; a second launch, dev or
//! release build alike, signals the first through a named event to show its settings window and
//! take the focus, then exits.
//!
//! Both names live in the session's `Local\` namespace, so they are per user. A run with
//! `FOXGLOVE_CONFIG_DIR` (tests, the Test lab) gets names of its own for that folder, so it never
//! hands off to, or blocks, the user's own Foxglove.

/// The names for a settings folder: the shared one, or one per `FOXGLOVE_CONFIG_DIR`.
fn names(config_dir: Option<&std::ffi::OsStr>) -> (String, String) {
    let scope = match config_dir {
        None => "user".to_string(),
        Some(dir) => format!(
            "dir-{:016x}",
            fnv1a(dir.to_string_lossy().to_lowercase().as_bytes())
        ),
    };
    (
        format!("Local\\BanditF.Foxglove.{scope}.instance"),
        format!("Local\\BanditF.Foxglove.{scope}.show"),
    )
}

/// FNV-1a: stable across builds and Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows::core::HSTRING;
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    /// The show event, kept for the process's life once this instance is the first.
    struct Show(HANDLE);
    unsafe impl Send for Show {}
    unsafe impl Sync for Show {}
    static SHOW: OnceLock<Show> = OnceLock::new();
    /// The guard itself, kept so a restart after a GPU reset (#346) can hand it to the copy.
    static MUTEX: OnceLock<Show> = OnceLock::new();

    pub fn claim(config_dir: Option<&std::ffi::OsStr>) -> bool {
        let (mutex_name, event_name) = super::names(config_dir);
        // The event first (auto-reset, opened if it exists), so a second launch can always signal
        // it, even while the first instance is still starting: the signal waits for its waiter.
        let Ok(event) = (unsafe { CreateEventW(None, false, false, &HSTRING::from(event_name)) })
        else {
            return true; // no guard rather than no Foxglove
        };
        let mutex = unsafe { CreateMutexW(None, false, &HSTRING::from(mutex_name)) };
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if mutex.is_err() {
            return true;
        }
        if already {
            unsafe {
                // Lets the running instance take the foreground, which Windows otherwise refuses.
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = SetEvent(event);
            }
            eprintln!("Foxglove: already running; showing that one");
            return false;
        }
        // Both handles stay open until the process exits, which releases the guard.
        let _ = SHOW.set(Show(event));
        if let Ok(mutex) = mutex {
            let _ = MUTEX.set(Show(mutex));
        }
        true
    }

    pub fn release() {
        if let Some(m) = MUTEX.get() {
            // SAFETY: the guard's handle, opened in `claim` and closed only here; the process
            // ends right after, so nothing uses it again.
            unsafe {
                let _ = CloseHandle(m.0);
            }
        }
    }

    pub fn on_show(show: impl Fn() + Send + 'static) {
        let Some(event) = SHOW.get().map(|s| s.0 .0 as isize) else {
            return;
        };
        // Blocks on the event, no timer; it ends with the process.
        let _ = std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || loop {
                let handle = HANDLE(event as *mut core::ffi::c_void);
                if unsafe { WaitForSingleObject(handle, INFINITE) } != WAIT_OBJECT_0 {
                    return;
                }
                show();
            });
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn claim(_config_dir: Option<&std::ffi::OsStr>) -> bool {
        true
    }

    pub fn on_show(_show: impl Fn() + Send + 'static) {}

    pub fn release() {}
}

/// Takes the guard at startup. False when another Foxglove already holds it; that one has been
/// asked to show itself, and this process should exit 0.
pub fn claim() -> bool {
    imp::claim(std::env::var_os("FOXGLOVE_CONFIG_DIR").as_deref())
}

/// Lets go of the guard just before this process hands over to a fresh copy (#346).
pub fn release() {
    imp::release()
}

/// Calls `show` whenever a second launch asks this instance to show itself.
pub fn on_show(show: impl Fn() + Send + 'static) {
    imp::on_show(show)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_and_release_share_names_and_a_config_dir_gets_its_own() {
        let (mutex, event) = names(None);
        assert_eq!(mutex, "Local\\BanditF.Foxglove.user.instance");
        assert_eq!(event, "Local\\BanditF.Foxglove.user.show");
        let a = names(Some("C:\\Temp\\lab".as_ref()));
        assert_eq!(
            a,
            names(Some("c:\\temp\\LAB".as_ref())),
            "case doesn't matter on Windows"
        );
        assert_ne!(a, names(Some("C:\\Temp\\other".as_ref())));
        assert_ne!(a.0, mutex);
    }

    #[cfg(windows)]
    #[test]
    fn a_second_claim_signals_the_first() {
        use std::sync::mpsc;
        // The folder is passed in, not set in the environment other tests change.
        let dir = std::env::temp_dir().join(format!("foxglove-single-{}", std::process::id()));
        assert!(imp::claim(Some(dir.as_os_str())), "the first claim wins");
        let (tx, rx) = mpsc::channel();
        on_show(move || {
            let _ = tx.send(());
        });
        assert!(
            !imp::claim(Some(dir.as_os_str())),
            "a second claim for the same folder loses"
        );
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("the first instance is asked to show");
    }
}
