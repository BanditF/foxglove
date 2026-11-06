//! "Check zones…" (#611, #90 row G; was "Find my LEDs…"): the zone drawing's "Not right?", the
//! Keyboard card's zone line while the zones are unknown, and the Scenes page's prompt while
//! the keyboard's map isn't checked. One lamp lights at a time; the user clicks every key it
//! lights (docs/lighting-zones.md section 4).
//!
//! It opens on "Check LED map" first ([`led_map_confirm`]): a dot runs along the LEDs on the
//! keyboard and on the drawing, and one to three answers (matches, backwards, mirrored, rows
//! flipped) settle the map. Clicking each LED is the fallback, for a board whose LEDs aren't one
//! per key or a chain the answers can't fix.
//!
//! The pattern is Philips Hue's entertainment-area setup and Nanoleaf's layout assistant: the
//! light you're asked about blinks in the room, you show the app where it is. Here it's a
//! click on each key that lit (a second click unpicks it) and Next, a click between keys for
//! underglow, "Nothing lit" for a dead lamp, Back to undo, and a count of how far along it is.
//! One answer is the whole group of keys a lamp lights, so the saved check is the board's
//! zones ([`led_map::BoardLeds::led_keys`]).
//!
//! While open it replaces the Lights page's cards. Leaving the page, closing the window,
//! quitting, pausing or going away (locked, display off) cancels it, which gives the lights
//! back. [`draw`] paints a [`Wizard`] and says what was asked, so the previews and tests need no
//! keyboard.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::key_layout::{KeyLayout, LayoutKey};
use super::led_map::{self, MapState, Status, Wizard, Worker};
use super::led_map_confirm::{self as confirm, Answer, Confirm, Source};
use super::parts::{upper_first, Parts};
use super::settings_ui::{card, split_row};
use super::theme;

/// Key units between the two halves on the drawing.
const GAP: f32 = 1.5;
/// How far outside a half's outline a click still counts, in key units (LEDs on the edge).
const MARGIN: f32 = 0.4;
/// The drawing's height, in points.
const DRAWING_HEIGHT: f32 = 230.0;

/// What the wizard's card shows besides the wizard itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The LED asked about is lit on the keyboard now.
    pub lit: bool,
    /// The keyboard refused or isn't there.
    pub error: Option<String>,
    /// A part that doesn't draw the frames (the other half not linked): its LEDs won't light.
    pub warn: Option<String>,
}

impl View {
    fn of(status: &Status, w: &Wizard) -> View {
        View {
            lit: status.shown.as_ref() == Some(&w.frame()),
            error: status.error.clone(),
            warn: status.warn.clone(),
        }
    }
}

/// What a frame asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Output {
    /// The wizard moved on or back: the keyboard gets its new frame.
    pub changed: bool,
    pub done: Option<Done>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    Save,
    Cancel,
}

/// Where the drawing puts the board: the halves side by side, or a one-piece board's whole
/// drawing as one area (#559), scaled to fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    origin: Pos2,
    /// Points per key unit.
    pub scale: f32,
    /// One area's size, in key units.
    area: Vec2,
    areas: usize,
}

impl Geometry {
    pub fn new(layout: &KeyLayout, halves: u8, within: Rect) -> Geometry {
        let (area, areas) = (layout.size, if halves == 1 { 1 } else { 2 });
        let total = Vec2::new(
            area.x * areas as f32 + GAP * (areas - 1) as f32 + MARGIN * 2.0,
            area.y + MARGIN * 2.0,
        );
        let scale = (within.width() / total.x)
            .min(within.height() / total.y)
            .max(1.0);
        let used = total * scale;
        let origin = within.center() - used / 2.0 + Vec2::splat(MARGIN * scale);
        Geometry {
            origin,
            scale,
            area,
            areas,
        }
    }

    /// Area `a`'s outline on screen.
    pub fn rect(&self, a: usize) -> Rect {
        let min = self.origin + Vec2::new(a as f32 * (self.area.x + GAP) * self.scale, 0.0);
        Rect::from_min_size(min, self.area * self.scale)
    }

    pub fn screen(&self, a: usize, p: [f32; 2]) -> Pos2 {
        self.rect(a).min + Vec2::new(p[0], p[1]) * self.scale
    }

    /// The point on area `a` under `pos`, in key units, when it's on (or just outside) its
    /// outline.
    pub fn at(&self, a: usize, pos: Pos2) -> Option<[f32; 2]> {
        let r = self.rect(a);
        if !r.expand(MARGIN * self.scale).contains(pos) {
            return None;
        }
        let p = (pos - r.min) / self.scale;
        Some([
            p.x.clamp(-MARGIN, self.area.x + MARGIN),
            p.y.clamp(-MARGIN, self.area.y + MARGIN),
        ])
    }

    /// Area `a`'s keys, in key units from its corner (#796: the keyboard's layout, turned
    /// where it turns them).
    pub fn keys<'a>(&self, layout: &'a KeyLayout, a: usize) -> &'a [LayoutKey] {
        layout.part(if self.areas > 1 { a } else { 0 })
    }

    /// A key on screen, turned: `fill` and `line` as the caller paints it.
    pub fn key_shape(&self, a: usize, k: &LayoutKey, fill: Color32, line: Stroke) -> egui::Shape {
        let r = Rect::from_min_max(
            self.screen(a, [k.rect.min.x, k.rect.min.y]),
            self.screen(a, [k.rect.max.x, k.rect.max.y]),
        )
        .shrink(self.scale * 0.06);
        egui::epaint::RectShape::new(r, 3.0, fill, line, egui::StrokeKind::Inside)
            .with_angle(k.r.to_radians())
            .into()
    }
}

/// What each part is called in the steps: "left half", "keyboard".
fn half_name(w: &Wizard, half: usize) -> String {
    Parts::halves(w.halves).lower(half)
}

/// The probe's title (#611, docs/lighting-zones.md section 4): one name, "Check zones · <device>".
pub fn title(name: &str) -> String {
    format!("Check zones · {name}")
}

