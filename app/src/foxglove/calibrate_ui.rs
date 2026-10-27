//! The colour calibrator: "Calibrate…" on a device of the Lights page, as a dialog over it.
//!
//! Calibration here is a set of matches the user confirms, never a measurement of what a colour
//! "really" is. For each test colour the device shows it and the reference (the screen, as a
//! swatch, or another device, the "master") shows it too. The user moves the **light** (what
//! the device is sent) or the **screen** (what Foxglove draws for this device), or both, with
//! the "Adjust" switch, until the two look alike, then clicks **These match**. Each side keeps
//! its own values (blue down 30% on the light and up 5% on the screen are both kept), and the
//! device's transforms are fitted to the matches (`foxglove_devices::calibration::Fit`).
//!
//! The test colours sit mid-range (about 60%, never a full channel), so either side can go up
//! as well as down. A slider change marks the frame, and the frame sends one write (the PC
//! lights' worker and the keyboard's writer drop any that a newer one replaces). Nothing moves,
//! nothing is written.
//!
//! Save keeps the matches; Cancel puts the previous calibration and colours back. [`show`] runs
//! it on the page; [`draw`] paints a [`Wizard`] and returns what was asked, so the previews and
//! tests need no device.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Color32, RichText, Sense, Stroke};
use foxglove_devices::calibration::{self, DeviceCalibration, Match, Side};

use super::calibrate::{self, Before, Kind, Target};
use super::settings_ui::card;
use super::theme;

/// The test colours, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    White,
    Grey,
    Red,
    Green,
    Blue,
    Skin,
    Warm,
}

/// The brightest a test colour's channel goes (60%): room above it for either side.
pub const TEST_LEVEL: u8 = 153;

impl Step {
    pub const ALL: [Step; 7] = [
        Step::White,
        Step::Grey,
        Step::Red,
        Step::Green,
        Step::Blue,
        Step::Skin,
        Step::Warm,
    ];

    /// The colour the step shows (sRGB): mid-range, no channel above [`TEST_LEVEL`].
    pub fn color(self) -> [u8; 3] {
        match self {
            Step::White => [153, 153, 153],
            Step::Grey => [115, 115, 115],
            Step::Red => [153, 0, 0],
            Step::Green => [0, 153, 0],
            Step::Blue => [0, 0, 153],
            Step::Skin => [153, 112, 92],
            Step::Warm => [153, 97, 38],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Step::White => "White",
            Step::Grey => "Grey",
            Step::Red => "Red",
            Step::Green => "Green",
            Step::Blue => "Blue",
            Step::Skin => "Skin",
            Step::Warm => "Warm",
        }
    }

    fn of(color: [u8; 3]) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.color() == color)
    }
}

const CHANNELS: [&str; 3] = ["Red", "Green", "Blue"];
const SHORT: [&str; 3] = ["R", "G", "B"];

/// A colour brought down to the test level, keeping its hue: a colour from a picker, matched.
pub fn test_color(c: [u8; 3]) -> [u8; 3] {
    let max = c.into_iter().max().unwrap_or(0);
    if max <= TEST_LEVEL {
        return c;
    }
    c.map(|v| (f32::from(v) * f32::from(TEST_LEVEL) / f32::from(max)).round() as u8)
}

/// The calibrator's state, kept by the page between frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Wizard {
    pub target: Target,
    /// What it's matched to: `None` is the screen.
    pub reference: Option<Target>,
    /// The zone being matched on its own (by id); `None` is every zone.
    pub zone: Option<String>,
    pub step: Step,
    /// A colour from a picker being matched, in place of the step's.
    pub custom: Option<[u8; 3]>,
    /// Which side the sliders move.
    pub side: Side,
    /// The match being made for the colour on show.
    pub current: Match,
    /// The calibration being made: the device's values and its matches.
    pub cal: DeviceCalibration,
    /// What was saved before, put back on Cancel.
    pub saved: Option<DeviceCalibration>,
    /// What the device and the reference showed before, put back at the end.
    pub before: Before,
    pub reference_before: Before,
    /// The import box's text, while it's open.
    pub import: Option<String>,
    pub note: Option<String>,
    /// The device hasn't been shown this colour yet.
    pub fresh: bool,
}

