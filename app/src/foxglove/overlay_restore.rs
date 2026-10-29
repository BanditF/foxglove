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

/// What the check does with the overlay window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// It's shown on a live display: leave it.
    Keep,
    /// Un-minimize it without taking the focus, then cover `to` (physical pixels) when known.
    Restore { to: Option<[i32; 4]> },
}

/// How long after a display change the overlay becoming minimized counts as part of it.
pub const SETTLE: Duration = Duration::from_secs(10);

/// The restore decision. `rect` is the window's outer rectangle (None when Windows didn't give
/// one), `monitors` the live displays' areas and `target` the chosen display's area the overlay
/// covers. A minimized window, or one that overlaps no live display, is restored onto `target`,
/// or onto the first display when the chosen one isn't known.
pub fn decide(
    iconic: bool,
    rect: Option<[i32; 4]>,
    monitors: &[[i32; 4]],
    target: Option<[i32; 4]>,
) -> Action {
    let on_a_display = rect.is_some_and(|r| monitors.iter().any(|m| overlaps(r, *m)));
    if !iconic && on_a_display {
        return Action::Keep;
    }
    Action::Restore {
        to: target.or_else(|| monitors.first().copied()),
    }
}

/// Whether the overlay just becoming minimized (`was_iconic` → `iconic`) belongs to a display
/// change `since_display_change` ago.
pub fn minimized_by_display_change(
    was_iconic: bool,
    iconic: bool,
    since_display_change: Option<Duration>,
) -> bool {
    !was_iconic && iconic && since_display_change.is_some_and(|d| d <= SETTLE)
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
        Some(describe(
            imp::is_iconic(hwnd),
            imp::is_visible(hwnd),
            imp::window_rect(hwnd),
            &crate::monitors::list(),
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = HWND.load(Ordering::Relaxed);
        None
    }
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

/// The log line for a restore.
fn restored_line(
    trigger: Trigger,
    iconic: bool,
    from: Option<[i32; 4]>,
    to: Option<[i32; 4]>,
) -> String {
    let state = if iconic {
        "minimized"
    } else {
        "off every display"
    };
    let from = from.map_or_else(|| "?".to_string(), |r| format!("{r:?}"));
    let to = to.map_or_else(|| "its restore rect".to_string(), |r| format!("{r:?}"));
    format!(
        "Foxglove: overlay window {state} at {from}, restored to {to} ({})",
        trigger.label()
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

/// Runs once per overlay frame, before anything is drawn. `was_iconic` is the window's state at
/// the last frame (updated here); `monitor` is the chosen display; `place` covers an area.
/// When the window was restored: `Some` of the area it now covers (None when no display is
/// known). Displays are listed only when a check is due.
#[cfg(windows)]
pub fn frame(
    hwnd: isize,
    was_iconic: &mut bool,
    monitor: &str,
    place: impl FnOnce(isize, [i32; 4]),
) -> Option<Option<[i32; 4]>> {
    imp::watch();
    HWND.store(hwnd, Ordering::Relaxed);
    let iconic = imp::is_iconic(hwnd);
    let became_minimized = minimized_by_display_change(*was_iconic, iconic, since_display_change());
    *was_iconic = iconic;
    let trigger = take().or(became_minimized.then_some(Trigger::DisplayChange))?;
    let rect = imp::window_rect(hwnd);
    let listed = crate::monitors::list();
    let monitors: Vec<[i32; 4]> = listed.iter().map(|m| m.work_area).collect();
    let Action::Restore { to } = decide(iconic, rect, &monitors, target(&listed, monitor)) else {
        if trigger == Trigger::Api {
            let state = describe(iconic, imp::is_visible(hwnd), rect, &listed);
            eprintln!("{}", kept_line(&state));
        }
        checked();
        return None;
    };
    imp::show_no_activate(hwnd);
    if let Some(to) = to {
        place(hwnd, to);
    }
    *was_iconic = imp::is_iconic(hwnd);
    eprintln!("{}", restored_line(trigger, iconic, rect, to));
    checked();
    Some(to)
}

#[cfg(windows)]
mod imp {
    use std::sync::Once;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowRect, IsIconic,
        IsWindowVisible, RegisterClassW, ShowWindow, MSG, SW_SHOWNOACTIVATE, WINDOW_STYLE,
        WM_DISPLAYCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
    };

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
        // Partly on a display still counts as on it.
        let straddling = [2000, 0, 3000, 1400];
        assert_eq!(
            decide(false, Some(straddling), &[MAIN], Some(MAIN)),
            Action::Keep
        );
    }

    #[test]
    fn a_minimized_overlay_is_restored_onto_the_chosen_display() {
        // The #612 capture: minimized at -32000 after the 4K display went away.
        assert_eq!(
            decide(true, Some(ICONIC_RECT), &[MAIN], Some(MAIN)),
            Action::Restore { to: Some(MAIN) }
        );
        // Minimized while its rect is still on a display (Show desktop): restored too.
        assert_eq!(
            decide(true, Some(MAIN), &[MAIN], Some(MAIN)),
            Action::Restore { to: Some(MAIN) }
        );
    }

    #[test]
    fn an_overlay_left_on_a_removed_display_is_moved_back() {
        assert_eq!(
            decide(false, Some(GONE_4K), &[MAIN], Some(MAIN)),
            Action::Restore { to: Some(MAIN) }
        );
        // No chosen area known: the first live display.
        assert_eq!(
            decide(false, Some(GONE_4K), &[MAIN], None),
            Action::Restore { to: Some(MAIN) }
        );
        // No rect from Windows counts as off every display.
        assert_eq!(
            decide(false, None, &[MAIN], Some(MAIN)),
            Action::Restore { to: Some(MAIN) }
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
            Action::Restore { to: None }
        );
    }

    #[test]
    fn edges_that_only_touch_are_not_on_the_display() {
        let right_of_main = [2560, 0, 3000, 1400];
        assert!(!overlaps(right_of_main, MAIN));
        assert_eq!(
            decide(false, Some(right_of_main), &[MAIN], Some(MAIN)),
            Action::Restore { to: Some(MAIN) }
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
            restored_line(Trigger::DisplayChange, true, Some(ICONIC_RECT), Some(MAIN)),
            "Foxglove: overlay window minimized at [-32000, -32000, -31840, -31972], \
             restored to [0, 0, 2560, 1400] (display change)"
        );
        assert_eq!(
            restored_line(Trigger::TrayClick, false, Some(GONE_4K), None),
            "Foxglove: overlay window off every display at [2560, 0, 6400, 2159], \
             restored to its restore rect (tray click)"
        );
    }
}
