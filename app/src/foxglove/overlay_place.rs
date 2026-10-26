//! The layer overlay's own place (the key popup), set by dragging it while Settings is open.
//! Without one, the overlay sits at its alignment's anchor on its display, as before.
//!
//! The overlay window covers one display. A drag follows the pointer across the desktop: when
//! the pointer leaves for another display, the overlay window moves there (at that display's
//! scaling) and the drag goes on, the same point of the overlay staying under the pointer. On
//! release the overlay is kept whole on the display under the pointer, and Settings' "Display"
//! follows. Choosing a display (or an alignment) in Settings drops the dragged place and goes
//! back to the anchor.
//!
//! The place is kept per machine, like the display (profiles never move the overlay), as the
//! top-left corner in logical units (points) from the display's work area, so it survives a
//! scale change. The logic is plain functions, tested without a window.

use serde::{Deserialize, Serialize};

use crate::monitors::Monitor;

const SECTION: &str = "overlay_place";

/// Where the overlay was dragged: its display and its top-left corner from that display's work
/// area, in points (`pt`). Both corners are `None` for the anchor. `at` is the corner in
/// physical pixels as the first version kept it; it's read, never written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    pub monitor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[i32; 2]>,
    pub pt: Option<[f32; 2]>,
}

impl Place {
    /// A dragged place (not the anchor).
    pub fn is_dragged(&self) -> bool {
        self.pt.is_some() || self.at.is_some()
    }

    /// The corner in points on a display of `scale` (physical pixels per point).
    fn corner(&self, scale: f32) -> Option<egui::Vec2> {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        match (self.pt, self.at) {
            (Some([x, y]), _) => Some(egui::vec2(x, y)),
            (None, Some([x, y])) => Some(egui::vec2(x as f32, y as f32) / scale),
            (None, None) => None,
        }
    }
}

pub fn load() -> Place {
    super::store::try_load(SECTION).unwrap_or_default()
}

pub fn save(place: &Place) {
    if let Err(e) = super::store::save(SECTION, place) {
        eprintln!("Foxglove: can't save where the overlay is: {e}");
    }
}

/// The overlay's top-left in window points, when it has a dragged place on `monitor` (the
/// display the overlay window covers, at `ppp`); `None` means "use the anchor". Kept inside
/// `window` (the overlay window's rectangle in points, its corner the work area's) for an
/// overlay of `size` points.
pub fn position(
    place: &Place,
    monitor: &str,
    ppp: f32,
    window: egui::Rect,
    size: egui::Vec2,
) -> Option<egui::Pos2> {
    if place.monitor != monitor {
        return None;
    }
    let corner = place.corner(ppp)?;
    let max = (window.max - size).max(window.min);
    Some((window.min + corner).clamp(window.min, max))
}

/// The display a desktop point (physical pixels) is on: the work area holding it, else the
/// nearest one (the taskbar, a gap between displays).
pub fn display_at(point: [i32; 2], monitors: &[Monitor]) -> Option<&Monitor> {
    let distance = |m: &Monitor| {
        let [l, t, r, b] = m.work_area;
        let dx = i64::from((l - point[0]).max(point[0] - (r - 1)).max(0));
        let dy = i64::from((t - point[1]).max(point[1] - (b - 1)).max(0));
        dx * dx + dy * dy
    };
    monitors.iter().min_by_key(|m| distance(m))
}

/// Where the overlay's corner is while dragged, in the points of the display under the pointer:
/// `cursor` in physical desktop pixels, `grab` the pointer's offset from the overlay's corner in
/// points (kept from the press, so the same point of the overlay stays under the pointer on a
/// display of another scaling). Not clamped: the overlay may hang over an edge until dropped.
pub fn dragged(cursor: [i32; 2], grab: egui::Vec2, monitors: &[Monitor]) -> Option<Place> {
    let monitor = display_at(cursor, monitors)?;
    let corner = egui::vec2(
        (cursor[0] - monitor.work_area[0]) as f32,
        (cursor[1] - monitor.work_area[1]) as f32,
    ) / monitor.scale()
        - grab;
    Some(Place {
        monitor: monitor.id.clone(),
        at: None,
        pt: Some([corner.x, corner.y]),
    })
}

/// Where a dragged overlay of `size` points lands when let go: on the display under the pointer,
/// pulled whole onto its work area.
pub fn dropped(
    cursor: [i32; 2],
    grab: egui::Vec2,
    size: egui::Vec2,
    monitors: &[Monitor],
) -> Option<Place> {
    let mut place = dragged(cursor, grab, monitors)?;
    let monitor = monitors.iter().find(|m| m.id == place.monitor)?;
    let [l, t, r, b] = monitor.work_area;
    let room = egui::vec2((r - l) as f32, (b - t) as f32) / monitor.scale() - size;
    let [x, y] = place.pt?;
    place.pt = Some([
        x.min(room.x).max(0.0).round(),
        y.min(room.y).max(0.0).round(),
    ]);
    Some(place)
}

