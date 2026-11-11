// SPDX-License-Identifier: MIT
//! Dragging a tab out into a pop-out and a pop-out back onto the strip (#1012 item 7, #1054
//! item 4; `docs/hideout-popouts-ux.md`, "Popping out" and "Docking back"). Pure: the strip and
//! the window code feed it points and pixels, so the tests need no window.
//!
//! - **Tear-off:** a tab dragged more than [`TEAR_OFF`] from the strip shows the ghost card;
//!   letting go outside Hideout's window opens the page in its own window there, on the strip
//!   it reorders, anywhere else in Hideout (or Esc, or a lost button) it does nothing.
//! - **Dock by drag:** a pop-out moved by its title row (Windows' move loop) with the pointer on
//!   the main window's strip, the main window topmost there, shows the insertion bar; letting go
//!   docks the page at the bar.
//!
//! Clean-room: written from the pop-outs plan and public Win32 docs.

use crate::placement::Rect;

/// How far (points) the pointer may leave the strip before a tab drag becomes a tear-off.
pub const TEAR_OFF: f32 = 24.0;
/// How far (points) above or below the strip a dragged pop-out still drops onto it.
pub const DOCK_REACH: f32 = 12.0;
/// Where the pointer sits on the ghost card (device-independent units from its corner): on the
/// page's icon, as in the plan's mockup.
pub const GHOST_GRAB: (f64, f64) = (18.0, 20.0);
/// The ghost card's size (device-independent units).
pub const GHOST_SIZE: (f32, f32) = (300.0, 92.0);

/// A rectangle in points (egui's units), edges inclusive on the left and top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Area {
    pub fn contains(&self, (x, y): (f32, f32)) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    fn grown(&self, dx: f32, dy: f32) -> Area {
        Area {
            left: self.left - dx,
            top: self.top - dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        }
    }
}

/// A tab's horizontal extent on the strip (points, as drawn: scrolled).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub left: f32,
    pub right: f32,
}

/// Where a dragged tab is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Zone {
    /// On the strip, or within [`TEAR_OFF`] of it: the drag reorders.
    #[default]
    Strip,
    /// Past [`TEAR_OFF`] but still over Hideout's window: the ghost shows; letting go cancels.
    Hub,
    /// Outside Hideout's window: letting go opens the page in its own window there.
    Outside,
}

/// The zone of the pointer `p`: `strip` is the strip's area, `window` Hideout's whole window
/// (its frame included), both in the main window's client points.
pub fn zone(strip: Area, window: Area, p: (f32, f32)) -> Zone {
    if strip.grown(TEAR_OFF, TEAR_OFF).contains(p) {
        Zone::Strip
    } else if window.contains(p) {
        Zone::Hub
    } else {
        Zone::Outside
    }
}

/// What ending a tab drag does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    /// Reorder: the tab at `from` goes to `to`.
    Move {
        from: usize,
        to: usize,
    },
    /// Open the page in its own window at the pointer.
    PopOut(usize),
    Nothing,
}

/// How a tab drag from `from` ends: let go in `zone` with the strip's landing place `to`.
/// `cancelled`: Esc, or the button came up somewhere Hideout didn't see (focus or capture lost).
/// A tab already in its own window only reorders: its window is where it is.
pub fn drop(from: usize, zone: Zone, to: usize, cancelled: bool, popped: bool) -> Drop {
    if cancelled {
        return Drop::Nothing;
    }
    match zone {
        Zone::Strip if to != from => Drop::Move { from, to },
        Zone::Outside if !popped => Drop::PopOut(from),
        _ => Drop::Nothing,
    }
}

/// The insertion slot (0..=n, before tab `slot`) for a pointer at `x`: the tabs whose centre is
/// left of it.
pub fn insert_slot(spans: &[Span], x: f32) -> usize {
    spans
        .iter()
        .filter(|s| (s.left + s.right) / 2.0 <= x)
        .count()
}

/// The strip position a tab at `from` ends at when it lands in slot `slot`.
pub fn slot_to_index(slot: usize, from: usize) -> usize {
    if slot > from {
        slot - 1
    } else {
        slot
    }
}

/// Where the insertion bar for `slot` goes: the left edge of tab `slot`, or the last tab's
/// right edge for the end, kept inside the visible strip `clip` (left, right).
pub fn bar_x(spans: &[Span], slot: usize, clip: (f32, f32)) -> Option<f32> {
    let x = match spans.get(slot) {
        Some(s) => s.left,
        None => spans.last()?.right,
    };
    Some(x.clamp(clip.0 + 1.0, clip.1 - 1.0))
}

