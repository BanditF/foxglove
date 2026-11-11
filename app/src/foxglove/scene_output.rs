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
//! - The keyboard while Windows Dynamic Lighting holds its lights (#749 b): a keyboard that is
//!   also a LampArray ([`keyboard_lamp_key`] pairs them by VID/PID, never by model) is drawn
//!   through Windows like any LampArray, each lamp at its own place on the keyboard's rect
//!   ([`KeyboardLamps`]), through the keyboard's colour calibration. Only on the lights owner's word (the "held" flag), never on a timer:
//!   held, the keyboard gets [`Out::Lamps`] and nothing on the host link; freed, host-link
//!   frames or its effect again, in full.
//!
//! Each output has its own interval (never faster than the scheduler's 30 fps cap), from the
//! measured rates file (`crates/devices/light-rates.toml`, #486: [`keyboard_interval`],
//! [`case_interval`], [`lamp_interval`]); [`CASE_INTERVAL`], [`KEYBOARD_INTERVAL`] and
//! [`LAMP_INTERVAL`] only where it says nothing. The keyboard is the slowest, since its colour
//! goes over the host link and on to the right half over BLE through the relay. A device gets
//! a frame only when its colours changed. This is pure: the app's `scene_run` sends what it returns.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use foxglove_devices::pc_lighting::rates::{self, Rates};

use foxglove_link::lamps::LampParts;
use foxglove_link::protocol::MmRect;
use foxglove_scene::scheduler::{Pace, Pacing};
use foxglove_scene::{Ambient, Burst, Bursts, Effect, LightRole, Rgb, RoomMap, SceneTime};

/// The keyboard's output id in [`Pacing`].
pub const KEYBOARD: &str = "keyboard";

/// The keyboard's own LampArray's output id in [`Pacing`] (#749 b), due only while held.
const KEYBOARD_LAMPS: &str = "keyboard-lamps";

/// The keyboard gets at most 10 colours a second (host link, then BLE to the right half).
pub const KEYBOARD_INTERVAL: Duration = Duration::from_millis(100);

/// A case board gets at most 15 frames a second (one 59-byte report per changed zone).
pub const CASE_INTERVAL: Duration = Duration::from_millis(66);

/// A Dynamic Lighting device gets at most 20 frames a second, however fast its attributes say
/// it takes them: each frame crosses Windows' lighting service and often a vendor's (#313).
pub const LAMP_INTERVAL: Duration = Duration::from_millis(50);

/// The bundled rates file, read once.
fn light_rates() -> &'static Rates {
    static RATES: OnceLock<Rates> = OnceLock::new();
    RATES.get_or_init(Rates::bundled)
}

/// The keyboard's frame interval: the rates file's (the slower bus's when both were measured),
/// else [`KEYBOARD_INTERVAL`].
pub fn keyboard_interval() -> Duration {
    light_rates()
        .interval(rates::KEYBOARD, None)
        .unwrap_or(KEYBOARD_INTERVAL)
}

/// A driven board's frame interval: its model's or its protocol's entry in the rates file, else
/// [`CASE_INTERVAL`].
pub fn case_interval(protocol: &str, key: &str) -> Duration {
    light_rates()
        .interval(protocol, Some(key))
        .unwrap_or(CASE_INTERVAL)
}

/// The least interval a Dynamic Lighting device gets: its model's or the class's entry in the
/// rates file, else [`LAMP_INTERVAL`].
pub fn lamp_interval(key: &str) -> Duration {
    light_rates()
        .interval(rates::LAMP_ARRAY, Some(key))
        .unwrap_or(LAMP_INTERVAL)
}

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

    /// Its own interval, never under [`lamp_interval`]'s.
    pub fn interval(&self) -> Duration {
        let least = lamp_interval(&self.key);
        self.min_interval.map_or(least, |m| m.max(least))
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

/// The keyboard's own LampArray (#749 b), for drawing the keyboard through Windows while
/// Windows Dynamic Lighting holds its lights.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardLamps {
    /// The writer's key (`vvvv:pppp`: every LampArray interface of the keyboard, USB and BLE)
    /// and its lamp count.
    pub target: LampTarget,
    /// Each lamp's place on the array's bounding box in index order, as fractions (0 at the
    /// left or top, 1 at the right or bottom); empty when Windows gives no positions (then one
    /// colour for all).
    pub places: Vec<[f32; 2]>,
    /// The keyboard's calibration key (`devices::keyboard_key`): its colour calibration goes on
    /// every lamp, as on its host-link frames. `None`: sent as drawn.
    pub calibration: Option<String>,
}

impl KeyboardLamps {
    /// With the keyboard's colour calibration (its calibration key) on every lamp.
    pub fn calibrated(mut self, key: Option<String>) -> Self {
        self.calibration = key;
        self
    }

    /// From the array's shape as Windows gives it (`pc_lights::LampLayout`: its bounding box and
    /// each lamp's position, mm); `None` before its attributes are read.
    pub fn new(key: String, shape: Option<([f32; 2], &[[f32; 2]])>) -> Self {
        let (lamps, places) = match shape {
            Some(([w, h], at)) => {
                let placed = w > 0.0 && h > 0.0 && at.iter().any(|&[x, y]| x != 0.0 || y != 0.0);
                let places = if placed {
                    at.iter()
                        .map(|&[x, y]| [(x / w).clamp(0.0, 1.0), (y / h).clamp(0.0, 1.0)])
                        .collect()
                } else {
                    Vec::new()
                };
                (at.len(), places)
            }
            None => (0, Vec::new()),
        };
        KeyboardLamps {
            target: LampTarget {
                key,
                lamps,
                min_interval: None,
            },
            places,
            calibration: None,
        }
    }
}

/// `vvvv:pppp[:serial]` as its parts.
fn key_parts(key: &str) -> Option<(u16, u16, Option<&str>)> {
    let mut parts = key.splitn(3, ':');
    let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
    Some((vid, pid, parts.next().filter(|s| !s.is_empty())))
}

/// The host-link keyboard's own LampArray (#749 b), paired by VID/PID, and by serial when both
/// keys carry one: the writer's key for it (`vvvv:pppp`, which opens each of its LampArray
/// interfaces, since Windows gives a LampArray no serial). `keyboard` is the keyboard's key
/// (`devices::keyboard_key`); `arrays` the keyboards' LampArrays Windows lists
/// (`pc_lights::Snapshot::keyboard_lamp_arrays`). Any keyboard that is both a Foxglove-link
/// keyboard and a LampArray pairs this way.
pub fn keyboard_lamp_key(
    keyboard: &str,
    arrays: &[foxglove_devices::pc_lighting::KeyboardLampArray],
) -> Option<String> {
    let (vid, pid, serial) = key_parts(keyboard)?;
    arrays
        .iter()
        .filter(|a| (a.vid, a.pid) == (vid, pid))
        .any(
            |a| match (serial, key_parts(&a.key).and_then(|(_, _, s)| s)) {
                (Some(k), Some(l)) => k.trim() == l.trim(),
                _ => true,
            },
        )
        .then(|| format!("{vid:04x}:{pid:04x}"))
}

