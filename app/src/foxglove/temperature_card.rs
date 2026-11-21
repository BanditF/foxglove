//! Temperature on the Scenes page (the plan: review/ux-plan-gpu-temp-effect.md, slice 3): the
//! words for every state, what refuses a start, and the controls on the Effect card (Reads,
//! Colours, Cool at / Hot at, Back to defaults, the Cool-to-hot scale).
//!
//! Words and rules are plain functions of the look and a [`TempView`], so tests and the review
//! images draw them with no device and no hub. Nothing here reads a sensor or sends a colour:
//! the reading comes from [`super::temperature_feed`], the light output is [`super::temperature_out`].
//!
//! The plan's never-shown list holds: no `sensor.*` keys, "fact", "subscribe", "demand",
//! "module", "plugin", "NVML", "GPU", "CPU", "unavailable", "unknown", and never "0 °C" for a
//! missing reading (a missing reading is [`temp::summary`]'s `None`, so it cannot be written).

use egui::{Align2, Color32, RichText, Sense, Stroke};
use foxglove_scene::Rgb;

use super::announce;
use super::scene_looks::Look;
use super::scenes_ui::{strip, Tone};
use super::temperature::{self as temp, Colours};
use super::temperature_feed::Status;
use super::theme;

/// The page's status line while the effect is picked and not running.
pub const NOT_RUNNING: &str = "Not running. The scale shows the last reading once it starts.";
/// Start refused: the PC sensors app is off or not installed.
pub const NEEDS_APP: &str =
    "Temperature needs the PC sensors app. Turn it on in Hideout's Apps page.";
/// Start refused: Foxglove has no permission to read the PC's readings.
pub const NOT_ALLOWED: &str =
    "Foxglove isn't allowed to see your PC's readings. Allow it in Hideout's Apps page.";
/// Start refused: Foxglove runs on its own.
pub const STANDALONE: &str =
    "Temperature needs Hideout and its PC sensors app. Start Foxglove from Hideout to use it.";
/// The button that takes the user to the page that fixes [`NEEDS_APP`] and [`NOT_ALLOWED`].
pub const OPEN_APPS: &str = "Open Apps page";
/// Under the steppers when the ends are as close as they may be.
pub const GAP_HINT: &str = "Hot has to be at least 10\u{a0}°C above cool.";
/// What the scale says with no reading at all.
pub const NO_READING: &str = "No reading";

/// What the page knows of Temperature.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempView {
    /// The reading's row of the plan's state table, as the link last said.
    pub status: Status,
    /// Foxglove runs as Hideout's app (`--module`).
    pub hosted: bool,
    /// The running scene is the Temperature look.
    pub live: bool,
}

/// Why Temperature cannot start now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Not under Hideout.
    Standalone,
    /// The PC sensors app is off or not installed.
    AppOff,
    /// Foxglove has no grant to read the readings.
    NotAllowed,
}

impl Gate {
    /// The line the status shows and the tone.
    pub fn words(self) -> (&'static str, Tone) {
        match self {
            Gate::Standalone => (STANDALONE, Tone::Muted),
            Gate::AppOff => (NEEDS_APP, Tone::Warn),
            Gate::NotAllowed => (NOT_ALLOWED, Tone::Warn),
        }
    }

    /// Hideout's Apps page fixes it.
    pub fn opens_apps(self) -> bool {
        self != Gate::Standalone
    }
}

impl TempView {
    /// What refuses a start: Foxglove on its own, the sensors app off, or no permission. A hub
    /// link that is not up yet (a launch restoring the look) refuses nothing: the lights show
    /// the fallback and the status says it is waiting.
    pub fn gate(&self) -> Option<Gate> {
        if !self.hosted {
            return Some(Gate::Standalone);
        }
        match self.status {
            Status::NotAllowed => Some(Gate::NotAllowed),
            Status::AppOff => Some(Gate::AppOff),
            _ => None,
        }
    }
}

/// What a sensor group is, in words (data keyed by group; no card or vendor name).
pub fn noun(group: &str) -> &'static str {
    match group {
        "gpu" => "graphics card",
        "cpu" => "processor",
        _ => "sensor",
    }
}

/// The same, to start a line.
pub fn noun_capital(group: &str) -> &'static str {
    match group {
        "gpu" => "Graphics card",
        "cpu" => "Processor",
        _ => "Sensor",
    }
}

/// The Reads row: one plain label while one temperature exists.
pub fn reads_label(group: &str) -> String {
    format!("{} temperature", noun_capital(group))
}

