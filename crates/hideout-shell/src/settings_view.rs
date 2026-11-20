// SPDX-License-Identifier: MIT
//! The hub's built-in **Settings** page (#991, the #1087 design review): the page the plans
//! name beside Modules (`hideout-tabs-ux.md`), shown in safe mode too. Sections in order:
//! **Privacy** (what the app watch keeps, and **Forget seen apps**), then **Start with Windows**
//! (H5.2, only when the hub gives it a row), then **About** (the version). The hub binary owns the page and its action; this draws it.

use crate::startup_switch::{self, Click, Row};
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
pub const OPEN_LOGS: &str = "Open log folder";
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

/// Draws the page without a Start with Windows section.
pub fn settings_ui(ui: &mut egui::Ui, t: &Tokens, version: &str, forgotten: bool) -> bool {
    settings_ui_with(ui, t, version, forgotten, None).forget
}

/// What a frame of the page did.
#[derive(Default)]
pub struct Actions {
    pub forget: bool,
    /// Open log folder was pressed.
    pub logs: bool,
    pub startup: Option<Click>,
}

fn startup_section(ui: &mut egui::Ui, t: &Tokens, row: &Row) -> Option<Click> {
    let mut click = None;
    heading(ui, t, startup_switch::HEADING);
    ui.add(egui::Label::new(RichText::new(&row.status).color(c(t.text))).wrap());
    ui.add_space(8.0);
    if let Some(replaces) = row.dialog {
        // The confirmation: the only way to Turn on. Not now is the first button, so Enter on
        // the opening focus never turns it on by itself.
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, c(t.text_muted)))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(startup_switch::DIALOG_TITLE)
                        .strong()
                        .color(c(t.text)),
                );
                ui.add(
                    egui::Label::new(RichText::new(startup_switch::DIALOG_BODY).color(c(t.text)))
                        .wrap(),
                );
                if row.apps {
                    ui.add(
                        egui::Label::new(
                            RichText::new(startup_switch::dialog_apps(
                                &startup_switch::join_names(&row.app_names),
                                row.app_names.len() <= 1,
                            ))
                            .color(c(t.text)),
                        )
                        .wrap(),
                    );
                }
                if replaces {
                    ui.label(RichText::new(startup_switch::DIALOG_REPLACES).color(c(t.text)));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // Not now is first and takes the focus when nothing else has it.
                    let not_now = ui.button(startup_switch::DIALOG_CANCEL);
                    if ui.memory(|m| m.focused().is_none()) {
                        not_now.request_focus();
                    }
                    if not_now.clicked() {
                        click = Some(Click::Cancel);
                    }
                    if ui.button(startup_switch::DIALOG_CONFIRM).clicked() {
                        click = Some(Click::Confirm);
                    }
                });
            });
        ui.add_space(8.0);
    }
    ui.horizontal_wrapped(|ui| {
        if row.turn_on && ui.button(startup_switch::TURN_ON).clicked() {
            click = Some(Click::Ask);
        }
        if row.turn_off && ui.button(startup_switch::TURN_OFF).clicked() {
            click = Some(Click::TurnOff);
        }
        if row.remove && ui.button(startup_switch::REMOVE).clicked() {
            click = Some(Click::Remove);
        }
        if let Some(m) = &row.message {
            ui.label(RichText::new(m).color(c(t.text_muted)));
        }
    });
    ui.add_space(20.0);
    click
}

/// Draws the page. `forgotten`: Forget seen apps was pressed (its line shows beside the button,
/// which stays, so the focus stays on it). `startup`: the Start with Windows row, when the hub
/// has one.
pub fn settings_ui_with(
    ui: &mut egui::Ui,
    t: &Tokens,
    version: &str,
    forgotten: bool,
    startup: Option<&Row>,
) -> Actions {
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
    let startup = startup.and_then(|row| startup_section(ui, t, row));
    heading(ui, t, ABOUT);
    ui.label(RichText::new(version_line(version)).color(c(t.text)));
    ui.add_space(8.0);
    let logs = ui.button(OPEN_LOGS).clicked();
    Actions {
        forget: pressed,
        logs,
        startup,
    }
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
