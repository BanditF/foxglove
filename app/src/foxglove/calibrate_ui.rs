//! The colour calibrator: "Calibrate…" on a device of the Lights page.
//!
//! Five steps: white, red, green, blue, then mid-grey. Each shows one colour on the device
//! and on the reference (the screen, as a swatch, or another device, the "master"). The user
//! nudges a few sliders until the device matches, and the device follows as they move: a
//! slider change marks the frame, and the frame sends one write (the PC lights' worker and the
//! keyboard's writer drop any that a newer one replaces). Nothing moves, nothing is written.
//!
//! - **White:** white balance (a gain per channel) and brightness, to match other devices.
//! - **Red, green, blue:** that primary's level and how much of the other two it takes (a
//!   column of the colour matrix): an LED's green is often bluer than a screen's.
//! - **Grey:** gamma per channel, so mid-tones look as bright as on the reference.
//!
//! Save keeps it (per device, or per zone when a zone is picked); Cancel puts the previous
//! calibration and colours back. [`show`] runs it on the page; [`draw`] paints a [`Wizard`]
//! and returns what was asked, so the previews and tests need no device.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Color32, RichText, Sense, Stroke};
use foxglove_devices::calibration::{self, Calibration, DeviceCalibration, IDENTITY_MATRIX};

use super::calibrate::{self, Before, Kind, Target};
use super::settings_ui::card;
use super::theme;

/// The calibrator's steps, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    White,
    Red,
    Green,
    Blue,
    Grey,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::White, Step::Red, Step::Green, Step::Blue, Step::Grey];

    /// The colour the step shows (sRGB).
    pub fn color(self) -> [u8; 3] {
        match self {
            Step::White => [255, 255, 255],
            Step::Red => [255, 0, 0],
            Step::Green => [0, 255, 0],
            Step::Blue => [0, 0, 255],
            Step::Grey => [128, 128, 128],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Step::White => "White",
            Step::Red => "Red",
            Step::Green => "Green",
            Step::Blue => "Blue",
            Step::Grey => "Grey",
        }
    }

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn hint(self) -> &'static str {
        match self {
            Step::White => {
                "Lower the colour that's too strong until the white has no tint (bluish white: \
                 lower Blue). Then set Brightness so it looks as bright as the reference."
            }
            Step::Red => {
                "Too deep or pinkish? Add a little green. Then set Red so it's as bright as \
                 the reference."
            }
            Step::Green => {
                "Too minty or bluish? Add a little red. Then set Green so it's as bright as the \
                 reference."
            }
            Step::Blue => {
                "Too violet? Add a little green; too cold, a little red. Then set Blue so it's \
                 as bright as the reference."
            }
            Step::Grey => {
                "Match how bright the grey looks: higher makes the middle tones darker. For a \
                 tint that only shows in grey, set red, green and blue on their own."
            }
        }
    }

    /// The primary this step works on: a column of the colour matrix.
    fn primary(self) -> Option<usize> {
        match self {
            Step::Red => Some(0),
            Step::Green => Some(1),
            Step::Blue => Some(2),
            _ => None,
        }
    }
}

const CHANNELS: [&str; 3] = ["Red", "Green", "Blue"];

/// The calibrator's state, kept by the page between frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Wizard {
    pub target: Target,
    /// What it's matched to: `None` is the screen.
    pub reference: Option<Target>,
    /// The zone being calibrated on its own (by id); `None` is the whole device.
    pub zone: Option<String>,
    pub step: Step,
    /// The calibration being made.
    pub cal: DeviceCalibration,
    /// What was saved before, put back on Cancel.
    pub saved: Option<DeviceCalibration>,
    /// What the device and the reference showed before, put back at the end.
    pub before: Before,
    pub reference_before: Before,
    /// The grey step's three gammas move together.
    pub linked: bool,
    /// The import box's text, while it's open.
    pub import: Option<String>,
    pub note: Option<String>,
    /// The device hasn't been shown this step's colour yet.
    pub fresh: bool,
}

impl Wizard {
    /// Starts on a device, from its saved calibration (else its model's default).
    pub fn new(target: Target, saved: Option<DeviceCalibration>) -> Self {
        let cal = saved.clone().unwrap_or_else(|| start(&target));
        Self {
            target,
            reference: None,
            zone: None,
            step: Step::White,
            cal,
            saved,
            before: Before::Unknown,
            reference_before: Before::Unknown,
            linked: true,
            import: None,
            note: None,
            fresh: true,
        }
    }

