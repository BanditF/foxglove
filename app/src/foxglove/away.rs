//! Away (#410): the session is locked or the display is off. While away, nothing in Foxglove
//! runs on a clock: the left panel goes back to the firmware (no clock minutes, no animation),
//! the art hotlist's timer stops, and QMK's layer-subscription renewal stops. Coming back is
//! the event that starts them again.
//!
//! Windows tells us both, so nothing here waits or repeats: the display state comes from a
//! power-setting callback (`GUID_CONSOLE_DISPLAY_STATE`, with no window), and lock/unlock from
//! `WTSRegisterSessionNotification` on a message-only window whose thread sleeps in
//! `GetMessageW`. Reads and keyboard events go on as usual.
//!
//! The keyboard's PC state (`pc_state.rs`, #609) needs the two apart: the display going off is
//! the PC asleep, a lock alone is not. [`subscribe_changes`] hears each of the four edges.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static LOCKED: AtomicBool = AtomicBool::new(false);
static DISPLAY_OFF: AtomicBool = AtomicBool::new(false);

type Listener = (u64, Box<dyn Fn(bool) + Send>);
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

/// One edge of the session lock or the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Locked,
    Unlocked,
    DisplayOff,
    DisplayOn,
}

type ChangeListener = (u64, Box<dyn Fn(Change) + Send>);
static CHANGE_LISTENERS: Mutex<Vec<ChangeListener>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Keeps a listener registered; dropping it removes the listener (and whatever it holds) at once.
pub struct Subscription(u64);

impl Drop for Subscription {
    fn drop(&mut self) {
        LISTENERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| *id != self.0);
        CHANGE_LISTENERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| *id != self.0);
    }
}

/// Whether the session is locked or the display is off.
pub fn away() -> bool {
    LOCKED.load(Ordering::SeqCst) || DISPLAY_OFF.load(Ordering::SeqCst)
}

/// Whether the display is off (the PC asleep, or about to be: Modern Standby starts there).
pub fn display_off() -> bool {
    DISPLAY_OFF.load(Ordering::SeqCst)
}

/// Calls `notify(change)` for each lock, unlock, display off and display on, until the
/// subscription is dropped, whether or not [`away`] changed with it. It runs on a system thread
/// with the listener list locked: only enqueue work there.
pub fn subscribe_changes(notify: impl Fn(Change) + Send + 'static) -> Subscription {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    CHANGE_LISTENERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, Box::new(notify)));
    Subscription(id)
}

/// Calls `notify(away)` each time [`away`] changes, until the subscription is dropped. It runs
/// on a system thread with the listener list locked: only enqueue work there.
pub fn subscribe(notify: impl Fn(bool) + Send + 'static) -> Subscription {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    LISTENERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, Box::new(notify)));
    Subscription(id)
}

/// Records the session's lock state and tells the listeners if that changed [`away`].
fn set_locked(locked: bool) {
    let change = if locked {
        Change::Locked
    } else {
        Change::Unlocked
    };
    update(
        &LOCKED,
        locked,
        "session locked",
        "session unlocked",
        change,
    );
}

/// Records the display state and tells the listeners if that changed [`away`].
fn set_display_off(off: bool) {
    let change = if off {
        Change::DisplayOff
    } else {
        Change::DisplayOn
    };
    update(&DISPLAY_OFF, off, "display off", "display on", change);
}

fn update(flag: &AtomicBool, value: bool, on: &str, off: &str, change: Change) {
    let before = away();
    if flag.swap(value, Ordering::SeqCst) == value {
        return;
    }
    for (_, notify) in CHANGE_LISTENERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        notify(change);
    }
    let now = away();
    if now == before {
        return;
    }
    eprintln!(
        "Foxglove: {} ({}), {}",
        if now { "away" } else { "back" },
        if value { on } else { off },
        if now {
            "background work stops"
        } else {
            "background work resumes"
        }
    );
    tell(now);
}

fn tell(now: bool) {
    for (_, notify) in LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        notify(now);
    }
}

