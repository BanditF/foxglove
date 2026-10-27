//! Colour calibration: one palette, every device showing it alike.
//!
//! Foxglove picks colours in one device-independent space: sRGB values, processed in linear
//! light. Devices that claim the same colour still render it differently (a warm white LED, a
//! weak blue channel, a brighter fan), so every colour sent to a lighting device passes through
//! that device's [`Calibration`] at the driver boundary: [`Registry::rgb`] for drivers that take
//! RGB, [`Registry::hsb`] for those that take hue, saturation and brightness. Both go through
//! [`Calibration::apply`].
//!
//! Calibration belongs to the device (and, where a zone renders differently, the zone), never to
//! a colour or a profile: changing the palette keeps each device's calibration.
//!
//! The pipeline, per colour: sRGB → linear → 3×3 matrix (optional) → clamp → per-channel gamma
//! → white balance gain × relative brightness → clamp → sRGB → 1D curve (optional). Gamma
//! comes before the gains, so it bends the mid-tones without moving white: white balance and
//! brightness stay put when gamma changes. The neutral calibration changes nothing.
//!
//! **Matches.** On top of those values, a device keeps the matches its user confirmed: a test
//! colour, the value its light was sent and the value the screen drew, which the user said look
//! alike ([`Match`]). Nothing here decides what a colour "really" is: the user's match is taken as
//! it is. Each match has two sides, kept together and applied at the same time: the **light**
//! (applied to what goes to the device) and the **screen preview** (applied to every swatch,
//! preview and picker colour Foxglove draws for the device), so blue down 30% on the light and
//! up 5% on the screen are both kept. [`Fit`] turns the matches into a transform per side: a
//! matched colour maps exactly to its match, and colours between matches are interpolated.
//! Calibrations from before matches load unchanged, as light-only values.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::Rgb;

/// The calibration data's version, in the settings and in exported files.
pub const VERSION: u32 = 1;

/// The 3×3 identity: each output channel takes only its own input.
pub const IDENTITY_MATRIX: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// sRGB-encoded (0-1) to linear light.
pub fn srgb_to_linear(v: f32) -> f32 {
    let v = clamp01(v);
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light (0-1) to sRGB-encoded.
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = clamp01(v);
    if v <= 0.003_130_8 {
        v * 12.92
    } else if v >= 1.0 {
        1.0
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn clamp01(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

/// How one device (or one zone of it) is corrected. [`Default`] is neutral: no change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Calibration {
    /// White balance: each channel's gain in linear light (1 = unchanged).
    #[serde(serialize_with = "tidy::three")]
    pub gain: [f32; 3],
    /// Each channel's gamma, applied to linear light before the gains (1 = unchanged; above 1
    /// darkens the mid-tones, never white).
    #[serde(serialize_with = "tidy::three")]
    pub gamma: [f32; 3],
    /// Relative brightness in linear light, to match devices to each other (1 = unchanged).
    #[serde(serialize_with = "tidy::one")]
    pub brightness: f32,
    /// A 3×3 colour matrix in linear light, rows = output channel (red, green, blue), columns
    /// = input channel. `None` is the identity.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "tidy::matrix"
    )]
    pub matrix: Option<[[f32; 3]; 3]>,
    /// A small 1D curve per channel, last in the pipeline: outputs (0-1) at evenly spaced
    /// sRGB inputs from 0 to 1, joined by straight lines. `None` is a straight line.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "tidy::curve"
    )]
    pub curve: Option<[Vec<f32>; 3]>,
}

/// Settings and exports write values short (0.85, not 0.8500000238418579): four decimals is
/// finer than any device shows.
mod tidy {
    use serde::Serializer;

    fn short(v: f32) -> f64 {
        (f64::from(v) * 1e4).round() / 1e4
    }

    pub fn one<S: Serializer>(v: &f32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(short(*v))
    }

    pub fn three<S: Serializer>(v: &[f32; 3], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(v.map(short))
    }

    pub fn matrix<S: Serializer>(v: &Option<[[f32; 3]; 3]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(m) => s.collect_seq(m.map(|row| row.map(short))),
            None => s.serialize_none(),
        }
    }

    pub fn curve<S: Serializer>(v: &Option<[Vec<f32>; 3]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(c) => s.collect_seq(
                c.iter()
                    .map(|points| points.iter().map(|p| short(*p)).collect::<Vec<_>>()),
            ),
            None => s.serialize_none(),
        }
    }
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            gain: [1.0; 3],
            gamma: [1.0; 3],
            brightness: 1.0,
            matrix: None,
            curve: None,
        }
    }
}

/// The most points a curve keeps.
pub const MAX_CURVE_POINTS: usize = 64;

impl Calibration {
    /// Changes nothing.
    pub fn is_neutral(&self) -> bool {
        self.gain.iter().all(|g| near(*g, 1.0))
            && self.gamma.iter().all(|g| near(*g, 1.0))
            && near(self.brightness, 1.0)
            && self.matrix.is_none_or(|m| {
                m.iter()
                    .flatten()
                    .zip(IDENTITY_MATRIX.iter().flatten())
                    .all(|(a, b)| near(*a, *b))
            })
            && self.curve.as_ref().is_none_or(|c| {
                c.iter().all(|points| {
                    let last = points.len().saturating_sub(1).max(1) as f32;
                    points
                        .iter()
                        .enumerate()
                        .all(|(i, p)| near(*p, i as f32 / last))
                })
            })
    }

    /// Within safe ranges, with no NaN: imported files and old settings go through this.
    pub fn sanitized(mut self) -> Self {
        let fix = |v: f32, lo: f32, hi: f32, neutral: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                neutral
            }
        };
        self.gain = self.gain.map(|g| fix(g, 0.0, 2.0, 1.0));
        self.gamma = self.gamma.map(|g| fix(g, 0.2, 5.0, 1.0));
        self.brightness = fix(self.brightness, 0.0, 2.0, 1.0);
        if let Some(m) = &mut self.matrix {
            for (row, id) in m.iter_mut().zip(IDENTITY_MATRIX) {
                for (v, n) in row.iter_mut().zip(id) {
                    *v = fix(*v, -2.0, 2.0, n);
                }
            }
        }
        if let Some(c) = &mut self.curve {
            for points in c.iter_mut() {
                // Too short to be a curve: that channel stays a straight line.
                if points.len() < 2 {
                    *points = vec![0.0, 1.0];
                }
                points.truncate(MAX_CURVE_POINTS);
                for p in points.iter_mut() {
                    *p = fix(*p, 0.0, 1.0, 0.0);
                }
            }
        }
        self
    }

    /// The matrix, the identity when there is none.
    pub fn matrix(&self) -> [[f32; 3]; 3] {
        self.matrix.unwrap_or(IDENTITY_MATRIX)
    }

    /// Calibrates one sRGB-encoded colour (each channel 0-1).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        if self.is_neutral() {
            return c.map(clamp01);
        }
        let lin = c.map(srgb_to_linear);
        let lin = match self.matrix {
            Some(m) => {
                [0, 1, 2].map(|row| m[row][0] * lin[0] + m[row][1] * lin[1] + m[row][2] * lin[2])
            }
            None => lin,
        };
        let mut out = [0.0; 3];
        for i in 0..3 {
            let v = clamp01(lin[i]).powf(self.gamma[i].max(0.01));
            let v = clamp01(v * self.gain[i] * self.brightness);
            let e = linear_to_srgb(v);
            out[i] = match &self.curve {
                Some(curve) => interpolate(&curve[i], e),
                None => e,
            };
        }
        out
    }

    /// Calibrates an RGB colour, as a driver sends it.
    pub fn apply_rgb(&self, c: Rgb) -> Rgb {
        apply_rgb(self, c)
    }

    /// Calibrates a hue (0-359), saturation and brightness (0-100), as a keyboard's underglow
    /// takes them. A colour that comes out grey keeps its hue, so the keyboard doesn't jump.
    pub fn apply_hsb(&self, hue: u16, sat: u8, brightness: u8) -> (u16, u8, u8) {
        apply_hsb(self, (hue, sat, brightness))
    }

    /// The colour that calibrates to `c`, near enough (sRGB-encoded, each channel 0-1): what
    /// the palette asked for, from what a device shows. Colours the calibration can't reach
    /// (brighter than its white) come back as the nearest it can.
    pub fn invert(&self, c: [f32; 3]) -> [f32; 3] {
        invert(self, c)
    }

    /// [`Self::invert`] for a hue, saturation and brightness; a grey keeps its hue.
    pub fn invert_hsb(&self, hsb: Hsb) -> Hsb {
        invert_hsb(self, hsb)
    }
}

