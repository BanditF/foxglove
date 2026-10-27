//! Ambient lights: area lights, such as a case's interior light, that fill a space rather than
//! sit at a point. A scanline that pings each fan as it passes would ping an ambient light
//! too if it were sampled at its position, which reads wrong for a light that washes the whole
//! case. So an ambient light takes the average of its device's point lights
//! ([`group`], which [`crate::Effect::render`] applies) and eases towards it ([`Ambient`]):
//! it swells as the light crosses its neighbours and fades after them.
//!
//! The average is taken in linear light (light adds up); the easing is exponential in Oklab,
//! where equal steps look equal (in linear light the first steps up from a dark colour look
//! like a jump), with a quicker attack than decay, the way a room's glow follows its lights.

use std::collections::BTreeMap;

use crate::clock::SceneTime;
use crate::colour::Rgb;
use crate::effects::Frame;
use crate::room::{LightRole, RoomMap};

/// How quickly an ambient light brightens towards its target: the time constant (63 % of
/// the way) in milliseconds.
pub const ATTACK_MS: f32 = 600.0;
/// How quickly it dims towards its target, in milliseconds.
pub const DECAY_MS: f32 = 1500.0;
/// A gap between frames longer than this (a pause, the PC away, a seek) starts again at the
/// target instead of easing from a colour that is long gone.
const GAP_MS: i64 = 2_000;

/// Sets each ambient light in `frame` to the average of its device's point lights as they
/// are in it. A device with ambient lights only keeps the colours `frame` has for them.
/// Call it again after changing point lights in a frame (a burst over the scene).
pub fn group(frame: &mut Frame, room: &RoomMap) {
    for d in &room.devices {
        if !d.lights.iter().any(|l| l.role == LightRole::Ambient) {
            continue;
        }
        let is =
            |light: &str, role: LightRole| d.lights.iter().any(|l| l.id == light && l.role == role);
        let points = frame
            .lights
            .iter()
            .filter(|l| l.device == d.id && is(&l.light, LightRole::Point))
            .map(|l| l.colour);
        let Some(average) = Rgb::average(points) else {
            continue;
        };
        for l in frame.lights.iter_mut() {
            if l.device == d.id && is(&l.light, LightRole::Ambient) {
                l.colour = average;
            }
        }
    }
}

/// Eases ambient lights from frame to frame. Keep one per stream of frames (a running scene,
/// the Scenes page's preview) and pass each frame through [`Ambient::apply`] in time order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ambient {
    last: Option<SceneTime>,
    /// Each ambient light's colour now, in Oklab, by (device, light).
    now: BTreeMap<(String, String), [f32; 3]>,
}

impl Ambient {
    pub const fn new() -> Self {
        Ambient {
            last: None,
            now: BTreeMap::new(),
        }
    }

    /// Replaces each ambient light's target in `frame` with its eased colour. The first
    /// frame, one after a gap, and one earlier than the last start at the target.
    pub fn apply(&mut self, frame: &mut Frame, room: &RoomMap) {
        let dt = self
            .last
            .map(|l| frame.t.ms() - l.ms())
            .filter(|dt| (0..=GAP_MS).contains(dt));
        self.last = Some(frame.t);
        let mut kept = BTreeMap::new();
        for l in frame.lights.iter_mut() {
            let ambient = room.device(&l.device).is_some_and(|d| {
                d.lights
                    .iter()
                    .any(|x| x.id == l.light && x.role == LightRole::Ambient)
            });
            if !ambient {
                continue;
            }
            let key = (l.device.clone(), l.light.clone());
            let target = l.colour.lab();
            let now = match (dt, self.now.get(&key)) {
                (Some(dt), Some(&from)) => ease(from, target, dt as f32),
                _ => target,
            };
            l.colour = Rgb::from_lab(now);
            kept.insert(key, now);
        }
        // Lights that left the room (or stopped being ambient) are forgotten.
        self.now = kept;
    }

    /// Forgets every light, so the next frame starts at its targets.
    pub fn reset(&mut self) {
        *self = Ambient::new();
    }
}

/// `dt` ms of easing from `from` towards `to` (Oklab): the attack when it brightens, else the
/// decay.
fn ease(from: [f32; 3], to: [f32; 3], dt: f32) -> [f32; 3] {
    let tau = if to[0] > from[0] { ATTACK_MS } else { DECAY_MS };
    let k = 1.0 - (-dt / tau).exp();
    [0, 1, 2].map(|i| from[i] + (to[i] - from[i]) * k)
}

#[cfg(test)]
mod tests;
