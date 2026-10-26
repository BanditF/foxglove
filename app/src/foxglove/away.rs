//! Away (#410): the session is locked or the display is off. While away, nothing in Foxglove
//! runs on a clock: the left panel goes back to the firmware (no clock minutes, no animation),
//! the art hotlist's timer stops, and QMK's layer-subscription renewal stops. Coming back is
//! the event that starts them again.
//!
//! Windows tells us both, so nothing here waits or repeats: the display state comes from a
//! power-setting callback (`GUID_CONSOLE_DISPLAY_STATE`, with no window), and lock/unlock from
//! `WTSRegisterSessionNotification` on a message-only window whose thread sleeps in
//! `GetMessageW`. Reads and keyboard events go on as usual.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static LOCKED: AtomicBool = AtomicBool::new(false);
static DISPLAY_OFF: AtomicBool = AtomicBool::new(false);

type Listener = (u64, Box<dyn Fn(bool) + Send>);
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Keeps a listener registered; dropping it removes the listener (and whatever it holds) at once.
pub struct Subscription(u64);

impl Drop for Subscription {
    fn drop(&mut self) {
        LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| *id != self.0);
    }
}

/// Whether the session is locked or the display is off.
pub fn away() -> bool {
    LOCKED.load(Ordering::SeqCst) || DISPLAY_OFF.load(Ordering::SeqCst)
}

/// Calls `notify(away)` each time [`away`] changes, until the subscription is dropped. It runs
/// on a system thread with the listener list locked: only enqueue work there.
pub fn subscribe(notify: impl Fn(bool) + Send + 'static) -> Subscription {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).push((id, Box::new(notify)));
    Subscription(id)
}

/// Records the session's lock state and tells the listeners if that changed [`away`].
fn set_locked(locked: bool) {
    update(&LOCKED, locked, "session locked", "session unlocked");
}

/// Records the display state and tells the listeners if that changed [`away`].
fn set_display_off(off: bool) {
    update(&DISPLAY_OFF, off, "display off", "display on");
}

fn update(flag: &AtomicBool, value: bool, on: &str, off: &str) {
    let before = away();
    if flag.swap(value, Ordering::SeqCst) == value {
        return;
    }
    let now = away();
    if now == before {
        return;
    }
    eprintln!("Foxglove: {} ({}), {}", if now { "away" } else { "back" }, if value { on } else { off }, if now { "background work stops" } else { "background work resumes" });
    tell(now);
}

fn tell(now: bool) {
    for (_, notify) in LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        notify(now);
    }
}

/// Starts listening for lock/unlock and display on/off. `FOXGLOVE_DISABLE=away` leaves it out
/// (Foxglove then behaves as if always present).
pub fn start() {
    if super::repaint_trace::disabled("away") {
        return;
    }
    #[cfg(windows)]
    imp::start();
}

#[cfg(windows)]
mod imp {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Power::{PowerSettingRegisterNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, POWERBROADCAST_SETTING};
    use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, DEVICE_NOTIFY_CALLBACK, HWND_MESSAGE, MSG,
        PBT_POWERSETTINGCHANGE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    /// `GUID_CONSOLE_DISPLAY_STATE`: 0 off, 1 on, 2 dimmed.
    const CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);

    pub fn start() {
        display();
        session();
    }

    unsafe extern "system" fn on_power(_context: *const core::ffi::c_void, kind: u32, setting: *const core::ffi::c_void) -> u32 {
        if kind != PBT_POWERSETTINGCHANGE || setting.is_null() {
            return 0;
        }
        // SAFETY: for PBT_POWERSETTINGCHANGE, `setting` points at a POWERBROADCAST_SETTING whose
        // Data holds DataLength bytes; the display state is one DWORD.
        let setting = unsafe { &*setting.cast::<POWERBROADCAST_SETTING>() };
        if setting.PowerSetting != CONSOLE_DISPLAY_STATE || (setting.DataLength as usize) < size_of::<u32>() {
            return 0;
        }
        let state = unsafe { std::ptr::addr_of!(setting.Data).cast::<u32>().read_unaligned() };
        super::set_display_off(state == 0);
        0
    }

    /// The display state, by callback: Windows sends the current state at once, then changes.
    fn display() {
        // Process-lifetime registration: the parameters must outlive it, so they're leaked.
        let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: std::ptr::null_mut() }));
        let mut handle = std::ptr::null_mut();
        // SAFETY: with DEVICE_NOTIFY_CALLBACK the recipient is a pointer to the subscribe
        // parameters, which live for the rest of the process; `handle` is a valid out pointer.
        let result = unsafe {
            PowerSettingRegisterNotification(
                &CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(std::ptr::from_mut(params).cast()),
                &mut handle,
            )
        };
        if result.is_err() {
            eprintln!("Foxglove: can't follow the display state: {result:?}");
        }
    }

    unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if message == WM_WTSSESSION_CHANGE {
            match wparam.0 as u32 {
                WTS_SESSION_LOCK => super::set_locked(true),
                WTS_SESSION_UNLOCK => super::set_locked(false),
                _ => {}
            }
            return LRESULT(0);
        }
        // SAFETY: the default handling for this window's other messages.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// Lock and unlock, on a message-only window (never shown, gets no broadcasts). Its thread
    /// sleeps in GetMessageW between notifications.
    fn session() {
        super::super::services::spawn_named("away-session", || {
            let class: Vec<u16> = "FoxgloveAway\0".encode_utf16().collect();
            // SAFETY: plain window-class and window creation on this thread; the class name
            // outlives both calls, and the window lives as long as this thread's message loop.
            unsafe {
                let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
                if RegisterClassW(&wc) == 0 {
                    eprintln!("Foxglove: can't follow the session lock (no window class)");
                    return;
                }
                let Ok(hwnd) = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(class.as_ptr()),
                    PCWSTR::null(),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                ) else {
                    eprintln!("Foxglove: can't follow the session lock (no window)");
                    return;
                };
                if let Err(e) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
                    eprintln!("Foxglove: can't follow the session lock: {e}");
                    return;
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = DispatchMessageW(&msg);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn locked_or_display_off_is_away_and_listeners_hear_each_change_once() {
        let (tx, rx) = mpsc::channel();
        let token = subscribe(move |away| {
            let _ = tx.send(away);
        });
        assert!(!away());
        set_display_off(true);
        set_locked(true); // already away: no second message
        set_display_off(false); // still locked
        assert!(away());
        set_locked(false);
        set_locked(false); // unchanged
        assert!(!away());
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [true, false]);
        drop(token);
        set_locked(true);
        set_locked(false);
        assert!(rx.try_iter().next().is_none(), "a dropped token stops the calls");
    }
}
