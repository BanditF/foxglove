//! Effects (#484): each is a pure `f(position, time) -> colour` over the room's bounds, so a
//! light's colour depends on where it is and on the clock, and nothing else. Any host with the
//! room's bounds and the clock's epoch can draw the same frame: a wallpaper draws the monitor's
//! part of a scanline while Foxglove sends the keys theirs.
//!
//! Directions are angles: 0 runs left to right, 90 top to bottom (room space has y down).
//! Periods are milliseconds on the scene clock; sizes are millimetres.

use std::collections::BTreeMap;

use noise::{NoiseFn, Perlin};
use serde::{Deserialize, Serialize};

use crate::clock::SceneTime;
use crate::colour::{gradient, Rgb};
use crate::room::{LightRole, RoomMap};
use crate::space::{along, extent, Point, Rect};

/// One effect and its parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "kebab-case")]
pub enum Effect {
    /// A soft band of `colour` that crosses the room once per period over `background`.
    Scanline {
        colour: Rgb,
        background: Rgb,
        angle_deg: f32,
        width_mm: f32,
        period_ms: u32,
    },
    /// Colours that scroll across the room, wrapping, one full turn per period.
    Sweep {
        stops: Vec<Rgb>,
        angle_deg: f32,
        period_ms: u32,
    },
    /// A ring of `colour` that grows from `origin` to the room's far corner once per period.
    Ripple {
        origin: Point,
        colour: Rgb,
        background: Rgb,
        width_mm: f32,
        period_ms: u32,
    },
    /// The whole room eases from `from` to `to` and back once per period.
    Breathe { from: Rgb, to: Rgb, period_ms: u32 },
    /// A still gradient across the room.
    Gradient { stops: Vec<Rgb>, angle_deg: f32 },
    /// A drifting noise field (Perlin), its values mapped onto `stops`. `scale_mm` is the size
    /// of one feature; the field moves one feature's worth per period.
    Noise {
        stops: Vec<Rgb>,
        scale_mm: f32,
        period_ms: u32,
        seed: u32,
    },
    /// A palette's tones (#871, docs/palette.md "Lights"): each light shows the colour of its
    /// tone, `colours[0]` (the accent) by default. `tones` gives a light another one, keyed
    /// `device/light` ([`tone_key`]): 1 the second accent (the thumb keys), 2 the third (the
    /// homing keys). Once per period tone 0 and tone 1 breathe in opposite phase between
    /// `floor` and full, and tone 2 holds steady. When no light has tone 1 (a board whose LEDs
    /// each light keys of several roles), every light crossfades `colours[0]` and `colours[1]`
    /// instead, so the two tones take turns in time rather than in place. Period 0 stands
    /// still at full.
    Tones {
        colours: Vec<Rgb>,
        #[serde(default)]
        tones: BTreeMap<String, u8>,
        period_ms: u32,
        floor: f32,
    },
}

/// The key of a light in [`Effect::Tones`]' map.
pub fn tone_key(device: &str, light: &str) -> String {
    format!("{device}/{light}")
}

/// Whether a tones map splits the room (some light has tone 1); else the tones crossfade.
pub fn tones_split(tones: &BTreeMap<String, u8>) -> bool {
    tones.values().any(|&t| t == 1)
}

/// [`Effect::Tones`]' colour for one tone at `t`.
pub fn tone_colour(
    colours: &[Rgb],
    split: bool,
    tone: u8,
    t: SceneTime,
    period_ms: u32,
    floor: f32,
) -> Rgb {
    let pick = |i: usize| {
        colours
            .get(i)
            .or_else(|| colours.last())
            .copied()
            .unwrap_or(Rgb::BLACK)
    };
    // 0 at the start of the loop, 1 half way.
    let w = if period_ms == 0 {
        0.0
    } else {
        0.5 - 0.5 * (t.phase(period_ms) * std::f32::consts::TAU).cos()
    };
    if !split {
        return pick(0).mix(pick(1), w);
    }
    let floor = if floor.is_finite() {
        floor.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let level = |up: f32| {
        if period_ms == 0 {
            1.0
        } else {
            floor + (1.0 - floor) * up
        }
    };
    let scaled = |c: Rgb, k: f32| Rgb::from_linear(c.linear().map(|v| v * k));
    match tone {
        1 => scaled(pick(1), level(w)),
        2 => pick(2),
        _ => scaled(pick(0), level(1.0 - w)),
    }
}

/// One light's colour in a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameLight {
    pub device: String,
    pub light: String,
    pub colour: Rgb,
}

/// Every light's colour at one moment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub t: SceneTime,
    pub lights: Vec<FrameLight>,
}

/// A soft edge: 1 at the centre of a band, 0 from `half` away, smooth in between.
fn band(distance: f32, half: f32) -> f32 {
    if half.is_nan() || half <= 0.0 {
        return if distance == 0.0 { 1.0 } else { 0.0 };
    }
    let x = (distance.abs() / half).clamp(0.0, 1.0);
    1.0 - x * x * (3.0 - 2.0 * x)
}

/// Where a band that crosses `span` mm once per period has its centre, in mm from where it
/// starts: it enters fully outside (half a width before the start) and leaves fully outside.
/// The wallpaper side computes the same: `phase * (span + width) - width / 2`.
pub fn band_centre(t: SceneTime, period_ms: u32, span: f32, width_mm: f32) -> f32 {
    let width = width_mm.max(0.0);
    t.phase(period_ms) * (span + width) - width / 2.0
}