/// A board the Lights worker drives: its key and each zone's id, in the driver's zone order.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseTarget {
    pub key: String,
    pub zone_ids: Vec<String>,
    /// Its frame interval ([`case_interval`]).
    pub interval: Duration,
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
        // Tones that crossfade (no LED off the first accent) are the firmware's breathe from
        // the accent to the second one, the same maths (#871); split ones need frames.
        Effect::Tones {
            colours,
            tones,
            period_ms,
            ..
        } if !foxglove_scene::tones_split(tones) && !colours.is_empty() => (
            EffectKind::Breathe,
            vec![c(&colours[0]), c(colours.get(1).unwrap_or(&colours[0]))],
            *period_ms,
            0.0,
            0.0,
            [0, 0],
        ),
        // Key rings (#490): the ring's colour then the background, a ring's time as the period;
        // each half starts its own rings from its own keys. Only on firmware with KEY_RINGS:
        // [`Outputs::frame`] sends it only while the keyboard's rings flag is set.
        Effect::KeyRings {
            colour,
            background,
            width_mm,
            period_ms,
        } => (
            EffectKind::KeyRings,
            vec![c(colour), c(background)],
            *period_ms,
            0.0,
            *width_mm,
            [0, 0],
        ),
        // Tide's per-key wave has no firmware effect, and Screen glow's colours come from the
        // screen (#496): frames.
        Effect::Gradient { .. }
        | Effect::Noise { .. }
        | Effect::Tones { .. }
        | Effect::Tide { .. }
        | Effect::Screen { .. } => return None,
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
    /// The keyboard draws key rings (#490, HELLO v2 KEY_RINGS): set by the writer from the
    /// keyboard found. Without it a key-rings look goes to the keyboard as frames.
    keyboard_rings: Arc<AtomicBool>,
    /// Set from elsewhere (the keyboard handed its frames back): send the keyboard's next frame
    /// in full even though nothing changed.
    keyboard_again: Arc<AtomicBool>,
    /// Someone else (Windows Dynamic Lighting) has the keyboard's lights (#749): no keyboard
    /// frame or effect is made at all, and the next one after goes in full.
    keyboard_held: Arc<AtomicBool>,
    /// The keyboard's own LampArray (#749 b): while held, the keyboard is drawn through it.
    keyboard_lamps: Option<KeyboardLamps>,
    /// Rings and sweeps over the scene (#489), drawn by these same frames.
    bursts: Bursts,
    /// Eases the ambient lights (the case's interior light) frame to frame.
    ambient: Ambient,
    /// Screen glow (#496): each light's eased, flash-limited screen colour, while the look is
    /// Screen glow.
    glow: Option<foxglove_scene::Glow>,
    /// The screens' newest edge colours, from the scene's reader.
    screen_feed: Option<Arc<super::screen_glow::Feed>>,
    /// Screen glow's frame pacing (#1085): the pictures drawn so far, ...
    seen_pictures: u64,
    /// ... something changed outside the frames (a look, the room, a hand-back) ...
    kick: bool,
    /// ... the keyboard's held flag as the last frame saw it ...
    held_seen: bool,
    /// ... and until when (scene ms) frames still come at the tick after a change, so every
    /// device gets the settled colours.
    busy_until: i64,
    /// Screen glow's hand-backs so far (each a generation), when the current one reached its
    /// own colours, and whether that was reported.
    hand_backs: u64,
    handed_at: Option<i64>,
    hand_back_reported: bool,
    /// Each area light's colour as the last frame showed it (after its easing): a hand-back
    /// fades it on from there.
    areas_shown: BTreeMap<(String, String), Rgb>,
}

/// A Screen glow look's own state, kept across a smoothing change.
fn glow_for(effect: &Effect, now: Option<foxglove_scene::Glow>) -> Option<foxglove_scene::Glow> {
    match effect {
        Effect::Screen { smoothing } => Some(match now {
            Some(mut g) => {
                g.smoothing = *smoothing;
                g
            }
            None => foxglove_scene::Glow::new(*smoothing),
        }),
        _ => None,
    }
}

impl Outputs {
    pub fn new(effect: Effect, room: RoomMap, cases: Vec<CaseTarget>) -> Self {
        let glow = glow_for(&effect, None);
        let mut o = Outputs {
            effect,
            room: RoomMap::default(),
            cases: Vec::new(),
            lamps: Vec::new(),
            keyboard: false,
            keyboard_parts: super::boards::lamp_parts(None),
            pacing: Pacing::default(),
            sent_case: BTreeMap::new(),
            sent_lamps: BTreeMap::new(),
            sent_keyboard: None,
            keyboard_effect: None,
            effect_sent: false,
            keyboard_effects: Arc::default(),
            keyboard_rings: Arc::default(),
            keyboard_again: Arc::default(),
            keyboard_held: Arc::default(),
            keyboard_lamps: None,
            bursts: Bursts::default(),
            ambient: Ambient::new(),
            glow,
            screen_feed: None,
            seen_pictures: 0,
            kick: true,
            held_seen: false,
            busy_until: 0,
            hand_backs: 0,
            handed_at: None,
            hand_back_reported: false,
            areas_shown: BTreeMap::new(),
        };
        o.set_room(room, cases);
        o
    }

    /// How long frames keep coming at the tick after a change: every device's interval, and
    /// one Screen glow update.
    fn linger_ms(&self) -> i64 {
        self.pacing.longest().as_millis() as i64 + foxglove_scene::screen::MIN_UPDATE_MS
    }

    /// When the scene's next frame is due (#1085). Effects that move with time tick. Screen
    /// glow ticks only while there is something to draw: a picture not drawn yet, the
    /// smoothing, the ambient easing or the flash limit still settling, a burst, a change that
    /// every device must get, or a fade. While capture failing holds the colours it waits for
    /// the fade; otherwise it sleeps until woken (a new picture, a control event).
    pub fn pace(&self, t: SceneTime) -> Pace {
        let Some(g) = &self.glow else {
            return Pace::Tick;
        };
        let picture = self
            .screen_feed
            .as_ref()
            .is_some_and(|f| f.pictures() != self.seen_pictures);
        let fading = g.handing_back() && g.fade_starts(t).is_none() && !self.hand_back_reported;
        if picture
            || fading
            || t.ms() < self.busy_until
            || !self.bursts.is_empty()
            || !self.ambient.settled()
        {
            return Pace::Tick;
        }
        match g.fade_starts(t) {
            Some(at) => Pace::After(Duration::from_millis((at.ms() - t.ms()).max(1) as u64)),
            None => Pace::Idle,
        }
    }

    /// Screen glow hands the lights back from `t` (#1085: a full-screen game, capture failing):
    /// each light fades to what it shows once the scene lets it go. The case's boards to
    /// `case_own` (by key: their colours from before the scene, `pc_lights`), the keyboard's
    /// halves to the mix they show now (its lease's colour, which it shows once its frames go
    /// back). Dynamic Lighting devices go back to Windows' own lights, which Foxglove can't
    /// read: they keep their colour until the fade ends. Returns the hand-back's generation,
    /// for [`Outputs::take_handed_back`]; none without Screen glow.
    pub fn hand_back(
        &mut self,
        why: foxglove_scene::screen::HandBack,
        case_own: &super::pc_lights::BoardColours,
        t: SceneTime,
    ) -> Option<u64> {
        let g = self.glow.as_mut()?;
        let mut own = BTreeMap::new();
        for c in &self.cases {
            for &(i, colour) in case_own.get(&c.key).into_iter().flatten() {
                if let Some(zone) = c.zone_ids.get(i) {
                    own.insert((c.room_id(), zone.to_string()), Rgb(colour));
                }
            }
        }
        if self.keyboard && !self.keyboard_held.load(Ordering::Relaxed) {
            let halves: Vec<_> = g
                .shown()
                .into_iter()
                .filter(|((d, _), _)| d.starts_with(KEYBOARD))
                .collect();
            let colours: Vec<Rgb> = halves.iter().map(|(_, c)| *c).collect();
            if let Some(mix) = mixed(&colours) {
                own.extend(halves.into_iter().map(|(k, _)| (k, mix)));
            }
        }
        g.hand_back(why, own, self.areas_shown.clone(), t);
        self.hand_backs += 1;
        self.handed_at = None;
        self.hand_back_reported = false;
        self.kick = true;
        Some(self.hand_backs)
    }