/// A colour transform: [`Calibration`] (a device's values) or [`Light`] (its matches too).
trait Transform {
    fn neutral(&self) -> bool;
    /// One sRGB-encoded colour (each channel 0-1).
    fn map(&self, c: [f32; 3]) -> [f32; 3];
}

impl Transform for Calibration {
    fn neutral(&self) -> bool {
        self.is_neutral()
    }
    fn map(&self, c: [f32; 3]) -> [f32; 3] {
        self.apply(c)
    }
}

fn apply_rgb(t: &impl Transform, c: Rgb) -> Rgb {
    if t.neutral() {
        return c;
    }
    to_rgb(t.map(from_rgb(c)))
}

fn apply_hsb(t: &impl Transform, (hue, sat, brightness): Hsb) -> Hsb {
    let (hue, sat, brightness) = (hue.min(359), sat.min(100), brightness.min(100));
    if t.neutral() {
        return (hue, sat, brightness);
    }
    let rgb = hsv_to_rgb(
        f32::from(hue),
        f32::from(sat) / 100.0,
        f32::from(brightness) / 100.0,
    );
    let (h, s, v) = rgb_to_hsv(t.map(rgb));
    let s = (s * 100.0).round() as u8;
    let h = if s == 0 {
        hue
    } else {
        (h.round() as u16) % 360
    };
    (h, s.min(100), ((v * 100.0).round() as u8).min(100))
}

fn invert(t: &impl Transform, c: [f32; 3]) -> [f32; 3] {
    let target = c.map(clamp01);
    if t.neutral() {
        return target;
    }
    // Damped fixed-point steps: the transform rises in each channel, so this settles.
    let mut x = target;
    for _ in 0..80 {
        let y = t.map(x);
        for i in 0..3 {
            x[i] = clamp01(x[i] + 0.8 * (target[i] - y[i]));
        }
    }
    x
}

fn invert_hsb(t: &impl Transform, (hue, sat, brightness): Hsb) -> Hsb {
    if t.neutral() {
        return (hue, sat, brightness);
    }
    let rgb = hsv_to_rgb(
        f32::from(hue.min(359)),
        f32::from(sat.min(100)) / 100.0,
        f32::from(brightness.min(100)) / 100.0,
    );
    let (h, s, v) = rgb_to_hsv(invert(t, rgb));
    let s = ((s * 100.0).round() as u8).min(100);
    let h = if s == 0 {
        hue
    } else {
        (h.round() as u16) % 360
    };
    (h, s, ((v * 100.0).round() as u8).min(100))
}

/// What a zone of a device is sent through: the light side of its matches, then its values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Light {
    pub fit: Fit,
    pub values: Calibration,
}

impl Transform for Light {
    fn neutral(&self) -> bool {
        self.is_neutral()
    }
    fn map(&self, c: [f32; 3]) -> [f32; 3] {
        self.apply(c)
    }
}

impl Light {
    pub fn is_neutral(&self) -> bool {
        self.fit.is_neutral() && self.values.is_neutral()
    }

    /// One sRGB-encoded colour (each channel 0-1).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        self.values.apply(self.fit.apply(c))
    }

    /// See [`Calibration::apply_rgb`].
    pub fn apply_rgb(&self, c: Rgb) -> Rgb {
        apply_rgb(self, c)
    }

    /// See [`Calibration::apply_hsb`].
    pub fn apply_hsb(&self, hue: u16, sat: u8, brightness: u8) -> Hsb {
        apply_hsb(self, (hue, sat, brightness))
    }

    /// See [`Calibration::invert_hsb`].
    pub fn invert_hsb(&self, hsb: Hsb) -> Hsb {
        invert_hsb(self, hsb)
    }
}

fn from_rgb(c: Rgb) -> [f32; 3] {
    [c.r, c.g, c.b].map(|v| f32::from(v) / 255.0)
}

fn to_rgb(c: [f32; 3]) -> Rgb {
    let byte = |v: f32| (clamp01(v) * 255.0).round() as u8;
    Rgb {
        r: byte(c[0]),
        g: byte(c[1]),
        b: byte(c[2]),
    }
}

/// One side of a match: the light (what the device is sent) or the screen preview (what
/// Foxglove draws for the device).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Light,
    Screen,
}

/// A match the user confirmed: with the device sent `light` and the screen drawing `screen`
/// for the test colour `color`, the two looked alike to them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    /// The test colour (sRGB), as the palette asks for it.
    pub color: [u8; 3],
    /// What the device was sent for it (before the device's values, if it has any).
    pub light: [u8; 3],
    /// What the screen drew for it.
    pub screen: [u8; 3],
    /// The zone it was made on (by id); `None` is every zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

impl Match {
    /// A match that changes neither side: the starting point for a test colour.
    pub fn plain(color: [u8; 3]) -> Self {
        Self {
            color,
            light: color,
            screen: color,
            zone: None,
        }
    }

    pub fn side(&self, side: Side) -> [u8; 3] {
        match side {
            Side::Light => self.light,
            Side::Screen => self.screen,
        }
    }

    pub fn side_mut(&mut self, side: Side) -> &mut [u8; 3] {
        match side {
            Side::Light => &mut self.light,
            Side::Screen => &mut self.screen,
        }
    }
}

/// How far a picked colour can be from every match (in hue and saturation, see [`Fit`]) and
/// still count as matched.
const COVERED: f32 = 0.15;