impl Wizard {
    /// Starts on a device, from its saved calibration (else its model's default).
    pub fn new(target: Target, saved: Option<DeviceCalibration>) -> Self {
        let cal = saved.clone().unwrap_or_else(|| start(&target));
        let mut w = Self {
            target,
            reference: None,
            zone: None,
            step: Step::White,
            custom: None,
            side: Side::Light,
            current: Match::plain(Step::White.color()),
            cal,
            saved,
            before: Before::Unknown,
            reference_before: Before::Unknown,
            import: None,
            note: None,
            fresh: true,
        };
        w.load_current();
        w
    }

    /// The colour on show.
    pub fn color(&self) -> [u8; 3] {
        self.custom.unwrap_or(self.step.color())
    }

    /// The saved match for this colour on this zone, if there is one.
    fn saved_match(&self, color: [u8; 3]) -> Option<usize> {
        self.cal
            .matches
            .iter()
            .position(|m| m.color == color && m.zone == self.zone)
    }

    /// Whether a colour has a match on the zone being matched.
    pub fn matched(&self, color: [u8; 3]) -> bool {
        self.saved_match(color).is_some()
    }

    /// The match to adjust for the colour on show: its saved one, else where the matches so far
    /// put it (so each match starts from the ones before).
    fn load_current(&mut self) {
        let color = self.color();
        self.current = match self.saved_match(color) {
            Some(i) => self.cal.matches[i].clone(),
            None => {
                let zone = self.zone.as_deref();
                let rgb = rgb(color);
                let light = self.cal.fit(zone, Side::Light).apply_rgb(rgb);
                let screen = self.cal.fit(zone, Side::Screen).apply_rgb(rgb);
                Match {
                    color,
                    light: [light.r, light.g, light.b],
                    screen: [screen.r, screen.g, screen.b],
                    zone: self.zone.clone(),
                }
            }
        };
    }

    /// Goes to a test colour (`custom`: a picked one).
    pub fn go_to(&mut self, step: Step, custom: Option<[u8; 3]>) {
        self.step = step;
        self.custom = custom.filter(|c| Step::of(*c).is_none());
        if let Some(s) = custom.and_then(Step::of) {
            self.step = s;
        }
        self.load_current();
    }

    /// "These match": keeps the current pair, replacing this colour's match on this zone.
    pub fn confirm(&mut self) {
        let m = Match {
            zone: self.zone.clone(),
            ..self.current.clone()
        };
        match self.saved_match(m.color) {
            Some(i) => self.cal.matches[i] = m,
            None => self.cal.matches.push(m),
        }
    }

    /// The colour on show was adjusted since its match (or its start) and not confirmed.
    pub fn unconfirmed(&self) -> bool {
        let mut probe = self.clone();
        probe.load_current();
        probe.current != self.current
    }

    /// The next test colour without a match, after this one.
    pub fn next_unmatched(&self) -> Option<Step> {
        let at = Step::ALL.iter().position(|s| *s == self.step).unwrap_or(0);
        (1..Step::ALL.len())
            .map(|i| Step::ALL[(at + i) % Step::ALL.len()])
            .find(|s| !self.matched(s.color()))
    }

    /// Forgets a match.
    pub fn remove(&mut self, i: usize) {
        if i < self.cal.matches.len() {
            self.cal.matches.remove(i);
            self.load_current();
        }
    }

    /// Picks a saved match again, to adjust and confirm it anew.
    pub fn redo(&mut self, i: usize) {
        let Some(m) = self.cal.matches.get(i).cloned() else {
            return;
        };
        self.zone = m.zone.clone();
        let step = Step::of(m.color);
        self.step = step.unwrap_or(self.step);
        self.custom = step.is_none().then_some(m.color);
        self.current = m;
    }

