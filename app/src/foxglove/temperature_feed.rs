//! Temperature's reading feed (the plan: review/ux-plan-gpu-temp-effect.md, slice 2). Pure
//! pieces, no pipe and no thread, so tests drive them with mock readings:
//! - [`Feed`]: what the hub's facts say now (`sensor.<group>.temp_c`, `sensor.<group>.state`,
//!   the sensors app's `module.org.foxglove.sensors.state`, whether the link is up and whether
//!   the optional grant was given) boiled down to one [`Status`], and from it what the lights do
//!   ([`Lights`]). A missing reading is never a number: only `state = ok` with a value reads.
//! - [`Subscription`]: the hub subscription to the group's facts, made and ended on events,
//!   never twice, even when the answer to the subscribe has not come back yet.
//! - [`Listener`]: the listener rule. Foxglove listens to the sensors only while Temperature is
//!   the look that lights something: the look reads, and no gate (away, paused, another app
//!   holding the lights) holds the scene. Every change is one edge to `want`; the hub counts the
//!   subscription as a listener (`facts.demand`, hub 1.5), so the PC sensors app samples while
//!   it holds one and stops when it goes. Foxglove adds no timer and no poll.
//!
//! The link ([`super::hub_link`]) owns a [`Feed`] and a [`Subscription`] per connection; the
//! running scene ([`super::scene_run`]) owns a [`Listener`]; [`want`] and [`publish`] are the two
//! one-way hand-offs between them. Nothing here is drawn or painted: the page and the lights
//! are slices 3 and 4, so what waits for them is allowed to be unused outside tests.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// The hub's fact for the PC sensors app's state (`running`, `off`, `crash-stopped`, ...).
pub const APP_STATE_KEY: &str = "module.org.foxglove.sensors.state";

/// The fact with a group's temperature, in degrees Celsius.
pub fn temp_key(group: &str) -> String {
    format!("sensor.{group}.temp_c")
}

/// The fact with a group's state: `ok`, `unavailable` or `unknown`.
pub fn state_key(group: &str) -> String {
    format!("sensor.{group}.state")
}

/// The one subscription pattern: both facts of the group.
pub fn pattern(group: &str) -> String {
    format!("sensor.{group}.*")
}

/// What the PC sensors app says about one group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SensorState {
    Ok,
    Unavailable,
    /// Not known yet (the app has not sampled), or any word this reader does not know.
    #[default]
    Unknown,
}

impl SensorState {
    fn of(v: &Value) -> SensorState {
        match v.as_str() {
            Some("ok") => SensorState::Ok,
            Some("unavailable") => SensorState::Unavailable,
            _ => SensorState::Unknown,
        }
    }
}

/// The PC sensors app, as the hub's state fact for it says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum App {
    /// Not told yet.
    #[default]
    Unknown,
    /// The hub has no such module (not installed): the fact is not there.
    Absent,
    Running,
    /// Installed and switched off.
    Off,
    /// Any other state: starting, updating, crash-stopped, ... It reads nothing now.
    Stopped,
}

impl App {
    fn of(v: &Value) -> App {
        match v.as_str() {
            Some("running") => App::Running,
            Some("off") => App::Off,
            Some(_) => App::Stopped,
            None => App::Absent,
        }
    }
}

/// What Temperature's reading is now, one row of the plan's state table (section 4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Status {
    /// There is no link to the hub (not under Hideout, or the hub is gone): no reading.
    #[default]
    HubLost,
    /// Foxglove has no grant to read the sensors.
    NotAllowed,
    /// The PC sensors app is not installed, or switched off.
    AppOff,
    /// The PC sensors app is not running (crashed, updating, ...).
    AppStopped,
    /// Subscribed, no value yet.
    Reading,
    /// A value, and the app says `ok`. `game`: game mode, a new value every 10 s.
    Working { temp_c: f32, game: bool },
    /// The reading went away (the app says `unavailable` after it had one, or its fact left).
    Missing,
    /// The card never reported (`unavailable` from the start).
    NoReport,
}

/// What the lights do for a status.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lights {
    /// The colour for this reading.
    Colour(f32),
    /// The slow breathe that says "running, no reading": never the cool colour, never the last
    /// colour held.
    Fallback,
}

