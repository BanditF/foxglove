//! Only one app may drive the left panel: a named mutex shared with Foxglove, so the old and the
//! new app never send frames at the same time. Windows mutexes belong to a thread, so take and give
//! it back on the same one (the worker).

pub const NAME: &str = r"Local\Foxglove.Panel";

#[cfg(windows)]
mod imp {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    pub struct PanelLock {
        handle: Option<HANDLE>,
        held: bool,
    }

    impl PanelLock {
        pub fn new() -> Self {
            Self::named(super::NAME)
        }

        pub fn named(name: &str) -> Self {
            let handle = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }.ok();
            PanelLock { handle, held: false }
        }

        /// Takes the lock if nobody holds it (or its holder died); true while we hold it.
        pub fn acquire(&mut self) -> bool {
            if self.held {
                return true;
            }
            let Some(h) = self.handle else {
                return true; // no mutex: behave as before rather than never drawing
            };
            let r = unsafe { WaitForSingleObject(h, 0) };
            self.held = r == WAIT_OBJECT_0 || r == WAIT_ABANDONED;
            self.held
        }

        pub fn release(&mut self) {
            if let (true, Some(h)) = (self.held, self.handle) {
                let _ = unsafe { ReleaseMutex(h) };
            }
            self.held = false;
        }
    }

    impl Drop for PanelLock {
        fn drop(&mut self) {
            self.release();
            if let Some(h) = self.handle.take() {
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
            std::thread::spawn(move || PanelLock::named(&name).acquire()).join().unwrap()
        };
        assert!(!rival());
        first.release();
        assert!(rival());
    }
}
