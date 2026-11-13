//! Keyboard lights self-heal (11/13 incident): a scene runs, Foxglove holds the keyboard's
//! LampArray, and the keyboard reports its lights off. After [`N_DARK`] the scene is repainted in
//! full, then the LampArrays are re-taken and the scene sent again; if that fails Foxglove backs
//! off and the user gets a "Repaint now" button (Scenes page, `foxglove-cli scene repaint`).
//!
//! [`Tracker`] is the pure state machine, driven by events with explicit instants (no sleeps in
//! tests). The worker at the bottom feeds it from the app's events and arms one delayed check
//! only while the keyboard is dark: no repeating timer. Wording follows
//! `docs/lights-selfheal-ux.md`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::health::{HealthRow, Level};

/// The subsystem of the health row.
pub const SUBSYSTEM: &str = "lighting.heal";
/// The row's item.
pub const ITEM: &str = "Keyboard lights during scenes";
/// Dark this long (with the scene running and the holds ours) before the first step.
pub const N_DARK: Duration = Duration::from_secs(120);
/// The re-check after a repaint or re-take.
pub const RECHECK: Duration = Duration::from_secs(10);
/// Recoveries within this window count for "keeps going dark".
pub const HOUR: Duration = Duration::from_secs(3600);
/// Recoveries in the last hour that turn the row to Warning.
pub const WARN_AT: usize = 3;
const FIRST_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
const SETTLE: Duration = Duration::from_secs(30 * 60);

/// A local clock time, for the words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hm(pub u8, pub u8);

impl std::fmt::Display for Hm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.0, self.1)
    }
}

/// What the tracker is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The keyboard's report of its lights (`lit` false: off), `unpowered` as it says.
    Lit { lit: bool, unpowered: bool },
    /// A scene runs, drawing, with the keyboard's LampArray held and ours (or not).
    Active(bool),
    /// A delayed check fell due.
    Tick,
    /// The user's Repaint now (the button, `scene repaint`).
    UserRepaint,
}

/// What the app does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Clear what is believed shown and send the scene's colours again.
    Repaint,
    /// Let the LampArrays go, hold them again, and send the scene again.
    Retake,
    Log(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Dark under [`N_DARK`], or not dark.
    Idle,
    Repainted(Instant),
    Retaken(Instant),
    /// Gave up; the next try at.
    GaveUp(Instant),
    /// Repaint now pressed (at); a re-check follows.
    User(Instant, Hm),
}

/// The state machine.
#[derive(Clone, Debug)]
pub struct Tracker {
    active: bool,
    ever_active: bool,
    lit: Option<bool>,
    unpowered: bool,
    dark_since: Option<(Instant, Hm)>,
    stage: Stage,
    /// Set once Repaint now did not help; cleared when lit again or inactive.
    user_failed: Option<Hm>,
    /// Dark episodes that reached a heal step.
    pub darks: u64,
    recoveries: VecDeque<Instant>,
    pub recovered: u64,
    last_recovery: Option<Hm>,
    backoff: Duration,
    last_try: Option<Instant>,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker {
            active: false,
            ever_active: false,
            lit: None,
            unpowered: false,
            dark_since: None,
            stage: Stage::Idle,
            user_failed: None,
            darks: 0,
            recoveries: VecDeque::new(),
            recovered: 0,
            last_recovery: None,
            backoff: FIRST_BACKOFF,
            last_try: None,
        }
    }
}

impl Stage {
    fn is_gave_up(self) -> bool {
        matches!(self, Stage::GaveUp(_))
    }
}

impl Tracker {
    fn dark_now(&self) -> bool {
        self.active && self.lit == Some(false) && !self.unpowered
    }

    /// When the next [`Event::Tick`] is due, while any step waits.
    pub fn deadline(&self) -> Option<Instant> {
        let (since, _) = self.dark_since?;
        Some(match self.stage {
            Stage::Idle => since + N_DARK,
            Stage::Repainted(at) | Stage::Retaken(at) | Stage::User(at, _) => at + RECHECK,
            Stage::GaveUp(next) => next,
        })
    }

    /// The scene is dark and a step is running or the user may act: Repaint now shows.
    pub fn dark(&self) -> bool {
        self.dark_since.is_some() && self.active
    }

