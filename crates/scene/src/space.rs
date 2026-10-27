//! Room space: millimetres, seen from the user's chair. x runs to the right, y down, and the
//! origin is the primary monitor's top-left corner. The desk is folded down in front of the
//! screens, so a scanline that leaves the bottom of the monitor reaches the keyboard next.

use serde::{Deserialize, Serialize};

/// A point (or an offset, or a size) in millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Point) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

/// A rectangle in millimetres: its top-left corner and its size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn centre(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn moved(&self, by: Point) -> Rect {
        Rect::new(self.x + by.x, self.y + by.y, self.w, self.h)
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, o: &Rect) -> Rect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }

    /// Whether the two share any area (touching edges don't count).
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    pub fn corners(&self) -> [Point; 4] {
        [
            Point::new(self.x, self.y),
            Point::new(self.right(), self.y),
            Point::new(self.x, self.bottom()),
            Point::new(self.right(), self.bottom()),
        ]
    }
}

/// How far along a direction a point lies within `bounds`, from 0 (the first corner the
/// direction meets) to 1 (the last). `angle_deg` 0 runs left to right, 90 top to bottom.
pub fn along(p: Point, bounds: &Rect, angle_deg: f32) -> f32 {
    let (s, c) = angle_deg.to_radians().sin_cos();
    let project = |q: Point| q.x * c + q.y * s;
    let (lo, hi) = bounds
        .corners()
        .iter()
        .map(|&q| project(q))
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    if hi - lo <= f32::EPSILON {
        return 0.0;
    }
    ((project(p) - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// The length of `bounds` along a direction, in millimetres.
pub fn extent(bounds: &Rect, angle_deg: f32) -> f32 {
    let (s, c) = angle_deg.to_radians().sin_cos();
    bounds.w * c.abs() + bounds.h * s.abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn along_runs_from_zero_to_one_in_the_direction() {
        let b = Rect::new(0.0, 0.0, 1000.0, 500.0);
        assert_eq!(along(Point::new(0.0, 0.0), &b, 0.0), 0.0);
        assert_eq!(along(Point::new(500.0, 0.0), &b, 0.0), 0.5);
        assert_eq!(along(Point::new(0.0, 500.0), &b, 90.0), 1.0);
        assert_eq!(along(Point::new(0.0, 0.0), &b, 180.0), 1.0);
        assert!((extent(&b, 90.0) - 500.0).abs() < 1e-3);
    }

    #[test]
    fn union_and_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(10.0, 5.0, 10.0, 10.0);
        assert!(!a.overlaps(&b), "touching isn't overlapping");
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 20.0, 15.0));
    }
}
