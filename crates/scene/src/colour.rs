//! Colours: 8-bit sRGB in and out, mixed in Oklab (`palette`, #474) so a blend between two
//! colours stays even in brightness instead of dipping through grey.

use palette::{FromColor, LinSrgb, Mix, Oklab, Srgb};
use serde::{Deserialize, Serialize};

/// An 8-bit sRGB colour, as devices take it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub const BLACK: Rgb = Rgb([0, 0, 0]);
    pub const WHITE: Rgb = Rgb([255, 255, 255]);

    fn oklab(self) -> Oklab {
        let [r, g, b] = self.0;
        Oklab::from_color(Srgb::new(r, g, b).into_format::<f32>().into_linear())
    }

    fn from_oklab(c: Oklab) -> Rgb {
        let lin = LinSrgb::from_color(c);
        let s: Srgb<u8> = Srgb::from_linear(lin);
        Rgb([s.red, s.green, s.blue])
    }

    /// `self` at `t` = 0, `to` at `t` = 1, through Oklab.
    pub fn mix(self, to: Rgb, t: f32) -> Rgb {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if t == 0.0 {
            return self;
        }
        if t == 1.0 {
            return to;
        }
        Rgb::from_oklab(self.oklab().mix(to.oklab(), t))
    }
}

/// The colour at `t` (0..=1) along evenly spaced stops; `cyclic` wraps the last stop back to
/// the first, for colours that scroll forever.
pub fn gradient(stops: &[Rgb], t: f32, cyclic: bool) -> Rgb {
    match stops {
        [] => Rgb::BLACK,
        [one] => *one,
        _ => {
            let t = if t.is_finite() { t } else { 0.0 };
            let (t, spans) = if cyclic {
                (t.rem_euclid(1.0), stops.len())
            } else {
                (t.clamp(0.0, 1.0), stops.len() - 1)
            };
            let at = t * spans as f32;
            let i = (at.floor() as usize).min(spans - 1);
            let next = (i + 1) % stops.len();
            stops[i].mix(stops[next], at - i as f32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_keeps_the_ends_and_stays_bright_in_the_middle() {
        let red = Rgb([255, 0, 0]);
        let blue = Rgb([0, 0, 255]);
        assert_eq!(red.mix(blue, 0.0), red);
        assert_eq!(red.mix(blue, 1.0), blue);
        let mid = red.mix(blue, 0.5);
        // A plain sRGB average is (128, 0, 128); Oklab keeps more light in the middle.
        assert!(mid.0[0] > 128 && mid.0[2] > 128, "{mid:?}");
    }

    #[test]
    fn gradients_wrap_only_when_cyclic() {
        let stops = [Rgb([255, 0, 0]), Rgb([0, 255, 0])];
        assert_eq!(gradient(&stops, 1.0, false), stops[1]);
        assert_eq!(gradient(&stops, 1.0, true), stops[0]);
        assert_eq!(gradient(&stops, 0.5, true), stops[1]);
        assert_eq!(gradient(&[], 0.3, true), Rgb::BLACK);
    }
}
