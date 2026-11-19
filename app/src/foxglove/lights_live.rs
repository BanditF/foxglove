//! What the keyboard's lights show now, as one colour per key (slice L1 of "lights live in
//! Foxglove's window", docs: review/ux-plan-lights-live.md sections 1 and 3).
//!
//! Pure: no UI, no timer, no read from the keyboard. [`look`] picks the source of the frame (the
//! first that applies, in the plan's order) and [`Look::colours`] draws it over a [`KeyLayout`]
//! at a moment `t`. The firmware's effects are the scene looks the Keyboard card already uses
//! ([`scene_card::scene_look`]) rendered over the keyboard's own lamps ([`scene_card::card_room`]),
//! at the keyboard's pace ([`scene_card::period_ms`]); a running scene is rendered over its
//! room. Still looks (Solid, off, a lease's colour, unknown) ask for no next frame.

use std::time::Duration;

use egui::Color32;
use foxglove_link::lights_owner::Owner;
use foxglove_link::protocol::{RgbHalf, RgbState};
use foxglove_scene::room::{Placed, PlacedLight};
use foxglove_scene::{Effect, Point, Rect, Rgb, RoomMap, SceneTime};

use super::key_layout::{KeyLayout, LayoutKey};
use super::lighting_leases::Lease;
use super::lights_page::shown_color;
use super::pc_lights::KeyboardLights;
use super::scene_card::{card_room, scene_look, CardLook, SceneLook};
use super::scenes_ui::PREVIEW_INTERVAL;

/// One colour per key, per part; `None` is a key drawn as an outline only.
pub type Keys = Vec<Vec<Option<Color32>>>;

/// What the keyboard last reported, and what Foxglove knows around it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    pub connected: bool,
    /// The keyboard's RGB_STATE; `None` before the first answer.
    pub rgb: Option<RgbState>,
    /// Percent of its brightness each half draws at now (the battery dim); 100 is undimmed.
    pub dim: [u8; 2],
    /// Windows holds the lights and Foxglove draws through it: the card's pick. `None` with a
    /// Windows owner means another app draws.
    pub windows: Option<CardLook>,
}

/// A scene that runs: its effect over its room (the keyboard's parts are `keyboard`, or
/// `keyboard:left` and `keyboard:right`).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneRun {
    pub name: String,
    pub effect: Effect,
    pub room: RoomMap,
}

/// The card's pick before the keyboard answers: each half's lights as drafted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub left: Option<RgbHalf>,
    pub right: Option<RgbHalf>,
}

/// Which source drew the frame, for the line under the drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    NotConnected,
    Reading,
    /// A plugin's colour over everything, by the lease's name.
    Lease(String),
    Scene(String),
    /// Windows holds the lights and Foxglove draws through it.
    ThroughWindows,
    /// Another app sets them: outlines only.
    AnotherApp,
    /// The keyboard draws its own effect.
    Keyboard,
    /// An effect Foxglove has no look for (its number); drawn as its colour, still.
    CannotDraw(u8),
}

#[derive(Clone, Debug, PartialEq)]
enum HalfPaint {
    Dark,
    Still(Color32),
    Moving(SceneLook),
}

#[derive(Clone, Debug, PartialEq)]
enum Paint {
    Outline,
    Halves([HalfPaint; 2]),
    Scene(Box<SceneRun>),
}

/// What the keyboard's lights show, ready to draw at any moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub state: State,
    /// A draft is drawn: the keyboard hasn't answered yet.
    pub sending: bool,
    paint: Paint,
}

/// The live look: the first source that applies wins (plan section 3).
pub fn look(
    report: &Report,
    owner: Option<Owner>,
    lease: Option<(&str, &Lease)>,
    scene: Option<&SceneRun>,
    draft: Option<&Draft>,
) -> Look {
    let made = |state, paint| Look {
        state,
        sending: false,
        paint,
    };
    let Some(rgb) = report.rgb.filter(|_| report.connected) else {
        let state = if report.connected {
            State::Reading
        } else {
            State::NotConnected
        };
        return made(state, Paint::Outline);
    };
    if let Some((name, l)) = lease {
        let c = HalfPaint::Still(shown_color(l.hue, l.sat, l.brightness));
        return made(
            State::Lease(name.to_string()),
            Paint::Halves([c.clone(), c]),
        );
    }
    if let Some(s) = scene {
        return made(
            State::Scene(s.name.clone()),
            Paint::Scene(Box::new(s.clone())),
        );
    }
    match owner {
        Some(Owner::LampArrayUsb | Owner::LampArrayBle) => windows(report, rgb),
        Some(Owner::Other(_)) => made(State::AnotherApp, Paint::Outline),
        _ => own(report, rgb, draft),
    }
}

