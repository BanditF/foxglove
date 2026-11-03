//! `foxglove-lab health` (#705): the keyboard's self-report as pass/fail per feature, the one test
//! after a batch flash. Read over the host link (`foxglove_link::health`), or through the app's
//! `health.read` (API 1.11) when the app holds the keyboard. Exit 0: nothing failed (warnings
//! are printed), 1: something failed, 2: blocked (no keyboard, firmware without the
//! self-report, the app's API out of reach).
//!
//! `--record FILE` keeps the D7 93 replies of a direct read; `--replay FILE` judges a recording
//! offline (no device), so a run on hardware can be read again and a recorded block is a test
//! fixture (tests/data/health-recorded.hex).

use anyhow::{anyhow, Result};
use foxglove_link::health::{self as kh, HealthRow, Level, Report, Thresholds};
use foxglove_link::protocol::{self, REPORT};
use foxglove_link::Transport;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

/// Exit codes.
pub const PASS: u8 = 0;
pub const FAIL: u8 = 1;
pub const BLOCKED: u8 = 2;

/// What one run found: the rows, and why it was blocked if it was.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub rows: Vec<HealthRow>,
    pub blocked: Option<String>,
}

impl Outcome {
    pub fn code(&self) -> u8 {
        if self.blocked.is_some() {
            BLOCKED
        } else if kh::worst(&self.rows) == Level::Fail {
            FAIL
        } else {
            PASS
        }
    }

    /// The printed lines: one per row (PASS / WARN / FAIL, the group, the line), then the result.
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .rows
            .iter()
            .map(|r| {
                let word = match r.level {
                    Level::Ok => "PASS",
                    Level::Warn => "WARN",
                    Level::Fail => "FAIL",
                };
                format!("{word} {:<10} {}", r.subsystem, r.line())
            })
            .collect();
        let fails = self.rows.iter().filter(|r| r.level == Level::Fail).count();
        let warns = self.rows.iter().filter(|r| r.level == Level::Warn).count();
        out.push(match (&self.blocked, self.code()) {
            (Some(why), _) => format!("BLOCKED: {why}"),
            (None, FAIL) => format!("FAIL: {fails} failed, {warns} warning(s)"),
            _ => format!("PASS: nothing failed, {warns} warning(s)"),
        });
        out
    }

    /// The same as JSON, for scripts: {"result", "blocked", "rows": [{subsystem, item, level, detail}]}.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "result": match self.code() { PASS => "pass", FAIL => "fail", _ => "blocked" },
            "blocked": self.blocked,
            "rows": self.rows.iter().map(|r| serde_json::json!({
                "subsystem": r.subsystem,
                "item": r.item,
                "level": match r.level { Level::Ok => "ok", Level::Warn => "warn", Level::Fail => "fail" },
                "detail": r.detail,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Judges one read of both halves. A left that can't be read (or firmware without the report)
/// blocks the test: nothing about the keyboard is known. A right that can't be read is a
/// failure: the relay or the right half is what's broken.
pub fn judge(report: &Report) -> Outcome {
    let rows = kh::rows(report, &Thresholds::default());
    let blocked = match &report.left {
        Ok(_) => None,
        Err(kh::ReadError::NotBuilt) => {
            Some("this firmware has no self-report (no HELLO bit 28): flash a build with it".into())
        }
        Err(e) => Some(format!("the left half's health couldn't be read: {e}")),
    };
    Outcome { rows, blocked }
}

/// Reads both halves over `t` (HELLO's `capabilities` say whether it has the report).
pub fn read(t: &impl Transport, capabilities: u32) -> Outcome {
    judge(&kh::read_both(t, capabilities))
}

/// A transport that keeps every reply it passes on: `--record`.
pub struct Recorder<'a, T: Transport> {
    pub inner: &'a T,
    pub replies: RefCell<Vec<[u8; REPORT]>>,
}

impl<'a, T: Transport> Recorder<'a, T> {
    pub fn new(inner: &'a T) -> Self {
        Recorder {
            inner,
            replies: RefCell::new(Vec::new()),
        }
    }

    /// The recording: a comment, the HELLO capabilities, then one reply per line in hex.
    pub fn text(&self, capabilities: u32) -> String {
        let mut s = format!(
            "# foxglove-lab health recording: D7 93 replies in the order they came\ncaps {capabilities:08x}\n"
        );
        for r in self.replies.borrow().iter() {
            s.push_str(&hex(r));
            s.push('\n');
        }
        s
    }
}

impl<T: Transport> Transport for Recorder<'_, T> {
    fn send(&self, report: &[u8]) -> Result<()> {
        self.inner.send(report)
    }
    fn request(
        &self,
        report: &[u8],
        proto: u8,
        cmd: u8,
        timeout: Duration,
    ) -> Result<[u8; REPORT]> {
        let r = self.inner.request(report, proto, cmd, timeout)?;
        self.replies.borrow_mut().push(r);
        Ok(r)
    }
}

fn hex(r: &[u8]) -> String {
    r.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A recording played back: each request gets the next reply. `--replay` and the offline test.
pub struct Replay {
    pub capabilities: u32,
    replies: RefCell<VecDeque<[u8; REPORT]>>,
}

impl Replay {
    /// Reads [`Recorder::text`]'s format; blank lines and `#` comments are skipped.
    pub fn parse(text: &str) -> Result<Replay> {
        let mut capabilities = protocol::capabilities::HEALTH;
        let mut replies = VecDeque::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(caps) = line.strip_prefix("caps ") {
                capabilities = u32::from_str_radix(caps.trim(), 16)
                    .map_err(|_| anyhow!("line {}: bad caps", i + 1))?;
                continue;
            }
            let bytes: Vec<u8> = line
                .split_whitespace()
                .map(|b| u8::from_str_radix(b, 16))
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| anyhow!("line {}: not hex bytes", i + 1))?;
            anyhow::ensure!(
                bytes.len() <= REPORT,
                "line {}: more than {REPORT} bytes",
                i + 1
            );
            let mut r = [0u8; REPORT];
            r[..bytes.len()].copy_from_slice(&bytes);
            replies.push_back(r);
        }
        Ok(Replay {
            capabilities,
            replies: RefCell::new(replies),
        })
    }
}

