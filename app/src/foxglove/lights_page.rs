//! The Lights page (#387, epic #393), by device (#533): the Keyboard card (effect pills, colour
//! swatches and a custom colour, brightness and speed, the accent and right-half switches,
//! Apply colour, and both halves' state with Refresh), then Other devices
//! ([`super::pc_lights_ui`]). The keyboard shows once on the page. Presets (#306) go between
//! the two once there are any; while there are none the page has no Presets card.
//!
//! [`show`] gathers the cached state and acts on clicks; [`draw`] paints a [`View`] and returns
//! [`Intent`]s, so the previews and tests draw it with no keyboard. Every change is a click or
//! a slider release; the lights' state is read on request and after each change, never on a
//! timer.

use std::sync::Mutex;

use egui::{Color32, RichText, Sense, Stroke};
use foxglove_link::protocol::{RgbHalf, RgbState};

use super::keyboard_settings::{self as ks, RgbSync};
use super::settings_ui::card;
use super::theme;

/// One of the keyboard's own effects (ZMK's underglow, by its number), as data: what it is
/// called, and whether it shows the colour picked for it (#616). ZMK's Spectrum and Swirl cycle
/// the hue themselves and ignore the colour, so the page greys the colour out for them, says
/// so in their label, and the case's "Match keyboard" has no colour to copy from them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardEffect {
    pub name: &'static str,
    /// It shows the chosen colour (hue and saturation).
    pub uses_colour: bool,
    /// What its name doesn't say, shown after it.
    pub note: Option<&'static str>,
}

impl KeyboardEffect {
    /// Its name with its note: "Swirl (rainbow; colour not used yet)".
    pub fn label(&self) -> String {
        match self.note {
            Some(note) => format!("{} ({note})", self.name),
            None => self.name.to_string(),
        }
    }
}

/// The keyboard's effects by number: 0 solid, 1 breathe, 2 spectrum, 3 swirl.
pub const EFFECT_TABLE: [KeyboardEffect; 4] = [
    KeyboardEffect {
        name: "Solid",
        uses_colour: true,
        note: None,
    },
    KeyboardEffect {
        name: "Breathe",
        uses_colour: true,
        note: None,
    },
    KeyboardEffect {
        name: "Spectrum",
        uses_colour: false,
        note: Some(RAINBOW),
    },
    KeyboardEffect {
        name: "Swirl",
        uses_colour: false,
        note: Some(RAINBOW),
    },
];

const RAINBOW: &str = "rainbow; colour not used yet";

/// The effects' names, in number order (the pills).
pub const EFFECTS: [&str; 4] = [
    EFFECT_TABLE[0].name,
    EFFECT_TABLE[1].name,
    EFFECT_TABLE[2].name,
    EFFECT_TABLE[3].name,
];

/// Effect number `n`, if the keyboard has it.
pub fn effect(n: u8) -> Option<&'static KeyboardEffect> {
    EFFECT_TABLE.get(usize::from(n))
}

/// Whether the colour controls apply to the effect shown: yes unless it is one that ignores
/// the colour (an unknown effect or none read yet keeps them).
pub fn colour_used(effect_now: Option<u8>) -> bool {
    effect_now.and_then(effect).is_none_or(|e| e.uses_colour)
}

/// The width, in points, from which a page's cards (or the Keyboard card's contents) sit in
/// two columns.
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
    /// "Lights follow the wallpaper" (#70).
    pub wallpaper: bool,
    /// The keymap has an RGB key (`&rgb_ug`), which the controls go through.
    pub controls: bool,
    /// A change is being sent.
    pub busy: bool,
    /// "Find my LEDs…" can run (frames and a board profile), and how far the LED map is known.
    pub leds: (bool, super::led_map::MapState),
    /// The keyboard can be calibrated (it's connected), and whether it has been.
    pub calibration: Option<bool>,
    /// The board's parts (#559): one row each, and the split-only switch on a split.
    pub parts: super::parts::Parts,
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
    Apply {
        hue: u16,
        sat: u8,
        brightness: u8,
    },
    Accent(bool),
    /// "Lights follow the wallpaper" (#70).
    Wallpaper(bool),
    Follow(bool),
    AskFollow,
    Refresh,
    FindLeds,
    /// Open the colour calibrator on the keyboard.
    Calibrate,
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
    // "Find my LEDs…" replaces the page's cards while it's open.
    if super::led_map_ui::show(ui) {
        return;
    }
    #[cfg(test)]
    if let Some(v) = PREVIEW.with_borrow(|v| v.clone()) {
        let mut draft = Draft::default();
        draft.settle(v.state.as_ref());
        draw(ui, &v, &mut draft);
        super::pc_lights_ui::show(ui);
        super::calibrate_ui::dialog(ui.ctx());
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
        wallpaper: super::accent::wallpaper_enabled(),
        controls: super::keyboard_actions::underglow_behavior().is_some(),
        busy: ks::invoking(),
        leds: (
            super::led_map_ui::available(),
            super::led_map::state(super::boards::current().as_ref()),
        ),
        calibration: super::devices::keyboard_key()
            .map(|k| foxglove_devices::calibration::global().device(&k).is_some()),
        parts: super::parts::Parts::current(),
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
            Intent::Wallpaper(on) => {
                super::accent::set_wallpaper(on);
                ks::refresh_rgb_state();
            }
            Intent::Follow(on) => ks::ask_rgb_sync(Some(on), ctx.clone()),
            Intent::AskFollow => ks::ask_rgb_sync(None, ctx.clone()),
            Intent::Refresh => {
                ks::refresh_rgb_state();
                ctx.request_repaint();
            }
            Intent::FindLeds => {
                super::calibrate_ui::cancel();
                super::led_map_ui::open(&ctx);
                ctx.request_repaint();
            }
            Intent::Calibrate => {
                if let Some(t) = super::calibrate_ui::keyboard_target() {
                    super::calibrate_ui::open(t);
                }
            }
        }
    }
    drop(draft);
    // The case and other lights (#423), and the colour calibrator over the page while it's open.
    super::pc_lights_ui::show(ui);
    super::calibrate_ui::dialog(&ctx);
}