/// Windows holds the lights: Foxglove's own pick through it, or another app's (outlines).
fn windows(report: &Report, rgb: RgbState) -> Look {
    let Some(card) = report.windows else {
        return Look {
            state: State::AnotherApp,
            sending: false,
            paint: Paint::Outline,
        };
    };
    let p = paint_card(&card);
    let on = rgb.left.on;
    let half = if on { p } else { HalfPaint::Dark };
    Look {
        state: State::ThroughWindows,
        sending: false,
        paint: Paint::Halves([half.clone(), half]),
    }
}

/// The keyboard draws its own effect, each half by its own report (or the draft).
fn own(report: &Report, rgb: RgbState, draft: Option<&Draft>) -> Look {
    let left = draft.and_then(|d| d.left).unwrap_or(rgb.left);
    // The right half follows the left until the keyboard has data from it.
    let reported = rgb.right.map_or(left, |(h, _)| h);
    let right = draft.and_then(|d| d.right).unwrap_or(reported);
    let ce = rgb.colour_effects;
    let halves = [(left, 0), (right, 1)].map(|(h, i)| half_paint(&h, report.dim[i], ce));
    let cannot = [left, right]
        .iter()
        .find(|h| h.on && h.effect > 3)
        .map(|h| h.effect);
    Look {
        state: cannot.map_or(State::Keyboard, State::CannotDraw),
        sending: draft.is_some(),
        paint: Paint::Halves(halves),
    }
}

fn half_paint(h: &RgbHalf, dim: u8, colour_effects: Option<u8>) -> HalfPaint {
    if !h.on {
        return HalfPaint::Dark;
    }
    let brightness = (u16::from(h.brightness) * u16::from(dim.min(100)) / 100) as u8;
    paint_card(&CardLook {
        lights: KeyboardLights {
            hue: h.hue,
            sat: h.sat,
            brightness,
            effect: h.effect,
        },
        speed: h.speed,
        in_colour: h.effect < 8 && colour_effects.is_some_and(|m| m & (1 << h.effect) != 0),
    })
}

/// A card pick as paint: moving when it has a scene look, else its colour, still.
fn paint_card(card: &CardLook) -> HalfPaint {
    match scene_look(card) {
        Some(l) => HalfPaint::Moving(l),
        None => {
            let l = &card.lights;
            HalfPaint::Still(shown_color(l.hue, l.sat, l.brightness))
        }
    }
}

impl Look {
    /// When the drawing wants its next frame: `None` for a still look. The page adds its own
    /// stops (minimised, away, not in front).
    pub fn next_frame(&self) -> Option<Duration> {
        let moves = match &self.paint {
            Paint::Outline => false,
            Paint::Halves(h) => h.iter().any(|h| matches!(h, HalfPaint::Moving(_))),
            Paint::Scene(s) => !matches!(s.effect, Effect::Gradient { .. }),
        };
        moves.then_some(PREVIEW_INTERVAL)
    }

    /// One colour per key of every part of `layout` at scene time `t`.
    pub fn colours(&self, layout: &KeyLayout, t: SceneTime) -> Keys {
        let split = layout.parts.len() == 2;
        layout
            .parts
            .iter()
            .enumerate()
            .map(|(p, keys)| self.part(keys, p, split, t))
            .collect()
    }