/// "Lamp 3 of 21 on the left half is lit in white on your keyboard." ("Lamp 3 of 61 is lit
/// …" on a one-piece board).
pub fn lamp_text(w: &Wizard, s: led_map::Step) -> String {
    let on = if w.halves == 1 {
        String::new()
    } else {
        format!(" on the {}", half_name(w, s.half))
    };
    format!(
        "Lamp {} of {}{on} is lit in white on your keyboard.",
        s.index + 1,
        w.part_total(s.half)
    )
}

/// The ask under it.
pub const ASK: &str = "Click the keys that lit, then Next.";

/// The muted line under the ask.
pub const HOW: &str =
    "A second click unpicks a key. Lit under no key (underglow)? Click where you see it.";

/// Paints the wizard (Check zones' main step: one lamp lit, the user clicks every key it
/// lights); returns what was asked this frame.
pub fn draw(ui: &mut egui::Ui, v: &View, w: &mut Wizard) -> Output {
    let mut out = Output::default();
    card(ui, &title(&w.name), |ui| {
        let p = theme::palette(ui);
        let total = w.total();
        let answered = w.answers.len();
        if let Some(intro) = &w.intro {
            ui.add(egui::Label::new(RichText::new(intro).color(p.muted)).wrap());
        }
        match w.step() {
            Some(s) => {
                split_row(
                    ui,
                    |ui| {
                        let nothing = answered - w.seen();
                        if nothing > 0 {
                            ui.label(
                                RichText::new(format!("{nothing} lit nothing")).color(p.muted),
                            );
                        }
                    },
                    |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(lamp_text(w, s)).color(p.text_strong))
                                .wrap(),
                        );
                    },
                );
                progress(ui, answered, total);
                ui.add(egui::Label::new(RichText::new(ASK).color(p.text_strong)).wrap());
                ui.add(egui::Label::new(RichText::new(HOW).color(p.muted).size(12.5)).wrap());
            }
            None => {
                ui.label(RichText::new(format!("All {total} lamps done")).color(p.text_strong));
                progress(ui, answered, total);
                let text = match total - w.seen() {
                    0 => "Save, and scenes light each zone where it is.".to_string(),
                    n => format!(
                        "{} lit something, {n} lit nothing: Foxglove leaves those out. Save, \
                         and scenes light each zone where it is.",
                        w.seen()
                    ),
                };
                ui.add(egui::Label::new(text).wrap());
            }
        }
        status_line(ui, v, w);
        ui.add_space(4.0);
        board(ui, w);
        ui.add_space(6.0);
        buttons(ui, w, &mut out);
        if let Some(note) = &w.note {
            ui.add(egui::Label::new(RichText::new(note).color(p.warn)).wrap());
        }
    });
    out
}

fn progress(ui: &mut egui::Ui, answered: usize, total: usize) {
    let done = answered as f32 / total.max(1) as f32;
    ui.add(egui::ProgressBar::new(done).desired_height(6.0));
}

/// Whether the lamp is lit yet, or what went wrong.
fn status_line(ui: &mut egui::Ui, v: &View, w: &Wizard) {
    let p = theme::palette(ui);
    if let Some(e) = &v.error {
        ui.add(egui::Label::new(RichText::new(e).color(p.warn)).wrap());
    } else if !v.lit && !w.done() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Lighting it…").color(p.muted));
        });
    }
    if let Some(warn) = v.warn.as_ref().filter(|_| v.error.is_none()) {
        ui.add(egui::Label::new(RichText::new(warn).color(p.warn)).wrap());
    }
}

/// The key of a part's drawing (`keys`) under `at` (key units), if any, turned keys as drawn.
pub fn key_at(keys: &[LayoutKey], at: [f32; 2]) -> Option<u8> {
    let at = Pos2::new(at[0], at[1]);
    keys.iter()
        .position(|k| k.contains(at))
        .and_then(|k| u8::try_from(k).ok())
}