    /// The calibration the sliders move: the picked zone's own (starting from the device's),
    /// or the device's.
    pub fn editing(&mut self) -> &mut Calibration {
        match &self.zone {
            Some(z) => {
                let device = self.cal.device.clone();
                self.cal.zones.entry(z.clone()).or_insert(device)
            }
            None => &mut self.cal.device,
        }
    }

    /// Puts this step's sliders back to neutral.
    pub fn reset_step(&mut self) {
        let step = self.step;
        let c = self.editing();
        match step.primary() {
            Some(col) => {
                let mut m = c.matrix();
                for (row, id) in m.iter_mut().zip(IDENTITY_MATRIX) {
                    row[col] = id[col];
                }
                c.matrix = Some(m);
            }
            None if step == Step::White => {
                c.gain = [1.0; 3];
                c.brightness = 1.0;
            }
            None => c.gamma = [1.0; 3],
        }
    }

    /// Everything back to the model's default, from the first step.
    pub fn start_over(&mut self) {
        self.cal = start(&self.target);
        self.zone = None;
        self.step = Step::White;
    }

    /// What Save keeps: `None` when it's just the model's default (nothing to store), with
    /// identity matrices dropped and zones that match the device folded in.
    pub fn result(&self) -> Option<DeviceCalibration> {
        let tidy = |c: &Calibration| {
            let mut c = c.clone().sanitized();
            if c.matrix.is_some_and(|m| m == IDENTITY_MATRIX) {
                c.matrix = None;
            }
            c
        };
        let device = tidy(&self.cal.device);
        let zones: BTreeMap<_, _> = self
            .cal
            .zones
            .iter()
            .map(|(z, c)| (z.clone(), tidy(c)))
            .filter(|(_, c)| *c != device)
            .collect();
        let default = tidy(&self.target.default.clone().unwrap_or_default());
        if zones.is_empty() && device == default && self.saved.is_none() {
            return None;
        }
        Some(DeviceCalibration {
            name: self.target.name.clone(),
            device,
            zones,
        })
    }
}

fn start(target: &Target) -> DeviceCalibration {
    DeviceCalibration {
        name: target.name.clone(),
        device: target.default.clone().unwrap_or_default(),
        zones: BTreeMap::new(),
    }
}

/// What the page around the calibrator offers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// Other devices to compare with: the keyboard and the PC lights Foxglove drives.
    pub others: Vec<Target>,
    /// The keyboard runs a moving effect, so its colour won't hold still.
    pub moving: bool,
}

/// How the calibrator ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    Cancel,
    Save,
}

/// What a frame asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    /// The calibration (or the step, or the zone) changed: show the step on the device.
    pub show: bool,
    /// The step or the reference changed: show the step on the reference too.
    pub show_reference: bool,
    /// A reference the user switched away from, to put back.
    pub old_reference: Option<Target>,
    pub done: Option<Done>,
    /// Text for the clipboard (an export).
    pub copy: Option<String>,
}

/// Paints the calibrator; returns what was asked this frame.
pub fn draw(ui: &mut egui::Ui, v: &View, w: &mut Wizard) -> Output {
    let mut out = Output {
        show: w.fresh,
        show_reference: w.fresh,
        ..Default::default()
    };
    w.fresh = false;
    let title = format!("Calibrate · {}", w.target.name);
    card(ui, &title, |ui| {
        let p = theme::palette(ui);
        ui.weak(
            "Make this device show Foxglove's colours the way your screen (or another device) \
             does. Five steps, about a minute: nudge the sliders until the two match. The device \
             follows as you move them.",
        );
        ui.add_space(6.0);
        choices(ui, v, w, &mut out);
        if v.moving {
            ui.label(
                RichText::new(
                    "The keyboard is running a moving effect: set it to Solid to compare colours.",
                )
                .color(p.warn),
            );
        }
        ui.add_space(6.0);
        steps(ui, w, &mut out);
        ui.add_space(8.0);
        let wide = ui.available_width() >= 560.0;
        if wide {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(220.0, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| swatches(ui, w),
                );
                ui.add_space(14.0);
                ui.vertical(|ui| sliders(ui, w, &mut out));
            });
        } else {
            swatches(ui, w);
            ui.add_space(8.0);
            sliders(ui, w, &mut out);
        }
        ui.add_space(10.0);
        buttons(ui, w, &mut out);
        ui.add_space(6.0);
        ui.separator();
        share(ui, w, &mut out);
        if let Some(note) = &w.note {
            ui.add_space(4.0);
            ui.label(RichText::new(note).color(p.muted).font(theme::mono(12.0)));
        }
    });
    out
}

