// SPDX-License-Identifier: MIT
//! The words of the hand-over from a standalone copy (first-run plan FR5,
//! `ux-plan-first-run.md` section 2 and decision 5).
//!
//! When an app is open on its own and the user allows it in Hideout, Hideout may close that copy
//! and start the app inside Hideout. The allow dialog says so in one line; if the copy stays open
//! (Hideout was in any doubt, the copy did not close, or someone else answered), a note on the
//! Modules page says the copy is still open and offers **Start again**. The app's name always
//! comes from the app's own manifest, never from here, and the one noun is "app".

/// The allow dialog's line, shown only when a standalone copy is open right now.
pub fn dialog_line(app: &str) -> String {
    format!(
        "{app} is open on its own right now. When you allow, Hideout closes that copy and starts \
         {app} inside Hideout. Your settings stay."
    )
}

/// The note when the copy stayed open.
pub fn refusal_line(app: &str) -> String {
    format!("{app} is still open on its own. Close it from its tray icon, then press Start again.")
}

/// The note's one button.
pub const START_AGAIN: &str = "Start again";

/// The note on the Modules page after a hand-over that did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// The app's id (what Start again starts).
    pub id: String,
    /// The app's name, from its manifest.
    pub name: String,
}

impl Note {
    pub fn line(&self) -> String {
        refusal_line(&self.name)
    }
}

#[cfg(windows)]
pub use ui::note_ui;

#[cfg(windows)]
mod ui {
    use super::{Note, START_AGAIN};
    use crate::theme::{Rgb, Tokens};
    use eframe::egui::{self, RichText};

    fn c(x: Rgb) -> egui::Color32 {
        egui::Color32::from_rgb(x.0, x.1, x.2)
    }

    /// Draws the note above the rows; true when Start again was pressed.
    pub fn note_ui(ui: &mut egui::Ui, t: &Tokens, note: &Note) -> bool {
        let mut again = false;
        egui::Frame::new()
            .fill(c(t.surface))
            .stroke(egui::Stroke::new(1.0_f32, c(t.attention)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let line = ui.label(RichText::new(note.line()).color(c(t.text)));
                ui.ctx().accesskit_node_builder(line.id, |n| {
                    n.set_live(egui::accesskit::Live::Polite);
                });
                ui.add_space(8.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new(START_AGAIN).color(c(t.on_accent)))
                            .fill(c(t.accent)),
                    )
                    .clicked()
                {
                    again = true;
                }
            });
        ui.add_space(8.0);
        again
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_are_the_plans_words_with_the_apps_own_name() {
        assert_eq!(
            dialog_line("Foxglove"),
            "Foxglove is open on its own right now. When you allow, Hideout closes that copy and \
             starts Foxglove inside Hideout. Your settings stay."
        );
        assert_eq!(
            refusal_line("Gizmo"),
            "Gizmo is still open on its own. Close it from its tray icon, then press Start again."
        );
    }

    #[test]
    fn no_banned_word_and_no_product_name_of_its_own() {
        let words = [
            dialog_line("Gizmo"),
            refusal_line("Gizmo"),
            START_AGAIN.to_string(),
        ];
        for w in &words {
            let l = w.to_lowercase();
            for banned in [
                "module",
                "manifest",
                "grant",
                "capability",
                "pipe",
                "process",
            ] {
                assert!(!l.contains(banned), "{w:?} says {banned}");
            }
            // The only product named besides the app is the hub itself.
            assert!(!l.contains("foxglove"), "{w:?} names an app of its own");
        }
    }
}