/// The board, top-down: the halves' keys, what the lamps before lit (faint), the keys picked
/// for the lit lamp (filled), and the click area on the lit half. A click on a key picks or
/// unpicks it; a click between keys places the lamp there.
fn board(ui: &mut egui::Ui, w: &mut Wizard) {
    let p = theme::palette(ui);
    let size = Vec2::new(ui.available_width(), DRAWING_HEIGHT);
    let step = w.step();
    let sense = if step.is_some() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, p.well);
    let g = Geometry::new(&w.layout, w.halves, rect.shrink(12.0));
    let active = step.map(|s| s.half);
    let areas = usize::from(w.halves.clamp(1, 2));
    let hover = step.and_then(|s| {
        let half = s.half.min(areas - 1);
        response
            .hover_pos()
            .and_then(|pos| g.at(half, pos))
            .map(|at| (half, at))
    });
    for a in 0..areas {
        let lit = active == Some(a);
        let outline = g.rect(a).expand(MARGIN * g.scale);
        if lit {
            painter.rect(
                outline,
                8.0,
                p.surface_raised,
                Stroke::new(1.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        let key_line = if lit || active.is_none() {
            p.text
        } else {
            p.line
        };
        let keys = g.keys(&w.layout, a);
        // What the lamps before lit on this part.
        let before: Vec<u8> = w
            .keys
            .iter()
            .enumerate()
            .filter(|(i, _)| w.part_of(*i).min(areas - 1) == a)
            .flat_map(|(_, k)| k.iter().copied())
            .collect();
        let hovered = hover
            .filter(|(h, _)| *h == a)
            .and_then(|(_, at)| key_at(keys, at));
        for (i, k) in keys.iter().enumerate() {
            let i = u8::try_from(i).ok();
            let picked = lit && i.is_some_and(|i| w.picked.contains(&i));
            let fill = if picked {
                p.accent.gamma_multiply(0.6)
            } else if i.is_some_and(|i| before.contains(&i)) {
                p.accent.gamma_multiply(0.15)
            } else {
                Color32::TRANSPARENT
            };
            let line = if hovered.is_some() && hovered == i {
                p.accent
            } else {
                key_line
            };
            painter.add(g.key_shape(a, k, fill, Stroke::new(1.0, line)));
        }
    }
    // The lamps placed between keys so far (in the profile's key units, #796).
    for (i, a) in w.answers.iter().enumerate() {
        let Some(at) = a else { continue };
        if w.keys.get(i).is_some_and(|k| !k.is_empty()) {
            continue;
        }
        let a = w.part_of(i).min(areas - 1);
        painter.circle_filled(
            g.screen(a, w.layout.point_from_profile(a, *at)),
            4.0,
            p.accent,
        );
    }
    let Some(s) = step else {
        return;
    };
    let half = s.half.min(areas - 1);
    if let Some(at) = w.point {
        let c = g.screen(half, w.layout.point_from_profile(half, at));
        painter.circle(c, 5.0, Color32::WHITE, Stroke::new(1.5, p.accent));
    }
    if let Some((_, at)) = hover {
        if key_at(g.keys(&w.layout, half), at).is_none() {
            painter.circle_stroke(g.screen(half, at), 6.0, Stroke::new(1.0, p.accent));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    if response.clicked() {
        if let Some(at) = response
            .interact_pointer_pos()
            .and_then(|pos| g.at(half, pos))
        {
            match key_at(g.keys(&w.layout, half), at) {
                Some(k) => w.toggle(k),
                None => w.pick_point(w.layout.point_to_profile(half, at)),
            }
        }
    }
}

fn buttons(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
    let mut cancel = false;
    split_row(
        ui,
        |ui| {
            cancel = ui
                .button("Cancel")
                .on_hover_text("Stop without saving; the lights go back as they were")
                .clicked();
        },
        |ui| {
            if w.done() {
                if ui
                    .add(theme::primary_button(ui, "Save"))
                    .on_hover_text("Keep what each lamp lights; scenes use it from now on")
                    .clicked()
                {
                    out.done = Some(Done::Save);
                }
            } else {
                if ui
                    .add_enabled(w.has_pick(), theme::primary_button(ui, "Next"))
                    .on_hover_text("These keys lit; light the next lamp")
                    .on_disabled_hover_text("Click the keys that lit first")
                    .clicked()
                {
                    w.next();
                    out.changed = true;
                }
                if ui
                    .button("Nothing lit")
                    .on_hover_text("This lamp lights nothing you can see (hidden, or not fitted)")
                    .clicked()
                {
                    w.skip();
                    out.changed = true;
                }
            }
            if ui
                .add_enabled(!w.answers.is_empty(), egui::Button::new("Back"))
                .on_hover_text("Light the lamp before again and answer it anew")
                .clicked()
            {
                w.back();
                out.changed = true;
            }
        },
    );
    if cancel {
        out.done = Some(Done::Cancel);
    }
}

/// What's open: the dot test, or the click-each-LED wizard.
enum Mode {
    Confirm(Confirm),
    Wizard(Wizard),
}

/// The wizard while it's open, with the worker holding the lights.
struct Open {
    mode: Mode,
    worker: Worker,
    /// Going away (locked, display off) cancels it.
    _away: super::away::Subscription,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' wizard, so they never touch a keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<(View, Wizard)>> = const { std::cell::RefCell::new(None) };
    /// The previews' dot test.
    pub static PREVIEW_CONFIRM: std::cell::RefCell<Option<(CheckView, Confirm)>> = const { std::cell::RefCell::new(None) };
}

/// The dot's pace the user picked (an index into [`confirm::PACES`]), kept while Foxglove runs.
static PACE: AtomicUsize = AtomicUsize::new(confirm::DEFAULT_PACE);

/// The dot run for a proposal, at the user's pace.
fn play(c: &Confirm) -> led_map::Play {
    let ms = confirm::step_ms(PACE.load(Ordering::Relaxed));
    led_map::Play {
        frames: c.frames(),
        every: std::time::Duration::from_millis(ms),
        rounds: confirm::rounds(c.steps().len(), ms),
    }
}

/// Whether the wizard can run on the connected keyboard: it takes SET_FRAME frames and has a
/// board profile (the map is the profile's). Never looks for a keyboard (the UI thread asks).
pub fn available() -> bool {
    super::devices::frames() && super::boards::current().is_some()
}

/// Opens the dot test on the connected keyboard's profile (or, when there's nothing to propose,
/// the click-each-LED wizard) and lights the keyboard.
pub fn open(ctx: &egui::Context) {
    let Some(board) = super::boards::current() else {
        return;
    };
    // One wizard at a time: an open one ends first, so its give-back comes before this take
    // (the new worker waits for the old one's thread).
    cancel();
    let wake = ctx.clone();
    let worker = Worker::spawn(Box::<led_map::AppKeyboard>::default(), move || {
        wake.request_repaint()
    });
    let mode = match Confirm::new(&board) {
        Some(c) => {
            worker.play(play(&c));
            Mode::Confirm(c)
        }
        None => {
            let w = Wizard::new(&board);
            worker.show(w.frame());
            Mode::Wizard(w)
        }
    };
    let away = super::away::subscribe(|away| {
        if away {
            // The listener list is locked here: cancel on a thread of its own.
            super::services::spawn_named("led-map-away", cancel);
        }
    });
    let what = match &mode {
        Mode::Confirm(_) => "Check LED map",
        Mode::Wizard(_) => "Find my LEDs",
    };
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Open {
        mode,
        worker,
        _away: away,
    });
    eprintln!("Foxglove: {what} started on {}", board.id);
}

/// The wizard in place of the Lights page's cards, while it's open; false when it isn't.
pub fn show(ui: &mut egui::Ui) -> bool {
    #[cfg(test)]
    if let Some((v, mut c)) = PREVIEW_CONFIRM.with_borrow(|v| v.clone()) {
        draw_check(ui, &v, &mut c);
        return true;
    }
    #[cfg(test)]
    if let Some((v, mut w)) = PREVIEW.with_borrow(|v| v.clone()) {
        draw(ui, &v, &mut w);
        return true;
    }
    let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(open) = guard.as_mut() else {
        return false;
    };
    let status = open
        .worker
        .status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let next = match &mut open.mode {
        Mode::Wizard(w) => show_wizard_ui(ui, &status, w, &open.worker).then_some(Next::Close),
        Mode::Confirm(c) => show_check_ui(ui, &status, c, &open.worker),
    };
    match next {
        None => {}
        Some(Next::Close) => *guard = None,
        Some(Next::Wizard(intro)) => {
            // The fallback: one click per LED, on the lights the test already holds.
            let board = match &open.mode {
                Mode::Confirm(c) => c.board.clone(),
                Mode::Wizard(w) => w.board.clone(),
            };
            match super::boards::current().filter(|b| b.id == board) {
                Some(b) => {
                    let mut w = Wizard::new(&b);
                    w.intro = intro;
                    open.worker.show(w.frame());
                    open.mode = Mode::Wizard(w);
                    eprintln!("Foxglove: Check LED map: clicking each LED instead");
                    ui.ctx().request_repaint();
                }
                None => *guard = None,
            }
        }
    }
    true
}

/// What the dot test asks of the page.
enum Next {
    Close,
    /// Click each LED instead, with why when none of the corrections fit.
    Wizard(Option<String>),
}

/// The click wizard's opening words when the dot test's corrections didn't fit.
pub const NONE_FIT: &str = "None of the quick fixes fit: these LEDs don't run along the keys in \
     a way Foxglove can turn round. So one lamp lights at a time instead, and you click the \
     keys it lights. About a minute; Nothing lit skips one.";

/// Draws the click-each-LED wizard and acts on it; true when it closes.
fn show_wizard_ui(ui: &mut egui::Ui, status: &Status, w: &mut Wizard, worker: &Worker) -> bool {
    let view = View::of(status, w);
    let out = draw(ui, &view, w);
    if out.changed {
        w.note = None;
        worker.show(w.frame());
    }
    match out.done {
        None => false,
        Some(Done::Cancel) => {
            eprintln!("Foxglove: Find my LEDs cancelled");
            true
        }
        Some(Done::Save) => match led_map::save(&w.board, w.result()) {
            Ok(()) => {
                eprintln!(
                    "Foxglove: Find my LEDs saved {} of {} LEDs for {}",
                    w.seen(),
                    w.total(),
                    w.board
                );
                true
            }
            Err(e) => {
                w.note = Some(format!("Can't save where the LEDs are: {e}"));
                false
            }
        },
    }
}

/// What the dot test's card shows besides the proposal itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckView {
    /// The dot's step lit on the keyboard now (the keyboard has its frame).
    pub step: Option<usize>,
    /// The keyboard has taken the lights (the first frame went out).
    pub started: bool,
    /// The dot ran all its rounds and stopped; the keyboard's own lights show.
    pub played: bool,
    pub error: Option<String>,
    /// A part that doesn't draw the dot (the other half not linked).
    pub warn: Option<String>,
    /// The dot's pace (an index into [`confirm::PACES`]).
    pub pace: usize,
}

impl CheckView {
    fn of(status: &Status) -> CheckView {
        CheckView {
            step: status.step,
            started: status.shown.is_some(),
            played: status.played,
            error: status.error.clone(),
            warn: status.warn.clone(),
            pace: PACE.load(Ordering::Relaxed),
        }
    }
}

/// What the dot test's card asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckIntent {
    /// Matches (save), or a correction (run the dot again on the new chain).
    Answer(Answer),
    Undo,
    RunAgain,
    /// Another pace (an index into [`confirm::PACES`]): the dot starts again at it.
    Pace(usize),
    /// Click each LED instead; `none_fit` when none of the corrections describe it.
    Wizard {
        none_fit: bool,
    },
    Cancel,
}

/// Paints the dot test; returns what was asked this frame.
pub fn draw_check(ui: &mut egui::Ui, v: &CheckView, c: &mut Confirm) -> Option<CheckIntent> {
    let mut out = None;
    card(ui, &title(&c.name), |ui| {
        let p = theme::palette(ui);
        let parts = Parts::halves(c.halves);
        let on = parts.all();
        let intro = match c.source {
            Source::Profile { checked: false } => format!(
                "Foxglove ships an LED map for the {}, not yet checked on a real one. A white dot \
                 runs along the LEDs on {on}; the drawing shows where Foxglove thinks it is.",
                c.name
            ),
            Source::Profile { checked: true } => format!(
                "The {}'s LED map is checked. A white dot runs along the LEDs on {on}, with \
                 the drawing alongside.",
                c.name
            ),
            Source::Found => {
                "Your map from clicking each LED. A white dot runs along the LEDs; the \
                 drawing shows where you placed each one."
                    .to_string()
            }
            Source::Guess => format!(
                "Foxglove guessed how the {}'s LEDs are chained: down and up each column, then \
                 the thumbs. A white dot runs along them on {on}; the drawing shows the \
                 guess.",
                c.name
            ),
        };
        ui.add(egui::Label::new(intro).wrap());
        let ask = match confirm::fixes_text(&c.fixes, c.halves) {
            Some(fixed) => format!("{fixed}. Watch again: does it match now?"),
            None => "Does the dot on the keyboard follow the one on the drawing?".to_string(),
        };
        ui.add(egui::Label::new(RichText::new(ask).color(p.text_strong)).wrap());
        if let Some(e) = &v.error {
            ui.add(egui::Label::new(RichText::new(e).color(p.warn)).wrap());
        } else if v.played {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "The dot stopped and the keyboard's own lights are back. Run again to \
                         watch once more.",
                    )
                    .color(p.muted),
                )
                .wrap(),
            );
        } else if !v.started {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Lighting it…").color(p.muted));
            });
        }
        if let Some(w) = v.warn.as_ref().filter(|_| v.error.is_none()) {
            ui.add(egui::Label::new(RichText::new(w).color(p.warn)).wrap());
        }
        ui.add_space(4.0);
        check_board(ui, v, c);
        ui.add_space(6.0);
        let ready = v.started && v.error.is_none();
        if ui
            .add_enabled(ready, theme::primary_button(ui, "Yes, it matches"))
            .on_hover_text("Keep this map; scenes light each LED in its place")
            .on_disabled_hover_text("Wait for the dot on the keyboard")
            .clicked()
        {
            out = Some(CheckIntent::Answer(Answer::Matches));
        }
        ui.add_space(6.0);
        if let Some(asked) = mismatch(ui, c) {
            out = Some(asked);
        }
        ui.add_space(6.0);
        if let Some(asked) = pace(ui, v) {
            out = Some(asked);
        }
        ui.add_space(4.0);
        let mut cancel = false;
        split_row(
            ui,
            |ui| {
                cancel = ui
                    .button("Cancel")
                    .on_hover_text("Stop without saving; the lights go back as they were")
                    .clicked();
            },
            |ui| {
                if !c.fixes.is_empty()
                    && ui
                        .button("Undo")
                        .on_hover_text("Take the last answer back")
                        .clicked()
                {
                    out = Some(CheckIntent::Undo);
                }
            },
        );
        if cancel {
            out = Some(CheckIntent::Cancel);
        }
        if let Some(note) = &c.note {
            ui.add(egui::Label::new(RichText::new(note).color(p.warn)).wrap());
        }
    });
    out
}

