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

/// The glyph as a PNG file (stored, uncompressed deflate): the notification's `IconUri`.
pub fn png(size: u32, state: State) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in bytes {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let c = crc(&out[start..]);
        out.extend_from_slice(&c.to_be_bytes());
    }
    let px = rgba(size, state);
    // Each row: filter byte 0, then RGBA.
    let mut raw = Vec::with_capacity(px.len() + size as usize);
    for row in px.chunks((size * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib: header, stored blocks of at most 65535 bytes, Adler-32.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let n = b.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
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
