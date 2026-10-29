//! Brings the overlay window back when Windows minimized it or left it off every display (#612).
//!
//! Windows minimizes the windows of a display that goes away (Settings > Display > "Minimize
//! windows when a monitor is disconnected"), and Show desktop minimizes the overlay with
//! everything else. The tray panel draws inside the overlay, so a minimized overlay means a tray
//! click opens nothing. Three events ask for a check, and the overlay's next frame runs it before
//! anything is drawn:
//! - a tray click ([`Trigger::TrayClick`], from the tray's click thread);
//! - a display change (`WM_DISPLAYCHANGE`, to a hidden window whose thread sleeps in
//!   `GetMessageW`), and the overlay becoming minimized within [`SETTLE`] of one, since Windows
//!   can minimize it after the broadcast;
//! - the taskbar starting (Explorer restarted), from the tray's taskbar event;
//! - an agent (#618): the local API's `overlay.restore` and `tray.set_panel`, which answer once
//!   the frame ran the check ([`request_and_wait`]). `overlay.window` reads the window's state
//!   ([`window_state`], the #612 probe) from the API's own thread.
//!
//! Nothing polls: each check is one event, and the settle window is a timestamp compared when
//! the minimize event arrives, not a timer.
//!
//! "Away" is minimized, or parked where Windows keeps minimized windows (-32000) without the
//! minimized flag: the #618 lab caught the overlay there, 160x28, mid-restore. The regular
//! placement ([`place_due`]) leaves an away window alone, so only these checks bring it back,
//! and a check restores any window that isn't on its chosen display's area ([`decide`]): onto
//! that area, never at the size it had while away. Every restore and every placement logs one
//! `Foxglove: overlay window …` line with the old and new rects and why.
//!
//! Why it was parked (#618, main 23941d8): winit only learns of a minimize from
//! `WM_SYSCOMMAND`, so a window minimized any other way (`ShowWindow`, as the lab and Show
//! desktop do) is still "not minimized" in winit's flags. The next winit flag change on the
//! overlay (the tray panel's mouse passthrough, when it opens or closes) rewrites the window's
//! style from those flags (winit 0.30 `WindowFlags::apply_diff`, `SetWindowLongW(GWL_STYLE)`),
//! which drops `WS_MINIMIZE`: the window is then no longer minimized, at -32000 and 160x28, with
//! nothing logged. The overlay's window hook ([`keep_minimized`]) keeps `WS_MINIMIZE` through
//! winit's style refresh, so only these checks bring it back. [`window_state`] waits for a
//! restore in progress, so the probe never reads a half-placed window.

// The window side runs on Windows only; elsewhere only the tests use the decision.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// What asked for the check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    TrayClick,
    DisplayChange,
    TaskbarRestart,
    /// The local API (#618): `overlay.restore`, or `tray.set_panel` opening the panel.
    Api,
}

impl Trigger {
    fn label(self) -> &'static str {
        match self {
            Trigger::TrayClick => "tray click",
            Trigger::DisplayChange => "display change",
            Trigger::TaskbarRestart => "taskbar restart",
            Trigger::Api => "api",
        }
    }
}

/// Why the window is restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Minimized,
    /// At -32000 without the minimized flag (see the module docs).
    Parked,
    OffEveryDisplay,
    /// On a display, but not covering the chosen display's area (e.g. 160x28 at 0,0).
    Misplaced,
}

impl Why {
    fn label(self) -> &'static str {
        match self {
            Why::Minimized => "minimized",
            Why::Parked => "parked off screen",
            Why::OffEveryDisplay => "off every display",
            Why::Misplaced => "not covering its display",
        }
    }
}

/// What the check does with the overlay window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// It's shown, covering its display's area (or on a live display when none is known).
    Keep,
    /// Un-minimize it without taking the focus, then cover `to` (physical pixels) when known.
    Restore { to: Option<[i32; 4]>, why: Why },
}

/// Where Windows parks minimized windows; a window whose corner is there is away.
const PARKED: i32 = -32000;

/// Minimized, or parked at -32000 without the flag (a minimize or restore in progress).
pub fn away(iconic: bool, rect: Option<[i32; 4]>) -> bool {
    iconic || rect.is_some_and(|r| r[0] <= PARKED && r[1] <= PARKED)
}

/// How many times the regular placement covers one area before leaving the window as it is.
pub const PLACEMENT_TRIES: u32 = 3;

/// Whether the regular placement (`keep_on_monitor`) should cover `target` now: never while the
/// window is away, since covering a minimized window un-minimizes it (#618: after the lab's
/// second minimize it left the overlay parked, then at 160x28, with no line); the checks here
/// bring an away window back.
pub fn place_due(iconic: bool, rect: Option<[i32; 4]>, target: [i32; 4], tries: u32) -> bool {
    !away(iconic, rect) && rect != Some(target) && tries < PLACEMENT_TRIES
}

const WS_MINIMIZE: u32 = 0x2000_0000;

/// The overlay window hook's answer to a style change (`WM_STYLECHANGING`, `GWL_STYLE`): while
/// the window is minimized, winit's style refresh (`in_winit_refresh`, bracketed by its
/// "Winit::SetRetainMaximized" message) keeps `WS_MINIMIZE` (the new style to use). Clearing it
/// would un-minimize the window without placing it (see the module docs). Any other change, and
/// any change while shown, goes through as asked.
pub fn keep_minimized(in_winit_refresh: bool, iconic: bool, old: u32, new: u32) -> Option<u32> {
    (in_winit_refresh && iconic && old & WS_MINIMIZE != 0 && new & WS_MINIMIZE == 0)
        .then_some(new | WS_MINIMIZE)
}

/// The log line when the hook kept the window minimized.
fn kept_minimized_line(rect: Option<[i32; 4]>) -> String {
    format!(
        "Foxglove: overlay window minimized at {}, kept minimized through a winit style refresh",
        rect_text(rect)
    )
}

/// How long after a display change the overlay becoming minimized counts as part of it.
pub const SETTLE: Duration = Duration::from_secs(10);

