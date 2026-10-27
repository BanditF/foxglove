//! Scene frames to devices (#485): what each output gets from a rendered frame, and when.
//!
//! - The OMEN case (and any catalog board with a driver): one colour per zone, at the zone's
//!   place in the room. Written through the Lights worker's drivers, so the colour calibration
//!   applies at the driver boundary as for every other write.
//! - The keyboard: a frame of both halves' LEDs (#529), each half its own colour, or each LED
//!   its own once the board profile places them; and the whole frame as one colour (the
//!   halves mixed) for firmware without SET_FRAME, which takes one `&rgb_ug` colour.
//!
//! Each output has its own interval (never faster than the scheduler's 30 fps cap): the case
//! [`CASE_INTERVAL`], the keyboard [`KEYBOARD_INTERVAL`], since its colour goes over the host
//! link and on to the right half over BLE through the relay. A device gets a frame only when
//! its colours changed. This is pure: the app's `scene_run` sends what it returns.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use foxglove_scene::scheduler::Pacing;
use foxglove_scene::{Burst, Bursts, Effect, Rgb, RoomMap, SceneTime};

/// The keyboard's output id in [`Pacing`].
pub const KEYBOARD: &str = "keyboard";

/// LEDs per keyboard half in a frame (the Eyelash Corne's strip, SET_FRAME's lamps 0-20 and
/// 21-41).
pub const HALF_LAMPS: usize = digitalis::protocol::frame::HALF_LAMPS;

/// The keyboard gets at most 10 colours a second (host link, then BLE to the right half).
pub const KEYBOARD_INTERVAL: Duration = Duration::from_millis(100);

/// A case board gets at most 15 frames a second (one 59-byte report per changed zone).
pub const CASE_INTERVAL: Duration = Duration::from_millis(66);

/// A board the Lights worker drives: its key and each zone's id, in the driver's zone order.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseTarget {
    pub key: String,
    pub zone_ids: Vec<String>,
}

impl CaseTarget {
    /// The room map's id for it (`scene_room`'s).
    fn room_id(&self) -> String {
        format!("light:{}", self.key)
    }
}

/// One write to send.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Every zone of a board (index into its zones, sRGB), when any of them changed.
    Case {
        key: String,
        colors: Vec<(usize, [u8; 3])>,
    },
    /// The keyboard's frame.
    Keyboard(KeyboardFrame),
}

/// One frame for the keyboard.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardFrame {
    /// Both halves' LEDs in strip order: [`HALF_LAMPS`] for the left, then the right's.
    pub lamps: Vec<Rgb>,
    /// All of it as one colour, for firmware without SET_FRAME.
    pub mix: Hsb,
}

/// The room id of a keyboard light's half: the left's LEDs come first in a frame.
fn half_of(device: &str) -> Option<usize> {
    match device {
        "keyboard:left" => Some(0),
        "keyboard:right" => Some(1),
        _ => None,
    }
}

/// The keyboard's lights in a rendered frame as one SET_FRAME frame. A light whose id is an LED
/// number (the board profile's LED map) colours that LED; any other light (a half lit as one)
/// colours every LED of its half that no numbered light covers. A one-piece keyboard, or a
/// half missing from the room, gets the mix of everything.
fn keyboard_frame(lights: &[(&str, &str, Rgb)]) -> Option<KeyboardFrame> {
    let all: Vec<Rgb> = lights.iter().map(|(_, _, c)| *c).collect();
    let mix = mixed(&all)?;
    let mut lamps = vec![mix; HALF_LAMPS * 2];
    for half in 0..2 {
        let own: Vec<(&str, Rgb)> = lights
            .iter()
            .filter(|(d, _, _)| half_of(d) == Some(half))
            .map(|(_, l, c)| (*l, *c))
            .collect();
        let whole: Vec<Rgb> = own
            .iter()
            .filter(|(l, _)| l.parse::<usize>().is_err())
            .map(|(_, c)| *c)
            .collect();
        let fill =
            mixed(&whole).or_else(|| mixed(&own.iter().map(|(_, c)| *c).collect::<Vec<_>>()));
        let Some(fill) = fill else {
            continue;
        };
        let slots = &mut lamps[half * HALF_LAMPS..(half + 1) * HALF_LAMPS];
        slots.fill(fill);
        for (l, c) in &own {
            if let Some(i) = l.parse::<usize>().ok().filter(|i| *i < HALF_LAMPS) {
                slots[i] = *c;
            }
        }
    }
    Some(KeyboardFrame {
        lamps,
        mix: hsb(mix),
    })
}

