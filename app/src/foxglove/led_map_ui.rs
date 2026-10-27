//! "Find my LEDs…" (#90 row G): Lights › Keyboard card, and the Scenes page's prompt while the
//! keyboard's LEDs aren't placed. One LED lights at a time; one click on the drawing places it.
//!
//! The pattern is Philips Hue's entertainment-area setup and Nanoleaf's layout assistant: the
//! light you're asked about blinks in the room, you show the app where it is. Here it's one
//! click per LED anywhere on the lit half's outline (underglow sits under and between keys),
//! "Can't see it" to skip, Back to undo, and a count of how far along it is.
//!
//! While open it replaces the Lights page's cards. Leaving the page, closing the window,
//! quitting, pausing or going away (locked, display off) cancels it, which gives the lights
//! back. [`draw`] paints a [`Wizard`] and says what was asked, so the previews and tests need no
//! keyboard.

use std::sync::Mutex;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::boards::Drawing;
use super::led_map::{self, Status, Wizard, Worker};
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
}

impl View {
    fn of(status: &Status, w: &Wizard) -> View {
        View {
            lit: status.shown.as_ref() == Some(&w.frame()),
            error: status.error.clone(),
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

/// Where the drawing puts the board: the halves side by side (a one-piece board is one area,
/// both halves joined), scaled to fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    origin: Pos2,
    /// Points per key unit.
    scale: f32,
    /// One area's size, in key units.
    area: Vec2,
    areas: usize,
}

impl Geometry {
    pub fn new(drawing: &Drawing, halves: u8, within: Rect) -> Geometry {
        let (w, h) = drawing.size();
        let (area, areas) = if halves == 1 {
            (Vec2::new(w * 2.0, h), 1)
        } else {
            (Vec2::new(w, h), 2)
        };
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

    /// Area `a`'s keys, in key units from its corner.
    pub fn keys(&self, drawing: &Drawing, a: usize) -> Vec<Rect> {
        if self.areas == 1 {
            let (w, _) = drawing.size();
            let mut keys = drawing.keys(false);
            keys.extend(
                drawing
                    .keys(true)
                    .into_iter()
                    .map(|k| k.translate(Vec2::X * w)),
            );
            return keys;
        }
        drawing.keys(a == 1)
    }
}

/// What each half is called in the steps.
fn half_name(w: &Wizard, half: usize) -> &'static str {
    match (w.halves, half) {
        (1, _) => "keyboard",
        (_, 0) => "left half",
        _ => "right half",
    }
}

/// Paints the wizard; returns what was asked this frame.
pub fn draw(ui: &mut egui::Ui, v: &View, w: &mut Wizard) -> Output {
    let mut out = Output::default();
    card(ui, "Find my LEDs", |ui| {
        let p = theme::palette(ui);
        let total = w.total();
        let answered = w.answers.len();
        match w.step() {
            Some(s) => {
                split_row(
                    ui,
                    |ui| {
                        let skipped = answered - w.seen();
                        if skipped > 0 {
                            ui.label(RichText::new(format!("{skipped} not seen")).color(p.muted));
                        }
                    },
                    |ui| {
                        ui.label(
                            RichText::new(format!(
                                "LED {} of {total} · {}",
                                s.lamp + 1,
                                half_name(w, s.half)
                            ))
                            .color(p.text_strong),
                        );
                    },
                );
                progress(ui, answered, total);
                let text = format!(
                    "One LED on the {} is lit white. Click where you see it on the drawing, \
                     under a key or between keys.",
                    half_name(w, s.half)
                );
                ui.add(egui::Label::new(text).wrap());
            }
            None => {
                ui.label(RichText::new(format!("All {total} LEDs done")).color(p.text_strong));
                progress(ui, answered, total);
                let text = match total - w.seen() {
                    0 => "Save, and scenes light each LED where it is.".to_string(),
                    n => format!(
                        "{} placed, {n} not seen: those take the colour of their half's middle. \
                         Save, and scenes light each LED where it is.",
                        w.seen()
                    ),
                };
                ui.add(egui::Label::new(text).wrap());
            }
        }
        status_line(ui, v, w);
        ui.add_space(4.0);
        board(ui, w, &mut out);
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

/// Whether the LED is lit yet, or what went wrong.
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
}

/// The board, top-down: the halves' keys, the LEDs placed so far, and the click area on the lit
/// half.
fn board(ui: &mut egui::Ui, w: &mut Wizard, out: &mut Output) {
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
    let g = Geometry::new(&w.drawing, w.halves, rect.shrink(12.0));
    let active = step.map(|s| s.half);
    let areas = usize::from(w.halves.clamp(1, 2));
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
        for k in g.keys(&w.drawing, a) {
            let r = Rect::from_min_max(
                g.screen(a, [k.min.x, k.min.y]),
                g.screen(a, [k.max.x, k.max.y]),
            )
            .shrink(g.scale * 0.06);
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, key_line), egui::StrokeKind::Inside);
        }
    }
    // The LEDs placed so far, the newest ringed.
    let newest = w.answers.iter().rposition(Option::is_some);
    for (i, a) in w.answers.iter().enumerate() {
        let Some(at) = a else { continue };
        let half = i / w.per_half.max(1);
        let c = g.screen(half.min(areas - 1), *at);
        painter.circle_filled(c, 4.0, p.accent);
        if Some(i) == newest && step.is_some() {
            painter.circle_stroke(c, 7.0, Stroke::new(1.0, p.accent));
        }
    }
    let Some(s) = step else {
        return;
    };
    let half = s.half.min(areas - 1);
    let hover = response.hover_pos().and_then(|pos| g.at(half, pos));
    if let Some(at) = hover {
        painter.circle(
            g.screen(half, at),
            6.0,
            Color32::WHITE,
            Stroke::new(1.0, p.accent),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if response.clicked() {
        if let Some(at) = response
            .interact_pointer_pos()
            .and_then(|pos| g.at(half, pos))
        {
            w.place(at);
            out.changed = true;
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
                    .on_hover_text("Keep where the LEDs are; scenes use it from now on")
                    .clicked()
                {
                    out.done = Some(Done::Save);
                }
            } else if ui
                .button("Can't see it")
                .on_hover_text("Skip this LED (hidden, or not fitted)")
                .clicked()
            {
                w.skip();
                out.changed = true;
            }
            if ui
                .add_enabled(!w.answers.is_empty(), egui::Button::new("Back"))
                .on_hover_text("Light the LED before again and place it anew")
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

/// The wizard while it's open, with the worker holding the lights.
struct Open {
    wizard: Wizard,
    worker: Worker,
    /// Going away (locked, display off) cancels it.
    _away: super::away::Subscription,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' wizard, so they never touch a keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<(View, Wizard)>> = const { std::cell::RefCell::new(None) };
}

/// Whether the wizard can run on the connected keyboard: it takes SET_FRAME frames and has a
/// board profile (the map is the profile's). Never looks for a keyboard (the UI thread asks).
pub fn available() -> bool {
    super::devices::frames() && super::boards::current().is_some()
}

/// Opens the wizard on the connected keyboard's profile and lights its first LED.
pub fn open(ctx: &egui::Context) {
    let Some(board) = super::boards::current() else {
        return;
    };
    // One wizard at a time: an open one ends first, so its give-back comes before this take
    // (the new worker waits for the old one's thread).
    cancel();
    let wizard = Wizard::new(&board);
    let wake = ctx.clone();
    let worker = Worker::spawn(Box::<led_map::AppKeyboard>::default(), move || {
        wake.request_repaint()
    });
    worker.show(wizard.frame());
    let away = super::away::subscribe(|away| {
        if away {
            // The listener list is locked here: cancel on a thread of its own.
            super::services::spawn_named("led-map-away", cancel);
        }
    });
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Open {
        wizard,
        worker,
        _away: away,
    });
    eprintln!("Foxglove: Find my LEDs started on {}", board.id);
}

/// The wizard in place of the Lights page's cards, while it's open; false when it isn't.
pub fn show(ui: &mut egui::Ui) -> bool {
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
    let view = View::of(&status, &open.wizard);
    let out = draw(ui, &view, &mut open.wizard);
    if out.changed {
        open.wizard.note = None;
        open.worker.show(open.wizard.frame());
    }
    match out.done {
        None => {}
        Some(Done::Cancel) => {
            eprintln!("Foxglove: Find my LEDs cancelled");
            *guard = None;
        }
        Some(Done::Save) => {
            let w = &open.wizard;
            match led_map::save(&w.board, w.result()) {
                Ok(()) => {
                    eprintln!(
                        "Foxglove: Find my LEDs saved {} of {} LEDs for {}",
                        w.seen(),
                        w.total(),
                        w.board
                    );
                    *guard = None;
                }
                Err(e) => open.wizard.note = Some(format!("Can't save where the LEDs are: {e}")),
            }
        }
    }
    true
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

/// The Lights page's Keyboard card: "Find my LEDs…" and whether they're placed. True when
/// clicked.
pub fn entry(ui: &mut egui::Ui, available: bool, placed: bool) -> bool {
    let p = theme::palette(ui);
    let mut clicked = false;
    split_row(
        ui,
        |ui| {
            let hint = if available {
                "Light each LED in turn and click where it is, so scenes light every LED in its place"
            } else {
                "Needs keyboard firmware that lights one LED at a time, and a board profile"
            };
            clicked = ui
                .add_enabled(available, egui::Button::new("Find my LEDs…"))
                .on_hover_text(hint)
                .on_disabled_hover_text(hint)
                .clicked();
        },
        |ui| {
            let (text, color) = if placed {
                ("LED positions: placed", p.ok)
            } else {
                ("LED positions: not placed yet", p.muted)
            };
            ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
        },
    );
    clicked
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::foxglove::boards::Board;

    /// A Corne with the built-in profile, `answered` LEDs in (every third one not seen).
    pub fn sample(answered: usize) -> (View, Wizard) {
        let mut board: Board =
            Board::parse(include_str!("../../resources/boards/eyelash-corne.toml")).unwrap();
        board.lighting.as_mut().unwrap().led_points.clear();
        let mut w = Wizard::new(&board);
        let keys_l = w.drawing.keys(false);
        let keys_r = w.drawing.keys(true);
        for i in 0..answered {
            let half = i / w.per_half;
            let keys = if half == 0 { &keys_l } else { &keys_r };
            let k = keys[i % keys.len()];
            if i % 3 == 2 {
                w.skip();
            } else {
                w.place([k.center().x, k.center().y + 0.4]);
            }
        }
        (
            View {
                lit: true,
                error: None,
            },
            w,
        )
    }

    #[test]
    fn the_geometry_maps_clicks_on_the_lit_half_only() {
        let (_, w) = sample(0);
        let g = Geometry::new(
            &w.drawing,
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
        // A one-piece board is one area, both halves' keys.
        let one = Geometry::new(&w.drawing, 1, g.rect(0).union(g.rect(1)));
        assert_eq!(one.keys(&w.drawing, 0).len(), 42);
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

    #[test]
    fn a_click_places_the_lit_led_and_the_buttons_skip_and_undo() {
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
        let (v, mut w) = sample(0);
        frame(&ctx, &v, &mut w, Vec::new());
        let (_, drawing) = frame(&ctx, &v, &mut w, Vec::new());
        let g = Geometry::new(&w.drawing, 2, drawing.expect("the drawing").shrink(12.0));
        let out = clicked(&ctx, &v, &mut w, g.screen(0, [1.5, 1.5]));
        assert!(out.changed);
        assert_eq!(w.answers.len(), 1);
        let at = w.answers[0].unwrap();
        assert!(
            (at[0] - 1.5).abs() < 0.05 && (at[1] - 1.5).abs() < 0.05,
            "{at:?}"
        );
        // A click on the other half isn't where the lit LED is.
        let out = clicked(&ctx, &v, &mut w, g.screen(1, [1.5, 1.5]));
        assert!(!out.changed);
        assert_eq!(w.answers.len(), 1);
        // "Can't see it" skips, Back undoes.
        let skip = button_at(&ctx, "Can't see it");
        assert!(clicked(&ctx, &v, &mut w, skip).changed);
        assert_eq!(w.answers, vec![Some(at), None]);
        let back = button_at(&ctx, "Back");
        assert!(clicked(&ctx, &v, &mut w, back).changed);
        assert_eq!(w.answers, vec![Some(at)]);
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