/// The slot a dragged pop-out drops into for a pointer at `p` (main client points), or `None`
/// off the strip. With no tabs drawn (none open) the strip still takes it, at slot 0.
pub fn dock_slot(strip: Area, spans: &[Span], p: (f32, f32)) -> Option<usize> {
    strip
        .grown(0.0, DOCK_REACH)
        .contains(p)
        .then(|| insert_slot(spans, p.0))
}

/// The dock bar's width (points) in its own window above a dragged pop-out: a point wider than
/// the strip's own 2-point bar, so it reads against the window it crosses.
pub const BAR_W: f32 = 3.0;

/// The dock bar's window on screen (corner and size, physical pixels): centred on `x` (main
/// client points), the strip's height `y` (top, bottom), from the client corner `origin`.
pub fn bar_rect(origin: (i32, i32), x: f32, y: (f32, f32), ppp: f32) -> ((i32, i32), (i32, i32)) {
    let ppp = if ppp > 0.0 { ppp } else { 1.0 };
    let w = ((BAR_W * ppp).round() as i32).max(2);
    let left = origin.0 + (x * ppp).round() as i32 - w / 2;
    let top = origin.1 + (y.0 * ppp).round() as i32;
    let h = (((y.1 - y.0) * ppp).round() as i32).max(1);
    ((left, top), (w, h))
}

/// A screen point (physical pixels) in the main window's client points: `origin` is its client
/// area's corner on screen, `ppp` egui's pixels per point there.
pub fn to_points(screen: (i32, i32), origin: (i32, i32), ppp: f32) -> (f32, f32) {
    let ppp = if ppp > 0.0 { ppp } else { 1.0 };
    (
        (screen.0 - origin.0) as f32 / ppp,
        (screen.1 - origin.1) as f32 / ppp,
    )
}

/// A top-level window as the drop test sees it: its handle, its rectangle on screen, and
/// whether it shows (visible, not minimised, not cloaked on another desktop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub id: isize,
    pub rect: Rect,
    pub shown: bool,
}

/// The window the user sees at `p`: the first shown layer (top of the z order first) holding
/// it, leaving out `skip` (the dragged window itself and the ghost).
pub fn top_at(layers: &[Layer], p: (i32, i32), skip: &[isize]) -> Option<isize> {
    layers
        .iter()
        .filter(|l| l.shown && !skip.contains(&l.id))
        .find(|l| {
            p.0 >= l.rect.x
                && p.0 < l.rect.x + l.rect.w
                && p.1 >= l.rect.y
                && p.1 < l.rect.y + l.rect.h
        })
        .map(|l| l.id)
}

/// The ghost card's corner on screen for a pointer at `p`, `scale` the screen's scaling there.
pub fn ghost_corner(p: (i32, i32), scale: f32) -> (i32, i32) {
    let s = if scale > 0.0 { scale as f64 } else { 1.0 };
    (
        p.0 - (GHOST_GRAB.0 * s).round() as i32,
        p.1 - (GHOST_GRAB.1 * s).round() as i32,
    )
}

/// A dragged pop-out's state as the main window sees it, one per move loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowDrag {
    /// The window being moved.
    pub hwnd: isize,
    /// The pointer on screen (physical pixels) at the last `WM_MOVING`.
    pub cursor: (i32, i32),
    /// The move loop ended (`WM_EXITSIZEMOVE`): the drop.
    pub ended: bool,
}

