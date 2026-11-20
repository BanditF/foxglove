//! Sign-in hand-over health (slice 6): how each sign-in start of Foxglove ended, counted in the
//! settings store (section `signin_counts`) and shown as one health row, so On-call's batch health
//! check sees a timeout without reading logs. Also the exact log lines for those outcomes.
//!
//! Counters: `signin.deferred` (Hideout runs Foxglove as its app; this copy exited),
//! `signin.on_its_own` (started on its own although Hideout starts Foxglove) and
//! `signin.timeout` (no answer from Hideout within 30 s; counted by the wait in `signin_handoff`). Counts only; no path, token or secret is stored or logged.

use serde::{Deserialize, Serialize};

pub const STORE_OWNER: &str = "core";
pub const SECTION: &str = "signin_counts";
pub const SUBSYSTEM: &str = "signin";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counts {
    pub deferred: u32,
    pub on_its_own: u32,
    pub timeout: u32,
    /// When the last timeout happened (unix seconds), so the warning can fade.
    pub last_timeout: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Deferred,
    OnItsOwn,
    Timeout,
}

impl Counts {
    pub fn bumped(&self, k: Kind, now: u64) -> Counts {
        let mut c = self.clone();
        if k == Kind::Timeout {
            c.last_timeout = Some(now);
        }
        let n = match k {
            Kind::Deferred => &mut c.deferred,
            Kind::OnItsOwn => &mut c.on_its_own,
            Kind::Timeout => &mut c.timeout,
        };
        *n = n.saturating_add(1);
        c
    }
}

pub const LOG_DEFERRED: &str = "sign-in: Hideout runs Foxglove as its app; this copy exits";
pub const LOG_TIMEOUT: &str = "sign-in: no answer from Hideout in 30 s; starting on its own";

/// ` (waited 4.2 s)` after a wait that Hideout answered; nothing for a start that did not wait.
fn waited_suffix(waited: Option<std::time::Duration>) -> String {
    waited.map_or_else(String::new, |d| {
        format!(" (waited {})", super::signin_handoff::secs(d))
    })
}

pub fn deferred_line(waited: Option<std::time::Duration>) -> String {
    format!("{LOG_DEFERRED}{}", waited_suffix(waited))
}

/// `reason`: why Hideout does not run Foxglove (safe mode, stopped after crashes, waiting for an
/// answer, runs on its own).
pub fn on_its_own_line(reason: &str, waited: Option<std::time::Duration>) -> String {
    format!(
        "sign-in: Hideout answered that it does not run Foxglove ({reason}); starting on its own{}",
        waited_suffix(waited)
    )
}

/// A timeout older than this no longer warns.
pub const RECENT_SECS: u64 = 7 * 24 * 3600;

fn times(n: u32) -> String {
    if n == 1 {
        "1 sign-in".to_string()
    } else {
        format!("{n} sign-ins")
    }
}

