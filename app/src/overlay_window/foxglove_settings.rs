//! Foxglove's own sections in the settings window.

use super::OverlayApp;
use crate::foxglove::settings_ui::section as titled_group;

impl OverlayApp {
    /// The keyboard itself: settings saved on it, both halves' lights, and everything its
    /// keys can do, all of which hold without Foxglove running.
    pub(super) fn foxglove_keyboard_group(ui: &mut egui::Ui) {
        crate::foxglove::update::group(ui);
        // The Keyboard page's own cards are drawn at its top by the shell (#386).
        // The Lights page (#387).
        titled_group(ui, "Lights", crate::foxglove::lights_page::show);
    }
}