impl Status {
    /// The lights for it. Only a working reading colours them.
    pub fn lights(self) -> Lights {
        match self {
            Status::Working { temp_c, .. } => Lights::Colour(temp_c),
            _ => Lights::Fallback,
        }
    }

    /// The reading, only when there is one.
    pub fn reading(self) -> Option<f32> {
        match self {
            Status::Working { temp_c, .. } => Some(temp_c),
            _ => None,
        }
    }
}

/// The facts a running Temperature reads, as the hub last said.
#[derive(Clone, Debug, PartialEq)]
pub struct Feed {
    group: String,
    temp: Option<f32>,
    state: SensorState,
    /// A reading has come since subscribing: telling "went away" from "never was".
    had: bool,
    app: App,
    connected: bool,
    granted: bool,
    game: bool,
}

impl Feed {
    pub fn new(group: &str) -> Feed {
        Feed {
            group: group.to_string(),
            temp: None,
            state: SensorState::Unknown,
            had: false,
            app: App::Unknown,
            connected: false,
            granted: false,
            game: false,
        }
    }

    pub fn group(&self) -> &str {
        &self.group
    }

    /// The link is up (`initialize` answered) or gone; `granted` says whether the hub allows
    /// reading `sensor.*`. Either way no earlier reading is kept.
    pub fn link(&mut self, connected: bool, granted: bool) {
        self.connected = connected;
        self.granted = connected && granted;
        self.clear();
    }

    /// Forgets the readings (unsubscribed, subscribing again, hub gone): the facts that come
    /// next are the truth, a value from before never is.
    pub fn clear(&mut self) {
        self.temp = None;
        self.state = SensorState::Unknown;
        self.had = false;
    }

    /// Game mode (`keyboard.game_mode`): the sensors app samples every 10 s.
    pub fn set_game(&mut self, on: bool) {
        self.game = on;
    }

    /// Follows another group (the look's `temp_source`); the readings start over.
    pub fn set_group(&mut self, group: &str) {
        if self.group != group {
            self.group = group.to_string();
            self.clear();
        }
    }

    /// One fact, or its withdrawal (`Value::Null`). Others are ignored.
    pub fn fact(&mut self, key: &str, value: &Value) {
        if key == APP_STATE_KEY {
            self.app = App::of(value);
        } else if key == temp_key(&self.group) {
            // Only a finite number reads; anything else is "no value".
            self.temp = value.as_f64().map(|t| t as f32).filter(|t| t.is_finite());
            if self.temp.is_some() {
                self.had = true;
            }
        } else if key == state_key(&self.group) {
            self.state = SensorState::of(value);
        }
    }

    /// The row of the plan's table this is.
    pub fn status(&self) -> Status {
        if !self.connected {
            return Status::HubLost;
        }
        if !self.granted {
            return Status::NotAllowed;
        }
        match self.app {
            App::Absent | App::Off => return Status::AppOff,
            App::Stopped => return Status::AppStopped,
            App::Running | App::Unknown => {}
        }
        match (self.state, self.temp) {
            // The value counts only while the app says it is good.
            (SensorState::Ok, Some(temp_c)) => Status::Working {
                temp_c,
                game: self.game,
            },
            (SensorState::Unavailable, _) => {
                if self.had {
                    Status::Missing
                } else {
                    Status::NoReport
                }
            }
            // Ok but the value left: gone after a reading, not here yet before one.
            (SensorState::Ok, None) if self.had => Status::Missing,
            _ => Status::Reading,
        }
    }
}

/// What the hub subscription to the sensors is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Asked (the request's id); the hub has not answered.
    Pending(i64),
    /// Asked, then no longer wanted before the answer: ended the moment it comes.
    Cancelling(i64),
    /// The hub's number for it.
    Held(u64),
}

/// What to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// `facts.subscribe` the group's pattern (then list it, for the values now); report the
    /// request's id through [`Subscription::sent`].
    Subscribe,
    /// `facts.unsubscribe` this subscription.
    Unsubscribe(u64),
}

/// The subscription, made when wanted and ended when not, once each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    phase: Phase,
}

impl Default for Subscription {
    fn default() -> Self {
        Subscription { phase: Phase::Idle }
    }
}

