//! Screen glow's own Windows events (#496), on one thread that exists only while a running
//! scene's look is Screen glow:
//!
//! - **Full screen entered or left** (`ABN_FULLSCREENAPP`): the thread's hidden window is
//!   registered as an appbar (`SHAppBarMessage(ABM_NEW)`, no space reserved, never shown), and
//!   Windows tells every appbar when a window goes full screen or stops being so. That covers a
//!   game that goes full screen after it is already in front (a loading screen first, Alt+Enter),
//!   which no foreground change would show.
//! - **The desktop switched** (`EVENT_SYSTEM_DESKTOPSWITCH`): a UAC prompt or Ctrl+Alt+Del moves
//!   to the secure desktop and back, which makes Desktop Duplication lose its access; coming back
//!   is the moment to open the screens again.
//!
//! The thread sleeps in `GetMessageW`; nothing here waits on a clock. Dropping the [`Watch`]
//! ends it (`WM_QUIT`) and unregisters both.

use std::sync::Arc;

/// What the watch tells.
pub type Tell = Arc<dyn Fn() + Send + Sync>;

/// Keeps the watch running; dropping it ends the thread.
pub struct Watch {
    /// Kept for its drop, which ends the thread.
    #[cfg(all(windows, not(test)))]
    _imp: imp::Thread,
}

impl Drop for Watch {
    /// Ends the watch (the thread's own drop posts `WM_QUIT` and joins it).
    fn drop(&mut self) {}
}

impl Watch {
    /// Starts the watch: `full_screen` on each full-screen change, `desktop` on each desktop
    /// switch. `None` when Windows refused (the gates then rest on the foreground watch alone).
    #[cfg(all(windows, not(test)))]
    pub fn start(full_screen: Tell, desktop: Tell) -> Option<Watch> {
        imp::Thread::start(full_screen, desktop).map(|imp| Watch { _imp: imp })
    }

    /// Tests and other systems have no such events: the fakes call the reader directly.
    #[cfg(any(not(windows), test))]
    pub fn start(_full_screen: Tell, _desktop: Tell) -> Option<Watch> {
        None
    }
}

#[cfg(all(windows, not(test)))]
mod imp {
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::sync::Once;
    use std::thread::JoinHandle;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows::Win32::UI::Shell::{
        SHAppBarMessage, ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP, APPBARDATA,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostThreadMessageW, RegisterClassW, EVENT_SYSTEM_DESKTOPSWITCH, MSG, WINDOW_STYLE,
        WINEVENT_OUTOFCONTEXT, WM_APP, WM_QUIT, WNDCLASSW, WS_EX_TOOLWINDOW,
    };

    use super::Tell;

    /// The appbar's callback message.
    const APPBAR: u32 = WM_APP + 0x496;

    thread_local! {
        /// This thread's callbacks: full screen, desktop switch.
        static TELL: RefCell<Option<(Tell, Tell)>> = const { RefCell::new(None) };
    }

    fn tell(which: usize) {
        TELL.with(|t| {
            if let Some((full, desktop)) = t.borrow().as_ref() {
                // A panic must not unwind into Windows.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if which == 0 {
                        full()
                    } else {
                        desktop()
                    }
                }));
            }
        });
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == APPBAR {
            if wparam.0 as u32 == ABN_FULLSCREENAPP {
                tell(0);
            }
            return LRESULT(0);
        }
        // SAFETY: the default handling for this window's other messages.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    unsafe extern "system" fn desktop_switched(
        _hook: HWINEVENTHOOK,
        _event: u32,
        _hwnd: HWND,
        _object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        tell(1);
    }

    pub struct Thread {
        id: u32,
        thread: Option<JoinHandle<()>>,
    }

    impl Thread {
        pub fn start(full_screen: Tell, desktop: Tell) -> Option<Thread> {
            let (ready, wait) = mpsc::channel::<Option<u32>>();
            let thread = std::thread::Builder::new()
                .name("screen-glow-watch".into())
                .spawn(move || run(full_screen, desktop, &ready))
                .ok()?;
            match wait.recv() {
                Ok(Some(id)) => Some(Thread {
                    id,
                    thread: Some(thread),
                }),
                _ => {
                    let _ = thread.join();
                    None
                }
            }
        }
    }

    impl Drop for Thread {
        fn drop(&mut self) {
            // SAFETY: posts WM_QUIT to the watch thread's queue (it made one with its window).
            unsafe {
                let _ = PostThreadMessageW(self.id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn run(full_screen: Tell, desktop: Tell, ready: &mpsc::Sender<Option<u32>>) {
        static CLASS: Once = Once::new();
        let class: Vec<u16> = "FoxgloveScreenGlowWatch\0".encode_utf16().collect();
        TELL.with(|t| *t.borrow_mut() = Some((full_screen, desktop)));
        // SAFETY: window-class registration (once per process; the class name is a static
        // string's copy that outlives the call), then a window, an appbar registration and a
        // WinEvent hook made and torn down on this thread. The window is never shown.
        unsafe {
            CLASS.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    lpszClassName: PCWSTR(class.as_ptr()),
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    eprintln!("Foxglove: screen glow watch: no window class");
                }
            });
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            ) else {
                eprintln!("Foxglove: screen glow watch: no window; full screen is seen on program changes only");
                let _ = ready.send(None);
                return;
            };
            let mut bar = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: hwnd,
                uCallbackMessage: APPBAR,
                ..Default::default()
            };
            let registered = SHAppBarMessage(ABM_NEW, &mut bar) != 0;
            if !registered {
                eprintln!("Foxglove: screen glow watch: no appbar; full screen is seen on program changes only");
            }
            let hook = SetWinEventHook(
                EVENT_SYSTEM_DESKTOPSWITCH,
                EVENT_SYSTEM_DESKTOPSWITCH,
                None,
                Some(desktop_switched),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            let _ = ready.send(Some(GetCurrentThreadId()));
            let mut msg = MSG::default();
            // 0 is WM_QUIT and -1 an error: both end the loop.
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = DispatchMessageW(&msg);
            }
            if !hook.is_invalid() {
                let _ = UnhookWinEvent(hook);
            }
            if registered {
                SHAppBarMessage(ABM_REMOVE, &mut bar);
            }
            let _ = DestroyWindow(hwnd);
        }
        TELL.with(|t| *t.borrow_mut() = None);
    }
}
