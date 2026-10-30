//! Quitting within a budget (#618). Foxglove hands things back before it exits: the left panel,
//! a running scene's or cue's lights, the right half's art, a calibrator's or "Find my LEDs"'
//! colours, the live feed and the wallpapers. Several of those wait on another thread with no
//! limit: the scene's control thread is joined (`scene_run::end`), and so is the last "Find my
//! LEDs" worker (`led_map::wait_for_last`). Each of those threads gives the lights back over the
//! devices' own links, one write after another, and the wallpapers get up to 6 s
//! (`wallpaper_sync::QUIT_LIMIT`). In the #669 gate the app took more than 30 s to go after it
//! was asked to (On-call: still alive at 30 s), so the Test lab's 20 s and 2 s waits let the
//! next app start beside it.
//!
//! Now every quit path ([`hand_back`]: the tray's Quit and the overlay window closing, e.g.
//! `taskkill` without /F or Windows signing out) runs two chains at once on their own threads
//! and waits for them at most [`BUDGET`]:
//!  - the keyboard and the lights, in the order they need: the panel, a cue, the scene (while the
//!    art service still runs to put a scene's art back), the art service, a calibrator, "Find my
//!    LEDs";
//!  - the live feed and the wallpapers, which don't touch the keyboard.
//!
//! What hasn't finished by then is left as it is, with a log line, and the process exits.
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The most a quit waits for its hand-backs: the process is gone in under 5 s.
pub const BUDGET: Duration = Duration::from_secs(4);

/// A chain of hand-backs that runs on its own thread.
pub type Chain = (&'static str, Box<dyn FnOnce() + Send>);

/// Runs each chain on its own thread and waits until `deadline` for all of them. Returns the
/// names of those that hadn't finished (their threads end with the process).
pub fn within(deadline: Instant, chains: Vec<Chain>) -> Vec<&'static str> {
    let (tx, rx) = mpsc::channel::<&'static str>();
    let mut open: Vec<&'static str> = Vec::new();
    for (name, chain) in chains {
        let tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("quit-{name}"))
            .spawn(move || {
                chain();
                let _ = tx.send(name);
            });
        if spawned.is_ok() {
            open.push(name);
        }
    }
    drop(tx);
    while !open.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(done) => open.retain(|n| *n != done),
            Err(_) => break, // the deadline (or every sender gone, which can't leave one open)
        }
    }
    open
}

/// The app's hand-backs before it exits, within [`BUDGET`]. Logs how it went.
pub fn hand_back(why: &str) {
    let start = Instant::now();
    let chains: Vec<Chain> = vec![
        (
            "lights",
            Box::new(|| {
                super::screens::stop(); // the left panel back to the firmware (<= 0.5 s)
                super::notifications::quit(); // a light cue gives the lights back
                super::scenes_ui::quitting(); // no profile starts a scene now
                super::scene_run::quit(); // a scene gives the lights back (or keeps its effect)
                super::art_service::stop();
                super::calibrate_ui::cancel(); // mid-calibration: the colours go back
                super::led_map_ui::quit(); // mid "Find my LEDs": the lights go back
            }),
        ),
        (
            "wallpapers",
            Box::new(|| {
                super::live_feed::stop();
                super::wallpaper_sync::quit(); // back to their own clocks
            }),
        ),
    ];
    let open = within(start + BUDGET, chains);
    let ms = start.elapsed().as_millis();
    // Written at once: the process exits right after.
    super::app_log::early(&if open.is_empty() {
        format!("Foxglove: quitting ({why}); handed everything back in {ms} ms")
    } else {
        format!(
            "Foxglove: quitting ({why}); {} not handed back after {ms} ms, left as they are",
            open.join(", ")
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_chains_finish_and_report_nothing_open() {
        let start = Instant::now();
        let open = within(
            start + Duration::from_secs(2),
            vec![("a", Box::new(|| {})), ("b", Box::new(|| {}))],
        );
        assert!(open.is_empty());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_stuck_chain_is_left_at_the_deadline_while_the_others_finish() {
        let (release, hold) = mpsc::channel::<()>();
        let start = Instant::now();
        let open = within(
            start + Duration::from_millis(300),
            vec![
                // Blocks like an unbounded join on a worker that never ends.
                (
                    "stuck",
                    Box::new(move || {
                        let _ = hold.recv();
                    }),
                ),
                ("quick", Box::new(|| {})),
            ],
        );
        let took = start.elapsed();
        assert_eq!(open, ["stuck"]);
        assert!(
            took >= Duration::from_millis(300) && took < Duration::from_secs(2),
            "{took:?}"
        );
        drop(release);
    }

    #[test]
    fn the_chains_run_at_once_not_one_after_the_other() {
        let start = Instant::now();
        let slow = || std::thread::sleep(Duration::from_millis(300));
        let open = within(
            start + Duration::from_secs(2),
            vec![("one", Box::new(slow)), ("two", Box::new(slow))],
        );
        assert!(open.is_empty());
        assert!(
            start.elapsed() < Duration::from_millis(550),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn the_budget_keeps_the_exit_under_five_seconds() {
        assert!(BUDGET < Duration::from_secs(5));
    }
}