/// "It doesn't match": how the keyboard's dot differs (for which half), or none of those, which
/// goes to clicking each LED. A found map has no corrections: clicking again is the one way.
fn mismatch(ui: &mut egui::Ui, c: &mut Confirm) -> Option<CheckIntent> {
    let p = theme::palette(ui);
    let mut out = None;
    if !c.correctable() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Doesn't match?").color(p.text_strong));
            if ui
                .button("Check each lamp…")
                .on_hover_text("Light one lamp at a time and click the keys it lights")
                .clicked()
            {
                out = Some(CheckIntent::Wizard { none_fit: false });
            }
        });
        return out;
    }
    let parts = Parts::halves(c.halves);
    ui.add(
        egui::Label::new(
            RichText::new("Doesn't match? Say how the keyboard's dot differs:")
                .color(p.text_strong),
        )
        .wrap(),
    );
    if parts.is_split() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("For").color(p.muted));
            ui.selectable_value(&mut c.half, None, upper_first(&parts.all()))
                .on_hover_text("Both dots run the same way, and both differ the same way");
            for h in 0..parts.len() {
                ui.selectable_value(&mut c.half, Some(h), parts.name(h))
                    .on_hover_text(format!(
                        "The two dots run differently: correct {} on its own",
                        parts.the(h)
                    ));
            }
        });
    }
    let each = if parts.is_split() && c.half.is_none() {
        format!(" of each {}", parts.noun())
    } else {
        String::new()
    };
    ui.horizontal_wrapped(|ui| {
        for (label, a, hint) in [
            (
                "Runs backwards",
                Answer::Backwards,
                "Same path, the other way round".to_string(),
            ),
            (
                "Mirrored",
                Answer::Mirrored,
                format!("It starts on the other side{each}: the outer and inner columns swapped"),
            ),
            (
                "Rows flipped",
                Answer::RowsFlipped,
                "It goes up where the drawing goes down".to_string(),
            ),
        ] {
            if ui.button(label).on_hover_text(hint).clicked() {
                out = Some(CheckIntent::Answer(a));
            }
        }
        if ui
            .button("None of these…")
            .on_hover_text("Light one lamp at a time and click the keys it lights: about a minute")
            .clicked()
        {
            out = Some(CheckIntent::Wizard { none_fit: true });
        }
    });
    if c.fixes.len() >= 3 {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Still not right after three answers? None of these… lights one lamp at \
                     a time instead.",
                )
                .color(p.muted),
            )
            .wrap(),
        );
    }
    out
}

