// SPDX-License-Identifier: MIT
//! The tray flyout (#991): a small hub-owned window above the tray icon that holds the tray
//! panel's tile grid while the main window stays hidden. This module is its pure part: where the
//! flyout goes for a taskbar on any edge, when a tray click opens or closes it, and the flag the
//! event loop reads (`wake::Gate`). The window itself is drawn by `shell`, placed by `platform`.
//!
//! All rectangles are physical pixels in virtual screen coordinates.

use crate::placement::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The flyout's width in points: two tiles and the panel's margins.
pub const WIDTH: f32 = 2.0 * 250.0 + 8.0 + 2.0 * 14.0;
/// The height it opens with, in points, before its first frame measures the grid.
pub const FIRST_HEIGHT: f32 = 320.0;
/// The gap between the flyout and the taskbar or the screen edge, in points.
pub const GAP: f32 = 12.0;
/// A tray click this soon after the flyout closed because the click took its focus is the same
/// click: it leaves the flyout closed (a toggle), instead of opening it again.
pub const TOGGLE_WINDOW: Duration = Duration::from_millis(400);
/// How long a shown flyout may go without ever getting the focus. Windows refuses the focus
/// hand-off when Hideout may not take the foreground (no recent input of its own), and such a
/// flyout could never close on focus loss: it would stay up, and keep the hidden hub drawing,
/// for good (#1000).
pub const FOCUS_GRACE: Duration = Duration::from_millis(750);

/// What the flyout does about the focus it should have got when it was shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusCheck {
    /// It has (or had) the focus, or its window can't say (no own window): nothing to do.
    Fine,
    /// Not yet: look again in this long (one wake, only while the flyout is up).
    LookIn(Duration),
    /// The hand-off failed: close it.
    Close,
}

/// The focus check for a flyout shown at `shown_at` (`None` before it is shown) that never had
/// the focus, now seen with `focused` (`None` when its window can't say, as in tests).
pub fn focus_check(
    had_focus: bool,
    focused: Option<bool>,
    shown_at: Option<Instant>,
    now: Instant,
) -> FocusCheck {
    let Some(shown) = shown_at else {
        return FocusCheck::Fine;
    };
    if had_focus || focused != Some(false) {
        return FocusCheck::Fine;
    }
    let up = now.saturating_duration_since(shown);
    if up >= FOCUS_GRACE {
        FocusCheck::Close
    } else {
        FocusCheck::LookIn(FOCUS_GRACE - up)
    }
}

/// Whether the flyout must be placed (sized) again for a grid now `height` points tall, last
/// placed at `placed` points, at `ppp` physical pixels per point: only for a change of at least
/// one whole pixel. Each placing resizes the window and the resize brings another frame, so a
/// sub-pixel difference (a fractional scale rounding back and forth) would loop (#1000).
pub fn needs_place(placed: Option<f32>, height: f32, ppp: f32) -> bool {
    placed.is_none_or(|h| ((h - height) * ppp).abs() >= 1.0)
}

/// The screen edge the taskbar is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

/// The taskbar's edge: the side where the work area is short of the monitor. With the taskbar
/// set to hide itself the work area is the whole monitor, so the edge the icon is nearest wins.
pub fn taskbar_edge(icon: Rect, monitor: Rect, work: Rect) -> Edge {
    let (mb, wb) = (monitor.y + monitor.h, work.y + work.h);
    let (mr, wr) = (monitor.x + monitor.w, work.x + work.w);
    if wb < mb {
        return Edge::Bottom;
    }
    if work.y > monitor.y {
        return Edge::Top;
    }
    if work.x > monitor.x {
        return Edge::Left;
    }
    if wr < mr {
        return Edge::Right;
    }
    let (cx, cy) = icon.center();
    [
        (mb - cy, Edge::Bottom),
        (cy - monitor.y, Edge::Top),
        (cx - monitor.x, Edge::Left),
        (mr - cx, Edge::Right),
    ]
    .into_iter()
    .min_by_key(|(d, _)| *d)
    .map(|(_, e)| e)
    .unwrap_or(Edge::Bottom)
}

