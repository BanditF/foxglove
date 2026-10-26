//! Draws the panel's layout onto a digitalis Panel.

pub use digitalis::panel::Panel;
use embedded_graphics::{
    mono_font::{
        ascii::{FONT_4X6, FONT_5X8, FONT_9X18_BOLD},
        MonoFont, MonoTextStyle,
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle, Triangle},
    text::{Baseline, Text},
};

/// Everything the panel shows; the providers fill it in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub time: String,
    pub date: String,
    pub volume: Option<(u8, bool)>, // percent, muted
    pub title: Option<String>,
    pub artist: Option<String>,
    pub playing: bool,
    pub hint: Option<String>,
}

fn text_width(font: &MonoFont, s: &str) -> i32 {
    (s.chars().count() as u32 * (font.character_size.width + font.character_spacing)) as i32
}

/// Text in a band x0..x0+w; centred if it fits, otherwise scrolling by `tick` (a marquee).
fn band_text(p: &mut Panel, font: &'static MonoFont, s: &str, y: i32, ink: BinaryColor, tick: u32) {
    let w = p.width as i32;
    let tw = text_width(font, s);
    let style = MonoTextStyle::new(font, ink);
    if tw <= w {
        Text::with_baseline(s, Point::new((w - tw) / 2, y), style, Baseline::Top).draw(p).ok();
        return;
    }
    let gap = 16;
    let x = -((tick as i32 * 2) % (tw + gap));
    Text::with_baseline(s, Point::new(x, y), style, Baseline::Top).draw(p).ok();
    Text::with_baseline(s, Point::new(x + tw + gap, y), style, Baseline::Top).draw(p).ok();
}

/// Only visible overflowing text needs animation frames, and media text only while it plays:
/// a paused title stands still, so a paused session costs no frames (#410).
pub fn animated(v: &View, width: usize) -> bool {
    let over = |font, text: &str| text_width(font, text) > width as i32;
    (v.playing
        && (v.title.as_deref().is_some_and(|s| over(&FONT_5X8, s))
            || (v.title.is_some() && v.hint.is_none()
                && v.artist.as_deref().is_some_and(|s| over(&FONT_4X6, s)))))
        || v.hint.as_deref().is_some_and(|s| over(&FONT_5X8, s))
}

pub fn draw(p: &mut Panel, v: &View, tick: u32) {
    p.clear();
    let on = BinaryColor::On;
    let w = p.width as i32;
    let fill = PrimitiveStyle::with_fill(on);
    let line = PrimitiveStyle::with_stroke(on, 1);

    // Clock
    band_text(p, &FONT_9X18_BOLD, &v.time, 0, on, 0);

    // Volume: speaker and a bar, crossed out when muted
    if let Some((vol, muted)) = v.volume {
        let y = 20;
        Rectangle::new(Point::new(0, y + 2), Size::new(2, 3)).into_styled(fill).draw(p).ok();
        Triangle::new(Point::new(2, y + 3), Point::new(5, y), Point::new(5, y + 6))
            .into_styled(fill)
            .draw(p)
            .ok();
        let bar = Rectangle::new(Point::new(8, y), Size::new((w - 8) as u32, 7));
        bar.into_styled(line).draw(p).ok();
        let inner = (w - 12) * vol.min(100) as i32 / 100;
        if inner > 0 {
            Rectangle::new(Point::new(10, y + 2), Size::new(inner as u32, 3)).into_styled(fill).draw(p).ok();
        }
        if muted {
            Line::new(Point::new(8, y), Point::new(w - 1, y + 6)).into_styled(line).draw(p).ok();
            Line::new(Point::new(8, y + 6), Point::new(w - 1, y)).into_styled(line).draw(p).ok();
        }
    }

    // Media, or the date when nothing is playing
    match &v.title {
        Some(title) => {
            // Paused: the start of the title, still (see `animated`).
            let media_tick = if v.playing { tick } else { 0 };
            band_text(p, &FONT_5X8, title, 30, on, media_tick);
            if v.hint.is_none() {
                if let Some(artist) = &v.artist {
                    band_text(p, &FONT_4X6, artist, 41, on, media_tick);
                }
            }
            if !v.playing {
                // paused: dotted underline under the title
                for x in (0..w).step_by(4) {
                    p.set(x, 39, true);
                }
            }
        }
        None => band_text(p, &FONT_5X8, &v.date, 30, on, 0),
    }

    // Game hint: inverted strip along the bottom
    if let Some(hint) = &v.hint {
        let y = p.height as i32 - 10;
        Rectangle::new(Point::new(0, y), Size::new(w as u32, 10)).into_styled(fill).draw(p).ok();
        band_text(p, &FONT_5X8, hint, y + 1, BinaryColor::Off, tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_visible_overflowing_bands_animate() {
        let mut v = View { time: "12:34".into(), date: "Fri 23 Oct".into(), playing: true, ..View::default() };
        assert!(!animated(&v, 64));
        v.artist = Some("An artist with a very long name".into());
        assert!(!animated(&v, 64)); // artist is hidden without a title
        v.title = Some("Short".into());
        assert!(animated(&v, 64));
        v.hint = Some("Hint".into());
        assert!(!animated(&v, 64)); // hint replaces artist
        v.title = Some("A title that needs to scroll".into());
        assert!(animated(&v, 64));
        v.hint = None;
        v.playing = false;
        assert!(!animated(&v, 64), "a paused title doesn't scroll");
        v.title = None;
        v.hint = Some("A hint that needs to scroll".into());
        assert!(animated(&v, 64));
        assert!(!animated(&v, 256));
    }

    #[test]
    fn draws_inside_the_panel() {
        let mut p = Panel::new(64, 52);
        let v = View {
            time: "14:05".into(),
            date: "Tue 20 Oct".into(),
            volume: Some((40, false)),
            title: Some("A very long song title that scrolls".into()),
            artist: Some("Artist".into()),
            playing: true,
            hint: Some("GAME: WASD".into()),
        };
        draw(&mut p, &v, 3);
        assert_eq!(p.bits.len(), 8 * 52);
        assert!(p.bits.iter().any(|&b| b != 0));
    }
}
