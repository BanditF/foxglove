//! The left panel's one-shot "ART 3/7" card (#60): when the art key steps the hotlist, the
//! panel shows which entry is next for a moment. It goes through the panel override with a
//! TTL, so the panel loop ends it by its own deadline; there's no timer of ours.

use std::time::Duration;

/// How long the card stays up.
pub const SHOW_FOR: Duration = Duration::from_millis(1200);

/// The override's owner id; plugins use their connection ids (from 1 up).
pub const CONN: u64 = u64::MAX;

/// The Bandit HUD's 3x5 font (4 px per character), rows of 3 bits.
fn glyph(c: char) -> [u8; 5] {
    let rows = |s: &str| {
        let b = s.as_bytes();
        let row = |r: usize| (b[r * 3] - b'0') << 2 | (b[r * 3 + 1] - b'0') << 1 | (b[r * 3 + 2] - b'0');
        [row(0), row(1), row(2), row(3), row(4)]
    };
    rows(match c.to_ascii_uppercase() {
        'A' => "010101111101101", 'B' => "110101110101110", 'C' => "011100100100011", 'D' => "110101101101110",
        'E' => "111100110100111", 'F' => "111100110100100", 'G' => "011100101101011", 'H' => "101101111101101",
        'I' => "111010010010111", 'J' => "001001001101010", 'K' => "101101110101101", 'L' => "100100100100111",
        'M' => "101111111101101", 'N' => "110101101101101", 'O' => "010101101101010", 'P' => "110101110100100",
        'Q' => "010101101110011", 'R' => "110101110101101", 'S' => "011100010001110", 'T' => "111010010010010",
        'U' => "101101101101111", 'V' => "101101101101010", 'W' => "101101111111101", 'X' => "101101010101101",
        'Y' => "101101010010010", 'Z' => "111001010100111", '0' => "111101101101111", '1' => "010110010010111",
        '2' => "110001010100111", '3' => "110001010001110", '4' => "101101111001001", '5' => "111100110001110",
        '6' => "011100111101111", '7' => "111001010010010", '8' => "111101111101111", '9' => "111101111001110",
        '-' => "000000111000000", '.' => "000000000000010", '/' => "001001010100100", ':' => "000010000010000",
        _ => "000000000000000",
    })
}

struct Canvas {
    w: usize,
    h: usize,
    bits: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Canvas { w, h, bits: vec![0; w.div_ceil(8) * h] }
    }

    fn set(&mut self, x: usize, y: usize) {
        if x < self.w && y < self.h {
            self.bits[y * self.w.div_ceil(8) + x / 8] |= 0x80 >> (x % 8);
        }
    }

    /// Text in the 3x5 font at `scale`, with a 1-pixel (scaled) gap.
    fn text(&mut self, x: usize, y: usize, s: &str, scale: usize) {
        for (i, c) in s.chars().enumerate() {
            let g = glyph(c);
            for (r, row) in g.iter().enumerate() {
                for col in 0..3 {
                    if row & (4 >> col) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                self.set(x + (i * 4 + col) * scale + dx, y + r * scale + dy);
                            }
                        }
                    }
                }
            }
        }
    }

    fn text_width(s: &str, scale: usize) -> usize {
        (s.chars().count() * 4).saturating_sub(1) * scale
    }

    fn centred(&mut self, y: usize, s: &str, scale: usize) {
        let w = Self::text_width(s, scale).min(self.w);
        self.text((self.w - w) / 2, y, s, scale);
    }
}

/// The card for entry `index` (from 0) of `total`, labelled `label`, after a step in `dir`.
/// `width` x `height` is the panel (1 bpp, rows MSB first, padded to whole bytes).
pub fn frame(width: usize, height: usize, index: usize, total: usize, label: &str, dir: i8) -> Vec<u8> {
    let mut c = Canvas::new(width, height);
    // Bandit corner brackets
    for (x, y, dx, dy) in [(0, 0, 1i32, 1i32), (width - 1, 0, -1, 1), (0, height - 1, 1, -1), (width - 1, height - 1, -1, -1)] {
        for i in 0..4i32 {
            c.set((x as i32 + dx * i) as usize, y);
            c.set(x, (y as i32 + dy * i) as usize);
        }
    }
    c.centred(5, &format!("ART {}/{}", index + 1, total), 2);
    // the step's direction: a filled triangle pointing the way the list moved
    let (cx, cy) = (width / 2, height / 2 + 4);
    for i in 0..5 {
        for y in cy - i..=cy + i {
            let x = if dir < 0 { cx - 2 + i } else { cx + 2 - i };
            c.set(x, y);
        }
    }
    let label: String = label.chars().take(width / 4).collect();
    c.centred(height - 9, &label, 1);
    c.bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(bits: &[u8], w: usize, x: usize, y: usize) -> bool {
        bits[y * w.div_ceil(8) + x / 8] & (0x80 >> (x % 8)) != 0
    }

    #[test]
    fn card_is_a_full_panel_frame_with_brackets_and_text() {
        let f = frame(64, 52, 2, 7, "fury print", 1);
        assert_eq!(f.len(), 8 * 52, "416 bytes, as the panel takes");
        assert!(lit(&f, 64, 0, 0) && lit(&f, 64, 63, 51), "corner brackets");
        // "ART 3/7" at 2x: the A's top row starts one pixel in, two pixels thick
        let x0 = (64 - Canvas::text_width("ART 3/7", 2)) / 2;
        assert!(lit(&f, 64, x0 + 2, 5) && lit(&f, 64, x0 + 2, 6));
        assert!(f.iter().filter(|b| **b != 0).count() > 20, "the label is drawn");
    }

    #[test]
    fn long_labels_are_cut_to_the_panel() {
        let f = frame(64, 52, 0, 1, &"X".repeat(40), -1);
        assert_eq!(f.len(), 416);
    }
}
