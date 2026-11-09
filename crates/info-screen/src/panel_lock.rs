//! Only one app may drive the Info screen: named mutexes shared with Foxglove, so the old and the
//! new app never send frames at the same time. Windows mutexes belong to a thread, so take and give
//! it back on the same one (the worker).

/// The original mutex name; hostpanel and older builds only know this one.
pub const NAME: &str = r"Local\Foxglove.Panel";
/// The Hideout name (H1.4, #922). The lock takes both, so old and new apps still exclude each other.
pub const INFO_SCREEN_NAME: &str = r"Local\Hideout.InfoScreen";

#[cfg(windows)]
mod imp {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    pub struct PanelLock {
        handles: Vec<HANDLE>,
        held: bool,
    }

    impl PanelLock {
        pub fn new() -> Self {
            Self::named_all(&[super::NAME, super::INFO_SCREEN_NAME])
        }

        pub fn named(name: &str) -> Self {
            Self::named_all(&[name])
        }

        fn named_all(names: &[&str]) -> Self {
            let handles = names
                .iter()
                .filter_map(|n| unsafe { CreateMutexW(None, false, &HSTRING::from(*n)) }.ok())
                .collect();
            PanelLock {
                handles,
                held: false,
            }
        }

        /// Takes the lock if nobody holds it (or its holder died); true while we hold it.
        pub fn acquire(&mut self) -> bool {
            if self.held {
                return true;
            }
            if self.handles.is_empty() {
                return true; // no mutex: behave as before rather than never drawing
            }
            let mut taken = Vec::new();
            for &h in &self.handles {
                let r = unsafe { WaitForSingleObject(h, 0) };
                if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
                    taken.push(h);
                } else {
                    for t in taken {
                        let _ = unsafe { ReleaseMutex(t) };
                    }
                    return false;
                }
            }
            self.held = true;
            true
        }

        pub fn release(&mut self) {
            if self.held {
                for &h in &self.handles {
                    let _ = unsafe { ReleaseMutex(h) };
                }
            }
            self.held = false;
        }
    }

    impl Drop for PanelLock {
        fn drop(&mut self) {
            self.release();
            for h in self.handles.drain(..) {
                let _ = unsafe { CloseHandle(h) };
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct PanelLock;
    impl PanelLock {
        pub fn new() -> Self {
            PanelLock
        }
        pub fn acquire(&mut self) -> bool {
            true
        }
        pub fn release(&mut self) {}
    }
}

pub use imp::PanelLock;

impl Default for PanelLock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::PanelLock;

    #[test]
    fn second_holder_waits_until_the_first_lets_go() {
        // Mutexes are recursive per thread, so the rival has to live on another one.
        let name = format!(r"Local\Foxglove.PanelTest.{}", std::process::id());
        let mut first = PanelLock::named(&name);
        assert!(first.acquire());
        let rival = || {
            let name = name.clone();
            std::thread::spawn(move || PanelLock::named(&name).acquire())
                .join()
                .unwrap()
        };
        assert!(!rival());
        first.release();
        assert!(rival());
    }
}