    /// A step runs now (Putting back).
    pub fn putting_back(&self) -> bool {
        self.dark()
            && matches!(
                self.stage,
                Stage::Repainted(_) | Stage::Retaken(_) | Stage::User(..)
            )
    }

    /// Past the first step and not recovered: Repaint now is offered.
    pub fn failed(&self) -> bool {
        self.dark() && (self.stage.is_gave_up() || self.user_failed.is_some())
    }

    pub fn dark_since(&self) -> Option<Hm> {
        self.dark_since.map(|(_, h)| h)
    }

    fn recent(&mut self, now: Instant) -> usize {
        while self
            .recoveries
            .front()
            .is_some_and(|t| now.duration_since(*t) > HOUR)
        {
            self.recoveries.pop_front();
        }
        self.recoveries.len()
    }

    /// Handles one event; `wall` is the local time now.
    pub fn event(&mut self, now: Instant, wall: Hm, ev: Event) -> Vec<Act> {
        let mut out = Vec::new();
        match ev {
            Event::Lit { lit, unpowered } => {
                self.lit = Some(lit);
                self.unpowered = unpowered;
            }
            Event::Active(a) => {
                self.active = a;
                self.ever_active |= a;
            }
            Event::UserRepaint => {
                if self.dark() && !self.putting_back() {
                    self.stage = Stage::User(now, wall);
                    out.push(Act::Log(format!(
                        "Foxglove: lights heal: Repaint now at {wall}: repainting"
                    )));
                    out.push(Act::Repaint);
                }
                return out;
            }
            Event::Tick => {}
        }
        if !self.dark_now() {
            if let Some((_, since)) = self.dark_since.take() {
                let how = match self.stage {
                    Stage::Repainted(_) | Stage::User(..) => Some("repaint"),
                    Stage::Retaken(_) => Some("re-take"),
                    Stage::GaveUp(_) => Some("itself, after the heal gave up"),
                    Stage::Idle => None,
                };
                if let Some(how) = how.filter(|_| self.active) {
                    self.recovered += 1;
                    self.recoveries.push_back(now);
                    self.last_recovery = Some(wall);
                    out.push(Act::Log(format!(
                        "Foxglove: lights heal: RECOVERED by {how} (dark since {since}; went dark {}, recovered {})",
                        self.darks, self.recovered
                    )));
                }
                if self.active
                    && self
                        .last_try
                        .is_some_and(|t| now.duration_since(t) > SETTLE)
                {
                    self.backoff = FIRST_BACKOFF;
                }
            }
            self.stage = Stage::Idle;
            self.user_failed = None;
            return out;
        }
        if self.dark_since.is_none() {
            self.dark_since = Some((now, wall));
            self.stage = Stage::Idle;
        }
        if ev != Event::Tick {
            return out;
        }
        let Some(due) = self.deadline().filter(|d| *d <= now) else {
            return out;
        };
        let _ = due;
        let since = self.dark_since.map(|(_, h)| h).unwrap_or(wall);
        match self.stage {
            Stage::Idle => {
                self.darks += 1;
                self.last_try = Some(now);
                self.stage = Stage::Repainted(now);
                out.push(Act::Log(format!(
                    "Foxglove: lights heal: keyboard dark since {since} while a scene runs and its LampArray is ours: repainting (went dark {})",
                    self.darks
                )));
                out.push(Act::Repaint);
            }
            Stage::Repainted(_) => {
                self.stage = Stage::Retaken(now);
                out.push(Act::Log(
                    "Foxglove: lights heal: still dark after the repaint: re-taking the LampArrays and sending the scene again".into(),
                ));
                out.push(Act::Retake);
            }
            Stage::Retaken(_) => {
                let next = now + self.backoff;
                out.push(Act::Log(format!(
                    "Foxglove: lights heal: STILL DARK after re-take; left (went dark {}, recovered {}); next try in {} s",
                    self.darks,
                    self.recovered,
                    self.backoff.as_secs()
                )));
                self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                self.stage = Stage::GaveUp(next);
            }
            Stage::User(_, at) => {
                self.user_failed = Some(at);
                let next = now + self.backoff;
                out.push(Act::Log(format!(
                    "Foxglove: lights heal: Repaint now at {at} didn't help; still dark"
                )));
                self.stage = Stage::GaveUp(next);
            }
            Stage::GaveUp(_) => {
                self.last_try = Some(now);
                self.stage = Stage::Repainted(now);
                out.push(Act::Log(
                    "Foxglove: lights heal: trying again: repainting".into(),
                ));
                out.push(Act::Repaint);
            }
        }
        out
    }

