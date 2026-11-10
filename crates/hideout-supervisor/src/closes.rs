// SPDX-License-Identifier: MIT
//! `run\hub-run.json` (§7.10): how the hub's last runs ended, so the hub can enter **safe mode**
//! by itself after closing 3 times in 5 minutes.
//!
//! The hub that holds the folder's supervisor lock writes a "running" mark when it starts and
//! clears it on a clean end: Quit Hideout in the tray, Windows ending the session
//! (`WM_ENDSESSION`), Ctrl+C or a console close. A start that still finds the mark follows an
//! unclean close (a crash, a kill, power loss): its time goes into a ring of the last
//! [`KEEP`]. The third unclean close within [`WINDOW`] turns safe mode on; the ring then starts
//! over, so leaving safe mode needs three new closes to come back.
//!
//! The hub has no self-restart today (no panic or GPU-reset relaunch: egui makes a lost window
//! again in the same process). A future one marks its exit clean with [`mark_clean`] before it
//! starts the new hub, so it never counts. Taking modules back (§7.9) happens in the new hub and
//! changes nothing here.
//!
//! Times are wall-clock milliseconds since 1970 (the file outlives the process). A time ahead of
//! now (the clock went back) doesn't count.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Unclean closes kept in the ring, and how many within [`WINDOW`] turn safe mode on.
pub const KEEP: usize = 3;
/// The window the closes must fall in.
pub const WINDOW: Duration = Duration::from_secs(5 * 60);

/// The running hub's mark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub pid: u32,
    /// When it started (ms since 1970).
    pub since_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubRun {
    /// Set while a hub runs; a start that finds it follows an unclean close.
    #[serde(default)]
    pub running: Option<Running>,
    /// When the last unclean closes were noticed (ms since 1970), oldest first, at most [`KEEP`].
    #[serde(default)]
    pub unclean_ms: Vec<u64>,
    /// Safe mode was turned on by the closes (not by hand): the banner says so. Cleared when
    /// safe mode is off at a start or by Start normally.
    #[serde(default)]
    pub safe_mode_by_closes: bool,
}

/// What a start found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Start {
    /// The last run ended without a clean exit.
    pub unclean: bool,
    /// Unclean closes within [`WINDOW`] of this start, this one included.
    pub recent: usize,
    /// The [`KEEP`]th of them: turn safe mode on.
    pub enter_safe_mode: bool,
    /// When the run that ended uncleanly had started (ms since 1970): a Windows start after
    /// it means the PC went down with it, which the search can't count (#1033).
    pub last_since_ms: Option<u64>,
}

impl HubRun {
    pub fn path(home: &Path) -> PathBuf {
        home.join("run").join("hub-run.json")
    }

