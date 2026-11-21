// SPDX-License-Identifier: MIT
//! One rule for every dialog, confirm and box with a safe answer (Not now, Cancel): Esc gives
//! it, and so does the window's close button. The window then closes to the tray as always;
//! a dialog that ignored the close button stayed open behind it and came back with the window.

use crate::egui;

fn close_requested(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().close_requested())
}

/// Esc, or the window's close button. Esc stays readable for whoever asks next.
pub fn dismissed(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape)) || close_requested(ctx)
}

/// [`dismissed`] for a confirm or box drawn in the page: Esc is taken, so it closes nothing
/// else in the same frame.
pub fn dismissed_taking_esc(ctx: &egui::Context) -> bool {
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        || close_requested(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(esc: bool, close: bool) -> (bool, bool, bool) {
        let mut raw = egui::RawInput::default();
        if esc {
            raw.events.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        if close {
            raw.viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
        }
        let ctx = egui::Context::default();
        let mut got = (false, false, false);
        let _ = ctx.run(raw, |c| {
            got = (dismissed(c), dismissed_taking_esc(c), dismissed(c));
        });
        got
    }

    #[test]
    fn esc_and_the_close_button_dismiss_and_a_quiet_frame_does_not() {
        assert_eq!(frame(false, false), (false, false, false));
        // Esc: seen, then taken, then gone for the next asker.
        assert_eq!(frame(true, false), (true, true, false));
        // The close request is not a key: nothing takes it away.
        assert_eq!(frame(false, true), (true, true, true));
    }
}
