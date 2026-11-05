//! The Lights page's "Other devices" card (#423): each lighting device Foxglove found, how
//! it's controlled and what it can do (chips), Identify, a colour per zone, Off and "Match
//! keyboard", and "Add a device…" for gear the catalog doesn't know.
//!
//! The controls follow the device's capabilities: zone pickers only for per-zone or per-LED
//! colour, swatches and Match only for devices that take a colour, Off and Identify only for
//! lights that switch. A Windows Dynamic Lighting device (#691) gets the same card once
//! Foxglove drives it: a picker per lamp for 2 to 12 lamps (more is a per-key board: one
//! "All lamps" row, per-key colour lives in Scenes), and one muted line on what Windows lets
//! Foxglove do with it.
//!
//! "Use OpenRGB's devices" (#471, off by default) adds the controllers a running OpenRGB
//! drives as rows like any other, "Through OpenRGB"; its host and port sit under the toggle.
//!
//! [`show`] reads the worker's snapshot and sends clicks to it; [`draw`] paints a [`View`] and
//! returns [`Intent`]s, so the previews and tests need no hardware. Colours are picked first
//! and written on Apply: dragging a picker sends nothing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Color32, RichText, Sense, Stroke};
use foxglove_devices::pc_lighting::{
    self as pc_lighting, openrgb, CatalogDevice, Control, Protocol,
};

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
    /// New OpenRGB settings (#471): the toggle, or a host and port to connect to.
    OpenRgb(openrgb::Settings),
    /// Connect to OpenRGB again at the same address.
    OpenRgbConnect,
}

/// The colours being picked per device (by key), until Apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub colors: BTreeMap<String, Vec<Option<[u8; 3]>>>,
    /// The device whose "Remove" was clicked in its More menu, waiting for the second click.
    pub removing: Option<String>,
    /// OpenRGB's host and port as being typed, until Connect.
    pub openrgb: Option<(String, String)>,
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
                // What the keyboard last reported, should the worker have none yet.
                if let (true, Some(k)) = (on, keyboard_shows()) {
                    // The accent the keyboard is sent may not be what it shows (#616): say
                    // which one the case follows.
                    if let Some(accent) = super::accent::followed() {
                        eprintln!(
                            "Foxglove: Match keyboard on for {key}: following the keyboard's \
                             report (effect {}, hue {}, sat {}, brightness {}), not the accent \
                             {} it is sent",
                            k.effect,
                            k.hue,
                            k.sat,
                            k.brightness,
                            pc_lights::hex(accent)
                        );
                    }
                    pc_lights::send(Msg::Keyboard(k));
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
            Intent::OpenRgb(settings) => {
                draft.openrgb = None;
                pc_lights::send(Msg::OpenRgbSettings(settings));
            }
            Intent::OpenRgbConnect => pc_lights::send(Msg::OpenRgbConnect),
        }
    }
}

/// What "Match keyboard" starts from: the keyboard's lights as it last reported them
/// (RGB_STATE), never the colour Foxglove meant to send it, such as the accent the keymap
/// can't take (#616).
fn keyboard_shows() -> Option<pc_lights::KeyboardLights> {
    let left = super::keyboard_settings::cached_rgb_state()?.left;
    Some(pc_lights::KeyboardLights::from_half(&left))
}