/// One side's transform, fitted to the matches: each output channel is the colour's own times
/// a gain, and the gain is interpolated between the matches' gains by inverse squared distance
/// (Shepard). So a matched colour maps exactly to its match, black stays black, and a colour
/// between matches takes gains between theirs. A channel a match doesn't light (0 in its test
/// colour) says nothing about that channel's gain.
///
/// Distance is mostly hue and saturation (each colour scaled to its brightest channel), plus
/// half the brightness difference: a brighter red follows the red match, and grey and white,
/// which share a hue, still tell apart by brightness.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fit {
    /// Each match's test colour (0-1) and gains per channel (`None`: that channel is dark).
    points: Vec<([f32; 3], [Option<f32>; 3])>,
}

fn chroma(c: [f32; 3]) -> ([f32; 3], f32) {
    let max = c[0].max(c[1]).max(c[2]);
    if max <= f32::EPSILON {
        ([0.0; 3], 0.0)
    } else {
        (c.map(|v| v / max), max)
    }
}

fn hue_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let ((na, _), (nb, _)) = (chroma(a), chroma(b));
    (0..3).map(|i| (na[i] - nb[i]).powi(2)).sum::<f32>().sqrt()
}

impl Fit {
    /// One side of these matches.
    pub fn new<'a>(matches: impl IntoIterator<Item = &'a Match>, side: Side) -> Self {
        let points = matches
            .into_iter()
            .map(|m| {
                let color = m.color.map(|v| f32::from(v) / 255.0);
                let out = m.side(side);
                let gains = [0, 1, 2]
                    .map(|i| (m.color[i] > 0).then(|| f32::from(out[i]) / f32::from(m.color[i])));
                (color, gains)
            })
            .collect();
        Self { points }
    }

    /// No match changes anything on this side.
    pub fn is_neutral(&self) -> bool {
        self.points
            .iter()
            .all(|(_, g)| g.iter().flatten().all(|g| near(*g, 1.0)))
    }

    /// Each channel's gain for a colour (sRGB, 0-1).
    pub fn gains(&self, c: [f32; 3]) -> [f32; 3] {
        let (_, bright) = chroma(c);
        [0, 1, 2].map(|i| {
            let (mut sum, mut weights) = (0.0f32, 0.0f32);
            for (m, gains) in &self.points {
                let Some(g) = gains[i] else { continue };
                let d = hue_distance(c, *m) + 0.5 * (bright - chroma(*m).1).abs();
                if d < 1e-4 {
                    return g;
                }
                let w = 1.0 / (d * d);
                sum += w * g;
                weights += w;
            }
            if weights > 0.0 {
                sum / weights
            } else {
                1.0
            }
        })
    }

    /// A colour (sRGB, 0-1) through this side.
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let c = c.map(clamp01);
        if self.points.is_empty() {
            return c;
        }
        let g = self.gains(c);
        [0, 1, 2].map(|i| clamp01(c[i] * g[i]))
    }

    /// A colour through this side, as bytes.
    pub fn apply_rgb(&self, c: Rgb) -> Rgb {
        to_rgb(self.apply(from_rgb(c)))
    }

    /// The colour this side maps to `out`, near enough (a picker drawn through the screen side
    /// stands for this colour).
    pub fn invert_rgb(&self, out: Rgb) -> Rgb {
        let target = from_rgb(out);
        let mut x = target;
        for _ in 0..40 {
            let g = self.gains(x);
            x = [0, 1, 2].map(|i| clamp01(target[i] / g[i].max(0.01)));
        }
        to_rgb(x)
    }

    /// Whether a colour lies within what the matches cover (near one of them in hue and
    /// saturation); black is always covered. No matches cover nothing.
    pub fn covers(&self, c: [f32; 3]) -> bool {
        chroma(c).1 <= f32::EPSILON
            || self
                .points
                .iter()
                .any(|(m, _)| hue_distance(c, *m) <= COVERED)
    }
}

/// A piecewise-linear curve through evenly spaced points.
fn interpolate(points: &[f32], x: f32) -> f32 {
    if points.len() < 2 {
        return x;
    }
    let pos = clamp01(x) * (points.len() - 1) as f32;
    let i = (pos.floor() as usize).min(points.len() - 2);
    let t = pos - i as f32;
    clamp01(points[i] + (points[i + 1] - points[i]) * t)
}

/// Hue in degrees, saturation and value 0-1, to sRGB-encoded 0-1.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.rem_euclid(360.0)) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// sRGB-encoded 0-1 to hue in degrees, saturation and value 0-1.
pub fn rgb_to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let [r, g, b] = c.map(clamp01);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

/// One device's calibration: its values (one for every zone, and optionally a zone's own) and
/// the matches its user confirmed. Settings from before matches read as values only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceCalibration {
    /// The device's name when it was calibrated, for people reading the settings.
    pub name: String,
    /// Every zone, unless the zone has its own in [`Self::zones`].
    pub device: Calibration,
    /// A zone's own calibration (by the zone's id), replacing [`Self::device`] for that zone.
    pub zones: BTreeMap<String, Calibration>,
    /// The matches, oldest first. Not written while empty, so a calibration without them
    /// saves as before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<Match>,
}

impl DeviceCalibration {
    /// The values a zone uses: its own, else the device's.
    pub fn zone(&self, zone: Option<&str>) -> &Calibration {
        zone.and_then(|z| self.zones.get(z)).unwrap_or(&self.device)
    }

    /// The matches a zone uses: its own when it has any, else the ones for every zone.
    pub fn zone_matches<'a>(&'a self, zone: Option<&'a str>) -> impl Iterator<Item = &'a Match> {
        let own = zone.is_some() && self.matches.iter().any(|m| m.zone.as_deref() == zone);
        let pick = if own { zone } else { None };
        self.matches
            .iter()
            .filter(move |m| m.zone.as_deref() == pick)
    }

    /// One side of a zone's matches, fitted.
    pub fn fit(&self, zone: Option<&str>, side: Side) -> Fit {
        Fit::new(self.zone_matches(zone), side)
    }

    /// What a zone of this device is sent through: the light side of its matches, then its
    /// values.
    pub fn light(&self, zone: Option<&str>) -> Light {
        Light {
            fit: self.fit(zone, Side::Light),
            values: self.zone(zone).clone(),
        }
    }

    pub fn sanitized(self) -> Self {
        Self {
            device: self.device.sanitized(),
            zones: self
                .zones
                .into_iter()
                .map(|(z, c)| (z, c.sanitized()))
                .collect(),
            ..self
        }
    }

    /// Changes nothing on any zone, on either side.
    pub fn is_neutral(&self) -> bool {
        self.device.is_neutral()
            && self.zones.values().all(Calibration::is_neutral)
            && Fit::new(&self.matches, Side::Light).is_neutral()
            && Fit::new(&self.matches, Side::Screen).is_neutral()
    }
}

/// Every device's calibration, by device key ([`device_key`]): the `[calibration]` settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calibrations {
    pub version: u32,
    pub devices: BTreeMap<String, DeviceCalibration>,
}

impl Default for Calibrations {
    fn default() -> Self {
        Self {
            version: VERSION,
            devices: BTreeMap::new(),
        }
    }
}

