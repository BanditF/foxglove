//! Foxglove (#370): the app's log file. Foxglove is a Windows GUI program, so its
//! `eprintln!("Foxglove: …")` lines and panic messages went nowhere in a normal run.
//!
//! At startup the process's stderr becomes a pipe; a reader thread blocks on it and writes each
//! line, with a UTC timestamp, to `logs\foxglove.log` in the settings folder (next to
//! `foxglove.toml`, or inside `FOXGLOVE_CONFIG_DIR`). At 1 MB the file rolls to `foxglove.1.log`,
//! so the two together stay under about 2 MB. A console run still sees its lines too. Panics are
//! written straight to the file by a hook, so a panic that ends the process isn't lost in the
//! pipe. Values that look like secrets (`token=…`, `password: …`) are masked before writing.
//!
//! No timers: the reader wakes only when there is something to write.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One file rolls at this size; two files at most.
pub const MAX_BYTES: u64 = 1_000_000;
const FILE: &str = "foxglove.log";
const OLD: &str = "foxglove.1.log";

/// The log folder: `logs` in the settings folder.
pub fn dir() -> Option<PathBuf> {
    Some(super::store::path()?.parent()?.join("logs"))
}

/// An open log: the current file and its size.
pub struct Log {
    dir: PathBuf,
    file: Option<File>,
    size: u64,
}

impl Log {
    pub fn open(dir: &Path) -> std::io::Result<Log> {
        std::fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(FILE))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Log {
            dir: dir.to_path_buf(),
            file: Some(file),
            size,
        })
    }

    /// Writes one line, masked and timestamped, rolling the file first if it would pass
    /// [`MAX_BYTES`]. Errors are dropped: the log must never take the app down.
    pub fn write_line(&mut self, unix_secs: u64, text: &str) {
        let line = format!("{} {}\n", timestamp(unix_secs), redact(text.trim_end()));
        if self.size > 0 && self.size + line.len() as u64 > MAX_BYTES {
            self.roll();
        }
        if let Some(f) = &mut self.file {
            if f.write_all(line.as_bytes()).is_ok() {
                let _ = f.flush();
                self.size += line.len() as u64;
            }
        }
    }

    fn roll(&mut self) {
        self.file = None; // closed before the rename
        let _ = std::fs::remove_file(self.dir.join(OLD));
        let _ = std::fs::rename(self.dir.join(FILE), self.dir.join(OLD));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(FILE))
            .ok();
        self.size = 0;
    }
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Writes a line to the log, if it's open.
pub fn line(text: &str) {
    if let Some(log) = LOG.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        log.write_line(now(), text);
    }
}

/// The last `n` lines of the log (the rolled file first), for "Copy diagnostics" (#371).
#[allow(dead_code)]
pub fn recent(n: usize) -> Vec<String> {
    dir().map(|d| recent_in(&d, n)).unwrap_or_default()
}

