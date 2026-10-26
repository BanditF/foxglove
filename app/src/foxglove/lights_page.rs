//! The Lights page (#387, epic #393), as the approved canvas draws it: an Underglow card
//! (effect pills, colour swatches and a custom colour, brightness and speed, the accent and
//! right-half switches, Apply colour) beside a Lighting presets card and both halves' state.
//!
//! [`show`] gathers the cached state and acts on clicks; [`draw`] paints a [`View`] and returns
//! [`Intent`]s, so the previews and tests draw it with no keyboard. Every change is a click or
//! a slider release; the lights' state is read on request and after each change, never on a
//! timer.

use std::sync::Mutex;

use digitalis::protocol::{RgbHalf, RgbState};
use egui::{Color32, RichText, Sense, Stroke};

use super::keyboard_settings::{self as ks, RgbSync};
use super::settings_ui::card;
use super::theme;

pub const EFFECTS: [&str; 4] = ["Solid", "Breathe", "Spectrum", "Swirl"];

/// The page's width, in points, from which the cards sit in two columns.
pub const TWO_COLUMNS: f32 = 760.0;

/// The canvas's swatches: (name, sRGB).
pub const SWATCHES: [(&str, [u8; 3]); 6] = [
    ("Bandit orange", [0xf0, 0x8a, 0x3c]),
    ("Ember", [0xff, 0x5f, 0x3c]),
    ("Amber", [0xf0, 0xd2, 0x3c]),
    ("Pine", [0x5f, 0xc4, 0x8e]),
    ("Steel blue", [0x7f, 0xb0, 0xe0]),
    ("White", [0xe6, 0xe6, 0xe6]),
];

/// What the page shows.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// Both halves' lights as last read; `None` before the first answer or with no keyboard.
    pub state: Option<RgbState>,
    pub sync: RgbSync,
    /// "Follow the Windows accent colour".
    pub accent: bool,
    /// The keymap has an RGB key (`&rgb_ug`), which the controls go through.
    pub controls: bool,
    /// A change is being sent.
    pub busy: bool,
}

/// What the user is choosing but hasn't sent: the colour until Apply, the speed until the
/// keyboard reports it back (ZMK saves it late).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Draft {
    /// Hue 0-359, saturation and brightness 0-100.
    pub color: Option<(u16, u8, u8)>,
    pub speed: Option<u8>,
    /// The custom colour's hue and saturation sliders are open.
    pub custom_open: bool,
}

impl Draft {
    /// Seeds the colour from the left half once, and drops a speed the keyboard now reports.
    pub fn settle(&mut self, state: Option<&RgbState>) {
        let Some(s) = state else { return };
        if self.color.is_none() {
            self.color = Some((s.left.hue, s.left.sat, s.left.brightness));
        }
        if self.speed == Some(s.left.speed) {
            self.speed = None;
        }
    }
}

/// What a click or slider asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Intent {
    Lights(bool),
    Effect(u8),
    Speed(u8),
    Apply { hue: u16, sat: u8, brightness: u8 },
    Accent(bool),
    Follow(bool),
    AskFollow,
    Refresh,
}

static DRAFT: Mutex<Draft> = Mutex::new(Draft {
    color: None,
    speed: None,
    custom_open: false,
});

#[cfg(test)]
thread_local! {
    /// The previews' page, so they never ask a real keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<View>> = const { std::cell::RefCell::new(None) };
}

/// The Lights section: the page, and what its clicks do.
pub fn show(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some(v) = PREVIEW.with_borrow(|v| v.clone()) {
        let mut draft = Draft::default();
        draft.settle(v.state.as_ref());
        draw(ui, &v, &mut draft);
        if !super::calibrate_ui::show(ui) {
            super::pc_lights_ui::show(ui);
        }
        return;
    }
    let ctx = ui.ctx().clone();
    let sync = ks::rgb_sync();
    if sync == RgbSync::Unknown {
        ks::ask_rgb_sync(None, ctx.clone());
    }
    let view = View {
        state: ks::rgb_state(&ctx),
        sync,
        accent: super::accent::enabled(),
        controls: super::keyboard_actions::underglow_behavior().is_some(),
        busy: ks::invoking(),
    };
    let mut draft = DRAFT.lock().unwrap_or_else(|e| e.into_inner());
    draft.settle(view.state.as_ref());
    for intent in draw(ui, &view, &mut draft) {
        match intent {
            Intent::Lights(on) => ks::set_lights(on, ctx.clone()),
            Intent::Effect(e) => ks::set_effect(e, ctx.clone()),
            Intent::Speed(s) => ks::set_speed(s, ctx.clone()),
            Intent::Apply {
                hue,
                sat,
                brightness,
            } => ks::set_color(hue, sat, brightness, ctx.clone()),
            Intent::Accent(on) => {
                super::accent::set_enabled(on);
                ks::refresh_rgb_state();
            }
            Intent::Follow(on) => ks::ask_rgb_sync(Some(on), ctx.clone()),
            Intent::AskFollow => ks::ask_rgb_sync(None, ctx.clone()),
            Intent::Refresh => {
                ks::refresh_rgb_state();
                ctx.request_repaint();
            }
        }
    }
    drop(draft);
    // The case and other lights (#423), or the colour calibrator while it's open.
    if !super::calibrate_ui::show(ui) {
        super::pc_lights_ui::show(ui);
    }
}

