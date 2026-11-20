//! A new health warning or failure as a notification (#814): a WARN or FAIL row that wasn't
//! there at the previous check goes out through `notice::show`, at most once a day per kind.
//!
//! - **A kind** is the row's subsystem and item ("raw_hid" + "Right half"). Its last day sent is
//!   kept in the store's own `[health_notices]` section, read and written alone, so a restart
//!   on the same day stays quiet.
//! - **New** means not WARN or FAIL at the previous check (none before the first, so a warning
//!   already there at start is told once that day). A WARN turning FAIL is the same kind, not new.
//! - **Health-only rows** never notify: Raw HID refusals (counts the card explains) and the
//!   "not tested" rows (a missing firmware bit, not a fault).

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::health::{HealthRow, Level};

const SECTION: &str = "health_notices";

/// The store's section: each kind's last day sent, `YYYY-MM-DD` in local time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sent {
    pub last: BTreeMap<String, String>,
}

/// The kinds that were WARN or FAIL at the previous check; `None` before the first.
static BEFORE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// A row that stays on the health card only.
pub fn health_only(r: &HealthRow) -> bool {
    let refusal = r.subsystem == "raw_hid" && r.detail.contains("refused");
    let not_tested = r.item.to_ascii_lowercase().contains("not tested")
        || r.detail.to_ascii_lowercase().contains("not tested");
    // A refused wallpaper connection is fixed by itself; the health row says it, no toast.
    let wallpaper = r.subsystem == "live_feed" && r.detail == "a wallpaper couldn't connect";
    refusal || not_tested || wallpaper
}

fn kind(r: &HealthRow) -> String {
    format!("{}|{}", r.subsystem, r.item)
}

/// A row's notification: title and body. A row's own (`HealthRow::notice`) wins over the
/// default "<item>: warning" / "<item> failed" and its detail.
pub fn text(r: &HealthRow) -> (String, String) {
    if let Some(own) = &r.notice {
        return own.clone();
    }
    let title = match r.level {
        Level::Fail => format!("{} failed", r.item),
        _ => format!("{}: warning", r.item),
    };
    let detail = r.detail.trim_end();
    let body = if detail.is_empty() {
        "See Keyboard health on the App page.".to_string()
    } else {
        let stop = if detail.ends_with(['.', '!', '?']) {
            ""
        } else {
            "."
        };
        format!("{detail}{stop} See Keyboard health on the App page.")
    };
    (title, body)
}

/// The rows to notify now, given the kinds flagged at the previous check, the days already
/// sent (updated for the ones returned) and today. Also gives the kinds flagged now.
pub fn due(
    before: Option<&HashSet<String>>,
    rows: &[HealthRow],
    sent: &mut Sent,
    today: &str,
) -> (Vec<HealthRow>, HashSet<String>) {
    let mut now = HashSet::new();
    let mut out = Vec::new();
    for r in rows
        .iter()
        .filter(|r| r.level >= Level::Warn && !health_only(r))
    {
        let k = kind(r);
        if !now.insert(k.clone()) {
            continue;
        }
        let new = before.is_none_or(|b| !b.contains(&k));
        if new && sent.last.get(&k).map(String::as_str) != Some(today) {
            sent.last.insert(k, today.to_string());
            out.push(r.clone());
        }
    }
    (out, now)
}

/// Most separate notifications one check sends; more are folded into one summary.
const MAX_SEPARATE: usize = 2;

/// What goes out for the rows due at a check: nothing for the first check in the hub's module
/// mode (the baseline: the warnings already there when it connects stay on the health card,
/// where the hub's own surface shows them), else one notification per row, or one summary
/// when more than [`MAX_SEPARATE`] are due at once.
pub fn outgoing(
    first_check: bool,
    module: bool,
    due: &[HealthRow],
) -> Vec<(String, String, super::live_wire::Level)> {
    use super::live_wire::Level as Wire;
    if due.is_empty() || (first_check && module) {
        return Vec::new();
    }
    let wire = |r: &HealthRow| {
        if r.level == Level::Fail {
            Wire::Warn
        } else {
            Wire::Info
        }
    };
    if due.len() <= MAX_SEPARATE {
        return due
            .iter()
            .map(|r| {
                let (t, b) = text(r);
                (t, b, wire(r))
            })
            .collect();
    }
    let items: Vec<&str> = due.iter().map(|r| r.item.as_str()).collect();
    let level = if due.iter().any(|r| r.level == Level::Fail) {
        Wire::Warn
    } else {
        Wire::Info
    };
    vec![(
        format!("Keyboard health: {} new warnings", due.len()),
        format!("{}. See Keyboard health on the App page.", items.join(", ")),
        level,
    )]
}

