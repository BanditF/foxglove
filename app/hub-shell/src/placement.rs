//! Where a hub window opens and where it has been (#439, #443): the logic behind Foxglove's
//! Settings window and Hideout's window. Where the window is, is read and set in physical pixels
//! through Win32 ([`NativeWindow`]) but remembered in points (logical pixels) of its display, so it
//! keeps its size on displays with other scaling. The window opens where it was last left: the same
//! place on the same display, or centred on that display (or the one under the mouse, if that one
//! is gone too) when the old place is no longer on any screen. The first time, it opens compact:
//! [`DEFAULT_SIZE`], never more than [`MAX_SHARE`] of the display, centred. Plain and tested
//! without a window.

use serde::{Deserialize, Serialize};

use crate::monitors::Monitor;
/// The size it first opens at, in points: compact but readable, room for the sidebar and a
/// page at the normal text size (#385). A remembered size wins over it.
pub const DEFAULT_SIZE: egui::Vec2 = egui::vec2(880.0, 600.0);
/// The smallest size, in points: sidebar labels and status chips still fit (#443's tests).
pub const MIN_SIZE: egui::Vec2 = egui::vec2(720.0, 420.0);
/// The first opening takes at most this share of the display's work area, each way.
pub const MAX_SHARE: f32 = 0.7;
/// How much of the title bar, in pixels, must be on a display for the old place to be used.
const GRIP: [i32; 2] = [120, 24];

/// Where the window was last: the display it was mostly on and its outer rectangle as left, top
/// (from that display's work-area corner), width and height, in points of that display.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub monitor: String,
    pub place: Option<[f32; 4]>,
    /// The first version's (#439) outer rectangle in physical desktop pixels: read while there's
    /// no `place` yet, never written.
    #[serde(skip_serializing)]
    pub rect: Option<[i32; 4]>,
}

