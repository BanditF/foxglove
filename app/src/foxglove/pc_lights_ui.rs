//! The Lights page's "Other devices" card (#423): each lighting device Foxglove found, how
//! it's controlled and what it can do (chips), Identify, a colour per zone, Off and "Match
//! keyboard", and "Add a device…" for gear the catalog doesn't know.
//!
//! The controls follow the device's capabilities: zone pickers only for per-zone or per-LED
//! colour, swatches and Match only for devices that take a colour, Off and Identify only for
//! lights that switch.
//!
//! [`show`] reads the worker's snapshot and sends clicks to it; [`draw`] paints a [`View`] and
//! returns [`Intent`]s, so the previews and tests need no hardware. Colours are picked first
//! and written on Apply: dragging a picker sends nothing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Color32, RichText, Sense, Stroke};
use foxglove_devices::pc_lighting::{CatalogDevice, Control, Protocol};

use super::pc_lights::{self, Msg, Row, Snapshot};
use super::pc_lights_add::{self as add, Wizard};
use super::settings_ui::card;
use super::theme;

/// What the card shows: the worker's snapshot. The keyboard isn't listed: it has the page's
/// Keyboard card (#533).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub pc: Snapshot,
}

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
    /// Add a device: blink one zone (only ever from a click).
    TestZone {
        vid: u16,
        pid: u16,
        path: String,
        protocol: Protocol,
        zone: u8,
    },
    Save(Box<CatalogDevice>),
    Remove(u16, u16),
    /// Put text on the clipboard (a catalog entry to share).
    Copy(String),
    /// Open the colour calibrator on a device (by key).
    Calibrate(String),
    /// Open it at a picked colour on a zone, to add a match for it.
    CalibrateColor(String, Option<String>, [u8; 3]),
}

/// The colours being picked per device (by key), until Apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub colors: BTreeMap<String, Vec<Option<[u8; 3]>>>,
    /// The device whose "Remove" was clicked in its More menu, waiting for the second click.
    pub removing: Option<String>,
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

static DRAFT: Mutex<Option<(Draft, Wizard)>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' card and wizard, so they never touch real devices.
    pub static PREVIEW: std::cell::RefCell<Option<(View, Wizard)>> = const { std::cell::RefCell::new(None) };
}

/// Drops a pending "Remove" confirm: the page was left.
pub fn forget_remove() {
    if let Some((draft, _)) = DRAFT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        draft.removing = None;
    }
}

/// The card, and what its clicks do.
pub fn show(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some((v, mut wizard)) = PREVIEW.with_borrow(|v| v.clone()) {
        draw(ui, &v, &mut Draft::default(), &mut wizard);
        return;
    }
    pc_lights::page_shown(ui.ctx());
    let view = View {
        pc: pc_lights::snapshot(),
    };
    let mut guard = DRAFT.lock().unwrap_or_else(|e| e.into_inner());
    let (draft, wizard) = guard.get_or_insert_with(Default::default);
    for intent in draw(ui, &view, draft, wizard) {
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
            Intent::TestZone {
                vid,
                pid,
                path,
                protocol,
                zone,
            } => pc_lights::send(Msg::TestZone {
                vid,
                pid,
                path,
                protocol,
                zone,
            }),
            Intent::Save(device) => pc_lights::send(Msg::SaveLocal(device)),
            Intent::Remove(vid, pid) => pc_lights::send(Msg::RemoveLocal(vid, pid)),
            Intent::Copy(text) => ui.ctx().copy_text(text),
            Intent::Calibrate(key) => {
                if let Some(row) = view.pc.devices.iter().find(|r| r.key == key) {
                    super::calibrate_ui::open(super::calibrate_ui::lights_target(row));
                }
            }
            Intent::CalibrateColor(key, zone, color) => {
                if let Some(row) = view.pc.devices.iter().find(|r| r.key == key) {
                    let target = super::calibrate_ui::lights_target(row);
                    super::calibrate_ui::open_at(target, zone, Some(color));
                }
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
        Control::Driver(_) => "Built-in support",
        Control::LampArray => "Windows lighting",
        Control::NotYet => "Not controllable yet",
    }
}

/// Paints the card; returns what was asked for this frame.
pub fn draw(ui: &mut egui::Ui, v: &View, draft: &mut Draft, wizard: &mut Wizard) -> Vec<Intent> {
    let mut out = Vec::new();
    let s = &v.pc;
    card(ui, "Other devices", |ui| {
        let p = theme::palette(ui);
        ui.weak(
            "Case lights, screens and other lighting devices on this PC, and what each can do.",
        );
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
        ui.add_space(8.0);
        ui.separator();
        add::draw(ui, s, wizard, &mut out);
        if let Some(status) = &s.status {
            ui.add_space(6.0);
            ui.label(RichText::new(status).color(p.muted).font(theme::mono(12.0)));
        }
    });
    out
}

/// A row of small capability chips.
pub fn chips(ui: &mut egui::Ui, chips: &[String]) {
    let p = theme::palette(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
        for c in chips {
            egui::Frame::new()
                .fill(p.well)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(9.0)
                .inner_margin(egui::Margin::symmetric(8, 2))
                .show(ui, |ui| {
                    ui.label(RichText::new(c).size(12.0).color(p.text));
                });
        }
    });
}