/// The status line's words after "Running: Temperature · ", and their tone.
pub fn running_detail(status: Status, group: &str, cool: u16, hot: u16) -> (String, Tone) {
    match status {
        Status::Reading => ("Reading…".into(), Tone::Plain),
        Status::Working { temp_c, game } => {
            let sum = temp::summary(Some(temp_c), cool, hot).unwrap_or_else(|| NO_READING.into());
            let mut s = format!("{} {sum}", noun_capital(group));
            if game {
                s.push_str(" · updates every 10 s while you play");
            }
            (s, Tone::Ok)
        }
        Status::Missing => (
            format!(
                "The {} isn't reporting its temperature right now.",
                noun(group)
            ),
            Tone::Warn,
        ),
        Status::NoReport => (
            if group == temp::DEFAULT_SOURCE {
                "This graphics card doesn't report its temperature to Windows. Today only NVIDIA cards do."
                    .into()
            } else {
                format!(
                    "This {} doesn't report its temperature to Windows.",
                    noun(group)
                )
            },
            Tone::Warn,
        ),
        Status::AppStopped => (
            "The PC sensors app stopped, so there's no reading.".into(),
            Tone::Warn,
        ),
        Status::AppOff => (
            "The PC sensors app is off, so there's no reading.".into(),
            Tone::Warn,
        ),
        Status::NotAllowed => (
            "Foxglove isn't allowed to see your PC's readings.".into(),
            Tone::Warn,
        ),
        Status::HubLost => (
            "Waiting for Hideout, so there's no reading yet.".into(),
            Tone::Plain,
        ),
    }
}

/// The page's status line for the Temperature look: running (the reading's row) or not (what
/// refuses it, else [`NOT_RUNNING`]).
pub fn status_line(running: bool, t: &TempView, look: &Look) -> (String, Tone) {
    if running && t.live {
        let (d, tone) = running_detail(t.status, &look.temp_source, look.cool_c, look.hot_c);
        return (format!("Running: {} · {d}", temp::NAME), tone);
    }
    match t.gate() {
        Some(g) => {
            let (w, tone) = g.words();
            (w.into(), tone)
        }
        None => (NOT_RUNNING.into(), Tone::Muted),
    }
}

/// What the scale writes at its marker: the reading and its word, or why there is none.
pub fn marker_text(running: bool, t: &TempView, look: &Look) -> String {
    if !(running && t.live) {
        return NO_READING.into();
    }
    match t.status {
        Status::Working { temp_c, .. } => temp::summary(Some(temp_c), look.cool_c, look.hot_c)
            .unwrap_or_else(|| NO_READING.into()),
        // One word for every state without a value (the plan, section 2): "doesn't report" is
        // the status line's, not the marker's.
        _ => NO_READING.into(),
    }
}

/// Which end a stepper moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Cool,
    Hot,
}

/// Whether the end may go down and up by [`temp::STEP_C`], keeping the range and the gap.
pub fn can_step(end: End, cool: u16, hot: u16) -> (bool, bool) {
    let (lo, hi) = (*temp::RANGE_C.start(), *temp::RANGE_C.end());
    match end {
        End::Cool => (
            cool >= lo + temp::STEP_C,
            cool + temp::STEP_C + temp::MIN_GAP_C <= hot,
        ),
        End::Hot => (
            hot >= cool + temp::MIN_GAP_C + temp::STEP_C,
            hot + temp::STEP_C <= hi,
        ),
    }
}

/// The ends after one step (`up` or down) of `end`; unchanged when it may not move.
pub fn step(end: End, up: bool, cool: u16, hot: u16) -> (u16, u16) {
    let (down_ok, up_ok) = can_step(end, cool, hot);
    if (up && !up_ok) || (!up && !down_ok) {
        return (cool, hot);
    }
    let by = |v: u16| {
        if up {
            v + temp::STEP_C
        } else {
            v - temp::STEP_C
        }
    };
    match end {
        End::Cool => (by(cool), hot),
        End::Hot => (cool, by(hot)),
    }
}

/// The ends are as close as they may be: the hint says why a stepper stops.
pub fn at_gap(cool: u16, hot: u16) -> bool {
    hot < cool + temp::MIN_GAP_C + temp::STEP_C
}

/// What "Back to" says: the defaults as the button's words.
pub fn back_label() -> String {
    format!(
        "Back to {}-{}\u{a0}°C",
        temp::DEFAULT_COOL_C,
        temp::DEFAULT_HOT_C
    )
}

/// What a screen reader hears after a stepper moved `end`: its name and new value, in words.
pub fn step_announcement(end: End, cool: u16, hot: u16) -> String {
    match end {
        End::Cool => format!("Cool at {cool} degrees Celsius"),
        End::Hot => format!("Hot at {hot} degrees Celsius"),
    }
}

