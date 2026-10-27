//! One-shot effects drawn over a running scene (#489): a ring from a pressed key, a sweep on a
//! layer change. Each starts at a scene moment, crosses the room once and is gone; nothing
//! here keeps time of its own, the scene's frames draw them.

use crate::clock::SceneTime;
use crate::colour::Rgb;
use crate::space::{along, extent, Point, Rect};

/// At most this many at once; a new one drops the oldest.
pub const MAX_BURSTS: usize = 4;

/// What a burst draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A ring growing from `origin`.
    Ring { origin: Point },
    /// A band crossing the room in this direction (0 left to right, 90 top to bottom).
    Sweep { angle_deg: f32 },
}

/// One ring or sweep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Burst {
    pub shape: Shape,
    pub colour: Rgb,
    /// When it starts, on the scene's clock.
    pub start: SceneTime,
    /// How long it takes to cross (ms).
    pub duration_ms: u32,
    /// The ring's or band's width (mm).
    pub width_mm: f32,
}

impl Burst {
    /// Whether it has crossed the room by `t`.
    pub fn done(&self, t: SceneTime) -> bool {
        t.ms() - self.start.ms() > i64::from(self.duration_ms)
    }

    /// How much of its colour a point shows at `t`, 0..=1.
    pub fn level(&self, at: Point, t: SceneTime, bounds: &Rect) -> f32 {
        let elapsed = t.ms() - self.start.ms();
        if elapsed < 0 || self.duration_ms == 0 || elapsed > i64::from(self.duration_ms) {
            return 0.0;
        }
        let p = elapsed as f32 / self.duration_ms as f32;
        let half = (self.width_mm / 2.0).max(1.0);
        // Fades as it goes, so its end is soft.
        let fade = 1.0 - p * p;
        let distance = match self.shape {
            Shape::Ring { origin } => {
                // Far enough to leave the room from wherever it starts.
                let reach = bounds
                    .corners()
                    .iter()
                    .map(|c| c.distance(origin))
                    .fold(0.0, f32::max)
                    + half;
                at.distance(origin) - p * reach
            }
            Shape::Sweep { angle_deg } => {
                let span = extent(bounds, angle_deg);
                let here = along(at, bounds, angle_deg) * span;
                here - (p * (span + 2.0 * half) - half)
            }
        };
        let x = (distance.abs() / half).clamp(0.0, 1.0);
        (1.0 - x * x * (3.0 - 2.0 * x)) * fade
    }

    /// `base` with this burst over it.
    pub fn over(&self, base: Rgb, at: Point, t: SceneTime, bounds: &Rect) -> Rgb {
        let l = self.level(at, t, bounds);
        if l <= 0.0 {
            base
        } else {
            base.mix(self.colour, l)
        }
    }
}

/// The bursts in flight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bursts {
    live: Vec<Burst>,
}

impl Bursts {
    /// Adds one; the oldest goes when there are already [`MAX_BURSTS`].
    pub fn add(&mut self, b: Burst) {
        if self.live.len() >= MAX_BURSTS {
            self.live.remove(0);
        }
        self.live.push(b);
    }

    /// Drops those done by `t`; true while any is left.
    pub fn prune(&mut self, t: SceneTime) -> bool {
        self.live.retain(|b| !b.done(t));
        !self.live.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// `base` at `at` with every burst over it, oldest first.
    pub fn over(&self, base: Rgb, at: Point, t: SceneTime, bounds: &Rect) -> Rgb {
        self.live.iter().fold(base, |c, b| b.over(c, at, t, bounds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgb = Rgb([255, 0, 0]);

    fn room() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 500.0,
        }
    }

    fn ring(start: i64) -> Burst {
        Burst {
            shape: Shape::Ring {
                origin: Point::new(500.0, 400.0),
            },
            colour: RED,
            start: SceneTime(start),
            duration_ms: 1000,
            width_mm: 60.0,
        }
    }

    #[test]
    fn a_ring_starts_at_its_key_and_grows_out() {
        let r = ring(0);
        let b = room();
        let key = Point::new(500.0, 400.0);
        assert!(r.level(key, SceneTime(0), &b) > 0.99, "at the key first");
        assert_eq!(r.level(key, SceneTime(500), &b), 0.0, "then gone from it");
        let far = Point::new(500.0 - 330.0, 400.0);
        assert!(r.level(far, SceneTime(500), &b) > 0.3, "halfway out");
        assert_eq!(r.level(key, SceneTime(-1), &b), 0.0, "not before it starts");
        assert!(r.done(SceneTime(1001)) && !r.done(SceneTime(1000)));
        assert_eq!(r.over(Rgb::BLACK, key, SceneTime(1001), &b), Rgb::BLACK);
    }

    #[test]
    fn a_sweep_crosses_the_room_once() {
        let s = Burst {
            shape: Shape::Sweep { angle_deg: 0.0 },
            colour: RED,
            start: SceneTime(1000),
            duration_ms: 1000,
            width_mm: 100.0,
        };
        let b = room();
        let left = Point::new(10.0, 250.0);
        let right = Point::new(990.0, 250.0);
        assert!(s.level(left, SceneTime(1050), &b) > 0.5);
        assert_eq!(s.level(right, SceneTime(1050), &b), 0.0);
        assert!(s.level(right, SceneTime(1950), &b) > 0.0);
        assert_eq!(s.level(left, SceneTime(1950), &b), 0.0);
    }

    #[test]
    fn at_most_four_and_done_ones_go() {
        let mut all = Bursts::default();
        for k in 0..6 {
            all.add(ring(k * 100));
        }
        assert_eq!(all.live.len(), MAX_BURSTS);
        assert_eq!(all.live[0].start, SceneTime(200), "the oldest went");
        assert!(all.prune(SceneTime(1400)));
        assert_eq!(all.live.len(), 2);
        assert!(!all.prune(SceneTime(5000)));
        assert!(all.is_empty());
    }
}