/// A half's lights in words: "on · Solid · 80% · speed 3", "off", or "no data yet".
pub fn half_text(half: Option<&RgbHalf>, age: Option<u8>) -> String {
    let Some(h) = half else {
        return "no data yet".into();
    };
    let mut text = if h.on {
        let effect = EFFECTS
            .get(usize::from(h.effect))
            .copied()
            .unwrap_or("other effect");
        format!("on · {effect} · {}% · speed {}", h.brightness, h.speed)
    } else {
        "off".to_string()
    };
    if let Some(age) = age.filter(|a| *a > 3) {
        text.push_str(&format!(" · {age} s ago"));
    }
    text
}

/// A swatch's hue and saturation, as the underglow takes them.
fn swatch_hue_sat(rgb: [u8; 3]) -> (u16, u8) {
    super::accent::hue_sat(rgb[0], rgb[1], rgb[2])
}

/// Whether the drafted colour is this swatch (within a few steps, as the keyboard rounds).
pub fn is_swatch(color: (u16, u8), rgb: [u8; 3]) -> bool {
    let (h, s) = swatch_hue_sat(rgb);
    let dh = (i32::from(color.0) - i32::from(h)).rem_euclid(360);
    let near_hue = dh.min(360 - dh) <= 3 || s < 5;
    near_hue && (i32::from(color.1) - i32::from(s)).abs() <= 3
}

/// The colour an underglow setting shows on screen.
pub(crate) fn shown_color(hue: u16, sat: u8, brightness: u8) -> Color32 {
    // Gamma space, so a swatch looks like the sRGB colour it came from.
    egui::ecolor::HsvaGamma {
        h: f32::from(hue) / 360.0,
        s: f32::from(sat) / 100.0,
        v: f32::from(brightness.max(15)) / 100.0,
        a: 1.0,
    }
    .into()
}

/// Paints the page; returns what was asked for this frame.
pub fn draw(ui: &mut egui::Ui, v: &View, draft: &mut Draft) -> Vec<Intent> {
    let mut out = Vec::new();
    let gap = 16.0;
    let width = ui.available_width();
    // Side by side only when both columns have room for their rows: the Underglow card's
    // colour row and the halves' longest line. Narrower, the cards stack.
    let wide = width >= TWO_COLUMNS;
    let left_w = if wide {
        ((width - gap) * 0.6).floor()
    } else {
        width
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(left_w);
                card(ui, "Underglow", |ui| underglow(ui, v, draft, &mut out));
            },
        );
        if wide {
            let right_w = width - left_w - gap;
            ui.allocate_ui_with_layout(
                egui::vec2(right_w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right_w);
                    side(ui, v, &mut out);
                },
            );
        }
    });
    if !wide {
        ui.add_space(10.0);
        side(ui, v, &mut out);
    }
    ui.add_space(6.0);
    out
}

/// The right-hand column: presets, then both halves as the keyboard reports them.
fn side(ui: &mut egui::Ui, v: &View, out: &mut Vec<Intent>) {
    card(ui, "Lighting presets", |ui| {
        ui.weak(
            "Looks that match the Bandit wallpapers, with their right-screen art if you want it.",
        );
        ui.add_space(2.0);
        // The presets' apply path is #306; until it lands there are none to list.
        ui.label(RichText::new("No presets in this version yet.").color(theme::palette(ui).muted));
    });
    ui.add_space(10.0);
    card(ui, "Both halves now", |ui| {
        let right = v.state.and_then(|s| s.right);
        for (name, half, age) in [
            ("Left", v.state.map(|s| s.left), None),
            ("Right", right.map(|(h, _)| h), right.map(|(_, a)| a)),
        ] {
            super::settings_ui::pair(ui, 52.0, name, |ui| {
                if let Some(h) = half {
                    let color = if h.on {
                        shown_color(h.hue, h.sat, h.brightness)
                    } else {
                        Color32::from_gray(20)
                    };
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, color);
                }
                ui.add(egui::Label::new(half_text(half.as_ref(), age)).wrap());
            });
        }
        ui.add_space(2.0);
        if ui
            .button("Refresh lights")
            .on_hover_text("Ask both halves for their lights again")
            .clicked()
        {
            out.push(Intent::Refresh);
        }
    });
}