    /// One side of the colour on show back to the colour itself.
    pub fn reset_side(&mut self, side: Side) {
        let color = self.current.color;
        *self.current.side_mut(side) = color;
    }

    /// Every match gone and the values back to the model's default, from the first colour.
    pub fn start_over(&mut self) {
        self.cal = start(&self.target);
        self.zone = None;
        self.go_to(Step::White, None);
    }

    /// What Save keeps: `None` when it's just the model's default with no matches (nothing to
    /// store, and a saved one is forgotten).
    pub fn result(&self) -> Option<DeviceCalibration> {
        let tidy = |c: &calibration::Calibration| {
            let mut c = c.clone().sanitized();
            if c.matrix.is_some_and(|m| m == calibration::IDENTITY_MATRIX) {
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
        if zones.is_empty() && device == default && self.cal.matches.is_empty() {
            return None;
        }
        Some(DeviceCalibration {
            name: self.target.name.clone(),
            device,
            zones,
            matches: self.cal.matches.clone(),
        })
    }

    /// What the registry holds while matching: the values only, so the device shows exactly
    /// the light value being adjusted.
    fn previewing(&self) -> DeviceCalibration {
        DeviceCalibration {
            matches: Vec::new(),
            ..self.cal.clone()
        }
    }
}

fn rgb(c: [u8; 3]) -> foxglove_devices::Rgb {
    foxglove_devices::Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

fn start(target: &Target) -> DeviceCalibration {
    DeviceCalibration {
        name: target.name.clone(),
        device: target.default.clone().unwrap_or_default(),
        ..Default::default()
    }
}

/// One side of a match in words: the light as each lit channel's level ("R 100 G 96 B 70"),
/// the screen as what moved ("B +5"; "as is" when nothing did).
pub fn side_text(m: &Match, side: Side) -> String {
    let out = m.side(side);
    let parts: Vec<String> = (0..3)
        .filter(|i| m.color[*i] > 0)
        .filter_map(|i| {
            let pct = (f32::from(out[i]) * 100.0 / f32::from(m.color[i])).round() as i32;
            match side {
                Side::Light => Some(format!("{} {pct}", SHORT[i])),
                Side::Screen => (pct != 100).then(|| format!("{} {:+}", SHORT[i], pct - 100)),
            }
        })
        .collect();
    if parts.is_empty() {
        "as is".into()
    } else {
        parts.join(" ")
    }
}

/// Both sides of a match in words: "Light: R 100 G 96 B 70 · Screen: B +5".
pub fn match_text(m: &Match) -> String {
    format!(
        "Light: {} · Screen: {}",
        side_text(m, Side::Light),
        side_text(m, Side::Screen)
    )
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
    /// The light value (or the colour, or the zone) changed: show it on the device.
    pub show: bool,
    /// The colour or the reference changed: show the colour on the reference too.
    pub show_reference: bool,
    /// The user picked another reference: the old one (`None`: the screen) is put back, and
    /// the new one's colours are captured before it shows the colour.
    pub reference_changed: Option<Option<Target>>,
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
            "Match this device to your screen (or another device), one test colour at a time: \
             adjust the light, the screen or both until the two look alike to you, then click \
             These match. Colours between your matches follow them.",
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
        matches(ui, w, &mut out);
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
                out.reference_changed = Some(std::mem::replace(&mut w.reference, r.cloned()));
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
                .on_hover_text("Match this zone on its own, when it doesn't look like the rest")
                .clicked()
                && !selected
            {
                w.zone = z.cloned();
                w.load_current();
                out.show = true;
            }
        };
        choose(ui, w, None, "All zones");
        for (id, name) in &zones {
            choose(ui, w, Some(id), name);
        }
    });
}

/// The test colours as pills, ticked once matched; any can be picked.
fn steps(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for step in Step::ALL {
            let tick = if w.matched(step.color()) { " ✓" } else { "" };
            let selected = w.step == step && w.custom.is_none();
            let text = format!("{}{tick}", step.name());
            if theme::pill(ui, selected, &text).clicked() && !selected {
                go(w, step, out);
            }
        }
        if let Some(c) = w.custom {
            theme::pill(ui, true, &format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
        }
    });
}