/// A half's lights in words: "on · Solid · 80% · speed 3", "off", or "no data yet".
pub fn half_text(half: Option<&RgbHalf>, age: Option<u8>) -> String {
    let Some(h) = half else {
        return "no data yet".into();
    };
    let mut text = if h.on {
        let effect = effect(h.effect).map_or_else(|| "other effect".into(), KeyboardEffect::label);
        format!("on · {effect} · {}% · speed {}", h.brightness, h.speed)
    } else {
        "off".to_string()
    };
    if let Some(age) = age.filter(|a| *a > super::keyboard_settings::STALE_AFTER_S) {
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

/// The colour an underglow setting shows on screen, through the keyboard's screen side.
pub(crate) fn shown_color(hue: u16, sat: u8, brightness: u8) -> Color32 {
    // Gamma space, so a swatch looks like the sRGB colour it came from.
    super::calibrate::keyboard_screen(
        egui::ecolor::HsvaGamma {
            h: f32::from(hue) / 360.0,
            s: f32::from(sat) / 100.0,
            v: f32::from(brightness.max(15)) / 100.0,
            a: 1.0,
        }
        .into(),
    )
}

/// Paints the page; returns what was asked for this frame.
pub fn draw(ui: &mut egui::Ui, v: &View, draft: &mut Draft) -> Vec<Intent> {
    let mut out = Vec::new();
    card(ui, "Keyboard", |ui| {
        let gap = 24.0;
        let width = ui.available_width();
        // Wide, the card has two columns: the colour controls, and the halves' state with the
        // keyboard's switches. Narrower, they stack.
        if width >= TWO_COLUMNS {
            let left_w = ((width - gap) * 0.6).floor();
            let right_w = width - left_w - gap;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (w, controls) in [(left_w, true), (right_w, false)] {
                    ui.allocate_ui_with_layout(
                        egui::vec2(w, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(w);
                            if controls {
                                underglow(ui, v, draft, &mut out);
                            } else {
                                keyboard_side(ui, v, &mut out);
                            }
                        },
                    );
                }
            });
        } else {
            underglow(ui, v, draft, &mut out);
            ui.add_space(8.0);
            keyboard_side(ui, v, &mut out);
        }
    });
    ui.add_space(10.0);
    out
}

