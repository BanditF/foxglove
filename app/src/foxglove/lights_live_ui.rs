//! The keyboard drawing at the top of the Lights page's Keyboard card (slice L2 of "lights live
//! in Foxglove's window", docs: review/ux-plan-lights-live.md section 4): what the keyboard's
//! lights show now, one colour per key, with one line under it saying what draws them.
//!
//! The colours are [`lights_live`]'s; this file reads the app's state into a [`Live`] (the page
//! builds one per frame), paints it, and decides when the next frame is wanted. Events first: a
//! still look asks for no frame, and a moving one asks for one every [`PREVIEW_INTERVAL`] only
//! while the window is in front, not minimised, the user is here and Windows' animation effects
//! are on. Behind another window the last drawn frame is held.

use std::time::Duration;

use egui::{Color32, RichText, Sense, Stroke, Vec2};
use foxglove_link::protocol::{RgbHalf, RgbState};
use foxglove_scene::{Clock, SceneTime};

use super::key_layout::KeyLayout;
use super::lights_live::{self, Keys, Look, Report, SceneRun, State};
use super::parts::Parts;
use super::pc_lights::KeyboardLights;
use super::scene_card::CardLook;
use super::theme;

/// The drawing's height: a share of the card's width, within these (min, max).
const HEIGHT: (f32, f32) = (84.0, 180.0);
/// A key that is off: the dark plate every key sits on.
const PLATE: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x20);
/// A lit key never shows darker than this share of full value: a dim key must still read as lit.
const FLOOR: u8 = 96;

pub const LIGHTS_OFF: &str = "Lights off";
pub const NOT_CONNECTED: &str = "Keyboard not connected. Its lights show here when it's back.";
pub const READING: &str = "Reading the keyboard's lights…";
pub const THROUGH_WINDOWS: &str = "Drawn through Windows Dynamic Lighting";
pub const ANOTHER_APP: &str = "Another app sets these lights. Foxglove can't see what it shows.";
pub const CANNOT_DRAW: &str = "Foxglove can't draw this effect here.";
pub const SENDING: &str = "Sending…";
pub const NOT_APPLIED: &str = "not applied yet";
pub const NO_REPORT: &str = "no report yet";
pub const STILL: &str = " · still (Windows animation effects are off)";
pub const ABOUT: &str = "What your keyboard's lights show now, drawn by Foxglove from what the keyboard reports. Changes here show at once.";
pub const ABOUT_MOVING: &str = "The pace matches the keyboard; the exact moment may differ.";

/// What the drawing shows, ready to paint at any moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    pub look: Look,
    pub layout: KeyLayout,
    /// The running scene's clock; the keyboard's own effects run on the system clock.
    pub clock: Option<Clock>,
    /// The line under the drawing.
    pub line: String,
    /// The drawing's name for screen readers: the state, never the frame.
    pub name: String,
    /// Windows' animation effects are on (a moving look moves).
    pub animations: bool,
}

