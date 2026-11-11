// SPDX-License-Identifier: MIT
//! The hub's built-in **Settings** page (#991, the #1087 design review): the page the plans
//! name beside Modules (`hideout-tabs-ux.md`), shown in safe mode too. Sections in order:
//! **Privacy** (what the app watch keeps, and **Forget seen apps**), then **About** (the
//! version). The hub binary owns the page and its action; this draws it.

use crate::theme::{Rgb, Tokens};
use eframe::egui::{self, RichText};

pub const ID: &str = "hub.settings";
pub const TITLE: &str = "Settings";
/// A published icon name (the gear).
pub const ICON: &str = "gear";

pub const PRIVACY: &str = "Privacy";
pub const ABOUT: &str = "About";
/// The privacy line (the #1040 review, without examples: the hub names no app, #1087).
pub const PRIVACY_LINE: &str = "Hideout notices when apps that your modules work with start or \
quit, so their tiles show up. It keeps a list of those apps on this PC only.";
pub const FORGET: &str = "Forget seen apps";
pub const FORGOTTEN: &str =
    "Hideout forgot the apps it has seen. Their tiles hide until the apps start again.";

/// "Hideout 0.3.0".
pub fn version_line(version: &str) -> String {
    format!("{} {version}", crate::PRODUCT)
}

fn c(x: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(x.0, x.1, x.2)
}

fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong().color(c(t.text)));
    ui.add_space(4.0);
}

/// Draws the page. `forgotten`: Forget seen apps was pressed (its line shows beside the button,
/// which stays, so the focus stays on it). Returns whether it was pressed now.
pub fn settings_ui(ui: &mut egui::Ui, t: &Tokens, version: &str, forgotten: bool) -> bool {
    heading(ui, t, PRIVACY);
    ui.add(egui::Label::new(RichText::new(PRIVACY_LINE).color(c(t.text))).wrap());
    ui.add_space(8.0);
    let pressed = ui
        .horizontal_wrapped(|ui| {
            let pressed = ui.button(FORGET).clicked();
            if forgotten || pressed {
                ui.label(RichText::new(FORGOTTEN).color(c(t.text_muted)));
            }
            pressed
        })
        .inner;
    ui.add_space(20.0);
    heading(ui, t, ABOUT);
    ui.label(RichText::new(version_line(version)).color(c(t.text)));
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_reads_as_the_review_says() {
        assert_eq!((ID, TITLE, ICON), ("hub.settings", "Settings", "gear"));
        assert!(hideout_contract::panel::ICONS.contains(&ICON));
        assert_eq!(version_line("0.3.0"), "Hideout 0.3.0");
        assert_eq!(
            PRIVACY_LINE,
            "Hideout notices when apps that your modules work with start or quit, so their \
             tiles show up. It keeps a list of those apps on this PC only."
        );
    }
}
