//! The room map (#483): every device and every light in one space (`space`), placed from what
//! Windows and the devices already say, with no user input. The monitors come from Windows'
//! display layout; the keyboard sits centred below the primary monitor, the mouse to its right,
//! other lighting devices to its left, and the case beside the monitors (the side is a setting).
//! A user's drag is an offset per device in [`Layout`], applied last.
//!
//! Inputs are plain data, so the app gathers them (Windows, the lighting catalog, LampArrays,
//! the board profile) and the placement itself is pure and tested here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::space::{Point, Rect};

/// What a device is, which decides where it goes by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    Monitor,
    /// A keyboard in one piece.
    Keyboard,
    KeyboardLeft,
    KeyboardRight,
    Mouse,
    /// A PC case (its front: fans, logo, light strips).
    Case,
    /// Any other lighting device: a LampArray of another kind, or one added by hand.
    Other,
}

impl DeviceKind {
    fn label(self) -> &'static str {
        match self {
            DeviceKind::Monitor => "monitor",
            DeviceKind::Keyboard => "keyboard",
            DeviceKind::KeyboardLeft => "keyboard left",
            DeviceKind::KeyboardRight => "keyboard right",
            DeviceKind::Mouse => "mouse",
            DeviceKind::Case => "case",
            DeviceKind::Other => "other",
        }
    }

    /// The size a device of this kind gets when nothing says its own, in millimetres.
    fn default_size(self) -> Point {
        match self {
            DeviceKind::Monitor => DEFAULT_MONITOR,
            DeviceKind::Keyboard => Point::new(440.0, 140.0),
            // A Corne-shaped half: 6.5 x 4.75 key units of 19.05 mm.
            DeviceKind::KeyboardLeft | DeviceKind::KeyboardRight => Point::new(124.0, 90.0),
            DeviceKind::Mouse => Point::new(75.0, 130.0),
            DeviceKind::Case => Point::new(205.0, 475.0),
            DeviceKind::Other => Point::new(120.0, 120.0),
        }
    }
}

/// A 27" 16:9 panel, for anchoring when Windows lists no monitor at all.
const DEFAULT_MONITOR: Point = Point::new(597.0, 336.0);
/// From the primary monitor's bottom edge to the keyboard's top edge.
const DESK_GAP: f32 = 150.0;
/// Between a split keyboard's halves.
const HALF_GAP: f32 = 80.0;
/// From the keyboard to the mouse (and to other desk devices on the left).
const SIDE_GAP: f32 = 80.0;
/// Between devices of one row (a second mouse, a second case).
const ROW_GAP: f32 = 40.0;
/// From the monitors (or the desk row, if wider) to the case.
const CASE_GAP: f32 = 150.0;

/// One display, as Windows reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Windows' device name (`\\.\DISPLAY1`).
    pub id: String,
    pub primary: bool,
    /// The whole display in desktop pixels: left, top, right, bottom.
    pub rect_px: [i32; 4],
    /// The panel's physical pixel density (Windows' raw DPI, from the monitor's EDID), when
    /// known.
    pub raw_dpi: Option<u32>,
    /// Windows' scaling as a DPI (96 = 100 %), the fallback for the density.
    pub dpi: u32,
}

impl MonitorInfo {
    /// Millimetres per pixel: the raw DPI when it's plausible, else the scaling DPI (which
    /// Windows picks to approximate the panel's density), else 96.
    pub fn mm_per_px(&self) -> f32 {
        let plausible = |d: u32| (40..=800).contains(&d).then_some(d);
        let dpi = self
            .raw_dpi
            .and_then(plausible)
            .or(plausible(self.dpi))
            .unwrap_or(96);
        25.4 / dpi as f32
    }

    fn px(&self) -> (f32, f32, f32, f32) {
        let [l, t, r, b] = self.rect_px;
        (l as f32, t as f32, r.max(l) as f32, b.max(t) as f32)
    }
}

/// One light on a device: an id the device's output knows it by (a zone number, a lamp index,
/// `all`) and where it sits on the device, in millimetres from its top-left corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSpec {
    pub id: String,
    /// `None`: spread evenly with the device's other unplaced lights.
    pub at: Option<Point>,
}

/// One device to place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable across restarts; a [`Layout`] offset is kept under it.
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// Width and height in millimetres; the kind's default when `None`.
    pub size: Option<Point>,
    pub lights: Vec<LightSpec>,
}

impl DeviceInfo {
    /// A device lit as one strip: a single light, `all`, in its centre. The keyboard halves
    /// are this until their per-LED positions arrive (#115, #274).
    pub fn whole(id: impl Into<String>, name: impl Into<String>, kind: DeviceKind) -> Self {
        DeviceInfo {
            id: id.into(),
            name: name.into(),
            kind,
            size: None,
            lights: vec![LightSpec {
                id: "all".into(),
                at: None,
            }],
        }
    }