/// What a screen reader hears after Back to the defaults: both ends.
pub fn back_announcement(cool: u16, hot: u16) -> String {
    format!("Back to usual: cool at {cool}, hot at {hot} degrees Celsius")
}

/// What the card remembers between frames to say a lost or returned reading once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Heard {
    /// The last status seen while running.
    seen: Option<Status>,
    /// A reading was lost (missing or the sensors app stopped) and has not come back yet.
    lost: bool,
}

/// The least time between two status announcements, in seconds (the plan, section 7).
pub const SAY_GAP_S: f64 = 5.0;
/// "Temperature reading missing".
pub const SAID_MISSING: &str = "Temperature reading missing";
/// "Temperature reading back".
pub const SAID_BACK: &str = "Temperature reading back";
/// "PC sensors app stopped".
pub const SAID_STOPPED: &str = "PC sensors app stopped";

impl Heard {
    /// What to say now that the status is `now`: only a change of state, never a number.
    /// Returns the new memory and the words (if any).
    pub fn next(self, now: Status) -> (Heard, Option<&'static str>) {
        let was = |f: fn(&Status) -> bool| self.seen.as_ref().is_some_and(f);
        let mut h = Heard {
            seen: Some(now),
            ..self
        };
        let said = match now {
            Status::Missing if !was(|s| matches!(s, Status::Missing)) => {
                h.lost = true;
                Some(SAID_MISSING)
            }
            Status::AppStopped if !was(|s| matches!(s, Status::AppStopped)) => {
                h.lost = true;
                Some(SAID_STOPPED)
            }
            Status::Working { .. } if self.lost => {
                h.lost = false;
                Some(SAID_BACK)
            }
            _ => None,
        };
        (h, said)
    }
}

/// Says a reading lost or back through the live region, at most once per [`SAY_GAP_S`]. A change
/// held back by the limit is said when the time is up. Not running: forgets.
fn announce_status(ui: &egui::Ui, live: bool, status: Status) {
    let ctx = ui.ctx();
    let key = region_id().with("heard");
    if !live {
        // Stopped (or the card is drawn again later): forget, and clear a status line left in
        // the region, so a screen reader reading the page finds no stale "reading missing".
        // Stepper words are not status words and stay.
        ctx.data_mut(|d| d.remove::<(Heard, f64)>(key));
        if announce::said(ctx, region_id())
            .is_some_and(|w| [SAID_MISSING, SAID_BACK, SAID_STOPPED].contains(&w.as_str()))
        {
            announce::say(ctx, region_id(), "");
        }
        return;
    }
    let (mem, at): (Heard, f64) = ctx
        .data(|d| d.get_temp::<(Heard, f64)>(key))
        .unwrap_or((Heard::default(), f64::NEG_INFINITY));
    let (next, said) = mem.next(status);
    let now = ctx.input(|i| i.time);
    match said {
        Some(_) if now - at < SAY_GAP_S => {
            // Too soon: keep the old memory so a later frame says it, and wake then.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAY_GAP_S - (now - at)));
        }
        Some(words) => {
            announce::say(ctx, region_id(), words);
            ctx.data_mut(|d| d.insert_temp(key, (next, now)));
        }
        None => {
            ctx.data_mut(|d| d.insert_temp(key, (next, at)));
        }
    }
}

/// The live region of this card (see [`announce`]).
fn region_id() -> egui::Id {
    egui::Id::new("temperature-card-announce")
}

/// Whether the ends were moved from the defaults.
pub fn moved(look: &Look) -> bool {
    (look.cool_c, look.hot_c) != (temp::DEFAULT_COOL_C, temp::DEFAULT_HOT_C)
}

/// What the card asks for.
#[derive(Default)]
pub struct Asked {
    /// "Open Apps page" was pressed.
    pub open_apps: bool,
}

/// Reads and Colours: above the palette pills.
pub fn head(ui: &mut egui::Ui, look: &Look, set: &mut impl FnMut(Look, bool)) {
    ui.label(theme::caps_label(ui, "Reads"));
    ui.label(reads_label(&look.temp_source));
    ui.label(theme::caps_label(ui, "Colours"));
    ui.horizontal_wrapped(|ui| {
        theme::pill_group(ui, "Colours", |ui, g| {
            for c in Colours::ALL {
                if g.pill(ui, look.temp_colours == c, c.label()).clicked() && look.temp_colours != c
                {
                    set(
                        Look {
                            temp_colours: c,
                            ..look.clone()
                        },
                        true,
                    );
                }
            }
        })
    });
}

