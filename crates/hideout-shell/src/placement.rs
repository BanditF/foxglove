// SPDX-License-Identifier: MIT
//! Per-monitor window placement (contract §2.5). All rectangles are physical pixels in virtual
//! screen coordinates, and each saved one remembers the scale it was saved at, so a monitor
//! whose DPI changed gets the same size in points.

use crate::prefs::WindowPrefs;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    pub fn contains(&self, (px, py): (i32, i32)) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

/// One display, as the OS reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// A stable name for the display (its device name on Windows).
    pub key: String,
    /// Its Windows display target path (`monitorDevicePath`, §2.5), empty when unknown.
    pub path: String,
    /// The whole monitor, in the virtual desktop.
    pub area: Rect,
    /// The work area: the monitor minus the taskbar.
    pub work: Rect,
    /// Physical pixels per point (1.0 at 96 DPI).
    pub scale: f32,
    pub primary: bool,
}

/// A window rectangle as saved in `hideout.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
}

/// The default window size, in points.
pub const DEFAULT_SIZE: (f32, f32) = (960.0, 640.0);
/// The smallest window, in points.
pub const MIN_SIZE: (f32, f32) = (560.0, 380.0);

/// Where the window opens: the last monitor's saved rect if that monitor is still there, else the
/// primary monitor's saved rect, else the default size centred on the primary monitor. The result
/// always lies inside a work area.
pub fn restore(prefs: &WindowPrefs, monitors: &[Monitor]) -> Option<(Rect, f32)> {
    let primary = monitors.iter().find(|m| m.primary).or(monitors.first())?;
    let pick = |key: &str| {
        let m = monitors.iter().find(|m| m.key == key)?;
        prefs.monitors.get(key).map(|s| (m, *s))
    };
    let saved = prefs
        .last_monitor
        .as_deref()
        .and_then(pick)
        .or_else(|| pick(&primary.key));
    Some(match saved {
        Some((m, s)) => {
            let k = if s.scale > 0.0 {
                m.scale / s.scale
            } else {
                1.0
            };
            let r = Rect {
                x: s.x,
                y: s.y,
                w: (s.w as f32 * k).round() as i32,
                h: (s.h as f32 * k).round() as i32,
            };
            (fit(r, m), m.scale)
        }
        None => {
            let (w, h) = (
                DEFAULT_SIZE.0 * primary.scale,
                DEFAULT_SIZE.1 * primary.scale,
            );
            let r = Rect {
                x: 0,
                y: 0,
                w: w.round() as i32,
                h: h.round() as i32,
            };
            (centre(fit(r, primary), primary), primary.scale)
        }
    })
}

/// Records `rect` for the monitor holding its centre (or the nearest); no monitors, no change.
pub fn remember(prefs: &mut WindowPrefs, rect: Rect, monitors: &[Monitor]) {
    let Some(m) = monitor_for(rect, monitors) else {
        return;
    };
    prefs.monitors.insert(
        m.key.clone(),
        SavedRect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            scale: m.scale,
        },
    );
    prefs.last_monitor = Some(m.key.clone());
}

/// The monitor holding `rect`'s centre, or else the one whose work area is nearest to it.
pub fn monitor_for(rect: Rect, monitors: &[Monitor]) -> Option<&Monitor> {
    let c = rect.center();
    monitors.iter().find(|m| m.work.contains(c)).or_else(|| {
        monitors.iter().min_by_key(|m| {
            let (mx, my) = m.work.center();
            (mx - c.0).unsigned_abs() as u64 + (my - c.1).unsigned_abs() as u64
        })
    })
}

/// Shrinks `r` to the monitor's work area (not below the minimum size where it fits) and moves
/// it inside.
fn fit(r: Rect, m: &Monitor) -> Rect {
    let min_w = ((MIN_SIZE.0 * m.scale) as i32).min(m.work.w);
    let min_h = ((MIN_SIZE.1 * m.scale) as i32).min(m.work.h);
    let w = r.w.clamp(min_w, m.work.w);
    let h = r.h.clamp(min_h, m.work.h);
    let x = r.x.clamp(m.work.x, m.work.x + m.work.w - w);
    let y = r.y.clamp(m.work.y, m.work.y + m.work.h - h);
    Rect { x, y, w, h }
}

