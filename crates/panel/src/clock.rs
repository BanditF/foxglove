//! The clock and date in the user's own Windows formats (#445): 12- or 24-hour, the date order
//! and the language all come from the region settings.
//!
//! The pictures are the user's short time (`LOCALE_SSHORTTIME`) and long date
//! (`LOCALE_SLONGDATE`), cut down to what fits on the panel: no seconds, no year, abbreviated
//! day and month names. `GetTimeFormatEx` / `GetDateFormatEx` turn them into text. They are read
//! once and again after `WM_SETTINGCHANGE` "intl" ([`invalidate`], called from `changes.rs`),
//! never on a timer.
//!
//! The panel's fonts are Latin-1, so text outside it falls back: the AM/PM mark to "AM"/"PM",
//! the date to the numeric short date without the year.

use std::sync::Mutex;

/// The clock's text for one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Texts {
    /// The time without its AM/PM mark, e.g. "2:05" or "14:05".
    pub time: String,
    /// The AM/PM mark ("" on a 24-hour clock) and whether it goes before the time.
    pub mark: String,
    pub mark_first: bool,
    /// The date with the weekday, e.g. "Mon, Oct 26" or "Mo., 26. Okt.".
    pub date: String,
    /// A shorter date for when `date` doesn't fit, e.g. "Oct 26" or "26. Okt.".
    pub date_short: String,
}

/// The pictures for one locale, already cut down for the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pictures {
    /// The time picture without the AM/PM mark.
    pub time: String,
    /// `Some(first)` on a 12-hour clock: whether the mark goes before the time.
    pub mark: Option<bool>,
    pub date: String,
    pub date_short: String,
    pub date_numeric: String,
}

impl Pictures {
    /// From the locale's short time, long date and short date pictures.
    pub fn new(short_time: &str, long_date: &str, short_date: &str) -> Self {
        let mut time = tokens(short_time);
        remove_fields(&mut time, |c, _| c == 's');
        let mark = time
            .iter()
            .position(|t| matches!(t, Tok::Field('t', _)))
            .map(|i| !time[..i].iter().any(|t| matches!(t, Tok::Field(..))));
        remove_fields(&mut time, |c, _| c == 't');

        let mut long = tokens(long_date);
        remove_fields(&mut long, |c, _| c == 'y' || c == 'g');
        for t in &mut long {
            if let Tok::Field('d' | 'M', n) = t {
                *n = (*n).min(3);
            }
        }
        if !long.contains(&Tok::Field('d', 3)) {
            long.insert(0, Tok::Lit(" ".into()));
            long.insert(0, Tok::Field('d', 3));
        }
        let mut short = long.clone();
        remove_fields(&mut short, |c, n| c == 'd' && n >= 3);
        let mut numeric = tokens(short_date);
        remove_fields(&mut numeric, |c, _| c == 'y' || c == 'g');

        Pictures {
            time: picture(&time),
            mark,
            date: picture(&long),
            date_short: picture(&short),
            date_numeric: picture(&numeric),
        }
    }
}

/// Whether the panel's Latin-1 fonts can draw all of `s`.
pub fn latin1(s: &str) -> bool {
    s.chars().all(|c| u32::from(c) <= 0xFF)
}

/// Picks the texts the panel can draw, given each picture already formatted.
pub fn choose(p: &Pictures, format: impl Fn(&str) -> String, hour: u32) -> Texts {
    let time = format(&p.time);
    let time = if latin1(&time) && !time.trim().is_empty() {
        time.trim().to_string()
    } else if p.mark.is_some() {
        format("h:mm")
    } else {
        format("HH:mm")
    };
    let mark = match p.mark {
        None => String::new(),
        Some(_) => {
            let m = format("tt");
            if latin1(&m) && !m.trim().is_empty() {
                m.trim().to_string()
            } else if hour < 12 {
                "AM".into()
            } else {
                "PM".into()
            }
        }
    };
    let numeric = format(&p.date_numeric);
    let numeric = if latin1(&numeric) {
        numeric
    } else {
        // Some short dates keep non-Latin separators; digits with '/' always draw.
        let mut q = tokens(&p.date_numeric);
        q.retain(|t| matches!(t, Tok::Field(..)));
        let q: Vec<Tok> = q
            .into_iter()
            .flat_map(|t| [Tok::Lit("/".into()), t])
            .skip(1)
            .collect();
        format(&picture(&q))
    };
    let date = format(&p.date);
    let (date, date_short) = if latin1(&date) && !date.trim().is_empty() {
        let short = (!p.date_short.is_empty())
            .then(|| format(&p.date_short))
            .filter(|s| latin1(s))
            .unwrap_or_else(|| numeric.clone());
        (date, short)
    } else {
        (numeric.clone(), numeric)
    };
    Texts {
        time,
        mark,
        mark_first: p.mark == Some(true),
        date: date.trim().to_string(),
        date_short: date_short.trim().to_string(),
    }
}