/// The restore decision. `rect` is the window's outer rectangle (None when Windows didn't give
/// one), `monitors` the live displays' areas and `target` the chosen display's area the overlay
/// covers. A window that is away, overlaps no live display, or doesn't cover `target` is
/// restored onto `target`, or onto the first display when the chosen one isn't known. The
/// window's own size never goes into `to`: while away it is a placeholder (160x28).
pub fn decide(
    iconic: bool,
    rect: Option<[i32; 4]>,
    monitors: &[[i32; 4]],
    target: Option<[i32; 4]>,
) -> Action {
    let on_a_display = rect.is_some_and(|r| monitors.iter().any(|m| overlaps(r, *m)));
    let why = if iconic {
        Why::Minimized
    } else if away(iconic, rect) {
        Why::Parked
    } else if !on_a_display {
        Why::OffEveryDisplay
    } else if target.is_some_and(|t| rect != Some(t)) {
        Why::Misplaced
    } else {
        return Action::Keep;
    };
    Action::Restore {
        to: target.or_else(|| monitors.first().copied()),
        why,
    }
}

/// Whether the overlay just going away (`was_away` → `away`, see [`away`]) belongs to a display
/// change `since_display_change` ago.
pub fn minimized_by_display_change(
    was_away: bool,
    away: bool,
    since_display_change: Option<Duration>,
) -> bool {
    !was_away && away && since_display_change.is_some_and(|d| d <= SETTLE)
}

/// One frame's check, window-free: the waiting trigger (or a display change's late minimize)
/// and what to do with the window. `was_away` is its state at the last frame, updated here.
pub fn check(
    pending: Option<Trigger>,
    was_away: &mut bool,
    iconic: bool,
    rect: Option<[i32; 4]>,
    since_display_change: Option<Duration>,
    monitors: &[[i32; 4]],
    target: Option<[i32; 4]>,
) -> Option<(Trigger, Action)> {
    let now_away = away(iconic, rect);
    let became_away = minimized_by_display_change(*was_away, now_away, since_display_change);
    *was_away = now_away;
    let trigger = pending.or(became_away.then_some(Trigger::DisplayChange))?;
    Some((trigger, decide(iconic, rect, monitors, target)))
}

fn overlaps(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The overlay window's state as [`window_state`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub iconic: bool,
    pub visible: bool,
    pub rect: Option<[i32; 4]>,
    /// The display it overlaps most.
    pub display: Option<String>,
    /// Shown, not minimized, and on a live display.
    pub on_screen: bool,
}

/// The overlay's state from what Windows says about it and the live displays: the display is
/// the one whose work area it overlaps most.
pub fn describe(
    iconic: bool,
    visible: bool,
    rect: Option<[i32; 4]>,
    monitors: &[crate::monitors::Monitor],
) -> WindowState {
    let area = |a: [i32; 4], b: [i32; 4]| {
        let w = i64::from(a[2].min(b[2]) - a[0].max(b[0]));
        let h = i64::from(a[3].min(b[3]) - a[1].max(b[1]));
        if w > 0 && h > 0 {
            w * h
        } else {
            0
        }
    };
    let display = rect.and_then(|r| {
        monitors
            .iter()
            .map(|m| (area(r, m.work_area), m))
            .filter(|(a, _)| *a > 0)
            .max_by_key(|(a, _)| *a)
            .map(|(_, m)| m.id.clone())
    });
    WindowState {
        iconic,
        visible,
        rect,
        on_screen: !iconic && visible && display.is_some(),
        display,
    }
}

/// The overlay window, from its first frame on (0 before).
static HWND: AtomicIsize = AtomicIsize::new(0);

/// The check waiting for the overlay's next frame (the first trigger wins until it's taken).
static PENDING: Mutex<Option<Trigger>> = Mutex::new(None);
/// Requests asked and checks run, so an agent's request is answered once its frame ran
/// ([`request_and_wait`]); sleeps on [`CHECKED`], no polling.
static SERVED: Mutex<(u64, u64)> = Mutex::new((0, 0));
static CHECKED: Condvar = Condvar::new();
/// The last display change.
static DISPLAY_CHANGED_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// Held by the frame while it restores the window (un-minimize, place, log): the probe waits for
/// it, so it never reads the window between two steps (#618: the minimized size moved onto the
/// display before its resize) or before the restore's line.
static RESTORING: Mutex<()> = Mutex::new(());

/// Runs `read` once no restore is in progress.
fn settled<T>(read: impl FnOnce() -> T) -> T {
    let _restoring = RESTORING.lock().unwrap_or_else(|e| e.into_inner());
    read()
}

/// The overlay window's handle, from its first frame on (for bringing it to the front).
pub fn hwnd() -> Option<isize> {
    Some(HWND.load(Ordering::Relaxed)).filter(|h| *h != 0)
}

/// Asks the overlay's next frame to check its window, and draws that frame.
pub fn request(trigger: Trigger) {
    if trigger == Trigger::DisplayChange {
        *DISPLAY_CHANGED_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert(trigger);
    SERVED.lock().unwrap_or_else(|e| e.into_inner()).0 += 1;
    super::tray_panel::wake();
    // The same events can lose device state too (#618): the re-apply hooks hear them.
    match trigger {
        Trigger::DisplayChange => super::reassert::fire(super::reassert::Trigger::DisplayChange),
        Trigger::TaskbarRestart => super::reassert::fire(super::reassert::Trigger::TaskbarRestart),
        Trigger::TrayClick | Trigger::Api => {}
    }
}

/// Every check asked so far was run (a frame took it): wakes the agents waiting.
fn checked() {
    let mut s = SERVED.lock().unwrap_or_else(|e| e.into_inner());
    s.1 = s.0;
    CHECKED.notify_all();
}

/// Asks for a check as [`request`] does, then sleeps until a frame ran it, at most `timeout`
/// (the fallback when no frame comes, e.g. the overlay isn't up). True when it ran.
pub fn request_and_wait(trigger: Trigger, timeout: Duration) -> bool {
    request(trigger);
    let s = SERVED.lock().unwrap_or_else(|e| e.into_inner());
    let n = s.0;
    let (s, _) = CHECKED
        .wait_timeout_while(s, timeout, |s| s.1 < n)
        .unwrap_or_else(|e| e.into_inner());
    s.1 >= n
}

/// The log line when an agent's check finds nothing to do.
fn kept_line(state: &WindowState) -> String {
    let rect = state
        .rect
        .map_or_else(|| "?".to_string(), |r| format!("{r:?}"));
    let display = state.display.as_deref().unwrap_or("no display");
    format!("Foxglove: overlay window shown at {rect} on {display}, nothing to restore (api)")
}

/// The overlay window's state now (the #612 probe), from any thread; None before its first
/// frame or off Windows.
pub fn window_state() -> Option<WindowState> {
    #[cfg(windows)]
    {
        let hwnd = HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return None;
        }
        let (iconic, visible, rect) = settled(|| {
            (
                imp::is_iconic(hwnd),
                imp::is_visible(hwnd),
                imp::window_rect(hwnd),
            )
        });
        Some(describe(iconic, visible, rect, &crate::monitors::list()))
    }
    #[cfg(not(windows))]
    {
        let _ = HWND.load(Ordering::Relaxed);
        None
    }
}