fn underglow(ui: &mut egui::Ui, v: &View, draft: &mut Draft, out: &mut Vec<Intent>) {
    let left = v.state.map(|s| s.left);
    ui.add_enabled_ui(v.controls && !v.busy, |ui| {
        let on = left.map(|l| l.on);
        super::settings_ui::split_row(
            ui,
            |ui| {
                if theme::pill(ui, on == Some(false), "Off").clicked() {
                    out.push(Intent::Lights(false));
                }
                if theme::pill(ui, on == Some(true), "On").clicked() {
                    out.push(Intent::Lights(true));
                }
            },
            |ui| {
                ui.label(theme::caps_label(ui, "Effect"));
            },
        );
        ui.horizontal_wrapped(|ui| {
            let current = left.map(|l| l.effect);
            for (i, name) in (0u8..).zip(EFFECTS) {
                if theme::pill(ui, current == Some(i), name).clicked() && current != Some(i) {
                    out.push(Intent::Effect(i));
                }
            }
        });
        ui.add_space(4.0);
        ui.label(theme::caps_label(ui, "Colour"));
        let Some((hue, sat, brightness)) = draft.color.as_mut() else {
            ui.weak("Waiting for the keyboard's colour…");
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for (name, rgb) in SWATCHES {
                let selected = is_swatch((*hue, *sat), rgb);
                if swatch(ui, Color32::from_rgb(rgb[0], rgb[1], rgb[2]), selected).on_hover_text(name).clicked() {
                    (*hue, *sat) = swatch_hue_sat(rgb);
                }
            }
            // Any other colour: hue and saturation sliders behind a pill.
            let custom = !SWATCHES.iter().any(|(_, rgb)| is_swatch((*hue, *sat), *rgb));
            if theme::pill(ui, custom || draft.custom_open, "Custom").clicked() {
                draft.custom_open = !draft.custom_open;
            }
        });
        if draft.custom_open {
            ui.add_space(2.0);
            value_row(ui, "Hue", format!("{hue}°"));
            full_slider(ui, egui::Slider::new(hue, 0..=359));
            value_row(ui, "Saturation", format!("{sat}%"));
            full_slider(ui, egui::Slider::new(sat, 0..=100));
        }
        ui.add_space(4.0);
        value_row(ui, "Brightness", format!("{brightness}%"));
        full_slider(ui, egui::Slider::new(brightness, 0..=100));

        let reported = left.map(|l| l.speed).filter(|s| (1..=5).contains(s));
        let mut speed = draft.speed.or(reported).unwrap_or(3);
        value_row(ui, "Speed", speed.to_string());
        let r = full_slider(ui, egui::Slider::new(&mut speed, 1..=5));
        if r.changed() {
            draft.speed = Some(speed);
        }
        // Sent when the drag ends (or on a key press), not on every step of a drag.
        if (r.drag_stopped() || (r.changed() && !r.dragged())) && Some(speed) != reported {
            out.push(Intent::Speed(speed));
        }
        ui.add_space(4.0);
        let mut accent = v.accent;
        let mut apply = false;
        super::settings_ui::split_row(
            ui,
            |ui| {
                apply = ui.add(theme::primary_button(ui, "Apply colour")).clicked();
                let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 14.0), Sense::hover());
                ui.painter().rect_filled(rect, 3.0, shown_color(*hue, *sat, *brightness));
            },
            |ui| {
            if ui
                .checkbox(&mut accent, "Follow the Windows accent colour")
                .on_hover_text(
                    "The underglow takes the accent colour's hue and saturation whenever it changes in Windows, \
                     and when the keyboard connects. The brightness stays the board's.",
                )
                .changed()
            {
                out.push(Intent::Accent(accent));
            }
            },
        );
        if apply {
            out.push(Intent::Apply { hue: *hue, sat: *sat, brightness: *brightness });
        }
    });
    follow_row(ui, v, out);
    if !v.controls {
        ui.add_space(2.0);
        ui.weak("The controls need an RGB key (&rgb_ug) somewhere in the keymap.");
    }
}

