//! The Pomodoro clock: focus, short rests, and a long rest every few rounds, kept as deadlines
//! (Unix seconds) so it survives the plugin restarting, the PC sleeping, or a late wake-up.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Focus,
    Rest,
    LongRest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Durations {
    pub focus: u64,
    pub rest: u64,
    pub long_rest: u64,
    /// Focus periods before a long rest.
    pub rounds: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub phase: Phase,
    /// 1-based focus round.
    pub round: u32,
    /// When the phase ends, while running.
    pub deadline: Option<u64>,
    /// Seconds left, while paused.
    pub paused_left: Option<u64>,
}

impl Durations {
    pub fn of(&self, phase: Phase) -> u64 {
        match phase {
            Phase::Focus => self.focus,
            Phase::Rest => self.rest,
            Phase::LongRest => self.long_rest,
        }
    }
}

impl Timer {
    /// A first focus period, starting now.
    pub fn start(now: u64, d: &Durations) -> Timer {
        Timer {
            phase: Phase::Focus,
            round: 1,
            deadline: Some(now + d.focus),
            paused_left: None,
        }
    }

    pub fn paused(&self) -> bool {
        self.paused_left.is_some()
    }

    /// Seconds left in the phase.
    pub fn left(&self, now: u64) -> u64 {
        self.paused_left
            .or(self.deadline.map(|d| d.saturating_sub(now)))
            .unwrap_or(0)
    }

    /// How far through the phase, 0.0 to 1.0.
    pub fn progress(&self, now: u64, d: &Durations) -> f32 {
        let total = d.of(self.phase).max(1);
        1.0 - self.left(now) as f32 / total as f32
    }

    fn next(&self, d: &Durations) -> (Phase, u32) {
        match self.phase {
            Phase::Focus if self.round >= d.rounds => (Phase::LongRest, self.round),
            Phase::Focus => (Phase::Rest, self.round),
            Phase::Rest => (Phase::Focus, self.round + 1),
            Phase::LongRest => (Phase::Focus, 1),
        }
    }

    /// Moves past every phase that has ended by `now`; true if the phase changed. Phases chain
    /// from the previous deadline, so time keeps counting while nothing watched; after a long
    /// absence (more than a whole cycle) it starts a fresh focus period instead.
    pub fn tick(&mut self, now: u64, d: &Durations) -> bool {
        let Some(mut deadline) = self.deadline else {
            return false;
        };
        if deadline > now {
            return false;
        }
        let cycle = d.focus * u64::from(d.rounds)
            + d.rest * u64::from(d.rounds.saturating_sub(1))
            + d.long_rest;
        if now - deadline > cycle {
            *self = Timer::start(now, d);
            return true;
        }
        while deadline <= now {
            let (phase, round) = self.next(d);
            self.phase = phase;
            self.round = round;
            deadline += d.of(phase);
        }
        self.deadline = Some(deadline);
        true
    }

    /// Pauses, or resumes where it was.
    pub fn toggle_pause(&mut self, now: u64) {
        match self.paused_left.take() {
            Some(left) => self.deadline = Some(now + left),
            None => {
                self.paused_left = Some(self.left(now));
                self.deadline = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: Durations = Durations {
        focus: 1500,
        rest: 300,
        long_rest: 900,
        rounds: 4,
    };

    #[test]
    fn rounds_rests_and_the_long_rest() {
        let mut t = Timer::start(0, &D);
        let mut seen = vec![];
        let mut now = 0;
        for _ in 0..9 {
            now = t.deadline.unwrap();
            assert!(t.tick(now, &D));
            seen.push((t.phase, t.round));
        }
        assert_eq!(
            seen,
            [
                (Phase::Rest, 1),
                (Phase::Focus, 2),
                (Phase::Rest, 2),
                (Phase::Focus, 3),
                (Phase::Rest, 3),
                (Phase::Focus, 4),
                (Phase::LongRest, 4),
                (Phase::Focus, 1),
                (Phase::Rest, 1)
            ]
        );
        assert_eq!(t.left(now), 300);
    }

    #[test]
    fn time_keeps_counting_while_away_and_a_long_absence_starts_fresh() {
        let mut t = Timer::start(0, &D);
        // Back 30 minutes later: focus (25) ended, 5 minutes of rest are gone too.
        assert!(t.tick(1800, &D));
        assert_eq!((t.phase, t.round, t.left(1800)), (Phase::Focus, 2, 1500));
        // Back the next day: a fresh first focus period.
        assert!(t.tick(100_000, &D));
        assert_eq!((t.phase, t.round, t.left(100_000)), (Phase::Focus, 1, 1500));
        assert!(!t.tick(100_001, &D));
    }

    #[test]
    fn pause_holds_the_time_and_survives_a_restart() {
        let mut t = Timer::start(0, &D);
        t.toggle_pause(100);
        assert_eq!(t.left(5000), 1400, "paused time doesn't run");
        assert!(!t.tick(5000, &D));
        let saved = serde_json::to_string(&t).unwrap();
        let mut back: Timer = serde_json::from_str(&saved).unwrap();
        back.toggle_pause(6000);
        assert_eq!((back.left(6000), back.deadline), (1400, Some(7400)));
        assert!((back.progress(6000, &D) - 100.0 / 1500.0).abs() < 1e-6);
    }
}
