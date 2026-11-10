// SPDX-License-Identifier: MIT
//! The tray and window glyph, drawn in code: a rounded orange square with a white roof-and-door
//! mark (a hideout), in four states (design UX8): normal, paused (grey), needs you (a red badge
//! at the top right: a stopped module, one that needs a newer Hideout) and safe mode (the same
//! badge in amber, docs/hideout-supervisor-ux.md). RGBA, straight alpha, row-major.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Normal,
    Paused,
    NeedsYou,
    SafeMode,
}

const ORANGE: [u8; 3] = [0xf0, 0x78, 0x1e];
const GREY: [u8; 3] = [0x8a, 0x84, 0x80];
const BADGE: [u8; 3] = [0xd9, 0x2b, 0x2b];
const AMBER: [u8; 3] = [0xf5, 0xb0, 0x1e];
const MARK: [u8; 3] = [0xff, 0xff, 0xff];

/// A `size` x `size` RGBA image of the glyph.
pub fn rgba(size: u32, state: State) -> Vec<u8> {
    let n = size as f32;
    let fill = if state == State::Paused { GREY } else { ORANGE };
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Pixel centre in 0..1 units.
            let (u, v) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);
            let a = coverage_rounded_square(u, v, 0.06, 0.22, n);
            let mut px = [fill[0], fill[1], fill[2], (a * 255.0).round() as u8];
            if a > 0.0 && in_mark(u, v) {
                px[..3].copy_from_slice(&MARK);
            }
            let badge = match state {
                State::NeedsYou => Some(BADGE),
                State::SafeMode => Some(AMBER),
                _ => None,
            };
            if let Some(b) = badge {
                let d = ((u - 0.80).powi(2) + (v - 0.20).powi(2)).sqrt();
                if d < 0.20 {
                    px = [b[0], b[1], b[2], 255];
                }
            }
            out.extend_from_slice(&px);
        }
    }
    out
}

/// Alpha of a square inset by `inset` with corner radius `r` (units), antialiased over a pixel.
fn coverage_rounded_square(u: f32, v: f32, inset: f32, r: f32, n: f32) -> f32 {
    let (lo, hi) = (inset + r, 1.0 - inset - r);
    let dx = (u.clamp(lo, hi) - u).abs();
    let dy = (v.clamp(lo, hi) - v).abs();
    let dist = (dx * dx + dy * dy).sqrt() - r;
    (0.5 - dist * n).clamp(0.0, 1.0)
}

/// The mark: a roof (triangle) over a house body with a door cut out.
fn in_mark(u: f32, v: f32) -> bool {
    let roof = (0.24..0.48).contains(&v) && (u - 0.5).abs() <= (v - 0.24) * 1.15;
    let body = (0.30..=0.70).contains(&u) && (0.46..=0.76).contains(&v);
    let door = (0.44..=0.56).contains(&u) && v >= 0.58;
    (roof || body) && !door
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn sizes_and_corners() {
        for s in [16, 32, 64] {
            let img = rgba(s, State::Normal);
            assert_eq!(img.len(), (s * s * 4) as usize);
            assert_eq!(px(&img, s, 0, 0)[3], 0, "corners are transparent");
        }
    }

    #[test]
    fn states_differ_where_they_should() {
        let s = 32;
        let (n, p, b) = (
            rgba(s, State::Normal),
            rgba(s, State::Paused),
            rgba(s, State::NeedsYou),
        );
        assert_eq!(px(&n, s, 4, 16), [ORANGE[0], ORANGE[1], ORANGE[2], 255]);
        assert_eq!(&px(&p, s, 4, 16)[..3], &GREY);
        assert_eq!(&px(&b, s, 26, 6)[..3], &BADGE);
        assert_eq!(px(&n, s, 11, 18), [255, 255, 255, 255], "the mark is white");
        assert_eq!(&px(&n, s, 16, 22)[..3], &ORANGE, "the door is cut out");
        assert_eq!(
            px(&n, s, 4, 28),
            px(&b, s, 4, 28),
            "the badge stays in its corner"
        );
    }
}