impl Calibrations {
    /// Settings as read: every value in range, and the current version (version 1 is the
    /// first; a later version's unknown fields would have failed to parse already).
    pub fn sanitized(self) -> Self {
        Self {
            version: VERSION,
            devices: self
                .devices
                .into_iter()
                .map(|(k, d)| (k, d.sanitized()))
                .collect(),
        }
    }

    /// What a zone of a device is sent through: its matches' light side and its values (the
    /// zone's own, else the device's), else the model's default (from the catalog), else
    /// neutral.
    pub fn lookup(&self, key: &str, zone: Option<&str>, default: Option<&Calibration>) -> Light {
        match self.devices.get(key) {
            Some(d) => d.light(zone),
            None => Light {
                values: default
                    .cloned()
                    .map(Calibration::sanitized)
                    .unwrap_or_default(),
                ..Default::default()
            },
        }
    }
}

/// A device's stable key: `vvvv:pppp`, plus its serial when the serial can tell boards apart.
pub fn device_key(vid: u16, pid: u16, serial: Option<&str>) -> String {
    let base = format!("{vid:04x}:{pid:04x}");
    match serial.filter(|s| crate::trusted_serial(s)) {
        Some(serial) => format!("{base}:{}", serial.trim()),
        None => base,
    }
}

/// A device's key from its device-layer id: its trusted serial when it has one.
pub fn key_for(id: &crate::DeviceId) -> String {
    let serial = match &id.identity {
        crate::DeviceIdentity::Serial(s) => Some(s.as_str()),
        crate::DeviceIdentity::Path(_) => None,
    };
    device_key(id.vid, id.pid, serial)
}

/// Hue (0-359), saturation and brightness (0-100).
pub type Hsb = (u16, u8, u8);

/// What a hue/saturation/brightness device was last asked for and sent, by device key. Kept for
/// the whole process, not in the device object: that is rebuilt on every reconnect, while the
/// keyboard keeps showing its colour.
#[derive(Clone, Copy, Debug)]
struct LastHsb {
    seq: u64,
    asked: Hsb,
    sent: Hsb,
}

static LAST_HSB: Mutex<BTreeMap<String, LastHsb>> = Mutex::new(BTreeMap::new());
static HSB_SEQ: AtomicU64 = AtomicU64::new(0);

/// The calibration boundary for drivers that take hue, saturation and brightness (a keyboard's
/// underglow): calibrates the colour Foxglove asks for, writes it with `write`, and remembers
/// both, so [`asked_hsb`] can turn what the device reports back into the asked colour. No lock
/// is held while `write` runs; of two writes at once, the one that started last is remembered.
pub fn send_hsb(
    key: &str,
    asked: Hsb,
    write: impl FnOnce(Hsb) -> crate::Result<()>,
) -> crate::Result<()> {
    let asked = (asked.0.min(359), asked.1.min(100), asked.2.min(100));
    let sent = global().hsb(key, None, None, asked);
    let seq = HSB_SEQ.fetch_add(1, Ordering::Relaxed);
    write(sent)?;
    let mut last = LAST_HSB.lock().unwrap_or_else(|e| e.into_inner());
    if last.get(key).is_none_or(|l| l.seq < seq) {
        last.insert(key.to_string(), LastHsb { seq, asked, sent });
    }
    Ok(())
}

/// The colour a hue/saturation/brightness device reports, in Foxglove's palette: what was asked
/// for while it still shows what was sent for it; otherwise (a colour from before this start,
/// brightness changed on the keyboard, a half dimmed on battery) the calibration's inverse.
pub fn asked_hsb(key: &str, reported: Hsb) -> Hsb {
    let last = LAST_HSB
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .copied();
    if let Some(l) = last {
        let hue = reported.0.abs_diff(l.sent.0);
        let same_hue = hue.min(360u16.saturating_sub(hue)) <= 2 || l.sent.1 == 0;
        if same_hue && reported.1.abs_diff(l.sent.1) <= 1 && reported.2.abs_diff(l.sent.2) <= 1 {
            return l.asked;
        }
    }
    global().get(key, None, None).invert_hsb(reported)
}

/// One device's calibration as a file, to export and import.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    version: u32,
    /// The device it was made on.
    key: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    device: Calibration,
    #[serde(default)]
    zones: BTreeMap<String, Calibration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    matches: Vec<Match>,
}

/// A device's calibration as TOML, to save or share.
pub fn export(key: &str, cal: &DeviceCalibration) -> Result<String, String> {
    let file = Export {
        version: VERSION,
        key: key.to_string(),
        name: cal.name.clone(),
        device: cal.device.clone(),
        zones: cal.zones.clone(),
        matches: cal.matches.clone(),
    };
    let body = toml::to_string(&file).map_err(|e| e.to_string())?;
    Ok(format!("# Foxglove colour calibration\n{body}"))
}

/// Reads an exported calibration: the key it was made on, and the calibration. A file from a
/// newer Foxglove is refused rather than half read.
pub fn import(text: &str) -> Result<(String, DeviceCalibration), String> {
    let file: Export = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if file.version > VERSION || file.version == 0 {
        return Err(format!(
            "calibration version {} isn't one this Foxglove reads (it reads {VERSION})",
            file.version
        ));
    }
    Ok((
        file.key,
        DeviceCalibration {
            name: file.name,
            device: file.device,
            zones: file.zones,
            matches: file.matches,
        }
        .sanitized(),
    ))
}

/// The calibrations drivers apply, shared by the whole process: the app loads the saved ones
/// at startup and changes them from the calibrator; drivers read them on every write.
#[derive(Debug, Default)]
pub struct Registry {
    all: RwLock<Calibrations>,
}

/// The process's registry.
pub fn global() -> &'static Registry {
    static GLOBAL: OnceLock<Registry> = OnceLock::new();
    GLOBAL.get_or_init(Registry::default)
}

impl Registry {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Calibrations> {
        self.all.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Replaces every calibration (the saved settings, at startup).
    pub fn replace(&self, all: Calibrations) {
        *self.all.write().unwrap_or_else(|e| e.into_inner()) = all.sanitized();
    }

    /// Every calibration, to save.
    pub fn all(&self) -> Calibrations {
        self.read().clone()
    }

    /// One device's calibration, when it has one.
    pub fn device(&self, key: &str) -> Option<DeviceCalibration> {
        self.read().devices.get(key).cloned()
    }

    /// Sets (or with `None`, forgets) one device's calibration. Drivers use it from their next
    /// write: the calibrator previews this way.
    pub fn set_device(&self, key: &str, cal: Option<DeviceCalibration>) {
        let mut all = self.all.write().unwrap_or_else(|e| e.into_inner());
        match cal {
            Some(c) => {
                all.devices.insert(key.to_string(), c.sanitized());
            }
            None => {
                all.devices.remove(key);
            }
        }
    }

    /// What a zone of a device uses (see [`Calibrations::lookup`]).
    pub fn get(&self, key: &str, zone: Option<&str>, default: Option<&Calibration>) -> Light {
        self.read().lookup(key, zone, default)
    }

