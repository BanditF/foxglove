//! Foxglove's own sections in the settings window.

use super::OverlayApp;
use crate::foxglove::keyboard_settings::{self, RgbSync};
use crate::ui_widgets::titled_group;

impl OverlayApp {
    /// Settings saved on the keyboard: they hold without Foxglove running.
    pub(super) fn foxglove_keyboard_group(ui: &mut egui::Ui) {
        titled_group(ui, "Keyboard", |ui| {
            let state = keyboard_settings::rgb_sync();
            if state == RgbSync::Unknown {
                keyboard_settings::ask_rgb_sync(None, ui.ctx().clone());
            }
            ui.horizontal(|ui| {
                match state {
                    RgbSync::Known { on } => {
                        let mut follow = on;
                        if ui
                            .checkbox(&mut follow, "Right half's lights follow the left")
                            .on_hover_text(
                                "On: the left half decides when both halves' underglow is on, \
                                 so they always match (it sees key presses on both halves). \
                                 Off: each half turns its own lights off and on when idle. \
                                 Saved on the keyboard.",
                            )
                            .changed()
                        {
                            keyboard_settings::ask_rgb_sync(Some(follow), ui.ctx().clone());
                        }
                    }
                    RgbSync::Unknown | RgbSync::Asking => {
                        ui.spinner();
                        ui.weak("Asking the keyboard…");
                    }
                    RgbSync::NoKeyboard => {
                        ui.weak("Right half's lights follow the left: no keyboard with Foxglove's firmware found");
                    }
                    RgbSync::NotSupported => {
                        ui.weak("Right half's lights follow the left: needs newer keyboard firmware");
                    }
                }
                if !matches!(state, RgbSync::Unknown | RgbSync::Asking)
                    && ui.small_button("↻").on_hover_text("Ask the keyboard again").clicked()
                {
                    keyboard_settings::ask_rgb_sync(None, ui.ctx().clone());
                }
            });
        });
    }
}
