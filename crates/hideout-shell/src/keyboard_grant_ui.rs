// SPDX-License-Identifier: MIT
//! Drawing for the keyboard grant (#1155-B): the permission dialog, the header line and the
//! Permissions row, from the words and rules of `keyboard_grant`. Drawn by the hub, never by a
//! module panel. The dialog takes focus only because the user just pressed Turn on or Review.

use crate::egui;
use crate::keyboard_grant::{
    Answer, Ask, Dialog, Header, Row, RowState, ALLOW, DIALOG_BINDING, DIALOG_FOOTER, DIALOG_INTRO,
    DIALOG_ITEMS, DIALOG_TITLE, NOT_NOW,
};
use crate::theme::{Rgb, Tokens};

fn c(x: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(x.0, x.1, x.2)
}

/// The dialog. Not now is focused when it opens (`first`), and Esc, the close box and Enter on
/// the focused Not now all answer it; Allow is a plain button, Not now carries the accent fill.
pub fn dialog_ui(ctx: &egui::Context, t: &Tokens, d: &Dialog, first: bool) -> Option<Answer> {
    let mut answer = None;
    let mut open = true;
    egui::Window::new(DIALOG_TITLE)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(460.0)
        .show(ctx, |ui| {
            if let Some(r) = d.reason {
                ui.label(egui::RichText::new(r).strong().color(c(t.attention)));
                ui.add_space(6.0);
            }
            ui.label(DIALOG_INTRO);
            ui.add_space(4.0);
            for i in DIALOG_ITEMS {
                ui.label(format!("\u{2022} {i}"));
            }
            ui.add_space(8.0);
            for f in DIALOG_FOOTER {
                ui.label(egui::RichText::new(f).color(c(t.text_muted)));
            }
            ui.add_space(8.0);
            ui.label(DIALOG_BINDING[0]);
            ui.label(egui::RichText::new(&d.path).monospace());
            ui.label(egui::RichText::new(DIALOG_BINDING[1]).color(c(t.text_muted)));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(ALLOW).clicked() {
                    answer = Some(Answer::Allow);
                }
                let not_now = ui.add(
                    egui::Button::new(egui::RichText::new(NOT_NOW).color(c(t.on_accent)))
                        .fill(c(t.accent)),
                );
                if first {
                    not_now.request_focus();
                }
                if not_now.clicked() {
                    answer = Some(Answer::NotNow);
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
        answer = answer.or(Some(Answer::NotNow));
    }
    answer
}

/// The header line over the Foxglove page. Returns `Some(true)` for Review…, `Some(false)` for
/// Not now (which hides the line until the next hub start). A polite live region.
pub fn header_ui(ui: &mut egui::Ui, t: &Tokens, h: &Header) -> Option<bool> {
    let mut out = None;
    egui::Frame::new()
        .fill(c(t.surface))
        .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let head = ui.label(egui::RichText::new(h.title).strong().color(c(t.text)));
            let said = h.title;
            ui.ctx().accesskit_node_builder(head.id, |n| {
                n.set_live(egui::accesskit::Live::Polite);
                n.set_label(said);
            });
            ui.label(egui::RichText::new(h.body).color(c(t.text_muted)));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let review = ui.add(
                    egui::Button::new(egui::RichText::new(h.review).color(c(t.on_accent)))
                        .fill(c(t.accent)),
                );
                if review.clicked() {
                    out = Some(true);
                }
                if ui.button(h.not_now).clicked() {
                    out = Some(false);
                }
            });
        });
    out
}

/// The Permissions row. The state is always words ("On", "Off", "Asks again"), colour besides.
pub fn row_ui(ui: &mut egui::Ui, t: &Tokens, r: &Row) -> Option<Ask> {
    let mut out = None;
    let colour = match r.state {
        RowState::On => t.ok,
        RowState::Off => t.text_muted,
        RowState::AsksAgain => t.attention,
    };
    ui.push_id("hideout.keyboard-grant-row", |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(r.title).strong().color(c(t.text)));
            ui.label(
                egui::RichText::new(r.state.word())
                    .strong()
                    .color(c(colour)),
            );
        });
        ui.label(egui::RichText::new(&r.line).color(c(t.text_muted)));
        let b = if r.main {
            ui.add(
                egui::Button::new(egui::RichText::new(r.button).color(c(t.on_accent)))
                    .fill(c(t.accent)),
            )
        } else {
            ui.button(r.button)
        };
        let said = r.said.clone();
        ui.ctx()
            .accesskit_node_builder(b.id, |n| n.set_description(said));
        if b.clicked() {
            out = Some(r.ask);
        }
    });
    out
}