/// The one health row, in plain words (the counter names stay in the store): absent until
/// something is counted; a warning only while the last timeout is recent (`now` is unix seconds).
pub fn rows(c: &Counts, now: u64) -> Vec<super::health::HealthRow> {
    use super::health::{HealthRow, Level};
    if *c == Counts::default() {
        return Vec::new();
    }
    let recent = c
        .last_timeout
        .is_some_and(|t| now.saturating_sub(t) <= RECENT_SECS);
    let mut parts = Vec::new();
    if c.deferred > 0 {
        parts.push(format!("Hideout started Foxglove at {}", times(c.deferred)));
    }
    if c.on_its_own > 0 {
        parts.push(format!(
            "Foxglove started on its own at {}",
            times(c.on_its_own)
        ));
    }
    if recent {
        parts.push(
            "Hideout did not answer in time at a recent sign-in, so Foxglove started on its own"
                .to_string(),
        );
    } else if c.timeout > 0 {
        parts.push(format!(
            "Hideout was slow to answer at {} (not recently)",
            times(c.timeout)
        ));
    }
    let level = if recent { Level::Warn } else { Level::Ok };
    vec![HealthRow::new(
        SUBSYSTEM,
        "Start at sign-in",
        level,
        parts.join("; "),
    )]
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn section() -> super::store::Section {
    super::store::section(STORE_OWNER, SECTION)
}

pub fn current() -> Counts {
    section().try_load::<Counts>().unwrap_or_default()
}

/// The health provider.
pub fn rows_now() -> Vec<super::health::HealthRow> {
    rows(&current(), unix_now())
}

/// Counts one outcome (best effort: a failed save only costs the count).
pub fn note(k: Kind) {
    let next = current().bumped(k, unix_now());
    if let Err(e) = section().save(&next) {
        eprintln!("Foxglove: sign-in hand-over: couldn't save the count: {e}");
    }
}

/// Registers the health row. Once, at start.
pub fn start() {
    super::health::register(SUBSYSTEM, rows_now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::health::Level;

    const NOW: u64 = 10_000_000;

    #[test]
    fn each_outcome_counts_its_own_counter() {
        let c = Counts::default()
            .bumped(Kind::Deferred, NOW)
            .bumped(Kind::Deferred, NOW);
        let c = c.bumped(Kind::OnItsOwn, NOW).bumped(Kind::Timeout, NOW);
        assert_eq!(
            c,
            Counts {
                deferred: 2,
                on_its_own: 1,
                timeout: 1,
                last_timeout: Some(NOW)
            }
        );
    }

    #[test]
    fn the_log_lines_are_exact() {
        assert_eq!(
            LOG_DEFERRED,
            "sign-in: Hideout runs Foxglove as its app; this copy exits"
        );
        assert_eq!(
            LOG_TIMEOUT,
            "sign-in: no answer from Hideout in 30 s; starting on its own"
        );
        assert_eq!(
            on_its_own_line("safe mode", None),
            "sign-in: Hideout answered that it does not run Foxglove (safe mode); starting on its own"
        );
        let w = Some(std::time::Duration::from_millis(1100));
        assert_eq!(
            on_its_own_line("safe mode", w),
            "sign-in: Hideout answered that it does not run Foxglove (safe mode); starting on its own (waited 1.1 s)"
        );
        assert_eq!(
            deferred_line(Some(std::time::Duration::from_millis(4200))),
            "sign-in: Hideout runs Foxglove as its app; this copy exits (waited 4.2 s)"
        );
        assert_eq!(deferred_line(None), LOG_DEFERRED);
    }

    #[test]
    fn the_row_is_absent_until_a_count_and_reads_in_plain_words() {
        assert!(rows(&Counts::default(), NOW).is_empty());
        let c = Counts::default()
            .bumped(Kind::Deferred, NOW)
            .bumped(Kind::OnItsOwn, NOW);
        let r = rows(&c, NOW);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].item, "Start at sign-in");
        assert_eq!(r[0].level, Level::Ok);
        assert_eq!(
            r[0].detail,
            "Hideout started Foxglove at 1 sign-in; Foxglove started on its own at 1 sign-in"
        );
        let many = c.bumped(Kind::Deferred, NOW);
        assert!(rows(&many, NOW)[0]
            .detail
            .starts_with("Hideout started Foxglove at 2 sign-ins"));
        assert!(!r[0].detail.contains("signin."));
    }

    #[test]
    fn a_timeout_warns_only_while_recent() {
        let c = Counts::default().bumped(Kind::Timeout, NOW);
        let r = rows(&c, NOW + 60);
        assert_eq!(r[0].level, Level::Warn);
        assert_eq!(
            r[0].detail,
            "Hideout did not answer in time at a recent sign-in, so Foxglove started on its own"
        );
        let old = rows(&c, NOW + RECENT_SECS + 1);
        assert_eq!(old[0].level, Level::Ok);
        assert_eq!(
            old[0].detail,
            "Hideout was slow to answer at 1 sign-in (not recently)"
        );
    }

    #[test]
    fn the_counts_are_stored_in_their_own_section() {
        let s = crate::foxglove::store::test_section("signin_counts");
        let c = Counts::default().bumped(Kind::OnItsOwn, NOW);
        s.save(&c).unwrap();
        assert_eq!(s.try_load::<Counts>(), Some(c));
    }
}