    fn part(
        &self,
        keys: &[LayoutKey],
        p: usize,
        split: bool,
        t: SceneTime,
    ) -> Vec<Option<Color32>> {
        let id = part_id(p, split);
        match &self.paint {
            Paint::Outline => vec![None; keys.len()],
            Paint::Halves(h) => match &h[p.min(1)] {
                HalfPaint::Dark => vec![Some(Color32::BLACK); keys.len()],
                HalfPaint::Still(c) => vec![Some(*c); keys.len()],
                HalfPaint::Moving(l) => moving(l, keys, id, t),
            },
            Paint::Scene(s) => scene_keys(s, keys.len(), id, t),
        }
    }
}

/// The keyboard's part ids in a room (as `scene_output` finds them).
fn part_id(p: usize, split: bool) -> &'static str {
    match (split, p) {
        (false, _) => "keyboard",
        (true, 0) => "keyboard:left",
        (true, _) => "keyboard:right",
    }
}

fn screen(c: Rgb) -> Color32 {
    super::calibrate::keyboard_screen(Color32::from_rgb(c.0[0], c.0[1], c.0[2]))
}

/// A half's keys as a room of its own, a light on each key's centre (key units).
fn key_room(id: &str, keys: &[LayoutKey]) -> RoomMap {
    let b = keys
        .iter()
        .map(|k| k.bounds())
        .reduce(|a, b| a.union(b))
        .unwrap_or(egui::Rect::ZERO);
    let rect = Rect::new(b.min.x, b.min.y, b.width(), b.height());
    let lights = keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let c = k.rect.center();
            PlacedLight::point(i.to_string(), Point::new(c.x, c.y))
        })
        .collect();
    let part = Placed {
        id: id.to_string(),
        name: id.to_string(),
        kind: foxglove_scene::DeviceKind::Keyboard,
        rect,
        lights,
    };
    RoomMap {
        devices: vec![part],
        bounds: rect,
    }
}

/// A firmware effect over one half's keys at `t`.
fn moving(look: &SceneLook, keys: &[LayoutKey], id: &str, t: SceneTime) -> Vec<Option<Color32>> {
    let Some(room) = card_room(&key_room(id, keys), look.spread) else {
        return vec![None; keys.len()];
    };
    let frame = look.effect.render(&room, t);
    frame
        .lights
        .iter()
        .map(|l| Some(screen(l.colour)))
        .collect()
}