impl Subscription {
    /// The wish changed (or is told again: nothing is sent twice).
    pub fn want(&mut self, on: bool) -> Option<Act> {
        match (on, self.phase) {
            (true, Phase::Idle) => {
                // The id is filled in by `sent`, straight after.
                self.phase = Phase::Pending(-1);
                Some(Act::Subscribe)
            }
            (true, Phase::Cancelling(id)) => {
                self.phase = Phase::Pending(id);
                None
            }
            (false, Phase::Held(n)) => {
                self.phase = Phase::Idle;
                Some(Act::Unsubscribe(n))
            }
            (false, Phase::Pending(id)) => {
                self.phase = Phase::Cancelling(id);
                None
            }
            _ => None,
        }
    }

    /// The id the subscribe request got.
    pub fn sent(&mut self, id: i64) {
        if let Phase::Pending(p) | Phase::Cancelling(p) = &mut self.phase {
            *p = id;
        }
    }

    /// Whether `id` is the subscribe request in flight.
    pub fn owns(&self, id: i64) -> bool {
        matches!(self.phase, Phase::Pending(p) | Phase::Cancelling(p) if p == id)
    }

    /// The hub's answer to the subscribe: its number, or `None` when it refused.
    pub fn answered(&mut self, number: Option<u64>) -> Option<Act> {
        match (self.phase, number) {
            (Phase::Pending(_), Some(n)) => {
                self.phase = Phase::Held(n);
                None
            }
            (Phase::Cancelling(_), Some(n)) => {
                self.phase = Phase::Idle;
                Some(Act::Unsubscribe(n))
            }
            (_, None) => {
                self.phase = Phase::Idle;
                None
            }
            _ => None,
        }
    }

    /// The hub said Ok but gave no subscription number (it always does today): nothing to hold
    /// and nothing to end, so it is a no-op, not a refusal (no refusal line, no retry).
    pub fn answered_without_number(&mut self) {
        if matches!(self.phase, Phase::Pending(_) | Phase::Cancelling(_)) {
            self.phase = Phase::Idle;
        }
    }

    /// Facts of the group may arrive: asked and not ended.
    pub fn listening(&self) -> bool {
        matches!(self.phase, Phase::Pending(_) | Phase::Held(_))
    }

    /// Nothing asked, nothing held.
    pub fn idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    pub fn held(&self) -> bool {
        matches!(self.phase, Phase::Held(_))
    }
}

/// The scene's gates, as far as listening goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    /// The user is away: locked, or the display is off.
    pub away: bool,
    /// Foxglove is paused.
    pub paused: bool,
    /// Another app holds the keyboard's lights.
    pub held: bool,
}

/// The listener rule: listening exactly while the look reads and nothing holds the scene.
pub struct Listener {
    reads: bool,
    gates: Gates,
    on: bool,
    tell: Box<dyn FnMut(bool) + Send>,
}

impl Listener {
    /// `tell` hears each change of the wish, and only changes.
    pub fn new(tell: Box<dyn FnMut(bool) + Send>) -> Listener {
        Listener {
            reads: false,
            gates: Gates::default(),
            on: false,
            tell,
        }
    }

    /// The scene's own, telling [`want`] under its own number, so a scene that ends cannot
    /// take away what a newer scene wants (the old scene's drop only ends its own wish).
    pub fn for_scene() -> Listener {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        Listener::new(Box::new(move |on| want(id, on)))
    }

    /// The look is Temperature (or is not). A scene's look change clears it first: the page
    /// says Temperature again after it.
    pub fn set_reads(&mut self, reads: bool) {
        self.reads = reads;
        self.sync();
    }

    pub fn set_gates(&mut self, gates: Gates) {
        self.gates = gates;
        self.sync();
    }

    pub fn listening(&self) -> bool {
        self.on
    }

    fn sync(&mut self) {
        let g = self.gates;
        let wanted = self.reads && !g.away && !g.paused && !g.held;
        if wanted != self.on {
            self.on = wanted;
            (self.tell)(wanted);
        }
    }
}

impl Drop for Listener {
    /// The scene ended, however it ended: listening ends with it.
    fn drop(&mut self) {
        self.reads = false;
        self.sync();
    }
}

// ---- the two hand-offs between the scene and the link ----

/// The scenes that want the sensors now, by their number: one per running scene, so the wish
/// is "any scene wants it", not one flag two scenes overwrite.
static WANTS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
type Wake = Arc<dyn Fn() + Send + Sync>;
static ON_WANT: Mutex<Option<Wake>> = Mutex::new(None);
static LATEST: Mutex<Status> = Mutex::new(Status::HubLost);
static SOURCE: Mutex<String> = Mutex::new(String::new());