/// Hue 0-359, saturation and brightness 0-100, as the keyboard takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hsb {
    pub hue: u16,
    pub sat: u8,
    pub brightness: u8,
}

/// An sRGB colour as the keyboard's hue, saturation and brightness.
pub fn hsb(c: Rgb) -> Hsb {
    let [r, g, b] = c.0.map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let hue = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let sat = if max <= f32::EPSILON { 0.0 } else { d / max };
    Hsb {
        hue: (hue.round() as u16) % 360,
        sat: (sat * 100.0).round().clamp(0.0, 100.0) as u8,
        brightness: (max * 100.0).round().clamp(0.0, 100.0) as u8,
    }
}

/// Several lights as one colour: their mix in Oklab, each counted once.
pub(crate) fn mixed(colours: &[Rgb]) -> Option<Rgb> {
    let (first, rest) = colours.split_first()?;
    Some(
        rest.iter()
            .enumerate()
            .fold(*first, |acc, (i, c)| acc.mix(*c, 1.0 / (i as f32 + 2.0))),
    )
}

/// One running scene's outputs: renders a frame for the devices that are due and sends only
/// what changed.
pub struct Outputs {
    effect: Effect,
    room: RoomMap,
    cases: Vec<CaseTarget>,
    keyboard: bool,
    pacing: Pacing,
    sent_case: BTreeMap<String, Vec<(usize, [u8; 3])>>,
    sent_keyboard: Option<KeyboardFrame>,
    /// Set from elsewhere (the keyboard handed its frames back): send the keyboard's next frame
    /// in full even though nothing changed.
    keyboard_again: Arc<AtomicBool>,
    /// Rings and sweeps over the scene (#489), drawn by these same frames.
    bursts: Bursts,
}

impl Outputs {
    pub fn new(effect: Effect, room: RoomMap, cases: Vec<CaseTarget>) -> Self {
        let mut o = Outputs {
            effect,
            room: RoomMap::default(),
            cases: Vec::new(),
            keyboard: false,
            pacing: Pacing::default(),
            sent_case: BTreeMap::new(),
            sent_keyboard: None,
            keyboard_again: Arc::default(),
            bursts: Bursts::default(),
        };
        o.set_room(room, cases);
        o
    }

    /// A ring or sweep over the scene from its next frame on (at most four at once).
    pub fn burst(&mut self, b: Burst) {
        self.bursts.add(b);
    }

    /// The room the scene is placed in now.
    pub fn room(&self) -> &RoomMap {
        &self.room
    }

    /// The scene's effect now.
    pub fn effect(&self) -> &Effect {
        &self.effect
    }

    /// A flag that sends the keyboard its next frame again, set from any thread: SET_FRAME's
    /// hand-back event (a static scene's frame timed out), the scene's lease showing again, the
    /// user coming back. Only the scheduler's frames send it, so nothing goes out while away or
    /// paused.
    pub fn keyboard_again(&self) -> Arc<AtomicBool> {
        self.keyboard_again.clone()
    }

