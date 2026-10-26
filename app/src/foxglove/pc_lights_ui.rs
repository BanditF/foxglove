//! The Lights page's "PC & peripherals" card (#423): each lighting device Foxglove found, how
//! it's controlled, Identify, a colour per zone, Off and "Match keyboard".
//!
//! [`show`] reads the worker's snapshot and sends clicks to it; [`draw`] paints a
//! [`Snapshot`] and returns [`Intent`]s, so the previews and tests need no hardware. Colours
//! are picked first and written on Apply: dragging a picker sends nothing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Color32, RichText, Sense, Stroke};
use foxglove_devices::pc_lighting::Control;

use super::pc_lights::{self, Msg, Row, Snapshot};
use super::settings_ui::card;
use super::theme;

/// What a click asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// Zone index and colour, the zones the user changed.
    Apply {
        key: String,
        colors: Vec<(usize, [u8; 3])>,
    },
    Off(String),
    Identify(String),
    Match(String, bool),
}

/// The colours being picked per device (by key), until Apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub colors: BTreeMap<String, Vec<Option<[u8; 3]>>>,
}

impl Draft {
    /// The changed zones of a device, as Apply sends them.
    pub fn changes(&self, row: &Row) -> Vec<(usize, [u8; 3])> {
        let Some(draft) = self.colors.get(&row.key) else {
            return Vec::new();
        };
        draft
            .iter()
            .zip(&row.zones)
            .enumerate()
            .filter_map(|(i, (d, (_, now)))| d.filter(|d| Some(*d) != *now).map(|d| (i, d)))
            .collect()
    }
}

static DRAFT: Mutex<Option<Draft>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' card, so they never touch real devices.
    pub static PREVIEW: std::cell::RefCell<Option<Snapshot>> = const { std::cell::RefCell::new(None) };
}

/// The card, and what its clicks do.
pub fn show(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some(s) = PREVIEW.with_borrow(|v| v.clone()) {
        draw(ui, &s, &mut Draft::default());
        return;
    }
    pc_lights::page_shown(ui.ctx());
    let snapshot = pc_lights::snapshot();
    let mut guard = DRAFT.lock().unwrap_or_else(|e| e.into_inner());
    let draft = guard.get_or_insert_with(Draft::default);
    for intent in draw(ui, &snapshot, draft) {
        match intent {
            Intent::Apply { key, colors } => {
                draft.colors.remove(&key);
                pc_lights::send(Msg::Set { key, colors });
            }
            Intent::Off(key) => {
                draft.colors.remove(&key);
                pc_lights::send(Msg::Off(key));
            }
            Intent::Identify(key) => pc_lights::send(Msg::Identify(key)),
            Intent::Match(key, on) => {
                if on {
                    if let Some(c) = follow_color() {
                        pc_lights::send(Msg::Follow(c));
                    }
                }
                pc_lights::send(Msg::Match(key, on));
            }
        }
    }
}

/// What "Match keyboard" starts from: the Windows accent when the keyboard follows it, else
/// the keyboard's colour as last read.
fn follow_color() -> Option<[u8; 3]> {
    if super::accent::enabled() {
        if let Some(c) = super::accent::current() {
            return Some(c);
        }
    }
    let left = super::keyboard_settings::cached_rgb_state()?.left;
    Some(pc_lights::hsv_rgb(left.hue, left.sat, left.brightness))
}

/// What the control kind is called on the page.
pub fn control_text(c: Control) -> &'static str {
    match c {
        Control::Driver(_) => "Foxglove driver",
        Control::LampArray => "Standard LampArray device",
        Control::NotYet => "Not controllable yet",
    }
}

/// Paints the card; returns what was asked for this frame.
pub fn draw(ui: &mut egui::Ui, s: &Snapshot, draft: &mut Draft) -> Vec<Intent> {
    let mut out = Vec::new();
    card(ui, "PC & peripherals", |ui| {
        let p = theme::palette(ui);
        ui.weak("Case lights and other lighting devices on this PC.");
        if s.light_studio {
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "OMEN Light Studio is running and may override Foxglove's case colours. \
                     Once Foxglove drives the case, you can remove Light Studio.",
                )
                .color(p.warn),
            )
            .on_hover_text(
                "Settings > Apps > Installed apps > OMEN Light Studio > Uninstall. \
                 OMEN Command Center stays: it runs the fans and performance modes.",
            );
        }
        if s.devices.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("No PC or peripheral lights found.").color(p.muted));
        }
        for row in &s.devices {
            ui.add_space(8.0);
            ui.separator();
            device(ui, row, s.busy, draft, &mut out);
        }
        if let Some(status) = &s.status {
            ui.add_space(6.0);
            ui.label(RichText::new(status).color(p.muted).font(theme::mono(12.0)));
        }
    });
    out
}

fn device(ui: &mut egui::Ui, row: &Row, busy: bool, draft: &mut Draft, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&row.name).color(p.text_strong).strong());
        ui.label(RichText::new(control_text(row.control)).color(p.muted));
        if matches!(row.control, Control::Driver(_)) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if ui
                        .button("Off")
                        .on_hover_text("Turn every zone dark")
                        .clicked()
                    {
                        out.push(Intent::Off(row.key.clone()));
                    }
                    if ui
                        .button("Identify")
                        .on_hover_text("Blink this device for a moment, then put its colours back")
                        .clicked()
                    {
                        out.push(Intent::Identify(row.key.clone()));
                    }
                });
            });
        }
    });
    match row.control {
        Control::Driver(_) => zones(ui, row, busy, draft, out),
        Control::LampArray => {
            ui.weak(
                "A Windows Dynamic Lighting device. Foxglove support is coming soon; until then \
                 Windows Settings > Personalization > Dynamic Lighting controls it.",
            );
        }
        Control::NotYet => {
            ui.weak("Found, not controllable yet.");
        }
    }
}

