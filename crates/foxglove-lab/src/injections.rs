//! The harness's own input (#618), shared with `scripts/lab/labkit.py`.
//!
//! A key press through the keyboard (D7 05 positions, INVOKE, the typing sink) is input to
//! Windows like the user's, and GetLastInputInfo can't tell them apart, so the "the user is
//! working" gate (PC idle for 2 minutes before a soak or a typing step) stopped the lab's own
//! later steps. Each injection is appended to one file (`FOXGLOVE_LAB_INJECTIONS`, or
//! `%LOCALAPPDATA%\Foxglove\lab-injections.log`), one line each:
//!
//! ```text
//! <start> <end> <the user's last input before it, or -> <what>     (Unix seconds)
//! ```
//!
//! The rule: input from an injection's start (less [`BEFORE`]) to [`AFTER`] past its end is the
//! harness's. When the PC's last input falls in such a window, the user's last input is the one
//! recorded when that injection began. Any other input, before or after, is the user's and still
//! blocks. labkit's `user_idle` reads the file the same way.
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Input this long after an injection ends is still the harness's.
pub const AFTER: f64 = 2.0;
/// ... and this long before it starts (clock granularity).
pub const BEFORE: f64 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub start: f64,
    pub end: f64,
    /// The user's last input when the injection began (None when Windows couldn't say).
    pub user_last: Option<f64>,
    pub what: String,
}

pub fn path() -> PathBuf {
    if let Some(p) = std::env::var_os("FOXGLOVE_LAB_INJECTIONS").filter(|p| !p.is_empty()) {
        return p.into();
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Foxglove")
        .join("lab-injections.log")
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

pub fn line(r: &Record) -> String {
    let before = r
        .user_last
        .map_or_else(|| "-".to_string(), |t| format!("{t:.3}"));
    format!(
        "{:.3} {:.3} {before} {}\n",
        r.start,
        r.end,
        r.what.replace('\n', " ")
    )
}

/// The records in `text`, oldest first (bad lines skipped).
pub fn parse(text: &str) -> Vec<Record> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(4, ' ');
            let start = parts.next()?.parse().ok()?;
            let end = parts.next()?.parse().ok()?;
            let user_last = match parts.next()? {
                "-" => None,
                t => Some(t.parse().ok()?),
            };
            Some(Record {
                start,
                end,
                user_last,
                what: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// The user's last input given the PC's (`raw`), by the rule in the module docs.
pub fn user_last(raw: Option<f64>, records: &[Record]) -> Option<f64> {
    let raw = raw?;
    match records
        .iter()
        .rev()
        .find(|r| r.start - BEFORE <= raw && raw <= r.end + AFTER)
    {
        Some(r) => r.user_last,
        None => Some(raw),
    }
}

/// When this PC last had keyboard or mouse input (Unix seconds), or None.
#[cfg(windows)]
pub fn last_input() -> Option<f64> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a valid, sized LASTINPUTINFO for the call to fill.
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return None;
    }
    // SAFETY: no arguments.
    let ago = unsafe { GetTickCount() }.wrapping_sub(info.dwTime);
    Some(now() - f64::from(ago) / 1000.0)
}
#[cfg(not(windows))]
pub fn last_input() -> Option<f64> {
    None
}

/// An injection in progress: recorded when dropped (after the input was made, also on an error).
pub struct Injection {
    start: f64,
    user_last: Option<f64>,
    what: String,
    path: PathBuf,
}

/// Starts recording an injection described by `what` in the shared file.
pub fn begin(what: impl Into<String>) -> Injection {
    begin_at(path(), what)
}

/// Starts recording an injection described by `what` in `path`.
pub fn begin_at(path: PathBuf, what: impl Into<String>) -> Injection {
    let records = std::fs::read_to_string(&path)
        .map(|t| parse(&t))
        .unwrap_or_default();
    Injection {
        user_last: user_last(last_input(), &records),
        start: now(),
        what: what.into(),
        path,
    }
}

impl Drop for Injection {
    fn drop(&mut self) {
        let r = Record {
            start: self.start,
            end: now(),
            user_last: self.user_last,
            what: std::mem::take(&mut self.what),
        };
        // Best effort: a missing record only makes the gate more cautious.
        let _ = self.path.parent().map(std::fs::create_dir_all);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = f.write_all(line(&r).as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(start: f64, end: f64, user_last: Option<f64>) -> Record {
        Record {
            start,
            end,
            user_last,
            what: "position 11 tap".into(),
        }
    }

    #[test]
    fn records_round_trip_through_the_shared_format() {
        let r = rec(100.0, 100.25, Some(40.5));
        assert_eq!(line(&r), "100.000 100.250 40.500 position 11 tap\n");
        assert_eq!(parse(&line(&r)), vec![r]);
        let unknown = rec(1.0, 2.0, None);
        assert_eq!(parse(&format!("junk\n{}", line(&unknown))), vec![unknown]);
    }

    #[test]
    fn input_in_an_injection_window_is_the_harness_and_other_input_the_user() {
        let records = [rec(100.0, 101.0, Some(40.0)), rec(200.0, 200.5, Some(40.0))];
        // During, and up to 2 s after: the harness's; the user's last input is from before.
        assert_eq!(user_last(Some(100.5), &records), Some(40.0));
        assert_eq!(user_last(Some(202.4), &records), Some(40.0));
        // Later than that, or between injections: the user's own.
        assert_eq!(user_last(Some(202.6), &records), Some(202.6));
        assert_eq!(user_last(Some(150.0), &records), Some(150.0));
        assert_eq!(user_last(None, &records), None);
    }
}