/// A sensor group as a plain name: letters, digits, `_` and `-`, so it can only ever make
/// `sensor.<name>.*` and never a pattern that leaves the group (`*`, `a.b`, empty).
pub fn valid_source(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= 32
        && group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The group the running look follows (`temp_source`): a valid name, else the default. The link
/// reads it after every message.
pub fn set_source(group: &str) {
    let group = if valid_source(group) {
        group
    } else {
        super::temperature::DEFAULT_SOURCE
    };
    *SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = group.to_string();
}

/// The group to follow now.
pub fn source() -> String {
    let g = SOURCE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if valid_source(&g) {
        g
    } else {
        super::temperature::DEFAULT_SOURCE.to_string()
    }
}

/// One scene's wish: listen to the sensors, or not. Tells the link when the wish of all scenes
/// changes.
pub fn want(scene: u64, on: bool) {
    let (before, after) = {
        let mut w = WANTS.lock().unwrap_or_else(|e| e.into_inner());
        let before = !w.is_empty();
        w.retain(|s| *s != scene);
        if on {
            w.push(scene);
        }
        (before, !w.is_empty())
    };
    if before != after {
        let wake = ON_WANT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(wake) = wake {
            wake();
        }
    }
}

/// Whether any scene wants the sensors now.
pub fn wanted() -> bool {
    !WANTS.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

/// The link's ear for a change of the wish (it sends itself a message).
pub fn on_want_change(wake: impl Fn() + Send + Sync + 'static) {
    *ON_WANT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(wake));
}

/// The link's word on the reading, for the page and the lights.
pub fn publish(status: Status) {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = status;
}

/// The status the link last published.
pub fn latest() -> Status {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn up() -> Feed {
        let mut f = Feed::new("gpu");
        f.link(true, true);
        f.fact(APP_STATE_KEY, &json!("running"));
        f
    }

    fn set(f: &mut Feed, state: &str, temp: Option<f64>) {
        f.fact("sensor.gpu.state", &json!(state));
        f.fact("sensor.gpu.temp_c", &temp.map_or(Value::Null, |t| json!(t)));
    }

    #[test]
    fn the_keys_are_the_groups() {
        assert_eq!(temp_key("gpu"), "sensor.gpu.temp_c");
        assert_eq!(state_key("gpu"), "sensor.gpu.state");
        assert_eq!(pattern("gpu"), "sensor.gpu.*");
    }

    #[test]
    fn unknown_then_ok_then_unavailable_then_ok() {
        let mut f = up();
        assert_eq!(f.status(), Status::Reading);
        assert_eq!(f.status().lights(), Lights::Fallback);
        set(&mut f, "unknown", None);
        assert_eq!(f.status(), Status::Reading);
        set(&mut f, "ok", Some(64.0));
        assert_eq!(
            f.status(),
            Status::Working {
                temp_c: 64.0,
                game: false
            }
        );
        assert_eq!(f.status().lights(), Lights::Colour(64.0));
        set(&mut f, "unavailable", None);
        assert_eq!(f.status(), Status::Missing);
        assert_eq!(f.status().lights(), Lights::Fallback);
        assert_eq!(f.status().reading(), None);
        set(&mut f, "ok", Some(51.0));
        assert_eq!(
            f.status(),
            Status::Working {
                temp_c: 51.0,
                game: false
            }
        );
    }

    #[test]
    fn a_card_that_never_reports_is_its_own_row() {
        let mut f = up();
        set(&mut f, "unavailable", None);
        assert_eq!(f.status(), Status::NoReport);
        assert_eq!(f.status().lights(), Lights::Fallback);
    }

    #[test]
    fn a_stale_value_never_reads_without_ok() {
        let mut f = up();
        set(&mut f, "ok", Some(70.0));
        // The state turns to unavailable while the old value stays in the fact: no reading.
        f.fact("sensor.gpu.state", &json!("unavailable"));
        assert_eq!(f.status(), Status::Missing);
        f.fact("sensor.gpu.state", &json!("unknown"));
        assert_eq!(f.status(), Status::Reading);
        assert_eq!(f.status().reading(), None);
    }

    #[test]
    fn a_withdrawn_fact_is_missing_not_zero() {
        let mut f = up();
        set(&mut f, "ok", Some(66.0));
        f.fact("sensor.gpu.temp_c", &Value::Null);
        assert_eq!(f.status(), Status::Missing);
        assert_eq!(f.status().reading(), None);
        // Not a number is no value either.
        f.fact("sensor.gpu.temp_c", &json!("hot"));
        assert_eq!(f.status(), Status::Missing);
        f.fact("sensor.gpu.temp_c", &json!(null));
        assert_eq!(f.status(), Status::Missing);
    }

    #[test]
    fn ok_before_the_value_has_come_is_still_reading() {
        let mut f = up();
        f.fact("sensor.gpu.state", &json!("ok"));
        assert_eq!(f.status(), Status::Reading);
    }

    #[test]
    fn the_sensors_app_state_rows() {
        let mut f = up();
        set(&mut f, "ok", Some(60.0));
        for (state, want) in [
            ("crash-stopped", Status::AppStopped),
            ("updating", Status::AppStopped),
            ("starting", Status::AppStopped),
            ("off", Status::AppOff),
        ] {
            f.fact(APP_STATE_KEY, &json!(state));
            assert_eq!(f.status(), want, "{state}");
            assert_eq!(f.status().lights(), Lights::Fallback);
        }
        // Its fact left: not installed.
        f.fact(APP_STATE_KEY, &Value::Null);
        assert_eq!(f.status(), Status::AppOff);
        f.fact(APP_STATE_KEY, &json!("running"));
        assert_eq!(
            f.status(),
            Status::Working {
                temp_c: 60.0,
                game: false
            }
        );
    }

    #[test]
    fn hub_lost_and_not_allowed() {
        let mut f = Feed::new("gpu");
        assert_eq!(f.status(), Status::HubLost);
        f.link(true, false);
        f.fact(APP_STATE_KEY, &json!("running"));
        assert_eq!(f.status(), Status::NotAllowed);
        f.link(true, true);
        f.fact(APP_STATE_KEY, &json!("running"));
        set(&mut f, "ok", Some(55.0));
        assert_eq!(
            f.status(),
            Status::Working {
                temp_c: 55.0,
                game: false
            }
        );
        // The hub goes: no value is kept for the next connection.
        f.link(false, false);
        assert_eq!(f.status(), Status::HubLost);
        f.link(true, true);
        f.fact(APP_STATE_KEY, &json!("running"));
        assert_eq!(f.status(), Status::Reading);
    }

    #[test]
    fn game_mode_is_said_not_lit() {
        let mut f = up();
        set(&mut f, "ok", Some(72.0));
        f.set_game(true);
        assert_eq!(
            f.status(),
            Status::Working {
                temp_c: 72.0,
                game: true
            }
        );
        assert_eq!(f.status().lights(), Lights::Colour(72.0));
    }

    #[test]
    fn facts_of_other_groups_are_ignored_and_a_new_group_starts_over() {
        let mut f = up();
        f.fact("sensor.cpu.state", &json!("ok"));
        f.fact("sensor.cpu.temp_c", &json!(50.0));
        assert_eq!(f.status(), Status::Reading);
        set(&mut f, "ok", Some(40.0));
        f.set_group("cpu");
        assert_eq!(f.status(), Status::Reading);
        f.fact("sensor.cpu.state", &json!("ok"));
        f.fact("sensor.cpu.temp_c", &json!(50.0));
        assert_eq!(
            f.status(),
            Status::Working {
                temp_c: 50.0,
                game: false
            }
        );
    }

    /// Property: whatever sequence of facts, links and withdrawals, the lights take a colour
    /// only from a value that came with `ok` and is still there, and a missing reading never
    /// reads as a number (0 included) and never as the cool end.
    #[test]
    fn random_sequences_never_make_a_reading_up() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..300 {
            let mut f = Feed::new("gpu");
            // What the model says is true now.
            let (mut up, mut granted, mut app) = (false, false, "running");
            let (mut state, mut temp): (&str, Option<f64>) = ("unknown", None);
            for _ in 0..60 {
                match next(8) {
                    0 => {
                        up = next(4) != 0;
                        granted = next(3) != 0;
                        f.link(up, granted);
                        (state, temp) = ("unknown", None);
                        f.fact(APP_STATE_KEY, &json!(app));
                    }
                    1 => {
                        state = ["ok", "unavailable", "unknown", "weird"][next(4) as usize];
                        f.fact("sensor.gpu.state", &json!(state));
                    }
                    2 => {
                        temp = Some(next(120) as f64);
                        f.fact("sensor.gpu.temp_c", &json!(temp));
                    }
                    3 => {
                        temp = None;
                        f.fact("sensor.gpu.temp_c", &Value::Null);
                    }
                    4 => {
                        app = ["running", "off", "crash-stopped", "starting"][next(4) as usize];
                        f.fact(APP_STATE_KEY, &json!(app));
                    }
                    5 => f.set_game(next(2) == 0),
                    6 => {
                        f.clear();
                        (state, temp) = ("unknown", None);
                    }
                    _ => {}
                }
                let status = f.status();
                match status.lights() {
                    Lights::Colour(t) => {
                        assert!(up && granted && app == "running" && state == "ok");
                        assert_eq!(Some(f64::from(t)), temp, "a colour only for a live value");
                    }
                    Lights::Fallback => assert_eq!(status.reading(), None),
                }
                if !(up && granted && app == "running" && state == "ok" && temp.is_some()) {
                    assert_eq!(status.reading(), None, "{status:?}");
                }
            }
        }
    }

    #[test]
    fn subscribes_once_and_unsubscribes_once() {
        let mut s = Subscription::default();
        assert_eq!(s.want(false), None);
        assert_eq!(s.want(true), Some(Act::Subscribe));
        s.sent(7);
        assert_eq!(s.want(true), None, "wanting again sends nothing");
        assert!(s.owns(7) && !s.owns(8));
        assert!(s.listening() && !s.held());
        assert_eq!(s.answered(Some(41)), None);
        assert!(s.held());
        assert_eq!(s.want(true), None);
        assert_eq!(s.want(false), Some(Act::Unsubscribe(41)));
        assert_eq!(s.want(false), None);
        assert!(!s.listening());
        // And again later: a fresh subscribe.
        assert_eq!(s.want(true), Some(Act::Subscribe));
    }

    #[test]
    fn ended_before_the_answer_ends_when_it_comes() {
        let mut s = Subscription::default();
        assert_eq!(s.want(true), Some(Act::Subscribe));
        s.sent(3);
        assert_eq!(s.want(false), None);
        assert!(!s.listening());
        assert!(s.owns(3));
        assert_eq!(s.answered(Some(9)), Some(Act::Unsubscribe(9)));
        assert!(!s.listening() && !s.held());
        // Wanted again before the answer: kept, not sent twice.
        let mut s = Subscription::default();
        s.want(true);
        s.sent(4);
        s.want(false);
        assert_eq!(s.want(true), None);
        assert_eq!(s.answered(Some(10)), None);
        assert!(s.held());
    }

    #[test]
    fn a_refused_subscribe_leaves_nothing_held() {
        let mut s = Subscription::default();
        s.want(true);
        s.sent(1);
        assert_eq!(s.answered(None), None);
        assert!(!s.listening());
        // No retry loop: the next wish (an event) asks again.
        assert_eq!(s.want(false), None);
        assert_eq!(s.want(true), Some(Act::Subscribe));
    }

    #[test]
    fn an_ok_answer_without_a_number_is_a_no_op_not_a_refusal() {
        let mut s = Subscription::default();
        s.want(true);
        s.sent(1);
        s.answered_without_number();
        assert!(!s.listening() && !s.held());
        // Nothing stuck: the next wish asks again, once.
        assert_eq!(s.want(true), Some(Act::Subscribe));
        // Ended before the answer, then an unnumbered answer: nothing to end.
        s.sent(2);
        assert_eq!(s.want(false), None);
        s.answered_without_number();
        assert_eq!(s.want(false), None);
        // A held one is not touched by a stray unnumbered answer.
        let mut h = Subscription::default();
        h.want(true);
        h.sent(3);
        h.answered(Some(5));
        h.answered_without_number();
        assert!(h.held());
    }

    #[test]
    fn a_source_is_a_plain_name() {
        for ok in ["gpu", "cpu", "gpu_2", "Fan-1", "a", "9"] {
            assert!(valid_source(ok), "{ok}");
        }
        let long = "a".repeat(33);
        for bad in [
            "", "*", "a.b", "a.*", "gpu ", "g/u", "gpu\n", "\u{e9}", "a*", &long,
        ] {
            assert!(!valid_source(bad), "{bad:?}");
        }
    }

    type Log = Arc<Mutex<Vec<bool>>>;

    fn listener() -> (Listener, Log) {
        let log: Log = Arc::default();
        let l2 = log.clone();
        (
            Listener::new(Box::new(move |on| l2.lock().unwrap().push(on))),
            log,
        )
    }

    fn told(log: &Log) -> Vec<bool> {
        log.lock().unwrap().clone()
    }

    #[test]
    fn nothing_listens_until_the_look_reads() {
        let (mut l, log) = listener();
        // Opening the page, picking the pill: nothing reaches the scene, and gates alone
        // listen to nothing.
        l.set_gates(Gates::default());
        l.set_gates(Gates {
            away: true,
            ..Gates::default()
        });
        l.set_gates(Gates::default());
        assert!(told(&log).is_empty());
        l.set_reads(true);
        assert_eq!(told(&log), [true]);
        l.set_reads(true);
        assert_eq!(told(&log), [true], "said once");
    }

    #[test]
    fn every_gate_ends_it_exactly_once() {
        for gate in [
            Gates {
                away: true,
                ..Gates::default()
            },
            Gates {
                paused: true,
                ..Gates::default()
            },
            Gates {
                held: true,
                ..Gates::default()
            },
        ] {
            let (mut l, log) = listener();
            l.set_reads(true);
            l.set_gates(gate);
            l.set_gates(gate);
            assert_eq!(told(&log), [true, false], "{gate:?}");
            assert!(!l.listening());
            // The gate opens: listening again, once.
            l.set_gates(Gates::default());
            assert_eq!(told(&log), [true, false, true], "{gate:?}");
        }
        // Two gates at once: ends once, and returns only when both are open.
        let (mut l, log) = listener();
        l.set_reads(true);
        l.set_gates(Gates {
            away: true,
            paused: true,
            held: false,
        });
        l.set_gates(Gates {
            away: false,
            paused: true,
            held: false,
        });
        assert_eq!(told(&log), [true, false]);
        l.set_gates(Gates::default());
        assert_eq!(told(&log), [true, false, true]);
    }

    #[test]
    fn switching_the_look_stopping_and_a_profile_switch_end_it_once() {
        // Switching to another effect, or a profile switching the look: reads goes false.
        let (mut l, log) = listener();
        l.set_reads(true);
        l.set_reads(false);
        l.set_reads(false);
        assert_eq!(told(&log), [true, false]);
        // Stop (or any end of the scene): the listener drops.
        let (mut l, log) = listener();
        l.set_reads(true);
        drop(l);
        assert_eq!(told(&log), [true, false]);
        // A scene that never listened says nothing when it ends.
        let (l, log) = listener();
        drop(l);
        assert!(told(&log).is_empty());
        // Gated when it ends: already ended, not ended twice.
        let (mut l, log) = listener();
        l.set_reads(true);
        l.set_gates(Gates {
            paused: true,
            ..Gates::default()
        });
        drop(l);
        assert_eq!(told(&log), [true, false]);
    }

    #[test]
    fn the_wish_is_per_scene_with_one_wake_per_change() {
        // The only test that touches the globals.
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woken.clone();
        on_want_change(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!wanted(), "off until the effect runs");
        want(1, false);
        assert_eq!(woken.load(Ordering::SeqCst), 0);
        want(1, true);
        want(1, true);
        assert!(wanted());
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        // A new scene wants it before the old one ends: the old one's end takes nothing away.
        want(2, true);
        want(1, false);
        assert!(wanted(), "the new scene still wants the readings");
        assert_eq!(woken.load(Ordering::SeqCst), 1, "no flap between scenes");
        want(2, false);
        assert!(!wanted());
        assert_eq!(woken.load(Ordering::SeqCst), 2);
        // The followed group is a plain name, else the default.
        set_source("cpu");
        assert_eq!(source(), "cpu");
        set_source("*");
        assert_eq!(source(), "gpu");
        set_source("a.b");
        assert_eq!(source(), "gpu");
        publish(Status::Reading);
        assert_eq!(latest(), Status::Reading);
        publish(Status::HubLost);
        *ON_WANT.lock().unwrap() = None;
    }
}