impl Transport for Replay {
    fn send(&self, _: &[u8]) -> Result<()> {
        Ok(())
    }
    fn request(&self, _: &[u8], _: u8, _: u8, _: Duration) -> Result<[u8; REPORT]> {
        self.replies
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| anyhow!("no reply from the keyboard (the recording ended)"))
    }
}

/// The app's `health.read` rows as an outcome. An app without the call (older than API 1.11)
/// or out of reach blocks the test.
pub fn from_app(timeout: Duration) -> Outcome {
    match crate::app_api::health(timeout) {
        Ok(h) => {
            let rows: Vec<HealthRow> = h
                .rows
                .into_iter()
                .map(|r| {
                    use foxglove_plugin_api::methods::HealthLevel;
                    let level = match r.level {
                        HealthLevel::Ok => Level::Ok,
                        HealthLevel::Warn => Level::Warn,
                        HealthLevel::Fail => Level::Fail,
                    };
                    HealthRow::new(r.subsystem, r.item, level, r.detail)
                })
                .collect();
            // The app reads the keyboard as the lab would; a firmware without the report shows
            // as its one "flash a newer build" row.
            let blocked = (rows.len() == 1 && rows[0].detail.contains("flash a newer build"))
                .then(|| "this firmware has no self-report (no HELLO bit 28)".to_string());
            Outcome { rows, blocked }
        }
        Err(e) => Outcome {
            rows: Vec::new(),
            blocked: Some(format!("the app's health.read: {e:#}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(half: u8) -> kh::Block {
        kh::Block {
            version: 1,
            half,
            flags: if half == 0 {
                kh::flags::RELAY_CENTRAL | kh::flags::HID_STATS
            } else {
                kh::flags::RELAY_PERIPHERAL
            },
            sha: "abc1234".into(),
            self_test: [1, 1, 1, 1],
            split_connected: true,
            leds: 21,
            leds_right: if half == 0 { 21 } else { 0 },
            pc_reason: 0xff,
            ..Default::default()
        }
    }

    fn recording(left: &kh::Block, right: &kh::Block) -> String {
        let mut s = String::from("caps 10000000\n");
        for r in left.replies(1).iter().chain(right.replies(1).iter()) {
            s.push_str(&hex(r));
            s.push('\n');
        }
        s
    }

    #[test]
    fn a_healthy_recording_passes() {
        let replay = Replay::parse(&recording(&block(0), &block(1))).unwrap();
        let out = read(&replay, replay.capabilities);
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(out
            .lines()
            .last()
            .unwrap()
            .starts_with("PASS: nothing failed"));
    }

    #[test]
    fn a_failure_exits_1_and_says_what() {
        let left = kh::Block {
            art_failed: 6,
            art_last_error: 6,
            ..block(0)
        };
        let replay = Replay::parse(&recording(&left, &block(1))).unwrap();
        let out = read(&replay, replay.capabilities);
        assert_eq!(out.code(), FAIL);
        assert!(out
            .lines()
            .iter()
            .any(|l| l == "FAIL screens    Right screen: 6 art pushes failed (relay timeout or write failure)"),
            "{:#?}", out.lines());
        assert_eq!(out.json()["result"], "fail");
    }

    #[test]
    fn no_report_or_no_left_is_blocked() {
        let out = read(&Replay::parse("").unwrap(), 0);
        assert_eq!(out.code(), BLOCKED);
        assert!(out
            .lines()
            .last()
            .unwrap()
            .starts_with("BLOCKED: this firmware has no self-report"));
        let out = read(
            &Replay::parse("caps 10000000\n").unwrap(),
            protocol::capabilities::HEALTH,
        );
        assert_eq!(out.code(), BLOCKED, "{:#?}", out.lines());
    }

    #[test]
    fn a_right_that_cant_be_read_fails() {
        let mut s = recording(&block(0), &block(1));
        // The right's chunk 0 says no relay link instead.
        let lines: Vec<&str> = s.lines().take(5).collect();
        s = lines.join("\n") + "\nd7 93 02 01\n";
        let replay = Replay::parse(&s).unwrap();
        let out = read(&replay, replay.capabilities);
        assert_eq!(out.code(), FAIL, "{:#?}", out.lines());
    }

    #[test]
    fn recordings_round_trip() {
        let replay = Replay::parse(&recording(&block(0), &block(1))).unwrap();
        let rec = Recorder::new(&replay);
        let first = kh::read(&rec, 0).unwrap();
        let text = rec.text(protocol::capabilities::HEALTH);
        assert!(text.starts_with("# foxglove-lab health recording"));
        let again = Replay::parse(&text).unwrap();
        assert_eq!(kh::read(&again, 0).unwrap(), first);
        assert!(Replay::parse("zz 01").is_err());
    }
}