fn go(w: &mut Wizard, step: Step, out: &mut Output) {
    w.go_to(step, None);
    out.show = true;
    out.show_reference = true;
}

/// The reference colour (the screen's swatch, through the screen side, or a note to look at the
/// other device) beside what goes to this device.
fn swatches(ui: &mut egui::Ui, w: &mut Wizard) {
    let p = theme::palette(ui);
    let sent = w
        .cal
        .zone(w.zone.as_deref())
        .apply_rgb(rgb(w.current.light));
    let reference = match &w.reference {
        None => "On your screen".to_string(),
        Some(t) => format!("On the {}", t.name),
    };
    let screen = match &w.reference {
        None => w.current.screen,
        Some(_) => w.current.color,
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (label, color, big) in [
            (reference.as_str(), screen, true),
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
            RichText::new("Both devices show this colour now: match this one to the other.")
                .size(12.0)
                .color(p.muted),
        );
    }
}

/// "Adjust: Light | Screen", each side's values, and the selected side's sliders.
fn sliders(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    let p = theme::palette(ui);
    let name = match w.custom {
        Some(_) => "Your colour".to_string(),
        None => w.step.name().to_string(),
    };
    ui.label(RichText::new(name).color(p.text_strong).strong());
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::caps_label(ui, "Adjust"));
        for (side, text, hover) in [
            (Side::Light, "Light", "Move what this device is sent"),
            (
                Side::Screen,
                "Screen",
                "Move what Foxglove draws for this device, when the light can't get there",
            ),
        ] {
            if theme::pill(ui, w.side == side, text)
                .on_hover_text(hover)
                .clicked()
            {
                w.side = side;
            }
        }
    });
    ui.label(RichText::new(match_text(&w.current)).font(theme::mono(12.0)));
    ui.add_space(4.0);
    let side = w.side;
    let color = w.current.color;
    let mut changed = false;
    for (i, name) in CHANNELS.iter().enumerate() {
        let lit = color[i] > 0;
        let value = &mut w.current.side_mut(side)[i];
        // Whole percent, as the slider steps: drawing it never counts as a move.
        let mut pct = if lit {
            (f32::from(*value) * 100.0 / f32::from(color[i])).round()
        } else {
            100.0
        };
        let moved = ui
            .add_enabled_ui(lit, |ui| slider(ui, name, &mut pct))
            .inner;
        if moved && lit {
            *value = (f32::from(color[i]) * pct / 100.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            changed = true;
        }
    }
    ui.label(
        RichText::new(
            "100% is the colour as it is; go either way. Channels this colour doesn't use stay off.",
        )
        .size(12.0)
        .color(p.muted),
    );
    if changed && side == Side::Light {
        out.show = true;
    }
}

/// One channel's slider across the column, in percent of the colour (0-200), its value in the
/// row above.
fn slider(ui: &mut egui::Ui, name: &str, pct: &mut f32) -> bool {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{pct:.0}%")).font(theme::mono(12.5)));
        });
    });
    ui.spacing_mut().slider_width = ui.available_width();
    ui.add(
        egui::Slider::new(pct, 0.0..=200.0)
            .show_value(false)
            .step_by(1.0),
    )
    .changed()
}

/// These match, reset per side, Start over, Cancel and Save.
fn buttons(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(theme::primary_button(ui, "These match"))
            .on_hover_text("Keep this pair: the light and the screen as they are now")
            .clicked()
        {
            w.confirm();
            let name = w
                .custom
                .map_or(w.step.name().to_string(), |_| "Your colour".into());
            w.note = Some(format!("Matched: {name}"));
            if let Some(next) = w.next_unmatched().filter(|_| w.custom.is_none()) {
                go(w, next, out);
            }
        }
        if ui
            .button("Reset light")
            .on_hover_text("The light back to the colour as it is")
            .clicked()
        {
            w.reset_side(Side::Light);
            out.show = true;
        }
        if ui
            .button("Reset screen")
            .on_hover_text("The screen back to the colour as it is")
            .clicked()
        {
            w.reset_side(Side::Screen);
        }
    });
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
            .on_hover_text("Forget every match and start from White")
            .clicked()
        {
            w.start_over();
            out.show = true;
            out.show_reference = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Save")
                .on_hover_text("Keep the matches made so far")
                .clicked()
            {
                out.done = Some(Done::Save);
            }
        });
    });
}

