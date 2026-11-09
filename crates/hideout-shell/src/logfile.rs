// SPDX-License-Identifier: MIT
//! The hub's log, `logs\hideout.log` under the hub folder (contract §2.3), through the `log`
//! facade. One line per record: UTC time, level, target, message. At `MAX_BYTES` the file moves
//! to `hideout.1.log` (one old file kept), checked when the log opens and as it grows.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "hideout.log";
pub const MAX_BYTES: u64 = 1024 * 1024;

pub struct FileLog {
    path: PathBuf,
    inner: Mutex<(File, u64)>,
    level: log::LevelFilter,
}

impl FileLog {
    /// Opens (or creates) the log in `dir`, rotating first if it is already full.
    pub fn open(dir: &Path, level: log::LevelFilter) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE_NAME);
        let (file, len) = open_rotating(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new((file, len)),
            level,
        })
    }

    /// Installs this log as the process's logger; `Err` if one is already set.
    pub fn install(self) -> Result<(), log::SetLoggerError> {
        let level = self.level;
        log::set_boxed_logger(Box::new(self))?;
        log::set_max_level(level);
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_line(&self, line: &str) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.1 + line.len() as u64 > MAX_BYTES {
            if let Ok(fresh) = open_rotating_now(&self.path) {
                *g = (fresh, 0);
            }
        }
        if g.0.write_all(line.as_bytes()).is_ok() {
            g.1 += line.len() as u64;
        }
    }
}

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.level
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            self.write_line(&format_line(
                SystemTime::now(),
                r.level(),
                r.target(),
                &r.args().to_string(),
            ));
        }
    }

    fn flush(&self) {
        if let Ok(mut g) = self.inner.lock() {
            let _ = g.0.flush();
        }
    }
}

fn open_rotating(path: &Path) -> std::io::Result<(File, u64)> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len >= MAX_BYTES {
        return Ok((open_rotating_now(path)?, 0));
    }
    let f = OpenOptions::new().create(true).append(true).open(path)?;
    Ok((f, len))
}

fn open_rotating_now(path: &Path) -> std::io::Result<File> {
    let old = path.with_file_name("hideout.1.log");
    let _ = std::fs::remove_file(&old);
    let _ = std::fs::rename(path, &old);
    OpenOptions::new().create(true).append(true).open(path)
}

/// `2026-11-09T14:03:07Z INFO  hideout: started`
pub fn format_line(t: SystemTime, level: log::Level, target: &str, msg: &str) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, mo, d) = civil(days as i64);
    let msg = msg.replace(['\r', '\n'], " ");
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z {level:<5} {target}: {msg}\n",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's civil_from_days).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;
    use std::time::Duration;

    #[test]
    fn lines_have_utc_time_level_and_one_line() {
        let t = UNIX_EPOCH + Duration::from_secs(1_794_232_987); // 2026-11-09T14:03:07Z
        assert_eq!(
            format_line(t, log::Level::Info, "hideout", "started\nok"),
            "2026-11-09T14:03:07Z INFO  hideout: started ok\n"
        );
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
    }

    #[test]
    fn writes_and_rotates() {
        let dir = std::env::temp_dir().join(format!("hideout-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = FileLog::open(&dir, log::LevelFilter::Info).unwrap();
        let rec = |msg: &str| {
            log.log(
                &log::Record::builder()
                    .level(log::Level::Info)
                    .target("t")
                    .args(format_args!("{msg}"))
                    .build(),
            );
        };
        rec("first");
        log.log(
            &log::Record::builder()
                .level(log::Level::Debug)
                .target("t")
                .args(format_args!("hidden"))
                .build(),
        );
        log.flush();
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("INFO  t: first") && !text.contains("hidden"));
        let big = "x".repeat(4096);
        for _ in 0..(MAX_BYTES / 4096 + 2) {
            rec(&big);
        }
        log.flush();
        assert!(dir.join("hideout.1.log").exists());
        assert!(std::fs::metadata(log.path()).unwrap().len() < MAX_BYTES);
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