    /// The file; empty when missing or unreadable (a first start, nothing counted).
    pub fn load(home: &Path) -> HubRun {
        std::fs::read_to_string(Self::path(home))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Writes the file atomically (temp file, then rename).
    pub fn save(&self, home: &Path) -> io::Result<()> {
        let path = Self::path(home);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// A start at `now_ms` by `pid`: counts the last run if it ended uncleanly, then marks this
    /// one running. Pure; [`on_start`] loads and saves.
    pub fn start(&mut self, pid: u32, now_ms: u64) -> Start {
        let last = self.running.take();
        let unclean = last.is_some();
        if unclean {
            self.unclean_ms.push(now_ms);
            let over = self.unclean_ms.len().saturating_sub(KEEP);
            self.unclean_ms.drain(..over);
        }
        let window = WINDOW.as_millis() as u64;
        let recent = self
            .unclean_ms
            .iter()
            .filter(|&&t| t <= now_ms && now_ms - t <= window)
            .count();
        let enter_safe_mode = unclean && recent >= KEEP;
        if enter_safe_mode {
            self.unclean_ms.clear();
            self.safe_mode_by_closes = true;
        }
        self.running = Some(Running {
            pid,
            since_ms: now_ms,
        });
        Start {
            unclean,
            recent,
            enter_safe_mode,
            last_since_ms: last.map(|r| r.since_ms),
        }
    }
}

/// [`HubRun::start`] on the file. Not saved is logged by the caller: the run then counts as
/// clean next time (nothing to find).
pub fn on_start(home: &Path, pid: u32, now_ms: u64) -> (Start, io::Result<()>) {
    let mut run = HubRun::load(home);
    let start = run.start(pid, now_ms);
    let saved = run.save(home);
    (start, saved)
}

/// Whether the file is there and reads (a start that finds it missing or broken can't tell how
/// the last run ended).
pub fn readable(home: &Path) -> bool {
    std::fs::read_to_string(HubRun::path(home))
        .ok()
        .is_some_and(|t| serde_json::from_str::<HubRun>(&t).is_ok())
}

/// A clean end: clears the running mark. Quick, for `WM_ENDSESSION` and the console handler.
pub fn mark_clean(home: &Path) -> io::Result<()> {
    let mut run = HubRun::load(home);
    if run.running.take().is_none() {
        return Ok(());
    }
    run.save(home)
}

/// Whether the closes turned safe mode on (the banner's line).
pub fn safe_mode_by_closes(home: &Path) -> bool {
    HubRun::load(home).safe_mode_by_closes
}

/// Safe mode is off (Start normally, or off at a start): a later hand-set flag reads neutral.
pub fn clear_safe_mode_cause(home: &Path) -> io::Result<()> {
    let mut run = HubRun::load(home);
    if !run.safe_mode_by_closes {
        return Ok(());
    }
    run.safe_mode_by_closes = false;
    run.save(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hideout-closes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Starts after `n` unclean closes (the mark left each time) at the given minutes.
    fn unclean_starts(home: &Path, at_min: &[u64]) -> Vec<Start> {
        at_min
            .iter()
            .map(|m| on_start(home, 7, 1_000_000_000 + m * MIN).0)
            .collect()
    }

    #[test]
    fn three_unclean_closes_in_five_minutes_turn_safe_mode_on() {
        let home = temp("three");
        // The first start finds nothing; then three runs killed at 1, 2 and 3 minutes.
        let s = unclean_starts(&home, &[0, 1, 2, 3]);
        assert!(!s[0].unclean && !s[0].enter_safe_mode);
        assert_eq!(
            s.iter().map(|s| s.enter_safe_mode).collect::<Vec<_>>(),
            [false, false, false, true]
        );
        assert_eq!(s[3].recent, 3);
        assert!(safe_mode_by_closes(&home));
        // The ring starts over: one more crash doesn't count as the fourth.
        let next = unclean_starts(&home, &[4]);
        assert!(next[0].unclean && !next[0].enter_safe_mode);
        assert_eq!(next[0].recent, 1);
        clear_safe_mode_cause(&home).unwrap();
        assert!(!safe_mode_by_closes(&home));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn clean_quits_dont_count() {
        let home = temp("clean");
        for m in 0..6 {
            let (s, saved) = on_start(&home, 7, 1_000_000_000 + m * MIN / 10);
            saved.unwrap();
            assert!(!s.unclean && !s.enter_safe_mode, "start {m}: {s:?}");
            mark_clean(&home).unwrap();
        }
        assert!(HubRun::load(&home).unclean_ms.is_empty());
        // Clean quits between crashes don't reset the count either: they just don't add.
        unclean_starts(&home, &[10]); // the mark of this run is left
        let s = unclean_starts(&home, &[11]);
        assert!(s[0].unclean);
        mark_clean(&home).unwrap();
        on_start(&home, 7, 1_000_000_000 + 12 * MIN).1.unwrap();
        let s = unclean_starts(&home, &[13]);
        assert_eq!(s[0].recent, 2);
        assert!(!s[0].enter_safe_mode);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn closes_spread_past_five_minutes_dont() {
        let home = temp("spread");
        let s = unclean_starts(&home, &[0, 1, 3, 7]);
        // Closes noticed at 1, 3 and 7 minutes: 1 is more than 5 minutes before 7.
        assert!(s[3].unclean && !s[3].enter_safe_mode, "{:?}", s[3]);
        assert_eq!(s[3].recent, 2);
        assert_eq!(HubRun::load(&home).unclean_ms.len(), 3);
        // A fourth start long after: the old ones are out of the window.
        let s = unclean_starts(&home, &[20]);
        assert_eq!(s[0].recent, 1);
        assert!(!s[0].enter_safe_mode);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_ring_keeps_three_and_a_clock_going_back_doesnt_count() {
        let mut r = HubRun::default();
        for m in 0..6 {
            r.start(1, m * 10 * MIN);
        }
        assert_eq!(r.unclean_ms.len(), KEEP);
        let mut r = HubRun {
            running: Some(Running {
                pid: 1,
                since_ms: 0,
            }),
            unclean_ms: vec![100 * MIN, 101 * MIN],
            safe_mode_by_closes: false,
        };
        let s = r.start(1, 50 * MIN);
        assert_eq!(s.recent, 1);
        assert!(!s.enter_safe_mode);
    }

    #[test]
    fn a_missing_or_broken_file_is_a_first_start() {
        let home = temp("broken");
        std::fs::create_dir_all(home.join("run")).unwrap();
        std::fs::write(HubRun::path(&home), "{not json").unwrap();
        let (s, _) = on_start(&home, 7, 5);
        assert_eq!(s, Start::default());
        mark_clean(&home).unwrap();
        assert_eq!(HubRun::load(&home).running, None);
        let _ = std::fs::remove_dir_all(&home);
    }
}