/// Whether Windows has `hwnd` minimized (for the regular placement, [`place_due`]).
#[cfg(windows)]
pub fn is_iconic(hwnd: isize) -> bool {
    imp::is_iconic(hwnd)
}

/// The waiting check, if any (clears it).
pub fn take() -> Option<Trigger> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

fn since_display_change() -> Option<Duration> {
    DISPLAY_CHANGED_AT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map(|at| at.elapsed())
}

fn rect_text(r: Option<[i32; 4]>) -> String {
    r.map_or_else(|| "?".to_string(), |r| format!("{r:?}"))
}

/// The log line for a restore: the old rect, the area asked for and, when Windows put the window
/// elsewhere, where it is now.
fn restored_line(
    trigger: Trigger,
    why: Why,
    from: Option<[i32; 4]>,
    to: Option<[i32; 4]>,
    now: Option<[i32; 4]>,
) -> String {
    let asked = to.map_or_else(|| "its restore rect".to_string(), |r| format!("{r:?}"));
    let landed = match (to, now) {
        (Some(to), Some(now)) if now != to => format!(", now at {now:?}"),
        _ => String::new(),
    };
    format!(
        "Foxglove: overlay window {} at {}, restored to {asked}{landed} ({})",
        why.label(),
        rect_text(from),
        trigger.label()
    )
}

/// The log line when the regular placement covers the chosen display's area (start, a changed
/// display, scaling or layout).
pub fn placed_line(from: Option<[i32; 4]>, to: [i32; 4], now: Option<[i32; 4]>) -> String {
    let landed = match now {
        Some(now) if now != to => format!(", now at {now:?}"),
        _ => String::new(),
    };
    format!(
        "Foxglove: overlay window at {}, placed at {to:?}{landed} (display area)",
        rect_text(from)
    )
}

/// The area the overlay covers on `monitor` (its chosen display, or the main one when that's
/// gone): the work area, one pixel short of the bottom like `keep_on_monitor` places it.
pub fn target(monitors: &[crate::monitors::Monitor], monitor: &str) -> Option<[i32; 4]> {
    crate::monitors::find(monitors, monitor).map(|m| {
        let [left, top, right, bottom] = m.work_area;
        [left, top, right, bottom - 1]
    })
}

/// Runs once per overlay frame, before anything is drawn. `was_away` is the window's state at
/// the last frame (updated here, see [`away`]); `monitor` is the chosen display; `place` covers
/// an area. When the window was restored: `Some` of the area it now covers (None when no display
/// is known). Displays are listed only when a check is due.
#[cfg(windows)]
pub fn frame(
    hwnd: isize,
    was_away: &mut bool,
    monitor: &str,
    place: impl Fn(isize, [i32; 4]),
) -> Option<Option<[i32; 4]>> {
    imp::watch();
    imp::hook(hwnd);
    HWND.store(hwnd, Ordering::Relaxed);
    let iconic = imp::is_iconic(hwnd);
    let rect = imp::window_rect(hwnd);
    let pending = take();
    let since = since_display_change();
    if pending.is_none() && !minimized_by_display_change(*was_away, away(iconic, rect), since) {
        *was_away = away(iconic, rect);
        return None;
    }
    let listed = crate::monitors::list();
    let monitors: Vec<[i32; 4]> = listed.iter().map(|m| m.work_area).collect();
    let checked_now = check(
        pending,
        was_away,
        iconic,
        rect,
        since,
        &monitors,
        target(&listed, monitor),
    );
    let (trigger, action) = checked_now?;
    let Action::Restore { to, why } = action else {
        if trigger == Trigger::Api {
            let state = describe(iconic, imp::is_visible(hwnd), rect, &listed);
            eprintln!("{}", kept_line(&state));
        }
        checked();
        return None;
    };
    let _restoring = RESTORING.lock().unwrap_or_else(|e| e.into_inner());
    imp::show_no_activate(hwnd);
    if let Some(to) = to {
        place(hwnd, to);
        // Un-minimizing can still be resizing the window after the first try (#618): once more.
        if imp::window_rect(hwnd) != Some(to) {
            place(hwnd, to);
        }
    }
    let now = imp::window_rect(hwnd);
    *was_away = away(imp::is_iconic(hwnd), now);
    eprintln!("{}", restored_line(trigger, why, rect, to, now));
    checked();
    Some(to)
}