/// What the control kind is called on the page.
pub fn control_text(c: Control) -> &'static str {
    match c {
        Control::Driver(_) => "Built-in support",
        Control::LampArray => "Windows lighting",
        Control::NotYet => "Not controllable yet",
        Control::OpenRgb => "Through OpenRGB",
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
        for c in &s.conflicts {
            ui.add_space(4.0);
            let line = ui.label(
                RichText::new(format!(
                    "{} is running and may override Foxglove's colours on {}.",
                    c.app, c.device
                ))
                .color(p.warn),
            );
            if let Some(hint) = &c.hint {
                line.on_hover_text(hint);
            }
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
        openrgb_row(ui, &s.openrgb, draft, &mut out);
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

/// "Use OpenRGB's devices" (#471): the toggle; while it's on, the server's host and port, a
/// Connect for typed ones, "try again" and how the connection is.
fn openrgb_row(
    ui: &mut egui::Ui,
    v: &pc_lights::OpenRgbView,
    draft: &mut Draft,
    out: &mut Vec<Intent>,
) {
    let p = theme::palette(ui);
    let s = &v.settings;
    let mut on = s.enabled;
    if ui
        .checkbox(
            &mut on,
            RichText::new("Use OpenRGB's devices")
                .color(p.text_strong)
                .strong(),
        )
        .on_hover_text(
            "List the devices a running OpenRGB drives (memory, graphics cards, motherboards, \
             fans) here, so Foxglove and its scenes can light them. Turn on OpenRGB's SDK \
             server first.",
        )
        .changed()
    {
        draft.openrgb = None;
        out.push(Intent::OpenRgb(openrgb::Settings {
            enabled: on,
            ..s.clone()
        }));
    }
    if !s.enabled {
        ui.weak("Off: Foxglove doesn't connect to OpenRGB.");
        return;
    }
    let (host, port) = draft
        .openrgb
        .get_or_insert_with(|| (s.host.clone(), s.port.to_string()));
    let typed = port
        .trim()
        .parse::<u16>()
        .ok()
        .map(|port| openrgb::Settings {
            host: host.trim().to_string(),
            port,
            ..s.clone()
        });
    let valid = typed.as_ref().filter(|t| t.addr().is_some());
    let mut connect = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(theme::caps_label(ui, "Host"));
        ui.add(egui::TextEdit::singleline(host).desired_width(120.0))
            .on_hover_text("OpenRGB's address: an IP address, or localhost");
        ui.label(theme::caps_label(ui, "Port"));
        ui.add(
            egui::TextEdit::singleline(port)
                .desired_width(56.0)
                .char_limit(5),
        );
        connect = ui
            .add_enabled(
                valid.is_some_and(|t| t != s) || !v.connected && valid.is_some(),
                egui::Button::new("Connect"),
            )
            .on_hover_text("Connect to OpenRGB at this address")
            .clicked();
    });
    if valid.is_none() {
        ui.label(
            RichText::new("Host: an IP address or localhost; port: 1 to 65535.").color(p.warn),
        );
    }
    let mut again = s.reconnect;
    if ui
        .checkbox(&mut again, "Reconnect automatically")
        .on_hover_text("A few tries, a little longer apart each time, then Foxglove stops")
        .changed()
    {
        out.push(Intent::OpenRgb(openrgb::Settings {
            reconnect: again,
            ..s.clone()
        }));
    }
    if let Some(status) = &v.status {
        ui.label(
            RichText::new(status)
                .color(if v.connected { p.muted } else { p.warn })
                .font(theme::mono(12.0)),
        );
    }
    if connect {
        match valid.cloned() {
            // The same address, not connected now: try it again.
            Some(t) if &t == s => out.push(Intent::OpenRgbConnect),
            Some(t) => out.push(Intent::OpenRgb(t)),
            None => {}
        }
    }
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

/// Whether Foxglove drives the device: a driver's, or a Dynamic Lighting device it opened
/// (#691; Windows' access to it is known only then).
pub fn driven(row: &Row) -> bool {
    match row.control {
        Control::Driver(_) | Control::OpenRgb => true,
        Control::LampArray => row.lamp_access.is_some(),
        Control::NotYet => false,
    }
}

/// The most lamps that get a picker each; more is a per-key board (lighting-zones.md, hide
/// rules): one row for all lamps.
pub const MAX_LAMP_PICKERS: usize = 12;

/// Whether a device gets a colour picker per zone (or lamp).
fn pickers(row: &Row) -> bool {
    row.caps.per_zone()
        && (!matches!(row.control, Control::LampArray | Control::OpenRgb)
            || row.zones.len() <= MAX_LAMP_PICKERS)
}

/// Whether the row already shows each zone by name: a colour picker per zone, drawn (the
/// device takes colour from Foxglove now, its effect uses colour, and it has few enough zones).
/// Same conditions as the row's own drawing, from the device's data alone.
fn named_pickers(row: &Row) -> bool {
    let drawn = match row.control {
        Control::Driver(_) | Control::OpenRgb => true,
        Control::LampArray => row.lamp_access.is_some(),
        Control::NotYet => false,
    };
    drawn
        && row.caps.takes_color()
        && effect_of(row.control).is_some_and(|e| e.uses_colour())
        && pickers(row)
}

/// Whether a device row offers **Show zones** (#611, the Hub's cut on #775): only where the
/// drawing adds what the row can't show, so not when each zone already has a named picker on
/// the row. A per-LED strip past [`MAX_LAMP_PICKERS`] (one "All" row) or a device Foxglove
/// can't drive yet keeps it.
pub fn offers_zones(row: &Row) -> bool {
    row.caps.zones.is_some()
        && super::zones_ui::drawable(row.caps.kind(false))
        && !named_pickers(row)
}

/// What Windows lets Foxglove do with a Dynamic Lighting device, in one line (#691): from
/// `IsEnabled` / `IsAvailable` and whether Foxglove has package identity (background control,
/// #179). `None` when there's nothing to say.
pub fn lamp_line(access: pc_lighting::LampAccess, identity: bool) -> Option<&'static str> {
    if !access.enabled {
        return Some("Turned off in Windows' Dynamic Lighting settings.");
    }
    match (access.available, identity) {
        (false, true) => Some(
            "Another app has these lights now. Settings > Personalization > Dynamic Lighting \
             sets which app comes first.",
        ),
        (false, false) => {
            Some("Windows shows Foxglove's colours here while Foxglove's window is in front.")
        }
        (true, false) => Some(
            "Windows shows Foxglove's colours while Foxglove's window is in front; in the \
             background its own Dynamic Lighting takes over.",
        ),
        (true, true) => None,
    }
}

