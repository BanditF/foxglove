// SPDX-License-Identifier: MIT
//! The text **Copy diagnostics** puts on the clipboard (docs/hideout-supervisor-ux.md, ladder
//! step 4): the module, its state and step ids, exit codes, crash times, the ladder steps tried,
//! the hub's and the module's versions and the tail of `logs\<id>.log`. Plain text, at most
//! [`MAX_BYTES`]. Never secrets or tokens, never another process's pid, and no user path outside
//! the hub folder: the profile folder reads `%USERPROFILE%`, the user name `<user>`.
//!
//! No I/O but [`log_tail`]; the driver gathers the rest ([`crate::driver::Supervisor::diagnostics`]).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The most the text may be, in bytes (UTF-8).
pub const MAX_BYTES: usize = 16 * 1024;
/// Lines of the module's log at the end of the text.
pub const LOG_LINES: usize = 50;
/// The most of the log file read from its end.
const LOG_READ: u64 = 32 * 1024;

/// What the text is made of.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub module: String,
    pub name: String,
    pub version: Option<String>,
    pub hub_version: String,
    /// The §7.8 state id; `None` when nothing is wrong and it doesn't run.
    pub state: Option<String>,
    /// `module.<id>.recovery`, the step the card offers.
    pub recovery: Option<String>,
    /// The version Go back names.
    pub previous: Option<String>,
    pub crashes_in_window: usize,
    pub stops: u32,
    /// The live instance's pid: the one pid the text may show.
    pub pid: Option<u32>,
    /// The policy's history, oldest first, with wall-clock times.
    pub history: Vec<(SystemTime, String)>,
    pub log_tail: Option<String>,
    /// What to hide: (text, what it reads instead), longest first is not needed.
    pub redact: Vec<(String, String)>,
}

/// The diagnostics text.
pub fn report(i: &Input) -> String {
    let mut head = String::new();
    let line = |head: &mut String, k: &str, v: &str| {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push('\n');
    };
    line(&mut head, "Hideout diagnostics", &utc(SystemTime::now()));
    line(&mut head, "Hub version", &i.hub_version);
    line(&mut head, "Module", &format!("{} ({})", i.module, i.name));
    line(
        &mut head,
        "Module version",
        i.version.as_deref().unwrap_or("unknown"),
    );
    line(&mut head, "State", i.state.as_deref().unwrap_or("idle"));
    line(
        &mut head,
        "Recovery",
        i.recovery.as_deref().unwrap_or("none"),
    );
    if let Some(p) = &i.previous {
        line(&mut head, "Previous version", p);
    }
    line(
        &mut head,
        "Crashes in the last 10 min",
        &i.crashes_in_window.to_string(),
    );
    line(&mut head, "Crash stops this sign-in", &i.stops.to_string());
    if let Some(p) = i.pid {
        line(&mut head, "Pid", &p.to_string());
    }
    head.push_str("\nHistory (UTC, oldest first):\n");
    if i.history.is_empty() {
        head.push_str("  (none)\n");
    }
    for (at, what) in &i.history {
        head.push_str(&format!("  {}  {what}\n", utc(*at)));
    }
    let head = clean(&head, i);
    let title = format!(
        "\nLog (last {LOG_LINES} lines of logs\\{}.log):\n",
        i.module
    );
    let tail = match &i.log_tail {
        Some(t) if !t.trim().is_empty() => clean(t, i),
        Some(_) => "  (empty)\n".into(),
        None => "  (no log)\n".into(),
    };
    // The head always fits; the log loses its oldest lines first.
    let room = MAX_BYTES.saturating_sub(head.len() + title.len());
    let mut out = head;
    out.truncate(floor_char(&out, MAX_BYTES));
    if room > 0 {
        out.push_str(&title);
        out.push_str(&keep_end(&tail, room));
    }
    out
}

/// The last [`LOG_LINES`] lines of a log file, from at most its last 32 KiB. `None` without the
/// file.
pub fn log_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(LOG_READ);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // cut mid-line
    }
    let from = lines.len().saturating_sub(LOG_LINES);
    let mut out = lines[from..].join("\n");
    out.push('\n');
    Some(out)
}