/// "Compare with" (the screen or another device) and, for a board with zones, which zone.
fn choices(ui: &mut egui::Ui, v: &View, w: &mut Wizard, out: &mut Output) {
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::caps_label(ui, "Compare with"));
        let mut pick = |ui: &mut egui::Ui, w: &mut Wizard, r: Option<&Target>, name: &str| {
            let selected = w.reference.as_ref().map(|t| &t.key) == r.map(|t| &t.key);
            if theme::pill(ui, selected, name).clicked() && !selected {
                out.old_reference = std::mem::replace(&mut w.reference, r.cloned());
                out.show_reference = true;
            }
        };
        pick(ui, w, None, "Screen");
        for t in &v.others {
            pick(ui, w, Some(t), &t.name);
        }
    });
    let Kind::Lights { zones } = w.target.kind.clone() else {
        return;
    };
    if zones.len() < 2 {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::caps_label(ui, "Zones"));
        let mut choose = |ui: &mut egui::Ui, w: &mut Wizard, z: Option<&String>, name: &str| {
            let selected = w.zone.as_ref() == z;
            if theme::pill(ui, selected, name)
                .on_hover_text("Calibrate this zone on its own, when it doesn't match the rest")
                .clicked()
                && !selected
            {
                w.zone = z.cloned();
                out.show = true;
            }
        };
        choose(ui, w, None, "All zones");
        for (id, name) in &zones {
            choose(ui, w, Some(id), name);
        }
    });
}

/// The five steps as numbered pills; any can be picked.
fn steps(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (i, step) in Step::ALL.iter().enumerate() {
            let text = format!("{} {}", i + 1, step.name());
            if theme::pill(ui, w.step == *step, &text).clicked() && w.step != *step {
                go(w, *step, out);
            }
        }
    });
}

fn go(w: &mut Wizard, step: Step, out: &mut Output) {
    w.step = step;
    out.show = true;
    out.show_reference = true;
}

/// The reference colour (the screen's swatch, or a note to look at the other device) beside
/// what goes to this device after calibration.
fn swatches(ui: &mut egui::Ui, w: &mut Wizard) {
    let p = theme::palette(ui);
    let c = w.step.color();
    let zone = w.zone.clone();
    let sent = w
        .cal
        .zone(zone.as_deref())
        .apply_rgb(foxglove_devices::Rgb {
            r: c[0],
            g: c[1],
            b: c[2],
        });
    let reference = match &w.reference {
        None => "On your screen".to_string(),
        Some(t) => format!("On the {}", t.name),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (label, color, big) in [
            (reference.as_str(), c, true),
            ("Sent to this device", [sent.r, sent.g, sent.b], false),
        ] {
            ui.vertical(|ui| {
                let size = if big { 120.0 } else { 72.0 };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(size, 120.0), Sense::hover());
                ui.painter().rect_filled(rect.expand(2.0), 10.0, p.well);
                ui.painter().rect(
                    rect,
                    8.0,
                    Color32::from_rgb(color[0], color[1], color[2]),
                    Stroke::new(1.0, p.line),
                    egui::StrokeKind::Inside,
                );
                ui.label(RichText::new(label).size(12.0).color(p.muted));
            });
        }
    });
    if w.reference.is_some() {
        ui.label(
            RichText::new("Both devices show this step now: match this one to the other.")
                .size(12.0)
                .color(p.muted),
        );
    }
}