    /// The Keyboard health card's row; `None` before any scene ran, or without a held LampArray.
    pub fn row(&mut self, now: Instant, held: bool) -> Option<HealthRow> {
        if !self.ever_active || !held {
            return None;
        }
        let row = |level, detail: String| HealthRow::new(SUBSYSTEM, ITEM, level, detail);
        let since = self.dark_since();
        if self.dark() {
            if let (Some(at), Some(since)) = (self.user_failed, since) {
                return Some(
                    row(
                        Level::Fail,
                        format!("dark since {since}; Repaint now at {at} didn't help. Click Stop on the Scenes page to get the keyboard's own lights back."),
                    )
                    .with_notice(NOTICE.0, NOTICE.1),
                );
            }
            if self.stage.is_gave_up() {
                let since = since?;
                return Some(
                    row(
                        Level::Fail,
                        format!("dark since {since} and Foxglove couldn't put the lights back. Click Repaint now on the Scenes page."),
                    )
                    .with_notice(NOTICE.0, NOTICE.1),
                );
            }
            if self.putting_back() {
                return Some(row(
                    Level::Ok,
                    "went dark, putting the lights back now…".into(),
                ));
            }
        }
        if self.darks == 0 {
            return Some(row(
                Level::Ok,
                "never went dark since Foxglove started".into(),
            ));
        }
        let last = self
            .last_recovery
            .map(|h| h.to_string())
            .unwrap_or_default();
        let recent = self.recent(now);
        if recent >= WARN_AT {
            return Some(row(
                Level::Warn,
                format!("went dark {recent} times in the last hour, put back each time (last at {last})"),
            ));
        }
        Some(row(
            Level::Ok,
            if self.darks == 1 {
                format!("went dark once since start, put back (at {last})")
            } else {
                format!(
                    "went dark {} times since start, put back each time (last at {last})",
                    self.darks
                )
            },
        ))
    }

    /// The Scenes page's status suffix (tone is warn when `Some`), after "Running: <look>".
    pub fn status_suffix(&self) -> Option<String> {
        if !self.dark() {
            return None;
        }
        let since = self.dark_since()?;
        if self.putting_back() {
            return Some("keyboard went dark, putting the lights back…".into());
        }
        if self.user_failed.is_some() {
            return Some("keyboard lights still dark after Repaint now".into());
        }
        self.stage
            .is_gave_up()
            .then(|| format!("keyboard lights dark since {since}"))
    }
}

// ---- the app side: one worker, events in, one delayed check only while dark ----

#[cfg(windows)]
fn local_hm() -> Hm {
    // SAFETY: no arguments; it returns the time by value.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    Hm(t.wHour as u8, t.wMinute as u8)
}

#[cfg(not(windows))]
fn local_hm() -> Hm {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Hm(((s / 3600) % 24) as u8, ((s / 60) % 60) as u8)
}

enum Msg {
    /// Re-read whether a scene draws through a LampArray that is ours.
    Context,
    Lit {
        lit: bool,
        unpowered: bool,
    },
    Ended,
}

static TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<Msg>>> = std::sync::Mutex::new(None);
static TRACKER: std::sync::LazyLock<std::sync::Mutex<Tracker>> =
    std::sync::LazyLock::new(Default::default);

fn tracker() -> std::sync::MutexGuard<'static, Tracker> {
    TRACKER.lock().unwrap_or_else(|e| e.into_inner())
}

fn send(m: Msg) {
    let mut tx = TX.lock().unwrap_or_else(|e| e.into_inner());
    if tx.is_none() {
        let (t, rx) = std::sync::mpsc::channel();
        *tx = Some(t);
        super::services::spawn_named("lights-heal", move || run(rx));
    }
    if let Some(t) = tx.as_ref() {
        let _ = t.send(m);
    }
}

/// A scene started or ended, the holds or the owner changed, or Foxglove paused or went away.
pub fn context_changed() {
    send(Msg::Context);
}

/// The scene ended (however it did).
pub fn scene_ended() {
    send(Msg::Ended);
}