/// The matches made so far, each with Redo and Remove.
fn matches(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    let p = theme::palette(ui);
    ui.label(theme::caps_label(ui, "Matches"));
    if w.cal.matches.is_empty() {
        ui.weak("None yet: match a colour, then click These match.");
        return;
    }
    let zones = match &w.target.kind {
        Kind::Lights { zones } => zones.clone(),
        Kind::Keyboard => Vec::new(),
    };
    let mut act: Option<(usize, bool)> = None;
    for (i, m) in w.cal.matches.iter().enumerate() {
        ui.horizontal(|ui| {
            for c in [m.screen, m.light] {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
            }
            let name = Step::of(m.color).map_or_else(
                || format!("#{:02x}{:02x}{:02x}", m.color[0], m.color[1], m.color[2]),
                |s| s.name().to_string(),
            );
            let zone = m.zone.as_ref().map(|z| {
                zones
                    .iter()
                    .find(|(id, _)| id == z)
                    .map_or(z.clone(), |(_, n)| n.clone())
            });
            let name = match zone {
                Some(z) => format!("{name} · {z}"),
                None => name,
            };
            ui.label(RichText::new(name).color(p.text_strong));
            ui.label(
                RichText::new(match_text(m))
                    .font(theme::mono(11.5))
                    .color(p.muted),
            );
            if ui.small_button("Redo").clicked() {
                act = Some((i, true));
            }
            if ui.small_button("Remove").clicked() {
                act = Some((i, false));
            }
        });
    }
    match act {
        Some((i, true)) => {
            w.redo(i);
            out.show = true;
            out.show_reference = true;
        }
        Some((i, false)) => {
            w.remove(i);
            out.show = true;
        }
        None => {}
    }
}

/// Copy the calibration (to keep or share), or paste one in.
fn share(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    ui.horizontal(|ui| {
        if ui
            .small_button("Copy calibration")
            .on_hover_text("This device's matches as text, to keep or to use on another one")
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
                w.load_current();
                w.note = Some("Imported: check the matches, then Save".into());
                out.show = true;
            }
            Err(e) => w.note = Some(format!("Can't use it: {e}")),
        }
    }
}

/// Beside a picker colour on the Lights page: "not matched yet" when the device has matches but
/// none near this colour. True when it's clicked, to add a match for it.
pub fn not_matched(ui: &mut egui::Ui, key: &str, zone: Option<&str>, color: [u8; 3]) -> bool {
    if calibrate::covered(key, zone, color) != Some(false) {
        return false;
    }
    let p = theme::palette(ui);
    ui.add(
        egui::Button::new(RichText::new("not matched yet").size(11.5).color(p.muted))
            .small()
            .frame(false),
    )
    .on_hover_text("None of this light's matches is near this colour: click to add a match for it")
    .clicked()
}

static OPEN: Mutex<Option<Wizard>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' calibrator, so they never touch real devices.
    pub static PREVIEW: std::cell::RefCell<Option<(View, Wizard)>> = const { std::cell::RefCell::new(None) };
}

/// Opens the calibrator on a device, from what it shows now.
pub fn open(target: Target) {
    open_at(target, None, None);
}

/// Opens the calibrator on a device at a picked colour (brought to the test level) and zone,
/// to add a match for it.
pub fn open_at(target: Target, zone: Option<String>, color: Option<[u8; 3]>) {
    let saved = calibration::global().device(&target.key);
    let mut w = Wizard::new(target, saved);
    w.zone = zone;
    w.go_to(Step::White, color.map(test_color));
    w.before = calibrate::capture(&w.target);
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
}

