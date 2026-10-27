//! Effects (#484): each is a pure `f(position, time) -> colour` over the room's bounds, so a
//! light's colour depends on where it is and on the clock, and nothing else. Any host with the
//! room's bounds and the clock's epoch can draw the same frame: a wallpaper draws the monitor's
//! part of a scanline while Foxglove sends the keys theirs.
//!
//! Directions are angles: 0 runs left to right, 90 top to bottom (room space has y down).
//! Periods are milliseconds on the scene clock; sizes are millimetres.

use noise::{NoiseFn, Perlin};
use serde::{Deserialize, Serialize};

use crate::clock::SceneTime;
use crate::colour::{gradient, Rgb};
use crate::room::RoomMap;
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
        match self {
            Effect::Scanline {
                colour,
                background,
                angle_deg,
                width_mm,
                period_ms,
            } => {
                let span = extent(bounds, *angle_deg);
                let here = along(at, bounds, *angle_deg) * span;
                let centre = band_centre(t, *period_ms, span, *width_mm);
                background.mix(*colour, band(here - centre, width_mm / 2.0))
            }
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
        }
    }

    /// Every light in the room at `t`. Monitors have no lights, so they're not in it.
    pub fn render(&self, room: &RoomMap, t: SceneTime) -> Frame {
        let perlin = self.perlin();
        let lights = room
            .devices
            .iter()
            .flat_map(|d| {
                d.lights.iter().map(|l| FrameLight {
                    device: d.id.clone(),
                    light: l.id.clone(),
                    colour: self.colour_with(l.at, t, &room.bounds, perlin.as_ref()),
                })
            })
            .collect();
        Frame { t, lights }
    }
}

#[cfg(test)]
mod tests;
