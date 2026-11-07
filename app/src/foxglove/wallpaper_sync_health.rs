//! Health of wallpaper sync (#586, #493), as plain data for the shared health view
//! (`health.read`, the app's health page, `foxglove-lab health`; #705): how many pushes went to
//! Wallpaper Engine in the last minute (each starts a `wallpaper64.exe -control` process), how
//! many changes needed none because the wallpaper already had them, how many syncs waited for
//! Wallpaper Engine's reload after a full-screen program, and why the last push went.
//!
//! The sync worker notes each sync as it happens; [`rows`] only reads. No polling: the minute's
//! count is worked out from the push times when it is read.
//!
//! The rows are the Keyboard health view's shared type (`foxglove_link::health::HealthRow`, #705),
//! registered with `health::register` as "wallpaper_sync".

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// The shared row type of the Keyboard health view (#705).
pub use super::health::{HealthRow, Level};

/// The subsystem these rows belong to in the health view.
pub const SUBSYSTEM: &str = "wallpaper.sync";

/// More pushes than this in a minute is a burst worth a look: each one starts a Wallpaper
/// Engine process.
pub const BURST_PER_MINUTE: usize = 20;

const MINUTE: Duration = Duration::from_secs(60);

/// Why a sync ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The worker started: the wallpapers get the current state.
    Start,
    /// The wallpapers on screen changed (Wallpaper Engine's config).
    Wallpapers,
    /// A scene started, stopped or was placed again.
    Scene,
    /// The live feed started or stopped listening.
    Live,
    /// Windows' app mode changed (light or dark apps).
    Theme,
    /// The keyboard's layer settled.
    Layer,
    /// A full-screen program left the front and Wallpaper Engine's reload had time to finish.
    Reload,
    /// A wallpaper on screen was installed again or updated in place: Wallpaper Engine reloads
    /// it from its saved properties, so it is sent everything again.
    Reinstalled,
    /// A wallpaper connected to the live feed (it just loaded): it is sent everything again.
    Reconnect,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Start => "first state",
            Reason::Wallpapers => "wallpapers on screen changed",
            Reason::Scene => "scene changed",
            Reason::Live => "live feed changed",
            Reason::Theme => "Windows app mode changed",
            Reason::Layer => "layer change",
            Reason::Reload => "after a full-screen program (Wallpaper Engine's reload)",
            Reason::Reinstalled => "a wallpaper on screen was reinstalled or updated",
            Reason::Reconnect => "a wallpaper connected to the live feed",
        }
    }
}

/// Wallpaper sync's health now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncHealth {
    /// Pushes since Foxglove started (one per monitor sent something).
    pub pushes: u64,
    /// When the pushes of the last minute went (older ones are dropped as new ones come).
    recent: VecDeque<Instant>,
    /// Monitors a sync reached that already had the newest properties: no push.
    pub suppressed: u64,
    /// Syncs that waited for Wallpaper Engine's reload after a full-screen program.
    pub held_for_reload: u64,
    /// Of those, the ones that sent everything again (Wallpaper Engine stops wallpapers under a
    /// full-screen program, so the reloaded one starts from its saved properties).
    pub resent_after_reload: u64,
    /// Why the last push went, and when.
    pub last: Option<(Reason, Instant)>,
    /// The last push that failed, while no push has gone since.
    pub last_error: Option<String>,
}

impl SyncHealth {
    pub const fn new() -> Self {
        SyncHealth {
            pushes: 0,
            recent: VecDeque::new(),
            suppressed: 0,
            held_for_reload: 0,
            resent_after_reload: 0,
            last: None,
            last_error: None,
        }
    }

    /// A sync for `reason` sent `pushed` monitors their properties and skipped `unchanged`
    /// that already had them.
    pub fn synced(&mut self, reason: Reason, pushed: usize, unchanged: usize, at: Instant) {
        self.suppressed = self.suppressed.saturating_add(unchanged as u64);
        if pushed == 0 {
            return;
        }
        self.pushes = self.pushes.saturating_add(pushed as u64);
        for _ in 0..pushed {
            self.recent.push_back(at);
        }
        self.drop_old(at);
        self.last = Some((reason, at));
        self.last_error = None;
    }

    /// A push failed with `e` (Wallpaper Engine quit, no answer in time).
    pub fn failed(&mut self, e: String) {
        self.last_error = Some(e);
    }

    /// A sync waited for Wallpaper Engine's reload; `resent` when it sent everything again.
    pub fn reload(&mut self, resent: bool) {
        self.held_for_reload = self.held_for_reload.saturating_add(1);
        if resent {
            self.resent_after_reload = self.resent_after_reload.saturating_add(1);
        }
    }

    fn drop_old(&mut self, now: Instant) {
        while self
            .recent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= MINUTE)
        {
            self.recent.pop_front();
        }
    }

    /// Pushes in the minute before `now`.
    pub fn per_minute(&self, now: Instant) -> usize {
        self.recent
            .iter()
            .filter(|t| now.saturating_duration_since(**t) < MINUTE)
            .count()
    }
}

