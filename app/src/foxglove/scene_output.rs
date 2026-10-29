//! Scene frames to devices (#485): what each output gets from a rendered frame, and when.
//!
//! - The OMEN case (and any catalog board with a driver): one colour per zone, at the zone's
//!   place in the room. Written through the Lights worker's drivers, so the colour calibration
//!   applies at the driver boundary as for every other write.
//! - The keyboard: with SET_EFFECT (HELLO bit 19, #542) the scene's effect itself, sent once:
//!   each half draws it from its LEDs' places on the scene's clock ([`keyboard_effect`]), so no
//!   keyboard frames go out while it runs. Scanline, sweep (1-4 stops), ripple and breathe map
//!   one to one; gradient and noise, and any ripple or sweep burst drawn over the scene (#489),
//!   take the frames path while they show. Else a frame of every part's LEDs (#529), each part
//!   its own colour, or each LED its own once the board profile places them; and the whole
//!   frame as one colour (the parts mixed) for firmware without SET_FRAME, which takes one
//!   `&rgb_ug` colour. How many LEDs each part has is the board's ([`LampParts`], #561).
//!
//! - Dynamic Lighting devices (any LampArray Windows lists, #503): a colour per lamp, each lamp
//!   at the place its own attributes give it in the room; one colour for all until those are
//!   read. Nothing here depends on the vendor or the kind: the room map places the device by its
//!   LampArray kind, and the writer (`scene_lamps`) goes through `Windows.Devices.Lights`.
//!
//! Each output has its own interval (never faster than the scheduler's 30 fps cap): the case
//! [`CASE_INTERVAL`], the keyboard [`KEYBOARD_INTERVAL`], since its colour goes over the host
//! link and on to the right half over BLE through the relay. A device gets a frame only when
//! its colours changed. This is pure: the app's `scene_run` sends what it returns.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use foxglove_link::lamps::LampParts;
use foxglove_link::protocol::MmRect;
use foxglove_scene::scheduler::Pacing;
use foxglove_scene::{Ambient, Burst, Bursts, Effect, LightRole, Rgb, RoomMap, SceneTime};

/// The keyboard's output id in [`Pacing`].
pub const KEYBOARD: &str = "keyboard";

/// The keyboard gets at most 10 colours a second (host link, then BLE to the right half).
pub const KEYBOARD_INTERVAL: Duration = Duration::from_millis(100);

/// A case board gets at most 15 frames a second (one 59-byte report per changed zone).
pub const CASE_INTERVAL: Duration = Duration::from_millis(66);

/// A Dynamic Lighting device gets at most 20 frames a second, however fast its attributes say
/// it takes them: each frame crosses Windows' lighting service and often a vendor's (#313).
pub const LAMP_INTERVAL: Duration = Duration::from_millis(50);

/// A Dynamic Lighting (LampArray) device a scene drives through Windows: any vendor, any kind.
#[derive(Clone, Debug, PartialEq)]
pub struct LampTarget {
    /// The Lights page's key (`vvvv:pppp`, maybe with a serial).
    pub key: String,
    /// Its lamp count; 0 until Windows' attributes are read (then one colour for all).
    pub lamps: usize,
    /// The shortest update interval its attributes give.
    pub min_interval: Option<Duration>,
}

impl LampTarget {
    fn room_id(&self) -> String {
        format!("light:{}", self.key)
    }

    /// Its own interval, never under [`LAMP_INTERVAL`].
    pub fn interval(&self) -> Duration {
        self.min_interval
            .map_or(LAMP_INTERVAL, |m| m.max(LAMP_INTERVAL))
    }
}

/// A Dynamic Lighting device's colours from its lights in a frame: one per lamp in index order.
/// A numbered light colours that lamp; lamps no numbered light reaches (or a device still lit
/// as one) take the mix of all. Unknown lamp count: one colour for the whole device.
pub(crate) fn lamp_colours(target: &LampTarget, lights: &[(&str, Rgb)]) -> Option<Vec<[u8; 3]>> {
    let all: Vec<Rgb> = lights.iter().map(|(_, c)| *c).collect();
    let mix = mixed(&all)?;
    if target.lamps == 0 {
        return Some(vec![mix.0]);
    }
    let mut out = vec![mix.0; target.lamps];
    for (l, c) in lights {
        if let Some(i) = l.parse::<usize>().ok().filter(|i| *i < target.lamps) {
            out[i] = c.0;
        }
    }
    Some(out)
}

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
    /// The keyboard draws the scene itself from now on (SET_EFFECT).
    KeyboardEffect(KeyboardEffect),
    /// A Dynamic Lighting device's colours, one per lamp (or one for all, see [`lamp_colours`]).
    Lamps { key: String, colors: Vec<[u8; 3]> },
}

/// The scene as one SET_EFFECT (#542): each part draws it from its LEDs' places.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardEffect {
    /// In the wire's units. `clock_ms` is 0 here: the writer fills it in from the scene's clock
    /// just before it goes ([`KeyboardEffect::at`]), so the parts run in phase with the case
    /// and the wallpapers.
    pub effect: foxglove_link::protocol::Effect,
    /// The room's bounds (mm), for EFFECT PLACE.
    pub room: MmRect,
    /// Each part's place in the room (mm), in frame order: the left half then the right, or
    /// the one piece.
    pub parts: Vec<MmRect>,
    /// The effect's colours as one, for the lease (what shows when the effect is handed back).
    pub mix: Hsb,
    /// Each part's LEDs where the room has them, in strip order and thousandths of the part
    /// (EFFECT LAMPS, #550), when every LED of every part is placed; `None` leaves the
    /// firmware's built-in table.
    pub lamps: Option<Vec<Vec<[u16; 2]>>>,
}