    /// The devices changed (one arrived, went or was found again): the new room. Every device
    /// gets its next frame in full, since one that came back shows its saved colours again.
    pub fn set_room(&mut self, room: RoomMap, cases: Vec<CaseTarget>) {
        let cases: Vec<CaseTarget> = cases
            .into_iter()
            .filter(|c| room.device(&c.room_id()).is_some())
            .collect();
        self.keyboard = room
            .devices
            .iter()
            .any(|d| d.id.starts_with(KEYBOARD) && !d.lights.is_empty());
        let mut paced: Vec<(String, Option<Duration>)> = cases
            .iter()
            .map(|c| (c.room_id(), Some(CASE_INTERVAL)))
            .collect();
        if self.keyboard {
            paced.push((KEYBOARD.into(), Some(KEYBOARD_INTERVAL)));
        }
        self.pacing = Pacing::new(paced);
        self.sent_case.clear();
        self.sent_keyboard = None;
        self.room = room;
        self.cases = cases;
    }

    /// Another look while the scene runs (the Scenes page, #487): every device gets the next
    /// frame in full.
    pub fn set_effect(&mut self, effect: Effect) {
        self.effect = effect;
        self.sent_case.clear();
        self.sent_keyboard = None;
    }

    /// The writes for scene time `t`: each due device whose colours changed.
    pub fn frame(&mut self, t: SceneTime) -> Vec<Out> {
        if self.keyboard_again.swap(false, Ordering::Relaxed) {
            self.sent_keyboard = None;
        }
        let due = self.pacing.due(t);
        if due.is_empty() {
            return Vec::new();
        }
        let mut frame = self.effect.render(&self.room, t);
        if self.bursts.prune(t) {
            // `render` lists the lights device by device, in the room's order.
            let places = self.room.devices.iter().flat_map(|d| d.lights.iter());
            for (l, place) in frame.lights.iter_mut().zip(places) {
                l.colour = self.bursts.over(l.colour, place.at, t, &self.room.bounds);
            }
        }
        let mut out = Vec::new();
        for id in due {
            if id == KEYBOARD {
                let lights: Vec<(&str, &str, Rgb)> = frame
                    .lights
                    .iter()
                    .filter(|l| l.device.starts_with(KEYBOARD))
                    .map(|l| (l.device.as_str(), l.light.as_str(), l.colour))
                    .collect();
                let Some(f) = keyboard_frame(&lights) else {
                    continue;
                };
                if self.sent_keyboard.as_ref() != Some(&f) {
                    self.sent_keyboard = Some(f.clone());
                    out.push(Out::Keyboard(f));
                }
                continue;
            }
            let Some(case) = self.cases.iter().find(|c| c.room_id() == id) else {
                continue;
            };
            let mut colors: Vec<(usize, [u8; 3])> = frame
                .lights
                .iter()
                .filter(|l| l.device == id)
                .filter_map(|l| {
                    let i = case.zone_ids.iter().position(|z| *z == l.light)?;
                    Some((i, l.colour.0))
                })
                .collect();
            colors.sort_by_key(|(i, _)| *i);
            if colors.is_empty() || self.sent_case.get(&case.key) == Some(&colors) {
                continue;
            }
            self.sent_case.insert(case.key.clone(), colors.clone());
            out.push(Out::Case {
                key: case.key.clone(),
                colors,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_scene::{DeviceInfo, DeviceKind, LightSpec, MonitorInfo, Point};

    const ORANGE: Rgb = Rgb([0xf0, 0x8a, 0x3c]);

    /// The user's desk: one 1440p monitor, the Corne's halves and the 45L with its catalog
    /// zones, as `scene_room` gathers them.
    fn desk() -> (RoomMap, Vec<CaseTarget>) {
        let monitors = [MonitorInfo {
            id: r"\\.\DISPLAY1".into(),
            primary: true,
            rect_px: [0, 0, 2560, 1440],
            raw_dpi: Some(109),
            dpi: 96,
        }];
        let zone = |id: &str, x: f32, y: f32| LightSpec {
            id: id.into(),
            at: Some(Point::new(x, y)),
        };
        let devices = [
            DeviceInfo::whole("keyboard:left", "Corne left", DeviceKind::KeyboardLeft),
            DeviceInfo::whole("keyboard:right", "Corne right", DeviceKind::KeyboardRight),
            DeviceInfo {
                id: "light:103c:84fd".into(),
                name: "OMEN 45L".into(),
                kind: DeviceKind::Case,
                size: Some(Point::new(205.0, 475.0)),
                // The catalog's order: 1, 2, 5, 6, 7.
                lights: vec![
                    zone("1", 102.0, 55.0),
                    zone("2", 120.0, 250.0),
                    zone("5", 102.0, 405.0),
                    zone("6", 102.0, 285.0),
                    zone("7", 102.0, 165.0),
                ],
            },
        ];
        let room = RoomMap::build(&monitors, &devices, &Default::default());
        let cases = vec![CaseTarget {
            key: "103c:84fd".into(),
            zone_ids: ["1", "2", "5", "6", "7"].map(String::from).to_vec(),
        }];
        (room, cases)
    }

    fn scanline() -> Effect {
        Effect::Scanline {
            colour: ORANGE,
            background: Rgb::BLACK,
            angle_deg: 90.0,
            width_mm: 80.0,
            period_ms: 4000,
        }
    }

    #[test]
    fn colours_convert_to_the_keyboards_hsb() {
        let hb = |r, g, b| {
            let h = hsb(Rgb([r, g, b]));
            (h.hue, h.sat, h.brightness)
        };
        assert_eq!(hb(255, 0, 0), (0, 100, 100));
        assert_eq!(hb(0, 255, 0), (120, 100, 100));
        assert_eq!(hb(0, 0, 255), (240, 100, 100));
        assert_eq!(hb(0, 0, 0), (0, 0, 0));
        assert_eq!(hb(255, 255, 255), (0, 0, 100));
        assert_eq!(hb(0xf0, 0x8a, 0x3c), (26, 75, 94));
        assert_eq!(hb(255, 0, 128).0, 330);
    }

    /// The live check's sweep: top to bottom the case's front fans 7, 6, 5, then the keys.
    #[test]
    fn a_scanline_runs_down_the_case_then_reaches_the_keyboard() {
        let (room, cases) = desk();
        let mut o = Outputs::new(scanline(), room, cases);
        // Every 10 ms of one period: when does each output first show the band's peak?
        let mut first_lit: BTreeMap<String, i64> = BTreeMap::new();
        for ms in (0..4000).step_by(10) {
            for out in o.frame(SceneTime(ms)) {
                match out {
                    Out::Case { colors, .. } => {
                        for (i, c) in colors {
                            if c[0] > 200 {
                                let zone = ["1", "2", "5", "6", "7"][i];
                                first_lit.entry(format!("zone {zone}")).or_insert(ms);
                            }
                        }
                    }
                    Out::Keyboard(f) => {
                        if f.mix.brightness > 80 {
                            first_lit.entry("keyboard".into()).or_insert(ms);
                        }
                    }
                }
            }
        }
        let at = |k: &str| {
            first_lit
                .get(k)
                .copied()
                .unwrap_or_else(|| panic!("{k}: {first_lit:?}"))
        };
        assert!(at("zone 7") < at("zone 6"), "{first_lit:?}");
        assert!(at("zone 6") < at("zone 5"), "{first_lit:?}");
        assert!(at("zone 5") < at("keyboard"), "{first_lit:?}");
    }

    #[test]
    fn each_device_at_its_own_rate_and_only_on_change() {
        let (room, cases) = desk();
        let mut o = Outputs::new(scanline(), room, cases);
        let (mut case_t, mut kb_t) = (Vec::new(), Vec::new());
        // Two seconds at the scheduler's fastest tick.
        for ms in (0..2000).step_by(33) {
            for out in o.frame(SceneTime(ms)) {
                match out {
                    Out::Case { .. } => case_t.push(ms),
                    Out::Keyboard(_) => kb_t.push(ms),
                }
            }
        }
        let min_gap = |t: &[i64]| t.windows(2).map(|w| w[1] - w[0]).min().unwrap_or(i64::MAX);
        assert!(min_gap(&case_t) >= 60, "case {case_t:?}");
        assert!(min_gap(&kb_t) >= 90, "keyboard {kb_t:?}");
        assert!(kb_t.len() <= 21, "at most 10 a second: {}", kb_t.len());
        assert!(!case_t.is_empty() && !kb_t.is_empty());

        // A still scene: one write each, then nothing.
        let (room, cases) = desk();
        let still = Effect::Gradient {
            stops: vec![ORANGE, Rgb([0, 0, 255])],
            angle_deg: 90.0,
        };
        let mut o = Outputs::new(still, room, cases);
        let writes: usize = (0..60).map(|i| o.frame(SceneTime(i * 33)).len()).sum();
        assert_eq!(writes, 2, "the case once, the keyboard once");
        // The case was found again (a replug puts its saved colours back): drawn again.
        let (room, cases) = desk();
        o.set_room(room, cases);
        assert_eq!(o.frame(SceneTime(2000)).len(), 2);
        // Another look from the Scenes page (#487): every device gets it at its next turn.
        assert!(o.frame(SceneTime(2100)).is_empty(), "still");
        o.set_effect(Effect::Gradient {
            stops: vec![Rgb([0, 0, 255]), ORANGE],
            angle_deg: 90.0,
        });
        assert_eq!(o.frame(SceneTime(2200)).len(), 2);
        // The same look again still redraws (a profile switch may send the same one).
        o.set_effect(Effect::Gradient {
            stops: vec![Rgb([0, 0, 255]), ORANGE],
            angle_deg: 90.0,
        });
        assert_eq!(o.frame(SceneTime(2400)).len(), 2);
    }

    #[test]
    fn a_case_frame_names_every_zone_and_the_keyboard_gets_both_halves() {
        let (room, cases) = desk();
        let still = Effect::Breathe {
            from: ORANGE,
            to: ORANGE,
            period_ms: 1000,
        };
        let mut o = Outputs::new(still, room, cases);
        let outs = o.frame(SceneTime(0));
        assert_eq!(
            outs,
            [
                Out::Case {
                    key: "103c:84fd".into(),
                    colors: (0..5).map(|i| (i, ORANGE.0)).collect(),
                },
                Out::Keyboard(KeyboardFrame {
                    lamps: vec![ORANGE; 2 * HALF_LAMPS],
                    mix: hsb(ORANGE),
                }),
            ]
        );
        let a = Rgb([255, 0, 0]);
        assert_eq!(mixed(&[a, a, a]), Some(a));
        assert_eq!(mixed(&[]), None);
    }

    #[test]
    fn each_half_gets_its_own_colour_and_numbered_leds_their_own() {
        let (red, blue, green) = (Rgb([255, 0, 0]), Rgb([0, 0, 255]), Rgb([0, 255, 0]));
        // Per half (the default): the left's 21 LEDs red, the right's blue, one colour mixed.
        let f = keyboard_frame(&[
            ("keyboard:left", "all", red),
            ("keyboard:right", "all", blue),
        ])
        .unwrap();
        assert_eq!(f.lamps[..HALF_LAMPS], [red; HALF_LAMPS]);
        assert_eq!(f.lamps[HALF_LAMPS..], [blue; HALF_LAMPS]);
        assert_eq!(f.mix, hsb(red.mix(blue, 0.5)));
        // Per LED (a board profile with an LED map): each numbered light its LED; LEDs the map
        // leaves out get the half's mix.
        let mut lights: Vec<(&str, &str, Rgb)> = Vec::new();
        let ids: Vec<String> = (0..HALF_LAMPS).map(|i| i.to_string()).collect();
        for (i, id) in ids.iter().enumerate().take(20) {
            lights.push(("keyboard:left", id, if i == 3 { green } else { red }));
        }
        lights.push(("keyboard:right", "all", blue));
        let f = keyboard_frame(&lights).unwrap();
        assert_eq!(f.lamps[3], green);
        assert_eq!(f.lamps[0], red);
        let left: Vec<Rgb> = lights
            .iter()
            .filter(|(d, _, _)| *d == "keyboard:left")
            .map(|(_, _, c)| *c)
            .collect();
        assert_eq!(f.lamps[20], mixed(&left).unwrap(), "the half's mix");
        assert_eq!(f.lamps[HALF_LAMPS], blue);
        // A one-piece keyboard (or one half placed): the mix everywhere.
        let f = keyboard_frame(&[("keyboard", "all", green)]).unwrap();
        assert_eq!(f.lamps, vec![green; 2 * HALF_LAMPS]);
        assert!(keyboard_frame(&[]).is_none());
    }

    #[test]
    fn the_keyboard_is_sent_again_when_asked() {
        let (room, cases) = desk();
        let still = Effect::Gradient {
            stops: vec![ORANGE, Rgb([0, 0, 255])],
            angle_deg: 90.0,
        };
        let mut o = Outputs::new(still, room, cases);
        let kb = |outs: Vec<Out>| {
            outs.iter()
                .filter(|o| matches!(o, Out::Keyboard(_)))
                .count()
        };
        assert_eq!(kb(o.frame(SceneTime(0))), 1);
        assert_eq!(kb(o.frame(SceneTime(200))), 0, "a still scene: once");
        // The keyboard handed its frames back (its timeout): the same frame again, once.
        o.keyboard_again().store(true, Ordering::Relaxed);
        assert_eq!(kb(o.frame(SceneTime(400))), 1);
        assert_eq!(kb(o.frame(SceneTime(600))), 0);
    }

    /// A key ripple (#489) over a still scene: the keyboard changes while the ring passes, then
    /// gets the scene's own colours back, and nothing is sent after that.
    #[test]
    fn a_ripple_draws_over_the_scene_and_then_gives_way() {
        use foxglove_scene::burst::Shape;
        let (room, cases) = desk();
        let still = Effect::Gradient {
            stops: vec![ORANGE, Rgb([0, 0, 255])],
            angle_deg: 90.0,
        };
        let key = room
            .devices
            .iter()
            .find(|d| d.id == "keyboard:left")
            .unwrap()
            .rect
            .centre();
        let mut o = Outputs::new(still, room, cases);
        let frames = |outs: Vec<Out>| -> Vec<KeyboardFrame> {
            outs.into_iter()
                .filter_map(|o| match o {
                    Out::Keyboard(f) => Some(f),
                    _ => None,
                })
                .collect()
        };
        let before = frames(o.frame(SceneTime(0)));
        assert_eq!(before.len(), 1);
        o.burst(Burst {
            shape: Shape::Ring { origin: key },
            colour: Rgb([255, 255, 255]),
            start: SceneTime(200),
            duration_ms: 1000,
            width_mm: 60.0,
        });
        let during = frames(o.frame(SceneTime(200)));
        assert_eq!(during.len(), 1, "the ring changes the keyboard");
        assert_ne!(during[0], before[0]);
        let after = frames(o.frame(SceneTime(1400)));
        assert_eq!(after, before, "the scene's colours again once it is gone");
        assert!(frames(o.frame(SceneTime(1600))).is_empty());
    }

    #[test]
    fn a_device_that_goes_drops_out_and_no_keyboard_means_no_keyboard_frames() {
        let (room, cases) = desk();
        let mut o = Outputs::new(scanline(), room.clone(), cases.clone());
        assert!(!o.frame(SceneTime(0)).is_empty());
        // The case went: only the keyboard is left.
        let mut no_case = room.clone();
        no_case.devices.retain(|d| !d.id.starts_with("light:"));
        o.set_room(no_case, cases.clone());
        for ms in (100..1000).step_by(100) {
            assert!(o
                .frame(SceneTime(ms))
                .iter()
                .all(|out| matches!(out, Out::Keyboard(_))));
        }
        // No keyboard in the room (no board profile and nothing placed): no keyboard frames.
        let mut no_kb = room;
        no_kb.devices.retain(|d| !d.id.starts_with(KEYBOARD));
        let mut o = Outputs::new(scanline(), no_kb, cases);
        assert!(o
            .frame(SceneTime(0))
            .iter()
            .all(|out| matches!(out, Out::Case { .. })));
    }
}
