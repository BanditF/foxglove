//! The after-lines' rules on a fake clock: no thread, no timer.

use super::*;

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| s.to_string()).collect()
}

#[test]
fn resume_says_so_until_the_next_change_or_ten_seconds() {
    let t0 = Instant::now();
    let mut t = Tracker::default();
    let held = names(&["Arete"]);
    assert_eq!(t.update(&held, true, true, false, false, t0), None);
    t.resume_pressed(&held, t0);
    // The hub has not dropped the hold yet: still held, no line.
    assert_eq!(t.update(&held, true, true, false, false, t0), None);
    let said = t.update(&[], false, true, false, false, t0);
    assert_eq!(said, Some(After::Resumed(held.clone())));
    assert_eq!(t.take_deadline(), Some(t0 + SHOW), "one wake-up to arm");
    assert_eq!(t.take_deadline(), None);
    // Unrelated refreshes keep it.
    let sec = Duration::from_secs(1);
    assert!(t.update(&[], false, true, false, false, t0 + sec).is_some());
    // Ten seconds end it, and no timer is left in the tracker.
    assert_eq!(t.update(&[], false, true, false, false, t0 + SHOW), None);
    assert!(!t.showing() && t.take_deadline().is_none());
    // A change ends it earlier: the wallpapers are held again.
    let mut t = Tracker::default();
    t.update(&held, true, true, false, false, t0);
    t.resume_pressed(&held, t0);
    t.update(&[], false, true, false, false, t0);
    assert_eq!(t.update(&held, true, true, false, false, t0 + sec), None);
    assert_eq!(t.update(&[], false, true, false, false, t0 + sec), None);
}

#[test]
fn a_stopped_app_is_named_only_when_it_was_the_last_and_stopped() {
    let t0 = Instant::now();
    let mut t = Tracker::default();
    let held = names(&["Arete"]);
    t.update(&held, true, true, false, false, t0);
    // It stopped: its connection closed, then nothing holds it.
    assert_eq!(
        t.update(&[], false, true, false, true, t0),
        Some(After::Stopped(Some("Arete".into())))
    );
    // It let go itself: no line.
    let mut t = Tracker::default();
    t.update(&held, true, true, false, false, t0);
    assert_eq!(t.update(&[], false, true, false, false, t0), None);
    // Others still hold it: the plain held row says who, no line.
    let mut t = Tracker::default();
    t.update(
        &names(&["Arete", "Render queue"]),
        true,
        true,
        false,
        false,
        t0,
    );
    assert_eq!(
        t.update(&names(&["Render queue"]), true, true, false, true, t0),
        None
    );
    // The wallpaper app quit: its own line (the section's), not "stopped".
    let mut t = Tracker::default();
    t.update(&held, true, true, false, false, t0);
    assert_eq!(t.update(&[], false, false, false, true, t0), None);
    // A fresh connection (a hub reconnect) never saw who was there: it says nothing.
    let mut t = Tracker::default();
    assert_eq!(t.update(&[], false, true, false, true, t0), None);
}

#[test]
fn a_stop_hint_is_used_once_and_goes_stale() {
    let t0 = Instant::now();
    holder_stopped(t0);
    assert!(take_stop(t0 + Duration::from_secs(1)));
    assert!(!take_stop(t0 + Duration::from_secs(1)), "used once");
    holder_stopped(t0);
    assert!(
        !take_stop(t0 + Duration::from_secs(60)),
        "an old stop is another's"
    );
}
