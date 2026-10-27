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

/// Whether the clock, with its mark as drawn, fits `width` without clipping.
pub fn clock_fits(v: &View, width: i32) -> bool {
    width_with(&v.time, &fitting_mark(&v.time, &v.time_mark, width)) <= width
}

const MARK_GAP: i32 = 2;

fn width_with(time: &str, mark: &str) -> i32 {
    let digits = text_width(&FONT_9X18_BOLD, time);
    if mark.is_empty() {
        digits
    } else {
        digits + MARK_GAP + text_width(&FONT_5X8, mark)
    }
}

/// The AM/PM mark as drawn: as the region writes it if it fits, else without spaces and dots
/// ("p. m." -> "pm"), else its first letter.
pub fn fitting_mark(time: &str, mark: &str, width: i32) -> String {
    let compact: String = mark
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '.')
        .collect();
    let first: String = compact.chars().take(1).collect();
    [mark.to_string(), compact, first]
        .into_iter()
        .find(|m| width_with(time, m) <= width)
        .unwrap_or_default()
}

fn draw_clock_with_mark(p: &mut Panel, v: &View) {
    let mark_text = fitting_mark(&v.time, &v.time_mark, p.width as i32);
    let x = ((p.width as i32 - width_with(&v.time, &mark_text)) / 2).max(0);
    let digits = text_width(&FONT_9X18_BOLD, &v.time);
    let mark = text_width(&FONT_5X8, &mark_text);
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
    Text::with_baseline(&mark_text, Point::new(mark_x, 7), small, Baseline::Top)
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

/// The PC load layout (#506): CPU, GPU and memory, each a line with its name and percentage over
/// a bar as wide as the panel. A value not known yet reads `--` over an empty bar.
pub fn draw_stats(p: &mut Panel, s: &crate::stats::Stats) {
    p.clear();
    let on = BinaryColor::On;
    let w = p.width as i32;
    let rows = [("CPU", s.cpu), ("GPU", s.gpu), ("RAM", s.ram)];
    // Rows of 17 points on the Corne's 52: 8 of text, a gap, a 6-point bar, a gap.
    let pitch = (p.height as i32 / rows.len() as i32).clamp(16, 24);
    let style = MonoTextStyle::new(&FONT_5X8, on);
    for (n, (name, value)) in rows.into_iter().enumerate() {
        let y = n as i32 * pitch;
        Text::with_baseline(name, Point::new(0, y), style, Baseline::Top)
            .draw(p)
            .ok();
        let number = value.map_or_else(|| "--".to_string(), |v| format!("{v}%"));
        let x = w - text_width(&FONT_5X8, &number);
        Text::with_baseline(&number, Point::new(x, y), style, Baseline::Top)
            .draw(p)
            .ok();
        Rectangle::new(Point::new(0, y + 9), Size::new(w as u32, 6))
            .into_styled(PrimitiveStyle::with_stroke(on, 1))
            .draw(p)
            .ok();
        let inner = (w - 4) * i32::from(value.unwrap_or(0).min(100)) / 100;
        if inner > 0 {
            Rectangle::new(Point::new(2, y + 11), Size::new(inner as u32, 2))
                .into_styled(PrimitiveStyle::with_fill(on))
                .draw(p)
                .ok();
        }
    }
}

/// A Windows notification on the panel (#507). `title` and `body` are only ever set for apps the
/// user allowed to show their text; otherwise the panel shows the app's name alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notice {
    pub app: String,
    pub title: Option<String>,
    pub body: Option<String>,
    /// Further notifications that arrived with this one.
    pub more: usize,
}

/// The large bell over an app's name, 11×12.
const BELL: [&str; 12] = [
    "....###....",
    "...#####...",
    "..#######..",
    "..#######..",
    "..#######..",
    ".#########.",
    ".#########.",
    ".#########.",
    "###########",
    "###########",
    "...........",
    "....###....",
];

/// The small bell in the header band, 7×7.
const SMALL_BELL: [&str; 7] = [
    "..###..", ".#####.", ".#####.", ".#####.", "#######", ".......", "..###..",
];

fn glyph(p: &mut Panel, rows: &[&str], x: i32, y: i32, on: bool) {
    for (dy, row) in rows.iter().enumerate() {
        for (dx, c) in row.chars().enumerate() {
            if c == '#' {
                p.set(x + dx as i32, y + dy as i32, on);
            }
        }
    }
}

