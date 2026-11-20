// SPDX-License-Identifier: MIT
//! DF2: a **Review...** or **Set up...** pressed on Settings > Developer asks the Modules page to
//! open that app's permission dialog. The dialog lives there (it needs the page's supervisor and
//! first-run state), so the Settings page only leaves a request here, from the user's own click,
//! and the Modules page takes it on its next frame. A request opens a dialog and nothing else:
//! the Allow in it is still the user's click.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Mutex;

/// (the app's id, true for Set up... and false for Review...), oldest first.
static ASKS: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

/// Leaves a request.
pub fn push(id: &str, set_up: bool) {
    ASKS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id.to_string(), set_up));
}

/// Takes every request left since the last take.
pub fn take() -> Vec<(String, bool)> {
    std::mem::take(&mut *ASKS.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_taken_once_in_order() {
        take();
        push("a", false);
        push("b", true);
        assert_eq!(
            take(),
            vec![("a".to_string(), false), ("b".to_string(), true)]
        );
        assert!(take().is_empty());
    }
}