/// After a health check: notifies the new warnings and failures, and keeps the days sent.
pub fn check(rows: &[HealthRow]) {
    let mut before = BEFORE.lock().unwrap_or_else(|e| e.into_inner());
    let first_check = before.is_none();
    // Only this section, read just now and written back on its own.
    let mut sent: Sent = super::store::section(STORE_OWNER, SECTION)
        .try_load()
        .unwrap_or_default();
    let (notify, now) = due(before.as_ref(), rows, &mut sent, &today());
    *before = Some(now);
    drop(before);
    if notify.is_empty() {
        return;
    }
    if let Err(e) = super::store::section(STORE_OWNER, SECTION).save(&sent) {
        eprintln!("Foxglove: can't keep the health notices' days: {e}");
    }
    let module = super::hub_link::module_mode();
    if first_check && module {
        eprintln!(
            "Foxglove: {} health warning(s) at connect stay on the health card (module mode)",
            notify.len()
        );
    }
    for (title, body, level) in outgoing(first_check, module, &notify) {
        super::notice::show(super::notice::Kind::Health, &title, &body, level);
    }
}

/// Today in local time, `YYYY-MM-DD`.
#[cfg(windows)]
pub(super) fn today() -> String {
    // SAFETY: no arguments; it returns the time by value.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

/// Today (UTC off Windows), `YYYY-MM-DD`.
#[cfg(not(windows))]
pub(super) fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "foxglove.link";

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sub: &str, item: &str, level: Level, detail: &str) -> HealthRow {
        HealthRow::new(sub, item, level, detail)
    }

    fn relay(level: Level) -> HealthRow {
        row(
            "relay",
            "Link to the right half",
            level,
            "2 art pushes failed (relay timeout)",
        )
    }

    #[test]
    fn a_new_warning_notifies_once_a_day() {
        let mut sent = Sent::default();
        let ok = [relay(Level::Ok)];
        let warn = [relay(Level::Warn)];
        // A first check with it OK, then it warns: new, once.
        let (n, before) = due(None, &ok, &mut sent, "2026-11-05");
        assert!(n.is_empty());
        let (n, before) = due(Some(&before), &warn, &mut sent, "2026-11-05");
        assert_eq!(n, warn.to_vec(), "a new WARN notifies");
        // Still there at the next check: not new.
        let (n, before) = due(Some(&before), &warn, &mut sent, "2026-11-05");
        assert!(n.is_empty(), "the same WARN, still there");
        // Gone and back the same day (or a restart): already sent today.
        let (_, gone) = due(Some(&before), &ok, &mut sent, "2026-11-05");
        let (n, _) = due(Some(&gone), &warn, &mut sent, "2026-11-05");
        assert!(n.is_empty(), "the same WARN on the same day");
        let (n, _) = due(None, &warn, &mut sent, "2026-11-05");
        assert!(n.is_empty(), "a restart the same day");
        // The next day it may come again.
        let (n, before) = due(None, &warn, &mut sent, "2026-11-06");
        assert_eq!(n.len(), 1, "again the next day");
        // Turning FAIL is the same kind: not new.
        let (n, _) = due(
            Some(&before),
            &[relay(Level::Fail)],
            &mut sent,
            "2026-11-06",
        );
        assert!(n.is_empty());
        assert_eq!(sent.last["relay|Link to the right half"], "2026-11-06");
    }

    #[test]
    fn module_mode_connect_is_quiet_and_bursts_fold() {
        let rows: Vec<HealthRow> = (0..6)
            .map(|i| row("x", &format!("Item {i}"), Level::Warn, "d"))
            .collect();
        assert!(outgoing(true, true, &rows).is_empty(), "module first check");
        let standalone = outgoing(true, false, &rows);
        assert_eq!(standalone.len(), 1, "a burst folds into one summary");
        assert!(standalone[0].0.contains("6 new warnings"));
        assert_eq!(
            outgoing(false, true, &rows[..2]).len(),
            2,
            "later, up to two"
        );
        assert_eq!(outgoing(false, true, &rows[..1]).len(), 1);
        assert!(outgoing(false, true, &[]).is_empty());
    }

    #[test]
    fn health_only_rows_never_notify() {
        let rows = [
            row(
                "raw_hid",
                "Right half",
                Level::Warn,
                "3 reports refused: 3 by USB, 0 by Bluetooth",
            ),
            row(
                "raw_hid",
                "Left half",
                Level::Fail,
                "2 reports refused by USB or Bluetooth",
            ),
            row(
                "lights",
                "Not tested",
                Level::Warn,
                "SET_EFFECT not tested: no HELLO bit 19",
            ),
            row(
                "knob",
                "Knob",
                Level::Warn,
                "KNOB not tested: no HELLO bit 31",
            ),
        ];
        let mut sent = Sent::default();
        for day in ["2026-11-05", "2026-11-06"] {
            let (n, _) = due(None, &rows, &mut sent, day);
            assert!(n.is_empty(), "{n:?}");
        }
        assert!(sent.last.is_empty());
        // A dropped report on the same subsystem isn't a refusal: it does notify.
        let dropped = row(
            "raw_hid",
            "Right half",
            Level::Warn,
            "4 reports dropped (the USB queue was full, or USB reset)",
        );
        assert_eq!(due(None, &[dropped], &mut sent, "2026-11-05").0.len(), 1);
    }

    #[test]
    fn the_texts_read_plainly() {
        assert_eq!(
            text(&relay(Level::Warn)),
            (
                "Link to the right half: warning".to_string(),
                "2 art pushes failed (relay timeout). See Keyboard health on the App page."
                    .to_string()
            )
        );
        assert_eq!(text(&relay(Level::Fail)).0, "Link to the right half failed");
        assert_eq!(
            text(&row("x", "Right screen", Level::Warn, "")).1,
            "See Keyboard health on the App page."
        );
    }

    /// A row's own notice text replaces the default; a row without one keeps the default.
    #[test]
    fn a_rows_own_notice_text_wins() {
        let own = row(
            "live_feed",
            "Live wallpaper",
            Level::Warn,
            "a wallpaper drew nothing",
        )
        .with_notice("Short title", "The body.");
        assert_eq!(
            text(&own),
            ("Short title".to_string(), "The body.".to_string())
        );
        let plain = row(
            "live_feed",
            "Live wallpaper",
            Level::Warn,
            "a wallpaper drew nothing",
        );
        assert_eq!(
            text(&plain),
            (
                "Live wallpaper: warning".to_string(),
                "a wallpaper drew nothing. See Keyboard health on the App page.".to_string()
            )
        );
        assert_eq!(
            text(&relay(Level::Warn)).0,
            "Link to the right half: warning"
        );
        // The once-per-kind-per-day rule doesn't look at the text.
        let mut sent = Sent::default();
        assert_eq!(
            due(None, std::slice::from_ref(&own), &mut sent, "2026-11-06")
                .0
                .len(),
            1
        );
        assert!(due(None, &[own], &mut sent, "2026-11-06").0.is_empty());
    }

    /// The section round-trips through the store alone, and a check sends through the (fake)
    /// toast sink once.
    #[test]
    fn the_days_sent_persist_in_their_own_section() {
        crate::foxglove::store::test_config_dir();
        let mut sent = Sent::default();
        sent.last.insert("relay|Link".into(), "2026-11-05".into());
        crate::foxglove::store::save(SECTION, &sent).unwrap();
        assert_eq!(
            crate::foxglove::store::try_load::<Sent>(SECTION),
            Some(sent)
        );
        crate::foxglove::notice::test_toasts::take_mine();
        let fresh = row("health_notice_test", "Test row", Level::Fail, "broken");
        check(std::slice::from_ref(&fresh));
        check(std::slice::from_ref(&fresh));
        let shown = crate::foxglove::notice::test_toasts::take_mine();
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0].title, "Test row failed");
        let kept: Sent = crate::foxglove::store::try_load(SECTION).unwrap();
        assert_eq!(kept.last.get("health_notice_test|Test row"), Some(&today()));
    }
}
