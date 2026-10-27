//! Scene frames to devices (#485): what each output gets from a rendered frame, and when.
//!
//! - The OMEN case (and any catalog board with a driver): one colour per zone, at the zone's
//!   place in the room. Written through the Lights worker's drivers, so the colour calibration
//!   applies at the driver boundary as for every other write.
//! - The keyboard: one colour for the whole board, the halves' colours mixed (the firmware
//!   takes one `&rgb_ug` colour for both halves; per-LED frames come with SET_FRAME, #272).
//!
//! Each output has its own interval (never faster than the scheduler's 30 fps cap): the case
//! [`CASE_INTERVAL`], the keyboard [`KEYBOARD_INTERVAL`], since its colour goes over the host
//! link and on to the right half over BLE through the relay. A device gets a frame only when
//! its colours changed. This is pure: the app's `scene_run` sends what it returns.

use std::collections::BTreeMap;
use std::time::Duration;

use foxglove_scene::scheduler::Pacing;
use foxglove_scene::{Effect, Rgb, RoomMap, SceneTime};

/// The keyboard's output id in [`Pacing`].
pub const KEYBOARD: &str = "keyboard";

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
    /// The keyboard's colour: hue 0-359, saturation and brightness 0-100.
    Keyboard(Hsb),
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
    sent_keyboard: Option<Hsb>,
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
        };
        o.set_room(room, cases);
        o
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
        let due = self.pacing.due(t);
        if due.is_empty() {
            return Vec::new();
        }
        let frame = self.effect.render(&self.room, t);
        let mut out = Vec::new();
        for id in due {
            if id == KEYBOARD {
                let halves: Vec<Rgb> = frame
                    .lights
                    .iter()
                    .filter(|l| l.device.starts_with(KEYBOARD))
                    .map(|l| l.colour)
                    .collect();
                let Some(c) = mixed(&halves).map(hsb) else {
                    continue;
                };
                if self.sent_keyboard != Some(c) {
                    self.sent_keyboard = Some(c);
                    out.push(Out::Keyboard(c));
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
                    Out::Keyboard(h) => {
                        if h.brightness > 80 {
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
    fn a_case_frame_names_every_zone_and_the_halves_mix() {
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
                Out::Keyboard(hsb(ORANGE)),
            ]
        );
        let a = Rgb([255, 0, 0]);
        assert_eq!(mixed(&[a, a, a]), Some(a));
        assert_eq!(mixed(&[]), None);
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