/// Where a flyout of `size` (physical pixels) goes for the tray icon at `icon`: against the
/// taskbar's edge, `gap` from it, centred on the icon along that edge and kept `gap` inside the
/// work area (and inside the icon's side of an auto-hidden taskbar).
pub fn place(icon: Rect, monitor: Rect, work: Rect, size: (i32, i32), gap: i32) -> Rect {
    let (w, h) = size;
    let edge = taskbar_edge(icon, monitor, work);
    let (cx, cy) = icon.center();
    let clamp = |v: i32, lo: i32, hi: i32| if hi < lo { lo } else { v.clamp(lo, hi) };
    let x_along = clamp(cx - w / 2, work.x + gap, work.x + work.w - w - gap);
    let y_along = clamp(cy - h / 2, work.y + gap, work.y + work.h - h - gap);
    let (x, y) = match edge {
        Edge::Bottom => (x_along, (work.y + work.h).min(icon.y) - h - gap),
        Edge::Top => (x_along, work.y.max(icon.y + icon.h) + gap),
        Edge::Left => (work.x.max(icon.x + icon.w) + gap, y_along),
        Edge::Right => ((work.x + work.w).min(icon.x) - w - gap, y_along),
    };
    Rect { x, y, w, h }
}

/// What a tray click does to the flyout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Open,
    Close,
    /// The click already closed it (it took the flyout's focus): stay closed.
    Nothing,
}

/// A left click on the tray icon: opens a closed flyout, closes an open one. A flyout that just
/// closed on focus loss was closed by this very click, so it stays closed.
pub fn on_click(open: bool, closed_by_focus_at: Option<Instant>, now: Instant) -> Click {
    if open {
        return Click::Close;
    }
    match closed_by_focus_at {
        Some(t) if now.saturating_duration_since(t) < TOGGLE_WINDOW => Click::Nothing,
        _ => Click::Open,
    }
}

/// True while the flyout's window exists (from the frame that shows it to the frame after it
/// closes, which drops the window). `wake::Gate` reads it: the flyout is drawn by the main
/// window's frame, so a hidden main window must still get frames while it is up (#935).
static LIVE: AtomicBool = AtomicBool::new(false);

pub fn set_live(live: bool) {
    LIVE.store(live, Ordering::Release);
}

pub fn live() -> bool {
    LIVE.load(Ordering::Acquire)
}

/// A tray click wants the main window's frame now (#1019). The click's own repaint request can
/// get lost while the main window is hidden: egui calls eframe's repaint callback only for a
/// request sooner than the one it already has, and the frame that hid the window left one due
/// at once that eframe can never answer (no `RedrawRequested` for a hidden window). So egui says
/// nothing, eframe's flow stays `Wait`, and the flyout was never drawn. `wake::Gate` takes this
/// at the end of the click's wake and runs the frame itself.
static KICK: AtomicBool = AtomicBool::new(false);

pub fn kick() {
    KICK.store(true, Ordering::Release);
}

/// Takes the kick (one frame per click).
pub fn take_kick() -> bool {
    KICK.swap(false, Ordering::AcqRel)
}