/// The end of a pop-out's move loop (`WM_EXITSIZEMOVE`): the drop with the pointer at `cursor`,
/// or `None` when the move was cancelled. Esc in Windows' move loop puts the window back where
/// it was and still ends the loop with the pointer wherever it is (over the strip, say), so a
/// window back at its rectangle from the start of the move (`start`; `now` at the end) means
/// "nothing happens", as Esc does for any window.
pub fn end_move(
    d: WindowDrag,
    start: Option<Rect>,
    now: Option<Rect>,
    cursor: Option<(i32, i32)>,
) -> Option<WindowDrag> {
    if start.is_some() && start == now {
        return None;
    }
    Some(WindowDrag {
        cursor: cursor.unwrap_or(d.cursor),
        ended: true,
        ..d
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Area {
        // The strip of a 1100 × 640 window with a 220-point sidebar.
        Area {
            left: 220.0,
            top: 0.0,
            right: 1100.0,
            bottom: 36.0,
        }
    }

    fn window() -> Area {
        // The whole window in client points: the caption above, the frame round it.
        Area {
            left: -8.0,
            top: -31.0,
            right: 1108.0,
            bottom: 648.0,
        }
    }

    fn spans() -> Vec<Span> {
        (0..4)
            .map(|i| Span {
                left: 220.0 + i as f32 * 120.0,
                right: 340.0 + i as f32 * 120.0,
            })
            .collect()
    }

    #[test]
    fn a_tab_drag_reorders_near_the_strip_and_tears_off_past_24_points() {
        assert_eq!(zone(strip(), window(), (500.0, 18.0)), Zone::Strip);
        // Within 24 points below the strip: still a reorder.
        assert_eq!(zone(strip(), window(), (500.0, 59.0)), Zone::Strip);
        // Into the sidebar sideways, within 24 points of the strip's left end: still the strip.
        assert_eq!(zone(strip(), window(), (200.0, 18.0)), Zone::Strip);
        // Past 24 points: the ghost shows, but Hideout is still under the pointer.
        assert_eq!(zone(strip(), window(), (500.0, 61.0)), Zone::Hub);
        assert_eq!(zone(strip(), window(), (100.0, 18.0)), Zone::Hub);
        // Up into the caption: within 24 points still the strip; past it Hideout (the caption
        // is Hideout's); out of the window on any side, outside.
        assert_eq!(zone(strip(), window(), (500.0, -20.0)), Zone::Strip);
        assert_eq!(zone(strip(), window(), (300.0, -30.0)), Zone::Hub);
        assert_eq!(zone(strip(), window(), (100.0, -30.0)), Zone::Hub);
        assert_eq!(zone(strip(), window(), (100.0, -40.0)), Zone::Outside);
        assert_eq!(zone(strip(), window(), (1200.0, 300.0)), Zone::Outside);
        assert_eq!(zone(strip(), window(), (500.0, 700.0)), Zone::Outside);
        assert_eq!(zone(strip(), window(), (-50.0, 300.0)), Zone::Outside);
    }

    #[test]
    fn letting_go_reorders_pops_out_or_does_nothing() {
        assert_eq!(
            drop(1, Zone::Strip, 3, false, false),
            Drop::Move { from: 1, to: 3 }
        );
        assert_eq!(drop(1, Zone::Strip, 1, false, false), Drop::Nothing);
        assert_eq!(drop(1, Zone::Outside, 1, false, false), Drop::PopOut(1));
        // On the page area, or with Esc, or a lost button: nothing.
        assert_eq!(drop(1, Zone::Hub, 3, false, false), Drop::Nothing);
        assert_eq!(drop(1, Zone::Outside, 1, true, false), Drop::Nothing);
        assert_eq!(drop(1, Zone::Strip, 3, true, false), Drop::Nothing);
        // A placeholder (its page is out already) only reorders.
        assert_eq!(drop(1, Zone::Outside, 1, false, true), Drop::Nothing);
        assert_eq!(
            drop(1, Zone::Strip, 0, false, true),
            Drop::Move { from: 1, to: 0 }
        );
    }

    #[test]
    fn the_insertion_slot_follows_tab_centres() {
        let s = spans();
        assert_eq!(insert_slot(&s, 100.0), 0);
        assert_eq!(insert_slot(&s, 279.0), 0);
        assert_eq!(insert_slot(&s, 281.0), 1);
        assert_eq!(insert_slot(&s, 500.0), 2);
        assert_eq!(insert_slot(&s, 1000.0), 4);
        assert_eq!(insert_slot(&[], 500.0), 0);
        // A tab at 1 dropped in slot 3 (before tab 3) ends at 2; in slot 0 at 0; its own slots
        // (1 and 2) leave it where it is.
        assert_eq!(slot_to_index(3, 1), 2);
        assert_eq!(slot_to_index(0, 1), 0);
        assert_eq!(slot_to_index(1, 1), 1);
        assert_eq!(slot_to_index(2, 1), 1);
        assert_eq!(slot_to_index(4, 1), 3);
    }

    #[test]
    fn the_bar_sits_before_its_tab_or_after_the_last() {
        let s = spans();
        let clip = (220.0, 1064.0);
        assert_eq!(bar_x(&s, 0, clip), Some(221.0));
        assert_eq!(bar_x(&s, 2, clip), Some(460.0));
        assert_eq!(bar_x(&s, 4, clip), Some(700.0));
        assert_eq!(bar_x(&[], 0, clip), None);
        // A tab scrolled out of view: the bar stays at the strip's edge.
        let scrolled = [Span {
            left: 1500.0,
            right: 1620.0,
        }];
        assert_eq!(bar_x(&scrolled, 0, clip), Some(1063.0));
    }

    #[test]
    fn a_dragged_window_drops_on_the_strip_only() {
        let s = spans();
        assert_eq!(dock_slot(strip(), &s, (500.0, 18.0)), Some(2));
        assert_eq!(dock_slot(strip(), &s, (500.0, 47.0)), Some(2));
        assert_eq!(dock_slot(strip(), &s, (500.0, -11.0)), Some(2));
        assert_eq!(dock_slot(strip(), &s, (500.0, 49.0)), None);
        assert_eq!(dock_slot(strip(), &s, (100.0, 18.0)), None, "the sidebar");
        assert_eq!(dock_slot(strip(), &[], (500.0, 18.0)), Some(0));
    }

    #[test]
    fn screen_pixels_map_to_main_window_points() {
        // A 150 % screen, the client corner at (2600, 140).
        assert_eq!(to_points((2600, 140), (2600, 140), 1.5), (0.0, 0.0));
        assert_eq!(to_points((3350, 167), (2600, 140), 1.5), (500.0, 18.0));
        // Left of a window on a screen at negative coordinates.
        assert_eq!(to_points((-1900, 100), (-1800, 100), 1.0), (-100.0, 0.0));
        assert_eq!(to_points((10, 10), (0, 0), 0.0), (10.0, 10.0));
    }

    fn layer(id: isize, x: i32, y: i32, w: i32, h: i32, shown: bool) -> Layer {
        Layer {
            id,
            rect: Rect { x, y, w, h },
            shown,
        }
    }

    #[test]
    fn the_drop_target_is_the_window_on_top_at_the_pointer() {
        // Top first: the dragged pop-out (7), the ghost (9), a hidden window (5), another app
        // (3) over the right half of Hideout (1).
        let layers = [
            layer(7, 300, 0, 600, 400, true),
            layer(9, 300, 0, 300, 90, true),
            layer(5, 0, 0, 2000, 2000, false),
            layer(3, 600, 0, 600, 800, true),
            layer(1, 0, 0, 1100, 700, true),
        ];
        assert_eq!(top_at(&layers, (400, 20), &[7, 9]), Some(1));
        assert_eq!(top_at(&layers, (700, 20), &[7, 9]), Some(3), "covered");
        assert_eq!(top_at(&layers, (400, 20), &[]), Some(7));
        assert_eq!(top_at(&layers, (1500, 900), &[7, 9]), None);
        // Right and bottom edges are outside.
        assert_eq!(top_at(&layers, (1100, 20), &[7, 9, 3]), None);
    }

    #[test]
    fn the_dock_bars_window_sits_on_the_strips_bar() {
        // 100 %: 3 px wide centred on x, the strip's 36 px.
        assert_eq!(
            bar_rect((100, 50), 460.0, (0.0, 36.0), 1.0),
            ((559, 50), (3, 36))
        );
        // 150 %: 4.5 → 5 px (rounded), 54 px tall.
        assert_eq!(
            bar_rect((100, 50), 460.0, (0.0, 36.0), 1.5),
            ((788, 50), (5, 54))
        );
        assert_eq!(bar_rect((0, 0), 10.0, (0.0, 36.0), 0.0), ((9, 0), (3, 36)));
    }

    #[test]
    fn esc_in_the_move_loop_cancels_the_drop() {
        let d = WindowDrag {
            hwnd: 7,
            cursor: (500, 20),
            ended: false,
        };
        let start = Rect {
            x: 900,
            y: 300,
            w: 640,
            h: 480,
        };
        let moved = Rect {
            x: 380,
            y: 5,
            ..start
        };
        // Let go where it was moved to: the drop, at the pointer now.
        let got = end_move(d, Some(start), Some(moved), Some((510, 22))).unwrap();
        assert!(got.ended);
        assert_eq!((got.hwnd, got.cursor), (7, (510, 22)));
        // Esc: Windows put it back at its rectangle from the start; no drop.
        assert_eq!(end_move(d, Some(start), Some(start), Some((510, 22))), None);
        // No pointer at the end (a keyboard move): the last one seen.
        assert_eq!(
            end_move(d, Some(start), Some(moved), None).unwrap().cursor,
            (500, 20)
        );
        // A rectangle that couldn't be read never cancels.
        assert!(end_move(d, None, None, None).is_some());
    }

    #[test]
    fn the_ghost_keeps_the_pointer_on_its_icon() {
        assert_eq!(ghost_corner((1000, 500), 1.0), (982, 480));
        assert_eq!(ghost_corner((1000, 500), 1.5), (973, 470));
        assert_eq!(ghost_corner((0, 0), 0.0), (-18, -20));
    }
}
