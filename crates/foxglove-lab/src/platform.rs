//! Native focus isolation and process exclusion on Windows.
use crate::spec::Step;
use anyhow::{ensure, Result};
use std::{
    io::{self, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

static CANCELLED: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

/// Installs a handler that only sets an atomic flag; normal execution writes the report.
pub fn cancellation() -> Result<Arc<AtomicBool>> {
    let flag = CANCELLED
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone();
    #[cfg(windows)]
    unsafe {
        unsafe extern "system" fn handler(kind: u32) -> windows::core::BOOL {
            if kind == 0 || kind == 1 {
                if let Some(flag) = CANCELLED.get() {
                    flag.store(true, Ordering::Relaxed);
                }
                return true.into();
            }
            false.into()
        }
        windows::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), true)?;
    }
    #[cfg(unix)]
    unsafe {
        extern "C" fn handler(_: libc::c_int) {
            if let Some(flag) = CANCELLED.get() {
                flag.store(true, Ordering::Relaxed);
            }
        }
        ensure!(
            libc::signal(libc::SIGINT, handler as libc::sighandler_t) != libc::SIG_ERR,
            "could not install SIGINT handler"
        );
    }
    Ok(flag)
}

/// The PC is locked (or on the secure desktop): the input desktop can't be opened for
/// switching. Foxglove hands the left panel back while away (#410), and typing goes nowhere.
#[cfg(windows)]
pub fn locked() -> bool {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_SWITCHDESKTOP,
    };
    // SAFETY: plain Win32 call with no pointers; the handle it returns is closed right here.
    match unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) } {
        Ok(desk) => {
            // SAFETY: `desk` came from OpenInputDesktop above and is closed exactly once.
            let _ = unsafe { CloseDesktop(desk) };
            false
        }
        Err(_) => true,
    }
}
#[cfg(not(windows))]
pub fn locked() -> bool {
    false
}