/// The keyboard reported its lights (`lit`), unpowered as it says.
pub fn lit_changed(lit: bool, unpowered: bool) {
    send(Msg::Lit { lit, unpowered });
}

/// Repaint now. An error says why not.
pub fn user_repaint() -> Result<(), String> {
    let mut t = tracker();
    if super::scene_run::running().is_none() {
        return Err("no scene is running".into());
    }
    if t.putting_back() {
        return Err("already putting the lights back".into());
    }
    if !t.dark() {
        return Err("the keyboard lights aren't dark".into());
    }
    // Applied here, so the caller sees "putting back" on return; the worker is woken to re-arm
    // its one deadline.
    let acts = t.event(Instant::now(), local_hm(), Event::UserRepaint);
    drop(t);
    execute(acts);
    send(Msg::Context);
    Ok(())
}

fn execute(acts: Vec<Act>) {
    for a in acts {
        match a {
            Act::Log(l) => eprintln!("{l}"),
            Act::Repaint => super::scene_run::repaint_lamps(),
            Act::Retake => {
                super::lamp_hold::retake();
                super::scene_run::repaint_lamps();
            }
        }
    }
}

/// Repaint now and wait (bounded, woken by the worker's events) until the step is over: whether
/// the keyboard is lit after.
pub fn repaint_and_wait(limit: Duration) -> Result<bool, String> {
    user_repaint()?;
    wait_settled(limit);
    Ok(!tracker().dark())
}

fn active_now() -> bool {
    use foxglove_link::lights_owner::Owner;
    let owner = super::lights_owner::lock(&super::lights_owner::shared()).owner();
    super::scene_run::running().is_some()
        && !super::away::away()
        && !super::pause::paused()
        && owner.is_some_and(Owner::lamp_array)
        && owner.is_some_and(super::lamp_hold::holds)
}

fn run(rx: std::sync::mpsc::Receiver<Msg>) {
    use std::sync::mpsc::RecvTimeoutError;
    loop {
        let deadline = tracker().deadline();
        let ev = match deadline {
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            },
        };
        let ev = match ev {
            None => Event::Tick,
            Some(Msg::Context) => Event::Active(active_now()),
            Some(Msg::Lit { lit, unpowered }) => Event::Lit { lit, unpowered },
            Some(Msg::Ended) => Event::Active(false),
        };
        let acts = tracker().event(Instant::now(), local_hm(), ev);
        execute(acts);
        super::scene_run::repaint_page();
        super::lights_owner::repaint();
        if tracker().failed() {
            super::health_notice::check(&super::health::all_rows());
        }
        settled();
    }
}

/// The health row now.
pub fn row_now() -> Vec<HealthRow> {
    let h = super::lamp_hold::now();
    tracker()
        .row(Instant::now(), h.usb.is_some() || h.ble.is_some())
        .into_iter()
        .collect()
}

/// What the Scenes page and the API show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub dark: bool,
    pub putting_back: bool,
    /// Past the first step: Repaint now is shown.
    pub failed: bool,
    /// Repaint now did not help: Stop is the primary button.
    pub user_failed: bool,
    pub dark_since: Option<String>,
    pub suffix: Option<String>,
    pub darks: u64,
    pub recovered: u64,
    /// The keyboard's last report (`None`: none yet).
    pub lit: Option<bool>,
    pub unpowered: bool,
    pub last_recovery: Option<String>,
    /// Local `hh:mm` of the step still to come while dark.
    pub next_try: Option<String>,
}

impl Hm {
    /// This time plus `d`, rounded down to the minute, past midnight wrapping.
    pub fn plus(self, d: Duration) -> Hm {
        let m = (self.0 as u64 * 60 + self.1 as u64 + d.as_secs() / 60) % (24 * 60);
        Hm((m / 60) as u8, (m % 60) as u8)
    }
}

impl Tracker {
    /// The view at `now` (local time `wall`).
    pub fn view(&self, now: Instant, wall: Hm) -> View {
        let next_try = self
            .dark()
            .then(|| self.deadline())
            .flatten()
            .map(|d| wall.plus(d.saturating_duration_since(now)));
        View {
            dark: self.dark(),
            putting_back: self.putting_back(),
            failed: self.failed(),
            user_failed: self.dark() && self.user_failed.is_some(),
            dark_since: self.dark_since().map(|h| h.to_string()),
            suffix: self.status_suffix(),
            darks: self.darks,
            recovered: self.recovered,
            lit: self.lit,
            unpowered: self.unpowered,
            last_recovery: self.last_recovery.map(|h| h.to_string()),
            next_try: next_try.map(|h| h.to_string()),
        }
    }
}