/// The label of one end's stepper.
fn end_name(end: End) -> &'static str {
    match end {
        End::Cool => "Cool at",
        End::Hot => "Hot at",
    }
}

/// One end's stepper: its name, a minus, the value, a plus. -1, 1 or 0 for what was pressed.
///
/// `gap` is this end when the ends are as close as they may be: the button that moves toward the
/// other end then says why it stopped (hover and screen reader).
fn stepper(
    ui: &mut egui::Ui,
    end: End,
    value: u16,
    (down, up): (bool, bool),
    gap: Option<End>,
) -> i8 {
    let mut pressed = 0;
    let name = end_name(end);
    ui.label(name);
    let why = |ui: &egui::Ui, r: egui::Response, stopped: bool| {
        if stopped {
            ui.ctx()
                .accesskit_node_builder(r.id, |n| n.set_description(GAP_HINT));
            r.on_disabled_hover_text(GAP_HINT)
        } else {
            r
        }
    };
    let minus = ui.add_enabled(down, egui::Button::new("−"));
    let minus = why(ui, minus, !down && gap == Some(end) && end == End::Hot);
    minus.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            down,
            format!("{name}, {value} degrees Celsius, lower by {}", temp::STEP_C),
        )
    });
    if minus.clicked() {
        pressed = -1;
    }
    // The unit is part of the value's own text, so a screen reader finds no stray "°C" item.
    let v = ui.label(RichText::new(format!("{value}\u{a0}°C")).font(theme::mono(14.0)));
    v.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Label,
            true,
            format!("{name}, {value} degrees Celsius"),
        )
    });
    let plus = ui.add_enabled(up, egui::Button::new("+"));
    let plus = why(ui, plus, !up && gap == Some(end) && end == End::Cool);
    plus.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            up,
            format!(
                "{name}, {value} degrees Celsius, higher by {}",
                temp::STEP_C
            ),
        )
    });
    if plus.clicked() {
        pressed = 1;
    }
    // Up and Down step by 5 while one of the two buttons has the keyboard (Tab reaches them);
    // the other arrows still move between controls.
    for b in [&minus, &plus] {
        if b.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    b.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
        }
    }
    if minus.has_focus() || plus.has_focus() {
        let key = |ui: &egui::Ui, k| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
        if key(ui, egui::Key::ArrowUp) && up {
            pressed = 1;
        } else if key(ui, egui::Key::ArrowDown) && down {
            pressed = -1;
        }
    }
    pressed
}

/// Cool at / Hot at, Back to the defaults, the scale and the reason under it: below the palette
/// pills.
pub fn tail(
    ui: &mut egui::Ui,
    look: &Look,
    t: &TempView,
    running: bool,
    set: &mut impl FnMut(Look, bool),
) -> Asked {
    let p = theme::palette(ui);
    let (cool, hot) = (look.cool_c, look.hot_c);
    let mut asked = Asked::default();
    announce_status(ui, running && t.live, t.status);
    announce::region(ui, region_id());
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for end in [End::Cool, End::Hot] {
            let value = if end == End::Cool { cool } else { hot };
            let gap = at_gap(cool, hot).then_some(end);
            let d = stepper(ui, end, value, can_step(end, cool, hot), gap);
            if d != 0 {
                let (c, h) = step(end, d > 0, cool, hot);
                announce::say(ui.ctx(), region_id(), step_announcement(end, c, h));
                set(
                    Look {
                        cool_c: c,
                        hot_c: h,
                        ..look.clone()
                    },
                    true,
                );
            }
            ui.add_space(14.0);
        }
    });
    if at_gap(cool, hot) {
        ui.add(egui::Label::new(RichText::new(GAP_HINT).color(p.muted)).wrap());
    }
    if moved(look)
        && ui
            .button(back_label())
            .on_hover_text("Cool at and Hot at go back to their usual ends")
            .clicked()
    {
        announce::say(
            ui.ctx(),
            region_id(),
            back_announcement(temp::DEFAULT_COOL_C, temp::DEFAULT_HOT_C),
        );
        set(
            Look {
                cool_c: temp::DEFAULT_COOL_C,
                hot_c: temp::DEFAULT_HOT_C,
                ..look.clone()
            },
            true,
        );
    }
    ui.add_space(6.0);
    let live = running && t.live;
    let reading = if live { t.status.reading() } else { None };
    scale(ui, look, reading, &marker_text(running, t, look));
    // The card repeats the reason under the scale.
    let (line, tone) = if live {
        running_detail(t.status, &look.temp_source, cool, hot)
    } else if let Some(g) = t.gate() {
        let (w, tone) = g.words();
        (w.to_string(), tone)
    } else {
        (NOT_RUNNING.to_string(), Tone::Muted)
    };
    let colour = match tone {
        Tone::Ok => p.ok,
        Tone::Warn => p.warn,
        Tone::Plain => p.text,
        Tone::Muted => p.muted,
    };
    // With a reading the status line and the scale already say it: the line is for the rest.
    if !(live && reading.is_some()) {
        ui.add(egui::Label::new(RichText::new(line).color(colour)).wrap());
    }
    let opens = if live {
        matches!(t.status, Status::AppOff | Status::NotAllowed)
    } else {
        t.gate().is_some_and(Gate::opens_apps)
    };
    if opens && ui.button(OPEN_APPS).clicked() {
        asked.open_apps = true;
    }
    asked
}

