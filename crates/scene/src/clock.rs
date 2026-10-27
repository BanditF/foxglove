//! The shared clock. Scene time is milliseconds since an epoch on the system clock (Unix
//! milliseconds), so anything that knows the epoch computes the same phase from its own clock:
//! a wallpaper handed the epoch through Wallpaper Engine's command line (#39, #493) draws the
//! scanline where the keys show it, with nothing streamed. The formula, for any host:
//!
//! ```text
//! phase = ((now_unix_ms - epoch_unix_ms) mod period_ms) / period_ms     (0 <= phase < 1)
//! ```
//!
//! A clock either starts now (Foxglove owns the scene) or aligns to an epoch it is given (one
//! a wallpaper or another host already runs on). Nothing here ticks: time is read on demand.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Milliseconds since the scene's epoch; negative before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SceneTime(pub i64);

impl SceneTime {
    pub fn ms(self) -> i64 {
        self.0
    }

    /// Where in a cycle of `period_ms` this time falls, 0 <= phase < 1. A zero period stands
    /// still at 0.
    pub fn phase(self, period_ms: u32) -> f32 {
        if period_ms == 0 {
            return 0.0;
        }
        let p = i64::from(period_ms);
        (self.0.rem_euclid(p) as f64 / p as f64) as f32
    }

    /// Seconds, for effects that move at a speed rather than in a cycle.
    pub fn seconds(self) -> f64 {
        self.0 as f64 / 1000.0
    }
}

/// One scene's clock: an epoch in Unix milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    epoch_unix_ms: i64,
}

/// The system clock now, in Unix milliseconds (what `Date.now()` returns in a wallpaper).
pub fn unix_ms_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

impl Clock {
    /// A clock on an epoch someone else chose: the shared start time a wallpaper (or another
    /// Foxglove host) already runs on.
    pub const fn aligned(epoch_unix_ms: i64) -> Clock {
        Clock { epoch_unix_ms }
    }

    /// A clock that starts at the system clock's now.
    pub fn starting_now() -> Clock {
        Clock::aligned(unix_ms_now())
    }

    /// The epoch to hand another host so it shares this clock.
    pub fn epoch_unix_ms(&self) -> i64 {
        self.epoch_unix_ms
    }

    /// Scene time at a system-clock moment.
    pub fn at(&self, unix_ms: i64) -> SceneTime {
        SceneTime(unix_ms.saturating_sub(self.epoch_unix_ms))
    }

    /// Scene time now.
    pub fn now(&self) -> SceneTime {
        self.at(unix_ms_now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_hosts_on_one_epoch_agree_on_the_phase() {
        // Foxglove starts a scene and hands the wallpaper its epoch.
        let ours = Clock::aligned(1_761_000_000_000);
        let theirs = Clock::aligned(ours.epoch_unix_ms());
        let now = 1_761_000_012_345;
        assert_eq!(ours.at(now), SceneTime(12_345));
        assert_eq!(ours.at(now).phase(4000), theirs.at(now).phase(4000));
        // The wallpaper's formula, written out.
        let wallpaper = ((now - 1_761_000_000_000) % 4000) as f32 / 4000.0;
        assert_eq!(ours.at(now).phase(4000), wallpaper);
    }

    #[test]
    fn phase_wraps_and_handles_time_before_the_epoch() {
        assert_eq!(SceneTime(0).phase(1000), 0.0);
        assert_eq!(SceneTime(1500).phase(1000), 0.5);
        assert_eq!(SceneTime(-250).phase(1000), 0.75);
        assert_eq!(SceneTime(123).phase(0), 0.0);
        let c = Clock::starting_now();
        assert!(c.now().ms() >= 0);
        assert!(unix_ms_now() > 1_700_000_000_000);
    }
}