    pub fn with_size(mut self, size: Point) -> Self {
        self.size = Some(size);
        self
    }

    fn size(&self) -> Point {
        self.size
            .filter(|s| s.x.is_finite() && s.y.is_finite() && s.x > 0.0 && s.y > 0.0)
            .unwrap_or_else(|| self.kind.default_size())
    }
}

/// Which side of the monitors the case stands on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    #[default]
    Right,
}

/// What the user changed: the app stores it (its `[scene]` section) and passes it back in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub case_side: Side,
    /// A drag per device id, in millimetres from where the defaults put it.
    pub offsets: BTreeMap<String, Point>,
}

impl Layout {
    /// Adds a drag to a device's offset.
    pub fn nudge(&mut self, id: &str, by: Point) {
        let o = self.offsets.entry(id.to_string()).or_default();
        *o = *o + by;
        if o.x == 0.0 && o.y == 0.0 {
            self.offsets.remove(id);
        }
    }

    /// Puts a device back where the defaults place it.
    pub fn reset(&mut self, id: &str) {
        self.offsets.remove(id);
    }
}

/// A light where it is in the room.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlacedLight {
    pub id: String,
    pub at: Point,
}

/// A device where it is in the room. Monitors have no lights: they are surfaces a wallpaper
/// draws on, and their rectangle is what it needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub rect: Rect,
    pub lights: Vec<PlacedLight>,
}

/// Every device and light in room space.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoomMap {
    pub devices: Vec<Placed>,
    /// Everything placed; effects that cross the room run over this.
    pub bounds: Rect,
}

impl RoomMap {
    /// Places everything, with no user input beyond `layout` (which may be empty).
    pub fn build(monitors: &[MonitorInfo], devices: &[DeviceInfo], layout: &Layout) -> RoomMap {
        let mut placed = place_monitors(monitors);
        let primary = monitors
            .iter()
            .position(|m| m.primary)
            .or((!monitors.is_empty()).then_some(0))
            .map(|i| placed[i].rect)
            .unwrap_or(Rect::new(0.0, 0.0, DEFAULT_MONITOR.x, DEFAULT_MONITOR.y));
        let wall = placed.iter().fold(primary, |r, p| r.union(&p.rect));
        let of = |kinds: &[DeviceKind]| -> Vec<&DeviceInfo> {
            devices.iter().filter(|d| kinds.contains(&d.kind)).collect()
        };

        // The keyboard, centred below the primary monitor.
        let top = primary.bottom() + DESK_GAP;
        let cx = primary.centre().x;
        let mut desk: Vec<(&DeviceInfo, Rect)> = Vec::new();
        for d in of(&[
            DeviceKind::Keyboard,
            DeviceKind::KeyboardLeft,
            DeviceKind::KeyboardRight,
        ]) {
            let s = d.size();
            let x = match d.kind {
                DeviceKind::KeyboardLeft => cx - HALF_GAP / 2.0 - s.x,
                DeviceKind::KeyboardRight => cx + HALF_GAP / 2.0,
                _ => cx - s.x / 2.0,
            };
            desk.push((d, Rect::new(x, top, s.x, s.y)));
        }
        let row = desk
            .iter()
            .map(|(_, r)| *r)
            .reduce(|a, b| a.union(&b))
            .unwrap_or(Rect::new(
                cx,
                top,
                0.0,
                DeviceKind::KeyboardLeft.default_size().y,
            ));
        let mid = row.centre().y;

        // Mice to the right of the keyboard, other desk devices to its left.
        let mut right = row.right() + SIDE_GAP;
        for d in of(&[DeviceKind::Mouse]) {
            let s = d.size();
            desk.push((d, Rect::new(right, mid - s.y / 2.0, s.x, s.y)));
            right += s.x + ROW_GAP;
        }
        let mut left = row.x - SIDE_GAP;
        for d in of(&[DeviceKind::Other]) {
            let s = d.size();
            left -= s.x;
            desk.push((d, Rect::new(left, mid - s.y / 2.0, s.x, s.y)));
            left -= ROW_GAP;
        }

        // Cases beside the monitors (or beside the desk row, when that reaches further), on
        // the desk: their bottoms level with the desk row's.
        let reach = desk.iter().fold(wall, |r, (_, d)| r.union(d));
        let floor = desk
            .iter()
            .fold(row.bottom(), |b, (_, d)| b.max(d.bottom()));
        let (mut out_r, mut out_l) = (reach.right() + CASE_GAP, reach.x - CASE_GAP);
        for d in of(&[DeviceKind::Case]) {
            let s = d.size();
            let x = match layout.case_side {
                Side::Right => {
                    let x = out_r;
                    out_r += s.x + ROW_GAP;
                    x
                }
                Side::Left => {
                    out_l -= s.x;
                    let x = out_l;
                    out_l -= ROW_GAP;
                    x
                }
            };
            desk.push((d, Rect::new(x, floor - s.y, s.x, s.y)));
        }

        // Input order within each kind, monitors first.
        let order = |d: &DeviceInfo| devices.iter().position(|x| x.id == d.id);
        desk.sort_by_key(|(d, _)| order(d));
        for (d, rect) in desk {
            placed.push(Placed {
                id: d.id.clone(),
                name: d.name.clone(),
                kind: d.kind,
                rect,
                lights: Vec::new(),
            });
            let lights = lights_on(rect, &d.lights);
            if let Some(last) = placed.last_mut() {
                last.lights = lights;
            }
        }

        // The user's drags, last.
        for p in &mut placed {
            if let Some(&by) = layout.offsets.get(&p.id) {
                if by.x.is_finite() && by.y.is_finite() {
                    p.rect = p.rect.moved(by);
                    for l in &mut p.lights {
                        l.at = l.at + by;
                    }
                }
            }
        }
        let bounds = placed
            .iter()
            .map(|p| p.rect)
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default();
        RoomMap {
            devices: placed,
            bounds,
        }
    }