/// Starts listening for lock/unlock and display on/off. `FOXGLOVE_DISABLE=away` leaves it out
/// (Foxglove then behaves as if always present). With the test hook
/// (`FOXGLOVE_AWAY_OVERRIDE=<file>`, [`OVERRIDE_ENV`]) the file stands in for Windows.
pub fn start() {
    if super::repaint_trace::disabled("away") {
        return;
    }
    if let Some(path) = override_path() {
        follow_override(path);
        return;
    }
    #[cfg(windows)]
    imp::start();
}

/// The test hook (#618, the 0.2 sitting's batch P): a watched file stands in for Windows' display
/// and lock notifications, so a lab run can turn "the display off" without turning it off. On
/// this PC a real display-off starts Modern Standby, which the probes that did it showed is not
/// safe to drive (#651). Everything after the edge (the PC state to the keyboard, the panel
/// handed back, the clocks stopped) runs as for a real one. The file holds lines `display off`,
/// `display on`, `locked` or `unlocked` (the last of each kind wins); a missing file is the
/// display on and unlocked. Rewriting it is the edge.
///
/// It is read only in a lab launch: with `FOXGLOVE_APP_OK=1` too (the marker every lab script
/// that starts a test app runs under, labkit's `require_app_ok`). A normal run never has it, so a
/// stray `FOXGLOVE_AWAY_OVERRIDE` in a user's environment is ignored (one log line) and the real
/// notifications are followed.
pub const OVERRIDE_ENV: &str = "FOXGLOVE_AWAY_OVERRIDE";
/// The lab's launch marker that arms the hook.
pub const LAB_ENV: &str = "FOXGLOVE_APP_OK";

fn override_path() -> Option<std::path::PathBuf> {
    let hook = std::env::var_os(OVERRIDE_ENV);
    let path = override_from(hook.clone(), std::env::var_os(LAB_ENV));
    if path.is_none() && hook.is_some_and(|h| !h.is_empty()) {
        eprintln!(
            "Foxglove: away test hook: ignored ({OVERRIDE_ENV} is set without {LAB_ENV}=1, so this isn't a lab run)"
        );
    }
    path
}

/// The hook's file, given the two variables: only a non-empty path together with the lab marker.
pub fn override_from(
    hook: Option<std::ffi::OsString>,
    lab: Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    if lab.as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }
    hook.filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
}

/// The hook file's text as (display off, locked).
pub fn parse_override(text: &str) -> (bool, bool) {
    let (mut off, mut locked) = (false, false);
    for line in text.lines().map(|l| l.trim().to_ascii_lowercase()) {
        match line.as_str() {
            "display off" => off = true,
            "display on" => off = false,
            "locked" => locked = true,
            "unlocked" => locked = false,
            _ => {}
        }
    }
    (off, locked)
}

fn apply_override(path: &std::path::Path) {
    let (off, locked) = parse_override(&std::fs::read_to_string(path).unwrap_or_default());
    // The display first: a lock and a display-off in one write is the PC going to sleep.
    set_display_off(off);
    set_locked(locked);
}

