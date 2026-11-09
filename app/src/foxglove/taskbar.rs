//! The tray icon without a taskbar. Started at sign-in, Foxglove can run before
//! Explorer's taskbar exists, and Explorer can restart under it. Neither may end the app.
//!
//! - [`Slot`] holds the tray icon, or nothing when it couldn't be made. Making it fails with a
//!   message in the log, never a panic, and Foxglove runs on without an icon.
//! - Explorer broadcasts the registered "TaskbarCreated" message to every top-level window when
//!   its taskbar starts. A hidden window on its own thread waits for it in `GetMessageW` (no
//!   timer) and wakes the UI, which makes the missing icon then. An icon that already exists
//!   re-adds itself (`tray-icon` handles the same message); its "Error removing system tray icon"
//!   line on an Explorer restart is the old taskbar's icon being gone, which is expected.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the watcher when the taskbar starts; taken by the UI.
static CREATED: AtomicBool = AtomicBool::new(false);

/// Whether the taskbar started since the last call (clears it).
pub fn take() -> bool {
    CREATED.swap(false, Ordering::AcqRel)
}

/// Records a taskbar start and wakes the UI.
fn created(wake: &dyn Fn()) {
    CREATED.store(true, Ordering::Release);
    wake();
}

/// The tray icon, or nothing until a taskbar start lets it be made.
pub struct Slot<T>(Option<T>);

impl<T> Slot<T> {
    /// Makes the icon now; a failure is logged and leaves the slot empty.
    pub fn new(build: impl FnOnce() -> Result<T, String>) -> Self {
        match build() {
            Ok(icon) => Self(Some(icon)),
            Err(e) => {
                eprintln!("Foxglove: no tray icon yet ({e}); it's added when the taskbar starts");
                Self(None)
            }
        }
    }

    /// The taskbar started: makes the icon if it's missing. True when it was made just now.
    pub fn taskbar_created(&mut self, build: impl FnOnce() -> Result<T, String>) -> bool {
        if self.0.is_some() {
            eprintln!("Foxglove: the taskbar restarted; the tray icon is added again");
            return false;
        }
        match build() {
            Ok(icon) => {
                self.0 = Some(icon);
                eprintln!("Foxglove: the taskbar is up; tray icon added");
                true
            }
            Err(e) => {
                eprintln!("Foxglove: still no tray icon ({e}); trying at the next taskbar start");
                false
            }
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.0.as_mut()
    }
}

/// Watches for the taskbar starting, waking the UI each time. Call once.
pub fn watch(wake: impl Fn() + Send + Sync + 'static) {
    imp::watch(Box::new(wake));
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DispatchMessageW,
        GetMessageW, RegisterClassW, RegisterWindowMessageW, MSG, MSGFLT_ALLOW, WNDCLASSW,
        WS_EX_TOOLWINDOW, WS_POPUP,
    };

    type Wake = Box<dyn Fn() + Send + Sync>;
    static WAKE: OnceLock<Wake> = OnceLock::new();
    static MESSAGE: OnceLock<u32> = OnceLock::new();

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if MESSAGE.get().is_some_and(|m| *m != 0 && *m == message) {
            if let Some(wake) = WAKE.get() {
                super::created(wake.as_ref());
            }
            return LRESULT(0);
        }
        // SAFETY: the default handling for this window's other messages.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    pub fn watch(wake: Wake) {
        if WAKE.set(wake).is_err() {
            return; // watching already
        }
        super::super::services::spawn_named("taskbar-watch", || {
            // SAFETY: registers a message name; no pointers kept.
            let message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
            if message == 0 {
                eprintln!("Foxglove: can't watch for the taskbar (no message id)");
                return;
            }
            let _ = MESSAGE.set(message);
            let class = w!("FoxgloveTaskbarWatch");
            // SAFETY: window-class and window creation on this thread; the class name is a
            // static string, and the window lives as long as this thread's message loop.
            unsafe {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    lpszClassName: class,
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    eprintln!("Foxglove: can't watch for the taskbar (no window class)");
                    return;
                }
                // A top-level window, never shown: message-only windows get no broadcasts.
                let Ok(hwnd) = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class,
                    PCWSTR::null(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                ) else {
                    eprintln!("Foxglove: can't watch for the taskbar (no window)");
                    return;
                };
                // Let the broadcast through when Foxglove runs elevated.
                let _ = ChangeWindowMessageFilterEx(hwnd, message, MSGFLT_ALLOW, None);
                let mut msg = MSG::default();
                // Sleeps here between messages; ends with the process.
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = DispatchMessageW(&msg);
                }
            }
        });
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn watch(_wake: Box<dyn Fn() + Send + Sync>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A tray that fails the first `fails` times it's made, like `Shell_NotifyIcon` or the
    /// tray window before the taskbar exists.
    struct Flaky {
        fails: Cell<u32>,
        made: Cell<u32>,
    }

    impl Flaky {
        fn new(fails: u32) -> Self {
            Self {
                fails: Cell::new(fails),
                made: Cell::new(0),
            }
        }
        fn build(&self) -> Result<u32, String> {
            self.made.set(self.made.get() + 1);
            if self.fails.get() > 0 {
                self.fails.set(self.fails.get() - 1);
                return Err("no taskbar".into());
            }
            Ok(self.made.get())
        }
    }

    #[test]
    fn a_failed_tray_leaves_the_app_running_without_an_icon() {
        let tray = Flaky::new(1);
        let mut slot = Slot::new(|| tray.build());
        assert!(slot.get_mut().is_none(), "no icon, and no panic");
        assert_eq!(tray.made.get(), 1);
    }

    #[test]
    fn the_icon_is_made_on_the_taskbar_event_and_only_then() {
        let tray = Flaky::new(2);
        let mut slot = Slot::new(|| tray.build());
        // No event, no retry: nothing polls.
        for _ in 0..5 {
            assert!(slot.get_mut().is_none());
        }
        assert_eq!(tray.made.get(), 1);
        // A taskbar start that still fails keeps waiting for the next one.
        assert!(!slot.taskbar_created(|| tray.build()));
        assert!(slot.get_mut().is_none());
        // The next one works.
        assert!(slot.taskbar_created(|| tray.build()));
        assert_eq!(slot.get_mut().copied(), Some(3));
        assert_eq!(tray.made.get(), 3);
    }

    #[test]
    fn an_existing_icon_is_kept_when_explorer_restarts() {
        let tray = Flaky::new(0);
        let mut slot = Slot::new(|| tray.build());
        assert!(!slot.taskbar_created(|| tray.build()), "not made again");
        assert_eq!(slot.get_mut().copied(), Some(1));
        assert_eq!(tray.made.get(), 1, "tray-icon re-adds its own icon");
    }

    #[test]
    fn the_event_is_taken_once_and_wakes_the_ui() {
        let woke = Cell::new(0);
        let _ = take();
        created(&|| woke.set(woke.get() + 1));
        assert_eq!(woke.get(), 1);
        assert!(take());
        assert!(!take(), "taken once");
    }
}