fn zones(ui: &mut egui::Ui, row: &Row, busy: bool, draft: &mut Draft, out: &mut Vec<Intent>) {
    let picks = draft
        .colors
        .entry(row.key.clone())
        .or_insert_with(|| vec![None; row.zones.len()]);
    picks.resize(row.zones.len(), None);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for (i, (name, now)) in row.zones.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_width(118.0);
                let shown = picks[i].or(*now);
                let mut color = shown.unwrap_or([0x80, 0x80, 0x80]);
                ui.horizontal(|ui| {
                    if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut color)
                        .on_hover_text(format!("{name}: pick a colour, then Apply"))
                        .changed()
                    {
                        picks[i] = Some(color);
                    }
                    if shown.is_none() {
                        ui.weak("not set");
                    } else if picks[i].is_some_and(|c| Some(c) != *now) {
                        ui.label(RichText::new("changed").color(theme::palette(ui).accent));
                    }
                });
                ui.label(RichText::new(name).size(12.5));
            });
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(theme::caps_label(ui, "All zones"));
        for (name, rgb) in super::lights_page::SWATCHES {
            let all = picks.iter().all(|c| *c == Some(rgb));
            if swatch(ui, Color32::from_rgb(rgb[0], rgb[1], rgb[2]), all)
                .on_hover_text(name)
                .clicked()
            {
                picks.iter_mut().for_each(|c| *c = Some(rgb));
            }
        }
    });
    let mut matching = row.match_keyboard;
    let changes = draft.changes(row);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!busy, |ui| {
            if ui
                .checkbox(&mut matching, "Match keyboard")
                .on_hover_text(
                    "Follow the keyboard's underglow colour when Foxglove sets it, or the Windows \
                     accent colour when the keyboard follows that. Picking a colour here stops it.",
                )
                .changed()
            {
                out.push(Intent::Match(row.key.clone(), matching));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let apply = ui.add_enabled(
                !busy && !changes.is_empty(),
                theme::primary_button(ui, "Apply colours"),
            );
            if apply.clicked() {
                out.push(Intent::Apply {
                    key: row.key.clone(),
                    colors: changes,
                });
            }
        });
    });
}

/// A small round colour swatch, ringed when every zone has it.
fn swatch(ui: &mut egui::Ui, color: Color32, selected: bool) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::click());
    let painter = ui.painter();
    let c = rect.center();
    if selected {
        painter.circle_stroke(c, 12.0, Stroke::new(2.0, p.text_strong));
    } else if response.hovered() || response.has_focus() {
        painter.circle_stroke(c, 12.0, Stroke::new(2.0, p.accent));
    }
    painter.circle_filled(c, 9.5, color);
    response
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use foxglove_devices::pc_lighting::Protocol;

    fn case(colors: [Option<[u8; 3]>; 5], matching: bool) -> Row {
        let names = [
            "Diamond display",
            "Interior light",
            "Front bottom fan",
            "Front middle fan",
            "Front top fan",
        ];
        Row {
            key: "103c:84fd".into(),
            name: "HP OMEN 45L case lights".into(),
            control: Control::Driver(Protocol::OmenTracer),
            zones: names
                .iter()
                .zip(colors)
                .map(|(n, c)| (n.to_string(), c))
                .collect(),
            match_keyboard: matching,
        }
    }

    /// The previews' sample: the 45L with colours, the cooler, the G502, Light Studio running.
    pub fn sample() -> Snapshot {
        let orange = Some([0xf0, 0x8a, 0x3c]);
        let pine = Some([0x5f, 0xc4, 0x8e]);
        Snapshot {
            devices: vec![
                case([orange, pine, orange, orange, orange], false),
                Row {
                    key: "03f0:7397".into(),
                    name: "HP OMEN liquid cooler".into(),
                    control: Control::NotYet,
                    zones: vec![],
                    match_keyboard: false,
                },
                Row {
                    key: "046d:c332".into(),
                    name: "G502 Proteus Spectrum".into(),
                    control: Control::LampArray,
                    zones: vec![],
                    match_keyboard: false,
                },
            ],
            light_studio: true,
            status: Some("HP OMEN 45L case lights: colours set".into()),
            busy: false,
        }
    }

    /// Before Foxglove has set anything, no Light Studio, matching the keyboard.
    pub fn sample_fresh() -> Snapshot {
        Snapshot {
            devices: vec![case([None; 5], true)],
            ..Default::default()
        }
    }

    #[test]
    fn apply_sends_only_the_zones_that_changed() {
        let row = case([Some([1, 1, 1]), None, None, None, None], false);
        let mut d = Draft::default();
        assert!(d.changes(&row).is_empty());
        d.colors.insert(
            row.key.clone(),
            vec![
                Some([1, 1, 1]),
                Some([2, 2, 2]),
                None,
                None,
                Some([3, 3, 3]),
            ],
        );
        assert_eq!(d.changes(&row), vec![(1, [2, 2, 2]), (4, [3, 3, 3])]);
    }

    #[test]
    fn controls_have_plain_names() {
        assert_eq!(
            control_text(Control::Driver(Protocol::OmenTracer)),
            "Foxglove driver"
        );
        assert_eq!(
            control_text(Control::LampArray),
            "Standard LampArray device"
        );
        assert_eq!(control_text(Control::NotYet), "Not controllable yet");
    }
}