    pub fn device(&self, id: &str) -> Option<&Placed> {
        self.devices.iter().find(|d| d.id == id)
    }

    /// One line for diagnostics: what is placed, and how big the room is.
    pub fn summary(&self) -> String {
        if self.devices.is_empty() {
            return "empty".into();
        }
        let parts: Vec<String> = self
            .devices
            .iter()
            .map(|d| {
                format!(
                    "{} {:.0}x{:.0} at {:.0},{:.0} ({} lights)",
                    d.kind.label(),
                    d.rect.w,
                    d.rect.h,
                    d.rect.x,
                    d.rect.y,
                    d.lights.len()
                )
            })
            .collect();
        format!(
            "{}; room {:.0}x{:.0} mm",
            parts.join(", "),
            self.bounds.w,
            self.bounds.h
        )
    }
}

/// Monitors in millimetres, each at its own density. Windows' layout is in pixels, and two
/// panels of different density don't line up in millimetres, so a monitor beside the primary
/// keeps the edge that faces it (one on the left is placed by its right edge) and its top (as
/// Windows lines them up); one above or below keeps its facing edge and its centre. Panels
/// that touch in Windows then touch in the room.
fn place_monitors(monitors: &[MonitorInfo]) -> Vec<Placed> {
    let Some(p) = monitors.iter().find(|m| m.primary).or(monitors.first()) else {
        return Vec::new();
    };
    let (pl, pt, pr, pb) = p.px();
    let s = p.mm_per_px();
    monitors
        .iter()
        .map(|m| {
            let (l, t, r, b) = m.px();
            let own = m.mm_per_px();
            let (w, h) = ((r - l) * own, (b - t) * own);
            let (x, y) = if r <= pl || l >= pr {
                let x = if r <= pl {
                    (r - pl) * s - w
                } else {
                    (l - pl) * s
                };
                (x, (t - pt) * s)
            } else if b <= pt || t >= pb {
                let y = if b <= pt {
                    (b - pt) * s - h
                } else {
                    (t - pt) * s
                };
                (((l + r) / 2.0 - pl) * s - w / 2.0, y)
            } else {
                // The primary itself (or a mirrored display).
                ((l - pl) * s, (t - pt) * s)
            };
            Placed {
                id: format!("monitor:{}", m.id),
                name: m.id.trim_start_matches(r"\\.\").to_string(),
                kind: DeviceKind::Monitor,
                rect: Rect::new(x, y, w, h),
                lights: Vec::new(),
            }
        })
        .collect()
}

/// A device's lights in room space: placed ones where they say, the rest spread evenly
/// along the device's long side (top to bottom on a tall one, such as a case's front).
fn lights_on(rect: Rect, lights: &[LightSpec]) -> Vec<PlacedLight> {
    let fixed = |l: &LightSpec| l.at.filter(|p| p.x.is_finite() && p.y.is_finite());
    let loose = lights.iter().filter(|l| fixed(l).is_none()).count();
    let mut n = 0;
    lights
        .iter()
        .map(|l| {
            let at = match fixed(l) {
                Some(p) => Point::new(rect.x, rect.y) + p,
                None => {
                    let f = (n as f32 + 0.5) / loose.max(1) as f32;
                    n += 1;
                    if rect.h >= rect.w {
                        Point::new(rect.centre().x, rect.y + rect.h * f)
                    } else {
                        Point::new(rect.x + rect.w * f, rect.centre().y)
                    }
                }
            };
            PlacedLight {
                id: l.id.clone(),
                at,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