impl KeyboardEffect {
    /// EFFECT PLACE's two rectangles: the left and right halves, or the one piece in both.
    pub fn place(&self) -> (MmRect, MmRect) {
        let first = self.parts.first().copied().unwrap_or_default();
        (first, self.parts.get(1).copied().unwrap_or(first))
    }

    /// The effect to send at scene time `t`: its clock is `t` modulo the period.
    pub fn at(&self, t: SceneTime) -> foxglove_link::protocol::Effect {
        let period = i64::from(self.effect.period_ms);
        let clock_ms = if period == 0 {
            0
        } else {
            t.ms().rem_euclid(period) as u32
        };
        foxglove_link::protocol::Effect {
            clock_ms,
            ..self.effect.clone()
        }
    }
}

/// Room millimetres as the wire carries them: whole, within i16.
fn mm(v: f32) -> i16 {
    v.round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

fn mm_rect(r: &foxglove_scene::Rect) -> MmRect {
    [mm(r.x), mm(r.y), mm(r.w), mm(r.h)]
}

/// The keyboard's parts in the room, in frame order: the left and right halves, or the one
/// piece. `None` without a keyboard (or with one half only).
fn keyboard_parts(room: &RoomMap) -> Option<Vec<&foxglove_scene::room::Placed>> {
    let device = |id: &str| room.devices.iter().find(|d| d.id == id);
    if let (Some(l), Some(r)) = (device("keyboard:left"), device("keyboard:right")) {
        return Some(vec![l, r]);
    }
    device(KEYBOARD).map(|d| vec![d])
}

/// One part's LEDs as EFFECT LAMPS points: each numbered light ("0", "1", ...) of the placed
/// part, in thousandths of the part from its top-left corner. An LED without a light is one
/// that lights nothing (#608): it gets the part's centre, which no one sees. `None` unless every
/// light of the part is one of its `count` LEDs (one light per part: no map).
fn part_lamps(d: &foxglove_scene::room::Placed, count: usize) -> Option<Vec<[u16; 2]>> {
    let numbered =
        |l: &&foxglove_scene::room::PlacedLight| l.id.parse::<usize>().is_ok_and(|i| i < count);
    if count == 0
        || d.lights.is_empty()
        || !d.lights.iter().all(|l| numbered(&l))
        || d.rect.w <= 0.0
        || d.rect.h <= 0.0
    {
        return None;
    }
    let per = |v: f32, from: f32, size: f32| {
        ((v - from) / size * 1000.0).round().clamp(0.0, 1000.0) as u16
    };
    Some(
        (0..count)
            .map(|i| match d.lights.iter().find(|l| l.id == i.to_string()) {
                Some(l) => [
                    per(l.at.x, d.rect.x, d.rect.w),
                    per(l.at.y, d.rect.y, d.rect.h),
                ],
                None => [500, 500],
            })
            .collect(),
    )
}

/// The scene's effect as the keyboard's own (#542), or `None` when it doesn't map (gradient,
/// noise, a sweep of more than 4 stops, a period past the wire's) or there is no keyboard in
/// the room: those take the frames path. `lamps` is the board's LEDs by part.
pub fn keyboard_effect(
    effect: &Effect,
    room: &RoomMap,
    lamps: &LampParts,
) -> Option<KeyboardEffect> {
    use foxglove_link::protocol::EffectKind;
    let placed = keyboard_parts(room)?;
    let c = |c: &Rgb| c.0;
    // Kind, colours, period, angle, width, origin.
    let (kind, colours, period_ms, angle_deg, width_mm, origin) = match effect {
        Effect::Scanline {
            colour,
            background,
            angle_deg,
            width_mm,
            period_ms,
        } => (
            EffectKind::Scanline,
            vec![c(colour), c(background)],
            *period_ms,
            *angle_deg,
            *width_mm,
            [0, 0],
        ),
        Effect::Sweep {
            stops,
            angle_deg,
            period_ms,
        } => (
            EffectKind::Sweep,
            stops.iter().map(c).collect(),
            *period_ms,
            *angle_deg,
            0.0,
            [0, 0],
        ),
        Effect::Ripple {
            origin,
            colour,
            background,
            width_mm,
            period_ms,
        } => (
            EffectKind::Ripple,
            vec![c(colour), c(background)],
            *period_ms,
            0.0,
            *width_mm,
            [mm(origin.x), mm(origin.y)],
        ),
        Effect::Breathe {
            from,
            to,
            period_ms,
        } => (
            EffectKind::Breathe,
            vec![c(from), c(to)],
            *period_ms,
            0.0,
            0.0,
            [0, 0],
        ),
        Effect::Gradient { .. } | Effect::Noise { .. } => return None,
    };
    let wire = foxglove_link::protocol::Effect {
        kind,
        colours,
        clock_ms: 0,
        period_ms,
        angle_deg,
        width_mm,
        origin_mm: origin,
    };
    // What the wire can't carry (colour count, period) stays on frames.
    foxglove_link::protocol::effect(&wire)?;
    let all: Vec<Rgb> = wire.colours.iter().map(|&c| Rgb(c)).collect();
    Some(KeyboardEffect {
        mix: hsb(mixed(&all)?),
        effect: wire,
        room: mm_rect(&room.bounds),
        parts: placed.iter().map(|d| mm_rect(&d.rect)).collect(),
        lamps: (placed.len() == lamps.len())
            .then(|| {
                placed
                    .iter()
                    .enumerate()
                    .map(|(p, d)| part_lamps(d, lamps.count(p)))
                    .collect::<Option<Vec<_>>>()
            })
            .flatten(),
    })
}

/// One frame for the keyboard.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardFrame {
    /// Every part's LEDs in strip order, part by part ([`LampParts`]): the left half's, then the
    /// right's, or the one piece's.
    pub lamps: Vec<Rgb>,
    /// All of it as one colour, for firmware without SET_FRAME.
    pub mix: Hsb,
}