fn recent_in(dir: &Path, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for name in [OLD, FILE] {
        if let Ok(f) = File::open(dir.join(name)) {
            lines.extend(std::io::BufReader::new(f).lines().map_while(Result::ok));
        }
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

/// `2026-10-25 14:03:07Z` for a unix time (UTC; civil-from-days, Howard Hinnant's algorithm).
fn timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Words that name a secret; the value after one (`=`, `:` or a space) is masked.
const SECRET_WORDS: &[&str] = &[
    "token",
    "password",
    "passwd",
    "secret",
    "api_key",
    "apikey",
    "authorization",
    "bearer",
    "credential",
];

/// Masks the value after a secret word and its `=` or `:` (`Bearer` takes a space), up to the
/// next space, comma or quote. `Authorization` masks the rest of the line.
pub fn redact(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let hit = SECRET_WORDS.iter().find(|w| {
            lower[i..].starts_with(*w) && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        });
        let Some(word) = hit else {
            let c = text[i..].chars().next().expect("in bounds");
            out.push(c);
            i += c.len_utf8();
            continue;
        };
        let mut j = i + word.len();
        // The rest of a longer word (`secret_key`, `tokens`).
        while j < text.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        while j < text.len() && matches!(bytes[j], b'"' | b'\'') {
            j += 1;
        }
        let separated = j < text.len()
            && (matches!(bytes[j], b'=' | b':') || (*word == "bearer" && bytes[j] == b' '));
        if !separated {
            out.push_str(&text[i..j]);
            i = j;
            continue;
        }
        j += 1;
        while j < text.len() && matches!(bytes[j], b' ' | b'"' | b'\'') {
            j += 1;
        }
        out.push_str(&text[i..j]);
        if *word == "authorization" {
            out.push_str("***");
            break;
        }
        let value_start = j;
        while j < text.len() && !matches!(bytes[j], b' ' | b',' | b'"' | b'\'' | b';' | b')') {
            j += 1;
        }
        out.push_str(if j > value_start { "***" } else { "" });
        i = j;
    }
    out
}

/// The first line of a run: the version, the pid, how long after boot (sign-in starts read a
/// minute or two, a later manual start more), and any arguments (the updater's helper).
fn started_line(pid: u32, since_boot: Option<u64>, args: &[String]) -> String {
    let mut text = format!("Foxglove {} started (pid {pid}", env!("CARGO_PKG_VERSION"));
    if let Some(s) = since_boot {
        text.push_str(&format!(", {s} s after boot"));
    }
    text.push(')');
    if !args.is_empty() {
        text.push_str(&format!(" with {}", args.join(" ")));
    }
    text
}

/// A line that must reach the file before the process exits right after it (a second launch,
/// a failed start): written at once, not through the stderr pipe, whose reader may not get to it.
/// Without a log (or capture) it goes to stderr as usual.
pub fn early(text: &str) {
    line(text);
    if !imp::capturing() {
        eprintln!("{text}");
    }
}

/// Opens the log and points stderr and panics at it. Call once, first in `main`.
pub fn start() {
    let Some(dir) = dir() else { return };
    match Log::open(&dir) {
        Ok(log) => *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(log),
        Err(_) => return, // no log rather than no Foxglove
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    line(&started_line(
        std::process::id(),
        imp::since_boot_secs(),
        &args,
    ));
    let console = imp::capture_stderr();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let at = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let text = format!(
            "Foxglove: panic in {}: {payload}{at}",
            thread.name().unwrap_or("a thread")
        );
        // try_lock: a panic inside a log write must not deadlock on the log's own lock.
        match LOG.try_lock() {
            Ok(mut guard) => {
                if let Some(log) = guard.as_mut() {
                    log.write_line(now(), &text);
                }
            }
            Err(std::sync::TryLockError::Poisoned(p)) => {
                if let Some(log) = p.into_inner().as_mut() {
                    log.write_line(now(), &text);
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        // A console run (or no capture) still gets the usual message on the real stderr.
        if console || !imp::capturing() {
            previous(info);
        }
    }));
}

/// Opens the log folder in Explorer (tray menu).
pub fn open_folder() {
    let Some(dir) = dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let null = std::process::Stdio::null;
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    if let Err(e) = std::process::Command::new(opener)
        .arg(&dir)
        .stdin(null())
        .stdout(null())
        .stderr(null())
        .spawn()
    {
        eprintln!("Foxglove: can't open the log folder: {e}");
    }
}

#[cfg(windows)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::mem::ManuallyDrop;
    use std::os::windows::io::FromRawHandle;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};
    use windows::Win32::System::Pipes::CreatePipe;

    static CAPTURING: AtomicBool = AtomicBool::new(false);

    pub fn capturing() -> bool {
        CAPTURING.load(Ordering::Relaxed)
    }

    /// Seconds since Windows started.
    pub fn since_boot_secs() -> Option<u64> {
        // SAFETY: no arguments; reads the tick count.
        Some(unsafe { windows::Win32::System::SystemInformation::GetTickCount64() } / 1000)
    }

    /// Points stderr at a pipe drained into the log. True when there was a real stderr (a
    /// console run), which keeps getting each line too.
    pub fn capture_stderr() -> bool {
        // SAFETY: reads the process's stderr handle; null or invalid in a GUI run.
        let original = unsafe { GetStdHandle(STD_ERROR_HANDLE) }.unwrap_or_default();
        let console = !original.is_invalid() && !original.0.is_null();
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        // SAFETY: two out-pointers to locals; the handles aren't inheritable.
        if unsafe { CreatePipe(&mut read, &mut write, None, 0) }.is_err() {
            return console;
        }
        // SAFETY: `write` is the pipe's write end, open for the process's life.
        if unsafe { SetStdHandle(STD_ERROR_HANDLE, write) }.is_err() {
            return console;
        }
        CAPTURING.store(true, Ordering::Relaxed);
        let (read, original) = (read.0 as isize, original.0 as isize);
        let spawned = std::thread::Builder::new()
            .name("app-log".into())
            .spawn(move || {
                // SAFETY: the read end is owned by this thread from here on.
                let pipe =
                    unsafe { std::fs::File::from_raw_handle(read as *mut core::ffi::c_void) };
                // Borrowed, never closed: the console's handle belongs to the console.
                let console_out = console.then(|| {
                    ManuallyDrop::new(unsafe {
                        std::fs::File::from_raw_handle(original as *mut core::ffi::c_void)
                    })
                });
                let mut console_out = console_out;
                // Blocks until a line arrives; ends when every write end is gone (process exit).
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                        let text = String::from_utf8_lossy(&line);
                        super::line(&text);
                        if let Some(out) = console_out.as_mut() {
                            let _ = out.write_all(&line);
                            let _ = out.write_all(b"\n");
                        }
                    }
                }));
                // The reader is gone: give stderr back, so a write can't fail on a closed pipe.
                CAPTURING.store(false, Ordering::Relaxed);
                // SAFETY: restores the handle stderr had before `capture_stderr`.
                unsafe {
                    let _ =
                        SetStdHandle(STD_ERROR_HANDLE, HANDLE(original as *mut core::ffi::c_void));
                }
                let _ = result;
            });
        if spawned.is_err() {
            // SAFETY: as above; no reader, so stderr goes back at once.
            unsafe {
                let _ = SetStdHandle(STD_ERROR_HANDLE, HANDLE(original as *mut core::ffi::c_void));
            }
            CAPTURING.store(false, Ordering::Relaxed);
        }
        console
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn capturing() -> bool {
        false
    }
    pub fn since_boot_secs() -> Option<u64> {
        None
    }
    /// Elsewhere only panics reach the file; stderr stays as it is.
    pub fn capture_stderr() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("foxglove-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(timestamp(1_792_937_407), "2026-10-25 14:10:07Z");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00:00Z", "a leap day");
    }

    #[test]
    fn lines_are_timestamped_and_kept() {
        let d = temp("lines");
        let mut log = Log::open(&d).unwrap();
        log.write_line(0, "Foxglove: hello\r\n");
        log.write_line(1, "Foxglove: again");
        assert_eq!(
            recent_in(&d, 10),
            vec![
                "1970-01-01 00:00:00Z Foxglove: hello",
                "1970-01-01 00:00:01Z Foxglove: again"
            ]
        );
        assert_eq!(
            recent_in(&d, 1),
            vec!["1970-01-01 00:00:01Z Foxglove: again"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rolls_at_a_megabyte_and_keeps_two_files() {
        let d = temp("roll");
        let mut log = Log::open(&d).unwrap();
        let text = "x".repeat(999);
        for _ in 0..3_500 {
            log.write_line(0, &text);
        }
        let size = |n: &str| std::fs::metadata(d.join(n)).map(|m| m.len()).unwrap_or(0);
        assert!(
            size(FILE) <= MAX_BYTES && size(OLD) <= MAX_BYTES,
            "{} {}",
            size(FILE),
            size(OLD)
        );
        assert!(size(OLD) > MAX_BYTES / 2, "rolled at least once");
        let files = std::fs::read_dir(&d).unwrap().count();
        assert_eq!(files, 2, "never more than two files");
        // Reopening carries on in the same file at its size.
        drop(log);
        let before = size(FILE);
        let mut log = Log::open(&d).unwrap();
        log.write_line(0, "after");
        assert!(size(FILE) > before);
        assert_eq!(recent_in(&d, 1), vec!["1970-01-01 00:00:00Z after"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn secrets_are_masked() {
        assert_eq!(
            redact("Foxglove obs: token=abc123 sent"),
            "Foxglove obs: token=*** sent"
        );
        assert_eq!(
            redact("password: hunter2, user bob"),
            "password: ***, user bob"
        );
        assert_eq!(redact("Authorization: Bearer xyz"), "Authorization: ***");
        assert_eq!(redact("sent Bearer xyz to obs"), "sent Bearer *** to obs");
        assert_eq!(redact("\"api_key\":\"k-1\""), "\"api_key\":\"***\"");
        assert_eq!(redact("secret_key=s3"), "secret_key=***");
        // Words that only contain a secret word, and ones without a value, are left alone.
        assert_eq!(
            redact("Foxglove: removing obs's secrets"),
            "Foxglove: removing obs's secrets"
        );
        assert_eq!(redact("tokenizer ready"), "tokenizer ready");
        assert_eq!(
            redact("no secrets here: just text"),
            "no secrets here: just text"
        );
        assert_eq!(
            redact("left · battery 61% — ünïcode"),
            "left · battery 61% — ünïcode"
        );
    }

    #[test]
    fn the_started_line_says_when_and_how() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(
            started_line(7, Some(52), &[]),
            format!("Foxglove {v} started (pid 7, 52 s after boot)")
        );
        assert_eq!(
            started_line(7, None, &["--apply-update".into(), "x".into()]),
            format!("Foxglove {v} started (pid 7) with --apply-update x")
        );
    }

    #[test]
    fn early_lines_reach_the_file_at_once() {
        let d = temp("early");
        *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(Log::open(&d).unwrap());
        early("Foxglove: already running; showing that one");
        *LOG.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let lines = recent_in(&d, 50);
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with("Foxglove: already running; showing that one")),
            "{lines:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_log_lives_in_the_settings_folder() {
        let d = dir().expect("a settings folder");
        assert_eq!(d.file_name().unwrap(), "logs");
    }
}