static HEALTH: Mutex<SyncHealth> = Mutex::new(SyncHealth::new());

/// Notes an event (the sync worker calls this as each happens).
pub fn note(f: impl FnOnce(&mut SyncHealth)) {
    f(&mut HEALTH.lock().unwrap_or_else(|e| e.into_inner()));
}

/// Wallpaper sync's health now.
pub fn now() -> SyncHealth {
    HEALTH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The health view's rows now (registered with `health::register`).
pub fn rows_now() -> Vec<HealthRow> {
    rows(&now(), Instant::now())
}

/// The health view's rows at `now`: Pushes, Duplicates suppressed, Last push.
pub fn rows(h: &SyncHealth, now: Instant) -> Vec<HealthRow> {
    let row = |what: &str, level, detail: String| {
        HealthRow::new(SUBSYSTEM, format!("Wallpaper sync · {what}"), level, detail)
    };
    let minute = h.per_minute(now);
    let pushes = if minute > BURST_PER_MINUTE {
        row(
            "Pushes",
            Level::Warn,
            format!(
                "{minute} in the last minute (each starts a Wallpaper Engine process), {} since Foxglove started",
                h.pushes
            ),
        )
    } else {
        row(
            "Pushes",
            Level::Ok,
            format!(
                "{minute} in the last minute, {} since Foxglove started",
                h.pushes
            ),
        )
    };
    let suppressed = row(
        "Duplicates suppressed",
        Level::Ok,
        format!(
            "{} change(s) the wallpapers already had; {} sync(s) waited for Wallpaper Engine's reload after a full-screen program, {} sent everything again",
            h.suppressed, h.held_for_reload, h.resent_after_reload
        ),
    );
    let last = match (&h.last_error, h.last) {
        (Some(e), _) => row(
            "Last push",
            Level::Warn,
            format!("the last push failed: {e}; it goes again on the next change"),
        ),
        (None, None) => row("Last push", Level::Ok, "none since Foxglove started".into()),
        (None, Some((reason, at))) => row(
            "Last push",
            Level::Ok,
            format!(
                "{}, {} s ago",
                reason.label(),
                now.saturating_duration_since(at).as_secs()
            ),
        ),
    };
    vec![pushes, suppressed, last]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signal fires: pushes, duplicates and reload holds show in the rows, and the minute's
    /// count lets old pushes go.
    #[test]
    fn each_sync_shows_in_the_rows() {
        let t0 = Instant::now();
        let mut h = SyncHealth::new();
        let r = rows(&h, t0);
        assert!(r
            .iter()
            .all(|r| r.level == Level::Ok && r.subsystem == SUBSYSTEM));
        assert_eq!(
            r[0].detail,
            "0 in the last minute, 0 since Foxglove started"
        );
        assert_eq!(r[2].detail, "none since Foxglove started");

        h.synced(Reason::Start, 2, 0, t0);
        h.synced(Reason::Wallpapers, 0, 2, t0 + Duration::from_secs(1));
        let r = rows(&h, t0 + Duration::from_secs(3));
        assert_eq!(
            r[0].detail,
            "2 in the last minute, 2 since Foxglove started"
        );
        assert!(r[1].detail.starts_with("2 change(s)"), "{}", r[1].detail);
        assert_eq!(
            r[2].detail, "first state, 3 s ago",
            "a sync with no push keeps the last"
        );

        h.reload(true);
        h.synced(Reason::Reload, 1, 1, t0 + Duration::from_secs(30));
        let r = rows(&h, t0 + Duration::from_secs(61));
        assert_eq!(
            h.per_minute(t0 + Duration::from_secs(61)),
            1,
            "the first two are over a minute old"
        );
        assert_eq!(
            r[0].detail,
            "1 in the last minute, 3 since Foxglove started"
        );
        assert!(
            r[1].detail.contains("1 sync(s) waited")
                && r[1].detail.contains("1 sent everything again")
        );
        assert!(
            r[2].detail.starts_with("after a full-screen program"),
            "{}",
            r[2].detail
        );

        h.failed("no answer in 5 s".into());
        assert_eq!(rows(&h, t0)[2].level, Level::Warn);
        h.synced(Reason::Layer, 1, 0, t0 + Duration::from_secs(62));
        assert_eq!(h.last_error, None, "a push that went clears the failure");

        // A burst: each push starts a Wallpaper Engine process.
        let t1 = t0 + Duration::from_secs(200);
        for i in 0..=BURST_PER_MINUTE {
            h.synced(
                Reason::Layer,
                1,
                0,
                t1 + Duration::from_millis(i as u64 * 100),
            );
        }
        assert_eq!(rows(&h, t1 + Duration::from_secs(5))[0].level, Level::Warn);
        assert_eq!(rows(&h, t1 + Duration::from_secs(120))[0].level, Level::Ok);
    }

    #[test]
    fn the_shared_signal_records_notes() {
        let before = now().suppressed;
        note(|h| h.synced(Reason::Layer, 0, 3, Instant::now()));
        assert!(now().suppressed >= before + 3);
    }
}