/// A running scene's colours on part `id`'s `n` keys: its lights spread over the keys.
fn scene_keys(s: &SceneRun, n: usize, id: &str, t: SceneTime) -> Vec<Option<Color32>> {
    let frame = s.effect.render(&s.room, t);
    let lit: Vec<Color32> = frame
        .lights
        .iter()
        .filter(|l| l.device == id)
        .map(|l| screen(l.colour))
        .collect();
    (0..n)
        .map(|i| (!lit.is_empty()).then(|| lit[(i * lit.len() / n.max(1)).min(lit.len() - 1)]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::lighting_leases::Priority;
    use crate::foxglove::scene_card::period_ms;

    fn half(effect: u8, hue: u16, brightness: u8) -> RgbHalf {
        RgbHalf {
            on: true,
            hue,
            sat: 100,
            brightness,
            effect,
            speed: 3,
        }
    }

    fn report(left: RgbHalf, right: Option<RgbHalf>) -> Report {
        Report {
            connected: true,
            rgb: Some(RgbState {
                left,
                right: right.map(|h| (h, 0)),
                colour_effects: Some(0x0B),
            }),
            dim: [100, 100],
            windows: None,
        }
    }

    fn lease() -> Lease {
        Lease {
            id: 1,
            conn: 1,
            device: "keyboard".into(),
            priority: Priority::High,
            hue: 0,
            sat: 100,
            brightness: 100,
            effect: "solid".into(),
        }
    }

    fn scene() -> SceneRun {
        let layout = KeyLayout::default();
        let room = RoomMap {
            devices: layout
                .parts
                .iter()
                .enumerate()
                .map(|(p, k)| {
                    let mut d = key_room(part_id(p, layout.parts.len() == 2), k).devices;
                    d.remove(0)
                })
                .collect(),
            bounds: Rect::new(0.0, 0.0, 20.0, 5.0),
        };
        SceneRun {
            name: "Hearth".into(),
            effect: Effect::Sweep {
                stops: vec![Rgb([255, 0, 0]), Rgb([0, 0, 255])],
                angle_deg: 0.0,
                period_ms: 4000,
            },
            room,
        }
    }

    fn at(ms: i64) -> SceneTime {
        SceneTime(ms)
    }

    fn lit(l: &Look, ms: i64) -> Keys {
        l.colours(&KeyLayout::default(), at(ms))
    }

    fn split_layout() -> KeyLayout {
        let d = crate::foxglove::boards::Drawing::default();
        let l = crate::foxglove::key_layout::from_drawing(
            &d,
            true,
            crate::foxglove::key_layout::LayoutSource::Guessed(0),
        );
        assert_eq!(l.parts.len(), 2, "a split board");
        l
    }

    #[test]
    fn no_report_draws_outlines_and_says_why() {
        let r = Report::default();
        let l = look(&r, None, None, None, None);
        assert_eq!(l.state, State::NotConnected);
        assert!(lit(&l, 0).iter().flatten().all(Option::is_none));
        let r = Report {
            connected: true,
            ..Report::default()
        };
        assert_eq!(look(&r, None, None, None, None).state, State::Reading);
        // A lease or a scene can't show before there is a report.
        let (s, ls) = (scene(), lease());
        let l = look(&r, None, Some(("Mic", &ls)), Some(&s), None);
        assert_eq!(l.state, State::Reading);
    }

    #[test]
    fn sources_win_in_the_plans_order() {
        let r = report(half(1, 30, 80), None);
        let (s, ls) = (scene(), lease());
        let owner = Some(Owner::LampArrayUsb);
        let l = look(&r, owner, Some(("Mic muted", &ls)), Some(&s), None);
        assert_eq!(l.state, State::Lease("Mic muted".into()));
        let l = look(&r, owner, None, Some(&s), None);
        assert_eq!(l.state, State::Scene("Hearth".into()));
        // Windows holds: another app by default, Foxglove's own pick through it.
        assert_eq!(look(&r, owner, None, None, None).state, State::AnotherApp);
        let other = look(&r, Some(Owner::Other(9)), None, None, None);
        assert_eq!(other.state, State::AnotherApp);
        let through = Report {
            windows: Some(CardLook {
                lights: KeyboardLights {
                    hue: 200,
                    sat: 100,
                    brightness: 50,
                    effect: 0,
                },
                speed: 3,
                in_colour: false,
            }),
            ..r
        };
        let l = look(&through, owner, None, None, None);
        assert_eq!(l.state, State::ThroughWindows);
        assert_eq!(l.next_frame(), None);
        // The keyboard's own effect last.
        assert_eq!(
            look(&r, Some(Owner::Keyboard), None, None, None).state,
            State::Keyboard
        );
        assert_eq!(look(&r, None, None, None, None).state, State::Keyboard);
    }

    #[test]
    fn solid_off_lease_and_unknown_are_still() {
        let solid = report(half(0, 120, 70), None);
        let l = look(&solid, None, None, None, None);
        assert_eq!(l.next_frame(), None);
        let green = Some(shown_color(120, 100, 70));
        assert!(lit(&l, 0).iter().flatten().all(|c| *c == green));
        assert_eq!(lit(&l, 0), lit(&l, 12_345));

        let mut h = half(1, 0, 70);
        h.on = false;
        let off = look(&report(h, Some(h)), None, None, None, None);
        assert_eq!(off.next_frame(), None);
        assert!(lit(&off, 0)
            .iter()
            .flatten()
            .all(|c| *c == Some(Color32::BLACK)));

        let ls = lease();
        let l = look(&solid, None, Some(("Mic", &ls)), None, None);
        assert_eq!(l.next_frame(), None);
        assert_eq!(lit(&l, 0), lit(&l, 999));

        let unknown = look(&report(half(7, 40, 60), None), None, None, None, None);
        assert_eq!(unknown.state, State::CannotDraw(7));
        assert_eq!(unknown.next_frame(), None);
        assert_eq!(lit(&unknown, 0), lit(&unknown, 999));
        assert!(lit(&unknown, 0).iter().flatten().all(Option::is_some));

        let outline = look(&Report::default(), None, None, None, None);
        assert_eq!(outline.next_frame(), None);
    }

    #[test]
    fn breathe_spectrum_and_swirl_move_at_the_keyboards_pace() {
        for effect in 1..=3u8 {
            let l = look(&report(half(effect, 0, 100), None), None, None, None, None);
            assert_eq!(l.next_frame(), Some(PREVIEW_INTERVAL), "effect {effect}");
            let period = i64::from(period_ms(effect, 3));
            assert!(period > 0);
            assert_ne!(lit(&l, 0), lit(&l, period / 2), "effect {effect} moves");
            assert_eq!(
                lit(&l, 0),
                lit(&l, period),
                "effect {effect} repeats each period"
            );
        }
    }

    #[test]
    fn swirl_runs_across_the_keys_and_spectrum_holds_them_together() {
        let swirl = look(&report(half(3, 0, 100), None), None, None, None, None);
        let keys = &lit(&swirl, 0)[0];
        assert!(
            keys.iter().any(|c| *c != keys[0]),
            "swirl differs along the keys"
        );
        let spectrum = look(&report(half(2, 0, 100), None), None, None, None, None);
        let keys = &lit(&spectrum, 0)[0];
        assert!(
            keys.iter().all(|c| *c == keys[0]),
            "spectrum is one colour at a time"
        );
    }

    #[test]
    fn halves_are_drawn_apart() {
        let r = report(half(0, 30, 60), Some(half(0, 240, 40)));
        let l = look(&r, None, None, None, None);
        let keys = l.colours(&split_layout(), at(0));
        assert_eq!(keys[0][0], Some(shown_color(30, 100, 60)));
        assert_eq!(keys[1][0], Some(shown_color(240, 100, 40)));
        // With no data from the right half it follows the left.
        let follows = look(&report(half(0, 30, 60), None), None, None, None, None);
        let keys = follows.colours(&split_layout(), at(0));
        assert_eq!(keys[0][0], keys[1][0]);
    }

    #[test]
    fn a_dimmed_half_is_drawn_dimmer() {
        let mut r = report(half(0, 30, 100), Some(half(0, 30, 100)));
        r.dim = [100, 30];
        let keys = look(&r, None, None, None, None).colours(&split_layout(), at(0));
        assert_eq!(keys[0][0], Some(shown_color(30, 100, 100)));
        assert_eq!(keys[1][0], Some(shown_color(30, 100, 30)));
        assert_ne!(keys[0][0], keys[1][0]);
    }

    #[test]
    fn a_running_scene_draws_its_clock_on_the_keys() {
        let (s, r) = (scene(), report(half(0, 0, 100), None));
        let l = look(&r, None, None, Some(&s), None);
        assert_eq!(l.next_frame(), Some(PREVIEW_INTERVAL));
        assert_ne!(lit(&l, 0), lit(&l, 1000));
        assert_eq!(lit(&l, 0), lit(&l, 4000));
        let still = SceneRun {
            effect: Effect::Gradient {
                stops: vec![Rgb([9, 9, 9])],
                angle_deg: 0.0,
            },
            ..s
        };
        assert_eq!(look(&r, None, None, Some(&still), None).next_frame(), None);
    }

    #[test]
    fn a_draft_shows_before_the_keyboard_answers() {
        let r = report(half(0, 30, 60), None);
        let d = Draft {
            left: Some(half(0, 200, 90)),
            right: None,
        };
        let l = look(&r, None, None, None, Some(&d));
        assert!(l.sending);
        let keys = l.colours(&split_layout(), at(0));
        assert_eq!(keys[0][0], Some(shown_color(200, 100, 90)));
        assert!(!look(&r, None, None, None, None).sending);
    }

    #[test]
    fn no_effect_flashes_faster_than_a_cycle_in_two_point_four_seconds() {
        // Breathe is the one that fades to dark: at its fastest it takes 2.4 s a cycle, so
        // far under three flashes a second. The hue turns are slower than a second a turn.
        let fastest = |effect| (1..=5).map(|s| period_ms(effect, s)).min().unwrap();
        assert!(fastest(1) >= 2400, "breathe {}", fastest(1));
        assert!(fastest(2) >= 1800 && fastest(3) >= 1800);
    }
}
