//! The left panel, drawn from Art's assets (`assets/plugins/org.foxglove.focus-timer/panel.json`,
//! docs/plugin-screens.md): phase icon and label, round, MM:SS, a progress bar and a hint.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::timer::Phase;

pub const W: usize = 64;
pub const H: usize = 52;
const STRIDE: usize = W / 8;

#[derive(Deserialize)]
struct Sprite {
    w: usize,
    h: usize,
    bitmap: String,
    /// `bitmap` decoded, filled in by [`Art::load`].
    #[serde(skip)]
    bits: Vec<u8>,
}

#[derive(Deserialize)]
struct At {
    x: i32,
    y: i32,
}

#[derive(Deserialize)]
struct Digits {
    x: i32,
    y: i32,
    advance: i32,
    colon_gap: i32,
}

#[derive(Deserialize)]
struct Bar {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Deserialize)]
struct Layout {
    icon: At,
    phase_label: At,
    round_label: At,
    digits: Digits,
    bar: Bar,
    bar_ticks: At2,
    hint: At,
}

#[derive(Deserialize)]
struct At2 {
    y: i32,
}

#[derive(Deserialize)]
struct Font {
    advance: i32,
    glyphs: BTreeMap<String, Sprite>,
}

#[derive(Deserialize)]
struct Assets {
    format: String,
    layout: Layout,
    digits: BTreeMap<String, Sprite>,
    colon: Sprite,
    icons: BTreeMap<String, Sprite>,
    font: Font,
}

/// What one frame shows.
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub phase: Phase,
    pub paused: bool,
    pub round: u32,
    pub rounds: u32,
    /// Seconds left.
    pub left: u64,
    /// 0.0 to 1.0.
    pub progress: f32,
    pub hint: &'a str,
}

/// Decoded sprites, ready to draw.
pub struct Art {
    a: Assets,
}

const JSON: &str = include_str!("../../../assets/plugins/org.foxglove.focus-timer/panel.json");

impl Art {
    pub fn load() -> Result<Art, String> {
        use base64::Engine;
        let mut a: Assets = serde_json::from_str(JSON).map_err(|e| e.to_string())?;
        if a.format != "foxglove-panel-assets" {
            return Err("not panel assets".into());
        }
        let all = a
            .digits
            .values_mut()
            .chain([&mut a.colon])
            .chain(a.icons.values_mut())
            .chain(a.font.glyphs.values_mut());
        for s in all {
            s.bits = base64::engine::general_purpose::STANDARD
                .decode(&s.bitmap)
                .map_err(|e| e.to_string())?;
            if s.bits.len() != s.w.div_ceil(8) * s.h {
                return Err("a sprite's size doesn't match its bitmap".into());
            }
        }
        Ok(Art { a })
    }

    fn blit(&self, frame: &mut [u8], s: &Sprite, x: i32, y: i32) {
        let b = &s.bits;
        let row = s.w.div_ceil(8);
        for r in 0..s.h {
            for c in 0..s.w {
                if b[r * row + c / 8] & (0x80 >> (c % 8)) != 0 {
                    set(frame, x + c as i32, y + r as i32);
                }
            }
        }
    }

    fn text_width(&self, text: &str) -> i32 {
        (text.chars().count() as i32 * self.a.font.advance - 1).max(0)
    }

    fn text(&self, frame: &mut [u8], text: &str, x: i32, y: i32) {
        for (i, ch) in text.to_uppercase().chars().enumerate() {
            if let Some(g) = self.a.font.glyphs.get(&ch.to_string()) {
                self.blit(frame, g, x + i as i32 * self.a.font.advance, y);
            }
        }
    }