/// "Right half's lights follow the left", saved on the keyboard.
fn follow_row(ui: &mut egui::Ui, v: &View, out: &mut Vec<Intent>) {
    ui.horizontal_wrapped(|ui| {
        match v.sync {
            RgbSync::Known { on } => {
                let mut follow = on;
                if ui
                    .checkbox(&mut follow, "Right half's lights follow the left")
                    .on_hover_text(
                        "On: the left half decides when both halves' underglow is on, so they always match \
                         (it sees key presses on both halves). Off: each half turns its own lights off and on \
                         when idle. Saved on the keyboard.",
                    )
                    .changed()
                {
                    out.push(Intent::Follow(follow));
                }
            }
            RgbSync::Unknown | RgbSync::Asking => {
                ui.spinner();
                ui.weak("Asking the keyboard…");
            }
            RgbSync::NoKeyboard => {
                ui.weak("Right half's lights follow the left: needs a keyboard with Foxglove's firmware");
            }
            RgbSync::NotSupported => {
                ui.weak("Right half's lights follow the left: needs newer keyboard firmware");
            }
        }
        if !matches!(v.sync, RgbSync::Unknown | RgbSync::Asking)
            && ui.small_button("↻").on_hover_text("Ask the keyboard again").clicked()
        {
            out.push(Intent::AskFollow);
        }
    });
}

/// A label with its value on the right, above a slider.
fn value_row(ui: &mut egui::Ui, name: &str, value: String) {
    super::settings_ui::split_row(
        ui,
        |ui| {
            ui.label(RichText::new(value).font(theme::mono(12.5)));
        },
        |ui| {
            ui.add(egui::Label::new(name).truncate());
        },
    );
}

/// A slider across the card, its value shown in the row above.
fn full_slider(ui: &mut egui::Ui, slider: egui::Slider) -> egui::Response {
    ui.spacing_mut().slider_width = ui.available_width();
    ui.add(slider.show_value(false))
}

/// A round colour swatch; selected ones get a light ring, as drawn.
pub fn swatch(ui: &mut egui::Ui, color: Color32, selected: bool) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
    let painter = ui.painter();
    let c = rect.center();
    if selected {
        painter.circle_stroke(c, 15.5, Stroke::new(2.0, p.text_strong));
    } else if response.hovered() || response.has_focus() {
        painter.circle_stroke(c, 15.5, Stroke::new(2.0, p.accent));
    }
    painter.circle_filled(c, 14.0, p.ground);
    painter.circle_filled(c, 12.0, color);
    response
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn half(on: bool, hue: u16, speed: u8) -> RgbHalf {
        RgbHalf {
            on,
            hue,
            sat: 76,
            brightness: 80,
            effect: 0,
            speed,
        }
    }

    #[test]
    fn halves_in_words() {
        assert_eq!(half_text(None, None), "no data yet");
        assert_eq!(
            half_text(Some(&half(true, 26, 3)), None),
            "on · Solid · 80% · speed 3"
        );
        assert_eq!(half_text(Some(&half(false, 26, 3)), Some(2)), "off");
        assert_eq!(
            half_text(Some(&half(false, 26, 3)), Some(12)),
            "off · 12 s ago"
        );
        let odd = RgbHalf {
            effect: 9,
            ..half(true, 0, 1)
        };
        assert!(half_text(Some(&odd), None).contains("other effect"));
    }

    #[test]
    fn swatches_match_their_own_colour_only() {
        for (name, rgb) in SWATCHES {
            let hs = swatch_hue_sat(rgb);
            let matching: Vec<_> = SWATCHES
                .iter()
                .filter(|(_, other)| is_swatch(hs, *other))
                .map(|(n, _)| *n)
                .collect();
            assert_eq!(matching, vec![name], "{name}");
        }
        assert!(
            is_swatch((358, 1), [0xe6, 0xe6, 0xe6]),
            "white has no hue to match"
        );
    }

    #[test]
    fn the_draft_seeds_once_and_lets_the_speed_catch_up() {
        let state = RgbState {
            left: half(true, 26, 3),
            right: None,
        };
        let mut d = Draft::default();
        d.settle(None);
        assert_eq!(d, Draft::default());
        d.settle(Some(&state));
        assert_eq!(d.color, Some((26, 76, 80)));
        d.color = Some((200, 50, 40));
        d.speed = Some(5);
        d.settle(Some(&state));
        assert_eq!(
            d.color,
            Some((200, 50, 40)),
            "the user's pick stays until Apply"
        );
        assert_eq!(d.speed, Some(5), "ZMK still reports the old speed");
        d.settle(Some(&RgbState {
            left: half(true, 26, 5),
            right: None,
        }));
        assert_eq!(d.speed, None, "the keyboard caught up");
    }

    /// The previews' sample: both halves on, the right one reporting a few seconds late.
    pub fn sample() -> View {
        View {
            state: Some(RgbState {
                left: half(true, 26, 3),
                right: Some((half(true, 26, 3), 1)),
            }),
            sync: RgbSync::Known { on: true },
            accent: false,
            controls: true,
            busy: false,
        }
    }

    /// No keyboard: nothing read, no RGB key known.
    pub fn sample_none() -> View {
        View {
            state: None,
            sync: RgbSync::NoKeyboard,
            accent: false,
            controls: false,
            busy: false,
        }
    }
}