/// How much of a scanline's colour a point shows, 0..=1: where the point lies along the
/// direction, against the band's centre at that moment. A wallpaper computes the same from the
/// payload in [`crate::wallpaper`] (`shared/foxglove-sync.js` in bandit-wallpapers, checked
/// against `tests/wallpaper_vectors.json` on both sides).
pub fn scanline_level(
    at: Point,
    t: SceneTime,
    bounds: &Rect,
    angle_deg: f32,
    width_mm: f32,
    period_ms: u32,
) -> f32 {
    let span = extent(bounds, angle_deg);
    let here = along(at, bounds, angle_deg) * span;
    let centre = band_centre(t, period_ms, span, width_mm);
    band(here - centre, width_mm / 2.0)
}

impl Effect {
    /// The colour at a point of the room at a moment. `bounds` is the room's
    /// ([`RoomMap::bounds`]).
    pub fn colour(&self, at: Point, t: SceneTime, bounds: &Rect) -> Rgb {
        self.colour_with(at, t, bounds, self.perlin().as_ref())
    }

    fn perlin(&self) -> Option<Perlin> {
        match self {
            Effect::Noise { seed, .. } => Some(Perlin::new(*seed)),
            _ => None,
        }
    }

    fn colour_with(&self, at: Point, t: SceneTime, bounds: &Rect, perlin: Option<&Perlin>) -> Rgb {
        self.colour_of(at, None, t, bounds, perlin)
    }

    /// The colour at a point, for the light `key` ([`tone_key`]) when it is one.
    fn colour_of(
        &self,
        at: Point,
        key: Option<&str>,
        t: SceneTime,
        bounds: &Rect,
        perlin: Option<&Perlin>,
    ) -> Rgb {
        match self {
            Effect::Scanline {
                colour,
                background,
                angle_deg,
                width_mm,
                period_ms,
            } => background.mix(
                *colour,
                scanline_level(at, t, bounds, *angle_deg, *width_mm, *period_ms),
            ),
            Effect::Sweep {
                stops,
                angle_deg,
                period_ms,
            } => gradient(
                stops,
                along(at, bounds, *angle_deg) - t.phase(*period_ms),
                true,
            ),
            Effect::Ripple {
                origin,
                colour,
                background,
                width_mm,
                period_ms,
            } => {
                let reach = bounds
                    .corners()
                    .iter()
                    .map(|c| c.distance(*origin))
                    .fold(0.0, f32::max);
                let radius = band_centre(t, *period_ms, reach, *width_mm);
                background.mix(*colour, band(at.distance(*origin) - radius, width_mm / 2.0))
            }
            Effect::Breathe {
                from,
                to,
                period_ms,
            } => {
                let a = t.phase(*period_ms) * std::f32::consts::TAU;
                from.mix(*to, 0.5 - 0.5 * a.cos())
            }
            Effect::Gradient { stops, angle_deg } => {
                gradient(stops, along(at, bounds, *angle_deg), false)
            }
            Effect::Noise {
                stops,
                scale_mm,
                period_ms,
                seed,
            } => {
                let scale = if *scale_mm > 0.0 { *scale_mm } else { 100.0 };
                let z = if *period_ms == 0 {
                    0.0
                } else {
                    t.ms() as f64 / f64::from(*period_ms)
                };
                let p = [
                    f64::from((at.x - bounds.x) / scale),
                    f64::from((at.y - bounds.y) / scale),
                    z,
                ];
                let n = match perlin {
                    Some(f) => f.get(p),
                    None => Perlin::new(*seed).get(p),
                };
                gradient(stops, (n * 0.5 + 0.5) as f32, false)
            }
            Effect::Tones {
                colours,
                tones,
                period_ms,
                floor,
            } => {
                let tone = key.and_then(|k| tones.get(k)).copied().unwrap_or(0);
                tone_colour(colours, tones_split(tones), tone, t, *period_ms, *floor)
            }
        }
    }

    /// Every light in the room at `t`. Monitors have no lights, so they're not in it.
    ///
    /// A point light shows the effect where it is. An ambient light shows the peak of its
    /// device's point lights (or, on a device with none, the effect's average across the
    /// device); this is its target, which [`crate::Ambient`] eases it towards over time.
    pub fn render(&self, room: &RoomMap, t: SceneTime) -> Frame {
        let perlin = self.perlin();
        let lights = room
            .devices
            .iter()
            .flat_map(|d| {
                let perlin = perlin.as_ref();
                d.lights.iter().map(move |l| FrameLight {
                    device: d.id.clone(),
                    light: l.id.clone(),
                    colour: match l.role {
                        LightRole::Point => self.colour_of(
                            l.at,
                            Some(&tone_key(&d.id, &l.id)),
                            t,
                            &room.bounds,
                            perlin,
                        ),
                        LightRole::Ambient => self.area(&d.rect, t, &room.bounds, perlin),
                    },
                })
            })
            .collect();
        let mut frame = Frame { t, lights };
        crate::ambient::group(&mut frame, room);
        frame
    }

    /// The effect's average over an area: a 3 x 3 grid across it.
    fn area(&self, r: &Rect, t: SceneTime, bounds: &Rect, perlin: Option<&Perlin>) -> Rgb {
        const N: usize = 3;
        let at = |i: usize, j: usize| {
            let f = |k: usize| (k as f32 + 0.5) / N as f32;
            Point::new(r.x + r.w * f(i), r.y + r.h * f(j))
        };
        let samples = (0..N * N).map(|k| self.colour_with(at(k % N, k / N), t, bounds, perlin));
        Rgb::average(samples).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
