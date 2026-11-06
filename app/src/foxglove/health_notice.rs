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
    refusal || not_tested
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

/// After a health check: notifies the new warnings and failures, and keeps the days sent.
pub fn check(rows: &[HealthRow]) {
    let mut before = BEFORE.lock().unwrap_or_else(|e| e.into_inner());
    // Only this section, read just now and written back on its own.
    let mut sent: Sent = super::store::try_load(SECTION).unwrap_or_default();
    let (notify, now) = due(before.as_ref(), rows, &mut sent, &today());
    *before = Some(now);
    drop(before);
    if notify.is_empty() {
        return;
    }
    if let Err(e) = super::store::save(SECTION, &sent) {
        eprintln!("Foxglove: can't keep the health notices' days: {e}");
    }
    for r in &notify {
        let (title, body) = text(r);
        let level = if r.level == Level::Fail {
            super::live_wire::Level::Warn
        } else {
            super::live_wire::Level::Info
        };
        super::notice::show(super::notice::Kind::Health, &title, &body, level);
    }
}

/// Today in local time, `YYYY-MM-DD`.
#[cfg(windows)]
fn today() -> String {
    // SAFETY: no arguments; it returns the time by value.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

/// Today (UTC off Windows), `YYYY-MM-DD`.
#[cfg(not(windows))]
fn today() -> String {
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
            "Live wallpaper pairing",
            Level::Warn,
            "a wallpaper is using an old pairing token",
        )
        .with_notice("Short title", "The body.");
        assert_eq!(
            text(&own),
            ("Short title".to_string(), "The body.".to_string())
        );
        let plain = row(
            "live_feed",
            "Live wallpaper pairing",
            Level::Warn,
            "a wallpaper is using an old pairing token",
        );
        assert_eq!(
            text(&plain),
            (
                "Live wallpaper pairing: warning".to_string(),
                "a wallpaper is using an old pairing token. See Keyboard health on the App page."
                    .to_string()
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
