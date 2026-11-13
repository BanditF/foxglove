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

/// A theme colour as egui's.
pub fn rgb(x: Rgb) -> egui::Color32 {
    c(x)
}

/// The dialog's two buttons, in Tab order, for tests of the focus trap.
pub(crate) type DialogIds = [egui::Id; 2];

/// The dialog, a modal like the shell's other questions: Not now is focused when it opens
/// (`first`), and Esc, a click outside, the close and Enter on the focused Not now all answer
/// it; Allow is a plain button, Not now carries the accent fill. Tab stays inside.
pub fn dialog_ui(ctx: &egui::Context, t: &Tokens, d: &Dialog, first: bool) -> Option<Answer> {
    dialog_ids(ctx, t, d, first).0
}

pub(crate) fn dialog_ids(
    ctx: &egui::Context,
    t: &Tokens,
    d: &Dialog,
    first: bool,
) -> (Option<Answer>, DialogIds) {
    let mut answer = None;
    let mut ids = [egui::Id::NULL; 2];
    let modal = egui::Modal::new(egui::Id::new("hideout.keyboard-grant")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.label(egui::RichText::new(DIALOG_TITLE).heading().color(c(t.text)));
        ui.add_space(6.0);
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
        // Wraps at the dialog's width and can be selected and copied.
        ui.add(
            egui::Label::new(egui::RichText::new(&d.path).monospace())
                .wrap()
                .selectable(true),
        );
        ui.label(egui::RichText::new(DIALOG_BINDING[1]).color(c(t.text_muted)));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let allow = ui.button(ALLOW);
            if allow.clicked() {
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
            ids = [allow.id, not_now.id];
        });
    });
    // Tab wraps inside: past the last button it goes to the first (Shift+Tab the other way)
    // instead of leaving the dialog for the page behind.
    let (tab, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.modifiers.shift));
    if tab
        && !ctx
            .memory(|m| m.focused())
            .is_some_and(|f| ids.contains(&f))
    {
        ctx.memory_mut(|m| m.request_focus(ids[usize::from(shift)]));
    }
    // The dialog's name and description for a screen reader.
    let said = format!("{DIALOG_INTRO} {} {}", DIALOG_FOOTER[0], DIALOG_FOOTER[1]);
    ctx.accesskit_node_builder(modal.response.id, |n| {
        n.set_role(egui::accesskit::Role::Dialog);
        n.set_label(DIALOG_TITLE);
        n.set_description(said);
    });
    if modal.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = answer.or(Some(Answer::NotNow));
    }
    (answer, ids)
}

/// The header line over the page. Returns `Some(true)` for Review…, `Some(false)` for Not now
/// (which hides this line until the next hub start), with the pressed button's id so the focus
/// can come back to it. A polite live region.
pub fn header_ui(ui: &mut egui::Ui, t: &Tokens, h: &Header) -> Option<(bool, egui::Id)> {
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
                    out = Some((true, review.id));
                }
                let not_now = ui.button(h.not_now);
                if not_now.clicked() {
                    out = Some((false, not_now.id));
                }
            });
        });
    out
}

/// The Permissions row. The state is always words ("On", "Off", "Asks again"), colour besides.
/// Returns what was asked and the button's id. The row is one group named by `Row::said`.
pub fn row_ui(ui: &mut egui::Ui, t: &Tokens, r: &Row) -> Option<(Ask, egui::Id)> {
    let mut out = None;
    let colour = match r.state {
        RowState::On => t.ok,
        RowState::Off => t.text_muted,
        RowState::AsksAgain => t.attention,
    };
    let group = ui.push_id("hideout.keyboard-grant-row", |ui| {
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
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
            if b.clicked() {
                out = Some((r.ask, b.id));
            }
        })
        .response
    });
    let said = r.said.clone();
    ui.ctx().accesskit_node_builder(group.inner.id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_label(said);
    });
    out
}