/// The dot's speed and Run again.
fn pace(ui: &mut egui::Ui, v: &CheckView) -> Option<CheckIntent> {
    let p = theme::palette(ui);
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Dot speed").color(p.muted));
        for (i, (name, ms)) in confirm::PACES.iter().enumerate() {
            let hint = format!("One LED every {:.1} s", *ms as f32 / 1000.0);
            if ui
                .selectable_label(v.pace == i, *name)
                .on_hover_text(hint)
                .clicked()
                && v.pace != i
            {
                out = Some(CheckIntent::Pace(i));
            }
        }
        if ui
            .button("Run again")
            .on_hover_text("Start the dot from the first LED")
            .clicked()
        {
            out = Some(CheckIntent::RunAgain);
        }
    });
    out
}

/// The board with the proposed chain: each half's keys, the path the dot should take (from a
/// ringed first LED), and the dot where the keyboard has it now.
fn check_board(ui: &mut egui::Ui, v: &CheckView, c: &Confirm) {
    let p = theme::palette(ui);
    let size = Vec2::new(ui.available_width(), DRAWING_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, p.well);
    let g = Geometry::new(&c.layout, c.halves, rect.shrink(12.0));
    let areas = usize::from(c.halves.clamp(1, 2));
    for a in 0..areas {
        for k in g.keys(&c.layout, a) {
            painter.add(g.key_shape(a, k, Color32::TRANSPARENT, Stroke::new(1.0, p.line)));
        }
    }
    // The chain's points are in the profile's key units; they land on the same keys here (#796).
    let screen = |a: usize, at: [f32; 2]| g.screen(a, c.layout.point_from_profile(a, at));
    for h in 0..usize::from(c.halves) {
        let a = h.min(areas - 1);
        let points: Vec<Pos2> = (0..c.per_half)
            .filter_map(|i| c.point(h, i))
            .map(|at| screen(a, at))
            .collect();
        painter.add(egui::Shape::line(points.clone(), Stroke::new(1.5, p.muted)));
        for pt in &points {
            painter.circle_filled(*pt, 2.5, p.muted);
        }
        if let Some(first) = c.point(h, 0) {
            painter.circle_stroke(screen(a, first), 7.0, Stroke::new(1.5, p.accent));
        }
        if let Some(at) = v.step.and_then(|i| c.point(h, c.lamp_at(i)?)) {
            let at = screen(a, at);
            painter.circle_filled(at, 8.0, p.accent.gamma_multiply(0.35));
            painter.circle(at, 5.0, Color32::WHITE, Stroke::new(1.5, p.accent));
        }
    }
}

