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

use std::collections::BTreeMap;
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
        if self.is_neutral() {
            return c;
        }
        let out = self.apply([c.r, c.g, c.b].map(|v| f32::from(v) / 255.0));
        let byte = |v: f32| (clamp01(v) * 255.0).round() as u8;
        Rgb {
            r: byte(out[0]),
            g: byte(out[1]),
            b: byte(out[2]),
        }
    }

    /// Calibrates a hue (0-359), saturation and brightness (0-100), as a keyboard's underglow
    /// takes them. A colour that comes out grey keeps its hue, so the keyboard doesn't jump.
    pub fn apply_hsb(&self, hue: u16, sat: u8, brightness: u8) -> (u16, u8, u8) {
        let (hue, sat, brightness) = (hue.min(359), sat.min(100), brightness.min(100));
        if self.is_neutral() {
            return (hue, sat, brightness);
        }
        let rgb = hsv_to_rgb(
            f32::from(hue),
            f32::from(sat) / 100.0,
            f32::from(brightness) / 100.0,
        );
        let (h, s, v) = rgb_to_hsv(self.apply(rgb));
        let s = (s * 100.0).round() as u8;
        let h = if s == 0 {
            hue
        } else {
            (h.round() as u16) % 360
        };
        (h, s.min(100), ((v * 100.0).round() as u8).min(100))
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

/// One device's calibration: one for every zone, and optionally a zone's own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceCalibration {
    /// The device's name when it was calibrated, for people reading the settings.
    pub name: String,
    /// Every zone, unless the zone has its own in [`Self::zones`].
    pub device: Calibration,
    /// A zone's own calibration (by the zone's id), replacing [`Self::device`] for that zone.
    pub zones: BTreeMap<String, Calibration>,
}

impl DeviceCalibration {
    /// The calibration a zone uses: its own, else the device's.
    pub fn zone(&self, zone: Option<&str>) -> &Calibration {
        zone.and_then(|z| self.zones.get(z)).unwrap_or(&self.device)
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

    /// Changes nothing on any zone.
    pub fn is_neutral(&self) -> bool {
        self.device.is_neutral() && self.zones.values().all(Calibration::is_neutral)
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

    /// What a zone of a device uses: the zone's own, else the device's, else the model's
    /// default (from the catalog), else neutral.
    pub fn lookup(
        &self,
        key: &str,
        zone: Option<&str>,
        default: Option<&Calibration>,
    ) -> Calibration {
        match self.devices.get(key) {
            Some(d) => d.zone(zone).clone(),
            None => default
                .cloned()
                .map(Calibration::sanitized)
                .unwrap_or_default(),
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

/// The calibration boundary for drivers that take hue, saturation and brightness (a keyboard's
/// underglow): [`HsbBoundary::send`] calibrates the colour Foxglove asks for and remembers what
/// it sent, and [`HsbBoundary::asked`] turns what the device reports back into the colour that
/// was asked for, so the app never calibrates the same colour twice.
#[derive(Debug, Default)]
pub struct HsbBoundary {
    last: Mutex<Option<(Hsb, Hsb)>>,
}

impl HsbBoundary {
    /// Calibrates `asked` for the device `key` and writes it with `write`.
    pub fn send(
        &self,
        key: &str,
        asked: Hsb,
        write: impl FnOnce(Hsb) -> crate::Result<()>,
    ) -> crate::Result<()> {
        let asked = (asked.0.min(359), asked.1.min(100), asked.2.min(100));
        let sent = global().hsb(key, None, None, asked);
        write(sent)?;
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some((asked, sent));
        Ok(())
    }

    /// The colour a device reports, in Foxglove's palette: the asked hue and saturation while
    /// it still shows what was sent for them, and the asked brightness while that is
    /// unchanged too (a half dimmed on battery, or brightness changed on the keyboard, reports
    /// its own).
    pub fn asked(&self, reported: Hsb) -> Hsb {
        let Some((asked, sent)) = *self.last.lock().unwrap_or_else(|e| e.into_inner()) else {
            return reported;
        };
        let hue = reported.0.abs_diff(sent.0);
        let same_hue = hue.min(360u16.saturating_sub(hue)) <= 2 || sent.1 == 0;
        if !same_hue || reported.1.abs_diff(sent.1) > 1 {
            return reported;
        }
        let brightness = if reported.2.abs_diff(sent.2) <= 1 {
            asked.2
        } else {
            reported.2
        };
        (asked.0, asked.1, brightness)
    }
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
}

/// A device's calibration as TOML, to save or share.
pub fn export(key: &str, cal: &DeviceCalibration) -> Result<String, String> {
    let file = Export {
        version: VERSION,
        key: key.to_string(),
        name: cal.name.clone(),
        device: cal.device.clone(),
        zones: cal.zones.clone(),
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
    pub fn get(&self, key: &str, zone: Option<&str>, default: Option<&Calibration>) -> Calibration {
        self.read().lookup(key, zone, default)
    }

    /// The driver boundary for RGB drivers: the colour this zone of this device is sent.
    pub fn rgb(&self, key: &str, zone: Option<&str>, default: Option<&Calibration>, c: Rgb) -> Rgb {
        self.get(key, zone, default).apply_rgb(c)
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
            all.lookup("103c:84fd", Some("5"), Some(&model)),
            model,
            "model default"
        );
        let wild = Calibration {
            gain: [9.0, 1.0, 1.0],
            ..Default::default()
        };
        assert_eq!(
            all.lookup("x", None, Some(&wild)).gain[0],
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
            },
        );
        assert_eq!(all.lookup("103c:84fd", Some("5"), Some(&model)), fan);
        assert_eq!(all.lookup("103c:84fd", Some("1"), Some(&model)), device);
        assert_eq!(all.lookup("103c:84fd", None, Some(&model)), device);
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
        };
        let text = export("103c:84fd", &cal).unwrap();
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
        global().set_device(
            key,
            Some(DeviceCalibration {
                device: Calibration {
                    gain: [1.0, 0.7, 0.5],
                    brightness: 0.8,
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        let b = HsbBoundary::default();
        assert_eq!(b.asked((10, 20, 30)), (10, 20, 30), "nothing sent yet");
        let mut sent = None;
        b.send(key, (200, 40, 90), |hsb| {
            sent = Some(hsb);
            Ok(())
        })
        .unwrap();
        let sent = sent.unwrap();
        assert_ne!(sent, (200, 40, 90), "calibrated on the way out");
        assert_eq!(b.asked(sent), (200, 40, 90));
        assert_eq!(
            b.asked((sent.0, sent.1, 20)),
            (200, 40, 20),
            "dimmed on its own"
        );
        assert_eq!(
            b.asked((5, 100, sent.2)),
            (5, 100, sent.2),
            "a colour set elsewhere"
        );
        // A failed write remembers nothing new.
        let failed = b.send(key, (0, 100, 100), |_| Err(crate::DeviceError::Timeout));
        assert!(failed.is_err());
        assert_eq!(b.asked(sent), (200, 40, 90));
        global().set_device(key, None);
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
            },
        );
        let text = toml::to_string(&all).unwrap();
        let back: Calibrations = toml::from_str(&text).unwrap();
        assert_eq!(back, all);
        let empty: Calibrations = toml::from_str("").unwrap();
        assert_eq!(empty.version, VERSION);
    }
}