fn centre(r: Rect, m: &Monitor) -> Rect {
    Rect {
        x: m.work.x + (m.work.w - r.w) / 2,
        y: m.work.y + (m.work.h - r.h) / 2,
        ..r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(key: &str, x: i32, w: i32, h: i32, scale: f32, primary: bool) -> Monitor {
        Monitor {
            key: key.into(),
            path: String::new(),
            area: Rect { x, y: 0, w, h },
            work: Rect { x, y: 0, w, h },
            scale,
            primary,
        }
    }

    fn two() -> Vec<Monitor> {
        vec![
            mon("A", 0, 2560, 1400, 1.0, true),
            mon("B", 2560, 3840, 2100, 1.5, false),
        ]
    }

    #[test]
    fn first_run_centres_the_default_size_on_the_primary() {
        let (r, s) = restore(&WindowPrefs::default(), &two()).unwrap();
        assert_eq!(s, 1.0);
        assert_eq!((r.w, r.h), (960, 640));
        assert_eq!(r.center(), (1280, 700));
    }

    #[test]
    fn no_monitors_no_placement() {
        assert_eq!(restore(&WindowPrefs::default(), &[]), None);
    }

    #[test]
    fn the_last_monitor_wins_and_round_trips() {
        let ms = two();
        let mut p = WindowPrefs::default();
        remember(
            &mut p,
            Rect {
                x: 100,
                y: 100,
                w: 800,
                h: 600,
            },
            &ms,
        );
        remember(
            &mut p,
            Rect {
                x: 3000,
                y: 200,
                w: 1500,
                h: 900,
            },
            &ms,
        );
        assert_eq!(p.last_monitor.as_deref(), Some("B"));
        assert_eq!(p.monitors.len(), 2);
        assert_eq!(
            restore(&p, &ms).unwrap(),
            (
                Rect {
                    x: 3000,
                    y: 200,
                    w: 1500,
                    h: 900
                },
                1.5
            )
        );
    }

    #[test]
    fn a_gone_monitor_falls_back_to_the_primarys_own_place() {
        let mut p = WindowPrefs::default();
        remember(
            &mut p,
            Rect {
                x: 100,
                y: 100,
                w: 800,
                h: 600,
            },
            &two(),
        );
        remember(
            &mut p,
            Rect {
                x: 3000,
                y: 200,
                w: 1500,
                h: 900,
            },
            &two(),
        );
        let only_a = vec![two().remove(0)];
        assert_eq!(
            restore(&p, &only_a).unwrap().0,
            Rect {
                x: 100,
                y: 100,
                w: 800,
                h: 600
            }
        );
    }

    #[test]
    fn a_dpi_change_keeps_the_size_in_points() {
        let mut p = WindowPrefs::default();
        remember(
            &mut p,
            Rect {
                x: 0,
                y: 0,
                w: 1000,
                h: 700,
            },
            &two(),
        );
        let ms = vec![mon("A", 0, 2560, 1400, 1.25, true)];
        assert_eq!(
            restore(&p, &ms).unwrap().0,
            Rect {
                x: 0,
                y: 0,
                w: 1250,
                h: 875
            }
        );
    }

    #[test]
    fn an_off_screen_or_oversized_rect_is_pulled_inside() {
        let ms = vec![mon("A", 0, 1280, 700, 1.0, true)];
        let mut p = WindowPrefs {
            last_monitor: Some("A".into()),
            ..Default::default()
        };
        p.monitors.insert(
            "A".into(),
            SavedRect {
                x: 5000,
                y: -300,
                w: 4000,
                h: 300,
                scale: 1.0,
            },
        );
        let (r, _) = restore(&p, &ms).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 380
            }
        );
    }

    #[test]
    fn a_window_between_monitors_goes_to_the_nearest() {
        let ms = vec![
            mon("A", 0, 1000, 1000, 1.0, true),
            mon("B", 2000, 1000, 1000, 1.0, false),
        ];
        let r = Rect {
            x: 1700,
            y: 0,
            w: 400,
            h: 400,
        };
        assert_eq!(monitor_for(r, &ms).unwrap().key, "B");
    }
}