/// Draws the dot test and acts on it: a correction, another pace or Run again restarts the
/// dot, Matches saves. What the page does next, if anything.
fn show_check_ui(
    ui: &mut egui::Ui,
    status: &Status,
    c: &mut Confirm,
    worker: &Worker,
) -> Option<Next> {
    let v = CheckView::of(status);
    match draw_check(ui, &v, c)? {
        CheckIntent::Answer(Answer::Matches) => match led_map::save(&c.board, c.result()) {
            Ok(()) => {
                let fixed =
                    confirm::fixes_text(&c.fixes, c.halves).unwrap_or_else(|| "as proposed".into());
                eprintln!("Foxglove: Check LED map: {} matches ({fixed})", c.board);
                Some(Next::Close)
            }
            Err(e) => {
                c.note = Some(format!("Can't save the LED map: {e}"));
                None
            }
        },
        CheckIntent::Answer(a) => {
            c.answer(a);
            c.note = None;
            worker.play(play(c));
            None
        }
        CheckIntent::Undo => {
            c.undo();
            worker.play(play(c));
            None
        }
        CheckIntent::RunAgain => {
            worker.play(play(c));
            None
        }
        CheckIntent::Pace(i) => {
            PACE.store(i.min(confirm::PACES.len() - 1), Ordering::Relaxed);
            worker.play(play(c));
            None
        }
        CheckIntent::Wizard { none_fit } => {
            if none_fit {
                eprintln!(
                    "Foxglove: Check LED map: none of the corrections fit ({} so far)",
                    c.fixes.len()
                );
            }
            Some(Next::Wizard(none_fit.then(|| NONE_FIT.to_string())))
        }
        CheckIntent::Cancel => {
            eprintln!("Foxglove: Check LED map cancelled");
            Some(Next::Close)
        }
    }
}

/// Ends an open wizard as Cancel does (the Lights page was left, the window closed, Foxglove
/// paused or went away): nothing is saved and the lights go back. Doesn't wait for them.
pub fn cancel() {
    let open = OPEN.lock().unwrap_or_else(|e| e.into_inner()).take();
    if open.is_some() {
        eprintln!("Foxglove: Find my LEDs cancelled");
    }
}

/// Foxglove quits: cancels, and waits until the lights are back.
pub fn quit() {
    cancel();
    led_map::wait_for_last();
}

/// Foxglove paused (#404): the wizard ends, since it may not drive the lights.
pub fn pause_changed(paused: bool) {
    if paused {
        cancel();
    }
}

