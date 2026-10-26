//! Foxglove: dragging the layer overlay to a place of its own (`foxglove::overlay_place`).
//! It can be dragged while Settings is open (the overlay shows then and takes the mouse over its
//! keys); a click without a drag still opens the keymap editor.

use super::OverlayApp;
use crate::foxglove::overlay_place;

impl OverlayApp {
    /// The overlay's top-left in window points: under the pointer while it's dragged, else where
    /// it was dropped on this display, else `None` for its alignment's anchor.
    pub(super) fn overlay_position(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
        if let Some(at) = self.ui.overlay_drag {
            return Some(at);
        }
        self.ui.overlay_place.at?;
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

    /// Follows a drag of the overlay (`rect` is where it was drawn this frame); when the drag
    /// ends, keeps the place, on another display if it was dropped there.
    pub(super) fn overlay_dragged(
        &mut self,
        ctx: &egui::Context,
        rect: egui::Rect,
        delta: egui::Vec2,
        stopped: bool,
    ) {
        if delta != egui::Vec2::ZERO {
            let from = self.ui.overlay_drag.unwrap_or(rect.min);
            self.ui.overlay_drag = Some(from + delta);
            ctx.request_repaint();
        }
        if !stopped {
            return;
        }
        let Some(at) = self.ui.overlay_drag.take() else {
            return;
        };
        ctx.request_repaint();
        let monitors = crate::monitors::list();
        let Some(current) = crate::monitors::find(&monitors, &self.settings.active.monitor) else {
            return;
        };
        // The overlay window's top-left is its display's work-area corner (keep_on_monitor).
        let origin = [current.work_area[0], current.work_area[1]];
        let dropped = egui::Rect::from_min_size(at, rect.size());
        let desktop = overlay_place::to_desktop(dropped, origin, ctx.pixels_per_point());
        let Some(place) = overlay_place::dropped(desktop, &monitors) else {
            return;
        };
        if place.monitor != current.id {
            // Settings' "Display" follows; the overlay window moves there next frame.
            self.settings.draft.monitor = place.monitor.clone();
            self.settings.active.monitor = place.monitor.clone();
            self.persist_settings();
        }
        overlay_place::save(&place);
        self.ui.overlay_place = place;
    }

    /// Back to the alignment's anchor (a display or an alignment was chosen in Settings).
    pub(super) fn reset_overlay_place(&mut self) {
        if self.ui.overlay_place.at.is_some() {
            self.ui.overlay_place = overlay_place::Place::default();
            overlay_place::save(&self.ui.overlay_place);
        }
    }

    /// The "Position" row of Settings → Layer overlay: says the overlay can be dragged, or that
    /// it was, with a button back to the anchor.
    pub(super) fn overlay_place_row(&mut self, ui: &mut egui::Ui) {
        ui.label("Position");
        if self.ui.overlay_place.at.is_some() {
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
            ui.weak("At the alignment. Drag the overlay to move it.");
        }
        ui.end_row();
    }
}