/// Physical pixels per point at a DPI (96 = 100 %).
pub fn scale_of(dpi: u32) -> f32 {
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// A window rectangle (physical desktop pixels) as a place on `monitor`: its corner in that
/// display's points, its size in points at the window's own scaling `dpi`.
pub fn to_points(rect: [i32; 4], monitor: &Monitor, dpi: u32) -> [f32; 4] {
    let s = monitor.scale();
    let w = scale_of(dpi);
    [
        (rect[0] - monitor.work_area[0]) as f32 / s,
        (rect[1] - monitor.work_area[1]) as f32 / s,
        (rect[2] - rect[0]) as f32 / w,
        (rect[3] - rect[1]) as f32 / w,
    ]
}

/// A place on `monitor` as a window rectangle in physical desktop pixels.
pub fn to_physical(place: [f32; 4], monitor: &Monitor) -> [i32; 4] {
    let s = monitor.scale();
    let px = |v: f32| (v * s).round() as i32;
    let left = monitor.work_area[0] + px(place[0]);
    let top = monitor.work_area[1] + px(place[1]);
    [left, top, left + px(place[2]), top + px(place[3])]
}

/// The size of the overlap of two rectangles: width, height (0 when apart).
fn overlap(a: [i32; 4], b: [i32; 4]) -> [i32; 2] {
    [
        (a[2].min(b[2]) - a[0].max(b[0])).max(0),
        (a[3].min(b[3]) - a[1].max(b[1])).max(0),
    ]
}

/// `size` centred in `area`, shrunk to fit it.
fn centred(size: [i32; 2], area: [i32; 4]) -> [i32; 4] {
    let w = size[0].min(area[2] - area[0]).max(1);
    let h = size[1].min(area[3] - area[1]).max(1);
    let left = area[0] + (area[2] - area[0] - w) / 2;
    let top = area[1] + (area[3] - area[1] - h) / 2;
    [left, top, left + w, top + h]
}

/// Enough of the title bar is on a display for it to be grabbed.
fn reachable(r: [i32; 4], monitors: &[Monitor]) -> bool {
    let grip = [r[0], r[1], r[2], r[1] + GRIP[1].min(r[3] - r[1])];
    let need = [GRIP[0].min(r[2] - r[0]), GRIP[1].min(r[3] - r[1])];
    monitors.iter().any(|m| {
        let o = overlap(grip, m.work_area);
        o[0] >= need[0] && o[1] >= need[1]
    })
}

/// The display a desktop point is on, by work area, else by whole-pixel distance to it ("" when
/// no display is known), so a pointer over the taskbar still counts.
pub fn monitor_at(point: [i32; 2], monitors: &[Monitor]) -> String {
    let dist = |m: &Monitor| {
        let a = m.work_area;
        let dx = (a[0] - point[0]).max(point[0] - (a[2] - 1)).max(0);
        let dy = (a[1] - point[1]).max(point[1] - (a[3] - 1)).max(0);
        i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
    };
    monitors
        .iter()
        .min_by_key(|m| dist(m))
        .map(|m| m.id.clone())
        .unwrap_or_default()
}

/// A first-opening size in points: `size`, at most [`MAX_SHARE`] of `monitor`'s work area each
/// way (but not below [`MIN_SIZE`]; the work area itself is the last limit, in [`centred`]).
pub fn compact(size: [f32; 2], monitor: &Monitor) -> [f32; 2] {
    let s = monitor.scale();
    let area = monitor.work_area;
    let cap = |v: f32, px: i32, min: f32| v.min((px as f32 / s * MAX_SHARE).max(min));
    [
        cap(size[0], area[2] - area[0], MIN_SIZE.x),
        cap(size[1], area[3] - area[1], MIN_SIZE.y),
    ]
}

/// Where to open the window, in physical desktop pixels. The saved place when enough of its
/// title bar is on a display (so it can still be grabbed); otherwise its remembered size
/// centred on the saved display, or on `fallback` (the display under the mouse, else the main
/// one) when that display is gone. With no remembered size, `size` in points made
/// [`compact`]. `None` when no display is known: leave it where Windows put it.
pub fn restore(
    saved: &Saved,
    monitors: &[Monitor],
    size: [f32; 2],
    fallback: &str,
) -> Option<[i32; 4]> {
    let home = monitors
        .iter()
        .find(|m| !saved.monitor.is_empty() && m.id == saved.monitor);
    let kept = match (saved.place, home) {
        (Some(place), Some(m)) => Some(to_physical(place, m)),
        (Some(_), None) => None,
        (None, _) => saved.rect,
    };
    if let Some(r) = kept.filter(|r| reachable(*r, monitors)) {
        return Some(r);
    }
    let monitor = home
        .cloned()
        .or_else(|| crate::monitors::find(monitors, fallback))
        .or_else(|| monitors.first().cloned())?;
    let size = saved
        .place
        .map(|p| [p[2], p[3]])
        .or_else(|| {
            let s = home.map_or(1.0, Monitor::scale);
            saved
                .rect
                .map(|r| [(r[2] - r[0]) as f32 / s, (r[3] - r[1]) as f32 / s])
        })
        .unwrap_or_else(|| compact(size, &monitor));
    let s = monitor.scale();
    let px = |v: f32| (v * s).round() as i32;
    Some(centred([px(size[0]), px(size[1])], monitor.work_area))
}

/// The display a rectangle is mostly on ("" when it's on none).
pub fn monitor_of(rect: [i32; 4], monitors: &[Monitor]) -> String {
    monitors
        .iter()
        .map(|m| {
            let o = overlap(rect, m.work_area);
            (i64::from(o[0]) * i64::from(o[1]), m)
        })
        .filter(|(area, _)| *area > 0)
        .max_by_key(|(area, _)| *area)
        .map(|(_, m)| m.id.clone())
        .unwrap_or_default()
}

/// The smallest outer size, in physical pixels, at a DPI: [`MIN_SIZE`] in points plus the
/// frame (`frame`, the title bar and borders at that DPI). egui gives winit the smallest size
/// once, in the pixels of the display the window was made on; on a display with other scaling
/// that would be too small or too big, and too big clamps the rectangle Windows suggests.
pub fn min_track_size(dpi: u32, frame: [i32; 2]) -> [i32; 2] {
    let s = scale_of(dpi);
    [
        (MIN_SIZE.x * s).ceil() as i32 + frame[0],
        (MIN_SIZE.y * s).ceil() as i32 + frame[1],
    ]
}

/// The window message asks to minimize it (title-bar button, taskbar, window menu), which
/// closes it instead.
pub fn minimize_closes(message: u32, wparam: usize) -> bool {
    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_MINIMIZE: usize = 0xF020;
    message == WM_SYSCOMMAND && wparam & 0xFFF0 == SC_MINIMIZE
}

/// The open window, as far as placing it goes.
pub trait NativeWindow {
    /// Its outer rectangle in physical pixels; `None` while minimized or maximized (those aren't
    /// places to come back to).
    fn rect(&self) -> Option<[i32; 4]>;
    /// Its scaling as a DPI (that of the display Windows counts it on).
    fn dpi(&self) -> u32;
    /// Moves and sizes it (physical pixels).
    fn move_to(&mut self, rect: [i32; 4]);
    /// Shows and focuses it, once it's in place.
    fn show(&mut self);
}

/// Where the window goes and where it has been, between opening and closing.
#[derive(Debug, Default)]
pub struct Memory {
    saved: Saved,
    placed: bool,
    /// The last rectangle seen and the window's DPI then.
    last: Option<([i32; 4], u32)>,
}

impl Memory {
    /// Whether the window was placed in this opening (its first frame has run).
    pub fn placed(&self) -> bool {
        self.placed
    }

    pub fn new(saved: Saved) -> Self {
        Self {
            saved,
            ..Self::default()
        }
    }

    /// A frame with the window open: the first one puts it in its place and shows it, the
    /// others only note where it is now. It never moves or sizes the window after that, so a
    /// resize (a scaling change, the user) can't start another one.
    pub fn frame(&mut self, window: &mut dyn NativeWindow, monitors: &[Monitor], fallback: &str) {
        if !self.placed {
            self.placed = true;
            let s = scale_of(window.dpi());
            let size = window
                .rect()
                .map(|r| [(r[2] - r[0]) as f32 / s, (r[3] - r[1]) as f32 / s])
                .unwrap_or([DEFAULT_SIZE.x, DEFAULT_SIZE.y]);
            if let Some(to) = restore(&self.saved, monitors, size, fallback) {
                window.move_to(to);
            }
            window.show();
            return;
        }
        if let Some(r) = window.rect() {
            self.last = Some((r, window.dpi()));
        }
    }

    /// What to save now (the window lost the focus, or is closing), if it moved since the last
    /// save. Nothing while it's on no display.
    pub fn due_save(&mut self, monitors: &[Monitor]) -> Option<Saved> {
        let (r, dpi) = self.last?;
        let id = monitor_of(r, monitors);
        let monitor = monitors.iter().find(|m| m.id == id)?;
        let saved = Saved {
            monitor: id,
            place: Some(to_points(r, monitor, dpi)),
            rect: None,
        };
        if saved == self.saved {
            return None;
        }
        self.saved = saved.clone();
        Some(saved)
    }

    /// The window closed: what to save, and it's placed again when it next opens.
    pub fn closed(&mut self, monitors: &[Monitor]) -> Option<Saved> {
        let save = self.due_save(monitors);
        self.placed = false;
        self.last = None;
        save
    }
}