/// The Keyboard card's entry for a keyboard without a zone line, in the card's header row
/// (#794 K32): how far the map is known as a chip, and "Check zones…" beside it. True when
/// clicked.
pub fn entry(ui: &mut egui::Ui, available: bool, state: MapState) -> bool {
    let (text, dot) = match state {
        MapState::Checked => ("LED map: checked", theme::Dot::Ok),
        MapState::Shipped => ("LED map: shipped, not checked yet", theme::Dot::Muted),
        MapState::Unknown => ("LED positions: not placed yet", theme::Dot::Muted),
    };
    theme::hint_chip(ui, text, None, Some(dot));
    let hint = match (available, state) {
        (false, _) => "Needs keyboard firmware that lights one LED at a time, and a board profile",
        (true, MapState::Unknown) => {
            "Show Foxglove where each LED is, so scenes light every LED in its place"
        }
        (true, _) => "A dot runs along the LEDs; say whether it follows the drawing",
    };
    ui.add_enabled(available, egui::Link::new(super::zones_ui::CHECK))
        .on_hover_text(hint)
        .on_disabled_hover_text(hint)
        .clicked()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::foxglove::boards::Board;
    use crate::foxglove::led_map_confirm::Answer;

    /// A Corne with the built-in profile, `answered` lamps in: every third lit nothing, the
    /// others lit a key (every other one two keys), as Check zones answers them.
    pub fn sample(answered: usize) -> (View, Wizard) {
        let mut board: Board =
            Board::parse(include_str!("../../resources/boards/eyelash-corne.toml")).unwrap();
        board.lighting.as_mut().unwrap().led_points.clear();
        let mut w = Wizard::new(&board);
        let keys = w.drawing.keys(false).len();
        for i in 0..answered {
            let k = (i % keys) as u8;
            if i % 3 == 2 {
                w.skip();
            } else {
                w.toggle(k);
                if i % 3 == 0 {
                    w.toggle((k + 1) % keys as u8);
                }
                w.next();
            }
        }
        (
            View {
                lit: true,
                ..Default::default()
            },
            w,
        )
    }

    /// The dot test on the Corne's shipped map, the dot at `step`, after `fixes`.
    pub fn check_sample(step: Option<usize>, fixes: &[Answer]) -> (CheckView, Confirm) {
        let board: Board =
            Board::parse(include_str!("../../resources/boards/eyelash-corne.toml")).unwrap();
        let mut c = Confirm::new(&board).unwrap();
        for a in fixes {
            c.answer(*a);
        }
        (
            CheckView {
                step,
                started: true,
                pace: confirm::DEFAULT_PACE,
                ..Default::default()
            },
            c,
        )
    }

    /// Draws the dot test for one frame with `events`.
    fn check_frame(
        ctx: &egui::Context,
        v: &CheckView,
        c: &mut Confirm,
        events: Vec<egui::Event>,
    ) -> Option<CheckIntent> {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            events,
            ..Default::default()
        };
        let mut out = None;
        let mut full = ctx.run_ui(raw, |ui| out = draw_check(ui, v, c));
        full.textures_delta.clear();
        SHAPES.set(full.shapes);
        out
    }

    #[test]
    fn the_dot_test_answers_and_falls_back_to_clicking() {
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
        let (v, mut c) = check_sample(Some(3), &[]);
        check_frame(&ctx, &v, &mut c, Vec::new());
        check_frame(&ctx, &v, &mut c, Vec::new());
        let press = |ctx: &egui::Context, c: &mut Confirm, label: &str| {
            let at = button_at(ctx, label);
            let first = check_frame(ctx, &v, c, click(at));
            first.or(check_frame(ctx, &v, c, Vec::new()))
        };
        for (label, want) in [
            ("Yes, it matches", CheckIntent::Answer(Answer::Matches)),
            ("Runs backwards", CheckIntent::Answer(Answer::Backwards)),
            ("Mirrored", CheckIntent::Answer(Answer::Mirrored)),
            ("Rows flipped", CheckIntent::Answer(Answer::RowsFlipped)),
            ("None of these…", CheckIntent::Wizard { none_fit: true }),
            ("Slower", CheckIntent::Pace(0)),
            ("Faster", CheckIntent::Pace(2)),
            ("Run again", CheckIntent::RunAgain),
            ("Cancel", CheckIntent::Cancel),
        ] {
            assert_eq!(press(&ctx, &mut c, label), Some(want), "{label}");
        }
        // The pace showing now is no change.
        assert_eq!(press(&ctx, &mut c, "Normal"), None);
        // Undo shows once there's a correction to take back.
        c.answer(Answer::Mirrored);
        check_frame(&ctx, &v, &mut c, Vec::new());
        assert_eq!(press(&ctx, &mut c, "Undo"), Some(CheckIntent::Undo));
        // A split board's answers can be for one half: picking it is no answer yet.
        assert_eq!(press(&ctx, &mut c, "Right half"), None);
        assert_eq!(c.half, Some(1));
        assert_eq!(
            press(&ctx, &mut c, "Rows flipped"),
            Some(CheckIntent::Answer(Answer::RowsFlipped))
        );
        // A found map has no corrections: clicking again is the way.
        let mut board: Board =
            Board::parse(include_str!("../../resources/boards/eyelash-corne.toml")).unwrap();
        board.lighting.as_mut().unwrap().led_points = vec![Some([1.0, 1.0]); 42];
        let mut found = Confirm::new(&board).unwrap();
        check_frame(&ctx, &v, &mut found, Vec::new());
        check_frame(&ctx, &v, &mut found, Vec::new());
        assert_eq!(
            press(&ctx, &mut found, "Check each lamp…"),
            Some(CheckIntent::Wizard { none_fit: false })
        );
    }

    /// The second desk's one-piece board: no half to pick, and the corrections and the dot run
    /// on its whole drawing.
    #[test]
    fn the_dot_test_on_a_one_piece_board_has_no_halves() {
        use crate::foxglove::fixtures::second_desk;
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
        let mut c = Confirm::new(&second_desk::board()).unwrap();
        let v = CheckView {
            step: Some(5),
            started: true,
            ..Default::default()
        };
        check_frame(&ctx, &v, &mut c, Vec::new());
        check_frame(&ctx, &v, &mut c, Vec::new());
        let has = |label: &str| {
            SHAPES.with_borrow(|shapes| {
                fn find(shape: &egui::Shape, label: &str) -> bool {
                    match shape {
                        egui::Shape::Vec(v) => v.iter().any(|s| find(s, label)),
                        egui::Shape::Text(t) => t.galley.job.text == label,
                        _ => false,
                    }
                }
                shapes.iter().any(|s| find(&s.shape, label))
            })
        };
        assert!(has("Mirrored") && has("None of these…"));
        assert!(!has("Left half") && !has("Right half") && !has("For"));
    }

    #[test]
    fn the_geometry_maps_clicks_on_the_lit_half_only() {
        let (_, w) = sample(0);
        let g = Geometry::new(
            &w.layout,
            2,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 230.0)),
        );
        let (left, right) = (g.rect(0), g.rect(1));
        assert!(left.max.x < right.min.x, "the halves sit apart");
        // A key's centre comes back as itself; the gap between the halves is no one's.
        let k = w.drawing.keys(false)[4];
        let at = g.at(0, g.screen(0, [k.center().x, k.center().y])).unwrap();
        assert!((at[0] - k.center().x).abs() < 1e-3 && (at[1] - k.center().y).abs() < 1e-3);
        let gap = Pos2::new((left.max.x + right.min.x) / 2.0, left.center().y);
        assert!(g.at(0, gap).is_none() && g.at(1, gap).is_none());
        // Just outside the outline still counts (an LED on the edge), clamped to the margin.
        let edge = g.at(0, left.left_center() - Vec2::X * 3.0).unwrap();
        assert!(edge[0] < 0.0 && edge[0] >= -MARGIN);
        // A one-piece board is one area, its whole drawing (#559).
        let one = Geometry::new(&w.layout, 1, g.rect(0).union(g.rect(1)));
        assert_eq!(one.keys(&w.layout, 0), w.layout.part(0));
        assert_eq!(
            w.layout.rects(0),
            w.drawing.keys(false),
            "the profile's keys (#796)"
        );
    }

    /// Draws the wizard on `ctx` for one frame with `events`; what it asked, and the drawing's
    /// rectangle (the click-sensing widget of its height).
    fn frame(
        ctx: &egui::Context,
        v: &View,
        w: &mut Wizard,
        events: Vec<egui::Event>,
    ) -> (Output, Option<Rect>) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            events,
            ..Default::default()
        };
        let mut out = Output::default();
        let mut full = ctx.run_ui(raw, |ui| out = draw(ui, v, w));
        full.textures_delta.clear();
        SHAPES.set(full.shapes);
        let drawing = ctx.viewport(|vp| {
            vp.prev_pass
                .widgets
                .layers()
                .flat_map(|(_, ws)| ws.iter())
                .find(|r| (r.rect.height() - DRAWING_HEIGHT).abs() < 0.5)
                .map(|r| r.rect)
        });
        (out, drawing)
    }

    fn click(pos: Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    thread_local! {
        /// The last test frame's shapes, for finding a button by its text.
        static SHAPES: std::cell::RefCell<Vec<egui::epaint::ClippedShape>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Where the button labelled `label` was drawn in the last frame.
    fn button_at(_ctx: &egui::Context, label: &str) -> Pos2 {
        fn find(shape: &egui::Shape, label: &str) -> Option<Pos2> {
            match shape {
                egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, label)),
                egui::Shape::Text(t) if t.galley.job.text == label => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                _ => None,
            }
        }
        SHAPES
            .with_borrow(|shapes| shapes.iter().find_map(|s| find(&s.shape, label)))
            .unwrap_or_else(|| panic!("no {label} button"))
    }

    /// Clicks at `pos` (after a frame that lays the card out), and lets the click land.
    fn clicked(ctx: &egui::Context, v: &View, w: &mut Wizard, pos: Pos2) -> Output {
        let (out, _) = frame(ctx, v, w, click(pos));
        let (after, _) = frame(ctx, v, w, Vec::new());
        Output {
            changed: out.changed || after.changed,
            done: out.done.or(after.done),
        }
    }

    /// Check zones' main step (#611, docs/lighting-zones.md section 4): a click on a key picks
    /// it, a second click unpicks it, Next takes the keys; a click between keys places the
    /// lamp there; Nothing lit marks a dead lamp, Back undoes.
    #[test]
    fn keys_that_lit_are_picked_then_next_and_the_buttons_skip_and_undo() {
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
        let (v, mut w) = sample(0);
        frame(&ctx, &v, &mut w, Vec::new());
        let (_, drawing) = frame(&ctx, &v, &mut w, Vec::new());
        let text = SHAPES.with_borrow(|shapes| {
            let mut t = String::new();
            for s in shapes {
                crate::foxglove::zones_ui::tests::collect(&s.shape, &mut t);
            }
            t
        });
        for want in [
            "Check zones · Eyelash Corne",
            "Lamp 1 of 21 on the left half is lit in white on your keyboard.",
            ASK,
            "Next",
            "Nothing lit",
            "Back",
            "Cancel",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        let g = Geometry::new(&w.layout, 2, drawing.expect("the drawing").shrink(12.0));
        let k = w.drawing.keys(false)[4].center();
        let on_key = g.screen(0, [k.x, k.y]);
        clicked(&ctx, &v, &mut w, on_key);
        assert_eq!(w.picked, [4]);
        clicked(&ctx, &v, &mut w, on_key);
        assert!(w.picked.is_empty(), "a second click unpicks");
        clicked(&ctx, &v, &mut w, on_key);
        let k5 = w.drawing.keys(false)[5].center();
        clicked(&ctx, &v, &mut w, g.screen(0, [k5.x, k5.y]));
        assert_eq!(w.picked, [4, 5]);
        // A click on the other half isn't where the lit lamp is.
        clicked(&ctx, &v, &mut w, g.screen(1, [1.5, 1.5]));
        assert_eq!(w.picked, [4, 5]);
        let next = button_at(&ctx, "Next");
        assert!(clicked(&ctx, &v, &mut w, next).changed);
        assert_eq!(w.keys, vec![vec![4, 5]]);
        let at = w.answers[0].unwrap();
        assert!((at[1] - (k.y + k5.y) / 2.0).abs() < 0.01, "{at:?}");
        // Between keys (underglow): a point, no keys.
        clicked(&ctx, &v, &mut w, g.screen(0, [-0.2, 0.1]));
        assert!(w.point.is_some() && w.picked.is_empty());
        let next = button_at(&ctx, "Next");
        clicked(&ctx, &v, &mut w, next);
        assert_eq!(w.keys, vec![vec![4, 5], vec![]]);
        assert!(w.answers[1].is_some());
        // "Nothing lit" skips, Back undoes.
        let skip = button_at(&ctx, "Nothing lit");
        assert!(clicked(&ctx, &v, &mut w, skip).changed);
        assert_eq!(w.answers.len(), 3);
        assert_eq!(w.answers[2], None);
        let back = button_at(&ctx, "Back");
        assert!(clicked(&ctx, &v, &mut w, back).changed);
        assert_eq!((w.answers.len(), w.keys.len()), (2, 2));
        let cancel = button_at(&ctx, "Cancel");
        assert_eq!(clicked(&ctx, &v, &mut w, cancel).done, Some(Done::Cancel));
    }

    #[test]
    fn the_last_led_offers_save_and_counts_what_was_not_seen() {
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
        let (v, mut w) = sample(42);
        assert!(w.done());
        assert_eq!(w.seen(), 28);
        assert_eq!(w.result().leds.len(), 28);
        // What each lamp lit, in the profile's shape: one entry per lamp, `[]` for nothing.
        let saved = w.result().led_keys;
        assert_eq!(saved.len(), 42);
        assert_eq!(saved[0].keys(), [0, 1]);
        assert_eq!(saved[1].keys(), [1]);
        assert!(saved[2].dead());
        frame(&ctx, &v, &mut w, Vec::new());
        frame(&ctx, &v, &mut w, Vec::new());
        let save = button_at(&ctx, "Save");
        assert_eq!(clicked(&ctx, &v, &mut w, save).done, Some(Done::Save));
        assert_eq!(w.answers.len(), 42, "nothing more answered");
    }

    #[test]
    fn the_led_shows_as_lit_once_the_keyboard_has_its_frame() {
        let (_, mut w) = sample(3);
        let mut status = Status::default();
        assert!(!View::of(&status, &w).lit);
        status.shown = Some(w.frame());
        assert!(View::of(&status, &w).lit);
        w.skip();
        assert!(!View::of(&status, &w).lit, "the next LED isn't lit yet");
    }
}