#[cfg(windows)]
mod imp {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::{Once, OnceLock};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowRect, IsIconic,
        IsWindowVisible, RegisterClassW, RegisterWindowMessageW, ShowWindow, GWL_STYLE, MSG,
        STYLESTRUCT, SW_SHOWNOACTIVATE, WINDOW_STYLE, WM_DISPLAYCHANGE, WM_NCDESTROY,
        WM_STYLECHANGING, WNDCLASSW, WS_EX_TOOLWINDOW,
    };

    /// Our subclass of the overlay window ("FoxO").
    const HOOK: usize = 0x466F_784F;

    /// winit sends this registered message to a window with wparam 1 before its style refresh
    /// (`WindowFlags::apply_diff`) and 0 after (winit 0.30 `SET_RETAIN_STATE_ON_SIZE_MSG_ID`).
    fn winit_refresh_msg() -> u32 {
        static ID: OnceLock<u32> = OnceLock::new();
        // SAFETY: registers (or looks up) a message name; no pointers kept.
        *ID.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Winit::SetRetainMaximized")) })
    }

    /// Inside winit's style refresh of the overlay window (its thread only).
    static IN_REFRESH: AtomicBool = AtomicBool::new(false);

    /// Hooks the overlay window's messages (see `proc`), once per window.
    pub fn hook(h: isize) {
        static HOOKED: AtomicIsize = AtomicIsize::new(0);
        if HOOKED.swap(h, Ordering::Relaxed) == h {
            return;
        }
        let _ = winit_refresh_msg();
        // SAFETY: the overlay window belongs to this (the UI) thread, as subclassing requires.
        if !unsafe { SetWindowSubclass(hwnd(h), Some(proc), HOOK, 0) }.as_bool() {
            eprintln!("Foxglove: can't hook the overlay window (minimized through style changes)");
        }
    }

    /// The overlay window's messages, before winit's: notes winit's style-refresh bracket, and
    /// keeps `WS_MINIMIZE` through it while minimized ([`super::keep_minimized`]).
    unsafe extern "system" fn proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        // SAFETY (whole body): Windows calls this on the window's own thread with valid
        // arguments for `message`; `lparam` of WM_STYLECHANGING points at a STYLESTRUCT that
        // may be changed for the call.
        unsafe {
            if message == WM_STYLECHANGING && wparam.0 as i32 == GWL_STYLE.0 {
                let result = DefSubclassProc(window, message, wparam, lparam);
                let s = &mut *(lparam.0 as *mut STYLESTRUCT);
                let iconic = IsIconic(window).as_bool();
                let refresh = IN_REFRESH.load(Ordering::Relaxed);
                if let Some(kept) = super::keep_minimized(refresh, iconic, s.styleOld, s.styleNew) {
                    s.styleNew = kept;
                    // Not eprintln: a closed stderr must not panic into Windows.
                    let line = super::kept_minimized_line(window_rect(window.0 as isize));
                    let _ = writeln!(std::io::stderr(), "{line}");
                }
                return result;
            }
            let refresh_msg = winit_refresh_msg();
            if refresh_msg != 0 && message == refresh_msg {
                IN_REFRESH.store(wparam.0 != 0, Ordering::Relaxed);
            } else if message == WM_NCDESTROY {
                let _ = RemoveWindowSubclass(window, Some(proc), HOOK);
            }
            DefSubclassProc(window, message, wparam, lparam)
        }
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    pub fn is_iconic(h: isize) -> bool {
        // SAFETY: a state query on the overlay window, alive while its frames run.
        unsafe { IsIconic(hwnd(h)) }.as_bool()
    }

    pub fn is_visible(h: isize) -> bool {
        // SAFETY: a state query; a stale handle just answers false.
        unsafe { IsWindowVisible(hwnd(h)) }.as_bool()
    }

    pub fn window_rect(h: isize) -> Option<[i32; 4]> {
        let mut r = RECT::default();
        // SAFETY: fills a RECT this function owns.
        unsafe { GetWindowRect(hwnd(h), &mut r) }.ok()?;
        Some([r.left, r.top, r.right, r.bottom])
    }

    pub fn show_no_activate(h: isize) {
        // SAFETY: shows the overlay window without activating it.
        unsafe {
            let _ = ShowWindow(hwnd(h), SW_SHOWNOACTIVATE);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_DISPLAYCHANGE {
            // A panic must not unwind into Windows.
            let _ = std::panic::catch_unwind(|| super::request(super::Trigger::DisplayChange));
            return LRESULT(0);
        }
        // SAFETY: the default handling for this window's other messages.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// Follows display changes from the first overlay frame on. `WM_DISPLAYCHANGE` goes to
    /// top-level windows only, so this is a top-level window that is never shown; its thread
    /// sleeps in `GetMessageW` and ends with the process.
    pub fn watch() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            super::super::services::spawn_named("overlay-displays", || {
                let class = w!("FoxgloveOverlayDisplays");
                // SAFETY: window-class and window creation on this thread; the class name is a
                // static string, and the window lives as long as this thread's message loop.
                unsafe {
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(window_proc),
                        lpszClassName: class,
                        ..Default::default()
                    };
                    if RegisterClassW(&wc) == 0 {
                        eprintln!(
                            "Foxglove: the overlay can't follow display changes (no window class)"
                        );
                        return;
                    }
                    if CreateWindowExW(
                        WS_EX_TOOLWINDOW,
                        class,
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
                    )
                    .is_err()
                    {
                        eprintln!("Foxglove: the overlay can't follow display changes (no window)");
                        return;
                    }
                    let mut msg = MSG::default();
                    // 0 is WM_QUIT and -1 an error: both end the loop.
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        let _ = DispatchMessageW(&msg);
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: [i32; 4] = [0, 0, 2560, 1400];
    const GONE_4K: [i32; 4] = [2560, 0, 6400, 2159];
    const ICONIC_RECT: [i32; 4] = [-32000, -32000, -31840, -31972];

    #[test]
    fn a_shown_overlay_on_a_live_display_is_left_alone() {
        assert_eq!(decide(false, Some(MAIN), &[MAIN], Some(MAIN)), Action::Keep);
        // Partly on a display still counts as on it when no chosen area is known.
        let straddling = [2000, 0, 3000, 1400];
        assert_eq!(decide(false, Some(straddling), &[MAIN], None), Action::Keep);
    }

    #[test]
    fn a_minimized_overlay_is_restored_onto_the_chosen_display() {
        // The #612 capture: minimized at -32000 after the 4K display went away.
        assert_eq!(
            decide(true, Some(ICONIC_RECT), &[MAIN], Some(MAIN)),
            Action::Restore {
                to: Some(MAIN),
                why: Why::Minimized
            }
        );
        // Minimized while its rect is still on a display (Show desktop): restored too.
        assert_eq!(
            decide(true, Some(MAIN), &[MAIN], Some(MAIN)),
            Action::Restore {
                to: Some(MAIN),
                why: Why::Minimized
            }
        );
    }

    #[test]
    fn an_overlay_left_on_a_removed_display_is_moved_back() {
        assert_eq!(
            decide(false, Some(GONE_4K), &[MAIN], Some(MAIN)),
            Action::Restore {
                to: Some(MAIN),
                why: Why::OffEveryDisplay
            }
        );
        // No chosen area known: the first live display.
        assert_eq!(
            decide(false, Some(GONE_4K), &[MAIN], None),
            Action::Restore {
                to: Some(MAIN),
                why: Why::OffEveryDisplay
            }
        );
        // No rect from Windows counts as off every display.
        assert_eq!(
            decide(false, None, &[MAIN], Some(MAIN)),
            Action::Restore {
                to: Some(MAIN),
                why: Why::OffEveryDisplay
            }
        );
    }

    #[test]
    fn the_target_is_the_chosen_display_or_the_main_one_when_it_is_gone() {
        let monitor = |id: &str, primary, work_area| crate::monitors::Monitor {
            id: id.into(),
            label: id.into(),
            primary,
            work_area,
            dpi: 96,
        };
        let main = monitor(r"\\.\DISPLAY6", true, [0, 0, 2560, 1440]);
        let side = monitor(r"\\.\DISPLAY7", false, [2560, 0, 4480, 1080]);
        let both = [main.clone(), side];
        assert_eq!(target(&both, r"\\.\DISPLAY7"), Some([2560, 0, 4480, 1079]));
        // The #612 case: the saved display is gone, so the main one.
        assert_eq!(target(&[main], r"\\.\DISPLAY5"), Some([0, 0, 2560, 1439]));
        assert_eq!(target(&[], ""), None);
    }

    #[test]
    fn with_no_display_known_it_is_only_un_minimized() {
        assert_eq!(
            decide(true, Some(ICONIC_RECT), &[], None),
            Action::Restore {
                to: None,
                why: Why::Minimized
            }
        );
    }

    #[test]
    fn edges_that_only_touch_are_not_on_the_display() {
        let right_of_main = [2560, 0, 3000, 1400];
        assert!(!overlaps(right_of_main, MAIN));
        assert_eq!(
            decide(false, Some(right_of_main), &[MAIN], Some(MAIN)),
            Action::Restore {
                to: Some(MAIN),
                why: Why::OffEveryDisplay
            }
        );
    }

    #[test]
    fn a_minimize_counts_as_a_display_change_only_just_after_one() {
        let just = Some(Duration::from_secs(1));
        assert!(minimized_by_display_change(false, true, just));
        assert!(minimized_by_display_change(false, true, Some(SETTLE)));
        // Too long after (Show desktop later on), or no display change at all: left alone.
        assert!(!minimized_by_display_change(
            false,
            true,
            Some(SETTLE + Duration::from_secs(1))
        ));
        assert!(!minimized_by_display_change(false, true, None));
        // Only the transition counts, not staying minimized or being restored.
        assert!(!minimized_by_display_change(true, true, just));
        assert!(!minimized_by_display_change(true, false, just));
    }

    /// The tests that take requests share PENDING: one at a time.
    static REQUESTS: Mutex<()> = Mutex::new(());

    #[test]
    fn a_request_is_taken_once_and_the_first_trigger_wins() {
        let _one = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take();
        request(Trigger::TrayClick);
        request(Trigger::TaskbarRestart);
        assert_eq!(take(), Some(Trigger::TrayClick));
        assert_eq!(take(), None, "taken once");
        request(Trigger::DisplayChange);
        assert!(since_display_change().is_some_and(|d| d <= SETTLE));
        assert_eq!(take(), Some(Trigger::DisplayChange));
    }

    fn monitor(id: &str, work_area: [i32; 4]) -> crate::monitors::Monitor {
        crate::monitors::Monitor {
            id: id.into(),
            label: id.into(),
            primary: false,
            work_area,
            dpi: 96,
        }
    }

    /// #618: the window probe as the API reads it: minimized at -32000 is on no display, a
    /// window straddling two displays is on the one it covers most, a hidden one isn't on screen.
    #[test]
    fn the_window_state_names_the_display_it_covers_most() {
        let both = [
            monitor(r"\\.\DISPLAY1", MAIN),
            monitor(r"\\.\DISPLAY2", GONE_4K),
        ];
        let s = describe(true, true, Some(ICONIC_RECT), &both);
        assert_eq!((s.display, s.on_screen), (None, false));
        let s = describe(false, true, Some(MAIN), &both);
        assert_eq!(s.display.as_deref(), Some(r"\\.\DISPLAY1"));
        assert!(s.on_screen);
        let mostly_right = [2000, 0, 4000, 1400];
        let s = describe(false, true, Some(mostly_right), &both);
        assert_eq!(s.display.as_deref(), Some(r"\\.\DISPLAY2"));
        assert!(
            !describe(false, false, Some(MAIN), &both).on_screen,
            "hidden"
        );
        assert!(!describe(false, true, None, &both).on_screen, "no rect");
        assert!(
            !describe(false, true, Some(GONE_4K), &both[..1]).on_screen,
            "display gone"
        );
        assert_eq!(
            kept_line(&describe(false, true, Some(MAIN), &both)),
            "Foxglove: overlay window shown at [0, 0, 2560, 1400] on \\\\.\\DISPLAY1, nothing to restore (api)"
        );
    }

    /// An agent's request is answered once a frame ran the check, or at the timeout without one.
    #[test]
    fn an_agent_check_is_answered_when_the_frame_ran_it() {
        let _one = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take();
        // Taken and run by a "frame" on another thread.
        let frame = std::thread::spawn(|| {
            for _ in 0..200 {
                if take() == Some(Trigger::Api) {
                    checked();
                    return true;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            false
        });
        assert!(request_and_wait(Trigger::Api, Duration::from_secs(10)));
        assert!(frame.join().unwrap());
        assert!(
            !request_and_wait(Trigger::Api, Duration::from_millis(20)),
            "no frame: timeout"
        );
        let _ = take();
        checked();
    }

    #[test]
    fn the_log_line_names_the_state_the_rects_and_the_trigger() {
        assert_eq!(
            restored_line(
                Trigger::DisplayChange,
                Why::Minimized,
                Some(ICONIC_RECT),
                Some(MAIN),
                Some(MAIN)
            ),
            "Foxglove: overlay window minimized at [-32000, -32000, -31840, -31972], \
             restored to [0, 0, 2560, 1400] (display change)"
        );
        assert_eq!(
            restored_line(
                Trigger::TrayClick,
                Why::OffEveryDisplay,
                Some(GONE_4K),
                None,
                None
            ),
            "Foxglove: overlay window off every display at [2560, 0, 6400, 2159], \
             restored to its restore rect (tray click)"
        );
        // Windows put it somewhere else: the line says where.
        assert_eq!(
            restored_line(
                Trigger::Api,
                Why::Misplaced,
                Some(SMALL),
                Some(MAIN),
                Some(SMALL)
            ),
            "Foxglove: overlay window not covering its display at [0, 0, 160, 28], \
             restored to [0, 0, 2560, 1400], now at [0, 0, 160, 28] (api)"
        );
        assert_eq!(
            placed_line(Some([0, 0, 2576, 1416]), MAIN, Some(MAIN)),
            "Foxglove: overlay window at [0, 0, 2576, 1416], placed at [0, 0, 2560, 1400] \
             (display area)"
        );
    }

    /// The minimized size moved onto the display: what the #618 lab read after the display
    /// change on 9355cf2.
    const SMALL: [i32; 4] = [0, 0, 160, 28];

    /// #618: parked at -32000 without the minimized flag is away; anywhere else isn't.
    #[test]
    fn a_window_parked_at_minus_32000_is_away_even_when_not_flagged_minimized() {
        assert!(away(true, Some(MAIN)));
        assert!(away(false, Some(ICONIC_RECT)));
        assert!(!away(false, Some(MAIN)));
        assert!(!away(false, Some(SMALL)));
        assert!(!away(false, Some(GONE_4K)));
        assert!(!away(false, None));
        assert_eq!(
            decide(false, Some(ICONIC_RECT), &[MAIN], Some(MAIN)),
            Action::Restore {
                to: Some(MAIN),
                why: Why::Parked
            }
        );
    }

    /// #618: on a display at the wrong size (the 160x28 placeholder) is restored onto the
    /// chosen display's area, never kept at that size.
    #[test]
    fn a_placeholder_size_on_a_display_is_restored_to_the_full_area() {
        assert_eq!(
            decide(false, Some(SMALL), &[MAIN], Some(MAIN)),
            Action::Restore {
                to: Some(MAIN),
                why: Why::Misplaced
            }
        );
        // The area restored to comes from the displays, never from the window's own rect.
        for rect in [Some(SMALL), Some(ICONIC_RECT), None] {
            for iconic in [false, true] {
                if let Action::Restore { to, .. } = decide(iconic, rect, &[MAIN], Some(MAIN)) {
                    assert_eq!(to, Some(MAIN), "{iconic} {rect:?}");
                }
            }
        }
    }

    /// #618: the regular placement never covers a minimized or parked window (that un-minimized
    /// it behind the checks' back), and stops after its tries.
    #[test]
    fn the_regular_placement_leaves_an_away_window_to_the_checks() {
        assert!(!place_due(true, Some(ICONIC_RECT), MAIN, 1), "minimized");
        assert!(!place_due(true, Some(MAIN), MAIN, 0), "minimized in place");
        assert!(!place_due(false, Some(ICONIC_RECT), MAIN, 1), "parked");
        assert!(place_due(false, Some(SMALL), MAIN, 1));
        assert!(place_due(false, Some(GONE_4K), MAIN, 0));
        assert!(!place_due(false, Some(MAIN), MAIN, 0), "already there");
        assert!(
            !place_due(false, Some(SMALL), MAIN, PLACEMENT_TRIES),
            "out of tries"
        );
    }

    /// A window-free stand-in for the overlay window and its frame loop: the checks and the
    /// regular placement as `frame` and `keep_on_monitor` run them, with the window's state set
    /// by the test the way Windows set it in the #618 lab run. Restoring or placing covers the
    /// area asked for; every restore and placement records its line.
    struct Sim {
        iconic: bool,
        rect: Option<[i32; 4]>,
        was_away: bool,
        tries: u32,
        lines: Vec<String>,
    }

    impl Sim {
        fn frame(&mut self, pending: Option<Trigger>, since: Option<Duration>) {
            let (iconic, rect) = (self.iconic, self.rect);
            if let Some((trigger, Action::Restore { to, why })) = check(
                pending,
                &mut self.was_away,
                iconic,
                rect,
                since,
                &[MAIN],
                Some(MAIN),
            ) {
                (self.iconic, self.rect) = (false, to);
                self.was_away = away(self.iconic, self.rect);
                self.tries = 1;
                self.lines
                    .push(restored_line(trigger, why, rect, to, self.rect));
            }
            if place_due(self.iconic, self.rect, MAIN, self.tries) {
                self.tries += 1;
                let from = self.rect;
                (self.iconic, self.rect) = (false, Some(MAIN));
                self.lines.push(placed_line(from, MAIN, self.rect));
            }
        }
    }

    /// #618, the 9355cf2 lab sequence: a tray-click restore, a second minimize that Windows left
    /// parked (not flagged minimized, -32000) and then at 160x28, and a display change within
    /// SETTLE. Every restore ends covering the display's area with one line each, and nothing
    /// un-minimizes the window without a line.
    #[test]
    fn the_lab_sequence_ends_at_the_full_area_with_a_line_per_restore() {
        let mut w = Sim {
            iconic: false,
            rect: Some(MAIN),
            was_away: false,
            tries: PLACEMENT_TRIES,
            lines: vec![],
        };
        w.frame(None, None);
        assert!(w.lines.is_empty(), "shown and in place: nothing");

        // The lab minimizes; a tray click restores it.
        (w.iconic, w.rect) = (true, Some(ICONIC_RECT));
        w.frame(None, None);
        assert!(w.lines.is_empty(), "no check yet");
        w.frame(Some(Trigger::TrayClick), None);
        assert_eq!((w.iconic, w.rect), (false, Some(MAIN)));
        assert_eq!(w.lines.len(), 1);
        assert!(
            w.lines[0].contains("minimized at [-32000") && w.lines[0].ends_with("(tray click)")
        );

        // The second minimize: frames keep running, and the placement (tries reset to 1 by the
        // restore) must not touch it.
        (w.iconic, w.rect) = (true, Some(ICONIC_RECT));
        for _ in 0..PLACEMENT_TRIES {
            w.frame(None, None);
        }
        assert_eq!(
            (w.iconic, w.rect),
            (true, Some(ICONIC_RECT)),
            "left minimized"
        );
        assert_eq!(w.lines.len(), 1, "no silent restore");

        // Windows' intermediate states: parked without the flag, then the placeholder at 0,0.
        (w.iconic, w.rect) = (false, Some(ICONIC_RECT));
        w.frame(None, None);
        assert_eq!(w.rect, Some(ICONIC_RECT), "parked: left to the checks");
        assert_eq!(w.lines.len(), 1);

        // The display change, 1 s after: restored from parked to the full area, with its line.
        let just = Some(Duration::from_secs(1));
        w.frame(Some(Trigger::DisplayChange), just);
        assert_eq!((w.iconic, w.rect), (false, Some(MAIN)));
        assert_eq!(w.lines.len(), 2);
        assert!(
            w.lines[1].contains("parked off screen at [-32000"),
            "{}",
            w.lines[1]
        );
        assert!(w.lines[1].ends_with("restored to [0, 0, 2560, 1400] (display change)"));

        // Had Windows left it at 160x28 on the display instead: still restored, with a line.
        w.rect = Some(SMALL);
        w.frame(Some(Trigger::DisplayChange), just);
        assert_eq!(w.rect, Some(MAIN));
        assert_eq!(w.lines.len(), 3);
        assert!(w.lines[2].contains("not covering its display at [0, 0, 160, 28]"));
        assert!(w.lines[2].ends_with("(display change)"));

        // Parked within SETTLE of a display change with no request: the late minimize counts.
        w.rect = Some(ICONIC_RECT);
        w.frame(None, just);
        assert_eq!(w.rect, Some(MAIN));
        assert_eq!(w.lines.len(), 4);
        assert!(
            w.lines[3].contains("parked off screen") && w.lines[3].ends_with("(display change)")
        );

        // The API's overlay.restore takes the same path: the placeholder comes back, one line.
        w.rect = Some(SMALL);
        w.frame(Some(Trigger::Api), None);
        assert_eq!(w.rect, Some(MAIN));
        assert_eq!(w.lines.len(), 5);
        assert!(w.lines[4].ends_with("(api)"));

        // And in place, a check does nothing (the API logs its own "nothing to restore").
        w.frame(Some(Trigger::TrayClick), None);
        assert_eq!(w.lines.len(), 5);
        assert!(w
            .lines
            .iter()
            .all(|l| l.starts_with("Foxglove: overlay window ")));
    }

    /// The regular placement's own moves are logged too (one per try).
    #[test]
    fn a_placement_after_a_display_layout_change_logs_its_line() {
        let mut w = Sim {
            iconic: false,
            rect: Some([0, 0, 2576, 1416]),
            was_away: false,
            tries: 0,
            lines: vec![],
        };
        w.frame(None, None);
        assert_eq!(w.rect, Some(MAIN));
        assert_eq!(w.lines.len(), 1);
        assert!(w.lines[0].ends_with("(display area)"));
    }

    const WS_POPUP: u32 = 0x8000_0000;

    /// #618: winit's style refresh keeps `WS_MINIMIZE` on a minimized overlay; nothing else is
    /// touched.
    #[test]
    fn a_winit_style_refresh_keeps_a_minimized_window_minimized() {
        let min = WS_POPUP | WS_MINIMIZE;
        assert_eq!(keep_minimized(true, true, min, WS_POPUP), Some(min));
        assert_eq!(
            keep_minimized(false, true, min, WS_POPUP),
            None,
            "not winit's refresh"
        );
        assert_eq!(
            keep_minimized(true, false, WS_POPUP, WS_POPUP),
            None,
            "shown"
        );
        assert_eq!(keep_minimized(true, true, min, min), None, "kept anyway");
        assert_eq!(
            kept_minimized_line(Some(ICONIC_RECT)),
            "Foxglove: overlay window minimized at [-32000, -32000, -31840, -31972], kept \
             minimized through a winit style refresh"
        );
    }

    /// A window-free model of the overlay window as Windows and winit 0.30 hold it, for the
    /// main 23941d8 lab run (#618). Windows: the `WS_MINIMIZE` style (what `IsIconic` reads), the
    /// rect, and the normal rect a minimized window is restored to. winit: its own `MINIMIZED`
    /// flag, set only by `WM_SYSCOMMAND` or its own `set_minimized`, never by `ShowWindow`. The
    /// frame runs the real `check`/`decide`, un-minimizes like `SW_SHOWNOACTIVATE` and places
    /// like `place_window` (move with `SWP_NOSIZE`, then size), recording what a probe on another
    /// thread could read between the steps.
    struct Model {
        minimized_style: bool,
        rect: [i32; 4],
        normal: [i32; 4],
        winit_minimized: bool,
        passthrough: bool,
        hooked: bool,
        was_away: bool,
        between_steps: Vec<[i32; 4]>,
        lines: Vec<String>,
    }

    impl Model {
        fn shown() -> Self {
            Model {
                minimized_style: false,
                rect: MAIN,
                normal: MAIN,
                winit_minimized: false,
                passthrough: true,
                hooked: false,
                was_away: false,
                between_steps: vec![],
                lines: vec![],
            }
        }

        fn probe(&self) -> (bool, [i32; 4]) {
            (self.minimized_style, self.rect)
        }

        /// The lab hook: `ShowWindow(SW_MINIMIZE)` from another process (no `WM_SYSCOMMAND`,
        /// so winit's flag stays "not minimized").
        fn lab_minimize(&mut self) {
            if !self.minimized_style {
                self.normal = self.rect;
            }
            (self.minimized_style, self.rect) = (true, ICONIC_RECT);
        }

        /// egui's `MousePassthrough` → winit `set_cursor_hittest` → `apply_diff`: inside the
        /// "SetRetainMaximized" bracket the style is rewritten from winit's flags (skipped only
        /// while winit itself thinks the window minimized), through the overlay's hook when
        /// hooked.
        fn set_passthrough(&mut self, on: bool) {
            if self.passthrough == on {
                return;
            }
            self.passthrough = on;
            let old = WS_POPUP | if self.minimized_style { WS_MINIMIZE } else { 0 };
            let mut new = WS_POPUP | if self.winit_minimized { WS_MINIMIZE } else { 0 };
            if self.winit_minimized {
                return;
            }
            if self.hooked {
                if let Some(kept) = keep_minimized(true, self.minimized_style, old, new) {
                    new = kept;
                    self.lines.push(kept_minimized_line(Some(self.rect)));
                }
            }
            // The style no longer says minimized: the window stays where it was (-32000, at the
            // minimized size), shown.
            self.minimized_style = new & WS_MINIMIZE != 0;
        }

        fn frame(&mut self, pending: Option<Trigger>, since: Option<Duration>) {
            let (iconic, rect) = (self.minimized_style, Some(self.rect));
            let Some((trigger, Action::Restore { to, why })) = check(
                pending,
                &mut self.was_away,
                iconic,
                rect,
                since,
                &[MAIN],
                Some(MAIN),
            ) else {
                return;
            };
            if self.minimized_style {
                (self.minimized_style, self.rect) = (false, self.normal);
            }
            let to = to.expect("a display is known");
            // place_window: moved first (SWP_NOSIZE), then sized.
            self.rect = [
                to[0],
                to[1],
                to[0] + self.rect[2] - self.rect[0],
                to[1] + self.rect[3] - self.rect[1],
            ];
            self.between_steps.push(self.rect);
            self.rect = to;
            self.was_away = away(self.minimized_style, Some(self.rect));
            self.lines
                .push(restored_line(trigger, why, rect, Some(to), Some(self.rect)));
        }

        /// The run: minimize, tray click, panel open, panel closed (its passthrough change lands
        /// after the lab's second minimize: the API answers mid-frame, the command runs at the
        /// frame's end), then a display change 1 s later.
        fn lab_run(hooked: bool) -> (Model, (bool, [i32; 4])) {
            let mut w = Model {
                hooked,
                ..Model::shown()
            };
            w.lab_minimize();
            assert_eq!(w.probe(), (true, ICONIC_RECT), "first minimize: iconic");
            w.frame(Some(Trigger::TrayClick), None);
            assert_eq!(w.probe(), (false, MAIN), "tray click: restored");
            w.set_passthrough(false); // the panel opens and takes the mouse
            w.lab_minimize(); // right after tray.set_panel close answered
            w.set_passthrough(true); // the close's passthrough, at the frame's end
            let after_second_minimize = w.probe();
            w.frame(Some(Trigger::DisplayChange), Some(Duration::from_secs(1)));
            (w, after_second_minimize)
        }
    }

    /// #618 on main 23941d8, derived: without the hook, winit's style refresh (the panel's
    /// passthrough) clears `WS_MINIMIZE` after the lab's second minimize. That is On-call's
    /// "iconic False, rect -32000…", and the display change's restore then moves the 160x28
    /// window to 0,0 before sizing it, which is the `[0, 0, 160, 28]` the probe read before the
    /// restore's line was out.
    #[test]
    fn without_the_hook_the_lab_run_reads_as_on_call_saw_it() {
        let (w, second) = Model::lab_run(false);
        assert_eq!(second, (false, ICONIC_RECT), "un-minimized, still parked");
        assert_eq!(w.between_steps[1], SMALL, "the minimized size at 0,0");
        assert!(w.lines[1].starts_with("Foxglove: overlay window parked off screen"));
    }

    /// The fix: the hook keeps the second minimize a minimize (one line), and the display change
    /// restores it as a minimized window: un-minimized to its full size, placed, one line, never
    /// at 160x28.
    #[test]
    fn with_the_hook_the_lab_run_stays_minimized_then_restores_in_full() {
        let (w, second) = Model::lab_run(true);
        assert_eq!(second, (true, ICONIC_RECT), "the second minimize holds");
        assert_eq!(w.probe(), (false, MAIN));
        assert!(!w.between_steps.contains(&SMALL), "{:?}", w.between_steps);
        assert_eq!(
            w.lines,
            [
                "Foxglove: overlay window minimized at [-32000, -32000, -31840, -31972], \
                 restored to [0, 0, 2560, 1400] (tray click)",
                "Foxglove: overlay window minimized at [-32000, -32000, -31840, -31972], kept \
                 minimized through a winit style refresh",
                "Foxglove: overlay window minimized at [-32000, -32000, -31840, -31972], \
                 restored to [0, 0, 2560, 1400] (display change)",
            ]
        );
    }

    /// The probe (`window_state`) waits for a restore in progress, so it never reads the window
    /// between `place_window`'s two steps or before the restore's line.
    #[test]
    fn the_probe_waits_for_a_restore_in_progress() {
        let restoring = RESTORING.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = std::sync::mpsc::channel();
        let probe = std::thread::spawn(move || {
            let _ = tx.send(settled(|| 7));
        });
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "waits while restoring"
        );
        drop(restoring);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(7));
        probe.join().unwrap();
    }
}