/// The Keyboard card's other column: both halves as the keyboard reports them, with Refresh
/// and Calibrate…, then the switches that are saved on the keyboard.
fn keyboard_side(ui: &mut egui::Ui, v: &View, out: &mut Vec<Intent>) {
    let right = v.state.and_then(|s| s.right);
    let parts = v.parts.clone().seen(if right.is_some() { 2 } else { 1 });
    ui.label(theme::caps_label(
        ui,
        &format!("{} now", super::parts::upper_first(&parts.all())),
    ));
    let mut rows = vec![(parts.short(0), v.state.map(|s| s.left), None)];
    if parts.is_split() {
        rows.push((parts.short(1), right.map(|(h, _)| h), right.map(|(_, a)| a)));
    }
    for (name, half, age) in rows {
        super::settings_ui::pair(ui, 52.0, &name, |ui| {
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
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Refresh")
            .on_hover_text(if parts.is_split() {
                format!("Ask {} for their lights again", parts.all())
            } else {
                "Ask the keyboard for its lights again".to_string()
            })
            .clicked()
        {
            out.push(Intent::Refresh);
        }
        if let Some(calibrated) = v.calibration {
            if ui
                .button("Calibrate…")
                .on_hover_text(
                    "Match how the keyboard shows colours to your screen or another device: \
                     white, red, green, blue and grey, about a minute",
                )
                .clicked()
            {
                out.push(Intent::Calibrate);
            }
            if calibrated {
                ui.label(RichText::new("calibrated").color(theme::palette(ui).muted));
            }
        }
    });
    ui.add_space(6.0);
    // Only a split has a second part to follow the first (#559).
    if parts.is_split() {
        follow_row(ui, v, &parts, out);
    }
    // Dimming on battery (firmware D7 0E).
    ui.add_space(4.0);
    super::battery_dim_ui::show(ui);
    ui.add_space(4.0);
    if super::led_map_ui::entry(ui, v.leds.0, v.leds.1) {
        out.push(Intent::FindLeds);
    }
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
            for (i, e) in (0u8..).zip(&EFFECT_TABLE) {
                let pill = theme::pill(ui, current == Some(i), e.name);
                let pill = match e.note {
                    Some(_) => pill.on_hover_text(e.label()),
                    None => pill,
                };
                if pill.clicked() && current != Some(i) {
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
        // An effect that ignores the colour greys the colour out and says why (#616).
        let current = left.map(|l| l.effect);
        let uses_colour = colour_used(current);
        if let Some(e) = current.and_then(effect).filter(|_| !uses_colour) {
            ui.weak(format!(
                "{} is a rainbow: it doesn't use the colour yet. Brightness and speed still apply.",
                e.name
            ));
        }
        let custom_open = &mut draft.custom_open;
        ui.add_enabled_ui(uses_colour, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (name, rgb) in SWATCHES {
                    let selected = is_swatch((*hue, *sat), rgb);
                    let shown = super::calibrate::keyboard_screen(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                    if swatch(ui, shown, selected).on_hover_text(name).clicked() {
                        (*hue, *sat) = swatch_hue_sat(rgb);
                    }
                }
                // Any other colour: hue and saturation sliders behind a pill.
                let custom = !SWATCHES.iter().any(|(_, rgb)| is_swatch((*hue, *sat), *rgb));
                if theme::pill(ui, custom || *custom_open, "Custom").clicked() {
                    *custom_open = !*custom_open;
                }
            });
            if *custom_open && uses_colour {
                ui.add_space(2.0);
                value_row(ui, "Hue", format!("{hue}°"));
                full_slider(ui, egui::Slider::new(&mut *hue, 0..=359));
                value_row(ui, "Saturation", format!("{sat}%"));
                full_slider(ui, egui::Slider::new(&mut *sat, 0..=100));
            }
        });
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
        let mut wallpaper = v.wallpaper;
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
                if ui
                    .checkbox(&mut wallpaper, "Lights follow the wallpaper")
                    .on_hover_text(
                        "The underglow, and the lights set to match the keyboard, take the accent of the \
                         wallpaper on screen whenever it changes. It comes from the wallpaper's Foxglove \
                         palette (foxglove.json); a wallpaper without one gives the Windows accent colour. \
                         A running scene keeps the lights until it ends.",
                    )
                    .changed()
                {
                    out.push(Intent::Wallpaper(wallpaper));
                }
            },
        );
        if apply {
            out.push(Intent::Apply { hue: *hue, sat: *sat, brightness: *brightness });
        }
    });
    if !v.controls {
        ui.add_space(2.0);
        ui.weak("The controls need an RGB key (&rgb_ug) somewhere in the keymap.");
    }
}

/// "Right half's lights follow the left", saved on the keyboard: the second part's name, then
/// the first's.
pub fn follow_label(parts: &super::parts::Parts) -> String {
    format!(
        "{}'s lights follow the {}",
        parts.name(1),
        super::parts::lower_first(&parts.short(0))
    )
}

/// The "follow the left" switch's tooltip (#606). The left is the central, so it sees every
/// key and wakes for either half's; the right sees only its own keys.
const FOLLOW_TIP: &str = "On: the left half decides when both halves' lights are on, so they \
    always match. Off: each half dims on its own idle timer. The left wakes for a key on either \
    half; the right only for its own keys. Saved on the keyboard.";

/// "Right half's lights follow the left", saved on the keyboard.
fn follow_row(ui: &mut egui::Ui, v: &View, parts: &super::parts::Parts, out: &mut Vec<Intent>) {
    let label = follow_label(parts);
    ui.horizontal_wrapped(|ui| {
        match v.sync {
            RgbSync::Known { on } => {
                let mut follow = on;
                if ui
                    .checkbox(&mut follow, &label)
                    .on_hover_text(FOLLOW_TIP)
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
                ui.weak(format!(
                    "{label}: needs a keyboard with Foxglove's firmware"
                ));
            }
            RgbSync::NotSupported => {
                ui.weak(format!("{label}: needs newer keyboard firmware"));
            }
        }
        if !matches!(v.sync, RgbSync::Unknown | RgbSync::Asking)
            && ui.small_button("↻").on_hover_text("Refresh").clicked()
        {
            out.push(Intent::AskFollow);
        }
    });
}

