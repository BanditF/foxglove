//! Foxglove: dragging the layer overlay to a place of its own (`foxglove::overlay_place`).
//! It can be dragged while Settings is open (the overlay shows then and takes the mouse over its
//! keys); a click without a drag still opens the keymap editor. A drag that reaches another
//! display moves the overlay window there and carries on.

use super::OverlayApp;
use crate::foxglove::overlay_place;

/// A drag under way: the pointer's offset from the overlay's corner (points) and the display
/// the overlay was on when it started.
pub struct Drag {
    grab: egui::Vec2,
    from: String,
}

impl OverlayApp {
    /// The overlay's top-left in window points: under the pointer while it's dragged, else where
    /// it was dropped on this display, else `None` for its alignment's anchor.
    pub(super) fn overlay_position(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
        if let Some(at) = self.ui.overlay_drag {
            return Some(at);
        }
        if !self.ui.overlay_place.is_dragged() {
            return None;
        }
        let monitors = crate::monitors::list();
        let current = crate::monitors::find(&monitors, &self.settings.active.monitor)?;
        overlay_place::position(
            &self.ui.overlay_place,
            &current.id,
            ctx.pixels_per_point(),
            ctx.viewport_rect(),
            self.ui.overlay_size,
        )
    }

    /// Follows a drag of the overlay (`rect` is where it was drawn this frame, `dragging` and
    /// `stopped` egui's view of it). The overlay follows the pointer across the desktop, its
    /// window moving to the display under the pointer; on release the place is kept.
    pub(super) fn overlay_dragged(
        &mut self,
        ctx: &egui::Context,
        rect: egui::Rect,
        dragging: bool,
        stopped: bool,
    ) {
        if !dragging && !stopped && self.ui.overlay_grab.is_none() {
            return;
        }
        let monitors = crate::monitors::list();
        let ppp = ctx.pixels_per_point();
        let origin = window_origin(ctx, &monitors, &self.settings.active.monitor);
        let Some(cursor) = crate::monitors::cursor().or_else(|| {
            // Without the OS's pointer (other platforms): egui's, in this window.
            let p = ctx.input(|i| i.pointer.latest_pos())?;
            Some([
                origin[0] + (p.x * ppp).round() as i32,
                origin[1] + (p.y * ppp).round() as i32,
            ])
        }) else {
            return;
        };
        let drag = self.ui.overlay_grab.get_or_insert_with(|| Drag {
            grab: overlay_place::in_window(cursor, origin, ppp) - rect.min,
            from: self.settings.active.monitor.clone(),
        });
        let grab = drag.grab;
        ctx.request_repaint();

        if dragging && !stopped {
            self.ui.overlay_drag = Some(overlay_place::in_window(cursor, origin, ppp) - grab);
            // The pointer reached another display: the overlay window moves there next frame
            // (keep_on_monitor, at that display's scaling) and the drag goes on in it.
            if let Some(there) = overlay_place::dragged(cursor, grab, &monitors) {
                let hosted = crate::monitors::find(&monitors, &self.settings.active.monitor);
                if hosted.is_some_and(|m| m.id != there.monitor) {
                    self.settings.draft.monitor = there.monitor.clone();
                    self.settings.active.monitor = there.monitor;
                }
            }
            return;
        }

        // Let go (or egui lost the drag): keep it whole on the display under the pointer.
        let from = self.ui.overlay_grab.take().map(|d| d.from).unwrap_or_default();
        self.ui.overlay_drag = None;
        let Some(place) = overlay_place::dropped(cursor, grab, rect.size(), &monitors) else {
            return;
        };
        if place.monitor != self.settings.active.monitor || place.monitor != from {
            // Settings' "Display" follows.
            self.settings.draft.monitor = place.monitor.clone();
            self.settings.active.monitor = place.monitor.clone();
            self.persist_settings();
        }
        overlay_place::save(&place);
        self.ui.overlay_place = place;
    }

    /// Ends a drag without keeping it (Settings closed): the overlay goes back to the display
    /// it was dragged from.
    pub(super) fn cancel_overlay_drag(&mut self) {
        self.ui.overlay_drag = None;
        if let Some(drag) = self.ui.overlay_grab.take() {
            self.settings.draft.monitor = drag.from.clone();
            self.settings.active.monitor = drag.from;
        }
    }

    /// Back to the alignment's anchor (a display or an alignment was chosen in Settings).
    pub(super) fn reset_overlay_place(&mut self) {
        if self.ui.overlay_place.is_dragged() {
            self.ui.overlay_place = overlay_place::Place::default();
            overlay_place::save(&self.ui.overlay_place);
        }
    }

    /// The "Position" row of Settings → Layer overlay: says the overlay can be dragged, or that
    /// it was, with a button back to the anchor.
    pub(super) fn overlay_place_row(&mut self, ui: &mut egui::Ui) {
        ui.label("Position");
        if self.ui.overlay_place.is_dragged() {
            ui.horizontal(|ui| {
                ui.label("Where you dragged it");
                if ui
                    .small_button("Reset")
                    .on_hover_text("Back to the alignment on its display")
                    .clicked()
                {
                    self.reset_overlay_place();
                }
            });
        } else {
            ui.weak("At the alignment. Drag the overlay to move it, to any display.");
        }
        ui.end_row();
    }
}

/// The overlay window's top-left in physical desktop pixels: where the window really is (it may
/// not have moved to a newly chosen display yet), else its display's work-area corner.
fn window_origin(ctx: &egui::Context, monitors: &[crate::monitors::Monitor], id: &str) -> [i32; 2] {
    let ppp = ctx.pixels_per_point();
    if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
        return [
            (outer.min.x * ppp).round() as i32,
            (outer.min.y * ppp).round() as i32,
        ];
    }
    crate::monitors::find(monitors, id).map_or([0, 0], |m| [m.work_area[0], m.work_area[1]])
}
