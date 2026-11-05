//! Oklab (`palette`, #474): where equal distances look about equally different, in dark blues
//! as in bright yellows. The calibrator chooses its candidates and measures how far two colours
//! are apart here; the transform pipeline (`calibration`) is unchanged.
//!
//! A distance ([`delta_e`]) of about [`JND`] is near one just-noticeable difference.

use palette::{FromColor, LinSrgb, Oklab, Srgb};

/// About one just-noticeable difference, in Oklab distance.
pub const JND: f32 = 0.02;

/// An 8-bit sRGB colour in Oklab (lightness, a, b).
pub fn from_srgb(c: [u8; 3]) -> [f32; 3] {
    let lab = Oklab::from_color(
        Srgb::new(c[0], c[1], c[2])
            .into_format::<f32>()
            .into_linear(),
    );
    [lab.l, lab.a, lab.b]
}

/// An Oklab colour as linear-light sRGB, unclamped (outside 0..=1 when it's out of gamut).
pub fn to_linear(lab: [f32; 3]) -> [f32; 3] {
    let lin = LinSrgb::from_color(Oklab::new(lab[0], lab[1], lab[2]));
    [lin.red, lin.green, lin.blue]
}

/// Linear-light sRGB as 8-bit sRGB, each channel clamped to 0..=1 first.
pub fn linear_to_srgb8(lin: [f32; 3]) -> [u8; 3] {
    let c = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let s: Srgb<u8> = Srgb::from_linear(LinSrgb::new(c(lin[0]), c(lin[1]), c(lin[2])));
    [s.red, s.green, s.blue]
}

/// An Oklab colour as 8-bit sRGB, clamped into gamut.
pub fn to_srgb(lab: [f32; 3]) -> [u8; 3] {
    linear_to_srgb8(to_linear(lab))
}

/// How far apart two Oklab colours are (Euclidean ΔE).
pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// How far apart two 8-bit sRGB colours look (Oklab ΔE): about [`JND`] is just noticeable.
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f32 {
    distance(from_srgb(a), from_srgb(b))
}

/// Chroma: how far from grey (the length of a, b).
pub fn chroma(lab: [f32; 3]) -> f32 {
    (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_colours_land_where_oklab_puts_them() {
        let white = from_srgb([255; 3]);
        assert!(
            (white[0] - 1.0).abs() < 1e-3 && chroma(white) < 1e-3,
            "{white:?}"
        );
        assert!(from_srgb([0; 3])[0].abs() < 1e-6);
        // Ottosson's reference: sRGB red is about L 0.628, a 0.225, b 0.126.
        let red = from_srgb([255, 0, 0]);
        assert!((red[0] - 0.628).abs() < 2e-3, "{red:?}");
        assert!((red[1] - 0.225).abs() < 2e-3 && (red[2] - 0.126).abs() < 2e-3);
    }

    #[test]
    fn round_trips_and_distances() {
        for c in [
            [153, 153, 153],
            [153, 0, 0],
            [0, 0, 153],
            [153, 112, 92],
            [12, 200, 77],
        ] {
            assert_eq!(to_srgb(from_srgb(c)), c, "{c:?}");
            assert!(delta_e(c, c) < 1e-6);
        }
        // One 8-bit step at the test level is far below a just-noticeable difference.
        assert!(delta_e([153; 3], [153, 153, 154]) < JND / 4.0);
        // A clearly warmer white is several.
        assert!(delta_e([153; 3], [153, 147, 107]) > 2.0 * JND);
        assert_eq!(to_srgb([2.0, 0.0, 0.0]), [255; 3], "out of gamut clamps");
    }
}
