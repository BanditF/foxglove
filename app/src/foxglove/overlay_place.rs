//! The layer overlay's own place (the key popup), set by dragging it while Settings is open.
//! Without one, the overlay sits at its alignment's anchor on its display, as before. A drag can
//! end on another display: the overlay moves there, and Settings' "Display" follows. Choosing a
//! display (or an alignment) in Settings drops the dragged place and goes back to the anchor.
//!
//! The place is kept per machine, like the display (profiles never move the overlay), as the
//! top-left corner in physical pixels from the display's work area, so it survives a scale
//! change. The logic is plain functions, tested without a window.

use serde::{Deserialize, Serialize};

use crate::monitors::Monitor;

const SECTION: &str = "overlay_place";

/// Where the overlay was dragged: its display and its top-left corner, in physical pixels from
/// that display's work area. `at` is `None` for the anchor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    pub monitor: String,
    pub at: Option<[i32; 2]>,
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
/// display the overlay window covers); `None` means "use the anchor". Kept inside `window`
/// (the overlay window's rectangle in points) for an overlay of `size` points.
pub fn position(
    place: &Place,
    monitor: &str,
    ppp: f32,
    window: egui::Rect,
    size: egui::Vec2,
) -> Option<egui::Pos2> {
    let [x, y] = place.at.filter(|_| place.monitor == monitor)?;
    let ppp = if ppp > 0.0 { ppp } else { 1.0 };
    let max = (window.max - size).max(window.min);
    Some((window.min + egui::vec2(x as f32, y as f32) / ppp).clamp(window.min, max))
}

/// Where a dragged overlay lands. `rect` is the overlay in physical desktop pixels when the drag
/// ended. The display is the one its centre is on (else the one it overlaps most); the corner is
/// kept so the whole overlay stays on that display's work area. `None` when it's on no display.
pub fn dropped(rect: [i32; 4], monitors: &[Monitor]) -> Option<Place> {
    let centre = [(rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2];
    let inside =
        |a: [i32; 4]| (a[0]..a[2]).contains(&centre[0]) && (a[1]..a[3]).contains(&centre[1]);
    let overlap = |a: [i32; 4]| {
        let w = (rect[2].min(a[2]) - rect[0].max(a[0])).max(0);
        let h = (rect[3].min(a[3]) - rect[1].max(a[1])).max(0);
        i64::from(w) * i64::from(h)
    };
    let monitor = monitors.iter().find(|m| inside(m.work_area)).or_else(|| {
        monitors
            .iter()
            .filter(|m| overlap(m.work_area) > 0)
            .max_by_key(|m| overlap(m.work_area))
    })?;
    let [left, top, right, bottom] = monitor.work_area;
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let x = (rect[0] - left).min(right - left - w).max(0);
    let y = (rect[1] - top).min(bottom - top - h).max(0);
    Some(Place {
        monitor: monitor.id.clone(),
        at: Some([x, y]),
    })
}

/// A rectangle in the overlay window's points as physical desktop pixels, the window's top-left
/// being `origin` (its display's work area corner).
pub fn to_desktop(rect: egui::Rect, origin: [i32; 2], ppp: f32) -> [i32; 4] {
    let px = |v: f32| (v * ppp).round() as i32;
    [
        origin[0] + px(rect.left()),
        origin[1] + px(rect.top()),
        origin[0] + px(rect.right()),
        origin[1] + px(rect.bottom()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::settings_window::tests::two_displays;

    const D1: &str = r"\\.\DISPLAY1";
    const D2: &str = r"\\.\DISPLAY2";

    #[test]
    fn a_drop_on_the_same_display_keeps_its_corner() {
        let place = dropped([300, 400, 1020, 640], &two_displays()).unwrap();
        assert_eq!(place.monitor, D1);
        assert_eq!(place.at, Some([300, 400]));
    }

    #[test]
    fn a_drop_on_the_other_display_moves_there() {
        // Centre past x = 2560: the second display, measured from its own corner.
        let place = dropped([2300, 100, 3020, 340], &two_displays()).unwrap();
        assert_eq!(place.monitor, D2);
        assert_eq!(place.at, Some([0, 100]), "pulled fully onto it");
    }

    #[test]
    fn a_drop_past_the_edge_is_pulled_back_on_screen() {
        let place = dropped([6200, 1900, 6920, 2140], &two_displays()).unwrap();
        assert_eq!(place.monitor, D2);
        assert_eq!(place.at, Some([3840 - 720, 2088 - 240]));
        // Below everything, nothing overlapping: no place (the drag is dropped).
        assert_eq!(dropped([100, 5000, 820, 5240], &two_displays()), None);
    }

    #[test]
    fn the_place_shows_only_on_its_display_and_stays_inside() {
        let place = Place {
            monitor: D2.into(),
            at: Some([300, 150]),
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
        // A smaller scale since the drag: kept inside the window.
        let far = Place {
            monitor: D2.into(),
            at: Some([5000, 5000]),
        };
        assert_eq!(
            position(&far, D2, 1.0, window, size),
            Some(egui::pos2(2080.0, 1232.0))
        );
    }

    #[test]
    fn window_points_become_desktop_pixels() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 60.0));
        assert_eq!(to_desktop(rect, [2560, 0], 1.5), [2575, 30, 2725, 90]);
    }

    #[test]
    fn the_place_round_trips_through_the_store() {
        crate::foxglove::store::test_config_dir();
        let place = Place {
            monitor: D2.into(),
            at: Some([12, 34]),
        };
        save(&place);
        assert_eq!(load(), place);
    }
}