/// The dialog's backdrop over the page (the layout guard knows it by this colour).
pub const BACKDROP: Color32 = Color32::from_black_alpha(140);

/// Where the dialog's frame was last drawn, kept in the context's temp data: the frame takes
/// the clicks between its controls, so the layout guard counts it as their container.
#[cfg(test)]
pub fn dialog_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    ctx.data(|d| d.get_temp(egui::Id::new(DIALOG_RECT)))
}

const DIALOG_RECT: &str = "foxglove-calibrate-rect";

/// The calibrator as a dialog over the Lights page (#533), while it's open. Escape cancels it,
/// as Cancel does; a click on the page behind it does nothing, so a stray click keeps the
/// matches made so far.
pub fn dialog(ctx: &egui::Context) {
    #[cfg(test)]
    if let Some((v, mut w)) = PREVIEW.with_borrow(|v| v.clone()) {
        modal(ctx, |ui| draw(ui, &v, &mut w));
        return;
    }
    let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(w) = guard.as_mut() else {
        return;
    };
    let view = View {
        others: others(&w.target),
        moving: moving(w),
    };
    let (mut out, escape) = modal(ctx, |ui| draw(ui, &view, w));
    if escape {
        // A Save clicked in the same frame wins.
        out.done.get_or_insert(Done::Cancel);
    }
    if let Some(text) = out.copy.clone() {
        ctx.copy_text(text);
    }
    if act(w, out) {
        *guard = None;
    }
}

/// Draws `body` in a modal dialog, centred, as wide as the window allows (up to 720 points)
/// and scrolling when it's taller than the window. True with it when Escape was pressed.
fn modal<R>(ctx: &egui::Context, body: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    // Escape in the import box only leaves the box (egui doesn't consume the key there).
    let typing = ctx
        .memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
    let screen = ctx.content_rect();
    let width = (screen.width() - 48.0).clamp(240.0, 720.0);
    let height = (screen.height() - 48.0).max(160.0);
    let r = egui::Modal::new(egui::Id::new("foxglove-calibrate"))
        .backdrop_color(BACKDROP)
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            // The area's room is last frame's size: set both, so the dialog can grow back.
            ui.set_width(width);
            ui.set_max_height(height);
            let shown = egui::ScrollArea::vertical()
                .max_height(height)
                .auto_shrink([false, true])
                .show(ui, body);
            let rect = shown.inner_rect;
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new(DIALOG_RECT), rect));
            shown.inner
        });
    let escape = r.is_top_modal
        && !r.any_popup_open
        && !typing
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    (r.inner, escape)
}

/// Ends an open calibrator as Cancel does: the Lights page was left, the settings window
/// closed, or the app is quitting. The saved calibration and the colours go back.
pub fn cancel() {
    let open = OPEN.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mut w) = open {
        act(
            &mut w,
            Output {
                done: Some(Done::Cancel),
                ..Default::default()
            },
        );
    }
}

/// Sends what a frame asked for; true when the calibrator is done.
fn act(w: &mut Wizard, out: Output) -> bool {
    if let Some(old) = &out.reference_changed {
        if let Some(old) = old {
            calibrate::restore(old, &w.reference_before);
        }
        w.reference_before = w
            .reference
            .as_ref()
            .map(calibrate::capture)
            .unwrap_or_default();
    }
    if out.show {
        // The registry holds the values without the matches, so the device shows the light
        // value being adjusted as it is.
        calibration::global().set_device(&w.target.key, Some(w.previewing()));
        calibrate::show(&w.target, w.current.light);
    }
    if out.show_reference {
        if let Some(r) = &w.reference {
            calibrate::show(r, w.current.color);
        }
    }
    let Some(done) = out.done else {
        return false;
    };
    match done {
        Done::Cancel => calibration::global().set_device(&w.target.key, w.saved.clone()),
        Done::Save => {
            // Adjusted but not confirmed yet: Save takes it as a match.
            if w.unconfirmed() {
                w.confirm();
            }
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
#[path = "calibrate_ui_tests.rs"]
pub mod tests;