/// The scale's name for a screen reader: its ends and what the marker says now, in words.
pub fn scale_name(look: &Look, marker: &str) -> String {
    let now = marker.replace("\u{a0}°C", " degrees Celsius");
    format!(
        "Colour scale from {} to {} degrees Celsius; now {}",
        look.cool_c,
        look.hot_c,
        if now == NO_READING {
            "no reading".to_string()
        } else {
            now
        }
    )
}

/// The still ramp of the chosen colours with its ends written under it and a marker at the
/// reading (when there is one) with its number and word.
fn scale(ui: &mut egui::Ui, look: &Look, reading: Option<f32>, marker: &str) {
    let p = theme::palette(ui);
    let ramp: Vec<Rgb> = temp::ramp(look.temp_colours, &look.palette);
    let font = theme::body(12.0);
    let (top, bar, bottom) = (28.0, 12.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), top + bar + bottom),
        Sense::hover(),
    );
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, scale_name(look, marker))
    });
    let painter = ui.painter();
    let bar_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + top),
        egui::vec2(rect.width(), bar),
    );
    strip(painter, bar_rect, &ramp);
    painter.rect_stroke(
        bar_rect,
        2.0,
        Stroke::new(1.0, p.muted),
        egui::StrokeKind::Outside,
    );
    let label_y = bar_rect.bottom() + 3.0;
    painter.text(
        egui::pos2(rect.left(), label_y),
        Align2::LEFT_TOP,
        format!("{}\u{a0}°C", look.cool_c),
        font.clone(),
        p.muted,
    );
    painter.text(
        egui::pos2(rect.right(), label_y),
        Align2::RIGHT_TOP,
        format!("{}\u{a0}°C", look.hot_c),
        font.clone(),
        p.muted,
    );
    let at = reading.and_then(|t| temp::position(t, look.cool_c, look.hot_c));
    match at {
        Some(at) => {
            let x = rect.left() + at * rect.width();
            // A small triangle over the bar, pointing down at the reading.
            let tip = egui::pos2(x, bar_rect.top() - 1.0);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    tip,
                    egui::pos2(x - 4.0, tip.y - 6.0),
                    egui::pos2(x + 4.0, tip.y - 6.0),
                ],
                p.text,
                Stroke::NONE,
            ));
            let galley = painter.layout_no_wrap(marker.to_string(), font, p.text);
            let w = galley.size().x;
            let left = (x - w / 2.0).clamp(rect.left(), (rect.right() - w).max(rect.left()));
            painter.galley(
                egui::pos2(left, rect.top() - 1.0),
                galley,
                Color32::PLACEHOLDER,
            );
        }
        None => {
            painter.text(
                egui::pos2(rect.center().x, rect.top() + 1.0),
                Align2::CENTER_TOP,
                marker,
                font,
                p.muted,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::scene_looks::Kind;

    fn look() -> Look {
        Look {
            kind: Kind::Temperature,
            ..Look::default()
        }
    }

    fn hosted(status: Status) -> TempView {
        TempView {
            status,
            hosted: true,
            live: true,
        }
    }

    /// Words the plan says never to show.
    const NEVER: [&str; 10] = [
        "sensor.",
        "fact",
        "subscribe",
        "demand",
        "module",
        "plugin",
        "NVML",
        "GPU",
        "CPU",
        "unavailable",
    ];

    fn every_status() -> Vec<Status> {
        vec![
            Status::HubLost,
            Status::NotAllowed,
            Status::AppOff,
            Status::AppStopped,
            Status::Reading,
            Status::Working {
                temp_c: 64.0,
                game: false,
            },
            Status::Working {
                temp_c: 64.0,
                game: true,
            },
            Status::Missing,
            Status::NoReport,
        ]
    }

    #[test]
    fn the_status_lines_are_the_plans_exactly() {
        let l = look();
        let line = |s| status_line(true, &hosted(s), &l);
        assert_eq!(
            line(Status::Reading),
            ("Running: Temperature · Reading…".into(), Tone::Plain)
        );
        assert_eq!(
            line(Status::Working {
                temp_c: 64.0,
                game: false
            }),
            (
                "Running: Temperature · Graphics card 64\u{a0}°C, warm".into(),
                Tone::Ok
            )
        );
        assert_eq!(
            line(Status::Working {
                temp_c: 64.0,
                game: true
            }),
            (
                "Running: Temperature · Graphics card 64\u{a0}°C, warm · updates every 10 s while you play"
                    .into(),
                Tone::Ok
            )
        );
        assert_eq!(
            line(Status::Missing),
            (
                "Running: Temperature · The graphics card isn't reporting its temperature right now."
                    .into(),
                Tone::Warn
            )
        );
        assert_eq!(
            line(Status::NoReport),
            (
                "Running: Temperature · This graphics card doesn't report its temperature to Windows. Today only NVIDIA cards do."
                    .into(),
                Tone::Warn
            )
        );
        assert_eq!(
            line(Status::AppStopped),
            (
                "Running: Temperature · The PC sensors app stopped, so there's no reading.".into(),
                Tone::Warn
            )
        );
    }

    #[test]
    fn a_start_is_refused_for_each_gate_and_not_otherwise() {
        let standalone = TempView {
            hosted: false,
            ..hosted(Status::HubLost)
        };
        assert_eq!(standalone.gate(), Some(Gate::Standalone));
        assert_eq!(hosted(Status::AppOff).gate(), Some(Gate::AppOff));
        assert_eq!(hosted(Status::NotAllowed).gate(), Some(Gate::NotAllowed));
        for s in [
            Status::HubLost,
            Status::AppStopped,
            Status::Reading,
            Status::Missing,
            Status::NoReport,
            Status::Working {
                temp_c: 50.0,
                game: false,
            },
        ] {
            assert_eq!(hosted(s).gate(), None, "{s:?}");
        }
        let idle = |t: TempView| status_line(false, &TempView { live: false, ..t }, &look());
        assert_eq!(idle(standalone), (STANDALONE.into(), Tone::Muted));
        assert_eq!(
            idle(hosted(Status::AppOff)),
            (
                "Temperature needs the PC sensors app. Turn it on in Hideout's Apps page.".into(),
                Tone::Warn
            )
        );
        assert_eq!(
            idle(hosted(Status::NotAllowed)),
            (
                "Foxglove isn't allowed to see your PC's readings. Allow it in Hideout's Apps page."
                    .into(),
                Tone::Warn
            )
        );
        assert_eq!(
            idle(hosted(Status::Reading)),
            (
                "Not running. The scale shows the last reading once it starts.".into(),
                Tone::Muted
            )
        );
        assert!(Gate::AppOff.opens_apps() && Gate::NotAllowed.opens_apps());
        assert!(!Gate::Standalone.opens_apps());
    }

    #[test]
    fn no_reading_is_words_never_zero() {
        let l = look();
        for s in every_status() {
            let t = hosted(s);
            let marker = marker_text(true, &t, &l);
            let line = status_line(true, &t, &l).0;
            if s.reading().is_none() {
                for text in [&marker, &line] {
                    assert!(
                        !text.contains("0\u{a0}°C")
                            && !text.contains("0 °C")
                            && !text.contains('°')
                            || text.contains("Today")
                    );
                    assert!(!text.trim_start_matches("Running: ").is_empty());
                }
                assert!(
                    !marker.chars().any(|c| c.is_ascii_digit()),
                    "{s:?}: {marker}"
                );
            }
        }
        assert_eq!(
            marker_text(
                true,
                &hosted(Status::Working {
                    temp_c: 64.0,
                    game: false
                }),
                &l
            ),
            "64\u{a0}°C, warm"
        );
        for s in [Status::Missing, Status::NoReport, Status::Reading] {
            assert_eq!(marker_text(true, &hosted(s), &l), "No reading");
        }
        assert_eq!(
            marker_text(false, &hosted(Status::Reading), &l),
            "No reading"
        );
    }

    #[test]
    fn no_line_uses_a_word_the_plan_forbids() {
        let mut texts: Vec<String> = vec![
            NOT_RUNNING.into(),
            NEEDS_APP.into(),
            NOT_ALLOWED.into(),
            STANDALONE.into(),
            OPEN_APPS.into(),
            GAP_HINT.into(),
            NO_READING.into(),
            back_label(),
            reads_label("gpu"),
            reads_label("cpu"),
            reads_label("anything-else"),
            temp::DESCRIPTION.into(),
        ];
        for group in ["gpu", "cpu", "other"] {
            let mut l = look();
            l.temp_source = group.into();
            for s in every_status() {
                texts.push(status_line(true, &hosted(s), &l).0);
                texts.push(running_detail(s, group, 40, 80).0);
                texts.push(marker_text(true, &hosted(s), &l));
            }
        }
        for t in &texts {
            for bad in NEVER {
                assert!(!t.contains(bad), "{bad:?} in {t:?}");
            }
            assert!(!t.contains("unknown") && !t.contains("Unknown"), "{t}");
        }
    }

    #[test]
    fn the_steppers_stop_at_the_range_and_the_gap() {
        // Defaults: both move.
        assert_eq!(can_step(End::Cool, 40, 80), (true, true));
        assert_eq!(can_step(End::Hot, 40, 80), (true, true));
        assert_eq!(step(End::Cool, true, 40, 80), (45, 80));
        assert_eq!(step(End::Hot, false, 40, 80), (40, 75));
        // The range: cool 20..., hot ...100.
        assert_eq!(step(End::Cool, false, 20, 80), (20, 80));
        assert_eq!(step(End::Hot, true, 40, 100), (40, 100));
        // The gap: the stepper that would cross stops.
        assert_eq!(can_step(End::Cool, 70, 80), (true, false));
        assert_eq!(step(End::Cool, true, 70, 80), (70, 80));
        assert_eq!(can_step(End::Hot, 70, 80), (false, true));
        assert_eq!(step(End::Hot, false, 70, 80), (70, 80));
        assert!(at_gap(70, 80) && !at_gap(40, 80) && !at_gap(65, 80));
        // Every step keeps the ends in range and 10 apart.
        for cool in (20..=90).step_by(5) {
            for hot in (30..=100).step_by(5) {
                if hot < cool + 10 {
                    continue;
                }
                for end in [End::Cool, End::Hot] {
                    for up in [true, false] {
                        let (c, h) = step(end, up, cool, hot);
                        assert_eq!(temp::clamp_ends(c, h), (c, h), "{end:?} {up} {cool} {hot}");
                    }
                }
            }
        }
        assert_eq!(back_label(), "Back to 40-80\u{a0}°C");
        assert!(!moved(&look()));
        assert!(moved(&Look {
            cool_c: 45,
            ..look()
        }));
    }

    #[test]
    fn a_step_is_said_with_its_value_and_unit_in_a_live_region() {
        super::super::store::test_config_dir();
        assert_eq!(
            step_announcement(End::Cool, 45, 80),
            "Cool at 45 degrees Celsius"
        );
        assert_eq!(
            step_announcement(End::Hot, 40, 75),
            "Hot at 75 degrees Celsius"
        );
        assert_eq!(
            back_announcement(40, 80),
            "Back to usual: cool at 40, hot at 80 degrees Celsius"
        );
        // Drawn: the region is a polite node named by the last thing said; with nothing said
        // yet it has no words (and never a number).
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let frame = |ctx: &egui::Context| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                announce::region(ui, region_id());
            });
            out.textures_delta.clear();
            out.platform_output.accesskit_update.expect("a tree")
        };
        let live = |u: &egui::accesskit::TreeUpdate| -> Vec<String> {
            u.nodes
                .iter()
                .map(|(_, n)| n)
                .filter(|n| n.live() == Some(egui::accesskit::Live::Polite))
                .map(|n| n.label().unwrap_or_default().to_string())
                .collect()
        };
        assert_eq!(live(&frame(&ctx)), vec![String::new()]);
        announce::say(&ctx, region_id(), step_announcement(End::Hot, 40, 75));
        assert_eq!(
            live(&frame(&ctx)),
            vec!["Hot at 75 degrees Celsius".to_string()]
        );
        // The words are plain: no reading, so nothing that could say 0.
        for t in [
            step_announcement(End::Cool, 20, 80),
            back_announcement(40, 80),
        ] {
            assert!(t.ends_with("degrees Celsius") && !t.contains('°'), "{t}");
        }
    }

    fn tree_of(
        ctx: &egui::Context,
        mut add: impl FnMut(&mut egui::Ui),
    ) -> Vec<egui::accesskit::Node> {
        let mut fonts = egui::FontDefinitions::default();
        theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut out = ctx.run_ui(egui::RawInput::default(), &mut add);
        out.textures_delta.clear();
        out.platform_output
            .accesskit_update
            .expect("a tree")
            .nodes
            .into_iter()
            .map(|(_, n)| n)
            .collect()
    }

    #[test]
    fn a_lost_reading_and_its_return_are_said_once_and_never_as_a_number() {
        let work = Status::Working {
            temp_c: 64.0,
            game: false,
        };
        let mut h = Heard::default();
        let mut said = Vec::new();
        for s in [
            Status::Reading,
            work,
            work,
            Status::Missing,
            Status::Missing,
            Status::Reading,
            work,
            work,
            Status::AppStopped,
            Status::AppStopped,
            work,
        ] {
            let (n, w) = h.next(s);
            h = n;
            said.extend(w);
        }
        assert_eq!(said, [SAID_MISSING, SAID_BACK, SAID_STOPPED, SAID_BACK]);
        for w in said {
            assert!(!w.chars().any(|c| c.is_ascii_digit()), "{w}");
        }
        // A start that never had a reading has nothing to say.
        assert_eq!(Heard::default().next(Status::NoReport).1, None);
        assert_eq!(Heard::default().next(work).1, None);
    }

    #[test]
    fn status_changes_reach_the_live_region_at_most_once_per_five_seconds() {
        super::super::store::test_config_dir();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let at = |t: f64| egui::RawInput {
            time: Some(t),
            ..Default::default()
        };
        let work = Status::Working {
            temp_c: 64.0,
            game: false,
        };
        let frame = |t: f64, st: Status| {
            let mut out = ctx.run_ui(at(t), |ui| announce_status(ui, true, st));
            out.textures_delta.clear();
            announce::said(&ctx, region_id())
        };
        assert_eq!(frame(0.0, work), None);
        assert_eq!(frame(1.0, Status::Missing).as_deref(), Some(SAID_MISSING));
        // Back at 2 s is held until 6 s.
        assert_eq!(frame(2.0, work).as_deref(), Some(SAID_MISSING));
        assert_eq!(frame(4.0, work).as_deref(), Some(SAID_MISSING));
        assert_eq!(frame(6.5, work).as_deref(), Some(SAID_BACK));
    }

    #[test]
    fn the_scale_is_named_with_what_the_marker_says() {
        let l = look();
        assert_eq!(
            scale_name(&l, "64\u{a0}°C, warm"),
            "Colour scale from 40 to 80 degrees Celsius; now 64 degrees Celsius, warm"
        );
        assert_eq!(
            scale_name(&l, NO_READING),
            "Colour scale from 40 to 80 degrees Celsius; now no reading"
        );
        assert!(!scale_name(&l, NO_READING).contains(" 0 "));
    }

    #[test]
    fn stopping_clears_a_status_line_but_keeps_stepper_words() {
        let ctx = egui::Context::default();
        let draw = |live: bool, status: Status| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                announce_status(ui, live, status);
            });
            out.textures_delta.clear();
        };
        draw(true, Status::Missing);
        assert_eq!(
            announce::said(&ctx, region_id()).as_deref(),
            Some(SAID_MISSING)
        );
        draw(false, Status::Missing);
        assert_eq!(announce::said(&ctx, region_id()).as_deref(), Some(""));
        announce::say(&ctx, region_id(), "Cool at 45 degrees Celsius");
        draw(false, Status::Missing);
        assert_eq!(
            announce::said(&ctx, region_id()).as_deref(),
            Some("Cool at 45 degrees Celsius")
        );
    }

    #[test]
    fn a_stepper_has_no_stray_unit_item_and_the_gap_is_said_on_the_stopped_button() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        // Cool 70, hot 80: Cool's plus and Hot's minus are stopped by the gap.
        let nodes = tree_of(&ctx, |ui| {
            ui.horizontal(|ui| {
                stepper(
                    ui,
                    End::Cool,
                    70,
                    can_step(End::Cool, 70, 80),
                    Some(End::Cool),
                );
                stepper(ui, End::Hot, 80, can_step(End::Hot, 70, 80), Some(End::Hot));
            });
        });
        let labels: Vec<&str> = nodes
            .iter()
            .flat_map(|n| [n.label(), n.value()])
            .flatten()
            .collect();
        assert!(!labels.contains(&"°C"), "{labels:?}");
        assert!(
            labels.contains(&"Cool at, 70 degrees Celsius"),
            "{labels:?}"
        );
        let stopped: Vec<&egui::accesskit::Node> = nodes
            .iter()
            .filter(|n| n.description() == Some(GAP_HINT))
            .collect();
        assert_eq!(stopped.len(), 2);
        assert!(stopped.iter().all(|n| n.is_disabled()));
        // Away from the gap nothing carries the hint.
        let nodes = tree_of(&ctx, |ui| {
            stepper(ui, End::Cool, 40, can_step(End::Cool, 40, 80), None);
        });
        assert!(nodes.iter().all(|n| n.description().is_none()));
    }
}