    /// The driver boundary for RGB drivers: the colour this zone of this device is sent.
    pub fn rgb(&self, key: &str, zone: Option<&str>, default: Option<&Calibration>, c: Rgb) -> Rgb {
        self.get(key, zone, default).apply_rgb(c)
    }

    /// The screen preview side of a zone of a device (no matches: neutral).
    pub fn screen(&self, key: &str, zone: Option<&str>) -> Fit {
        self.read()
            .devices
            .get(key)
            .map(|d| d.fit(zone, Side::Screen))
            .unwrap_or_default()
    }

    /// A colour as Foxglove draws it for a device: through its screen preview side.
    pub fn on_screen(&self, key: &str, zone: Option<&str>, c: Rgb) -> Rgb {
        self.screen(key, zone).apply_rgb(c)
    }

    /// The colour a picker drawn through [`Self::on_screen`] stands for: what to send.
    pub fn from_screen(&self, key: &str, zone: Option<&str>, c: Rgb) -> Rgb {
        self.screen(key, zone).invert_rgb(c)
    }

    /// Whether the device's matches cover a colour: `None` when it has no matches.
    pub fn covers(&self, key: &str, zone: Option<&str>, c: Rgb) -> Option<bool> {
        let all = self.read();
        let d = all.devices.get(key)?;
        let fit = d.fit(zone, Side::Light);
        (!fit.points.is_empty()).then(|| fit.covers(from_rgb(c)))
    }

    /// The driver boundary for hue/saturation/brightness drivers (a keyboard's underglow).
    pub fn hsb(
        &self,
        key: &str,
        zone: Option<&str>,
        default: Option<&Calibration>,
        (hue, sat, brightness): (u16, u8, u8),
    ) -> (u16, u8, u8) {
        self.get(key, zone, default).apply_hsb(hue, sat, brightness)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORANGE: Rgb = Rgb {
        r: 0xf0,
        g: 0x8a,
        b: 0x3c,
    };

    fn every_byte() -> impl Iterator<Item = Rgb> {
        (0..=255u8).step_by(5).flat_map(|r| {
            (0..=255u8)
                .step_by(15)
                .flat_map(move |g| (0..=255u8).step_by(51).map(move |b| Rgb { r, g, b }))
        })
    }

    #[test]
    fn neutral_changes_nothing() {
        let n = Calibration::default();
        assert!(n.is_neutral());
        for c in every_byte() {
            assert_eq!(n.apply_rgb(c), c);
        }
        for h in (0..360).step_by(7) {
            for s in [0, 37, 100] {
                assert_eq!(n.apply_hsb(h, s, 64), (h, s, 64));
            }
        }
        // An explicit identity matrix and straight curve are neutral too.
        let explicit = Calibration {
            matrix: Some(IDENTITY_MATRIX),
            curve: Some([
                vec![0.0, 0.5, 1.0],
                vec![0.0, 1.0],
                vec![0.0, 0.25, 0.5, 0.75, 1.0],
            ]),
            ..Default::default()
        };
        assert!(explicit.is_neutral());
        assert_eq!(explicit.apply_rgb(ORANGE), ORANGE);
    }

    #[test]
    fn srgb_round_trips_every_byte() {
        for v in 0..=255u8 {
            let e = f32::from(v) / 255.0;
            let back = linear_to_srgb(srgb_to_linear(e));
            assert_eq!((back * 255.0).round() as u8, v);
        }
        assert!(
            (srgb_to_linear(0.5) - 0.214).abs() < 0.001,
            "mid-grey in linear light"
        );
    }

    #[test]
    fn white_balance_scales_each_channel_in_linear_light() {
        let warm = Calibration {
            gain: [1.0, 0.9, 0.6],
            ..Default::default()
        };
        let white = warm.apply_rgb(Rgb {
            r: 255,
            g: 255,
            b: 255,
        });
        assert_eq!(white.r, 255);
        let want = |gain: f32| (linear_to_srgb(gain) * 255.0).round() as u8;
        assert_eq!(white.g, want(0.9));
        assert_eq!(white.b, want(0.6));
        // Black stays black, and a pure red only loses nothing (its gain is 1).
        assert_eq!(
            warm.apply_rgb(Rgb { r: 0, g: 0, b: 0 }),
            Rgb { r: 0, g: 0, b: 0 }
        );
        assert_eq!(warm.apply_rgb(Rgb { r: 200, g: 0, b: 0 }).r, 200);
    }

    #[test]
    fn gamma_round_trips() {
        let up = Calibration {
            gamma: [2.2, 1.6, 0.7],
            ..Default::default()
        };
        let down = Calibration {
            gamma: up.gamma.map(|g| 1.0 / g),
            ..Default::default()
        };
        for c in every_byte() {
            let back = down.apply_rgb(up.apply_rgb(c));
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                // Bytes in the middle crush the darkest steps (gamma 2.2 sends 5 to 0); above
                // them it costs at most a few steps.
                assert!(a < 40 || a.abs_diff(b) <= 4, "{c:?} -> {back:?}");
            }
            let f = [c.r, c.g, c.b].map(|v| f32::from(v) / 255.0);
            let back = down.apply(up.apply(f));
            for (a, b) in f.iter().zip(back) {
                assert!((a - b).abs() < 1e-3, "{f:?} -> {back:?}");
            }
        }
        // Gamma above 1 darkens mid-grey, and leaves black and white alone.
        let grey = up.apply_rgb(Rgb {
            r: 128,
            g: 128,
            b: 128,
        });
        // Gamma doesn't move white, so white balance and brightness stay where they were set.
        let balanced = Calibration {
            gain: [1.0, 0.8, 0.6],
            brightness: 0.9,
            ..Default::default()
        };
        let curved = Calibration {
            gamma: [1.8, 0.6, 1.3],
            ..balanced.clone()
        };
        assert_eq!(curved.apply([1.0; 3]), balanced.apply([1.0; 3]));
        assert_ne!(curved.apply([0.5; 3]), balanced.apply([0.5; 3]));
        assert!(grey.r < 128 && grey.g < 128 && grey.b > 128);
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        assert_eq!(up.apply_rgb(white), white);
    }