pub fn prompt(ask: &str, cancelled: &Arc<AtomicBool>) -> Result<String> {
    ensure!(
        io::stdin().is_terminal(),
        "manual prompt requires a TTY: {ask}"
    );
    println!("{ask}");
    io::stdout().flush()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut answer = String::new();
        let result = io::stdin().read_line(&mut answer).map(|n| (n, answer));
        let _ = tx.send(result);
    });
    let end = Instant::now() + Duration::from_secs(300);
    loop {
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "manual prompt interrupted: {ask}"
        );
        ensure!(Instant::now() < end, "manual prompt timeout: {ask}");
        if let Ok(answer) = rx.recv_timeout(Duration::from_millis(50)) {
            let (n, answer) = answer?;
            ensure!(n > 0, "manual prompt EOF: {ask}");
            // Preserve the entire answer, including its line ending, in the report.
            return Ok(answer);
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use windows::{
        core::w,
        Win32::{
            Foundation::{
                CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT,
                WPARAM,
            },
            System::{
                LibraryLoader::GetModuleHandleW,
                Threading::{
                    AttachThreadInput, CreateMutexW, GetCurrentThreadId, ReleaseMutex,
                    WaitForSingleObject,
                },
            },
            UI::WindowsAndMessaging::*,
        },
    };

    /// Holds Foxglove's single-instance mutex open; says whether the app was running.
    pub struct AppGuard(HANDLE, bool);
    impl AppGuard {
        /// Foxglove was running when the guard was taken (its mutex existed; `--allow-app`).
        pub fn app_running(&self) -> bool {
            self.1
        }
    }
    impl Drop for AppGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    pub fn guard(allow: bool) -> Result<AppGuard> {
        unsafe {
            let h = CreateMutexW(None, false, w!("Local\\Foxglove.Panel"))?;
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            let guard = AppGuard(h, exists);
            ensure!(
                !exists || allow,
                "Foxglove is running (Local\\Foxglove.Panel); tray → Quit, or use --allow-app"
            );
            Ok(guard)
        }
    }
    /// One foxglove-lab at a time on the keyboard: two runners interleaving requests on the same
    /// link make each other's steps fail ("no reply"). A named mutex is released by Windows
    /// when its owner exits, so a crashed run never leaves a stale lock.
    pub struct LabLock(HANDLE);
    impl Drop for LabLock {
        fn drop(&mut self) {
            unsafe {
                let _ = ReleaseMutex(self.0);
                let _ = CloseHandle(self.0);
            }
        }
    }
    pub fn lab_lock(wait: Duration) -> Result<LabLock> {
        unsafe {
            let h = CreateMutexW(None, false, w!("Local\\Foxglove.Lab"))?;
            let lock = LabLock(h);
            let mut status = WaitForSingleObject(h, 0);
            if status.0 == 0x102 {
                eprintln!("waiting for another foxglove-lab run to finish...");
                status = WaitForSingleObject(h, wait.as_millis().min(u32::MAX as u128) as u32);
            }
            // WAIT_OBJECT_0 (0) or WAIT_ABANDONED (0x80): ours now.
            ensure!(
                status.0 == 0 || status.0 == 0x80,
                "another foxglove-lab run is using the keyboard; try again when it finishes"
            );
            Ok(lock)
        }
    }
    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    fn pump(hwnd: HWND, text: &mut Vec<u16>) -> Result<()> {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                ensure!(
                    msg.message != WM_QUIT && msg.message != WM_CLOSE,
                    "sink closed"
                );
                if msg.hwnd == hwnd && msg.message == WM_CHAR {
                    text.push(msg.wParam.0 as u16);
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            ensure!(IsWindow(Some(hwnd)).as_bool(), "sink closed");
        }
        Ok(())
    }
    /// Windows only lets the process that owns the foreground window hand focus over, so a
    /// console-launched runner can lose that race. Joining the foreground thread's input queue
    /// for the call is the documented way to be allowed to take it.
    unsafe fn take_foreground(hwnd: HWND) {
        let current = GetForegroundWindow();
        let theirs = GetWindowThreadProcessId(current, None);
        let ours = GetCurrentThreadId();
        let attached =
            theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(ours, theirs, false);
        }
    }

    pub fn sink(
        text: &str,
        via: &[Step],
        cancelled: &Arc<AtomicBool>,
        mut inject: impl FnMut(&Step) -> Result<()>,
    ) -> Result<String> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("FoxgloveLabSink");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // A prior typing case may have registered the class already.
            let _ = RegisterClassW(&wc);
            let title = windows::core::HSTRING::from(format!(
                "Foxglove lab typing sink — expected: {text}"
            ));
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                &title,
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                700,
                160,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let _window = Window(hwnd);
            let mut received = Vec::new();
            let end = Instant::now() + Duration::from_secs(5);
            let mut asked = Instant::now() - Duration::from_secs(1);
            while GetForegroundWindow() != hwnd {
                ensure!(!cancelled.load(Ordering::Relaxed), "sink interrupted");
                if asked.elapsed() >= Duration::from_millis(250) {
                    take_foreground(hwnd);
                    asked = Instant::now();
                }
                pump(hwnd, &mut received)?;
                ensure!(
                    Instant::now() < end,
                    "sink did not receive foreground focus within 5 seconds"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            pump(hwnd, &mut received)?;
            received.clear();
            for step in via {
                ensure!(!cancelled.load(Ordering::Relaxed), "sink interrupted");
                pump(hwnd, &mut received)?;
                ensure!(
                    GetForegroundWindow() == hwnd,
                    "sink lost foreground focus; injection stopped"
                );
                inject(step)?;
                let end = Instant::now() + Duration::from_millis(80);
                while Instant::now() < end {
                    ensure!(GetForegroundWindow() == hwnd, "sink lost foreground focus");
                    pump(hwnd, &mut received)?;
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            let end = Instant::now() + Duration::from_millis(500);
            while Instant::now() < end {
                ensure!(GetForegroundWindow() == hwnd, "sink lost foreground focus");
                pump(hwnd, &mut received)?;
                ensure!(!cancelled.load(Ordering::Relaxed), "sink interrupted");
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(String::from_utf16(&received)?)
        }
    }
}
#[cfg(windows)]
pub use native::{guard, lab_lock, sink};

#[cfg(not(windows))]
pub struct AppGuard;
#[cfg(not(windows))]
impl AppGuard {
    pub fn app_running(&self) -> bool {
        false
    }
}
#[cfg(not(windows))]
pub fn guard(_: bool) -> Result<AppGuard> {
    Ok(AppGuard)
}
#[cfg(not(windows))]
pub fn lab_lock(_: Duration) -> Result<()> {
    Ok(())
}
#[cfg(not(windows))]
pub fn sink(
    _: &str,
    _: &[Step],
    _: &Arc<AtomicBool>,
    _: impl FnMut(&Step) -> Result<()>,
) -> Result<String> {
    anyhow::bail!("typing sink requires Windows")
}
