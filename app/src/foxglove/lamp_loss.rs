//! A held LampArray's lights lost without Windows saying so (#925).
//!
//! On 11/09 the keyboard went dark while the hold still read "Foxglove's": a game restarted and
//! another lighting app ran, and no `AvailabilityChanged` reached Foxglove. So the holder
//! (`lamp_hold`) reads `IsAvailable` again on events it already gets: a foreground-app change
//! (the info panel's foreground hook), a session unlock or the display coming back (`away`), a
//! LampArray or keyboard arriving, and a change of the keyboard's owner byte. Nothing runs on a
//! timer, and a burst of foreground changes is read at most once per [`Policy::min_check_gap`].
//!
//! A held array Foxglove last knew as available that now reads unavailable, with no
//! `AvailabilityChanged` in between, is a **silent loss**: counted and logged once. The holder
//! then re-takes that hold (lets it go, opens it again) and redraws, **once per loss**, with a
//! backoff between recoveries. If Windows still ranks another controller first after that (a
//! controller the user chose, or an app in front), Foxglove leaves it there: the loss ends only
//! when Windows gives the array back. Nothing here knows a vendor or a device: an array is its
//! Windows device id, and the timings are [`Policy`] data.
//!
//! [`Tracker`] is the state machine, pure and driven with explicit instants (tested with fake
//! events below).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The event a check rides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Another app came to the front.
    Foreground,
    /// The session was unlocked.
    Unlock,
    /// The display came back (resume from standby, or the screen waking).
    DisplayOn,
    /// A LampArray or the keyboard arrived.
    Device,
    /// The keyboard's owner byte changed.
    Owner,
}

impl Trigger {
    pub fn words(self) -> &'static str {
        match self {
            Trigger::Foreground => "a foreground-app change",
            Trigger::Unlock => "a session unlock",
            Trigger::DisplayOn => "the display coming back",
            Trigger::Device => "a device arrival",
            Trigger::Owner => "an owner-byte change",
        }
    }
}

/// The checks' bounds and the recoveries' backoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Foreground changes closer than this are read once (other triggers are rare).
    pub min_check_gap: Duration,
    /// The wait after the first recovery before another loss may be recovered.
    pub first_backoff: Duration,
    /// The longest wait (the backoff doubles per recovery up to this).
    pub max_backoff: Duration,
    /// With no loss for this long, the backoff starts over.
    pub settle: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min_check_gap: Duration::from_secs(2),
            first_backoff: Duration::from_secs(30),
            max_backoff: Duration::from_secs(30 * 60),
            settle: Duration::from_secs(30 * 60),
        }
    }
}

/// One held array as read now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    /// Windows' device id.
    pub id: String,
    /// Over Bluetooth LE, not USB.
    pub ble: bool,
    /// `IsAvailable`.
    pub available: bool,
}

/// What one check found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Arrays lost silently since the last read (each once per loss).
    pub lost: Vec<Reading>,
    /// Lost arrays available again by themselves.
    pub back: Vec<String>,
    /// Lost arrays to re-take now (once per loss).
    pub recover: Vec<String>,
    /// Lost arrays not tried yet wait this long for the backoff (the next check after it tries).
    pub wait: Option<Duration>,
}

/// The counts for the health row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Silent losses since start.
    pub losses: u64,
    /// Losses a re-taken hold ended.
    pub recoveries: u64,
    /// Losses still unavailable after their one recovery (left to the controller Windows
    /// ranks first).
    pub left: u64,
    /// Arrays lost now.
    pub lost_now: usize,
    /// The app in front at the last loss, when known.
    pub front: Option<String>,
}

/// The detect/recover state machine.
pub struct Tracker {
    policy: Policy,
    /// Per array id: available as Foxglove last knew it (an event, an open or a check).
    known: BTreeMap<String, bool>,
    /// Per array lost now: whether its one recovery was tried.
    lost: BTreeMap<String, bool>,
    /// Recoveries in the current backoff run.
    step: u32,
    next_try: Option<Instant>,
    last_check: Option<Instant>,
    last_loss: Option<Instant>,
    pub counts: Counts,
}