/// Reads the hook file now and again on each change in its folder (a folder watch: no timer).
fn follow_override(path: std::path::PathBuf) {
    eprintln!(
        "Foxglove: away test hook: following {} in place of Windows' display and lock",
        path.display()
    );
    apply_override(&path);
    let wake = {
        let path = path.clone();
        std::sync::Arc::new(move || apply_override(&path))
    };
    match super::screens_watch::settings(&path, wake) {
        // Process-lifetime watch, like the real notifications' registration.
        Some(watch) => std::mem::forget(watch),
        None => eprintln!("Foxglove: away test hook: can't watch {}", path.display()),
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Power::{
        PowerSettingRegisterNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
        POWERBROADCAST_SETTING,
    };
    use windows::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        DEVICE_NOTIFY_CALLBACK, HWND_MESSAGE, MSG, PBT_POWERSETTINGCHANGE, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    /// `GUID_CONSOLE_DISPLAY_STATE`: 0 off, 1 on, 2 dimmed.
    const CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);

    pub fn start() {
        display();
        session();
    }

    unsafe extern "system" fn on_power(
        _context: *const core::ffi::c_void,
        kind: u32,
        setting: *const core::ffi::c_void,
    ) -> u32 {
        if kind != PBT_POWERSETTINGCHANGE || setting.is_null() {
            return 0;
        }
        // SAFETY: for PBT_POWERSETTINGCHANGE, `setting` points at a POWERBROADCAST_SETTING whose
        // Data holds DataLength bytes; the display state is one DWORD.
        let setting = unsafe { &*setting.cast::<POWERBROADCAST_SETTING>() };
        if setting.PowerSetting != CONSOLE_DISPLAY_STATE
            || (setting.DataLength as usize) < size_of::<u32>()
        {
            return 0;
        }
        let state = unsafe {
            std::ptr::addr_of!(setting.Data)
                .cast::<u32>()
                .read_unaligned()
        };
        super::set_display_off(state == 0);
        0
    }

    /// The display state, by callback: Windows sends the current state at once, then changes.
    fn display() {
        // Process-lifetime registration: the parameters must outlive it, so they're leaked.
        let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(on_power),
            Context: std::ptr::null_mut(),
        }));
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

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
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
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    lpszClassName: PCWSTR(class.as_ptr()),
                    ..Default::default()
                };
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

    /// The flags are process-wide: the tests that move them take turns.
    static FLAGS: Mutex<()> = Mutex::new(());

    #[test]
    fn locked_or_display_off_is_away_and_listeners_hear_each_change_once() {
        let _flags = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
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
        assert!(
            rx.try_iter().next().is_none(),
            "a dropped token stops the calls"
        );
    }

    /// The test hook's file (#618 batch P): the last line of each kind wins, anything else is
    /// ignored, and an empty file is the PC present.
    #[test]
    fn the_override_file_reads_display_and_lock() {
        assert_eq!(parse_override(""), (false, false));
        assert_eq!(parse_override("display off\n"), (true, false));
        assert_eq!(parse_override("Display Off\r\nlocked\r\n"), (true, true));
        assert_eq!(parse_override("display off\ndisplay on\n"), (false, false));
        assert_eq!(parse_override("locked\nunlocked\nnoise\n"), (false, false));
    }

    /// The hook only arms in a lab launch (FOXGLOVE_APP_OK=1): a user's run with a stray variable
    /// follows Windows as always.
    #[test]
    fn the_override_needs_the_lab_marker() {
        let s = |v: &str| Some(std::ffi::OsString::from(v));
        assert_eq!(override_from(s("C:/x/away.txt"), None), None);
        assert_eq!(override_from(s("C:/x/away.txt"), s("0")), None);
        assert_eq!(override_from(s(""), s("1")), None);
        assert_eq!(override_from(None, s("1")), None);
        assert_eq!(
            override_from(s("C:/x/away.txt"), s("1")),
            Some(std::path::PathBuf::from("C:/x/away.txt"))
        );
    }

    /// Rewriting the hook's file is the edge: the change listeners hear it as from Windows.
    #[test]
    fn the_override_file_drives_the_edges() {
        let _flags = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = mpsc::channel();
        let token = subscribe_changes(move |c| {
            let _ = tx.send(c);
        });
        let dir = std::env::temp_dir().join(format!("foxglove-away-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("away.txt");
        std::fs::write(&file, "display off\n").unwrap();
        apply_override(&file);
        assert!(display_off());
        std::fs::write(&file, "display on\n").unwrap();
        apply_override(&file);
        std::fs::remove_file(&file).unwrap();
        apply_override(&file); // gone: present, no edge
        assert!(!away());
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [Change::DisplayOff, Change::DisplayOn]
        );
        drop(token);
        let _ = std::fs::remove_dir(dir);
    }

    /// The PC state (#609) tells a lock from the display going off: each edge once, even when
    /// [`away`] doesn't change with it.
    #[test]
    fn change_listeners_hear_every_edge_and_display_off_reads_the_display() {
        let _flags = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = mpsc::channel();
        let token = subscribe_changes(move |c| {
            let _ = tx.send(c);
        });
        set_display_off(true);
        assert!(display_off());
        set_locked(true); // away already: still an edge here
        set_display_off(true); // unchanged: nothing
        set_display_off(false);
        assert!(!display_off());
        set_locked(false);
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [
                Change::DisplayOff,
                Change::Locked,
                Change::DisplayOn,
                Change::Unlocked
            ]
        );
        drop(token);
        set_locked(true);
        set_locked(false);
        assert!(rx.try_iter().next().is_none());
    }
}