/// Paths and names first, then secrets, then other processes' pids.
fn clean(text: &str, i: &Input) -> String {
    let mut t = text.to_string();
    for (from, to) in &i.redact {
        if from.len() >= 2 {
            t = replace_ci(&t, from, to);
            // The same path written with forward slashes.
            if from.contains('\\') {
                t = replace_ci(&t, &from.replace('\\', "/"), &to.replace('\\', "/"));
            }
        }
    }
    let t = redact_secrets(&t);
    redact_pids(&t, i.pid)
}

/// Words after which a value is a secret: `token=…`, `"token": "…"`, `Bearer …`.
const SECRET_WORDS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "authorization",
    "bearer",
];

/// Replaces the value after any [`SECRET_WORDS`] word with `<redacted>`.
pub fn redact_secrets(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'scan: while i < b.len() {
        for w in SECRET_WORDS {
            if lower[i..].starts_with(w) {
                let mut j = i + w.len();
                // The rest of the key (`token_id`, `HIDEOUT_TOKEN"`), then `:`/`=`/space, quotes.
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let key_end = j;
                // A header's value is the rest of its line (`Authorization: Bearer …`).
                let whole_line = matches!(*w, "authorization" | "bearer");
                let mut sep = false;
                while j < b.len() && matches!(b[j], b'"' | b'\'' | b' ' | b':' | b'=') {
                    sep |= matches!(b[j], b':' | b'=') || (b[j] == b' ' && *w == "bearer");
                    j += 1;
                }
                let v = j;
                while j < b.len()
                    && !matches!(b[j], b'\n' | b'\r')
                    && (whole_line
                        || !matches!(b[j], b'"' | b'\'' | b',' | b'}' | b' ' | b';' | b'&'))
                {
                    j += 1;
                }
                if sep && j > v {
                    out.push_str(&text[i..v]);
                    out.push_str("<redacted>");
                    i = j;
                } else {
                    out.push_str(&text[i..key_end]);
                    i = key_end;
                }
                continue 'scan;
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `pid 1234` / `pid=1234` / `pid: 1234` for any pid but `own` reads `pid <other>`.
fn redact_pids(text: &str, own: Option<u32>) -> String {
    let lower = text.to_ascii_lowercase();
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let word_start = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if word_start && lower[i..].starts_with("pid") {
            let mut j = i + 3;
            while j < b.len() && matches!(b[j], b' ' | b'=' | b':') {
                j += 1;
            }
            let d = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > d && (j == b.len() || !b[j].is_ascii_alphanumeric()) {
                let n: Option<u32> = text[d..j].parse().ok();
                out.push_str(&text[i..d]);
                if n.is_some() && n == own {
                    out.push_str(&text[d..j]);
                } else {
                    out.push_str("<other>");
                }
                i = j;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Case-insensitive (ASCII) replace; Windows paths and user names compare that way.
fn replace_ci(text: &str, from: &str, to: &str) -> String {
    let lt = text.to_ascii_lowercase();
    let lf = from.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut at = 0;
    while let Some(k) = lt[at..].find(&lf) {
        let k = at + k;
        out.push_str(&text[last..k]);
        out.push_str(to);
        last = k + from.len();
        at = last;
    }
    out.push_str(&text[last..]);
    out
}

fn floor_char(s: &str, max: usize) -> usize {
    let mut n = max.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    n
}

/// The end of `s` within `max` bytes, cut at a line start.
fn keep_end(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.into();
    }
    let mut from = s.len() - max;
    while !s.is_char_boundary(from) {
        from += 1;
    }
    match s[from..].find('\n') {
        Some(n) => s[from + n + 1..].into(),
        None => String::new(),
    }
}

/// `2026-11-10T14:03:07Z`.
pub fn utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Input {
        let t0 = UNIX_EPOCH + Duration::from_secs(1_794_312_000); // 2026-11-10T12:00:00Z
        Input {
            module: "m.art".into(),
            name: "Art".into(),
            version: Some("0.5.0".into()),
            hub_version: "0.1.0".into(),
            state: Some("crash-stopped".into()),
            recovery: Some("defaults".into()),
            previous: Some("0.4.2".into()),
            crashes_in_window: 0,
            stops: 2,
            pid: Some(4242),
            history: vec![
                (t0, "started instance 7".into()),
                (
                    t0 + Duration::from_secs(3),
                    "exited with code 3221225477 (0xC0000005)".into(),
                ),
                (t0 + Duration::from_secs(3), "crash 5 of 5 in 10 min".into()),
                (
                    t0 + Duration::from_secs(60),
                    "step tried: start-again".into(),
                ),
            ],
            log_tail: Some(
                "starting in C:\\Users\\Fuchs\\AppData\\Local\\Hideout\\modules\\m.art\n\
                 pipe \\\\.\\pipe\\hideout-fuchs ok; HIDEOUT_TOKEN=abc123 set\n\
                 {\"token\": \"s3cr3t\", \"n\": 1}\n\
                 Authorization: Bearer xyz.789\n\
                 pid 4242 runs; parent pid 999 gone; pid=123\n\
                 read c:/users/fuchs/Documents/notes.txt\n"
                    .into(),
            ),
            redact: vec![
                ("C:\\Users\\fuchs".into(), "%USERPROFILE%".into()),
                ("fuchs".into(), "<user>".into()),
            ],
        }
    }

    #[test]
    fn the_text_has_what_a_bug_report_needs_and_no_secrets() {
        let text = report(&sample());
        for want in [
            "Hub version: 0.1.0",
            "Module: m.art (Art)",
            "Module version: 0.5.0",
            "State: crash-stopped",
            "Recovery: defaults",
            "Previous version: 0.4.2",
            "Crash stops this sign-in: 2",
            "2026-11-10T12:00:03Z  exited with code 3221225477 (0xC0000005)",
            "2026-11-10T12:01:00Z  step tried: start-again",
            "Log (last 50 lines of logs\\m.art.log):",
            "%USERPROFILE%\\AppData\\Local\\Hideout\\modules\\m.art",
            "hideout-<user>",
            "pid 4242 runs; parent pid <other> gone; pid=<other>",
            "%USERPROFILE%/Documents/notes.txt",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
        for never in [
            "fuchs", "Fuchs", "abc123", "s3cr3t", "xyz.789", "999", "=123",
        ] {
            assert!(!text.contains(never), "{never:?} leaked:\n{text}");
        }
        assert!(text.contains("HIDEOUT_TOKEN=<redacted>"), "{text}");
        if let Ok(dir) = std::env::var("HIDEOUT_REVIEW_DIR") {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(Path::new(&dir).join("copy-diagnostics-sample.txt"), &text);
        }
    }

    #[test]
    fn the_text_stays_under_16_kb_and_keeps_the_newest_log_lines() {
        let mut i = sample();
        let long: String = (0..2000)
            .map(|n| format!("line {n} {}\n", "x".repeat(40)))
            .collect();
        i.log_tail = Some(long);
        i.history = (0..32)
            .map(|n| (UNIX_EPOCH, format!("exited with code {n}")))
            .collect();
        let text = report(&i);
        assert!(text.len() <= MAX_BYTES, "{} bytes", text.len());
        assert!(text.contains("line 1999 "), "the newest line went");
        assert!(!text.contains("line 0 "), "the oldest line stayed");
        assert!(text.contains("Hub version"));
    }

    #[test]
    fn the_log_tail_is_the_last_50_lines() {
        let dir = std::env::temp_dir().join(format!("hideout-diag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("m.log");
        let text: String = (0..80).map(|n| format!("l{n}\n")).collect();
        std::fs::write(&f, text).unwrap();
        let tail = log_tail(&f).unwrap();
        assert_eq!(tail.lines().count(), LOG_LINES);
        assert!(tail.starts_with("l30\n") && tail.ends_with("l79\n"));
        assert!(log_tail(&dir.join("none.log")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_words_without_a_value_stay() {
        assert_eq!(redact_secrets("no token here"), "no token here");
        assert_eq!(redact_secrets("tokens"), "tokens");
        assert_eq!(redact_secrets("a=1"), "a=1");
    }

    #[test]
    fn utc_reads_a_known_day() {
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
    }
}