fn device(ui: &mut egui::Ui, row: &Row, busy: bool, draft: &mut Draft, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    let driven = matches!(row.control, Control::Driver(_));
    // The buttons at the right (right to left: More ▸ last); the name and its notes wrap in the
    // room left of them.
    super::settings_ui::split_row(
        ui,
        |ui| {
            if let Some(entry) = &row.entry {
                more(ui, row, entry, draft, out);
            }
            if driven && row.caps.switchable() {
                ui.add_enabled_ui(!busy, |ui| {
                    if row.caps.takes_color() && calibrate_button(ui).clicked() {
                        out.push(Intent::Calibrate(row.key.clone()));
                    }
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
            }
        },
        |ui| {
            ui.label(RichText::new(&row.name).color(p.text_strong).strong());
            ui.label(RichText::new(control_text(row.control)).color(p.muted));
            if row.entry.is_some() {
                ui.label(RichText::new("· added by you").color(p.muted));
            }
            if row.calibrated {
                ui.label(RichText::new("· calibrated").color(p.muted));
            }
        },
    );
    chips(ui, &row.caps.chips(false));
    match row.control {
        Control::Driver(_) if row.caps.takes_color() => zones(ui, row, busy, draft, out),
        Control::Driver(_) => {}
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
    // Remove's second click: the device's entry goes only once it's confirmed here.
    if draft.removing.as_deref() == Some(row.key.as_str()) {
        let device = row.entry.as_deref().and_then(add::parse_entry);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Forget your entry for this device?").color(p.warn));
            if let Some(device) = &device {
                if ui.button("Remove").clicked() {
                    draft.removing = None;
                    out.push(Intent::Remove(device.vid, device.pid));
                }
            }
            if ui.button("Keep").clicked() {
                draft.removing = None;
            }
        });
    }
}

/// "More ▸": the device's less used actions, for a device you added (Copy entry, Remove).
/// Remove asks again under the device before it forgets the entry.
fn more(ui: &mut egui::Ui, row: &Row, entry: &str, draft: &mut Draft, out: &mut Vec<Intent>) {
    ui.menu_button("More ▸", |ui| {
        if ui
            .button("Copy entry")
            .on_hover_text(
                "Copy this device's catalog entry (TOML) to share it, or to contribute it \
                 to Foxglove's lighting-catalog.toml",
            )
            .clicked()
        {
            out.push(Intent::Copy(entry.to_string()));
        }
        if add::parse_entry(entry).is_some()
            && ui
                .button("Remove…")
                .on_hover_text("Forget your entry for this device")
                .clicked()
        {
            draft.removing = Some(row.key.clone());
        }
    })
    .response
    .on_hover_text("Copy entry, Remove");
}

fn zones(ui: &mut egui::Ui, row: &Row, busy: bool, draft: &mut Draft, out: &mut Vec<Intent>) {
    let picks = draft
        .colors
        .entry(row.key.clone())
        .or_insert_with(|| vec![None; row.zones.len()]);
    picks.resize(row.zones.len(), None);
    if row.caps.per_zone() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (i, (name, now)) in row.zones.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(118.0);
                    let shown = picks[i].or(*now);
                    // Drawn through the device's screen side; a pick stands for what it's sent.
                    let zone = row.zone_ids.get(i).map(String::as_str);
                    let drawn = shown.unwrap_or([0x80; 3]);
                    let mut color = super::calibrate::on_screen(&row.key, zone, drawn);
                    ui.horizontal(|ui| {
                        if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut color)
                            .on_hover_text(format!("{name}: pick a colour, then Apply"))
                            .changed()
                        {
                            picks[i] = Some(super::calibrate::from_screen(&row.key, zone, color));
                        }
                        if shown.is_none() {
                            ui.weak("not set");
                        } else if picks[i].is_some_and(|c| Some(c) != *now) {
                            ui.label(RichText::new("changed").color(theme::palette(ui).accent));
                        }
                    });
                    ui.label(RichText::new(name).size(12.5));
                    if let Some(c) = shown {
                        if super::calibrate_ui::not_matched(ui, &row.key, zone, c) {
                            let zone = zone.map(String::from);
                            out.push(Intent::CalibrateColor(row.key.clone(), zone, c));
                        }
                    }
                });
            }
        });
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(theme::caps_label(
            ui,
            if row.caps.per_zone() {
                "All zones"
            } else {
                "Colour"
            },
        ));
        for (name, rgb) in super::lights_page::SWATCHES {
            let all = picks.iter().all(|c| *c == Some(rgb));
            if swatch(ui, super::calibrate::screen_color(&row.key, None, rgb), all)
                .on_hover_text(name)
                .clicked()
            {
                picks.iter_mut().for_each(|c| *c = Some(rgb));
            }
        }
    });
    let mut matching = row.match_keyboard;
    let changes = draft.changes(row);
    let mut apply = false;
    super::settings_ui::split_row(
        ui,
        |ui| {
            apply = ui
                .add_enabled(
                    !busy && !changes.is_empty(),
                    theme::primary_button(ui, "Apply colours"),
                )
                .clicked();
        },
        |ui| {
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
        },
    );
    if apply {
        out.push(Intent::Apply {
            key: row.key.clone(),
            colors: changes,
        });
    }
}

