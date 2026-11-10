// SPDX-License-Identifier: MIT
//! The bar of finding the module that breaks Hideout (§7.10, #1033, docs/hideout-safe-mode-ux.md):
//! at the top of Hideout's pages while a search runs, in the safe-mode amber. A round's bar, the
//! between-rounds card, the paused and asked questions and the result are the same bar with
//! other words ([`hideout_supervisor::bisect::view`]); this file only draws them.
//!
//! Keyboard and screen reader: the bar is a landmark region named "Finding the module that
//! breaks Hideout", first in tab order (the shell draws it before the sidebar), its buttons in
//! the order they read. The shell says the view's words once in its polite live region and gives
//! a card's primary button the focus when it appears; Esc does nothing here.

use hideout_supervisor::bisect::{words, Act, View};

use crate::egui;
use crate::theme::{Rgb, Tokens};

fn c(x: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(x.0, x.1, x.2)
}

/// When the bar's "ends in <m> min" line next changes, so the window repaints once a minute
/// (only while it shows; the shell decides that).
pub fn next_minute(left: std::time::Duration) -> std::time::Duration {
    let s = left.as_secs();
    let m = s.div_ceil(60).max(1);
    // The line changes when the seconds left drop to (m - 1) minutes.
    let to = s.saturating_sub((m - 1) * 60);
    std::time::Duration::from_secs(to.max(1))
}

/// Draws the bar for `v`; `focus`: its primary button takes the focus now. Returns the button
/// clicked and the primary button's id.
pub fn bar_ui(
    ui: &mut egui::Ui,
    t: &Tokens,
    v: &View,
    focus: bool,
) -> (Option<Act>, Option<egui::Id>) {
    use egui::RichText;
    let mut act = None;
    let mut primary = None;
    ui.scope(|ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |n| {
            n.set_role(egui::accesskit::Role::Region);
            n.set_label(words::REGION);
        });
        egui::Frame::new()
            .fill(c(crate::theme::warn_fill(t)))
            .stroke(egui::Stroke::new(1.0_f32, c(t.warn)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&v.title).size(16.0).strong().color(c(t.text)));
                if !v.lines.is_empty() {
                    ui.add_space(2.0);
                }
                for line in &v.lines {
                    ui.label(RichText::new(line).color(c(t.text)));
                }
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    for (i, (a, label)) in v.buttons.iter().enumerate() {
                        let r = if i == 0 {
                            let b = egui::Button::new(RichText::new(label).color(c(t.on_accent)))
                                .fill(c(t.accent));
                            let r = ui.add(b);
                            if focus {
                                r.request_focus();
                            }
                            primary = Some(r.id);
                            r
                        } else {
                            ui.label(RichText::new("\u{b7}").color(c(t.text_muted)));
                            ui.link(RichText::new(label).color(c(t.text)))
                        };
                        if r.clicked() {
                            act = Some(*a);
                        }
                    }
                });
            });
    });
    (act, primary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_minute_line_repaints_when_it_changes() {
        // 4:59 left reads "5 min"; it reads "4 min" after 59 s.
        assert_eq!(
            next_minute(Duration::from_secs(299)),
            Duration::from_secs(59)
        );
        assert_eq!(
            next_minute(Duration::from_secs(240)),
            Duration::from_secs(60)
        );
        assert_eq!(
            next_minute(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        assert_eq!(next_minute(Duration::ZERO), Duration::from_secs(1));
    }
}