/// A label with its value on the right, above a slider.
pub(crate) fn value_row(ui: &mut egui::Ui, name: &str, value: String) {
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
pub(crate) fn full_slider(ui: &mut egui::Ui, slider: egui::Slider) -> egui::Response {
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

    /// The page by device (#533): one Keyboard card holding the halves' state, Refresh and the
    /// keyboard's Calibrate…; no Presets card while there are none, no separate halves card.
    #[test]
    fn the_keyboard_card_holds_the_halves_state() {
        let ctx = super::super::pc_lights_ui::tests::test_ctx();
        let mut text = String::new();
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                draw(ui, &sample(), &mut Draft::default());
            });
            out.textures_delta.clear();
            text.clear();
            for clipped in &out.shapes {
                collect(&clipped.shape, &mut text);
            }
        }
        for want in [
            "Keyboard",
            "BOTH HALVES NOW",
            "Refresh",
            "Calibrate…",
            "calibrated",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(
            !text.contains("Presets") && !text.contains("No presets"),
            "{text}"
        );
        assert_eq!(
            text.lines().filter(|l| *l == "Keyboard").count(),
            1,
            "one card title: {text}"
        );
    }

    fn collect(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
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
            wallpaper: false,
            controls: true,
            busy: false,
            leds: (true, super::super::led_map::MapState::Shipped),
            calibration: Some(true),
            parts: crate::foxglove::fixtures::users_desk::board().parts(),
        }
    }

    /// No keyboard: nothing read, no RGB key known.
    pub fn sample_none() -> View {
        View {
            state: None,
            sync: RgbSync::NoKeyboard,
            accent: false,
            wallpaper: false,
            controls: false,
            busy: false,
            leds: (false, super::super::led_map::MapState::Unknown),
            calibration: None,
            parts: crate::foxglove::fixtures::users_desk::board().parts(),
        }
    }

    /// #559: on a one-piece board the Keyboard card has one row, the keyboard's, and no
    /// "follow the left" switch; the Corne keeps both.
    #[test]
    fn a_one_piece_board_has_no_halves_in_the_keyboard_card() {
        let ctx = super::super::pc_lights_ui::tests::test_ctx();
        let draw_text = |v: &View| {
            let mut text = String::new();
            for _ in 0..2 {
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    draw(ui, v, &mut Draft::default());
                });
                out.textures_delta.clear();
                text.clear();
                for clipped in &out.shapes {
                    collect(&clipped.shape, &mut text);
                }
            }
            text
        };
        let plank = View {
            state: Some(RgbState {
                left: half(true, 26, 3),
                right: None,
            }),
            parts: crate::foxglove::fixtures::second_desk::board().parts(),
            ..sample()
        };
        let text = draw_text(&plank);
        assert!(text.contains("THE KEYBOARD NOW"), "{text}");
        assert!(!text.to_lowercase().contains("half"), "{text}");
        assert!(!text.contains("follow"), "{text}");
        let corne = draw_text(&sample());
        assert!(
            corne.contains("Right half's lights follow the left"),
            "{corne}"
        );
    }

    /// #616: Swirl and Spectrum say they're a rainbow that doesn't use the colour, and the
    /// colour controls under them are greyed out; Solid keeps them. #606: the follow tooltip
    /// says how each half wakes with the switch off.
    #[test]
    fn a_rainbow_effect_greys_the_colour_and_says_so() {
        let ctx = super::super::pc_lights_ui::tests::test_ctx();
        let draw_text = |v: &View| {
            let mut text = String::new();
            for _ in 0..2 {
                let mut draft = Draft {
                    color: Some((26, 76, 80)),
                    ..Draft::default()
                };
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    draw(ui, v, &mut draft);
                });
                out.textures_delta.clear();
                text.clear();
                for clipped in &out.shapes {
                    collect(&clipped.shape, &mut text);
                }
            }
            text
        };
        let mut swirl = sample();
        if let Some(s) = swirl.state.as_mut() {
            s.left.effect = 3;
        }
        let text = draw_text(&swirl);
        assert!(
            text.contains("Swirl is a rainbow: it doesn't use the colour yet"),
            "{text}"
        );
        assert!(
            text.contains("Swirl (rainbow; colour not used yet)"),
            "{text}"
        );
        let text = draw_text(&sample());
        assert!(!text.contains("doesn't use the colour"), "{text}");
        assert!(FOLLOW_TIP.contains(
            "Off: each half dims on its own idle timer. The left wakes for a key on either half; \
             the right only for its own keys."
        ));
    }
}
