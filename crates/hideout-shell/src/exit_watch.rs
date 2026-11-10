// SPDX-License-Identifier: MIT
//! Clean ends of the hub (#1033, §7.10): the caller's hook runs once when the hub ends on
//! purpose, so a start can tell a crash or a kill from a quit.
//!
//! - **Quit Hideout** in the tray: the shell calls [`clean_exit`] before it exits.
//! - Windows ending the session: a hidden top-level window on its own thread gets
//!   `WM_ENDSESSION` (message-only windows don't get the broadcast); the hook runs before the
//!   answer, as Windows may end the process any time after it. A cancelled end runs nothing.
//! - Ctrl+C, Ctrl+Break, a console close, log-off and shut-down of a console run: a console
//!   control handler runs the hook, then lets Windows' default handler end the process.
//!
//! The window thread blocks in `GetMessageW`: no timer, no polling. Only the hub that holds the
//! folder's supervisor lock installs it.

use std::sync::{Once, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

/// What a clean end does (marks `run\hub-run.json` clean).
pub type Hook = Box<dyn Fn() + Send + Sync>;

static HOOK: OnceLock<Hook> = OnceLock::new();
static RAN: Once = Once::new();

const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;

/// Sets the hook and starts watching for the session's end and console signals. Once per
/// process; later calls are ignored.
pub fn install(on_clean_exit: Hook) {
    if HOOK.set(on_clean_exit).is_err() {
        return;
    }
    unsafe {
        use windows::Win32::System::Console::SetConsoleCtrlHandler;
        if let Err(e) = SetConsoleCtrlHandler(Some(on_ctrl), true) {
            log::warn!("console control handler not set: {e}");
        }
    }
    if let Err(e) = std::thread::Builder::new()
        .name("hideout-session-end".into())
        .spawn(session_window)
    {
        log::warn!("session-end watcher not started: {e}");
    }
}

/// Runs the hook (once; later calls do nothing). Any thread.
pub fn clean_exit() {
    RAN.call_once(|| {
        if let Some(h) = HOOK.get() {
            h();
        }
    });
}

/// Whether a message to a top-level window says the session ends now: `WM_ENDSESSION` with
/// `wParam` TRUE. `WM_QUERYENDSESSION` only asks, and FALSE is a cancelled end.
pub fn ends_session(msg: u32, wparam: usize) -> bool {
    msg == WM_ENDSESSION && wparam != 0
}

/// Console signals that end the process (all of them do: the hub has no Ctrl+C action).
pub fn ctrl_ends(kind: u32) -> bool {
    // CTRL_C, CTRL_BREAK, CTRL_CLOSE, CTRL_LOGOFF, CTRL_SHUTDOWN.
    matches!(kind, 0 | 1 | 2 | 5 | 6)
}

unsafe extern "system" fn on_ctrl(kind: u32) -> windows::core::BOOL {
    if ctrl_ends(kind) {
        log::info!("console signal {kind}: clean exit");
        clean_exit();
        log::logger().flush();
    }
    // FALSE: the default handler ends the process as before.
    false.into()
}

unsafe extern "system" fn session_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
    if msg == WM_QUERYENDSESSION {
        return LRESULT(1);
    }
    if ends_session(msg, wp.0) {
        log::info!("Windows is ending the session: clean exit");
        clean_exit();
        log::logger().flush();
        return LRESULT(0);
    }
    DefWindowProcW(h, msg, wp, lp)
}

/// A hidden top-level window (never shown, no taskbar entry) and its message loop.
fn session_window() {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, MSG, WNDCLASSW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    unsafe {
        let Ok(inst) = GetModuleHandleW(None) else {
            log::warn!("session-end watcher: no module handle");
            return;
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(session_proc),
            hInstance: inst.into(),
            lpszClassName: w!("BanditF.Hideout.SessionEnd"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let made = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("BanditF.Hideout.SessionEnd"),
            w!("Hideout session end"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(inst.into()),
            None,
        );
        if let Err(e) = made {
            log::warn!("session-end watcher: no window: {e}");
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_real_session_end_and_ending_signals_count() {
        assert!(ends_session(WM_ENDSESSION, 1));
        assert!(!ends_session(WM_ENDSESSION, 0), "a cancelled end");
        assert!(!ends_session(WM_QUERYENDSESSION, 1), "only a question");
        assert!(!ends_session(0x0010, 1), "WM_CLOSE");
        for k in [0, 1, 2, 5, 6] {
            assert!(ctrl_ends(k), "{k}");
        }
        assert!(!ctrl_ends(3) && !ctrl_ends(4));
    }

    /// Without a hook installed, a clean exit does nothing (a second hub, tests).
    #[test]
    fn a_clean_exit_without_a_hook_is_a_no_op() {
        clean_exit();
        clean_exit();
    }
}