/// One piece of a Windows date or time picture.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    /// A run of one field letter, e.g. ('M', 3) for "MMM".
    Field(char, usize),
    /// Literal text (quoted or plain).
    Lit(String),
}

const FIELDS: &str = "hHmstdMyg";

fn tokens(p: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    let mut chars = p.chars().peekable();
    let lit = |out: &mut Vec<Tok>, c: char| match out.last_mut() {
        Some(Tok::Lit(s)) => s.push(c),
        _ => out.push(Tok::Lit(c.to_string())),
    };
    while let Some(c) = chars.next() {
        if c == '\'' {
            // Quoted literal; '' inside quotes is a quote.
            while let Some(q) = chars.next() {
                if q == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                        lit(&mut out, '\'');
                    } else {
                        break;
                    }
                } else {
                    lit(&mut out, q);
                }
            }
        } else if FIELDS.contains(c) {
            let mut n = 1;
            while chars.peek() == Some(&c) {
                chars.next();
                n += 1;
            }
            out.push(Tok::Field(c, n));
        } else {
            lit(&mut out, c);
        }
    }
    out
}

/// Removes the matching fields with the literal that joined each to the rest.
/// The separator goes with it: the punctuation before it, else after it. A word after a year
/// ("2026. gada", "2026年") is the year's own and goes too; other words stay.
fn remove_fields(toks: &mut Vec<Tok>, drop: impl Fn(char, usize) -> bool) {
    let wordy =
        |t: Option<&Tok>| matches!(t, Some(Tok::Lit(s)) if s.chars().any(char::is_alphabetic));
    let plain =
        |t: Option<&Tok>| matches!(t, Some(Tok::Lit(s)) if !s.chars().any(char::is_alphabetic));
    while let Some(i) = toks
        .iter()
        .position(|t| matches!(t, Tok::Field(c, n) if drop(*c, *n)))
    {
        let year = matches!(toks[i], Tok::Field('y' | 'g', _));
        toks.remove(i);
        if year && wordy(toks.get(i)) {
            toks.remove(i);
        } else if i > 0 && (year || plain(toks.get(i - 1))) {
            toks.remove(i - 1);
        } else if plain(toks.get(i)) {
            toks.remove(i);
        }
    }
    // A literal left at either end is a dangling separator.
    while matches!(toks.first(), Some(Tok::Lit(s)) if s.trim().is_empty()) {
        toks.remove(0);
    }
    while matches!(toks.last(), Some(Tok::Lit(s)) if s.trim().is_empty() || is_separator(s)) {
        toks.pop();
    }
}

fn is_separator(s: &str) -> bool {
    s.trim()
        .chars()
        .all(|c| matches!(c, ',' | '/' | '-' | ':' | ';'))
}

fn picture(toks: &[Tok]) -> String {
    let mut s = String::new();
    for t in toks {
        match t {
            Tok::Field(c, n) => s.extend(std::iter::repeat_n(*c, *n)),
            Tok::Lit(l) if l.chars().any(|c| c.is_alphabetic() || c == '\'') => {
                s.push('\'');
                s.push_str(&l.replace('\'', "''"));
                s.push('\'');
            }
            Tok::Lit(l) => s.push_str(l),
        }
    }
    s
}

static CACHE: Mutex<Option<Pictures>> = Mutex::new(None);

/// Forget the cached pictures; the next [`texts`] reads the region settings again. Called on
/// `WM_SETTINGCHANGE` "intl".
pub fn invalidate() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The clock's texts for `now` in the user's formats.
pub fn texts(now: &chrono::DateTime<chrono::Local>) -> Texts {
    let pictures = {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.get_or_insert_with(imp::user_pictures).clone()
    };
    use chrono::Timelike;
    choose(&pictures, |p| imp::format(None, p, now), now.hour())
}

#[cfg(windows)]
mod imp {
    use super::Pictures;
    use chrono::{Datelike, Timelike};
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::Globalization::{
        GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, LOCALE_SLONGDATE, LOCALE_SSHORTDATE,
        LOCALE_SSHORTTIME, TIME_FORMAT_FLAGS,
    };

    fn locale_name(locale: Option<&str>) -> Option<HSTRING> {
        locale.map(HSTRING::from)
    }

    fn pcwstr(h: &Option<HSTRING>) -> PCWSTR {
        // LOCALE_NAME_USER_DEFAULT is NULL.
        h.as_ref().map_or(PCWSTR::null(), |h| PCWSTR(h.as_ptr()))
    }

