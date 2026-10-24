//! Native focus isolation and process exclusion on Windows.
use crate::spec::{Invoke, Step};
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
            System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
            UI::WindowsAndMessaging::*,
        },
    };

    pub struct AppGuard(HANDLE);
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
            let guard = AppGuard(h);
            ensure!(
                !exists || allow,
                "Foxglove is running (Local\\Foxglove.Panel); tray → Quit, or use --allow-app"
            );
            Ok(guard)
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
    pub fn sink(
        text: &str,
        via: &[Step],
        cancelled: &Arc<AtomicBool>,
        mut inject: impl FnMut(&Invoke) -> Result<()>,
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
            let _ = SetForegroundWindow(hwnd);
            let mut received = Vec::new();
            let end = Instant::now() + Duration::from_secs(5);
            while GetForegroundWindow() != hwnd {
                ensure!(!cancelled.load(Ordering::Relaxed), "sink interrupted");
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
                let Step::Invoke(i) = step else {
                    anyhow::bail!("sink via needs invoke")
                };
                inject(i)?;
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
pub use native::{guard, sink};

#[cfg(not(windows))]
pub fn guard(_: bool) -> Result<()> {
    Ok(())
}
#[cfg(not(windows))]
pub fn sink(
    _: &str,
    _: &[Step],
    _: &Arc<AtomicBool>,
    _: impl FnMut(&Invoke) -> Result<()>,
) -> Result<String> {
    anyhow::bail!("typing sink requires Windows")
}