/// A device's real lighting resolution (#611): one summary line ("5 lighting zones",
/// "Per-LED lighting, 24 lamps") with what it means on hover, **Show zones** where the
/// drawing adds to the row ([`offers_zones`]), and its screens' chips. It
/// replaces the light, brightness and onboard-effect chips (section 6).
fn light_line(ui: &mut egui::Ui, row: &Row) {
    let caps = &row.caps;
    let p = theme::palette(ui);
    let mut others = caps.screen_chips();
    let mut open = false;
    match caps.summary(false) {
        Some(summary) => {
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(RichText::new(summary).color(p.text)).wrap())
                    .on_hover_text(super::light_zones::TIP);
                if offers_zones(row) {
                    open = super::zones_ui::toggle(ui, &row.key, caps.kind(false));
                }
            });
        }
        None if others.is_empty() => others.push("No lights Foxglove knows of".into()),
        None => {}
    }
    if open {
        let names: Vec<String> = row.zones.iter().map(|(n, _)| n.clone()).collect();
        super::zones_ui::device(ui, caps, &names);
    }
    if !others.is_empty() {
        chips(ui, &others);
    }
}

fn device(ui: &mut egui::Ui, row: &Row, busy: bool, draft: &mut Draft, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    let driven = driven(row);
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
    light_line(ui, row);
    match row.control {
        Control::Driver(_) | Control::OpenRgb if row.caps.takes_color() => {
            zones(ui, row, busy, draft, out)
        }
        Control::Driver(_) | Control::OpenRgb => {}
        Control::LampArray => match row.lamp_access {
            Some(access) => {
                if row.caps.takes_color() {
                    zones(ui, row, busy, draft, out);
                }
                if let Some(line) = lamp_line(access, super::scene_lamps::identity()) {
                    ui.add_space(2.0);
                    ui.label(RichText::new(line).color(p.muted).size(12.5));
                }
            }
            None if row.zones.is_empty() => {
                ui.weak("Windows lighting: reading its lamps from Windows.");
            }
            None => {
                ui.weak("Windows lists no Dynamic Lighting device for it right now.");
            }
        },
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

/// The effect a PC light runs from Foxglove, from the effect table (#670): its class's
/// steady colour. A device Foxglove can't drive has none.
pub fn effect_of(control: Control) -> Option<&'static super::effect_controls::EffectSpec> {
    use super::effect_controls::{spec, EffectClass};
    match control {
        Control::Driver(_) | Control::OpenRgb => spec(EffectClass::PcLights, "static"),
        Control::LampArray => spec(EffectClass::LampArray, "static"),
        Control::NotYet => None,
    }
}

fn zones(ui: &mut egui::Ui, row: &Row, busy: bool, draft: &mut Draft, out: &mut Vec<Intent>) {
    // Only the controls its effect uses (#670); every one Foxglove drives takes a colour.
    if !effect_of(row.control).is_some_and(|e| e.uses_colour()) {
        return;
    }
    let picks = draft
        .colors
        .entry(row.key.clone())
        .or_insert_with(|| vec![None; row.zones.len()]);
    picks.resize(row.zones.len(), None);
    if pickers(row) {
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
                    if let Some(c) = hex_box(ui, (&row.key, i), name, shown) {
                        picks[i] = Some(c);
                    }
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
            match (row.caps.per_zone(), row.control) {
                (false, _) => "Colour",
                (true, Control::LampArray) => "All lamps",
                (true, Control::OpenRgb) => "All LEDs",
                (true, _) => "All zones",
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
                        "Show the colour the keyboard's lights show, as the keyboard reports it. \
                         While the keyboard plays a rainbow (Spectrum, Swirl) there's no colour to \
                         copy: this device keeps its colours and the line below says so. It \
                         doesn't go dark when the keyboard dims. Picking a colour here stops it.",
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

/// A zone's colour as `#rrggbb` to type (#617): an exact colour, such as a saved one to put
/// back. The text being typed stays while the box has focus; a whole colour is a pick (sent on
/// Apply, like the picker's). The value is the device's colour, before the screen's mapping.
fn hex_box(
    ui: &mut egui::Ui,
    id: (&str, usize),
    name: &str,
    shown: Option<[u8; 3]>,
) -> Option<[u8; 3]> {
    let id = ui.make_persistent_id(("pc-zone-hex", id.0, id.1));
    let current = shown.map(pc_lights::hex).unwrap_or_default();
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| current.clone());
    let r = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(72.0)
                .char_limit(7)
                .font(egui::TextStyle::Monospace)
                .hint_text("#rrggbb"),
        )
        .on_hover_text(format!("{name}: type a colour as #rrggbb, then Apply"));
    let picked = r.changed().then(|| parse_typed(&text)).flatten();
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    picked
}

/// A typed colour: `#rrggbb` or `rrggbb`, any case.
pub fn parse_typed(text: &str) -> Option<[u8; 3]> {
    let t = text.trim();
    pc_lights::parse_hex(&format!("#{}", t.strip_prefix('#').unwrap_or(t)))
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
                // As the catalog gives it (#611): five areas.
                zones: Some(pc_lighting::Zones {
                    list: names
                        .iter()
                        .map(|n| pc_lighting::Zone {
                            area: Some(n.to_string()),
                            ..Default::default()
                        })
                        .collect(),
                    dead: vec![0],
                    source: pc_lighting::ZoneSource::Catalog,
                    reported: None,
                }),
                ..Default::default()
            },
            entry: None,
            zone_ids: ["1", "2", "5", "6", "7"].map(String::from).to_vec(),
            calibration: Some(Default::default()),
            calibrated: false,
            size: None,
            zone_positions: vec![],
            zone_roles: vec![],
            kind: foxglove_devices::pc_lighting::DeviceKind::Case,
            lamp_access: None,
            calibration_hint: None,
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
    /// LampArray attributes, the case's vendor tool running, two unknown devices.
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
                        kind: Default::default(),
                        lamp_access: None,
                        calibration_hint: None,
                    },
                    Row {
                        key: "046d:c332".into(),
                        name: "G502 Proteus Spectrum".into(),
                        control: Control::LampArray,
                        zones: vec![
                            ("Lamp 1 · Logo".into(), orange),
                            ("Lamp 2 · Accent".into(), pine),
                        ],
                        match_keyboard: false,
                        caps: LightCaps::from_lamp_array(LampArrayInfo {
                            lamp_count: 2,
                            positions: true,
                            min_update_us: 33_000,
                            kind: 2,
                        }),
                        entry: None,
                        zone_ids: vec!["0".into(), "1".into()],
                        calibration: None,
                        calibrated: false,
                        size: None,
                        zone_positions: vec![],
                        zone_roles: vec![],
                        kind: Default::default(),
                        lamp_access: Some(Default::default()),
                        calibration_hint: None,
                    },
                ],
                conflicts: vec![pc_lights::Conflicting {
                    app: "OMEN Light Studio".into(),
                    device: "HP OMEN 45L case lights".into(),
                    key: "103c:84fd".into(),
                    hint: Some("Settings > Apps > Installed apps > OMEN Light Studio".into()),
                }],
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
                lamp_applies: Default::default(),
                keyboard_lamp_arrays: Vec::new(),
                openrgb: Default::default(),
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

    /// The second desk (`fixtures`): another vendor's case board driven by a fake driver, and
    /// a lightbar on Dynamic Lighting. (The keyboard has the page's own card, #533.)
    pub fn sample_second_desk() -> View {
        use crate::foxglove::fixtures::second_desk as desk;
        let mut bar = desk::lightbar_row();
        bar.caps = LightCaps::from_lamp_array(desk::lightbar_info());
        for (i, z) in pc_lighting::lamp_array::lamp_zones(&desk::lightbar_desc().lamps)
            .into_iter()
            .enumerate()
        {
            bar.zones
                .push((z.name, (i % 3 == 0).then_some([0x5f, 0xc4, 0x8e])));
            bar.zone_ids.push(z.id.to_string());
        }
        bar.lamp_access = Some(pc_lighting::LampAccess {
            enabled: true,
            available: false,
        });
        View {
            pc: Snapshot {
                devices: vec![desk::case_row(), bar],
                ..Default::default()
            },
        }
    }

    /// An OpenRGB controller as the worker lists it (#471): `leds` LEDs, a zone each.
    pub fn openrgb_row(name: &str, leds: usize, color: Option<[u8; 3]>) -> Row {
        Row {
            key: format!("openrgb:{name}:#0"),
            name: name.into(),
            control: Control::OpenRgb,
            zones: (0..leds)
                .map(|i| (format!("LED {}", i + 1), color))
                .collect(),
            match_keyboard: false,
            caps: LightCaps {
                color: Color::PerLed,
                count: leds as u32,
                effects: vec!["Direct".into(), "Static".into(), "Rainbow".into()],
                ..Default::default()
            },
            entry: None,
            zone_ids: (0..leds).map(|i| i.to_string()).collect(),
            calibration: None,
            calibrated: false,
            size: None,
            zone_positions: vec![],
            zone_roles: vec![],
            kind: Default::default(),
            lamp_access: None,
            calibration_hint: None,
        }
    }

    /// The second desk's case board with "Use OpenRGB's devices" on (#471): a four-LED memory
    /// module and a 40-LED fan hub through OpenRGB.
    pub fn sample_openrgb() -> View {
        let mut v = sample_second_desk();
        v.pc.devices.truncate(1);
        v.pc.devices
            .push(openrgb_row("Memory module", 4, Some([0xf0, 0x8a, 0x3c])));
        v.pc.devices.push(openrgb_row("ARGB fan hub", 40, None));
        v.pc.openrgb = pc_lights::OpenRgbView {
            settings: openrgb::Settings {
                enabled: true,
                ..Default::default()
            },
            connected: true,
            status: Some("Connected to OpenRGB at 127.0.0.1:6742: 2 devices".into()),
        };
        v
    }

    /// #471: off by default, the card says so and lists nothing of OpenRGB's; on, its
    /// controllers are rows like any other ("Through OpenRGB") with pickers up to 12 LEDs and
    /// one "All LEDs" row past that, and the host and port sit under the toggle.
    #[test]
    fn openrgb_is_off_until_turned_on_then_its_devices_are_rows() {
        let text = shown(&sample_second_desk());
        assert!(text.contains("Use OpenRGB's devices"), "{text}");
        assert!(
            text.contains("Foxglove doesn't connect to OpenRGB"),
            "{text}"
        );
        assert!(!text.contains("Through OpenRGB") && !text.contains("HOST"));
        let text = shown(&sample_openrgb());
        for want in [
            "Memory module",
            "ARGB fan hub",
            "Through OpenRGB",
            "LED 4",
            "ALL LEDS",
            "HOST",
            "127.0.0.1",
            "6742",
            "Reconnect automatically",
            "Connected to OpenRGB at 127.0.0.1:6742: 2 devices",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(!text.contains("LED 40"), "40 LEDs: one row, no picker each");
        assert_eq!(
            text.matches("Through OpenRGB").count(),
            2,
            "one label per device"
        );
        assert!(driven(&openrgb_row("x", 1, None)));
        assert!(effect_of(Control::OpenRgb).is_some_and(|e| e.uses_colour()));
    }

    /// A typed host that is not an IP address or localhost says why (no name lookups).
    #[test]
    fn a_typed_openrgb_address_is_checked_before_connect() {
        let v = sample_openrgb();
        let mut draft = Draft {
            openrgb: Some(("openrgb.example".into(), "6742".into())),
            ..Draft::default()
        };
        let text = shown_with(&v, &mut draft);
        assert!(text.contains("an IP address or localhost"), "{text}");
        // Typed another address: the draft holds it until Connect.
        let mut draft = Draft {
            openrgb: Some(("192.168.1.20".into(), "6743".into())),
            ..Draft::default()
        };
        let text = shown_with(&v, &mut draft);
        assert!(!text.contains("an IP address or localhost"), "{text}");
        assert!(text.contains("192.168.1.20") && text.contains("Connect"));
    }

    #[test]
    fn the_second_desk_shows_its_own_devices_and_no_one_elses() {
        let v = sample_second_desk();
        let text = shown(&v);
        for want in [
            "Nimbus Strata case lights",
            "Top light bar",
            "Glass glow",
            "Lumen desk lightbar",
            "Windows lighting",
            // The fake driver: shown as driven, with its zone pickers.
            "Built-in support",
            "Apply colours",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        for not in [
            "OMEN",
            "Light Studio",
            "Corne",
            "G502",
            "left half",
            "right half",
        ] {
            assert!(!text.contains(not), "{not}: {text}");
        }
    }

    /// #558: a conflict is the catalog's app on its device, one line each, nobody's by name.
    #[test]
    fn conflicting_apps_warn_one_line_per_app_and_device() {
        use crate::foxglove::fixtures::second_desk as desk;
        let mut v = sample_second_desk();
        let one = |app: &str, device: &str| pc_lights::Conflicting {
            app: app.into(),
            device: device.into(),
            key: String::new(),
            hint: Some("Quit it from its tray icon.".into()),
        };
        v.pc.conflicts = vec![
            one(desk::CONFLICT_APP, "Nimbus Strata case lights"),
            one("Lumen Studio", "Lumen desk lightbar"),
        ];
        let text = shown(&v);
        assert!(
            text.contains(
                "Nimbus Glow is running and may override Foxglove's colours on Nimbus Strata case lights."
            ),
            "{text}"
        );
        assert!(
            text.contains("Lumen Studio is running and may override Foxglove's colours on Lumen desk lightbar."),
            "{text}"
        );
        assert!(!text.contains("OMEN") && !text.contains("Light Studio"));
        // None running: no line.
        let text = shown(&sample_second_desk());
        assert!(!text.contains("is running"), "{text}");
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

    /// #617: each zone shows its colour as `#rrggbb` to type; a typed colour is a pick.
    #[test]
    fn each_zone_has_a_hex_box() {
        let v = sample();
        let case =
            v.pc.devices
                .iter()
                .find(|r| r.caps.per_zone() && r.zones.iter().any(|z| z.1.is_some()))
                .expect("a per-zone device with colours in the sample");
        let text = shown(&v);
        for c in case.zones.iter().filter_map(|z| z.1) {
            assert!(
                text.contains(&pc_lights::hex(c)),
                "{} in {text}",
                pc_lights::hex(c)
            );
        }
        assert_eq!(parse_typed("#A57500"), Some([0xa5, 0x75, 0x00]));
        assert_eq!(parse_typed(" a57500 "), Some([0xa5, 0x75, 0x00]));
        assert_eq!(parse_typed("#a575"), None);
        assert_eq!(parse_typed("red"), None);
    }

    /// #691: a Dynamic Lighting device Foxglove drives has the card any light has, from the
    /// effect table's LampArray row: its lamps by name, All lamps, Apply, Match, Identify and
    /// Off, and one line on what Windows lets Foxglove do. No "coming soon".
    #[test]
    fn a_lamp_array_gets_the_card_from_the_effect_table() {
        let text = shown(&sample());
        for want in [
            "G502 Proteus Spectrum",
            "Windows lighting",
            "Lamp 1 · Logo",
            "Lamp 2 · Accent",
            "ALL LAMPS",
            "Windows shows Foxglove's colours while Foxglove's window is in front",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(!text.contains("coming soon"), "{text}");
        assert_eq!(
            text.matches("Match keyboard").count(),
            2,
            "the case and the mouse"
        );
        assert_eq!(text.matches("Identify").count(), 2);
        // The effect table decides: the LampArray's one effect takes a colour.
        let mouse = &sample().pc.devices[2];
        assert!(effect_of(mouse.control).is_some_and(|e| e.uses_colour()));
        // Another vendor's 12-lamp lightbar, which Windows keeps from Foxglove right now.
        let text = shown(&sample_second_desk());
        assert!(text.contains("Lamp 12 · Accent"), "{text}");
        assert!(
            text.contains("while Foxglove's window is in front."),
            "{text}"
        );
    }

    /// #611's hide rule: more than 12 lamps is a per-key board, so one All lamps row and no
    /// picker per lamp; a lamp array not opened yet says why, with no controls.
    #[test]
    fn a_big_lamp_array_gets_one_row_and_an_unopened_one_no_controls() {
        let mut v = sample_second_desk();
        let bar = &mut v.pc.devices[1];
        bar.zones = (0..24).map(|i| (format!("Lamp {}", i + 1), None)).collect();
        bar.zone_ids = (0..24).map(|i| i.to_string()).collect();
        bar.lamp_access = Some(Default::default());
        let text = shown(&v);
        assert!(
            text.contains("ALL LAMPS") && !text.contains("Lamp 24"),
            "{text}"
        );
        v.pc.devices[1].lamp_access = None;
        let text = shown(&v);
        assert!(
            text.contains("Windows lists no Dynamic Lighting device"),
            "{text}"
        );
        v.pc.devices[1].zones.clear();
        let text = shown(&v);
        assert!(text.contains("reading its lamps"), "{text}");
        assert_eq!(text.matches("Match keyboard").count(), 1, "the case only");
    }

    /// A 40-LED fan hub through OpenRGB, its LEDs reported (no picker each: one All LEDs row).
    pub fn strip_row() -> Row {
        let mut row = openrgb_row("ARGB fan hub", 40, None);
        row.caps.zones = Some(pc_lighting::Zones::from_report(&[40]));
        row
    }

    /// #611's hide rules on Other devices (with the Hub's cut on #775): a device whose zones
    /// already show as named colour pickers (the case's 5 zones, the mouse's 2 lamps) gets no
    /// Show zones; a per-LED strip with one All row keeps it, with the source line and no
    /// zone-looks line; one colour gets no link and no pickers. No device offers Check zones
    /// (that's the keyboard's probe).
    #[test]
    fn device_zone_drawings_follow_the_hide_rules_per_kind() {
        use crate::foxglove::zones_ui::{CHECK, HIDE, PREVIEW_OPEN, SHOW};
        use foxglove_devices::pc_lighting::zones::ZONE_LOOKS;
        let mut v = sample();
        assert!(v.pc.devices.iter().all(|r| !offers_zones(r)));
        PREVIEW_OPEN.set(v.pc.devices.iter().map(|r| r.key.clone()).collect());
        let text = shown(&v);
        PREVIEW_OPEN.set(vec![]);
        for not in [SHOW, HIDE, ZONE_LOOKS, "From Foxglove's device list"] {
            assert!(!text.contains(not), "named pickers: {not}: {text}");
        }
        assert!(text.contains("Diamond display") && text.contains("Lamp 1 · Logo"));
        // The same mouse before Windows opens it: no pickers drawn, so the drawing shows its lamps.
        v.pc.devices[2].lamp_access = None;
        assert!(offers_zones(&v.pc.devices[2]));
        v.pc.devices[2].lamp_access = Some(Default::default());
        // A per-LED strip: one All LEDs row, so Show zones stays.
        v.pc.devices.push(strip_row());
        let closed = shown(&v);
        assert_eq!(closed.matches(SHOW).count(), 1, "the strip only: {closed}");
        PREVIEW_OPEN.set(v.pc.devices.iter().map(|r| r.key.clone()).collect());
        let open = shown(&v);
        PREVIEW_OPEN.set(vec![]);
        for want in [HIDE, "Reported by the device", "ALL LEDS"] {
            assert!(open.contains(want), "{want}: {open}");
        }
        assert!(
            !open.contains(ZONE_LOOKS) && !open.contains(CHECK),
            "{open}"
        );
        // One colour: no link, no pickers.
        let mut one = sample_fresh();
        one.pc.devices[0].caps.color = Color::Single;
        one.pc.devices[0].caps.zones = None;
        PREVIEW_OPEN.set(vec![one.pc.devices[0].key.clone()]);
        let text = shown(&one);
        PREVIEW_OPEN.set(vec![]);
        assert!(text.contains("1 zone (whole device)"), "{text}");
        for not in [SHOW, HIDE, ZONE_LOOKS, "Front top fan"] {
            assert!(!text.contains(not), "one colour: {not}: {text}");
        }
    }

    #[test]
    fn the_lamp_line_says_what_windows_lets_foxglove_do() {
        use pc_lighting::LampAccess;
        let on = LampAccess::default();
        let taken = LampAccess {
            available: false,
            ..on
        };
        let off = LampAccess {
            enabled: false,
            ..on
        };
        assert!(lamp_line(off, true).unwrap().contains("Turned off"));
        assert!(lamp_line(taken, true).unwrap().contains("Another app"));
        assert!(lamp_line(taken, false).unwrap().contains("in front."));
        assert!(lamp_line(on, false).unwrap().contains("background"));
        assert_eq!(lamp_line(on, true), None);
    }

    #[test]
    fn controls_have_plain_names() {
        assert_eq!(
            control_text(Control::Driver(Protocol::OmenTracer)),
            "Built-in support"
        );
        assert_eq!(control_text(Control::LampArray), "Windows lighting");
        assert_eq!(control_text(Control::NotYet), "Not controllable yet");
        assert_eq!(control_text(Control::OpenRgb), "Through OpenRGB");
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