/// The part of the board a keyboard light's room device is, in frame order: the left half's
/// LEDs come first; a one-piece keyboard is its only part.
fn part_of(device: &str, parts: &LampParts) -> Option<usize> {
    match device {
        "keyboard:left" => Some(0),
        "keyboard:right" => Some(1),
        KEYBOARD if parts.len() == 1 => Some(0),
        _ => None,
    }
}

/// The keyboard's lights in a rendered frame as one SET_FRAME frame of `parts`' LEDs. A light
/// whose id is an LED number (the board profile's LED map) colours that LED of its part; any
/// other light (a part lit as one) colours every LED of its part that no numbered light covers.
/// A part with numbered lights only leaves the LEDs without one black: the map says they light
/// nothing (#608). A part missing from the room gets the mix of everything.
pub(crate) fn keyboard_frame(
    lights: &[(&str, &str, Rgb)],
    parts: &LampParts,
) -> Option<KeyboardFrame> {
    let all: Vec<Rgb> = lights.iter().map(|(_, _, c)| *c).collect();
    let mix = mixed(&all)?;
    let mut lamps = vec![mix; parts.total()];
    for part in 0..parts.len() {
        let own: Vec<(&str, Rgb)> = lights
            .iter()
            .filter(|(d, _, _)| part_of(d, parts) == Some(part))
            .map(|(_, l, c)| (*l, *c))
            .collect();
        let whole: Vec<Rgb> = own
            .iter()
            .filter(|(l, _)| l.parse::<usize>().is_err())
            .map(|(_, c)| *c)
            .collect();
        let fill = if whole.is_empty() && !own.is_empty() {
            Some(Rgb([0, 0, 0]))
        } else {
            mixed(&whole)
        };
        let Some(fill) = fill else {
            continue;
        };
        let slots = &mut lamps[parts.range(part)];
        slots.fill(fill);
        for (l, c) in &own {
            if let Some(slot) = l.parse::<usize>().ok().and_then(|i| slots.get_mut(i)) {
                *slot = *c;
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
    /// Dynamic Lighting devices, as Foxglove found them (only those in the room get frames).
    lamps: Vec<LampTarget>,
    keyboard: bool,
    /// The keyboard's LEDs by part: the size of its frames (#561).
    keyboard_parts: LampParts,
    pacing: Pacing,
    sent_case: BTreeMap<String, Vec<(usize, [u8; 3])>>,
    sent_lamps: BTreeMap<String, Vec<[u8; 3]>>,
    sent_keyboard: Option<KeyboardFrame>,
    /// The scene as the keyboard's own effect, when it maps (#542).
    keyboard_effect: Option<KeyboardEffect>,
    /// That effect went out: the keyboard needs nothing more until something changes.
    effect_sent: bool,
    /// The keyboard takes SET_EFFECT (HELLO bit 19): set by the scene from the keyboard found,
    /// cleared by the writer when it refuses one.
    keyboard_effects: Arc<AtomicBool>,
    /// Set from elsewhere (the keyboard handed its frames back): send the keyboard's next frame
    /// in full even though nothing changed.
    keyboard_again: Arc<AtomicBool>,
    /// Rings and sweeps over the scene (#489), drawn by these same frames.
    bursts: Bursts,
    /// Eases the ambient lights (the case's interior light) frame to frame.
    ambient: Ambient,
}

impl Outputs {
    pub fn new(effect: Effect, room: RoomMap, cases: Vec<CaseTarget>) -> Self {
        let mut o = Outputs {
            effect,
            room: RoomMap::default(),
            cases: Vec::new(),
            lamps: Vec::new(),
            keyboard: false,
            keyboard_parts: LampParts::old_firmware(),
            pacing: Pacing::default(),
            sent_case: BTreeMap::new(),
            sent_lamps: BTreeMap::new(),
            sent_keyboard: None,
            keyboard_effect: None,
            effect_sent: false,
            keyboard_effects: Arc::default(),
            keyboard_again: Arc::default(),
            bursts: Bursts::default(),
            ambient: Ambient::new(),
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

    /// A flag saying the keyboard takes SET_EFFECT (#542), shared with the keyboard's writer:
    /// while set, a scene that maps goes to the keyboard as one effect instead of frames.
    pub fn keyboard_effects(&self) -> Arc<AtomicBool> {
        self.keyboard_effects.clone()
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
        self.room = room;
        self.cases = cases;
        self.repace();
        // Placed again: a drag or a monitor change moves the effect too.
        self.keyboard_effect = keyboard_effect(&self.effect, &self.room, &self.keyboard_parts);
        self.effect_sent = false;
    }

    /// The keyboard's LEDs by part (the board profile's, `boards::lamp_parts`): its frames and
    /// its effect's LED points take that many. The keyboard's next frame goes in full.
    pub fn set_keyboard_parts(&mut self, parts: LampParts) {
        if parts == self.keyboard_parts {
            return;
        }
        self.keyboard_parts = parts;
        self.sent_keyboard = None;
        self.keyboard_effect = keyboard_effect(&self.effect, &self.room, &self.keyboard_parts);
        self.effect_sent = false;
    }

    /// The Dynamic Lighting devices found now (#503). Each one in the room gets its next frame
    /// in full.
    pub fn set_lamps(&mut self, lamps: Vec<LampTarget>) {
        self.lamps = lamps;
        self.repace();
    }

    /// The Dynamic Lighting devices' next frames go in full: they were let go (pause, away)
    /// and show Windows' own lights until then.
    pub fn redraw_lamps(&mut self) {
        self.sent_lamps.clear();
    }

    /// Each output's pacing from the room and the targets; every device's next frame in full.
    fn repace(&mut self) {
        let mut paced: Vec<(String, Option<Duration>)> = self
            .cases
            .iter()
            .map(|c| (c.room_id(), Some(CASE_INTERVAL)))
            .collect();
        paced.extend(
            self.lamps
                .iter()
                .filter(|l| self.room.device(&l.room_id()).is_some())
                .map(|l| (l.room_id(), Some(l.interval()))),
        );
        if self.keyboard {
            paced.push((KEYBOARD.into(), Some(KEYBOARD_INTERVAL)));
        }
        self.pacing = Pacing::new(paced);
        self.sent_case.clear();
        self.sent_lamps.clear();
        self.sent_keyboard = None;
    }

    /// Another look while the scene runs (the Scenes page, #487): every device gets the next
    /// frame in full. The ambient lights fade across to the new look rather than jumping (so
    /// the case is written at its own rate until they settle, a few seconds).
    pub fn set_effect(&mut self, effect: Effect) {
        self.effect = effect;
        self.sent_case.clear();
        self.sent_lamps.clear();
        self.sent_keyboard = None;
        self.keyboard_effect = keyboard_effect(&self.effect, &self.room, &self.keyboard_parts);
        self.effect_sent = false;
    }

    /// The writes for scene time `t`: each due device whose colours changed.
    pub fn frame(&mut self, t: SceneTime) -> Vec<Out> {
        if self.keyboard_again.swap(false, Ordering::Relaxed) {
            self.sent_keyboard = None;
            self.effect_sent = false;
        }
        let mut due = self.pacing.due(t);
        if due.is_empty() {
            return Vec::new();
        }
        let bursting = self.bursts.prune(t);
        let mut out = Vec::new();
        // The keyboard draws the scene itself: sent once, then nothing per frame. A burst over
        // the scene is drawn by frames (they take over from the effect on the keyboard), and
        // the effect goes again once it is gone.
        let effect = self.keyboard_effect.as_ref().filter(|_| {
            self.keyboard && !bursting && self.keyboard_effects.load(Ordering::Relaxed)
        });
        if let Some(e) = effect {
            if let Some(i) = due.iter().position(|id| id == KEYBOARD) {
                due.remove(i);
                if !self.effect_sent {
                    self.effect_sent = true;
                    self.sent_keyboard = None;
                    out.push(Out::KeyboardEffect(e.clone()));
                }
            }
            if due.is_empty() {
                return out;
            }
        } else {
            self.effect_sent = false;
        }
        let mut frame = self.effect.render(&self.room, t);
        if bursting {
            // `render` lists the lights device by device, in the room's order.
            let places = self.room.devices.iter().flat_map(|d| d.lights.iter());
            for (l, place) in frame.lights.iter_mut().zip(places) {
                if place.role == LightRole::Point {
                    l.colour = self.bursts.over(l.colour, place.at, t, &self.room.bounds);
                }
            }
            // An area light follows the burst on its neighbours rather than being hit itself.
            foxglove_scene::ambient::group(&mut frame, &self.room);
        }
        self.ambient.apply(&mut frame, &self.room);
        for id in due {
            if id == KEYBOARD {
                let lights: Vec<(&str, &str, Rgb)> = frame
                    .lights
                    .iter()
                    .filter(|l| l.device.starts_with(KEYBOARD))
                    .map(|l| (l.device.as_str(), l.light.as_str(), l.colour))
                    .collect();
                let Some(f) = keyboard_frame(&lights, &self.keyboard_parts) else {
                    continue;
                };
                if self.sent_keyboard.as_ref() != Some(&f) {
                    self.sent_keyboard = Some(f.clone());
                    out.push(Out::Keyboard(f));
                }
                continue;
            }
            if let Some(target) = self.lamps.iter().find(|l| l.room_id() == id) {
                let lights: Vec<(&str, Rgb)> = frame
                    .lights
                    .iter()
                    .filter(|l| l.device == id)
                    .map(|l| (l.light.as_str(), l.colour))
                    .collect();
                let Some(colors) = lamp_colours(target, &lights) else {
                    continue;
                };
                if self.sent_lamps.get(&target.key) != Some(&colors) {
                    self.sent_lamps.insert(target.key.clone(), colors.clone());
                    out.push(Out::Lamps {
                        key: target.key.clone(),
                        colors,
                    });
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
        let zone = |id: &str, x: f32, y: f32| LightSpec::point(id, Some(Point::new(x, y)));
        let devices = [
            DeviceInfo::whole("keyboard:left", "Keyboard left", DeviceKind::KeyboardLeft),
            DeviceInfo::whole(
                "keyboard:right",
                "Keyboard right",
                DeviceKind::KeyboardRight,
            ),
            DeviceInfo {
                id: "light:103c:84fd".into(),
                name: "Case".into(),
                kind: DeviceKind::Case,
                size: Some(Point::new(205.0, 475.0)),
                // The catalog's order: 1, 2, 5, 6, 7.
                lights: vec![
                    zone("1", 102.0, 55.0),
                    zone("2", 120.0, 250.0).with_role(foxglove_scene::LightRole::Ambient),
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

    /// The second desk (`fixtures`): two monitors of different DPI, the one-piece Plank 3 with
    /// 61 LEDs, another vendor's case board and a lightbar, as `scene_room` gathers them.
    fn second_desk() -> (RoomMap, Vec<CaseTarget>) {
        use crate::foxglove::fixtures::second_desk as desk;
        let rows = [desk::case_row(), desk::lightbar_row()];
        let devices =
            crate::foxglove::scene_room::devices(Some(&desk::board()), &rows, &desk::lamps());
        let room = RoomMap::build(&desk::monitors(), &devices, &Default::default());
        let cases = vec![CaseTarget {
            key: desk::CASE_ID.trim_start_matches("light:").into(),
            zone_ids: ["1", "2", "3", "4"].map(String::from).to_vec(),
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

    /// A Dynamic Lighting device of no particular vendor: a headset-kind array of 8 lamps in a
    /// row, placed by the room map as any "other" device.
    fn desk_with_a_lamparray() -> (RoomMap, LampTarget) {
        let devices = [DeviceInfo {
            id: "light:2a5f:0101".into(),
            name: "Desk headset".into(),
            kind: DeviceKind::Other,
            size: Some(Point::new(160.0, 40.0)),
            lights: (0..8)
                .map(|i| LightSpec::point(i.to_string(), Some(Point::new(20.0 * i as f32, 20.0))))
                .collect(),
        }];
        let room = RoomMap::build(&[], &devices, &Default::default());
        let target = LampTarget {
            key: "2a5f:0101".into(),
            lamps: 8,
            // It says it takes 100 frames a second: still paced at LAMP_INTERVAL.
            min_interval: Some(Duration::from_millis(10)),
        };
        (room, target)
    }

    #[test]
    fn a_lamparray_gets_a_colour_per_lamp_paced_and_only_on_change() {
        let (room, target) = desk_with_a_lamparray();
        let sweep = Effect::Sweep {
            stops: vec![ORANGE, Rgb([0x20, 0x40, 0xf0])],
            angle_deg: 0.0,
            period_ms: 2000,
        };
        let mut o = Outputs::new(sweep, room.clone(), Vec::new());
        o.set_lamps(vec![target.clone()]);
        let mut sent: Vec<(i64, Vec<[u8; 3]>)> = Vec::new();
        for ms in (0..2000).step_by(10) {
            for out in o.frame(SceneTime(ms)) {
                if let Out::Lamps { key, colors } = out {
                    assert_eq!(key, "2a5f:0101");
                    sent.push((ms, colors));
                }
            }
        }
        assert!(!sent.is_empty());
        assert!(sent.iter().all(|(_, c)| c.len() == 8), "a colour per lamp");
        assert!(
            sent.windows(2).all(|w| w[1].0 - w[0].0 >= 45),
            "never faster than LAMP_INTERVAL"
        );
        assert!(sent.len() <= 40, "{} frames in 2 s", sent.len());
        assert!(sent[0].1[0] != sent[0].1[7], "lamps at their own places");
        // A still look: one frame, then nothing until something changes.
        let still = Effect::Breathe {
            from: ORANGE,
            to: ORANGE,
            period_ms: 4000,
        };
        let mut o = Outputs::new(still, room, Vec::new());
        o.set_lamps(vec![target]);
        let count = |o: &mut Outputs, from: i64| {
            (from..from + 1000)
                .step_by(10)
                .flat_map(|ms| o.frame(SceneTime(ms)))
                .filter(|out| matches!(out, Out::Lamps { .. }))
                .count()
        };
        assert_eq!(count(&mut o, 0), 1);
        assert_eq!(count(&mut o, 1000), 0);
        // Let go while paused: the next frame goes in full.
        o.redraw_lamps();
        assert_eq!(count(&mut o, 2000), 1);
    }

    #[test]
    fn lamp_colours_fill_unplaced_lamps_and_unknown_counts() {
        let t = |lamps| LampTarget {
            key: "k".into(),
            lamps,
            min_interval: None,
        };
        let red = Rgb([255, 0, 0]);
        // Not read yet: one colour for the whole device.
        assert_eq!(
            lamp_colours(&t(0), &[("all", red)]),
            Some(vec![[255, 0, 0]])
        );
        // Read, lit as one: every lamp.
        assert_eq!(
            lamp_colours(&t(3), &[("all", red)]),
            Some(vec![[255, 0, 0]; 3])
        );
        // Numbered lights colour their lamps; one past the count is ignored.
        let c = lamp_colours(&t(2), &[("0", red), ("1", Rgb::BLACK), ("5", red)]).unwrap();
        assert_eq!(c[0], [255, 0, 0]);
        assert_eq!(c[1], [0, 0, 0]);
        assert_eq!(t(0).interval(), LAMP_INTERVAL);
        assert_eq!(
            LampTarget {
                min_interval: Some(Duration::from_millis(80)),
                ..t(1)
            }
            .interval(),
            Duration::from_millis(80)
        );
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
                    Out::KeyboardEffect(_) => unreachable!("no SET_EFFECT keyboard here"),
                    Out::Lamps { .. } => unreachable!("no Dynamic Lighting device here"),
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
    fn the_interior_light_fades_with_the_fans_rather_than_pinging() {
        let (room, cases) = desk();
        let mut o = Outputs::new(scanline(), room, cases);
        let (mut fans, mut interior) = (0u8, 0u8);
        let (mut low, mut high) = (u8::MAX, 0u8);
        let mut last: Option<Vec<(usize, [u8; 3])>> = None;
        let step = |a: [u8; 3], b: [u8; 3]| (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
        // Two periods at the scheduler's fastest tick; the case writes as often as it may.
        for ms in (0..8000).step_by(33) {
            for out in o.frame(SceneTime(ms)) {
                let Out::Case { colors, .. } = out else {
                    continue;
                };
                if let Some(p) = &last {
                    // Zone order 1, 2, 5, 6, 7: index 1 is the interior light.
                    for i in 2..5 {
                        fans = fans.max(step(p[i].1, colors[i].1));
                    }
                    interior = interior.max(step(p[1].1, colors[1].1));
                }
                if ms >= 4000 {
                    low = low.min(colors[1].1[0]);
                    high = high.max(colors[1].1[0]);
                }
                last = Some(colors);
            }
        }
        assert!(fans >= 40, "the fans ping: {fans}");
        // The case writes every 60 ms or more, so a write carries two frames of easing at most:
        // about 10 % of the way from black to full orange (a ping is the whole way at once).
        assert!(
            interior <= 24,
            "the interior light eases: {interior} per write"
        );
        assert!(
            high.saturating_sub(low) >= 10,
            "and fades in and out: {low}..{high}"
        );
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
                    Out::Keyboard(_) | Out::KeyboardEffect(_) => kb_t.push(ms),
                    Out::Lamps { .. } => unreachable!("no Dynamic Lighting device here"),
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
                    lamps: vec![ORANGE; corne().total()],
                    mix: hsb(ORANGE),
                }),
            ]
        );
        let a = Rgb([255, 0, 0]);
        assert_eq!(mixed(&[a, a, a]), Some(a));
        assert_eq!(mixed(&[]), None);
    }

    #[test]
    fn the_second_desks_case_gets_every_zone_and_runs_top_to_bottom() {
        let (room, cases) = second_desk();
        let still = Effect::Breathe {
            from: ORANGE,
            to: ORANGE,
            period_ms: 1000,
        };
        let mut o = Outputs::new(still, room.clone(), cases.clone());
        let outs = o.frame(SceneTime(0));
        assert!(
            outs.contains(&Out::Case {
                key: cases[0].key.clone(),
                colors: (0..4).map(|i| (i, ORANGE.0)).collect(),
            }),
            "{outs:?}"
        );
        assert!(
            outs.iter().any(|o| matches!(o, Out::Keyboard(_))),
            "the one-piece keyboard gets frames too"
        );
        // A scanline down the room: the case's top bar before its upper ring before its lower.
        let mut o = Outputs::new(scanline(), room, cases);
        let mut first_lit: BTreeMap<usize, i64> = BTreeMap::new();
        for ms in (0..4000).step_by(10) {
            for out in o.frame(SceneTime(ms)) {
                if let Out::Case { colors, .. } = out {
                    for (i, c) in colors {
                        if c[0] > 200 {
                            first_lit.entry(i).or_insert(ms);
                        }
                    }
                }
            }
        }
        let at = |i: usize| first_lit.get(&i).copied().unwrap_or(i64::MAX);
        assert!(at(0) < at(1) && at(1) < at(2), "{first_lit:?}");
    }

    /// The Eyelash Corne's LEDs: two halves of 21.
    fn corne() -> LampParts {
        crate::foxglove::boards::lamp_parts(Some(&crate::foxglove::fixtures::users_desk::board()))
    }

    /// The one-piece Plank 3's 61 LEDs, from its profile.
    fn plank() -> LampParts {
        crate::foxglove::boards::lamp_parts(Some(&crate::foxglove::fixtures::second_desk::board()))
    }

    #[test]
    fn the_second_desks_keyboard_frame_has_one_colour_per_led() {
        let (room, cases) = second_desk();
        let still = Effect::Breathe {
            from: ORANGE,
            to: ORANGE,
            period_ms: 1000,
        };
        let mut o = Outputs::new(still, room, cases);
        o.set_keyboard_parts(plank());
        let frame = o.frame(SceneTime(0)).into_iter().find_map(|o| match o {
            Out::Keyboard(f) => Some(f),
            _ => None,
        });
        let lamps = crate::foxglove::fixtures::second_desk::LAMPS;
        assert_eq!(frame.map(|f| f.lamps.len()), Some(lamps));
    }

    #[test]
    fn the_second_desks_keyboard_effect_places_every_led() {
        let (room, _) = second_desk();
        let e = keyboard_effect(&scanline(), &room, &plank()).expect("a scanline maps");
        let lamps = crate::foxglove::fixtures::second_desk::LAMPS;
        let placed = e
            .lamps
            .as_ref()
            .map(|parts| parts.iter().map(Vec::len).sum::<usize>());
        assert_eq!(placed, Some(lamps), "every LED of the one piece");
        // The one piece is one part, placed whole.
        let whole = room.device(KEYBOARD).unwrap().rect;
        assert_eq!(e.parts, [mm_rect(&whole)]);
        assert_eq!(e.place(), (mm_rect(&whole), mm_rect(&whole)));
    }

    /// A ripple through the one-piece board: 61-LED frames whose colours move with the ring.
    #[test]
    fn a_ripple_on_the_second_desk_draws_every_led_of_the_one_piece() {
        let (room, cases) = second_desk();
        let kb = room.device(KEYBOARD).unwrap().rect;
        let ripple = Effect::Ripple {
            origin: foxglove_scene::Point::new(kb.x + kb.w / 2.0, kb.y + kb.h / 2.0),
            colour: ORANGE,
            background: Rgb::BLACK,
            width_mm: 40.0,
            period_ms: 2000,
        };
        let mut o = Outputs::new(ripple, room, cases);
        o.set_keyboard_parts(plank());
        let frames: Vec<KeyboardFrame> = (0..10)
            .flat_map(|i| o.frame(SceneTime(i * 100)))
            .filter_map(|o| match o {
                Out::Keyboard(f) => Some(f),
                _ => None,
            })
            .collect();
        assert!(frames.len() > 3, "{}", frames.len());
        assert!(frames.iter().all(|f| f.lamps.len() == 61));
        assert!(
            frames
                .iter()
                .any(|f| f.lamps.iter().any(|c| *c != f.lamps[0])),
            "the ring lights some LEDs and not others"
        );
    }

    /// A split board whose halves differ (30 + 24): each half's LEDs in its own part of the
    /// frame, nothing of one half spilling into the other.
    #[test]
    fn uneven_halves_get_frames_of_their_own_size() {
        let (red, blue) = (Rgb([255, 0, 0]), Rgb([0, 0, 255]));
        let parts = LampParts::new(&[30, 24]).unwrap();
        let mut lights: Vec<(&str, &str, Rgb)> = Vec::new();
        let ids: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        for id in &ids {
            lights.push(("keyboard:left", id, red));
        }
        for id in &ids[..24] {
            lights.push(("keyboard:right", id, blue));
        }
        let f = keyboard_frame(&lights, &parts).unwrap();
        assert_eq!(f.lamps.len(), 54);
        assert_eq!(f.lamps[..30], [red; 30]);
        assert_eq!(f.lamps[30..], [blue; 24]);
    }

    #[test]
    fn each_half_gets_its_own_colour_and_numbered_leds_their_own() {
        let (red, blue, green) = (Rgb([255, 0, 0]), Rgb([0, 0, 255]), Rgb([0, 255, 0]));
        let corne = corne();
        let half = corne.count(0);
        // Per half (the default): the left's 21 LEDs red, the right's blue, one colour mixed.
        let f = keyboard_frame(
            &[
                ("keyboard:left", "all", red),
                ("keyboard:right", "all", blue),
            ],
            &corne,
        )
        .unwrap();
        assert_eq!(f.lamps[..half], vec![red; half]);
        assert_eq!(f.lamps[half..], vec![blue; half]);
        assert_eq!(f.mix, hsb(red.mix(blue, 0.5)));
        // Per LED (a board profile with an LED map): each numbered light its LED; LEDs the map
        // leaves out light nothing (#608), so they stay black.
        let mut lights: Vec<(&str, &str, Rgb)> = Vec::new();
        let ids: Vec<String> = (0..half).map(|i| i.to_string()).collect();
        for (i, id) in ids.iter().enumerate().take(20) {
            lights.push(("keyboard:left", id, if i == 3 { green } else { red }));
        }
        lights.push(("keyboard:right", "all", blue));
        let f = keyboard_frame(&lights, &corne).unwrap();
        assert_eq!(f.lamps[3], green);
        assert_eq!(f.lamps[0], red);
        assert_eq!(
            f.lamps[20],
            Rgb([0, 0, 0]),
            "a dead LED: black, not the half's mix"
        );
        assert_eq!(f.lamps[half], blue);
        // A one-piece keyboard on split firmware (or one half placed): the mix everywhere.
        let f = keyboard_frame(&[("keyboard", "all", green)], &corne).unwrap();
        assert_eq!(f.lamps, vec![green; corne.total()]);
        assert!(keyboard_frame(&[], &corne).is_none());
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

    /// The four effects the keyboard draws map one to one onto SET_EFFECT (#542); gradient,
    /// noise and a sweep the wire can't carry stay on frames.
    #[test]
    fn scenes_map_onto_the_keyboards_own_effects() {
        use foxglove_link::protocol::EffectKind;
        let (room, _) = desk();
        let corne = corne();
        let keyboard_effect = |e: &Effect, room: &RoomMap| keyboard_effect(e, room, &corne);
        let e = keyboard_effect(&scanline(), &room).expect("a scanline maps");
        assert_eq!(e.effect.kind, EffectKind::Scanline);
        assert_eq!(e.effect.colours, [ORANGE.0, [0, 0, 0]]);
        assert_eq!((e.effect.period_ms, e.effect.width_mm), (4000, 80.0));
        assert_eq!(e.effect.angle_deg, 90.0);
        // Placed as the room map has the halves, in whole mm.
        let rect = |id: &str| {
            let r = room.devices.iter().find(|d| d.id == id).unwrap().rect;
            [r.x, r.y, r.w, r.h].map(|v| v.round() as i16)
        };
        assert_eq!(e.parts, [rect("keyboard:left"), rect("keyboard:right")]);
        assert_eq!(e.place(), (rect("keyboard:left"), rect("keyboard:right")));
        let b = room.bounds;
        assert_eq!(e.room, [b.x, b.y, b.w, b.h].map(|v| v.round() as i16));
        assert_eq!(e.mix, hsb(ORANGE.mix(Rgb::BLACK, 0.5)));
        assert_eq!(e.lamps, None, "a half lit as one: the firmware's own table");

        let blue = Rgb([0, 0, 255]);
        let sweep = |n: usize| Effect::Sweep {
            stops: vec![blue; n],
            angle_deg: 0.0,
            period_ms: 3000,
        };
        assert_eq!(
            keyboard_effect(&sweep(4), &room).map(|e| e.effect.kind),
            Some(EffectKind::Sweep)
        );
        assert!(
            keyboard_effect(&sweep(5), &room).is_none(),
            "5 stops: frames"
        );
        let ripple = Effect::Ripple {
            origin: foxglove_scene::Point::new(120.4, -30.6),
            colour: ORANGE,
            background: blue,
            width_mm: 60.0,
            period_ms: 2000,
        };
        let r = keyboard_effect(&ripple, &room).unwrap();
        assert_eq!(
            (r.effect.kind, r.effect.origin_mm),
            (EffectKind::Ripple, [120, -31])
        );
        let breathe = Effect::Breathe {
            from: ORANGE,
            to: blue,
            period_ms: 5000,
        };
        assert_eq!(
            keyboard_effect(&breathe, &room).map(|e| e.effect.colours),
            Some(vec![ORANGE.0, blue.0])
        );
        let gradient = Effect::Gradient {
            stops: vec![ORANGE, blue],
            angle_deg: 0.0,
        };
        assert!(keyboard_effect(&gradient, &room).is_none());
        let noise = Effect::Noise {
            stops: vec![ORANGE, blue],
            scale_mm: 100.0,
            period_ms: 8000,
            seed: 1,
        };
        assert!(keyboard_effect(&noise, &room).is_none());
        let slow = Effect::Breathe {
            from: ORANGE,
            to: blue,
            period_ms: 0x100_0000,
        };
        assert!(
            keyboard_effect(&slow, &room).is_none(),
            "past 24 bits of ms"
        );
        // No keyboard in the room: nothing to place.
        let mut no_kb = room.clone();
        no_kb.devices.retain(|d| !d.id.starts_with(KEYBOARD));
        assert!(keyboard_effect(&scanline(), &no_kb).is_none());
    }

    /// The effect's clock is the scene's time modulo its period, so a send at any moment
    /// starts the halves in phase with the case and the wallpapers on the same epoch.
    #[test]
    fn the_effects_clock_is_the_scenes_phase() {
        let (room, _) = desk();
        let corne = corne();
        let e = keyboard_effect(&scanline(), &room, &corne).unwrap();
        assert_eq!(e.at(SceneTime(0)).clock_ms, 0);
        assert_eq!(e.at(SceneTime(10_500)).clock_ms, 2500);
        assert_eq!(e.at(SceneTime(-500)).clock_ms, 3500, "before the epoch");
        let still = Effect::Breathe {
            from: ORANGE,
            to: ORANGE,
            period_ms: 0,
        };
        let s = keyboard_effect(&still, &room, &corne).unwrap();
        assert_eq!(s.at(SceneTime(1234)).clock_ms, 0);
        // A one-piece keyboard is one part, placed whole (PLACE's two rectangles both it).
        let mut one = room.clone();
        let left = one
            .devices
            .iter()
            .position(|d| d.id == "keyboard:left")
            .unwrap();
        one.devices.retain(|d| d.id != "keyboard:right");
        one.devices[left].id = KEYBOARD.into();
        one.devices[left].rect = foxglove_scene::Rect::new(0.0, 500.0, 300.0, 90.0);
        let e = keyboard_effect(&scanline(), &one, &plank()).unwrap();
        assert_eq!(e.parts, [[0, 500, 300, 90]]);
        assert_eq!(e.place(), ([0, 500, 300, 90], [0, 500, 300, 90]));
    }

    #[test]
    fn a_keyboard_with_effects_gets_the_effect_once_and_no_frames() {
        let (room, cases) = desk();
        let mut o = Outputs::new(scanline(), room, cases);
        o.keyboard_effects().store(true, Ordering::Relaxed);
        let kb = |outs: &[Out]| -> (usize, usize) {
            let effects = outs
                .iter()
                .filter(|o| matches!(o, Out::KeyboardEffect(_)))
                .count();
            let frames = outs
                .iter()
                .filter(|o| matches!(o, Out::Keyboard(_)))
                .count();
            (effects, frames)
        };
        let mut sent = (0, 0);
        let mut case = 0;
        for ms in (0..2000).step_by(33) {
            let outs = o.frame(SceneTime(ms));
            let (e, f) = kb(&outs);
            sent = (sent.0 + e, sent.1 + f);
            case += outs.len() - e - f;
        }
        assert_eq!(sent, (1, 0), "one effect, no keyboard frames");
        assert!(case > 10, "the case still gets its frames: {case}");
        // Asked again (a reconnect, the lease showing again): once more.
        o.keyboard_again().store(true, Ordering::Relaxed);
        let mut n = 0;
        for ms in (2000..2400).step_by(33) {
            n += kb(&o.frame(SceneTime(ms))).0;
        }
        assert_eq!(n, 1);
        // A new look, or the room again (a drag): once more each.
        o.set_effect(Effect::Breathe {
            from: ORANGE,
            to: Rgb::BLACK,
            period_ms: 3000,
        });
        assert_eq!(kb(&o.frame(SceneTime(2600))).0, 1);
        let (room, cases) = desk();
        o.set_room(room, cases);
        assert_eq!(kb(&o.frame(SceneTime(2800))).0, 1);
        // A look the keyboard can't draw: frames instead.
        o.set_effect(Effect::Gradient {
            stops: vec![ORANGE, Rgb([0, 0, 255])],
            angle_deg: 0.0,
        });
        assert_eq!(kb(&o.frame(SceneTime(3000))), (0, 1));
        // The writer found the keyboard takes none after all: frames.
        o.set_effect(scanline());
        o.keyboard_effects().store(false, Ordering::Relaxed);
        assert_eq!(kb(&o.frame(SceneTime(3200))), (0, 1));
    }

    /// A key ripple over an effect scene (#489) is drawn by frames, which take over from the
    /// effect; once it is gone the effect goes again.
    #[test]
    fn a_burst_over_an_effect_scene_draws_frames_then_the_effect_again() {
        use foxglove_scene::burst::Shape;
        let (room, cases) = desk();
        let key = room
            .devices
            .iter()
            .find(|d| d.id == "keyboard:left")
            .unwrap()
            .rect
            .centre();
        let mut o = Outputs::new(scanline(), room, cases);
        o.keyboard_effects().store(true, Ordering::Relaxed);
        let kinds = |outs: Vec<Out>| -> Vec<&'static str> {
            outs.iter()
                .filter_map(|o| match o {
                    Out::Keyboard(_) => Some("frame"),
                    Out::KeyboardEffect(_) => Some("effect"),
                    Out::Case { .. } | Out::Lamps { .. } => None,
                })
                .collect()
        };
        assert_eq!(kinds(o.frame(SceneTime(0))), ["effect"]);
        o.burst(Burst {
            shape: Shape::Ring { origin: key },
            colour: Rgb([255, 255, 255]),
            start: SceneTime(200),
            duration_ms: 500,
            width_mm: 60.0,
        });
        assert_eq!(kinds(o.frame(SceneTime(200))), ["frame"]);
        assert_eq!(
            kinds(o.frame(SceneTime(1000))),
            ["effect"],
            "the ring is gone"
        );
        assert!(kinds(o.frame(SceneTime(1200))).is_empty());
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