/// This step's sliders.
fn sliders(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    let p = theme::palette(ui);
    let step = w.step;
    ui.label(
        RichText::new(format!("{}. {}", step.index() + 1, step.name()))
            .color(p.text_strong)
            .strong(),
    );
    ui.label(RichText::new(step.hint()).size(12.5).color(p.text));
    ui.add_space(6.0);
    let linked = w.linked;
    let c = w.editing();
    let mut changed = false;
    match step.primary() {
        None if step == Step::White => {
            for (i, name) in CHANNELS.iter().enumerate() {
                changed |= slider(ui, name, &mut c.gain[i], 0.1..=1.0);
            }
            changed |= slider(ui, "Brightness", &mut c.brightness, 0.1..=1.0);
        }
        Some(col) => {
            let mut m = c.matrix();
            let mut any = slider(ui, CHANNELS[col], &mut m[col][col], 0.2..=1.0);
            for (row, name) in CHANNELS.iter().enumerate().filter(|(r, _)| *r != col) {
                any |= slider(
                    ui,
                    &format!("Add {}", name.to_lowercase()),
                    &mut m[row][col],
                    0.0..=0.5,
                );
            }
            if any {
                c.matrix = Some(m);
                changed = true;
            }
        }
        None if linked => {
            let mut all = c.gamma.iter().sum::<f32>() / 3.0;
            if slider(ui, "Mid-tones", &mut all, 0.4..=2.5) {
                c.gamma = [all; 3];
                changed = true;
            }
            ui.checkbox(&mut w.linked, "Same for red, green and blue");
        }
        None => {
            for (i, name) in CHANNELS.iter().enumerate() {
                changed |= slider(ui, &format!("{name} mid-tones"), &mut c.gamma[i], 0.4..=2.5);
            }
            ui.checkbox(&mut w.linked, "Same for red, green and blue");
        }
    }
    out.show |= changed;
}

/// One slider across the column, its value in the row above.
fn slider(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{value:.2}")).font(theme::mono(12.5)));
        });
    });
    ui.spacing_mut().slider_width = ui.available_width();
    ui.add(
        egui::Slider::new(value, range)
            .show_value(false)
            .step_by(0.01),
    )
    .changed()
}

/// Back and Next (Save on the last step), Reset step, Start over and Cancel.
fn buttons(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    ui.horizontal(|ui| {
        if ui
            .button("Cancel")
            .on_hover_text("Put back the calibration and colours it had")
            .clicked()
        {
            out.done = Some(Done::Cancel);
        }
        if ui
            .button("Start over")
            .on_hover_text("Every step back to its default, from White")
            .clicked()
        {
            w.start_over();
            go(w, Step::White, out);
        }
        if ui.button("Reset step").clicked() {
            w.reset_step();
            out.show = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let last = w.step == Step::Grey;
            let next = if last { "Save" } else { "Next" };
            if ui.add(theme::primary_button(ui, next)).clicked() {
                match Step::ALL.get(w.step.index() + 1) {
                    Some(s) => go(w, *s, out),
                    None => out.done = Some(Done::Save),
                }
            }
            if w.step != Step::White && ui.button("Back").clicked() {
                go(w, Step::ALL[w.step.index() - 1], out);
            }
            if !last
                && ui
                    .button("Save now")
                    .on_hover_text("Keep what's set so far")
                    .clicked()
            {
                out.done = Some(Done::Save);
            }
        });
    });
}

/// Copy the calibration (to keep or share), or paste one in.
fn share(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    ui.horizontal(|ui| {
        if ui
            .small_button("Copy calibration")
            .on_hover_text("This device's calibration as text, to keep or to use on another one")
            .clicked()
        {
            match calibration::export(&w.target.key, &w.cal) {
                Ok(text) => {
                    out.copy = Some(text);
                    w.note = Some("Calibration copied".into());
                }
                Err(e) => w.note = Some(format!("Can't copy it: {e}")),
            }
        }
        let open = w.import.is_some();
        if ui
            .small_button(if open { "Close import" } else { "Import…" })
            .clicked()
        {
            w.import = if open { None } else { Some(String::new()) };
        }
    });
    let Some(text) = w.import.as_mut() else {
        return;
    };
    ui.add(
        egui::TextEdit::multiline(text)
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .font(theme::mono(12.0))
            .hint_text("Paste a copied calibration here"),
    );
    if ui.button("Use it").clicked() {
        match calibration::import(text) {
            Ok((_, cal)) => {
                w.cal = DeviceCalibration {
                    name: w.target.name.clone(),
                    ..cal
                };
                w.import = None;
                w.note = Some("Imported: check each step, then Save".into());
                out.show = true;
            }
            Err(e) => w.note = Some(format!("Can't use it: {e}")),
        }
    }
}

