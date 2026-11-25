// SPDX-License-Identifier: MIT
//! The hub's look in one place (review\hideout-ui-overhaul.md): the spacing grid, the type scale
//! and how a control is drawn. Pages take these from the style, so a page that adds a plain
//! `ui.button` gets the hub's button, not egui's default.

use crate::theme::{Rgb, Tokens};
use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

/// Between related items in a group.
pub const GAP: f32 = 8.0;
/// Between sections of a page.
pub const SECTION: f32 = 24.0;
/// The launcher rail on the left of the window, and one icon on it.
pub const RAIL: f32 = 56.0;
pub const RAIL_ITEM: f32 = 40.0;
/// The working area's header: the open page's name and Pop out.
pub const HEADER: f32 = 44.0;
/// The height of a button, a text box and a menu row.
pub const CONTROL: f32 = 32.0;
/// Corner radius of a control.
pub const RADIUS: u8 = 6;
/// Corner radius of a card or a dialog.
pub const CARD_RADIUS: u8 = 8;

/// The space inside a dialog's edge.
pub const DIALOG_PAD: i8 = 24;

/// Type sizes, in points.
pub const BODY: f32 = 14.0;
pub const SMALL: f32 = 12.0;
pub const TITLE: f32 = 22.0;

fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// `a` moved `by` (0 to 1) of the way to `b`.
pub fn mix(a: Rgb, b: Rgb, by: f32) -> Rgb {
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * by).round() as u8;
    Rgb(ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

/// The fill of a default (not primary) button: lifted off a card and off the page.
pub fn raised(t: &Tokens) -> Rgb {
    mix(t.surface, t.text, if t.dark { 0.08 } else { 0.06 })
}

/// Spacing and type: the same for light and dark.
pub fn metrics(s: &mut egui::Style) {
    s.spacing.item_spacing = egui::vec2(GAP, GAP);
    s.spacing.button_padding = egui::vec2(12.0, 6.0);
    s.spacing.interact_size = egui::vec2(40.0, CONTROL);
    s.spacing.menu_margin = egui::Margin::same(6);
    s.spacing.icon_width = 18.0;
    s.spacing.icon_spacing = 8.0;
    let p = FontFamily::Proportional;
    s.text_styles = [
        (TextStyle::Small, FontId::new(SMALL, p.clone())),
        (TextStyle::Body, FontId::new(BODY, p.clone())),
        (TextStyle::Button, FontId::new(BODY, p.clone())),
        (TextStyle::Heading, FontId::new(TITLE, p)),
        (
            TextStyle::Monospace,
            FontId::new(SMALL, FontFamily::Monospace),
        ),
    ]
    .into();
}

/// How controls are drawn in `t`: a default button is a raised fill with a border, corners are
/// the kit's, and the hover and press steps come from the same two colours.
pub fn controls(v: &mut egui::Visuals, t: &Tokens) {
    let r = egui::CornerRadius::same(RADIUS);
    let fill = raised(t);
    let border = egui::Stroke::new(1.0_f32, c(t.border));
    let w = &mut v.widgets;
    for (state, by) in [
        (&mut w.inactive, 0.0),
        (&mut w.hovered, 0.05),
        (&mut w.active, 0.10),
        (&mut w.open, 0.05),
    ] {
        let f = c(mix(fill, t.text, by));
        state.weak_bg_fill = f;
        state.bg_fill = f;
        state.bg_stroke = border;
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    w.hovered.bg_stroke = egui::Stroke::new(1.0_f32, c(mix(t.border, t.text, 0.25)));
    w.noninteractive.corner_radius = r;
    v.window_corner_radius = egui::CornerRadius::same(CARD_RADIUS + 4);
    v.menu_corner_radius = egui::CornerRadius::same(CARD_RADIUS);
}

/// The frame of every dialog the hub draws: the popup's fill, border and shadow, with room
/// inside.
pub fn dialog_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::popup(style)
        .inner_margin(egui::Margin::same(DIALOG_PAD))
        .corner_radius(egui::CornerRadius::same(CARD_RADIUS + 4))
}

/// The plate that says who is asking (the app's file): the same in every dialog.
pub fn plate(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(c(t.bg))
        .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
        .corner_radius(egui::CornerRadius::same(CARD_RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
}

/// A dialog's last row: `left` at the left edge (small print), the buttons of `right` against
/// the right edge, in the order they are added (so Tab order is reading order). The buttons'
/// width comes from the frame before; the very first frame draws them at the left.
pub fn dialog_footer<R>(
    ui: &mut egui::Ui,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = ui.id().with("hideout.dialog.footer");
    let w: Option<f32> = ui.data(|d| d.get_temp(id));
    ui.horizontal(|ui| {
        left(ui);
        if let Some(w) = w {
            ui.add_space((ui.available_width() - w).max(0.0));
        }
        let x0 = ui.cursor().left();
        let r = right(ui);
        let used = ui.min_rect().right() - x0;
        if w.is_none_or(|w| (w - used).abs() > 0.5) {
            ui.data_mut(|d| d.insert_temp(id, used));
            ui.ctx().request_repaint();
        }
        r
    })
    .inner
}

/// [`dialog_footer`] with the buttons only.
pub fn dialog_buttons<R>(ui: &mut egui::Ui, right: impl FnOnce(&mut egui::Ui) -> R) -> R {
    dialog_footer(ui, |_| {}, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{contrast, DARK, LIGHT};

    #[test]
    fn text_reads_on_a_default_button_in_every_state() {
        for t in [LIGHT, DARK] {
            for by in [0.0, 0.05, 0.10] {
                let fill = mix(raised(&t), t.text, by);
                assert!(contrast(t.text, fill) >= 7.0, "{t:?} {by}");
            }
        }
    }

    #[test]
    fn a_default_button_stands_off_its_card_by_its_border() {
        // The fill is a small step; the border carries the edge (3:1 is not asked of a fill
        // that has a label, WCAG 1.4.11), so it must differ from the card it sits on.
        for t in [LIGHT, DARK] {
            assert_ne!(raised(&t), t.surface);
            assert_ne!(t.border, t.surface);
        }
    }
}
