//! Sign-in hand-over health (slice 6): how each sign-in start of Foxglove ended, counted in the
//! settings store (section `signin_counts`) and shown as one health row, so On-call's batch health
//! check sees a timeout without reading logs. Also the exact log lines for those outcomes.
//!
//! Counters: `signin.deferred` (Hideout runs Foxglove as its app; this copy exited),
//! `signin.on_its_own` (started on its own although Hideout starts Foxglove) and
//! `signin.timeout` (no answer from Hideout in time; the wait itself is a later slice, so nothing
//! counts it yet). Counts only; no path, token or secret is stored or logged.

#![allow(dead_code)] // `Kind::Timeout` waits for the wait slice

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Deferred,
    OnItsOwn,
    Timeout,
}

impl Counts {
    pub fn bumped(&self, k: Kind) -> Counts {
        let mut c = self.clone();
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

/// `reason`: why Hideout does not run Foxglove (safe mode, stopped after crashes, waiting for an
/// answer, runs on its own).
pub fn on_its_own_line(reason: &str) -> String {
    format!(
        "sign-in: Hideout answered that it does not run Foxglove ({reason}); starting on its own"
    )
}

/// The one health row: OK, with the counts; a warning once a sign-in timed out.
pub fn rows(c: &Counts) -> Vec<super::health::HealthRow> {
    use super::health::{HealthRow, Level};
    if *c == Counts::default() {
        return Vec::new();
    }
    let level = if c.timeout > 0 {
        Level::Warn
    } else {
        Level::Ok
    };
    let detail = format!(
        "signin.deferred {}, signin.on_its_own {}, signin.timeout {}",
        c.deferred, c.on_its_own, c.timeout
    );
    vec![HealthRow::new(
        SUBSYSTEM,
        "Sign-in hand-over",
        level,
        detail,
    )]
}

fn section() -> super::store::Section {
    super::store::section(STORE_OWNER, SECTION)
}

pub fn current() -> Counts {
    section().try_load::<Counts>().unwrap_or_default()
}

/// The health provider.
pub fn rows_now() -> Vec<super::health::HealthRow> {
    rows(&current())
}

/// Counts one outcome (best effort: a failed save only costs the count).
pub fn note(k: Kind) {
    let next = current().bumped(k);
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

    #[test]
    fn each_outcome_counts_its_own_counter() {
        let c = Counts::default()
            .bumped(Kind::Deferred)
            .bumped(Kind::Deferred);
        let c = c.bumped(Kind::OnItsOwn).bumped(Kind::Timeout);
        assert_eq!(
            c,
            Counts {
                deferred: 2,
                on_its_own: 1,
                timeout: 1
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
            on_its_own_line("safe mode"),
            "sign-in: Hideout answered that it does not run Foxglove (safe mode); starting on its own"
        );
    }

    #[test]
    fn the_row_is_absent_until_a_count_and_warns_on_a_timeout() {
        assert!(rows(&Counts::default()).is_empty());
        let ok = rows(&Counts::default().bumped(Kind::Deferred));
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].level, super::super::health::Level::Ok);
        assert_eq!(
            ok[0].detail,
            "signin.deferred 1, signin.on_its_own 0, signin.timeout 0"
        );
        let w = rows(&Counts::default().bumped(Kind::Timeout));
        assert_eq!(w[0].level, super::super::health::Level::Warn);
    }

    #[test]
    fn the_counts_are_stored_in_their_own_section() {
        let s = crate::foxglove::store::test_section("signin_counts");
        let c = Counts::default().bumped(Kind::OnItsOwn);
        s.save(&c).unwrap();
        assert_eq!(s.try_load::<Counts>(), Some(c));
    }
}