/// A desktop point (physical pixels) in the points of a window whose top-left is `origin`
/// (physical pixels), drawn at `ppp`.
pub fn in_window(point: [i32; 2], origin: [i32; 2], ppp: f32) -> egui::Pos2 {
    let ppp = if ppp > 0.0 { ppp } else { 1.0 };
    egui::pos2(
        (point[0] - origin[0]) as f32 / ppp,
        (point[1] - origin[1]) as f32 / ppp,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::settings_window::tests::two_displays;

    const D1: &str = r"\\.\DISPLAY1";
    const D2: &str = r"\\.\DISPLAY2";

    #[test]
    fn the_display_under_a_point_or_the_nearest() {
        let d = two_displays();
        assert_eq!(display_at([100, 100], &d).map(|m| m.id.as_str()), Some(D1));
        assert_eq!(display_at([2560, 100], &d).map(|m| m.id.as_str()), Some(D2));
        // On the first display's taskbar (below its work area): still the first.
        assert_eq!(display_at([300, 1420], &d).map(|m| m.id.as_str()), Some(D1));
        assert_eq!(display_at([0, 0], &[]), None);
    }

    /// The hand-off: the same point of the overlay stays under the pointer when it crosses
    /// from the 100% display to the 150% one, and the window's points agree.
    #[test]
    fn the_grabbed_point_stays_under_the_pointer_across_displays() {
        let d = two_displays();
        let grab = egui::vec2(30.0, 20.0);
        let before = dragged([2500, 600], grab, &d).unwrap();
        assert_eq!(before.monitor, D1);
        assert_eq!(before.pt, Some([2470.0, 580.0]));

        let after = dragged([2620, 600], grab, &d).unwrap();
        assert_eq!(
            after.monitor, D2,
            "the window moves to the display under the pointer"
        );
        let [x, y] = after.pt.unwrap();
        assert_eq!([x, y], [60.0 / 1.5 - 30.0, 600.0 / 1.5 - 20.0]);
        // Back to desktop pixels: corner + (corner pt + grab) * scale is the pointer.
        let back = [2560.0 + (x + grab.x) * 1.5, (y + grab.y) * 1.5];
        assert_eq!(back, [2620.0, 600.0]);
        // Drawn in the moved window (its corner the work area's, at 1.5): the same corner.
        let p = in_window([2620, 600], [2560, 0], 1.5) - grab;
        assert_eq!([p.x, p.y], [x, y]);
        // Before the window has moved (still on the first display at 1.0), it's drawn under
        // the pointer there, hanging off the right edge.
        let p = in_window([2620, 600], [0, 0], 1.0) - grab;
        assert_eq!([p.x, p.y], [2590.0, 580.0]);
    }

    #[test]
    fn a_drop_is_pulled_whole_onto_the_display_in_points() {
        let d = two_displays();
        let size = egui::vec2(480.0, 160.0);
        // Grabbed near its right edge, let go just over the border: the overlay would hang
        // off the left of the second display, so it's pulled onto it.
        let place = dropped([2600, 300], egui::vec2(460.0, 10.0), size, &d).unwrap();
        assert_eq!(place.monitor, D2);
        assert_eq!(place.pt, Some([0.0, 190.0]));
        assert_eq!(place.at, None);
        // Past the bottom right of the 150% display (2560 x 1392 points).
        let place = dropped([6399, 2087], egui::vec2(10.0, 10.0), size, &d).unwrap();
        assert_eq!(place.pt, Some([2560.0 - 480.0, 1392.0 - 160.0]));
        // On the same display it keeps its corner.
        let place = dropped([400, 500], egui::vec2(100.0, 100.0), size, &d).unwrap();
        assert_eq!(
            (place.monitor.as_str(), place.pt),
            (D1, Some([300.0, 400.0]))
        );
    }

    #[test]
    fn the_place_shows_only_on_its_display_and_stays_inside() {
        let place = Place {
            monitor: D2.into(),
            at: None,
            pt: Some([200.0, 100.0]),
        };
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2560.0, 1392.0));
        let size = egui::vec2(480.0, 160.0);
        assert_eq!(
            position(&place, D2, 1.5, window, size),
            Some(egui::pos2(200.0, 100.0))
        );
        assert_eq!(
            position(&place, D1, 1.5, window, size),
            None,
            "another display"
        );
        assert_eq!(
            position(&Place::default(), D2, 1.5, window, size),
            None,
            "no place: the anchor"
        );
        let far = Place {
            pt: Some([5000.0, 5000.0]),
            ..place
        };
        assert_eq!(
            position(&far, D2, 1.5, window, size),
            Some(egui::pos2(2080.0, 1232.0))
        );
    }

    #[test]
    fn a_place_from_the_first_version_is_read_in_pixels() {
        let old = Place {
            monitor: D2.into(),
            at: Some([300, 150]),
            pt: None,
        };
        assert!(old.is_dragged());
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2560.0, 1392.0));
        assert_eq!(
            position(&old, D2, 1.5, window, egui::vec2(10.0, 10.0)),
            Some(egui::pos2(200.0, 100.0))
        );
    }

    #[test]
    fn the_place_round_trips_through_the_store() {
        crate::foxglove::store::test_config_dir();
        let place = Place {
            monitor: D2.into(),
            at: None,
            pt: Some([12.0, 34.5]),
        };
        save(&place);
        assert_eq!(load(), place);
    }
}