static OPEN: Mutex<Option<Wizard>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' calibrator, so they never touch real devices.
    pub static PREVIEW: std::cell::RefCell<Option<(View, Wizard)>> = const { std::cell::RefCell::new(None) };
}

/// Opens the calibrator on a device, from what it shows now.
pub fn open(target: Target) {
    let saved = calibration::global().device(&target.key);
    let mut w = Wizard::new(target, saved);
    w.before = calibrate::capture(&w.target);
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
}

/// The calibrator in place of the "PC & peripherals" card, while it's open; false when it
/// isn't.
pub fn show(ui: &mut egui::Ui) -> bool {
    #[cfg(test)]
    if let Some((v, mut w)) = PREVIEW.with_borrow(|v| v.clone()) {
        draw(ui, &v, &mut w);
        return true;
    }
    let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(w) = guard.as_mut() else {
        return false;
    };
    let view = View {
        others: others(&w.target),
        moving: moving(w),
    };
    let out = draw(ui, &view, w);
    if let Some(text) = out.copy.clone() {
        ui.ctx().copy_text(text);
    }
    if act(w, out) {
        *guard = None;
    }
    true
}

/// Sends what a frame asked for; true when the calibrator is done.
fn act(w: &mut Wizard, out: Output) -> bool {
    let color = w.step.color();
    if let Some(old) = &out.old_reference {
        calibrate::restore(old, &w.reference_before);
        w.reference_before = w
            .reference
            .as_ref()
            .map(calibrate::capture)
            .unwrap_or_default();
    }
    if out.show {
        // The registry holds the calibration being made, so the device shows it.
        calibration::global().set_device(&w.target.key, Some(w.cal.clone()));
        calibrate::show(&w.target, color);
    }
    if out.show_reference {
        if let Some(r) = &w.reference {
            calibrate::show(r, color);
        }
    }
    let Some(done) = out.done else {
        return false;
    };
    match done {
        Done::Cancel => calibration::global().set_device(&w.target.key, w.saved.clone()),
        Done::Save => {
            if let Err(e) = calibrate::commit(&w.target.key, w.result()) {
                w.note = Some(format!("Can't save the calibration: {e}"));
                return false;
            }
        }
    }
    calibrate::restore(&w.target, &w.before);
    if let Some(r) = &w.reference {
        calibrate::restore(r, &w.reference_before);
    }
    true
}

/// The keyboard as a calibrator target, while it's connected.
pub fn keyboard_target() -> Option<Target> {
    let key = super::devices::keyboard_key()?;
    let s = super::screens::snapshot();
    let name = if s.product.is_empty() {
        "Keyboard".to_string()
    } else {
        s.product.clone()
    };
    Some(Target {
        key,
        name,
        kind: Kind::Keyboard,
        default: None,
    })
}

/// A PC light as a calibrator target.
pub fn lights_target(row: &super::pc_lights::Row) -> Target {
    Target {
        key: row.key.clone(),
        name: row.name.clone(),
        kind: Kind::Lights {
            zones: row
                .zone_ids
                .iter()
                .cloned()
                .zip(row.zones.iter().map(|(n, _)| n.clone()))
                .collect(),
        },
        default: row.calibration.clone(),
    }
}

/// Everything else the calibrator can compare with.
fn others(target: &Target) -> Vec<Target> {
    let lights = super::pc_lights::snapshot()
        .devices
        .iter()
        .filter(|r| matches!(r.control, foxglove_devices::pc_lighting::Control::Driver(_)))
        .filter(|r| r.caps.takes_color())
        .map(lights_target)
        .collect::<Vec<_>>();
    keyboard_target()
        .into_iter()
        .chain(lights)
        .filter(|t| t.key != target.key)
        .collect()
}