    /// One frame: 1 bpp, rows MSB first, 8 bytes a row.
    pub fn frame(&self, v: &View) -> Vec<u8> {
        let View {
            phase,
            paused,
            round,
            rounds,
            left,
            progress,
            hint,
        } = *v;
        let l = &self.a.layout;
        let mut f = vec![0u8; STRIDE * H];
        let (icon, label) = match (paused, phase) {
            (true, _) => ("paused", "PAUSED"),
            (false, Phase::Focus) => ("focus", "FOCUS"),
            (false, _) => ("rest", "REST"),
        };
        if let Some(i) = self.a.icons.get(icon) {
            self.blit(&mut f, i, l.icon.x, l.icon.y);
        }
        self.text(&mut f, label, l.phase_label.x, l.phase_label.y);
        let r = format!("{round}/{rounds}");
        self.text(
            &mut f,
            &r,
            l.round_label.x - self.text_width(&r),
            l.round_label.y,
        );
        // MM:SS: digit, +advance, colon with a gap each side, digit, +advance.
        let (m, s) = ((left / 60).min(99), left % 60);
        let digits = [m / 10, m % 10, s / 10, s % 10].map(|d| &self.a.digits[&d.to_string()]);
        let d = &l.digits;
        let colon_x = d.x + d.advance + digits[1].w as i32 + d.colon_gap;
        let after = colon_x + self.a.colon.w as i32 + d.colon_gap;
        for (digit, x) in digits
            .iter()
            .zip([d.x, d.x + d.advance, after, after + d.advance])
        {
            self.blit(&mut f, digit, x, d.y);
        }
        self.blit(&mut f, &self.a.colon, colon_x, d.y);
        // The bar: an outline, filled 2 px in by the progress; ticks under it every 20%.
        let b = &l.bar;
        for x in b.x..b.x + b.w {
            set(&mut f, x, b.y);
            set(&mut f, x, b.y + b.h - 1);
        }
        for y in b.y..b.y + b.h {
            set(&mut f, b.x, y);
            set(&mut f, b.x + b.w - 1, y);
        }
        let inner = b.w - 4;
        let filled = (progress.clamp(0.0, 1.0) * inner as f32).round() as i32;
        for x in b.x + 2..b.x + 2 + filled {
            for y in b.y + 2..b.y + b.h - 2 {
                set(&mut f, x, y);
            }
        }
        for k in 0..=5 {
            set(&mut f, b.x + (k * (b.w - 1)) / 5, l.bar_ticks.y);
        }
        let hint: String = hint.chars().take(15).collect();
        self.text(
            &mut f,
            &hint,
            l.hint.x - self.text_width(&hint) / 2,
            l.hint.y,
        );
        f
    }
}

fn set(frame: &mut [u8], x: i32, y: i32) {
    if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
        frame[y as usize * STRIDE + x as usize / 8] |= 0x80 >> (x as usize % 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(f: &[u8], x: usize, y: usize) -> bool {
        f[y * STRIDE + x / 8] & (0x80 >> (x % 8)) != 0
    }

    #[test]
    fn draws_a_frame_from_the_assets() {
        let art = Art::load().unwrap();
        let f = art.frame(&View {
            phase: Phase::Focus,
            paused: false,
            round: 2,
            rounds: 4,
            left: 24 * 60 + 59,
            progress: 0.5,
            hint: "NEXT REST 5M",
        });
        assert_eq!(f.len(), 416);
        // The bar's outline corners, half its inside filled, the right half empty.
        assert!(lit(&f, 2, 38) && lit(&f, 61, 43));
        assert!(lit(&f, 4, 40) && lit(&f, 31, 41));
        assert!(!lit(&f, 50, 40));
        // Something drawn in the digits' band and the label row.
        assert!((13..34).any(|y| (5..57).any(|x| lit(&f, x, y))));
        assert!((4..9).any(|y| (14..40).any(|x| lit(&f, x, y))));
        // Paused shows another frame, and the time changes the digits.
        assert_ne!(
            f,
            art.frame(&View {
                phase: Phase::Focus,
                paused: true,
                round: 2,
                rounds: 4,
                left: 24 * 60 + 59,
                progress: 0.5,
                hint: "NEXT REST 5M"
            })
        );
        assert_ne!(
            f,
            art.frame(&View {
                phase: Phase::Focus,
                paused: false,
                round: 2,
                rounds: 4,
                left: 60,
                progress: 0.5,
                hint: "NEXT REST 5M"
            })
        );
    }
}

#[cfg(test)]
mod look {
    use super::*;

    /// Writes a frame as hex to `FOCUS_FRAME_OUT`, to look at: `cargo test -p plugin-focus-timer look -- --ignored`.
    #[test]
    #[ignore]
    fn dump_a_frame() {
        let art = Art::load().unwrap();
        let f = art.frame(&View {
            phase: Phase::Focus,
            paused: false,
            round: 2,
            rounds: 4,
            left: 24 * 60 + 59,
            progress: 0.35,
            hint: "NEXT REST 5M",
        });
        let hex: String = f.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(std::env::var("FOCUS_FRAME_OUT").unwrap(), hex).unwrap();
    }
}