    #[test]
    fn everything_clamps() {
        let hot = Calibration {
            gain: [2.0, 2.0, 2.0],
            brightness: 2.0,
            matrix: Some([[1.0, 1.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
            ..Default::default()
        };
        let out = hot.apply([1.0, 1.0, 1.0]);
        assert_eq!(out, [1.0, 0.0, 1.0], "over 1 clips to 1, under 0 to 0");
        assert_eq!(hot.apply([f32::NAN, -3.0, 9.0])[2], 1.0);
        let wild = Calibration {
            gain: [f32::NAN, 99.0, -1.0],
            gamma: [0.0, f32::INFINITY, 1.0],
            brightness: -5.0,
            matrix: Some([[f32::NAN, 0.0, 0.0], [0.0, 9.0, 0.0], [0.0, 0.0, 1.0]]),
            curve: Some([vec![0.0, 2.0], vec![0.0], vec![0.0, 1.0]]),
        }
        .sanitized();
        assert_eq!(wild.gain, [1.0, 2.0, 0.0]);
        assert_eq!(wild.gamma, [0.2, 1.0, 1.0]);
        assert_eq!(wild.brightness, 0.0);
        assert_eq!(wild.matrix.unwrap()[0][0], 1.0);
        assert_eq!(wild.matrix.unwrap()[1][1], 2.0);
        let curve = wild.curve.clone().unwrap();
        assert_eq!(curve[0], vec![0.0, 1.0], "clamped");
        assert_eq!(
            curve[1],
            vec![0.0, 1.0],
            "a one-point curve is a straight line"
        );
        assert_eq!(
            wild.apply_rgb(ORANGE),
            Rgb { r: 0, g: 0, b: 0 },
            "brightness 0"
        );
    }

    #[test]
    fn brightness_and_matrix_and_curve() {
        let dim = Calibration {
            brightness: 0.5,
            ..Default::default()
        };
        let white = dim.apply([1.0; 3]);
        assert!(
            (white[0] - linear_to_srgb(0.5)).abs() < 1e-5,
            "half the light"
        );
        // A red channel that leaks into green: the matrix takes it back out.
        let m = Calibration {
            matrix: Some([[1.0, 0.0, 0.0], [-0.2, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            ..Default::default()
        };
        let yellow = m.apply_rgb(Rgb {
            r: 255,
            g: 255,
            b: 0,
        });
        assert_eq!(yellow.r, 255);
        assert_eq!(yellow.g, (linear_to_srgb(0.8) * 255.0).round() as u8);
        let curve = Calibration {
            curve: Some([vec![0.0, 1.0], vec![0.0, 0.5], vec![1.0, 1.0]]),
            ..Default::default()
        };
        assert_eq!(
            curve.apply_rgb(Rgb {
                r: 100,
                g: 200,
                b: 0
            }),
            Rgb {
                r: 100,
                g: 100,
                b: 255
            }
        );
    }

    #[test]
    fn hsb_goes_through_the_same_transform() {
        let warm = Calibration {
            gain: [1.0, 1.0, 0.5],
            ..Default::default()
        };
        // White comes out yellowish and keeps full brightness.
        let (h, s, b) = warm.apply_hsb(200, 0, 80);
        assert!((50..=70).contains(&h) && s > 0 && b == 80, "{h} {s} {b}");
        // The same colour by RGB lands on the same hue.
        let rgb = warm.apply_rgb(Rgb {
            r: 204,
            g: 204,
            b: 204,
        });
        let (rh, rs, _) = rgb_to_hsv([rgb.r, rgb.g, rgb.b].map(|v| f32::from(v) / 255.0));
        assert!((rh - f32::from(h)).abs() <= 2.0 && ((rs * 100.0).round() as u8).abs_diff(s) <= 1);
        // A grey result keeps the hue it was given.
        let dim = Calibration {
            brightness: 0.25,
            ..Default::default()
        };
        assert_eq!(dim.apply_hsb(123, 0, 100).0, 123);
        assert_eq!(dim.apply_hsb(0, 0, 0), (0, 0, 0));
    }

    #[test]
    fn a_zone_override_wins_over_the_device_and_the_device_over_the_model() {
        let model = Calibration {
            brightness: 0.9,
            ..Default::default()
        };
        let mut all = Calibrations::default();
        assert_eq!(
            all.lookup("103c:84fd", Some("5"), Some(&model)).values,
            model,
            "model default"
        );
        let wild = Calibration {
            gain: [9.0, 1.0, 1.0],
            ..Default::default()
        };
        assert_eq!(
            all.lookup("x", None, Some(&wild)).values.gain[0],
            2.0,
            "a catalog default is kept in range too"
        );
        assert!(
            all.lookup("103c:84fd", None, None).is_neutral(),
            "else neutral"
        );
        let device = Calibration {
            gain: [1.0, 0.8, 0.7],
            ..Default::default()
        };
        let fan = Calibration {
            gain: [0.6, 1.0, 1.0],
            ..Default::default()
        };
        all.devices.insert(
            "103c:84fd".into(),
            DeviceCalibration {
                name: "case".into(),
                device: device.clone(),
                zones: [("5".to_string(), fan.clone())].into(),
                ..Default::default()
            },
        );
        assert_eq!(all.lookup("103c:84fd", Some("5"), Some(&model)).values, fan);
        assert_eq!(
            all.lookup("103c:84fd", Some("1"), Some(&model)).values,
            device
        );
        assert_eq!(all.lookup("103c:84fd", None, Some(&model)).values, device);
    }

    #[test]
    fn export_and_import_round_trip() {
        let cal = DeviceCalibration {
            name: "HP OMEN 45L case lights".into(),
            device: Calibration {
                gain: [1.0, 0.85, 0.7],
                gamma: [1.1, 1.0, 0.9],
                brightness: 0.8,
                matrix: Some([[0.9, 0.0, 0.0], [0.1, 1.0, 0.0], [0.0, 0.0, 1.0]]),
                curve: None,
            },
            zones: [(
                "5".to_string(),
                Calibration {
                    brightness: 0.7,
                    ..Default::default()
                },
            )]
            .into(),
            matches: vec![Match {
                color: [0, 0, 153],
                light: [0, 0, 107],
                screen: [0, 0, 161],
                zone: None,
            }],
        };
        let text = export("103c:84fd", &cal).unwrap();
        assert!(text.contains("[[matches]]"), "{text}");
        assert!(
            text.starts_with("# Foxglove colour calibration\nversion = 1\n"),
            "{text}"
        );
        assert!(
            text.contains("gain = [1.0, 0.85, 0.7]"),
            "written short: {text}"
        );
        let (key, back) = import(&text).unwrap();
        assert_eq!(key, "103c:84fd");
        assert_eq!(back, cal);
        assert!(import(&text.replace("version = 1", "version = 9"))
            .unwrap_err()
            .contains("version 9"));
        assert!(import("version = 1\nkey = \"x\"\ncolour = 3\n").is_err());
        // Values out of range come in clamped.
        let (_, wild) =
            import("version = 1\nkey = \"x\"\n[device]\ngain = [5.0, 1.0, 1.0]\n").unwrap();
        assert_eq!(wild.device.gain[0], 2.0);
    }

    #[test]
    fn the_registry_applies_what_was_set_and_forgets() {
        let r = Registry::default();
        assert_eq!(r.rgb("k", None, None, ORANGE), ORANGE);
        let dim = DeviceCalibration {
            device: Calibration {
                brightness: 0.5,
                ..Default::default()
            },
            ..Default::default()
        };
        r.set_device("k", Some(dim.clone()));
        assert_ne!(r.rgb("k", Some("1"), None, ORANGE), ORANGE);
        assert_eq!(
            r.rgb("other", None, None, ORANGE),
            ORANGE,
            "only that device"
        );
        assert_ne!(r.hsb("k", None, None, (30, 80, 100)), (30, 80, 100));
        assert_eq!(r.device("k"), Some(dim));
        r.set_device("k", None);
        assert_eq!(r.rgb("k", None, None, ORANGE), ORANGE);
    }

    #[test]
    fn an_hsb_driver_reports_the_colour_it_was_asked_for() {
        let key = "hsb-boundary-test";
        let cal = Calibration {
            gain: [1.0, 0.7, 0.5],
            brightness: 0.8,
            ..Default::default()
        };
        global().set_device(
            key,
            Some(DeviceCalibration {
                device: cal.clone(),
                ..Default::default()
            }),
        );
        let mut sent = None;
        send_hsb(key, (200, 40, 90), |hsb| {
            sent = Some(hsb);
            Ok(())
        })
        .unwrap();
        let sent = sent.unwrap();
        assert_ne!(sent, (200, 40, 90), "calibrated on the way out");
        // Any device object for this key (a reconnect makes a new one) reads it back.
        assert_eq!(asked_hsb(key, sent), (200, 40, 90));
        // Something else: the inverse, never the calibrated colour taken as the palette's.
        let dimmed = asked_hsb(key, (sent.0, sent.1, 20));
        assert!(dimmed.2 > 20, "{dimmed:?}");
        // A failed write remembers nothing new.
        let failed = send_hsb(key, (0, 100, 100), |_| Err(crate::DeviceError::Timeout));
        assert!(failed.is_err());
        assert_eq!(asked_hsb(key, sent), (200, 40, 90));
        // Nothing sent this run (the colour is from before a restart): the inverse too.
        let other = "hsb-boundary-test-2";
        global().set_device(
            other,
            Some(DeviceCalibration {
                device: cal.clone(),
                ..Default::default()
            }),
        );
        let back = asked_hsb(other, cal.apply_hsb(30, 80, 70));
        assert!(
            back.0.abs_diff(30) <= 2 && back.1.abs_diff(80) <= 2 && back.2.abs_diff(70) <= 2,
            "{back:?}"
        );
        assert_eq!(asked_hsb("never-calibrated", (5, 6, 7)), (5, 6, 7));
        global().set_device(key, None);
        global().set_device(other, None);
    }

    #[test]
    fn the_inverse_undoes_the_calibration() {
        let cals = [
            Calibration {
                gain: [1.0, 0.8, 0.6],
                brightness: 0.9,
                gamma: [1.2, 1.0, 0.8],
                ..Default::default()
            },
            Calibration {
                matrix: Some([[0.9, 0.0, 0.0], [0.08, 0.85, 0.0], [0.0, 0.1, 1.0]]),
                ..Default::default()
            },
        ];
        for cal in &cals {
            for c in every_byte().step_by(7) {
                let f = [c.r, c.g, c.b].map(|v| f32::from(v) / 255.0);
                let back = cal.invert(cal.apply(f));
                let again = cal.apply(back);
                for (a, b) in cal.apply(f).iter().zip(again) {
                    assert!((a - b).abs() < 2e-3, "{cal:?} {f:?} -> {back:?}");
                }
            }
        }
        assert_eq!(
            Calibration::default().invert([0.2, 0.4, 0.6]),
            [0.2, 0.4, 0.6]
        );
    }

    #[test]
    fn device_ids_give_the_same_keys() {
        use crate::{BackendId, DeviceId};
        let zmk = BackendId("zmk".into());
        let path = DeviceId::new(zmk.clone(), 0x1d50, 0x615e, None, "1d50:615e");
        assert_eq!(key_for(&path), "1d50:615e");
        let serial = DeviceId::new(zmk, 0x1d50, 0x615e, Some("E1A5C0DE"), "x");
        assert_eq!(key_for(&serial), "1d50:615e:E1A5C0DE");
    }

    #[test]
    fn keys_use_trusted_serials_only() {
        assert_eq!(device_key(0x103c, 0x84fd, None), "103c:84fd");
        assert_eq!(
            device_key(0x103c, 0x84fd, Some("A1B2C3D4")),
            "103c:84fd:A1B2C3D4"
        );
        assert_eq!(device_key(0x103c, 0x84fd, Some("0000")), "103c:84fd");
    }

    #[test]
    fn settings_toml_round_trips() {
        let mut all = Calibrations::default();
        all.devices.insert(
            "1d50:615e".into(),
            DeviceCalibration {
                name: "Eyelash Corne".into(),
                device: Calibration {
                    gain: [1.0, 0.9, 0.8],
                    ..Default::default()
                },
                zones: BTreeMap::new(),
                matches: Vec::new(),
            },
        );
        let text = toml::to_string(&all).unwrap();
        assert!(
            !text.contains("matches"),
            "no matches aren't written: {text}"
        );
        let back: Calibrations = toml::from_str(&text).unwrap();
        assert_eq!(back, all);
        let empty: Calibrations = toml::from_str("").unwrap();
        assert_eq!(empty.version, VERSION);
    }

    /// Blue down 30% on the light, up 5% on the screen: both kept, side by side.
    fn blue_match() -> Match {
        Match {
            color: [0, 0, 153],
            light: [0, 0, 107],
            screen: [0, 0, 161],
            zone: None,
        }
    }

    #[test]
    fn matches_persist_with_both_sides() {
        let mut all = Calibrations::default();
        let white = Match {
            color: [153; 3],
            light: [153, 147, 120],
            screen: [153; 3],
            zone: None,
        };
        let fan = Match {
            zone: Some("5".into()),
            ..white.clone()
        };
        all.devices.insert(
            "103c:84fd".into(),
            DeviceCalibration {
                name: "HP OMEN 45L case lights".into(),
                matches: vec![blue_match(), white, fan],
                ..Default::default()
            },
        );
        let text = toml::to_string(&all).unwrap();
        let back: Calibrations = toml::from_str(&text).unwrap();
        assert_eq!(back, all);
        let m = &back.devices["103c:84fd"].matches[0];
        assert_eq!((m.light, m.screen), ([0, 0, 107], [0, 0, 161]));
        assert_eq!(
            back.devices["103c:84fd"].matches[2].zone.as_deref(),
            Some("5")
        );
        // A registry loaded from them sends and draws both sides.
        let r = Registry::default();
        r.replace(back);
        let blue = Rgb { r: 0, g: 0, b: 153 };
        assert_eq!(
            r.rgb("103c:84fd", None, None, blue),
            Rgb { r: 0, g: 0, b: 107 }
        );
        assert_eq!(
            r.on_screen("103c:84fd", None, blue),
            Rgb { r: 0, g: 0, b: 161 }
        );
    }

    #[test]
    fn settings_from_before_matches_load_as_light_only_values() {
        let old = "version = 1\n\
                   [devices.\"103c:84fd\"]\n\
                   name = \"HP OMEN 45L case lights\"\n\
                   [devices.\"103c:84fd\".device]\n\
                   gain = [1.0, 0.9, 0.7]\n\
                   brightness = 0.8\n\
                   [devices.\"103c:84fd\".zones.\"5\"]\n\
                   brightness = 0.6\n";
        let all: Calibrations = toml::from_str(old).unwrap();
        let d = &all.devices["103c:84fd"];
        assert_eq!(d.device.gain, [1.0, 0.9, 0.7]);
        assert_eq!(d.zones["5"].brightness, 0.6);
        assert!(d.matches.is_empty());
        assert!(d.fit(None, Side::Screen).is_neutral(), "light only");
        // Sent as before, drawn as is.
        let r = Registry::default();
        r.replace(all.clone());
        assert_eq!(
            r.rgb("103c:84fd", None, None, ORANGE),
            d.device.apply_rgb(ORANGE)
        );
        assert_eq!(r.on_screen("103c:84fd", None, ORANGE), ORANGE);
        assert_eq!(r.covers("103c:84fd", None, ORANGE), None);
        // Written back, it's the same settings: nothing new appears.
        let text = toml::to_string(&all).unwrap();
        assert!(!text.contains("matches"), "{text}");
        assert_eq!(toml::from_str::<Calibrations>(&text).unwrap(), all);
        let (_, cal) =
            import("version = 1\nkey = \"x\"\n[device]\ngain = [1.0, 1.0, 0.7]\n").unwrap();
        assert_eq!(cal.device.gain[2], 0.7);
        assert!(cal.matches.is_empty());
    }

    fn grey(v: u8) -> Rgb {
        Rgb { r: v, g: v, b: v }
    }

    #[test]
    fn the_fit_is_exact_at_every_match() {
        let matches = vec![
            Match {
                color: [153; 3],
                light: [150, 140, 110],
                screen: [153, 153, 158],
                zone: None,
            },
            Match {
                color: [128; 3],
                light: [126, 120, 96],
                screen: [128; 3],
                zone: None,
            },
            Match {
                color: [153, 0, 0],
                light: [120, 0, 0],
                screen: [160, 0, 0],
                zone: None,
            },
            blue_match(),
            Match {
                color: [153, 112, 92],
                light: [140, 100, 70],
                screen: [153, 115, 92],
                zone: None,
            },
        ];
        for side in [Side::Light, Side::Screen] {
            let fit = Fit::new(&matches, side);
            for m in &matches {
                let c = Rgb {
                    r: m.color[0],
                    g: m.color[1],
                    b: m.color[2],
                };
                let out = fit.apply_rgb(c);
                assert_eq!([out.r, out.g, out.b], m.side(side), "{side:?} {m:?}");
            }
            assert_eq!(fit.apply_rgb(grey(0)), grey(0), "black stays black");
        }
        // Through the registry too: what the driver sends for a matched colour is the match.
        let r = Registry::default();
        r.set_device(
            "k",
            Some(DeviceCalibration {
                matches: matches.clone(),
                ..Default::default()
            }),
        );
        let red = r.rgb("k", Some("1"), None, Rgb { r: 153, g: 0, b: 0 });
        assert_eq!(red, Rgb { r: 120, g: 0, b: 0 });
        let (h, s, b) = r.hsb("k", None, None, (0, 100, 60));
        assert_eq!((h, s), (0, 100));
        assert_eq!(b, (120.0f32 / 255.0 * 100.0).round() as u8);
    }

    #[test]
    fn the_fit_is_monotonic_between_matches() {
        let matches = [
            Match {
                color: [128; 3],
                light: [124, 118, 100],
                screen: [128, 128, 131],
                zone: None,
            },
            Match {
                color: [153; 3],
                light: [150, 140, 115],
                screen: [153, 153, 160],
                zone: None,
            },
        ];
        for side in [Side::Light, Side::Screen] {
            let fit = Fit::new(&matches, side);
            let mut last = grey(0);
            for v in 0..=255u8 {
                let out = fit.apply_rgb(grey(v));
                assert!(
                    out.r >= last.r && out.g >= last.g && out.b >= last.b,
                    "{side:?} at {v}: {last:?} then {out:?}"
                );
                last = out;
            }
            // Between the two greys, each channel lies between the two matches'.
            let mid = fit.apply_rgb(grey(140));
            for i in 0..3 {
                let v = [mid.r, mid.g, mid.b][i];
                let (a, b) = (matches[0].side(side)[i], matches[1].side(side)[i]);
                assert!((a.min(b)..=a.max(b)).contains(&v), "{side:?} {i}: {v}");
            }
        }
        // A brighter red than the one matched follows the red match's gains.
        let red = [Match {
            color: [153, 0, 0],
            light: [107, 0, 0],
            screen: [153, 0, 0],
            zone: None,
        }];
        let fit = Fit::new(&red, Side::Light);
        assert_eq!(fit.apply_rgb(Rgb { r: 250, g: 0, b: 0 }).r, 175);
    }

    #[test]
    fn the_light_and_the_screen_sides_apply_together() {
        let r = Registry::default();
        let values = Calibration {
            brightness: 0.9,
            ..Default::default()
        };
        r.set_device(
            "case",
            Some(DeviceCalibration {
                device: values.clone(),
                matches: vec![blue_match()],
                ..Default::default()
            }),
        );
        let blue = Rgb { r: 0, g: 0, b: 153 };
        // The device gets the light side, then its values; the screen draws the screen side.
        let sent = r.rgb("case", Some("1"), None, blue);
        assert_eq!(sent, values.apply_rgb(Rgb { r: 0, g: 0, b: 107 }));
        assert_eq!(r.on_screen("case", None, blue), Rgb { r: 0, g: 0, b: 161 });
        // A picker drawn on screen stands for the colour it was drawn from.
        let back = r.from_screen("case", None, Rgb { r: 0, g: 0, b: 161 });
        assert!(back.b.abs_diff(153) <= 1 && back.r == 0, "{back:?}");
        // Another device: drawn as is.
        assert_eq!(r.on_screen("other", None, blue), blue);
        assert_eq!(r.from_screen("other", None, blue), blue);
        // A zone with matches of its own uses those; the others use the device's.
        let mut d = r.device("case").unwrap();
        d.matches.push(Match {
            color: [0, 0, 153],
            light: [0, 0, 153],
            screen: [0, 0, 140],
            zone: Some("5".into()),
        });
        r.set_device("case", Some(d));
        assert_eq!(r.on_screen("case", Some("5"), blue).b, 140);
        assert_eq!(r.on_screen("case", Some("1"), blue).b, 161);
    }

    #[test]
    fn colours_far_from_every_match_are_not_matched_yet() {
        let r = Registry::default();
        assert_eq!(r.covers("k", None, ORANGE), None, "no matches: no hint");
        r.set_device(
            "k",
            Some(DeviceCalibration {
                matches: vec![blue_match()],
                ..Default::default()
            }),
        );
        assert_eq!(r.covers("k", None, Rgb { r: 0, g: 0, b: 255 }), Some(true));
        assert_eq!(
            r.covers(
                "k",
                None,
                Rgb {
                    r: 10,
                    g: 10,
                    b: 200
                }
            ),
            Some(true)
        );
        assert_eq!(r.covers("k", None, ORANGE), Some(false));
        assert_eq!(r.covers("k", None, grey(0)), Some(true));
    }
}