    pub fn info(locale: Option<&str>, kind: u32) -> String {
        let name = locale_name(locale);
        let mut buf = [0u16; 128];
        // SAFETY: the locale name lives in `name` for the call; the buffer is ours and its length
        // is passed with it.
        let n = unsafe { GetLocaleInfoEx(pcwstr(&name), kind, Some(&mut buf)) };
        let n = usize::try_from(n).unwrap_or(0).saturating_sub(1);
        String::from_utf16_lossy(&buf[..n.min(buf.len())])
    }

    pub fn pictures(locale: Option<&str>) -> Pictures {
        let (time, long, short) = (
            info(locale, LOCALE_SSHORTTIME),
            info(locale, LOCALE_SLONGDATE),
            info(locale, LOCALE_SSHORTDATE),
        );
        if time.is_empty() || long.is_empty() || short.is_empty() {
            eprintln!(
                "Foxglove: the region's date and time formats are unavailable; using defaults"
            );
            return Pictures::new("HH:mm", "ddd d MMM", "dd/MM");
        }
        Pictures::new(&time, &long, &short)
    }

    pub fn user_pictures() -> Pictures {
        pictures(None)
    }

    /// Formats one picture, a time one with GetTimeFormatEx or a date one with GetDateFormatEx
    /// (every picture here is one or the other).
    pub fn format(
        locale: Option<&str>,
        picture: &str,
        now: &chrono::DateTime<chrono::Local>,
    ) -> String {
        let st = SYSTEMTIME {
            wYear: now.year().clamp(1601, 30827) as u16,
            wMonth: now.month() as u16,
            wDayOfWeek: now.weekday().num_days_from_sunday() as u16,
            wDay: now.day() as u16,
            wHour: now.hour() as u16,
            wMinute: now.minute() as u16,
            wSecond: now.second().min(59) as u16,
            wMilliseconds: 0,
        };
        let name = locale_name(locale);
        let pic = HSTRING::from(picture);
        let mut buf = [0u16; 128];
        let is_time = super::tokens(picture)
            .iter()
            .any(|t| matches!(t, super::Tok::Field('h' | 'H' | 'm' | 't', _)));
        // SAFETY: the locale name, picture and SYSTEMTIME outlive the call; the buffer is ours
        // and its length is passed with it.
        let n = unsafe {
            if is_time {
                GetTimeFormatEx(
                    pcwstr(&name),
                    TIME_FORMAT_FLAGS(0),
                    Some(&st),
                    &pic,
                    Some(&mut buf),
                )
            } else {
                GetDateFormatEx(
                    pcwstr(&name),
                    Default::default(),
                    Some(&st),
                    &pic,
                    Some(&mut buf),
                    PCWSTR::null(),
                )
            }
        };
        let n = usize::try_from(n).unwrap_or(0).saturating_sub(1);
        String::from_utf16_lossy(&buf[..n.min(buf.len())])
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Pictures;

    pub fn user_pictures() -> Pictures {
        Pictures::new("HH:mm", "ddd d MMM", "dd/MM")
    }

    /// Without Windows: the pictures' fields through chrono, in English.
    pub fn format(
        _locale: Option<&str>,
        picture: &str,
        now: &chrono::DateTime<chrono::Local>,
    ) -> String {
        let mut out = String::new();
        for t in super::tokens(picture) {
            match t {
                super::Tok::Lit(l) => out.push_str(&l),
                super::Tok::Field(c, n) => {
                    let f = match (c, n) {
                        ('H', 1) => "%-H",
                        ('H', _) => "%H",
                        ('h', 1) => "%-I",
                        ('h', _) => "%I",
                        ('m', _) => "%M",
                        ('t', _) => "%p",
                        ('d', 1) => "%-d",
                        ('d', 2) => "%d",
                        ('d', _) => "%a",
                        ('M', 1) => "%-m",
                        ('M', 2) => "%m",
                        ('M', _) => "%b",
                        _ => "",
                    };
                    out.push_str(&now.format(f).to_string());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_drop_seconds_years_and_long_names() {
        // en-US
        let p = Pictures::new("h:mm tt", "dddd, MMMM d, yyyy", "M/d/yyyy");
        assert_eq!(p.time, "h:mm");
        assert_eq!(p.mark, Some(false));
        assert_eq!(p.date, "ddd, MMM d");
        assert_eq!(p.date_short, "MMM d");
        assert_eq!(p.date_numeric, "M/d");
        // de-DE: 24-hour, day before month, no AM/PM
        let p = Pictures::new("HH:mm", "dddd, d. MMMM yyyy", "dd.MM.yyyy");
        assert_eq!((p.time.as_str(), p.mark), ("HH:mm", None));
        assert_eq!(p.date, "ddd, d. MMM");
        assert_eq!(p.date_short, "d. MMM");
        assert_eq!(p.date_numeric, "dd.MM");
        // en-GB: no weekday in the long date, so one is added
        let p = Pictures::new("HH:mm:ss", "dd MMMM yyyy", "dd/MM/yyyy");
        assert_eq!(p.time, "HH:mm");
        assert_eq!(p.date, "ddd dd MMM");
        assert_eq!(p.date_short, "dd MMM");
        // ko-KR / zh-CN style: the mark first, quoted literals, year first
        let p = Pictures::new("tt h:mm", "yyyy'년' M'월' d'일' dddd", "yyyy-MM-dd");
        assert_eq!((p.time.as_str(), p.mark), ("h:mm", Some(true)));
        assert_eq!(p.date, "M'월 'd'일 'ddd");
        assert_eq!(p.date_numeric, "MM-dd");
        // lv-LV: the year's own word goes with it
        let p = Pictures::new("HH:mm", "dddd, yyyy. 'gada' d. MMMM", "dd.MM.yyyy");
        assert_eq!(p.date, "ddd, d. MMM");
        assert_eq!(p.date_short, "d. MMM");
        // es-ES / pt-BR: the " de " before the year goes with it
        let p = Pictures::new("H:mm", "dddd, d' de 'MMMM' de 'yyyy", "dd/MM/yyyy");
        assert_eq!(p.date, "ddd, d' de 'MMM");
        assert_eq!(p.date_short, "d' de 'MMM");
    }

    #[test]
    fn quotes_round_trip() {
        let t = tokens("d 'de' MMMM 'o''clock'");
        assert_eq!(picture(&t), "d' de 'MMMM' o''clock'");
    }

    /// A stand-in formatter: field letters become fixed sample text.
    fn fake<'a>(sample: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> String + 'a {
        move |p| {
            let mut out = String::new();
            for t in tokens(p) {
                match t {
                    Tok::Lit(l) => out.push_str(&l),
                    Tok::Field(c, n) => {
                        let key: String = std::iter::repeat_n(c, n).collect();
                        let v = sample
                            .iter()
                            .find(|(k, _)| *k == key)
                            .map_or("?", |(_, v)| v);
                        out.push_str(v);
                    }
                }
            }
            out
        }
    }

    #[test]
    fn non_latin_text_falls_back_to_what_the_fonts_draw() {
        let p = Pictures::new("tt h:mm", "yyyy'年'M'月'd'日' dddd", "yyyy/M/d");
        let f = fake(&[
            ("tt", "下午"),
            ("h", "2"),
            ("mm", "05"),
            ("M", "10"),
            ("d", "26"),
            ("ddd", "周一"),
        ]);
        let t = choose(&p, f, 14);
        assert_eq!((t.time.as_str(), t.mark.as_str()), ("2:05", "PM"));
        assert!(t.mark_first);
        assert_eq!((t.date.as_str(), t.date_short.as_str()), ("10/26", "10/26"));
    }

    #[cfg(windows)]
    fn sample() -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(2026, 10, 26, 14, 5, 0)
            .single()
            .unwrap()
    }

    /// The real Windows formatter, for fixed locales (Windows ships their data).
    #[cfg(windows)]
    fn windows_texts(locale: &str) -> Texts {
        let p = imp::pictures(Some(locale));
        choose(&p, |pic| imp::format(Some(locale), pic, &sample()), 14)
    }

    #[cfg(windows)]
    #[test]
    fn windows_formats_12_hour_24_hour_and_german() {
        let us = windows_texts("en-US");
        assert_eq!((us.time.as_str(), us.mark.as_str()), ("2:05", "PM"));
        assert!(!us.mark_first);
        assert_eq!(us.date, "Mon, Oct 26");
        let gb = windows_texts("en-GB");
        assert_eq!((gb.time.as_str(), gb.mark.as_str()), ("14:05", ""));
        assert_eq!(gb.date, "Mon 26 Oct");
        let de = windows_texts("de-DE");
        assert_eq!((de.time.as_str(), de.mark.as_str()), ("14:05", ""));
        assert!(de.date.starts_with("Mo"), "{}", de.date);
        assert!(de.date.contains("26. Okt"), "{}", de.date);
        assert_eq!(de.date_short, "26. Okt");
        let ja = windows_texts("ja-JP");
        assert!(latin1(&ja.date) && latin1(&ja.time) && latin1(&ja.mark));
        assert_eq!(ja.date, "10/26");
    }
}