/// A double click on the tray icon, in its handler before Hideout's window shows: the flyout's
/// window, if it is on screen, is hidden at once with no DWM transition (#997, #1076); the
/// caller marks it for the next frame to drop without counting a close by focus.
pub fn hide_now() {
    let h = crate::platform::flyout_hwnd();
    if h != 0 {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED,
        };
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        let hwnd = HWND(h as *mut _);
        // A Win32 BOOL, TRUE: transitions off for this window.
        let off: i32 = 1;
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED,
                (&off as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    // No kick: Hideout's window shows right after, and its next frame drops the flyout.
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    const SIZE: (i32, i32) = (536, 400);

    fn icon(x: i32, y: i32) -> Rect {
        Rect { x, y, w: 24, h: 24 }
    }

    #[test]
    fn bottom_taskbar_puts_it_above_the_icon_inside_the_work_area() {
        let work = Rect { h: 1032, ..MON };
        assert_eq!(taskbar_edge(icon(1800, 1044), MON, work), Edge::Bottom);
        let r = place(icon(1800, 1044), MON, work, SIZE, 12);
        // Centred on the icon would run off the right: kept 12 px inside.
        assert_eq!(
            r,
            Rect {
                x: 1920 - 536 - 12,
                y: 1032 - 400 - 12,
                w: 536,
                h: 400
            }
        );
        let r = place(icon(900, 1044), MON, work, SIZE, 12);
        assert_eq!(r.x, 912 - 268);
    }

    #[test]
    fn top_left_and_right_taskbars() {
        let top = Rect {
            y: 48,
            h: 1032,
            ..MON
        };
        let r = place(icon(1800, 12), MON, top, SIZE, 12);
        assert_eq!((r.y, r.x + r.w), (60, 1908));
        let left = Rect {
            x: 60,
            w: 1860,
            ..MON
        };
        let r = place(icon(18, 1000), MON, left, SIZE, 12);
        assert_eq!((r.x, r.y + r.h), (72, 1068));
        let right = Rect { w: 1860, ..MON };
        let r = place(icon(1878, 500), MON, right, SIZE, 12);
        assert_eq!((r.x + r.w, r.y), (1848, 312));
    }

    #[test]
    fn an_auto_hidden_taskbar_goes_by_the_nearest_edge_and_clears_the_icon() {
        assert_eq!(taskbar_edge(icon(1800, 1050), MON, MON), Edge::Bottom);
        assert_eq!(taskbar_edge(icon(1800, 4), MON, MON), Edge::Top);
        assert_eq!(taskbar_edge(icon(2, 500), MON, MON), Edge::Left);
        let r = place(icon(1800, 1050), MON, MON, SIZE, 12);
        assert_eq!(r.y + r.h, 1050 - 12);
    }

    #[test]
    fn a_second_monitor_left_of_the_primary() {
        let mon = Rect {
            x: -2560,
            y: 0,
            w: 2560,
            h: 1440,
        };
        let work = Rect { h: 1392, ..mon };
        let r = place(icon(-100, 1404), mon, work, (804, 600), 18);
        assert_eq!((r.x + r.w, r.y + r.h), (-18, 1392 - 18));
    }

    #[test]
    fn a_flyout_that_never_got_the_focus_closes_after_the_grace() {
        let now = Instant::now();
        let shown = Some(now);
        let end = now + FOCUS_GRACE;
        // Before it is shown, with the focus, or when its window can't say: nothing.
        assert_eq!(focus_check(false, Some(false), None, end), FocusCheck::Fine);
        assert_eq!(focus_check(true, Some(false), shown, end), FocusCheck::Fine);
        assert_eq!(focus_check(false, Some(true), shown, end), FocusCheck::Fine);
        assert_eq!(focus_check(false, None, shown, end), FocusCheck::Fine);
        // Shown without the focus: one look when the grace ends, then it closes.
        let quarter = Duration::from_millis(250);
        assert_eq!(
            focus_check(false, Some(false), shown, now + quarter),
            FocusCheck::LookIn(FOCUS_GRACE - quarter)
        );
        assert_eq!(
            focus_check(false, Some(false), shown, end),
            FocusCheck::Close
        );
    }

    #[test]
    fn it_is_placed_again_only_for_a_whole_pixel() {
        assert!(needs_place(None, 300.0, 1.25));
        assert!(!needs_place(Some(300.0), 300.0, 1.25));
        // 0.6 pt at 125 % is 0.75 px: the same size.
        assert!(!needs_place(Some(300.0), 300.6, 1.25));
        assert!(needs_place(Some(300.0), 301.0, 1.25));
        assert!(needs_place(Some(300.0), 299.0, 1.0));
        assert!(!needs_place(Some(300.0), 299.5, 1.0));
    }

    #[test]
    fn a_click_toggles_and_the_closing_click_does_not_reopen() {
        let now = Instant::now();
        assert_eq!(on_click(false, None, now), Click::Open);
        assert_eq!(on_click(true, None, now), Click::Close);
        let just = now - Duration::from_millis(80);
        assert_eq!(on_click(false, Some(just), now), Click::Nothing);
        let long_ago = now - Duration::from_secs(3);
        assert_eq!(on_click(false, Some(long_ago), now), Click::Open);
    }
}