/// `s` cut to `width` pixels, ending in ".." when it was longer.
fn clipped(font: &MonoFont, s: &str, width: i32) -> String {
    if text_width(font, s) <= width {
        return s.to_string();
    }
    let per = (font.character_size.width + font.character_spacing) as i32;
    let keep = (width / per - 2).max(0) as usize;
    let mut out: String = s.chars().take(keep).collect::<String>().trim_end().into();
    out.push_str("..");
    out
}

/// Word-wrapped lines of at most `width` pixels, no more than `lines` of them; a long word is
/// broken, and text that doesn't fit ends the last line in "..".
pub fn wrap(font: &MonoFont, s: &str, width: i32, lines: usize) -> Vec<String> {
    let per = (font.character_size.width + font.character_spacing) as i32;
    let max = (width / per).max(1) as usize;
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = line.chars().count();
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= max {
                if gap == 1 {
                    line.push(' ');
                }
                line.extend(word.iter());
                break;
            }
            if used > 0 {
                out.push(std::mem::take(&mut line));
                continue;
            }
            // A word longer than the line: break it.
            line = word.drain(..max).collect();
            out.push(std::mem::take(&mut line));
            if word.is_empty() {
                break;
            }
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if lines == 0 {
        return Vec::new();
    }
    if out.len() > lines {
        // The last line takes as much of the rest as fits.
        let rest = out[lines - 1..].join(" ");
        out.truncate(lines - 1);
        let cut: String = rest.chars().take(max.saturating_sub(2)).collect();
        out.push(format!("{}..", cut.trim_end()));
    }
    out
}

fn text_at(p: &mut Panel, font: &MonoFont, s: &str, x: i32, y: i32, ink: BinaryColor) {
    Text::with_baseline(
        s,
        Point::new(x, y),
        MonoTextStyle::new(font, ink),
        Baseline::Top,
    )
    .draw(p)
    .ok();
}

fn centred(p: &mut Panel, font: &MonoFont, s: &str, y: i32) {
    let x = ((p.width as i32 - text_width(font, s)) / 2).max(0);
    text_at(p, font, s, x, y, BinaryColor::On);
}

