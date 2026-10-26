//! Draws the panel's layout onto a digitalis Panel.

pub use digitalis::panel::Panel;
use embedded_graphics::{
    mono_font::{
        // Latin-1, so region formats and titles with accents draw (#445).
        iso_8859_1::{FONT_4X6, FONT_5X8, FONT_9X18_BOLD},
        MonoFont,
        MonoTextStyle,
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle, Triangle},
    text::{Baseline, Text},
};

/// Everything the panel shows; the providers fill it in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The time without its AM/PM mark; `time_mark` is "" on a 24-hour clock (#445).
    pub time: String,
    pub time_mark: String,
    pub time_mark_first: bool,
    pub date: String,
    /// A shorter date for when `date` doesn't fit.
    pub date_short: String,
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
        Text::with_baseline(s, Point::new((w - tw) / 2, y), style, Baseline::Top)
            .draw(p)
            .ok();
        return;
    }
    let gap = 16;
    let x = -((tick as i32 * 2) % (tw + gap));
    Text::with_baseline(s, Point::new(x, y), style, Baseline::Top)
        .draw(p)
        .ok();
    Text::with_baseline(s, Point::new(x + tw + gap, y), style, Baseline::Top)
        .draw(p)
        .ok();
}

/// Only visible overflowing text needs animation frames, and media text only while it plays:
/// a paused title stands still, so a paused session costs no frames (#410).
pub fn animated(v: &View, width: usize) -> bool {
    let over = |font, text: &str| text_width(font, text) > width as i32;
    (v.playing
        && (v.title.as_deref().is_some_and(|s| over(&FONT_5X8, s))
            || (v.title.is_some()
                && v.hint.is_none()
                && v.artist.as_deref().is_some_and(|s| over(&FONT_4X6, s)))))
        || v.hint.as_deref().is_some_and(|s| over(&FONT_5X8, s))
}

/// The date, or its shorter form when the full one doesn't fit `width`.
pub fn date_for(v: &View, width: i32) -> &str {
    if text_width(&FONT_5X8, &v.date) <= width || v.date_short.is_empty() {
        &v.date
    } else {
        &v.date_short
    }
}

/// Whether the clock (with its mark) fits `width` without clipping.
pub fn clock_fits(v: &View, width: i32) -> bool {
    clock_width(v) <= width
}

const MARK_GAP: i32 = 2;

fn clock_width(v: &View) -> i32 {
    let digits = text_width(&FONT_9X18_BOLD, &v.time);
    if v.time_mark.is_empty() {
        digits
    } else {
        digits + MARK_GAP + text_width(&FONT_5X8, &v.time_mark)
    }
}

fn draw_clock_with_mark(p: &mut Panel, v: &View) {
    let x = ((p.width as i32 - clock_width(v)) / 2).max(0);
    let digits = text_width(&FONT_9X18_BOLD, &v.time);
    let mark = text_width(&FONT_5X8, &v.time_mark);
    let (time_x, mark_x) = if v.time_mark_first {
        (x + mark + MARK_GAP, x)
    } else {
        (x, x + digits + MARK_GAP)
    };
    let big = MonoTextStyle::new(&FONT_9X18_BOLD, BinaryColor::On);
    let small = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    Text::with_baseline(&v.time, Point::new(time_x, 0), big, Baseline::Top)
        .draw(p)
        .ok();
    // Level with the bottom of the digits.
    Text::with_baseline(&v.time_mark, Point::new(mark_x, 7), small, Baseline::Top)
        .draw(p)
        .ok();
}

pub fn draw(p: &mut Panel, v: &View, tick: u32) {
    p.clear();
    let on = BinaryColor::On;
    let w = p.width as i32;
    let fill = PrimitiveStyle::with_fill(on);
    let line = PrimitiveStyle::with_stroke(on, 1);

    // Clock, with a small AM/PM mark beside it on a 12-hour clock
    if v.time_mark.is_empty() {
        band_text(p, &FONT_9X18_BOLD, &v.time, 0, on, 0);
    } else {
        draw_clock_with_mark(p, v);
    }

    // Volume: speaker and a bar, crossed out when muted
    if let Some((vol, muted)) = v.volume {
        let y = 20;
        Rectangle::new(Point::new(0, y + 2), Size::new(2, 3))
            .into_styled(fill)
            .draw(p)
            .ok();
        Triangle::new(Point::new(2, y + 3), Point::new(5, y), Point::new(5, y + 6))
            .into_styled(fill)
            .draw(p)
            .ok();
        let bar = Rectangle::new(Point::new(8, y), Size::new((w - 8) as u32, 7));
        bar.into_styled(line).draw(p).ok();
        let inner = (w - 12) * vol.min(100) as i32 / 100;
        if inner > 0 {
            Rectangle::new(Point::new(10, y + 2), Size::new(inner as u32, 3))
                .into_styled(fill)
                .draw(p)
                .ok();
        }
        if muted {
            Line::new(Point::new(8, y), Point::new(w - 1, y + 6))
                .into_styled(line)
                .draw(p)
                .ok();
            Line::new(Point::new(8, y + 6), Point::new(w - 1, y))
                .into_styled(line)
                .draw(p)
                .ok();
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
        None => band_text(p, &FONT_5X8, date_for(v, w), 30, on, 0),
    }

    // Game hint: inverted strip along the bottom
    if let Some(hint) = &v.hint {
        let y = p.height as i32 - 10;
        Rectangle::new(Point::new(0, y), Size::new(w as u32, 10))
            .into_styled(fill)
            .draw(p)
            .ok();
        band_text(p, &FONT_5X8, hint, y + 1, BinaryColor::Off, tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_visible_overflowing_bands_animate() {
        let mut v = View {
            time: "12:34".into(),
            date: "Fri 23 Oct".into(),
            playing: true,
            ..View::default()
        };
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
            ..View::default()
        };
        draw(&mut p, &v, 3);
        assert_eq!(p.bits.len(), 8 * 52);
        assert!(p.bits.iter().any(|&b| b != 0));
    }

    /// 12-hour, 24-hour and German-style clocks all fit the Corne's 64-pixel panel (#445).
    #[test]
    fn region_formats_fit_the_panel() {
        let cases = [
            ("12:59", "PM", false, "Wed, Sep 30", "Sep 30"),
            ("23:59", "", false, "Wed 30 Sep", "30 Sep"),
            ("23:59", "", false, "Mi., 30. Sep.", "30. Sep."),
            ("12:59", "PM", true, "09/30", "09/30"),
        ];
        for (time, mark, first, date, short) in cases {
            let v = View {
                time: time.into(),
                time_mark: mark.into(),
                time_mark_first: first,
                date: date.into(),
                date_short: short.into(),
                ..View::default()
            };
            assert!(clock_fits(&v, 64), "{time} {mark}");
            assert!(text_width(&FONT_5X8, date_for(&v, 64)) <= 64, "{date}");
            let mut p = Panel::new(64, 52);
            draw(&mut p, &v, 0);
            assert!(p.bits.iter().any(|&b| b != 0));
        }
        let de = View {
            date: "Mi., 30. Sep.".into(),
            date_short: "30. Sep.".into(),
            ..View::default()
        };
        assert_eq!(date_for(&de, 64), "30. Sep.", "13 characters are 65 pixels");
        assert_eq!(date_for(&de, 160), "Mi., 30. Sep.");
    }
}
