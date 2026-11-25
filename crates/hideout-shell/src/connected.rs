// SPDX-License-Identifier: MIT
//! Home's **Connected** section: what is connected to Hideout now, one row each, with the
//! switch that row already has elsewhere (the keyboard's Turn off, the wallpapers' Resume) or
//! a button to the page that has it. A row grants nothing and has no rule of its own: the words
//! and the actions are the ones of the page that owns it (`Page::connected`).
//!
//! Sentence case; the state is always words, colour besides.

use crate::theme::{Rgb, Tokens};
use eframe::egui;

pub const HEADING: &str = "Connected";
pub const NONE: &str = "Nothing is connected yet.";
pub const MANAGE: &str = "Manage";

pub const WALLPAPERS: &str = "Wallpapers";
pub const RESUME: &str = "Resume";
pub const RESUME_NAME: &str = "Resume wallpapers";

pub const SEEN: &str = "Apps Hideout has seen";
pub const SEEN_NAME: &str = "Manage seen apps in Settings";

fn c(rgb: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// "OBS", "OBS and Zed", "OBS and 2 more".
pub fn names_line(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, rest @ ..] => format!("{a} and {} more", rest.len()),
    }
}

/// One row of the section.
pub struct Row<'a> {
    pub title: &'a str,
    /// The state in words and its colour ("On", "Off", "Asks again"), where the row has one.
    pub state: Option<(&'a str, Rgb)>,
    pub line: &'a str,
    pub button: &'a str,
    /// The button's accessible name (it says what it acts on).
    pub button_name: &'a str,
    /// The button is the row's main action (filled).
    pub main: bool,
}

impl Row<'_> {
    /// What a screen reader says for the row: "Keyboard, On. Allowed on 3 May."
    pub fn said(&self) -> String {
        match self.state {
            Some((word, _)) => format!("{}, {word}. {}", self.title, self.line),
            None => format!("{}. {}", self.title, self.line),
        }
    }
}

/// Draws a row as a card, its button at the right, and the gap under it. Returns the button's
/// id when it was pressed. The row is one group named by [`Row::said`].
pub fn row_ui(ui: &mut egui::Ui, t: &Tokens, r: &Row) -> Option<egui::Id> {
    let mut out = None;
    let group = ui
        .push_id(("hideout.connected", r.title), |ui| {
            crate::kit::card(t)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = if r.main {
                                ui.add(
                                    egui::Button::new(
                                        egui::RichText::new(r.button).color(c(t.on_accent)),
                                    )
                                    .fill(c(t.accent)),
                                )
                            } else {
                                ui.button(r.button)
                            };
                            let name = r.button_name.to_string();
                            ui.ctx().accesskit_node_builder(b.id, |n| n.set_label(name));
                            if b.clicked() {
                                out = Some(b.id);
                            }
                            // The words take what the button leaves, and wrap there.
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(r.title).strong().color(c(t.text)),
                                    );
                                    if let Some((word, colour)) = r.state {
                                        ui.label(
                                            egui::RichText::new(word).strong().color(c(colour)),
                                        );
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(r.line)
                                        .size(crate::kit::SMALL)
                                        .color(c(t.text_muted)),
                                );
                            });
                        });
                    });
                })
                .response
        })
        .inner;
    let said = r.said();
    ui.ctx().accesskit_node_builder(group.id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_label(said);
    });
    ui.add_space(crate::kit::GAP);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_one_two_or_counted() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(names_line(&n(&[])), "");
        assert_eq!(names_line(&n(&["OBS"])), "OBS");
        assert_eq!(names_line(&n(&["OBS", "Zed"])), "OBS and Zed");
        assert_eq!(names_line(&n(&["OBS", "Zed", "Krita"])), "OBS and 2 more");
    }

    #[test]
    fn a_row_says_its_state_in_words() {
        let mut r = Row {
            title: "Keyboard",
            state: Some(("On", Rgb(0, 0, 0))),
            line: "Allowed on 3 May.",
            button: "Turn off",
            button_name: "Turn off",
            main: false,
        };
        assert_eq!(r.said(), "Keyboard, On. Allowed on 3 May.");
        r.state = None;
        assert_eq!(r.said(), "Keyboard. Allowed on 3 May.");
    }
}