impl Tracker {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            known: BTreeMap::new(),
            lost: BTreeMap::new(),
            step: 0,
            next_try: None,
            last_check: None,
            last_loss: None,
            counts: Counts::default(),
        }
    }

    /// Whether a check on `trigger` at `now` runs (foreground bursts are read once per gap).
    pub fn due(&self, now: Instant, trigger: Trigger) -> bool {
        trigger != Trigger::Foreground
            || self
                .last_check
                .is_none_or(|t| now.saturating_duration_since(t) >= self.policy.min_check_gap)
    }

    /// What Windows said (an `AvailabilityChanged`, or an open): the new baseline, no loss
    /// counted. Returns the lost arrays available again.
    pub fn heard(&mut self, readings: &[Reading]) -> Vec<String> {
        self.known = readings
            .iter()
            .map(|r| (r.id.clone(), r.available))
            .collect();
        let mut back = Vec::new();
        self.lost.retain(|id, _| {
            match readings.iter().find(|r| &r.id == id) {
                Some(r) if r.available => {
                    back.push(id.clone());
                    false
                }
                Some(_) => true,
                // No longer held: a reopen or a removal ends the loss.
                None => false,
            }
        });
        self.counts.lost_now = self.lost.len();
        back
    }

    /// Reads the held arrays again on an event with no `AvailabilityChanged` in it: an array
    /// known available that reads unavailable is a silent loss. `front` is the app in front.
    pub fn check(&mut self, now: Instant, readings: &[Reading], front: Option<&str>) -> Outcome {
        self.last_check = Some(now);
        if self.lost.is_empty()
            && self
                .last_loss
                .is_some_and(|t| now.saturating_duration_since(t) >= self.policy.settle)
        {
            self.step = 0;
            self.next_try = None;
        }
        let mut out = Outcome::default();
        for r in readings {
            let was = self.known.insert(r.id.clone(), r.available);
            if was == Some(true) && !r.available && !self.lost.contains_key(&r.id) {
                self.lost.insert(r.id.clone(), false);
                self.counts.losses += 1;
                self.counts.front = front.map(str::to_owned);
                self.last_loss = Some(now);
                out.lost.push(r.clone());
            } else if r.available && self.lost.remove(&r.id).is_some() {
                out.back.push(r.id.clone());
            }
        }
        self.known
            .retain(|id, _| readings.iter().any(|r| &r.id == id));
        self.lost
            .retain(|id, _| readings.iter().any(|r| &r.id == id));
        let untried: Vec<String> = self
            .lost
            .iter()
            .filter(|(_, tried)| !**tried)
            .map(|(id, _)| id.clone())
            .collect();
        if !untried.is_empty() {
            match self.next_try.filter(|t| now < *t) {
                Some(t) => out.wait = Some(t - now),
                None => {
                    for id in &untried {
                        self.lost.insert(id.clone(), true);
                    }
                    let backoff = self
                        .policy
                        .first_backoff
                        .saturating_mul(1u32 << self.step.min(16))
                        .min(self.policy.max_backoff);
                    self.next_try = Some(now + backoff);
                    self.step = self.step.saturating_add(1);
                    out.recover = untried;
                }
            }
        }
        self.counts.lost_now = self.lost.len();
        out
    }

    /// After re-taking `tried`, the held arrays read again (`after`): returns `(recovered,
    /// left)`. A left one keeps its loss (tried) until Windows gives it back.
    pub fn recovered(&mut self, tried: &[String], after: &[Reading]) -> (Vec<String>, Vec<String>) {
        let (mut ok, mut left) = (Vec::new(), Vec::new());
        for id in tried {
            match after.iter().find(|r| &r.id == id) {
                Some(r) if r.available => {
                    self.lost.remove(id);
                    self.known.insert(id.clone(), true);
                    self.counts.recoveries += 1;
                    ok.push(id.clone());
                }
                _ => {
                    self.counts.left += 1;
                    left.push(id.clone());
                }
            }
        }
        self.counts.lost_now = self.lost.len();
        (ok, left)
    }

    /// Lost now (tried or not).
    #[cfg(test)]
    pub fn is_lost(&self, id: &str) -> bool {
        self.lost.contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, available: bool) -> Reading {
        Reading {
            id: id.into(),
            ble: id == "ble",
            available,
        }
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn tracker() -> (Tracker, Instant) {
        (Tracker::new(Policy::default()), Instant::now())
    }

    #[test]
    fn an_event_s_unavailability_is_not_a_loss() {
        let (mut t, now) = tracker();
        t.heard(&[r("usb", true), r("ble", true)]);
        // Windows said so (AvailabilityChanged): the baseline moves, nothing is counted.
        t.heard(&[r("usb", true), r("ble", false)]);
        let out = t.check(now, &[r("usb", true), r("ble", false)], None);
        assert_eq!(out, Outcome::default());
        assert_eq!(t.counts.losses, 0);
    }

    #[test]
    fn a_silent_loss_is_counted_once_and_recovered_once() {
        let (mut t, now) = tracker();
        t.heard(&[r("usb", true), r("ble", true)]);
        let out = t.check(now, &[r("usb", true), r("ble", false)], Some("cod.exe"));
        assert_eq!(out.lost, vec![r("ble", false)]);
        assert_eq!(out.recover, vec!["ble".to_string()]);
        assert_eq!(t.counts.losses, 1);
        assert_eq!(t.counts.front.as_deref(), Some("cod.exe"));
        // The same loss on the next events: neither counted nor tried again.
        let again = t.check(now + secs(60), &[r("usb", true), r("ble", false)], None);
        assert_eq!(again, Outcome::default());
        assert_eq!(t.counts.losses, 1);
    }

    #[test]
    fn a_re_taken_hold_that_reads_available_is_a_recovery() {
        let (mut t, now) = tracker();
        t.heard(&[r("ble", true)]);
        let out = t.check(now, &[r("ble", false)], None);
        let (ok, left) = t.recovered(&out.recover, &[r("ble", true)]);
        assert_eq!((ok, left), (vec!["ble".to_string()], vec![]));
        assert_eq!(t.counts.recoveries, 1);
        assert_eq!(t.counts.lost_now, 0);
        assert!(!t.is_lost("ble"));
    }

    /// Windows still ranks another controller first after the one re-take: Foxglove leaves it
    /// (no second try for that loss) until Windows gives it back.
    #[test]
    fn a_controller_windows_ranks_first_is_not_fought() {
        let (mut t, now) = tracker();
        t.heard(&[r("ble", true)]);
        let out = t.check(now, &[r("ble", false)], None);
        let (ok, left) = t.recovered(&out.recover, &[r("ble", false)]);
        assert!(ok.is_empty());
        assert_eq!(left, vec!["ble".to_string()]);
        assert_eq!(t.counts.left, 1);
        for s in [60, 3600, 7200] {
            let o = t.check(now + secs(s), &[r("ble", false)], None);
            assert!(o.recover.is_empty() && o.lost.is_empty(), "never fought");
        }
        // Given back (a check, or an event): the loss ends.
        let o = t.check(now + secs(7300), &[r("ble", true)], None);
        assert_eq!(o.back, vec!["ble".to_string()]);
        assert_eq!(t.counts.lost_now, 0);
    }

    #[test]
    fn recoveries_back_off_and_settle() {
        let (mut t, start) = tracker();
        t.heard(&[r("ble", true)]);
        let mut now = start;
        // Loss 1 at once; loss 2 within the 30 s backoff waits for it.
        let o = t.check(now, &[r("ble", false)], None);
        assert_eq!(o.recover.len(), 1);
        t.recovered(&o.recover, &[r("ble", true)]);
        now += secs(10);
        let o = t.check(now, &[r("ble", false)], None);
        assert_eq!(o.lost.len(), 1);
        assert!(o.recover.is_empty());
        assert_eq!(o.wait, Some(secs(20)));
        // The next event after the backoff tries it; the backoff then doubles (60 s).
        now += secs(25);
        let o = t.check(now, &[r("ble", false)], None);
        assert_eq!(o.recover, vec!["ble".to_string()]);
        t.recovered(&o.recover, &[r("ble", true)]);
        now += secs(30);
        let o = t.check(now, &[r("ble", false)], None);
        assert_eq!(o.wait, Some(secs(30)));
        // Recovered by itself; after the settle time the backoff starts over.
        t.heard(&[r("ble", true)]);
        now += secs(31 * 60);
        let o = t.check(now, &[r("ble", false)], None);
        assert_eq!(o.recover.len(), 1, "settled: no wait");
        assert_eq!(t.counts.losses, 4);
        assert_eq!(t.counts.recoveries, 2);
    }

    #[test]
    fn foreground_bursts_are_read_once_per_gap() {
        let (mut t, now) = tracker();
        assert!(t.due(now, Trigger::Foreground));
        t.check(now, &[], None);
        assert!(!t.due(now + Duration::from_millis(500), Trigger::Foreground));
        assert!(t.due(now + Duration::from_millis(500), Trigger::Unlock));
        assert!(t.due(now + secs(2), Trigger::Foreground));
    }

    #[test]
    fn an_array_let_go_or_reopened_ends_its_loss() {
        let (mut t, now) = tracker();
        t.heard(&[r("usb", true), r("ble", true)]);
        let o = t.check(now, &[r("usb", true), r("ble", false)], None);
        t.recovered(&o.recover, &[r("usb", true), r("ble", false)]);
        assert!(t.is_lost("ble"));
        // Windows no longer lists it (or a reopen dropped it): the loss is over.
        t.heard(&[r("usb", true)]);
        assert!(!t.is_lost("ble"));
        assert_eq!(t.counts.lost_now, 0);
    }

    #[test]
    fn an_array_first_seen_on_a_check_is_a_baseline() {
        let (mut t, now) = tracker();
        let o = t.check(now, &[r("new", false)], None);
        assert_eq!(o, Outcome::default(), "never known available: not a loss");
    }
}