    /// The hold ended: Screen glow eases back to the screen from where the fade left the lights.
    pub fn resume(&mut self) {
        if let Some(g) = self.glow.as_mut() {
            g.resume();
        }
        self.handed_at = None;
        self.hand_back_reported = false;
        self.kick = true;
    }

    /// Once per hand-back, after the frame at `t`: every light reached its own colour and every
    /// device has had that frame, so the scene may let them go. The hand-back's generation.
    pub fn take_handed_back(&mut self, t: SceneTime) -> Option<u64> {
        let g = self.glow.as_ref()?;
        if self.hand_back_reported || !g.handed_back(t) {
            return None;
        }
        let at = *self.handed_at.get_or_insert(t.ms());
        if t.ms() < at + self.linger_ms() {
            return None;
        }
        self.hand_back_reported = true;
        Some(self.hand_backs)
    }

    /// Where Screen glow finds the screens' newest edge colours (#496).
    pub fn set_screen_feed(&mut self, feed: Arc<super::screen_glow::Feed>) {
        self.screen_feed = Some(feed);
    }

    /// A ring or sweep over the scene from its next frame on (at most four at once).
    pub fn burst(&mut self, b: Burst) {
        self.bursts.add(b);
        self.kick = true;
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

    /// A flag set while someone else has the keyboard's lights (#749), from the lights owner
    /// (`lights_owner::State::watch_hold`): the keyboard gets nothing while it is.
    pub fn keyboard_held(&self) -> Arc<AtomicBool> {
        self.keyboard_held.clone()
    }

    /// A flag saying the keyboard takes SET_EFFECT (#542), shared with the keyboard's writer:
    /// while set, a scene that maps goes to the keyboard as one effect instead of frames.
    pub fn keyboard_effects(&self) -> Arc<AtomicBool> {
        self.keyboard_effects.clone()
    }

    /// A flag saying the keyboard draws key rings itself (#490), shared with the keyboard's
    /// writer: while set (with [`Self::keyboard_effects`]), a key-rings look goes to the
    /// keyboard as one SET_EFFECT kind 5 and its rings never become frames.
    pub fn keyboard_rings(&self) -> Arc<AtomicBool> {
        self.keyboard_rings.clone()
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
        self.kick = true;
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
        self.kick = true;
    }

    /// The Dynamic Lighting devices found now (#503). Each one in the room gets its next frame
    /// in full.
    pub fn set_lamps(&mut self, lamps: Vec<LampTarget>) {
        self.lamps = lamps;
        self.repace();
    }

    /// The keyboard's own LampArray, paired with it ([`keyboard_lamp_key`]), or none (#749 b):
    /// while Windows Dynamic Lighting holds the keyboard's lights, the keyboard is drawn through
    /// it instead of the host link. Its next frame goes in full.
    pub fn set_keyboard_lamps(&mut self, lamps: Option<KeyboardLamps>) {
        if lamps == self.keyboard_lamps {
            return;
        }
        self.keyboard_lamps = lamps;
        self.repace();
    }

    /// The Dynamic Lighting devices' next frames go in full: they were let go (pause, away)
    /// and show Windows' own lights until then.
    pub fn redraw_lamps(&mut self) {
        self.sent_lamps.clear();
        self.kick = true;
    }

    /// Each output's pacing from the room and the targets; every device's next frame in full.
    fn repace(&mut self) {
        let mut paced: Vec<(String, Option<Duration>)> = self
            .cases
            .iter()
            .map(|c| (c.room_id(), Some(c.interval)))
            .collect();
        paced.extend(
            self.lamps
                .iter()
                .filter(|l| self.room.device(&l.room_id()).is_some())
                .map(|l| (l.room_id(), Some(l.interval()))),
        );
        if self.keyboard {
            paced.push((KEYBOARD.into(), Some(keyboard_interval())));
            if let Some(k) = &self.keyboard_lamps {
                paced.push((KEYBOARD_LAMPS.into(), Some(k.target.interval())));
            }
        }
        self.pacing = Pacing::new(paced);
        self.sent_case.clear();
        self.sent_lamps.clear();
        self.sent_keyboard = None;
        self.kick = true;
    }

    /// Another look while the scene runs (the Scenes page, #487): every device gets the next
    /// frame in full. The ambient lights fade across to the new look rather than jumping (so
    /// the case is written at its own rate until they settle, a few seconds).
    pub fn set_effect(&mut self, effect: Effect) {
        self.glow = glow_for(&effect, self.glow.take());
        self.effect = effect;
        self.sent_case.clear();
        self.sent_lamps.clear();
        self.sent_keyboard = None;
        self.keyboard_effect = keyboard_effect(&self.effect, &self.room, &self.keyboard_parts);
        self.effect_sent = false;
        self.kick = true;
    }

    /// The writes for scene time `t`: each due device whose colours changed.
    pub fn frame(&mut self, t: SceneTime) -> Vec<Out> {
        if self.keyboard_again.swap(false, Ordering::Relaxed) {
            self.sent_keyboard = None;
            self.effect_sent = false;
            self.kick = true;
        }
        let held = self.keyboard_held.load(Ordering::Relaxed);
        if held != self.held_seen {
            self.held_seen = held;
            self.kick = true;
        }
        if std::mem::take(&mut self.kick) {
            self.busy_until = self.busy_until.max(t.ms() + self.linger_ms());
        }
        // Screen glow: at most 20 updates a second reach the lights (#496).
        if self.glow.as_ref().is_some_and(|g| !g.due(t)) {
            return Vec::new();
        }
        let mut due = self.pacing.due(t);
        if self.keyboard_held.load(Ordering::Relaxed) {
            // Held (#749): the keyboard streams nothing over the host link until the lights are
            // free, and then gets its frame or effect in full. Its own LampArray, when paired,
            // takes the scene through Windows meanwhile (b).
            due.retain(|id| id != KEYBOARD);
            self.sent_keyboard = None;
            self.effect_sent = false;
        } else {
            // Free: the host link only, and the next hold draws the LampArray in full.
            due.retain(|id| id != KEYBOARD_LAMPS);
            if let Some(k) = &self.keyboard_lamps {
                self.sent_lamps.remove(&k.target.key);
            }
        }
        if due.is_empty() {
            return Vec::new();
        }
        let bursting = self.bursts.prune(t);
        let mut out = Vec::new();
        // The keyboard draws the scene itself: sent once, then nothing per frame. A burst over
        // the scene is drawn by frames (they take over from the effect on the keyboard), and
        // the effect goes again once it is gone. Key rings (#490) are the exception: the
        // keyboard draws its own rings, so it keeps the effect while Foxglove's rings from the
        // same presses cross the rest of the room; without KEY_RINGS they are frames.
        let rings = matches!(self.effect, Effect::KeyRings { .. });
        let effect = self.keyboard_effect.as_ref().filter(|_| {
            self.keyboard
                && self.keyboard_effects.load(Ordering::Relaxed)
                && if rings {
                    self.keyboard_rings.load(Ordering::Relaxed)
                } else {
                    !bursting
                }
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
        let mut frame = match (self.glow.as_mut(), &self.screen_feed) {
            (Some(g), Some(feed)) => {
                // Counted before the read: a picture arriving meanwhile is drawn next time.
                self.seen_pictures = feed.pictures();
                g.render(&self.room, &feed.latest(), t)
            }
            (Some(g), None) => g.render(&self.room, &[], t),
            (None, _) => self.effect.render(&self.room, t),
        };
        let handing_back = self.glow.as_ref().is_some_and(|g| g.handing_back());
        // Still settling (a fade paces itself: [`Outputs::pace`]).
        if self
            .glow
            .as_ref()
            .is_some_and(|g| !g.handing_back() && !g.settled())
        {
            self.busy_until = self.busy_until.max(t.ms() + self.linger_ms());
        }
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
        if handing_back {
            // The fade eases the area lights itself; after it they start again from there.
            self.ambient.reset();
        } else {
            self.ambient.apply(&mut frame, &self.room);
            if self.glow.is_some() {
                let places = self.room.devices.iter().flat_map(|d| d.lights.iter());
                self.areas_shown = frame
                    .lights
                    .iter()
                    .zip(places)
                    .filter(|(_, p)| p.role == LightRole::Ambient)
                    .map(|(l, _)| ((l.device.clone(), l.light.clone()), l.colour))
                    .collect();
            }
        }
        for id in due {
            if id == KEYBOARD_LAMPS {
                let Some((key, colors)) = self.keyboard_lamp_colours(&frame, t, bursting) else {
                    continue;
                };
                if self.sent_lamps.get(&key) != Some(&colors) {
                    self.sent_lamps.insert(key.clone(), colors.clone());
                    out.push(Out::Lamps { key, colors });
                }
                continue;
            }
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
        if self.glow.is_some() && !out.is_empty() {
            super::screen_glow::sent();
        }
        out
    }

    /// The keyboard's colours through its own LampArray (#749 b), with the writer's key: each
    /// lamp the scene's colour at its place on the keyboard's rect in the room (both halves
    /// together, as the array's bounding box spans them), a burst drawn over it as on the host
    /// link. Without positions, one colour for all: the keyboard's lights mixed.
    fn keyboard_lamp_colours(
        &self,
        frame: &foxglove_scene::Frame,
        t: SceneTime,
        bursting: bool,
    ) -> Option<(String, Vec<[u8; 3]>)> {
        let k = self.keyboard_lamps.as_ref()?;
        let parts = keyboard_parts(&self.room)?;
        let (first, rest) = parts.split_first()?;
        let rect = rest.iter().fold(first.rect, |r, d| r.union(&d.rect));
        let bounds = &self.room.bounds;
        // Screen glow's colours aren't a formula of the place: the keyboard's lights mixed.
        let colors = if k.places.is_empty() || self.glow.is_some() {
            let lights: Vec<Rgb> = frame
                .lights
                .iter()
                .filter(|l| l.device.starts_with(KEYBOARD))
                .map(|l| l.colour)
                .collect();
            vec![mixed(&lights)?.0]
        } else {
            k.places
                .iter()
                .map(|&[fx, fy]| {
                    let at = foxglove_scene::Point::new(rect.x + fx * rect.w, rect.y + fy * rect.h);
                    let c = self.effect.colour(at, t, bounds);
                    let c = if bursting {
                        self.bursts.over(c, at, t, bounds)
                    } else {
                        c
                    };
                    c.0
                })
                .collect()
        };
        // The keyboard's calibration, as its host-link colour gets it (before the "changed?"
        // check, so a new calibration shows on the next frame that differs).
        let colors = match &k.calibration {
            Some(cal) => colors
                .into_iter()
                .map(|c| super::calibrate::on_device(cal, None, c))
                .collect(),
            None => colors,
        };
        Some((k.target.key.clone(), colors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_scene::{DeviceInfo, DeviceKind, LightSpec, MonitorInfo, Point};

    const ORANGE: Rgb = Rgb([0xf0, 0x8a, 0x3c]);

    /// #496 on a recorded frame (the screen's left half blue, its right half red): the case on
    /// the screen's right takes the right edge's red, the keyboard takes frames (never a firmware
    /// effect), and at most 20 updates a second go out.
    #[test]
    fn screen_glow_lights_the_room_from_the_recorded_edges() {
        use foxglove_scene::screen::{Edges, ScreenImage, Smoothing};
        let (room, cases) = desk();
        let mut o = Outputs::new(
            Effect::Screen {
                smoothing: Smoothing::Quick,
            },
            room,
            cases,
        );
        o.keyboard_effects().store(true, Ordering::Relaxed);
        let feed = Arc::new(crate::foxglove::screen_glow::Feed::default());
        o.set_screen_feed(feed.clone());
        let mut img = ScreenImage::filled(r"\\.\DISPLAY1", 256, 144, [20, 40, 230]);
        img.paint((128, 0, 256, 144), [230, 20, 20]);
        feed.put(Edges::of(&img));
        let (mut updates, mut case, mut keyboard) = (0, None, 0);
        // The scene's frame thread at 30 a second, for a second.
        for i in 0..30 {
            let outs = o.frame(SceneTime(i * 1000 / 30));
            if !outs.is_empty() {
                updates += 1;
            }
            for out in outs {
                match out {
                    Out::Case { colors, .. } => case = Some(colors),
                    Out::Keyboard(_) => keyboard += 1,
                    Out::KeyboardEffect(_) => panic!("Screen glow has no firmware effect"),
                    Out::Lamps { .. } => {}
                }
            }
        }
        assert!(updates <= 20, "{updates} updates in a second");
        assert!(keyboard >= 1, "the keyboard gets frames");
        let case = case.expect("the case was lit");
        for (zone, [r, _, b]) in &case {
            assert!(*r > 150 && *b < 120, "zone {zone}: {case:?}");
        }
        assert!(keyboard_effect(
            &Effect::Screen {
                smoothing: Smoothing::Even
            },
            o.room(),
            &crate::foxglove::boards::lamp_parts(None)
        )
        .is_none());
        // Another look: Screen glow's state goes with it.
        o.set_effect(crate::foxglove::scene_run::demo_effect());
        assert!(o.glow.is_none());
    }

    fn screen_of(c: [u8; 3]) -> foxglove_scene::screen::Edges {
        use foxglove_scene::screen::{Edges, ScreenImage};
        Edges::of(&ScreenImage::filled(r"\\.\DISPLAY1", 256, 144, c))
    }

    /// A Screen glow on the desk, lit by a red screen for a second at 30 frames a second; the
    /// next frame's time.
    fn glowing_red(
        smoothing: foxglove_scene::Smoothing,
    ) -> (Outputs, Arc<crate::foxglove::screen_glow::Feed>, i64) {
        let (room, cases) = desk();
        let mut o = Outputs::new(Effect::Screen { smoothing }, room, cases);
        let feed = Arc::new(crate::foxglove::screen_glow::Feed::default());
        o.set_screen_feed(feed.clone());
        feed.put(screen_of([230, 20, 20]));
        let mut t = 0;
        while t < 1000 {
            o.frame(SceneTime(t));
            t += 1000 / 30;
        }
        (o, feed, t)
    }

    /// Every case write in `outs`, by zone index.
    fn case_of(outs: &[Out]) -> Option<Vec<(usize, [u8; 3])>> {
        outs.iter().find_map(|o| match o {
            Out::Case { colors, .. } => Some(colors.clone()),
            _ => None,
        })
    }

    /// #1085: a full-screen game. The case fades over 1 s to its own colours (its area light
    /// too), the keyboard's halves to the mix they show (the lease's colour it shows once its
    /// frames go back), and only then is the hand-back reported, once, so the scene lets them
    /// go. The hold's end eases back to the screen.
    #[test]
    fn a_game_fades_the_lights_to_their_own_colours_then_hands_them_back() {
        use foxglove_scene::screen::{HandBack, GAME_FADE_MS};
        let (mut o, feed, t0) = glowing_red(foxglove_scene::Smoothing::Even);
        let blue = [20, 40, 230];
        let own = BTreeMap::from([("103c:84fd".to_string(), (0..5).map(|i| (i, blue)).collect())]);
        let generation = o.hand_back(HandBack::Game, &own, SceneTime(t0)).unwrap();
        let mut case = None;
        let mut mid = None;
        let mut reported = None;
        let mut t = t0;
        while t < t0 + 3000 {
            let outs = o.frame(SceneTime(t));
            if let Some(c) = case_of(&outs) {
                if t - t0 >= 450 && mid.is_none() {
                    mid = Some(c.clone());
                }
                case = Some(c);
            }
            if let Some(g) = o.take_handed_back(SceneTime(t)) {
                assert!(reported.is_none(), "reported once");
                reported = Some((g, t));
            }
            if reported.is_none() {
                assert_eq!(
                    o.pace(SceneTime(t)),
                    Pace::Tick,
                    "the fade draws at the tick"
                );
            }
            t += 1000 / 30;
        }
        let (g, at) = reported.expect("handed back");
        assert_eq!(g, generation);
        assert!(
            at >= t0 + GAME_FADE_MS && at < t0 + GAME_FADE_MS + 500,
            "{at}"
        );
        let case = case.unwrap();
        assert!(
            case.iter().all(|(_, c)| *c == blue),
            "the case's own colours: {case:?}"
        );
        let mid = mid.unwrap();
        assert!(
            mid.iter().all(|(_, [r, _, b])| *r > 40 && *b > 60),
            "half way, red to blue: {mid:?}"
        );
        // The keyboard's halves: both at the mix they showed, red.
        let shown = o.glow.as_ref().unwrap().shown();
        for half in ["keyboard:left", "keyboard:right"] {
            let c = shown[&(half.to_string(), "all".to_string())];
            assert!(c.0[0] > 150 && c.0[2] < 80, "{half}: {c:?}");
        }
        // Handed back, nothing more to draw: it sleeps until the hold ends.
        assert_eq!(o.pace(SceneTime(t)), Pace::Idle);
        // The game ends: the screen is green, the case eases back from blue.
        o.resume();
        feed.put(screen_of([20, 210, 40]));
        let first = loop {
            if let Some(c) = case_of(&o.frame(SceneTime(t))) {
                break c;
            }
            t += 1000 / 30;
        };
        assert!(
            first.iter().all(|(_, [_, g, b])| *g < 200 && *b > 30),
            "no jump to the screen: {first:?}"
        );
    }

    /// #1085 review: the case's area light (zone 2) eases behind its neighbours; a hand-back
    /// fades it on from the colour it showed, not from its neighbours' peak.
    #[test]
    fn an_area_light_fades_on_from_what_it_showed() {
        use foxglove_scene::screen::HandBack;
        let (mut o, feed, mut t) = glowing_red(foxglove_scene::Smoothing::Quick);
        // The screen turns blue: the zones follow at once, the area light lags behind.
        feed.put(screen_of([20, 40, 230]));
        let mut last = None;
        for _ in 0..6 {
            if let Some(c) = case_of(&o.frame(SceneTime(t))) {
                last = Some(c);
            }
            t += 1000 / 30;
        }
        let last = last.unwrap();
        let area = last[1].1;
        let zone1 = last[0].1;
        let gap = |a: [u8; 3], b: [u8; 3]| (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap();
        assert!(gap(area, zone1) > 40, "the area light lags: {last:?}");
        let blue = [20, 40, 230];
        let own = BTreeMap::from([("103c:84fd".to_string(), (0..5).map(|i| (i, blue)).collect())]);
        o.hand_back(HandBack::Game, &own, SceneTime(t)).unwrap();
        let first = loop {
            if let Some(c) = case_of(&o.frame(SceneTime(t))) {
                break c;
            }
            t += 1000 / 30;
        };
        assert!(
            gap(first[1].1, area) <= 12,
            "no jump: {area:?} -> {:?}",
            first[1].1
        );
    }

    /// #1085: capture failing holds the last colours for 2 s with no frame meanwhile (the
    /// thread waits for the fade's start), then fades over 3 s.
    #[test]
    fn a_failure_waits_out_the_hold_then_fades() {
        use foxglove_scene::screen::{HandBack, FAIL_FADE_MS, FAIL_HOLD_MS};
        let (mut o, _feed, t0) = glowing_red(foxglove_scene::Smoothing::Even);
        let own = BTreeMap::from([(
            "103c:84fd".to_string(),
            (0..5).map(|i| (i, [255, 255, 255])).collect(),
        )]);
        o.hand_back(HandBack::Failed, &own, SceneTime(t0)).unwrap();
        // The change is drawn to every device first, then it waits.
        let mut t = t0;
        while o.pace(SceneTime(t)) == Pace::Tick {
            o.frame(SceneTime(t));
            t += 1000 / 30;
            assert!(t < t0 + 1000, "the change settles");
        }
        match o.pace(SceneTime(t)) {
            Pace::After(d) => {
                let left = (t0 + FAIL_HOLD_MS - t) as u128;
                assert!(d.as_millis().abs_diff(left) <= 1, "{d:?} vs {left} ms");
            }
            p => panic!("waits for the fade: {p:?}"),
        }
        let held = case_of(&o.frame(SceneTime(t0 + FAIL_HOLD_MS - 1)));
        assert!(held.is_none(), "nothing changes while held: {held:?}");
        let mut end = None;
        let mut t = t0 + FAIL_HOLD_MS;
        while end.is_none() {
            o.frame(SceneTime(t));
            end = o.take_handed_back(SceneTime(t)).map(|_| t);
            t += 1000 / 30;
            assert!(t < t0 + 10_000);
        }
        assert!(end.unwrap() >= t0 + FAIL_HOLD_MS + FAIL_FADE_MS);
    }

    /// #1085: on a still screen the scene's frame thread sleeps. Measured on a real scheduler
    /// with recorded pictures: after a picture it wakes only while the lights settle, then not
    /// at all; the next picture wakes it again.
    #[test]
    fn a_still_screen_lets_the_frame_thread_sleep() {
        use foxglove_scene::scheduler::{Gate, Scheduler, MIN_INTERVAL};
        use std::sync::Mutex;
        let (mut room, cases) = desk();
        // Point lights only: an area light's slow easing would keep it ticking for seconds.
        for d in &mut room.devices {
            for l in &mut d.lights {
                l.role = LightRole::Point;
            }
        }
        let o = Arc::new(Mutex::new(Outputs::new(
            Effect::Screen {
                smoothing: foxglove_scene::Smoothing::Quick,
            },
            room,
            cases,
        )));
        let feed = Arc::new(crate::foxglove::screen_glow::Feed::default());
        o.lock().unwrap().set_screen_feed(feed.clone());
        let frames = o.clone();
        let sched = Scheduler::paced(
            foxglove_scene::Clock::starting_now(),
            MIN_INTERVAL,
            move |t| {
                let mut o = frames.lock().unwrap();
                o.frame(t);
                o.pace(t)
            },
        );
        feed.wake_with(sched.waker());
        feed.put(screen_of([230, 20, 20]));
        sched.set(Gate::Scene, true);
        // The wake count once it stops growing (bounded).
        let settled = || {
            let mut last = sched.frames();
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(150));
                let now = sched.frames();
                if now == last {
                    return now;
                }
                last = now;
            }
            panic!("the frame thread never slept: {last} frames");
        };
        let first = settled();
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(sched.frames(), first, "no wake-up on a still screen");
        feed.put(screen_of([20, 40, 230]));
        let second = settled();
        let woke = second - first;
        assert!(
            (1..=30).contains(&woke),
            "{woke} frames for one new picture"
        );
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(sched.frames(), second, "asleep again");
        eprintln!("still screen: {first} frames to settle the first picture, {woke} for the next, 0 while still");
    }

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
            interval: CASE_INTERVAL,
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
            interval: CASE_INTERVAL,
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
            // It says it takes 100 frames a second: still paced at the rates file's least.
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
        // The pacing's tenth of slack, and the 30 fps cap.
        let least = target
            .interval()
            .max(foxglove_scene::scheduler::MIN_INTERVAL);
        let gap = least.as_millis() as i64 * 9 / 10;
        assert!(
            sent.windows(2).all(|w| w[1].0 - w[0].0 >= gap),
            "never faster than its interval"
        );
        assert!(
            sent.len() as i64 <= 2000 / gap + 1,
            "{} frames in 2 s",
            sent.len()
        );
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
        assert_eq!(t(0).interval(), lamp_interval("k"));
        assert_eq!(lamp_interval("k"), LAMP_INTERVAL, "the bundled default");
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

    /// #749: while Windows Dynamic Lighting holds the keyboard's lights, the keyboard streams
    /// nothing (frames or its effect) and the case keeps its frames; once they're free, the
    /// keyboard gets its frame or effect in full, once.
    #[test]
    fn a_held_keyboard_streams_nothing_and_gets_it_all_after() {
        let count = |outs: &[Out]| -> (usize, usize, usize) {
            let e = outs
                .iter()
                .filter(|o| matches!(o, Out::KeyboardEffect(_)))
                .count();
            let f = outs
                .iter()
                .filter(|o| matches!(o, Out::Keyboard(_)))
                .count();
            (e, f, outs.len() - e - f)
        };
        for effects in [false, true] {
            let (room, cases) = desk();
            let mut o = Outputs::new(scanline(), room, cases);
            o.keyboard_effects().store(effects, Ordering::Relaxed);
            let held = o.keyboard_held();
            let run = |o: &mut Outputs, from: i64, to: i64| {
                let mut sum = (0, 0, 0);
                for ms in (from..to).step_by(33) {
                    let (e, f, c) = count(&o.frame(SceneTime(ms)));
                    sum = (sum.0 + e, sum.1 + f, sum.2 + c);
                }
                sum
            };
            let (e, f, _) = run(&mut o, 0, 500);
            assert!(e + f > 0, "streams before");
            held.store(true, Ordering::Relaxed);
            let (e, f, case) = run(&mut o, 500, 3000);
            assert_eq!((e, f), (0, 0), "nothing for the keyboard while held");
            assert!(case > 10, "the case still gets its frames: {case}");
            held.store(false, Ordering::Relaxed);
            let (e, f, _) = run(&mut o, 3000, 3100);
            if effects {
                assert_eq!((e, f), (1, 0), "the effect again, once");
            } else {
                assert!(f >= 1 && e == 0, "frames again");
            }
        }
    }

    fn kb_array(key: &str, bluetooth: bool) -> foxglove_devices::pc_lighting::KeyboardLampArray {
        let (vid, pid, _) = key_parts(key).unwrap();
        foxglove_devices::pc_lighting::KeyboardLampArray {
            key: key.into(),
            name: "Some keyboard".into(),
            vid,
            pid,
            bluetooth,
        }
    }

    /// #749 b: a host-link keyboard pairs with its own LampArray by VID/PID (and serial when
    /// both say one), whatever the model; the writer's key opens its USB and BLE interfaces.
    #[test]
    fn a_link_keyboard_pairs_with_its_own_lamparray_by_vid_pid() {
        let arrays = [
            kb_array("1d50:615e:ABC123", false),
            kb_array("1d50:615e", true),
        ];
        assert_eq!(
            keyboard_lamp_key("1d50:615e", &arrays).as_deref(),
            Some("1d50:615e")
        );
        assert_eq!(
            keyboard_lamp_key("1d50:615e:ABC123", &arrays).as_deref(),
            Some("1d50:615e"),
            "the serial matches the USB interface's"
        );
        assert_eq!(
            keyboard_lamp_key("1d50:615e:OTHER", &[kb_array("1d50:615e:ABC123", false)]),
            None,
            "another keyboard of the same model"
        );
        assert_eq!(
            keyboard_lamp_key("1d50:615e:OTHER", &arrays).as_deref(),
            Some("1d50:615e"),
            "the BLE interface carries no serial"
        );
        assert_eq!(keyboard_lamp_key("feed:0001", &arrays), None, "not its");
        assert_eq!(keyboard_lamp_key("1d50:615e", &[]), None, "no LampArray");
        assert_eq!(keyboard_lamp_key("not a key", &arrays), None);
    }

    #[test]
    fn keyboard_lamps_take_their_places_from_windows() {
        let at = [[0.0, 0.0], [100.0, 50.0], [300.0, 100.0]];
        let k = KeyboardLamps::new("k".into(), Some(([200.0, 100.0], &at[..])));
        assert_eq!(k.target.lamps, 3);
        assert_eq!(k.places, [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]);
        let unplaced = KeyboardLamps::new("k".into(), Some(([200.0, 100.0], &[[0.0; 2]; 4][..])));
        assert_eq!((unplaced.target.lamps, unplaced.places.len()), (4, 0));
        let unread = KeyboardLamps::new("k".into(), None);
        assert_eq!((unread.target.lamps, unread.places.len()), (0, 0));
        assert_eq!(unread.target.interval(), LAMP_INTERVAL);
    }

    /// The Corne's LampArray as a firmware might describe it: 42 lamps across a box spanning
    /// both halves, left to right.
    fn corne_lamps() -> KeyboardLamps {
        let at: Vec<[f32; 2]> = (0..42).map(|i| [i as f32 * 7.0, 30.0]).collect();
        KeyboardLamps::new("1d50:615e".into(), Some(([294.0, 60.0], &at[..])))
    }

    /// #749 b: held, the keyboard is drawn through its own LampArray (a colour per lamp at its
    /// place, paced and only on change) with nothing on the host link; freed, the host link
    /// gets its frame or effect in full and the LampArray nothing; held again, the LampArray
    /// in full.
    #[test]
    fn a_held_keyboard_with_a_lamparray_is_drawn_through_windows() {
        for effects in [false, true] {
            let (room, cases) = desk();
            let sweep = Effect::Sweep {
                stops: vec![ORANGE, Rgb([0x20, 0x40, 0xf0])],
                angle_deg: 0.0,
                period_ms: 2000,
            };
            let mut o = Outputs::new(sweep, room, cases);
            o.keyboard_effects().store(effects, Ordering::Relaxed);
            o.set_keyboard_lamps(Some(corne_lamps()));
            let held = o.keyboard_held();
            // (host link, keyboard LampArray writes with their times, case)
            let run = |o: &mut Outputs, from: i64, to: i64| {
                let (mut link, mut lamps, mut case) = (0, Vec::new(), 0);
                for ms in (from..to).step_by(10) {
                    for out in o.frame(SceneTime(ms)) {
                        match out {
                            Out::Keyboard(_) | Out::KeyboardEffect(_) => link += 1,
                            Out::Lamps { key, colors } => {
                                assert_eq!(key, "1d50:615e");
                                lamps.push((ms, colors));
                            }
                            Out::Case { .. } => case += 1,
                        }
                    }
                }
                (link, lamps, case)
            };
            let (link, lamps, _) = run(&mut o, 0, 500);
            assert!(link > 0 && lamps.is_empty(), "free: the host link only");
            held.store(true, Ordering::Relaxed);
            let (link, lamps, case) = run(&mut o, 500, 2500);
            assert_eq!(link, 0, "held: nothing on the host link");
            assert!(case > 10, "the case keeps its frames");
            assert!(
                lamps.len() > 5,
                "the scene through Windows: {}",
                lamps.len()
            );
            assert!(
                lamps.iter().all(|(_, c)| c.len() == 42),
                "a colour per lamp"
            );
            assert!(
                lamps.windows(2).all(|w| w[1].0 - w[0].0 >= 45),
                "never faster than LAMP_INTERVAL"
            );
            assert!(
                lamps.iter().any(|(_, c)| c[0] != c[41]),
                "lamps at their own places"
            );
            held.store(false, Ordering::Relaxed);
            let (link, lamps, _) = run(&mut o, 2500, 2600);
            assert!(link >= 1, "freed: the host link in full");
            assert!(lamps.is_empty(), "and nothing through Windows");
            if effects {
                assert_eq!(link, 1, "the effect again, once");
            }
            held.store(true, Ordering::Relaxed);
            let (link, lamps, _) = run(&mut o, 2600, 2700);
            assert_eq!(link, 0);
            assert!(!lamps.is_empty(), "held again: the LampArray in full");
        }
    }

    /// A still look through the keyboard's LampArray: one write, then none until it is freed
    /// and held again. Without lamp positions: one colour for every lamp.
    #[test]
    fn a_held_keyboard_lamparray_gets_only_changes() {
        let (room, cases) = desk();
        let still = Effect::Breathe {
            from: ORANGE,
            to: ORANGE,
            period_ms: 4000,
        };
        let mut o = Outputs::new(still, room, cases);
        o.set_keyboard_lamps(Some(KeyboardLamps::new(
            "1d50:615e".into(),
            Some(([0.0; 2], &[[0.0; 2]; 42][..])),
        )));
        o.keyboard_held().store(true, Ordering::Relaxed);
        let lamps = |o: &mut Outputs, from: i64| -> Vec<Vec<[u8; 3]>> {
            (from..from + 1000)
                .step_by(10)
                .flat_map(|ms| o.frame(SceneTime(ms)))
                .filter_map(|out| match out {
                    Out::Lamps { colors, .. } => Some(colors),
                    _ => None,
                })
                .collect()
        };
        let first = lamps(&mut o, 0);
        assert_eq!(first.len(), 1, "once: {first:?}");
        assert_eq!(first[0].len(), 1, "one colour for every lamp");
        assert!(
            (0..3).all(|i| first[0][0][i].abs_diff(ORANGE.0[i]) <= 1),
            "{first:?}"
        );
        assert!(lamps(&mut o, 1000).is_empty());
        // No LampArray paired (none listed): held means nothing for the keyboard at all.
        o.set_keyboard_lamps(None);
        assert!(lamps(&mut o, 2000).is_empty());
    }

    /// #749 b follow-up: the keyboard drawn through its LampArray takes the keyboard's colour
    /// calibration on every lamp, as its host-link frames do; the scene still animates (one
    /// write per changed frame, at the LampArray's pace).
    #[test]
    fn a_held_keyboard_lamparray_goes_through_the_keyboards_calibration() {
        use foxglove_devices::calibration::{self, Calibration, DeviceCalibration};
        let cal_key = "1d50:615e:SCENE749D";
        let cal = DeviceCalibration {
            device: Calibration {
                gain: [0.5, 1.0, 0.75],
                ..Default::default()
            },
            ..Default::default()
        };
        calibration::global().set_device(cal_key, Some(cal.clone()));
        let breathe = Effect::Breathe {
            from: ORANGE,
            to: Rgb([0x20, 0x40, 0xf0]),
            period_ms: 1000,
        };
        let writes = |calibrated: bool| -> Vec<Vec<[u8; 3]>> {
            let (room, cases) = desk();
            let mut o = Outputs::new(breathe.clone(), room, cases);
            let lamps = corne_lamps();
            o.set_keyboard_lamps(Some(if calibrated {
                lamps.calibrated(Some(cal_key.into()))
            } else {
                lamps
            }));
            o.keyboard_held().store(true, Ordering::Relaxed);
            (0..1000)
                .step_by(10)
                .flat_map(|ms| o.frame(SceneTime(ms)))
                .filter_map(|out| match out {
                    Out::Lamps { colors, .. } => Some(colors),
                    _ => None,
                })
                .collect()
        };
        let (raw, sent) = (writes(false), writes(true));
        assert!(
            raw.len() > 5,
            "Breathe animates through Windows: {}",
            raw.len()
        );
        let through = |c: [u8; 3]| {
            let o = cal.zone(None).apply_rgb(foxglove_devices::Rgb {
                r: c[0],
                g: c[1],
                b: c[2],
            });
            [o.r, o.g, o.b]
        };
        let expected: Vec<Vec<[u8; 3]>> = raw
            .iter()
            .map(|f| f.iter().map(|&c| through(c)).collect())
            .collect();
        // Calibration can make two frames equal that weren't; every calibrated write is one
        // of the expected ones, in order.
        let mut it = expected.iter();
        assert!(
            sent.iter().all(|s| it.any(|e| e == s)),
            "calibrated writes: {sent:?}"
        );
        assert!(sent.len() > 5);
        assert_ne!(sent[0], raw[0], "the calibration changes the colours");
        calibration::global().set_device(cal_key, None);
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
        // #871: tones with no LED off the first accent are the firmware's breathe from the
        // accent to the second one; split tones and Tide need frames.
        let fade = Effect::Tones {
            colours: vec![ORANGE, blue],
            tones: Default::default(),
            period_ms: 8000,
            floor: 0.35,
        };
        let e = keyboard_effect(&fade, &room).unwrap();
        assert_eq!(
            (e.effect.kind, e.effect.colours.clone(), e.effect.period_ms),
            (EffectKind::Breathe, vec![ORANGE.0, blue.0], 8000)
        );
        let split = Effect::Tones {
            tones: [(foxglove_scene::tone_key("keyboard:left", "0"), 1u8)]
                .into_iter()
                .collect(),
            colours: vec![ORANGE, blue],
            period_ms: 8000,
            floor: 0.35,
        };
        assert!(keyboard_effect(&split, &room).is_none());
        let tide = Effect::Tide {
            colours: vec![ORANGE, blue],
            tones: Default::default(),
            period_ms: 12_000,
            centre_x: 0.0,
            inner_mm: 0.0,
            outer_mm: 100.0,
            floor: 0.4,
            dip_ms: 2000,
        };
        assert!(keyboard_effect(&tide, &room).is_none());
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

    /// Key rings (#490): the look as the scene effect, ring orange over night, 1.6 s, 60 mm.
    fn key_rings() -> Effect {
        Effect::KeyRings {
            colour: ORANGE,
            background: Rgb([6, 8, 14]),
            width_mm: 60.0,
            period_ms: 1600,
        }
    }

    /// What reaches the keyboard each frame, by kind.
    fn keyboard_kinds(outs: &[Out]) -> Vec<&'static str> {
        outs.iter()
            .filter_map(|o| match o {
                Out::Keyboard(_) => Some("frame"),
                Out::KeyboardEffect(_) => Some("effect"),
                Out::Case { .. } | Out::Lamps { .. } => None,
            })
            .collect()
    }

    /// #490, state A: a keyboard with KEY_RINGS gets the look once as SET_EFFECT kind 5, the
    /// bytes foxglove-link's encoder makes for it, and nothing per frame after.
    #[test]
    fn key_rings_go_to_a_keyboard_with_key_rings_once_as_kind_5() {
        use foxglove_link::protocol::{self as wire, EffectKind};
        let (room, cases) = desk();
        let mut o = Outputs::new(key_rings(), room, cases);
        o.keyboard_effects().store(true, Ordering::Relaxed);
        o.keyboard_rings().store(true, Ordering::Relaxed);
        let mut effects = Vec::new();
        let mut frames = 0;
        for ms in (0..2000).step_by(33) {
            for out in o.frame(SceneTime(ms)) {
                match out {
                    Out::KeyboardEffect(e) => effects.push(e),
                    Out::Keyboard(_) => frames += 1,
                    _ => {}
                }
            }
        }
        assert_eq!((effects.len(), frames), (1, 0), "sent once, no frames");
        let e = &effects[0];
        assert_eq!(
            e.effect,
            wire::Effect {
                kind: EffectKind::KeyRings,
                colours: vec![ORANGE.0, [6, 8, 14]],
                clock_ms: 0,
                period_ms: 1600,
                angle_deg: 0.0,
                width_mm: 60.0,
                origin_mm: [0, 0],
            }
        );
        // As the writer sends it at scene time 1700: the clock modulo the ring's time (100).
        let bytes = wire::effect(&e.at(SceneTime(1700))).unwrap();
        assert_eq!(
            bytes,
            [
                0xD7, 0x0F, 2, 5, 2, // EFFECT, kind 5 (key rings), 2 colours
                100, 0, 0, // clock (ms)
                0x40, 0x06, 0, // period: 1600 ms, a ring's time (speed 8)
                0, 0, // angle: unused
                0x58, 0x02, // width: 600 tenths of a mm (60 mm)
                0, 0, 0, 0, // origin: unused
                ORANGE.0[0], ORANGE.0[1], ORANGE.0[2], // the ring
                6, 8, 14, // the background
            ]
        );
        // A control change (another speed) goes again, once.
        o.set_effect(Effect::KeyRings {
            colour: ORANGE,
            background: Rgb([6, 8, 14]),
            width_mm: 60.0,
            period_ms: 1000,
        });
        assert_eq!(keyboard_kinds(&o.frame(SceneTime(2100))), ["effect"]);
        assert!(keyboard_kinds(&o.frame(SceneTime(2200))).is_empty());
    }

    /// #490: while the keyboard draws its own rings, a press's app-drawn ring (pick a) crosses
    /// the rest of the room by frames and the keyboard keeps its effect: no frames take over.
    #[test]
    fn with_key_rings_a_press_rings_the_room_and_the_keyboard_keeps_its_effect() {
        use foxglove_scene::burst::Shape;
        let (room, cases) = desk();
        let key = room
            .devices
            .iter()
            .find(|d| d.id == "keyboard:left")
            .unwrap()
            .rect
            .centre();
        // Two scenes alike, a press on one of them only.
        let mut pressed = Outputs::new(key_rings(), room.clone(), cases.clone());
        let mut quiet = Outputs::new(key_rings(), room, cases);
        for o in [&mut pressed, &mut quiet] {
            o.keyboard_effects().store(true, Ordering::Relaxed);
            o.keyboard_rings().store(true, Ordering::Relaxed);
            assert_eq!(keyboard_kinds(&o.frame(SceneTime(0))), ["effect"]);
        }
        pressed.burst(Burst {
            shape: Shape::Ring { origin: key },
            colour: ORANGE,
            start: SceneTime(200),
            duration_ms: 1600,
            width_mm: 60.0,
        });
        let case = |outs: &[Out]| -> Vec<Out> {
            outs.iter()
                .filter(|o| matches!(o, Out::Case { .. }))
                .cloned()
                .collect()
        };
        let mut differs = false;
        for ms in (200..1900).step_by(33) {
            let p = pressed.frame(SceneTime(ms));
            let q = quiet.frame(SceneTime(ms));
            assert!(keyboard_kinds(&p).is_empty(), "{ms}: {p:?}");
            differs |= case(&p) != case(&q);
        }
        assert!(differs, "the ring reaches the case");
    }

    /// #490, state B: without KEY_RINGS (every GA build, older firmware, another keyboard) the
    /// keyboard gets the still background as frames, and the ring over it as frames too.
    #[test]
    fn without_key_rings_the_keyboard_gets_frames_and_the_ring_over_them() {
        use foxglove_scene::burst::Shape;
        let (room, cases) = desk();
        let key = room
            .devices
            .iter()
            .find(|d| d.id == "keyboard:left")
            .unwrap()
            .rect
            .centre();
        for effects in [false, true] {
            let mut o = Outputs::new(key_rings(), room.clone(), cases.clone());
            o.keyboard_effects().store(effects, Ordering::Relaxed);
            o.keyboard_rings().store(false, Ordering::Relaxed);
            let first = o.frame(SceneTime(0));
            assert_eq!(keyboard_kinds(&first), ["frame"], "effects {effects}");
            let Some(Out::Keyboard(f)) = first.iter().find(|o| matches!(o, Out::Keyboard(_)))
            else {
                unreachable!()
            };
            assert!(
                f.lamps.iter().all(|c| *c == Rgb([6, 8, 14])),
                "the still background"
            );
            o.burst(Burst {
                shape: Shape::Ring { origin: key },
                colour: ORANGE,
                start: SceneTime(100),
                duration_ms: 1600,
                width_mm: 60.0,
            });
            let mut ringed = false;
            for ms in (100..1800).step_by(33) {
                for out in o.frame(SceneTime(ms)) {
                    match out {
                        Out::KeyboardEffect(_) => panic!("no kind 5 without KEY_RINGS"),
                        Out::Keyboard(f) => ringed |= f.lamps.iter().any(|c| *c != Rgb([6, 8, 14])),
                        _ => {}
                    }
                }
            }
            assert!(ringed, "the ring shows on the keyboard's frames");
        }
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