pub fn view() -> View {
    tracker().view(Instant::now(), local_hm())
}

/// Bumped after every event the worker handled, so a waiter wakes on a change, not on a timer.
static SETTLED: (std::sync::Mutex<u64>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

fn settled() {
    let mut n = SETTLED.0.lock().unwrap_or_else(|e| e.into_inner());
    *n += 1;
    SETTLED.1.notify_all();
}

/// Waits until no step runs (Putting back is over), at most `limit`; true when it is over.
/// Woken by the worker's events; the worker's own re-check (10 s) ends a step.
pub fn wait_settled(limit: Duration) -> bool {
    let end = Instant::now() + limit;
    let mut n = SETTLED.0.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if !tracker().putting_back() {
            return true;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        n = SETTLED
            .1
            .wait_timeout(n, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

/// The Failed notification.
pub const NOTICE: (&str, &str) = (
    "Keyboard lights stayed dark",
    "A scene is running but the keyboard shows no light. Open the Scenes page and click Repaint now.",
);

/// The muted line under a failed status.
pub const HINT: &str =
    "Repaint now sends the scene's colours again. Stop gives the keyboard its own lights back.";

#[cfg(test)]
mod tests {
    use super::*;

    const W: Hm = Hm(14, 3);
    fn lit(l: bool) -> Event {
        Event::Lit {
            lit: l,
            unpowered: false,
        }
    }
    fn rig() -> (Tracker, Instant) {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.event(now, W, Event::Active(true));
        t.event(now, W, lit(true));
        (t, now)
    }
    fn acts(v: &[Act]) -> Vec<&Act> {
        v.iter().filter(|a| !matches!(a, Act::Log(_))).collect()
    }

    #[test]
    fn no_row_before_a_scene_then_healthy() {
        let mut t = Tracker::default();
        let now = Instant::now();
        assert!(t.row(now, true).is_none());
        t.event(now, W, Event::Active(true));
        assert_eq!(t.row(now, false), None);
        let r = t.row(now, true).unwrap();
        assert_eq!(
            (r.level, r.detail.as_str()),
            (Level::Ok, "never went dark since Foxglove started")
        );
    }

    #[test]
    fn dark_under_two_minutes_changes_nothing() {
        let (mut t, now) = rig();
        t.event(now, W, lit(false));
        assert!(t.dark() && !t.putting_back());
        assert_eq!(t.deadline(), Some(now + N_DARK));
        assert!(t
            .event(now + Duration::from_secs(119), W, Event::Tick)
            .is_empty());
        t.event(now + Duration::from_secs(60), W, lit(true));
        assert_eq!(t.darks, 0);
        assert!(t.deadline().is_none());
    }

    #[test]
    fn repaint_recovers_and_says_so() {
        let (mut t, now) = rig();
        t.event(now, W, lit(false));
        let at = now + N_DARK;
        assert_eq!(
            acts(&t.event(at, Hm(14, 5), Event::Tick)),
            vec![&Act::Repaint]
        );
        assert!(t.putting_back());
        let r = t.row(at, true).unwrap();
        assert_eq!(
            (r.level, r.detail.as_str()),
            (Level::Ok, "went dark, putting the lights back now…")
        );
        assert_eq!(
            t.status_suffix().unwrap(),
            "keyboard went dark, putting the lights back…"
        );
        t.event(at + Duration::from_secs(3), Hm(14, 5), lit(true));
        let r = t.row(at, true).unwrap();
        assert_eq!(
            (r.level, r.detail.as_str()),
            (Level::Ok, "went dark once since start, put back (at 14:05)")
        );
        assert!(!t.dark());
    }

    #[test]
    fn re_take_then_give_up_then_repaint_now_fails() {
        let (mut t, now) = rig();
        t.event(now, W, lit(false));
        let a = now + N_DARK;
        t.event(a, W, Event::Tick);
        let b = a + RECHECK;
        assert_eq!(acts(&t.event(b, W, Event::Tick)), vec![&Act::Retake]);
        let c = b + RECHECK;
        assert!(acts(&t.event(c, Hm(14, 6), Event::Tick)).is_empty());
        assert!(t.failed());
        let r = t.row(c, true).unwrap();
        assert_eq!(r.level, Level::Fail);
        assert_eq!(r.detail, "dark since 14:03 and Foxglove couldn't put the lights back. Click Repaint now on the Scenes page.");
        assert_eq!(r.notice.as_ref().unwrap().0, "Keyboard lights stayed dark");
        assert_eq!(
            t.status_suffix().unwrap(),
            "keyboard lights dark since 14:03"
        );
        // the user's button
        let d = c + Duration::from_secs(5);
        assert_eq!(
            acts(&t.event(d, Hm(14, 7), Event::UserRepaint)),
            vec![&Act::Repaint]
        );
        assert!(t.putting_back());
        t.event(d + RECHECK, Hm(14, 7), Event::Tick);
        let r = t.row(d, true).unwrap();
        assert_eq!(r.detail, "dark since 14:03; Repaint now at 14:07 didn't help. Click Stop on the Scenes page to get the keyboard's own lights back.");
        assert_eq!(
            t.status_suffix().unwrap(),
            "keyboard lights still dark after Repaint now"
        );
    }

    #[test]
    fn backoff_doubles_and_a_later_try_repaints() {
        let (mut t, now) = rig();
        t.event(now, W, lit(false));
        let mut at = now + N_DARK;
        t.event(at, W, Event::Tick);
        at += RECHECK;
        t.event(at, W, Event::Tick);
        at += RECHECK;
        t.event(at, W, Event::Tick);
        assert_eq!(t.deadline(), Some(at + FIRST_BACKOFF));
        at += FIRST_BACKOFF;
        assert_eq!(acts(&t.event(at, W, Event::Tick)), vec![&Act::Repaint]);
        at += RECHECK;
        t.event(at, W, Event::Tick);
        at += RECHECK;
        t.event(at, W, Event::Tick);
        assert_eq!(t.deadline(), Some(at + FIRST_BACKOFF * 2));
    }

    #[test]
    fn repaint_now_only_while_dark() {
        let (mut t, now) = rig();
        assert!(t.event(now, W, Event::UserRepaint).is_empty());
    }

    #[test]
    fn unpowered_and_inactive_are_not_dark() {
        let (mut t, now) = rig();
        t.event(
            now,
            W,
            Event::Lit {
                lit: false,
                unpowered: true,
            },
        );
        assert!(!t.dark());
        t.event(now, W, lit(false));
        assert!(t.dark());
        t.event(now, W, Event::Active(false));
        assert!(!t.dark() && t.deadline().is_none());
    }

    #[test]
    fn three_recoveries_in_an_hour_warn() {
        let (mut t, mut now) = rig();
        for i in 0..3u64 {
            t.event(now, W, lit(false));
            now += N_DARK;
            t.event(now, W, Event::Tick);
            t.event(now, Hm(14, 10 + i as u8), lit(true));
            now += Duration::from_secs(60);
        }
        let r = t.row(now, true).unwrap();
        assert_eq!(r.level, Level::Warn);
        assert_eq!(
            r.detail,
            "went dark 3 times in the last hour, put back each time (last at 14:12)"
        );
        let r = t.row(now + Duration::from_secs(7200), true).unwrap();
        assert_eq!(r.level, Level::Ok);
        assert_eq!(
            r.detail,
            "went dark 3 times since start, put back each time (last at 14:12)"
        );
    }

    #[test]
    fn a_clock_time_plus_a_wait_wraps_past_midnight() {
        assert_eq!(Hm(14, 3).plus(Duration::from_secs(120)), Hm(14, 5));
        assert_eq!(Hm(23, 59).plus(Duration::from_secs(120)), Hm(0, 1));
    }

    #[test]
    fn the_view_names_the_time_the_next_step_runs() {
        let t0 = Instant::now();
        let mut t = Tracker::default();
        t.event(t0, Hm(14, 0), Event::Active(true));
        t.event(
            t0,
            Hm(14, 0),
            Event::Lit {
                lit: false,
                unpowered: false,
            },
        );
        let v = t.view(t0, Hm(14, 0));
        assert_eq!(v.next_try.as_deref(), Some("14:02"));
        assert_eq!(v.lit, Some(false));
    }
}
