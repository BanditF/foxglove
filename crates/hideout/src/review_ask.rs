// SPDX-License-Identifier: MIT
//! DF2: a **Review...** or **Set up...** pressed on Settings > Developer asks the Modules page to
//! open that app's permission dialog. The dialog lives there (it needs the page's supervisor and
//! first-run state), so the Settings page only leaves a request here, from the user's own click,
//! and the Modules page takes it on its next frame. A request opens a dialog and nothing else:
//! the Allow in it is still the user's click.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A request older than this was not made by a click on the page now showing: it is dropped, so
/// it can never open a dialog on a later, unrelated visit to the Modules page.
const LIFE: Duration = Duration::from_secs(5);

/// (the app's id, true for Set up... and false for Review..., when it was asked).
static ASKS: Mutex<Vec<(String, bool, Instant)>> = Mutex::new(Vec::new());

/// Leaves a request. Only the newest stands: a click replaces what was waiting.
pub fn push(id: &str, set_up: bool) {
    push_at(id, set_up, Instant::now());
}

fn push_at(id: &str, set_up: bool, at: Instant) {
    let mut a = ASKS.lock().unwrap_or_else(|e| e.into_inner());
    a.clear();
    a.push((id.to_string(), set_up, at));
}

/// Takes the request left since the last take, if it is still fresh. One use only.
pub fn take() -> Vec<(String, bool)> {
    take_at(Instant::now())
}

fn take_at(now: Instant) -> Vec<(String, bool)> {
    std::mem::take(&mut *ASKS.lock().unwrap_or_else(|e| e.into_inner()))
        .into_iter()
        .filter(|(_, _, at)| now.saturating_duration_since(*at) <= LIFE)
        .map(|(id, up, _)| (id, up))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test: the queue is process-wide.
    #[test]
    fn a_request_is_used_once_and_expires() {
        take();
        push("a", false);
        push("b", true);
        // The newest stands, once.
        assert_eq!(take(), vec![("b".to_string(), true)]);
        assert!(take().is_empty());
        // Older than its life (the page was not shown right away): dropped, never opens later.
        let t0 = Instant::now();
        push_at("c", false, t0);
        assert!(take_at(t0 + LIFE + Duration::from_secs(1)).is_empty());
        // The stale one is gone for good, even when a later visit asks.
        assert!(take().is_empty());
        push_at("d", false, t0);
        assert_eq!(take_at(t0 + LIFE), vec![("d".to_string(), false)]);
    }
}