/// `part`'s lights as a card pick (what the keyboard's own firmware draws).
fn card_of(h: &RgbHalf, in_colour: bool) -> CardLook {
    CardLook {
        lights: KeyboardLights {
            hue: h.hue,
            sat: h.sat,
            brightness: h.brightness,
            effect: h.effect,
        },
        speed: h.speed,
        in_colour,
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// One half in words: "Swirl · Orange · 60%", "Spectrum · its own colours · 60%", "Off".
fn half_words(rgb: &RgbState, h: &RgbHalf, dim: u8) -> String {
    if !h.on {
        return LIGHTS_OFF.into();
    }
    let level = u16::from(h.brightness) * u16::from(dim.min(100)) / 100;
    match super::effect_controls::underglow_in(Some(rgb), h.effect) {
        Some(spec) if spec.rainbow() => format!("{} · its own colours · {level}%", spec.name),
        Some(spec) => format!(
            "{} · {} · {level}%",
            spec.name,
            capital(super::effect_controls::colour_name(h.hue, h.sat))
        ),
        None => format!("Effect {} · {level}%", h.effect),
    }
}

/// The line under the drawing and the drawing's name for screen readers, from the look's source.
/// `rgb` is what the keyboard reported, `dim` each half's battery dim, `parts` names the halves.
pub fn words(
    look: &Look,
    rgb: Option<&RgbState>,
    dim: [u8; 2],
    parts: &Parts,
    animations: bool,
) -> (String, String) {
    words_with(look, rgb, dim, parts, animations, false)
}

/// [`words`], and `right_apart`: the right half's lights are apart from the left's, so with no
/// report from it the line says so instead of speaking for both halves.
pub fn words_with(
    look: &Look,
    rgb: Option<&RgbState>,
    dim: [u8; 2],
    parts: &Parts,
    animations: bool,
    right_apart: bool,
) -> (String, String) {
    let (line, name) = match &look.state {
        State::NotConnected => (
            NOT_CONNECTED.to_string(),
            "Keyboard lights: keyboard not connected".to_string(),
        ),
        State::Reading => (READING.into(), "Keyboard lights: reading".into()),
        State::Lease(who) => (
            format!("{who} shows over your lights."),
            format!("Keyboard lights: {who} shows over your lights"),
        ),
        State::Scene(scene) => (
            format!("Scene: {scene}"),
            format!("Keyboard lights: scene {scene}"),
        ),
        State::ThroughWindows => (
            THROUGH_WINDOWS.into(),
            "Keyboard lights: drawn through Windows".into(),
        ),
        State::AnotherApp => (
            ANOTHER_APP.into(),
            "Keyboard lights: set by another app".into(),
        ),
        State::CannotDraw(n) => {
            let colour = rgb.map_or("", |r| {
                super::effect_controls::colour_name(r.left.hue, r.left.sat)
            });
            (
                CANNOT_DRAW.into(),
                format!("Keyboard lights: effect {n}, {colour}"),
            )
        }
        State::Keyboard => match rgb {
            Some(rgb) => own_words(rgb, dim, parts, right_apart),
            None => (READING.into(), "Keyboard lights: reading".into()),
        },
    };
    let mut line = if look.sending {
        format!("{line} · {SENDING}")
    } else {
        line
    };
    if look.next_frame().is_some() && !animations {
        line.push_str(STILL);
    }
    (line, name)
}

/// The keyboard's own effect: both halves in one phrase when they agree, else side by side.
fn own_words(rgb: &RgbState, dim: [u8; 2], parts: &Parts, right_apart: bool) -> (String, String) {
    let left = half_words(rgb, &rgb.left, dim[0]);
    let unheard = right_apart && rgb.right.is_none();
    let right = match rgb.right {
        _ if unheard => NO_REPORT.to_string(),
        Some((h, _)) => half_words(rgb, &h, dim[1]),
        None => left.clone(),
    };
    let name_of = |h: &RgbHalf, dim: u8| {
        if !h.on {
            return "off".to_string();
        }
        let effect = super::effect_controls::underglow_in(Some(rgb), h.effect)
            .map_or_else(|| format!("effect {}", h.effect), |s| s.name.to_lowercase());
        let colour = super::effect_controls::colour_name(h.hue, h.sat);
        format!(
            "{effect}, {colour}, {} percent",
            u16::from(h.brightness) * u16::from(dim.min(100)) / 100
        )
    };
    let right_half = rgb.right.map_or(rgb.left, |(h, _)| h);
    if left == right && !unheard {
        let name = if left == LIGHTS_OFF {
            "Keyboard lights off".to_string()
        } else {
            let first = super::effect_controls::underglow_in(Some(rgb), rgb.left.effect)
                .map_or_else(
                    || format!("effect {}", rgb.left.effect),
                    |s| s.name.to_string(),
                );
            let rest = name_of(&rgb.left, dim[0]);
            let rest = rest.split_once(", ").map_or("", |(_, r)| r);
            format!("Keyboard lights: {first}, {rest}")
        };
        return (left, name);
    }
    let two = parts.is_split();
    let (a, b) = if two {
        (parts.short(0), parts.short(1))
    } else {
        ("Left".into(), "Right".into())
    };
    (
        format!("{a}: {left}   {b}: {right}"),
        format!(
            "Keyboard lights: {} {}, {} {}",
            a.to_lowercase(),
            name_of(&rgb.left, dim[0]),
            b.to_lowercase(),
            if unheard {
                NO_REPORT.to_string()
            } else {
                name_of(&right_half, dim[1])
            }
        ),
    )
}

/// The line and name of a pick drawn but not applied: the drawing is not the keyboard's lights.
pub fn not_applied(line: String, name: String) -> (String, String) {
    (
        format!("{line} · {NOT_APPLIED}"),
        format!("{name}, {NOT_APPLIED}"),
    )
}

/// The hover on the ⓘ.
pub fn about(look: &Look) -> String {
    if look.next_frame().is_some() {
        format!("{ABOUT}\n{ABOUT_MOVING}")
    } else {
        ABOUT.into()
    }
}

/// Who holds a lease, by the plugin's name in its manifest; "An app" for anyone else.
fn lease_name(conn: u64) -> String {
    foxglove_plugin_host::plugin_of(conn)
        .and_then(|id| {
            super::plugin_install::installed()
                .into_iter()
                .find(|p| p.manifest.id == id)
                .map(|p| p.manifest.name)
        })
        .unwrap_or_else(|| "An app".into())
}

/// What the keyboard's lights show now, from the app's state: the keyboard's report, who holds
/// the lights, a running scene. `layout` is the keyboard's key drawing.
pub fn live_now(rgb: Option<&RgbState>, layout: KeyLayout) -> Live {
    live_with(rgb, layout, None)
}

/// [`live_now`] with the card's draft: the lights the user picked or sent, drawn before the
/// keyboard reports them (L3). The line and the name say the drafted lights too.
pub fn live_with(
    rgb: Option<&RgbState>,
    layout: KeyLayout,
    draft: Option<&lights_live::Draft>,
) -> Live {
    use super::lights_owner::owner_now;
    use foxglove_link::lights_owner::Owner;
    let drafted = rgb.map(|r| {
        let mut r = *r;
        if let Some(d) = draft {
            r.left = d.left.unwrap_or(r.left);
            if let (Some(h), Some((_, age))) = (d.right, r.right) {
                r.right = Some((h, age));
            }
        }
        r
    });
    let rgb = drafted.as_ref();
    let key = super::devices::keyboard_key();
    let connected = key.is_some();
    let owner = owner_now();
    // Foxglove's own pick through Windows is drawn as the keyboard reports it; another app's
    // is not known.
    let windows = match (owner, rgb) {
        (Some(o @ (Owner::LampArrayUsb | Owner::LampArrayBle)), Some(r))
            if super::lamp_hold::holds(o) =>
        {
            Some(card_of(
                &r.left,
                r.colour_effects
                    .is_some_and(|m| r.left.effect < 8 && m & (1 << r.left.effect) != 0),
            ))
        }
        _ => None,
    };
    let report = Report {
        connected,
        rgb: rgb.copied(),
        dim: super::battery_dim_ui::levels(),
        windows,
        right_apart: matches!(
            super::keyboard_settings::rgb_sync(),
            super::keyboard_settings::RgbSync::Known { on: false }
        ),
    };
    let lease = key
        .as_deref()
        .and_then(super::lighting_leases::showing_on)
        .map(|l| (lease_name(l.conn), l));
    let scene = super::scene_run::running_look();
    let run = scene.as_ref().map(|(name, effect, _)| SceneRun {
        name: name.clone(),
        effect: effect.clone(),
        room: super::scene_room::room(),
    });
    let mut look = lights_live::look(
        &report,
        owner,
        lease.as_ref().map(|(who, l)| (who.as_str(), l)),
        run.as_ref(),
        draft,
    );
    // Windows draws the pick too; a lease or a scene over everything is not waiting on it.
    if let Some(d) = draft {
        look.sending = d.sent
            && matches!(
                look.state,
                State::Keyboard | State::CannotDraw(_) | State::ThroughWindows
            );
    }
    let animations = super::screen_glow::animations_on();
    let (mut line, mut name) = words_with(
        &look,
        rgb,
        report.dim,
        &Parts::current(),
        animations,
        report.right_apart,
    );
    // A pick not sent yet is drawn, and the words say it is not the keyboard's lights.
    if draft.is_some_and(|d| !d.sent)
        && matches!(
            look.state,
            State::Keyboard | State::CannotDraw(_) | State::ThroughWindows
        )
    {
        (line, name) = not_applied(line, name);
    }
    Live {
        look,
        layout,
        clock: scene.map(|(_, _, c)| c),
        line,
        name,
        animations,
    }
}

thread_local! {
    /// Tests set the moment drawn.
    #[cfg(test)]
    pub static NOW: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
    /// Frames asked for, so tests can count them.
    #[cfg(test)]
    pub static REPAINTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn now_ms() -> i64 {
    #[cfg(test)]
    if let Some(t) = NOW.get() {
        return t;
    }
    foxglove_scene::clock::unix_ms_now()
}

/// When the drawing wants its next frame: the look's own pace while the window is in front, not
/// minimised, the user is here and animation effects are on; never otherwise.
pub fn frame_request(
    look: &Look,
    front: bool,
    minimised: bool,
    away: bool,
    animations: bool,
) -> Option<Duration> {
    if front && !minimised && !away && animations {
        look.next_frame()
    } else {
        None
    }
}

/// The keys to draw: now while the window is in front, else the last drawn.
pub(crate) fn held(ctx: &egui::Context, id: egui::Id, front: bool, now: Keys) -> Keys {
    let id = id.with("held-keys");
    let last = ctx.data(|d| d.get_temp::<Keys>(id));
    let shown = match last {
        Some(last) if !front => last,
        _ => now,
    };
    ctx.data_mut(|d| d.insert_temp(id, shown.clone()));
    shown
}

/// A lit key's colour on screen: lifted by a perceptual curve so a 60% orange still reads as
/// orange, and never darker than [`FLOOR`], so a dim key reads as lit.
fn lit(c: Color32) -> Color32 {
    let lift = |v: u8| (255.0 * (f32::from(v) / 255.0).powf(0.75)).round() as u8;
    let c = Color32::from_rgb(lift(c.r()), lift(c.g()), lift(c.b()));
    let top = c.r().max(c.g()).max(c.b());
    if top >= FLOOR {
        return c;
    }
    let k = f32::from(FLOOR) / f32::from(top.max(1));
    let up = |v: u8| (f32::from(v) * k).round().min(255.0) as u8;
    if top == 0 {
        return Color32::from_gray(FLOOR);
    }
    Color32::from_rgb(up(c.r()), up(c.g()), up(c.b()))
}

/// The scene time drawn at system moment `now`: the scene's own clock, else the system's; with
/// Windows' animation effects off, the first frame, still.
pub(crate) fn scene_time(live: &Live, now: i64) -> SceneTime {
    match (live.animations, live.clock) {
        (false, _) => SceneTime(0),
        (true, Some(c)) => c.at(now),
        (true, None) => SceneTime(now),
    }
}

/// The drawing, its line and the ⓘ.
pub fn show(ui: &mut egui::Ui, live: &Live) {
    let p = theme::palette(ui);
    let ctx = ui.ctx().clone();
    let front = ctx.input(|i| i.focused);
    let minimised = ctx.input(|i| i.viewport().minimized) == Some(true);
    let halves = live
        .layout
        .parts
        .iter()
        .filter(|k| !k.is_empty())
        .count()
        .clamp(1, 2) as u8;
    let width = ui.available_width();
    let height = (width * 0.16).clamp(HEIGHT.0, HEIGHT.1);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let name = live.name.clone();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, &name));
    let id = response.id;
    let t = scene_time(live, now_ms());
    let keys = held(&ctx, id, front, live.look.colours(&live.layout, t));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, p.well);
    let g = super::led_map_ui::Geometry::new(&live.layout, halves, rect.shrink(8.0));
    for a in 0..usize::from(halves) {
        let part = keys.get(a);
        for (i, k) in g.keys(&live.layout, a).iter().enumerate() {
            let colour = part.and_then(|p| p.get(i)).copied().flatten();
            // The outline reads against the well in both themes; on the light one every key sits
            // on the dark plate, so white and yellow have something to show on.
            let outline = Stroke::new(1.0, p.muted);
            if !p.dark {
                painter.add(g.key_shape(a, k, PLATE, outline));
            }
            let shape = match colour {
                None if p.dark => g.key_shape(a, k, Color32::TRANSPARENT, outline),
                None => continue,
                Some(c) if c == Color32::BLACK => g.key_shape(a, k, PLATE, outline),
                Some(c) => g.key_shape(a, k, lit(c), Stroke::new(1.0, p.line)),
            };
            painter.add(shape);
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(&live.line).color(p.muted).size(12.5)).wrap());
        theme::info_dot(ui, &about(&live.look));
    });
    let away = super::away::away();
    if let Some(after) = frame_request(&live.look, front, minimised, away, live.animations) {
        ctx.request_repaint_after(after);
        #[cfg(test)]
        REPAINTS.set(REPAINTS.get() + 1);
    }
}

#[cfg(test)]
#[path = "lights_live_ui_tests.rs"]
pub(crate) mod tests;