/// A notification (#507), still: nothing scrolls, so it costs one frame.
/// - The app's name alone (the default): a bell, then the name in up to two lines, and
///   "+N more" when others came with it.
/// - With the app's text allowed: an inverted band with a small bell and the name, the title,
///   then the body in up to four small lines.
pub fn draw_notice(p: &mut Panel, n: &Notice) {
    p.clear();
    let w = p.width as i32;
    let h = p.height as i32;
    let more = (n.more > 0).then(|| format!("+{}", n.more));
    if n.title.is_none() && n.body.is_none() {
        glyph(p, &BELL, (w - 11) / 2, 2, true);
        let name = wrap(&FONT_5X8, &n.app, w, 2);
        let top = if name.len() > 1 { 18 } else { 22 };
        for (i, line) in name.iter().enumerate() {
            centred(p, &FONT_5X8, line, top + 9 * i as i32);
        }
        if let Some(more) = more {
            centred(p, &FONT_4X6, &format!("{more} more"), h - 7);
        }
        return;
    }
    Rectangle::new(Point::new(0, 0), Size::new(w as u32, 10))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(p)
        .ok();
    glyph(p, &SMALL_BELL, 1, 1, false);
    let more_w = more.as_deref().map_or(0, |m| text_width(&FONT_4X6, m) + 1);
    let name = clipped(&FONT_5X8, &n.app, w - 10 - more_w);
    text_at(p, &FONT_5X8, &name, 10, 1, BinaryColor::Off);
    if let Some(more) = &more {
        text_at(p, &FONT_4X6, more, w - more_w + 1, 2, BinaryColor::Off);
    }
    let mut y = 12;
    if let Some(title) = n.title.as_deref().filter(|t| !t.trim().is_empty()) {
        text_at(
            p,
            &FONT_5X8,
            &clipped(&FONT_5X8, title.trim(), w),
            0,
            y,
            BinaryColor::On,
        );
        y += 10;
    }
    if let Some(body) = &n.body {
        let room = ((h - y) / 7).max(0) as usize;
        for line in wrap(&FONT_4X6, body, w, room) {
            text_at(p, &FONT_4X6, &line, 0, y, BinaryColor::On);
            y += 7;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #507: both notice forms stay inside the Corne's 64×52 panel, the name-only form shows no
    /// text but the name, and wrapping keeps to its width and line count.
    #[test]
    fn notices_fit_the_panel() {
        let ink = |p: &Panel| p.bits.iter().map(|b| b.count_ones()).sum::<u32>();
        let mut name_only = Panel::new(64, 52);
        draw_notice(
            &mut name_only,
            &Notice {
                app: "Microsoft Teams".into(),
                more: 2,
                ..Notice::default()
            },
        );
        let mut with_text = Panel::new(64, 52);
        draw_notice(
            &mut with_text,
            &Notice {
                app: "Discord".into(),
                title: Some("Raid tonight".into()),
                body: Some("Bring the good keyboard and a charger for the right half".into()),
                more: 0,
            },
        );
        assert!(ink(&name_only) > 0 && ink(&with_text) > 0);
        assert_ne!(name_only.bits, with_text.bits);
        // The name-only form's header row is clear: no band.
        assert!(name_only.bits[..name_only.stride].iter().all(|&b| b == 0));
        // The text form's band: its first row is all ink.
        assert!(with_text.bits[..with_text.stride]
            .iter()
            .all(|&b| b == 0xff));

        for (s, lines) in [
            ("Bring the good keyboard and a charger", 2),
            ("Supercalifragilisticexpialidocious", 3),
            ("", 2),
        ] {
            let out = wrap(&FONT_5X8, s, 64, lines);
            assert!(out.len() <= lines, "{s}: {out:?}");
            assert!(
                out.iter().all(|l| text_width(&FONT_5X8, l) <= 64),
                "{out:?}"
            );
        }
        assert_eq!(
            wrap(&FONT_5X8, "Bring the good keyboard and a charger", 64, 2),
            ["Bring the", "good keybo.."]
        );
        assert_eq!(wrap(&FONT_5X8, "Outlook", 64, 2), ["Outlook"]);
        assert_eq!(clipped(&FONT_5X8, "Microsoft Teams", 54), "Microsof..");
    }

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
            // en-CA and es-MX marks, too wide as written (#449 review)
            ("12:59", "p.m.", false, "Wed, Sep 30", "Sep 30"),
            ("12:59", "p.\u{a0}m.", false, "mié, 30 sep", "30 sep"),
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
        assert_eq!(fitting_mark("12:59", "p.m.", 64), "pm");
        assert_eq!(fitting_mark("2:05", "p.m.", 64), "p.m.");
        assert_eq!(fitting_mark("12:59", "nachm.", 64), "n");
    }
    /// The PC load layout (#506) fits the Corne's 64×52 panel: three rows, numbers inside the
    /// edge, and an unknown value draws `--` with an empty bar.
    #[test]
    fn stats_fit_the_panel() {
        use crate::stats::Stats;
        let ink = |p: &Panel| p.bits.iter().map(|b| b.count_ones()).sum::<u32>();
        let mut full = Panel::new(64, 52);
        draw_stats(
            &mut full,
            &Stats {
                cpu: Some(100),
                gpu: Some(100),
                ram: Some(100),
            },
        );
        let mut empty = Panel::new(64, 52);
        draw_stats(&mut empty, &Stats::default());
        let mut half = Panel::new(64, 52);
        draw_stats(
            &mut half,
            &Stats {
                cpu: Some(37),
                gpu: Some(5),
                ram: Some(62),
            },
        );
        assert!(ink(&full) > ink(&half) && ink(&half) > ink(&empty));
        // Nothing below the third bar: rows end at 3 × 17 - 2.
        let stride = full.stride;
        assert!(full.bits[51 * stride..].iter().all(|&b| b == 0));
        // A value change touches only its own row's lines (small panel writes).
        let mut cpu_up = Panel::new(64, 52);
        draw_stats(
            &mut cpu_up,
            &Stats {
                cpu: Some(38),
                ..Stats {
                    cpu: Some(37),
                    gpu: Some(5),
                    ram: Some(62),
                }
            },
        );
        let changed: Vec<usize> = (0..52)
            .filter(|y| {
                half.bits[y * stride..(y + 1) * stride] != cpu_up.bits[y * stride..(y + 1) * stride]
            })
            .collect();
        assert!(
            !changed.is_empty() && changed.iter().all(|y| *y < 17),
            "{changed:?}"
        );
    }
}