/// "Calibrate…": opens the colour calibrator on a device.
fn calibrate_button(ui: &mut egui::Ui) -> egui::Response {
    ui.button("Calibrate…").on_hover_text(
        "Match how this device shows colours to your screen or another device: white, red, \
         green, blue and grey, about a minute",
    )
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
    use foxglove_devices::pc_lighting::{Color, LampArrayInfo, LightCaps, Screen, Unknown};

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
            caps: LightCaps {
                color: Color::PerZone,
                count: 5,
                effects: ["breathing", "colour cycle", "blinking", "wave", "radial"]
                    .map(String::from)
                    .to_vec(),
                brightness: true,
                ..Default::default()
            },
            entry: None,
            zone_ids: ["1", "2", "5", "6", "7"].map(String::from).to_vec(),
            calibration: Some(Default::default()),
            calibrated: false,
            size: None,
            zone_positions: vec![],
            zone_roles: vec![],
        }
    }

    pub fn unknown_strip() -> Unknown {
        Unknown {
            vid: 0x1209,
            pid: 0xa1e5,
            name: "Desk strip".into(),
            usage_pages: vec![0xff00, 0x0001],
            path: "strip".into(),
        }
    }

    /// The previews' sample: the keyboard, the 45L with colours, the cooler, the G502 with its
    /// LampArray attributes, Light Studio running, two unknown devices.
    pub fn sample() -> View {
        let orange = Some([0xf0, 0x8a, 0x3c]);
        let pine = Some([0x5f, 0xc4, 0x8e]);
        View {
            pc: Snapshot {
                devices: vec![
                    case([orange, pine, orange, orange, orange], false),
                    Row {
                        key: "03f0:7397".into(),
                        name: "HP OMEN liquid cooler".into(),
                        control: Control::NotYet,
                        zones: vec![],
                        match_keyboard: false,
                        caps: LightCaps {
                            screens: vec![Screen {
                                images: true,
                                animations: true,
                                ..Default::default()
                            }],
                            ..Default::default()
                        },
                        entry: None,
                        zone_ids: vec![],
                        calibration: None,
                        calibrated: false,
                        size: None,
                        zone_positions: vec![],
                        zone_roles: vec![],
                    },
                    Row {
                        key: "046d:c332".into(),
                        name: "G502 Proteus Spectrum".into(),
                        control: Control::LampArray,
                        zones: vec![],
                        match_keyboard: false,
                        caps: LightCaps::from_lamp_array(LampArrayInfo {
                            lamp_count: 2,
                            positions: true,
                            min_update_us: 33_000,
                            kind: 2,
                        }),
                        entry: None,
                        zone_ids: vec![],
                        calibration: None,
                        calibrated: false,
                        size: None,
                        zone_positions: vec![],
                        zone_roles: vec![],
                    },
                ],
                light_studio: true,
                status: Some("HP OMEN 45L case lights: colours set".into()),
                busy: false,
                unknown: vec![
                    unknown_strip(),
                    Unknown {
                        vid: 0x045e,
                        pid: 0x0b12,
                        name: "Xbox Wireless Controller".into(),
                        usage_pages: vec![0x0001],
                        path: "pad".into(),
                    },
                ],
                local_file: Some(
                    r"C:\Users\you\AppData\Roaming\Foxglove\lighting-catalog.local.toml".into(),
                ),
            },
        }
    }

    /// Before Foxglove has set anything, no Light Studio, matching the keyboard.
    pub fn sample_fresh() -> View {
        View {
            pc: Snapshot {
                devices: vec![case([None; 5], true)],
                ..Default::default()
            },
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
            "Built-in support"
        );
        assert_eq!(control_text(Control::LampArray), "Windows lighting");
        assert_eq!(control_text(Control::NotYet), "Not controllable yet");
    }

    /// A headless context with the app's fonts and theme.
    pub fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::platform::add_phosphor_to_fonts(&mut fonts);
        theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        theme::apply_choice(&ctx, theme::Choice::Dark);
        ctx
    }

    /// Draws the card once headless and returns the labels and buttons it showed.
    fn shown(v: &View) -> String {
        shown_with(v, &mut Draft::default())
    }

    fn shown_with(v: &View, draft: &mut Draft) -> String {
        let ctx = test_ctx();
        let mut text = String::new();
        // The first frame lays out; the second's shapes carry every label's text.
        let mut first = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw(ui, v, draft, &mut Wizard::default());
        });
        first.textures_delta.clear();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw(ui, v, draft, &mut Wizard::default());
        });
        out.textures_delta.clear();
        for clipped in &out.shapes {
            collect_text(&clipped.shape, &mut text);
        }
        text
    }

    fn collect_text(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    #[test]
    fn controls_follow_capabilities() {
        let mut v = sample_fresh();
        let text = shown(&v);
        assert!(text.contains("Apply colours") && text.contains("Front top fan"));
        assert!(text.contains("Identify") && text.contains("Match keyboard"));
        assert!(
            text.contains("Calibrate…"),
            "a driven colour device can be calibrated"
        );
        // The same board, said to take only one colour: no zone pickers, still a colour.
        v.pc.devices[0].caps.color = Color::Single;
        let text = shown(&v);
        assert!(!text.contains("Front top fan"), "{text}");
        assert!(text.contains("Match keyboard") && text.contains("COLOUR"));
        // No lights at all: no colour controls, no Off or Identify.
        v.pc.devices[0].caps = LightCaps::default();
        let text = shown(&v);
        assert!(!text.contains("Match keyboard") && !text.contains("Identify"));
        assert!(!text.contains("Calibrate…"));
        assert!(text.contains("No lights Foxglove knows of"));
    }

    /// The keyboard has its own card (#533): no "Foxglove firmware" row here. A device you added
    /// keeps Copy entry and Remove behind More ▸, and Remove asks again before it forgets.
    #[test]
    fn the_keyboard_is_not_listed_and_more_holds_the_entry_actions() {
        let mut v = sample();
        let text = shown(&v);
        assert!(!text.contains("Foxglove firmware"), "{text}");
        // Any text: the menu shows for an entry; Remove… only for one that parses.
        v.pc.devices[0].entry = Some("[[device]]".into());
        let text = shown(&v);
        assert!(text.contains("More ▸"), "{text}");
        assert!(!text.contains("Copy entry") && !text.contains("Forget your entry"));
        let mut draft = Draft {
            removing: Some(v.pc.devices[0].key.clone()),
            ..Draft::default()
        };
        let text = shown_with(&v, &mut draft);
        assert!(
            text.contains("Forget your entry for this device?"),
            "{text}"
        );
        assert!(text.contains("Keep"));
    }
}