/// Whether the keyboard is in the comparison and running a moving effect.
fn moving(w: &Wizard) -> bool {
    let keyboard = w.target.kind == Kind::Keyboard
        || w.reference
            .as_ref()
            .is_some_and(|r| r.kind == Kind::Keyboard);
    keyboard
        && super::keyboard_settings::cached_rgb_state()
            .is_some_and(|s| s.left.on && s.left.effect != 0)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn case() -> Target {
        Target {
            key: "103c:84fd".into(),
            name: "HP OMEN 45L case lights".into(),
            kind: Kind::Lights {
                zones: [
                    ("1", "Diamond display"),
                    ("2", "Interior light"),
                    ("5", "Front bottom fan"),
                    ("6", "Front middle fan"),
                    ("7", "Front top fan"),
                ]
                .map(|(i, n)| (i.to_string(), n.to_string()))
                .to_vec(),
            },
            default: Some(Calibration::default()),
        }
    }

    pub fn corne() -> Target {
        Target {
            key: "1d50:615e".into(),
            name: "Eyelash Corne".into(),
            kind: Kind::Keyboard,
            default: None,
        }
    }

    /// The previews' calibrator on the 45L, at a step, part-way through.
    pub fn sample(step: Step, master: bool) -> (View, Wizard) {
        let mut w = Wizard::new(case(), None);
        w.step = step;
        w.fresh = false;
        w.cal.device.gain = [1.0, 0.86, 0.72];
        w.cal.device.brightness = 0.9;
        w.cal.device.matrix = Some([[0.94, 0.06, 0.0], [0.08, 0.82, 0.0], [0.0, 0.0, 1.0]]);
        w.cal.device.gamma = [1.12, 1.12, 1.12];
        if master {
            w.reference = Some(corne());
        }
        (
            View {
                others: vec![corne()],
                moving: false,
            },
            w,
        )
    }

    fn ui_run(v: &View, w: &mut Wizard, input: egui::RawInput) -> Output {
        let ctx = super::super::pc_lights_ui::tests::test_ctx();
        let mut out = Output::default();
        let mut first = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw(ui, v, &mut w.clone());
        });
        first.textures_delta.clear();
        let mut frame = ctx.run_ui(input, |ui| out = draw(ui, v, w));
        frame.textures_delta.clear();
        out
    }

    #[test]
    fn the_first_frame_shows_the_step_and_a_quiet_frame_writes_nothing() {
        let (v, mut w) = sample(Step::White, false);
        w.fresh = true;
        let out = ui_run(&v, &mut w, egui::RawInput::default());
        assert!(out.show && out.show_reference && out.done.is_none());
        let out = ui_run(&v, &mut w, egui::RawInput::default());
        assert_eq!(out, Output::default(), "nothing moved: nothing is written");
    }

    #[test]
    fn steps_have_their_colours_and_go_in_order() {
        let colors: Vec<_> = Step::ALL.iter().map(|s| s.color()).collect();
        assert_eq!(colors[0], [255; 3]);
        assert_eq!(colors[4], [128; 3]);
        let (_, mut w) = sample(Step::White, false);
        let mut out = Output::default();
        go(&mut w, Step::Red, &mut out);
        assert!(out.show && out.show_reference);
        assert_eq!(w.step.index(), 1);
    }

    #[test]
    fn reset_step_and_start_over() {
        let (_, mut w) = sample(Step::Red, false);
        w.reset_step();
        let m = w.cal.device.matrix();
        assert_eq!([m[0][0], m[1][0], m[2][0]], [1.0, 0.0, 0.0], "red's column");
        assert_eq!(m[1][1], 0.82, "green's column stays");
        assert_eq!(w.cal.device.gain, [1.0, 0.86, 0.72], "white stays");
        w.step = Step::White;
        w.reset_step();
        assert_eq!(
            (w.cal.device.gain, w.cal.device.brightness),
            ([1.0; 3], 1.0)
        );
        w.step = Step::Grey;
        w.reset_step();
        assert_eq!(w.cal.device.gamma, [1.0; 3]);
        let (_, mut w) = sample(Step::Blue, true);
        w.zone = Some("5".into());
        w.editing().brightness = 0.5;
        w.start_over();
        assert_eq!(w.cal, start(&case()));
        assert_eq!((w.step, w.zone.clone()), (Step::White, None));
    }

    #[test]
    fn a_zone_starts_from_the_device_and_save_keeps_only_what_differs() {
        let (_, mut w) = sample(Step::White, false);
        w.zone = Some("5".into());
        let device = w.cal.device.clone();
        assert_eq!(*w.editing(), device, "starts from the device's");
        w.zone = Some("6".into());
        w.editing().brightness = 0.6;
        let saved = w.result().unwrap();
        assert!(
            !saved.zones.contains_key("5"),
            "same as the device: folded in"
        );
        assert_eq!(saved.zones["6"].brightness, 0.6);
        assert_eq!(saved.name, "HP OMEN 45L case lights");
        // Back to the model's default with nothing saved before: nothing to keep.
        w.start_over();
        assert_eq!(w.result(), None);
        // An identity matrix isn't kept.
        w.cal.device.matrix = Some(IDENTITY_MATRIX);
        w.cal.device.gain = [1.0, 0.9, 0.9];
        assert_eq!(w.result().unwrap().device.matrix, None);
    }

    #[test]
    fn cancel_puts_back_what_was_saved() {
        let key = "calibrate-ui-cancel";
        let saved = DeviceCalibration {
            device: Calibration {
                brightness: 0.7,
                ..Default::default()
            },
            ..Default::default()
        };
        calibration::global().set_device(key, Some(saved.clone()));
        let mut target = case();
        target.key = key.into();
        // Before there is a device or page: show and restore only queue (no worker runs).
        let mut w = Wizard::new(target, Some(saved.clone()));
        w.cal.device.brightness = 0.3;
        let preview = Output {
            show: true,
            ..Default::default()
        };
        assert!(!act(&mut w, preview));
        assert_eq!(
            calibration::global().device(key).unwrap().device.brightness,
            0.3
        );
        let cancel = Output {
            done: Some(Done::Cancel),
            ..Default::default()
        };
        assert!(act(&mut w, cancel));
        assert_eq!(calibration::global().device(key), Some(saved));
        calibration::global().set_device(key, None);
    }

    #[test]
    fn copy_and_import_round_trip() {
        let (_, w) = sample(Step::Grey, false);
        let text = calibration::export(&w.target.key, &w.cal).unwrap();
        let (_, cal) = calibration::import(&text).unwrap();
        assert_eq!(cal.device, w.cal.device.clone().sanitized());
        assert!(calibration::import("not a calibration").is_err());
    }

    #[test]
    fn linked_mid_tones_move_together() {
        let (v, mut w) = sample(Step::Grey, false);
        let text = shown(&v, &mut w);
        assert!(
            text.contains("Mid-tones") && !text.contains("Red mid-tones"),
            "{text}"
        );
        w.linked = false;
        let text = shown(&v, &mut w);
        assert!(text.contains("Red mid-tones") && text.contains("Blue mid-tones"));
    }

    #[test]
    fn every_step_has_its_sliders_and_zones_show_for_the_case() {
        for (step, label) in [
            (Step::White, "Brightness"),
            (Step::Red, "Add green"),
            (Step::Green, "Add red"),
            (Step::Blue, "Add green"),
            (Step::Grey, "Mid-tones"),
        ] {
            let (v, mut w) = sample(step, false);
            let text = shown(&v, &mut w);
            assert!(text.contains(label), "{step:?}: {text}");
            assert!(text.contains("Front top fan") && text.contains("All zones"));
            assert!(text.contains(if step == Step::Grey { "Save" } else { "Next" }));
        }
        let (v, mut w) = sample(Step::Blue, true);
        let text = shown(&v, &mut w);
        assert!(text.contains("On the Eyelash Corne"), "{text}");
        let mut keyboard = Wizard::new(corne(), None);
        let text = shown(&View::default(), &mut keyboard);
        assert!(!text.contains("All zones"), "the keyboard has one colour");
        let moving = View {
            moving: true,
            ..View::default()
        };
        assert!(shown(&moving, &mut keyboard).contains("Solid"));
    }

    /// Draws the calibrator headless and returns its text.
    fn shown(v: &View, w: &mut Wizard) -> String {
        let ctx = super::super::pc_lights_ui::tests::test_ctx();
        let mut first = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw(ui, v, &mut w.clone());
        });
        first.textures_delta.clear();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw(ui, v, w);
        });
        out.textures_delta.clear();
        let mut text = String::new();
        for clipped in &out.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
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
}
