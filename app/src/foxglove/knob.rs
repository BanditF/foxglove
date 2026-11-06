//! The knob (#719): the keyboard's encoder points at master volume or at one app's.
//!
//! The keyboard's `&knob` / `&knob_press` (firmware widgets/knob.h) are the volume keys and mute
//! on their own, so master volume is Windows' own and works without Foxglove. While Foxglove runs
//! unpaused it holds the knob (D7 14 KNOB, HELLO bit 31; `foxglove_link::protocol::knob`), as it
//! holds the art keys:
//!
//! - **click:** the next app with an active audio session that has played sound during this run
//!   (#854: a background helper holding a silent stream isn't offered), in the order first heard;
//!   past the last one, back to master;
//! - **turn:** the target's level, 2 % a detent: an app's through Windows' own session volume
//!   (ISimpleAudioVolume, the Volume Mixer's); master's through the default output device's
//!   IAudioEndpointVolume, unmuting it as Windows' volume keys do (#855);
//! - **double click:** mutes or unmutes the target;
//! - **hold** (half a second): back to master.
//!
//! Master volume (#855): firmware with HELLO v2's `KNOB_VOLUME` (word 2's bit 35, D7 16) lets
//! Foxglove hold master volume too, a held mode of its own: then master's turns and double click
//! come here, Foxglove sets the level and mute and shows its own card (`knob_popup.rs`), and
//! Windows shows no volume flyout (no volume key went). Without it (older firmware, Foxglove
//! paused or quit, or killed and its hold lease lapsed, #847) master is the keyboard's volume
//! keys and mute, and Windows' own flyout, as before.
//!
//! Nothing is kept: the target lives as long as its session. When the session expires or
//! disconnects the knob goes back to master and the keyboard is told. No hooks, no system
//! volume keys intercepted. Polling: none. Start events: a link's connect, a knob event from the
//! keyboard, a session change from Windows, pause/resume, a lapsed hold lease (#847,
//! `hold_lease.rs`), quit. Each sends at most one report;
//! the thread sleeps on its channel between them.
//!
//! Which sessions have played (#854): Windows has no event for sound, so the knob reads the
//! sessions' peak meters, and only in its per-app mode: once on each click, and while an app is
//! the target every [`LISTEN_EVERY`] for at most [`LISTEN_FOR`] after the last knob event (so a
//! session that starts playing meanwhile is there on the next click). On master: no reads.

use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use foxglove_link::hello_v2::capabilities as caps2;
use foxglove_link::link::{Event, EventSubscription, Link};
use foxglove_link::protocol::knob::{self as wire, EventKind, Target as WireTarget};
use foxglove_link::protocol::{self, capabilities};
use foxglove_link::Transport;
use foxglove_panel::sessions::{Change, Heard, Sessions};

use super::knob_health::{self, LastLevel};
use super::knob_popup::{self, Card};

/// The app the knob points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// The session instance id (`foxglove_panel::sessions::Session::id`).
    pub id: String,
    pub name: String,
}

/// What the keyboard must be told after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tell {
    Nothing,
    /// Hold the knob with this target (the turns go to the PC only on an app).
    Target(WireTarget),
}

/// The knob's target and what each event does with it, over Windows' sessions (or a test's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cycle {
    /// `None`: master.
    pub target: Option<App>,
    /// The last level (or mute) set for an app, for the health view.
    pub last_level: Option<LastLevel>,
    /// Why the knob last went back to master on its own.
    pub last_back: Option<String>,
    /// The target's level and mute as last read or set (the left screen's line).
    pub level: Option<(u8, bool)>,
    /// The popup the last event calls for, and for how long (`knob_popup.rs`); the service
    /// takes it.
    pub popup: Option<(Card, Duration)>,
    /// The sessions heard during this run: the only ones a click offers (#854).
    pub heard: Heard,
    /// The keyboard sends master's turns and double click here (Foxglove holds master volume,
    /// D7 16, #855); `false`: on master it sends the volume keys and mute itself.
    pub volume: bool,
}

/// How often the knob reads the peaks while an app is its target.
pub const LISTEN_EVERY: Duration = Duration::from_millis(250);
/// For how long after the last knob event it keeps reading them.
pub const LISTEN_FOR: Duration = Duration::from_secs(30);

impl Cycle {
    /// The target as the keyboard knows it.
    pub fn wire(&self) -> WireTarget {
        if self.target.is_some() {
            WireTarget::App
        } else {
            WireTarget::Master
        }
    }

    /// Sets the target; what to tell the keyboard (only a change between master and an app).
    fn set(&mut self, target: Option<App>) -> Tell {
        let was = self.wire();
        if self.target.as_ref().map(|a| &a.id) != target.as_ref().map(|a| &a.id) {
            self.level = None;
        }
        self.target = target;
        if self.wire() == was {
            Tell::Nothing
        } else {
            Tell::Target(self.wire())
        }
    }

    /// Notes the sessions playing now (one peak read each).
    pub fn listen(&mut self, sessions: &dyn Sessions) {
        match sessions.peaks() {
            Ok(peaks) => {
                self.heard.note(&peaks);
            }
            Err(e) => eprintln!("Foxglove: knob: no audio peaks: {e}"),
        }
    }

    /// A click: the next heard active session after the target, in the order first heard; the
    /// first from master; master again after the last (and when nothing plays).
    pub fn click(&mut self, sessions: &dyn Sessions) -> Tell {
        self.listen(sessions);
        let list = match sessions.active() {
            Ok(list) => self.heard.offer(list),
            Err(e) => {
                eprintln!("Foxglove: knob: no audio sessions: {e}");
                return Tell::Nothing;
            }
        };
        let at = match &self.target {
            None => 0,
            Some(t) => list.iter().position(|s| s.id == t.id).map_or(0, |i| i + 1),
        };
        let next = list.get(at).map(|s| App {
            id: s.id.clone(),
            name: s.name.clone(),
        });
        eprintln!(
            "Foxglove: knob: {}",
            next.as_ref()
                .map_or("master".to_string(), |a| a.name.clone())
        );
        let was_app = self.target.is_some();
        let tell = self.set(next);
        self.popup = Some(match &self.target {
            Some(app) => {
                self.level = sessions.level(&app.id).ok();
                (
                    Card::App {
                        name: app.name.clone(),
                        place: Some((at + 1, list.len())),
                        level: self.level.map(|l| l.0),
                        muted: self.level.is_some_and(|l| l.1),
                    },
                    knob_popup::CLICK,
                )
            }
            None => (
                Card::Master {
                    level: sessions.master().ok().map(|m| m.0),
                    note: if was_app {
                        "back to master"
                    } else {
                        "no app playing sound"
                    },
                    muted: false,
                },
                knob_popup::MASTER,
            ),
        });
        tell
    }

    /// The card for the target now (after a turn or a mute).
    fn app_card(&mut self, sessions: &dyn Sessions, time: Duration) {
        let Some(app) = &self.target else {
            return;
        };
        if let Ok(l) = sessions.level(&app.id) {
            self.level = Some(l);
        }
        self.popup = Some((
            Card::App {
                name: app.name.clone(),
                place: None,
                level: self.level.map(|l| l.0),
                muted: self.level.is_some_and(|l| l.1),
            },
            time,
        ));
    }

    /// Master's card after a turn or a mute (#855): its level and mute now.
    fn master_card(&mut self, sessions: &dyn Sessions, time: Duration) {
        let now = sessions.master().ok();
        self.popup = Some((
            Card::Master {
                level: now.map(|m| m.0),
                note: "",
                muted: now.is_some_and(|m| m.1),
            },
            time,
        ));
    }

    /// Detents (+ clockwise) on master while Foxglove holds master volume (#855): its level, 2 %
    /// each, unmuted as Windows' volume keys do.
    fn master_turn(&mut self, sessions: &dyn Sessions, steps: i8) {
        let last = match sessions.step_master(i32::from(steps)) {
            Ok(percent) => LastLevel::Set {
                app: MASTER.into(),
                percent,
            },
            Err(why) => {
                eprintln!("Foxglove: knob: master's level: {why}");
                LastLevel::Failed {
                    app: MASTER.into(),
                    why,
                }
            }
        };
        let ok = matches!(last, LastLevel::Set { .. });
        self.last_level = Some(last);
        if ok {
            self.master_card(sessions, knob_popup::TURN);
        }
    }

    /// Detents (+ clockwise): the target's level, 2 % each.
    pub fn turn(&mut self, sessions: &dyn Sessions, steps: i8) {
        let Some(app) = &self.target else {
            if self.volume {
                self.master_turn(sessions, steps);
            }
            // Else master is the keyboard's volume keys: a turn here came by mistake.
            return;
        };
        let last = match sessions.step(&app.id, i32::from(steps)) {
            Ok(percent) => LastLevel::Set {
                app: app.name.clone(),
                percent,
            },
            Err(why) => {
                eprintln!("Foxglove: knob: {}'s level: {why}", app.name);
                LastLevel::Failed {
                    app: app.name.clone(),
                    why,
                }
            }
        };
        let ok = matches!(last, LastLevel::Set { .. });
        self.last_level = Some(last);
        if ok {
            self.app_card(sessions, knob_popup::TURN);
        }
    }

    /// A double click: mutes or unmutes the target. On master without master volume the
    /// keyboard tapped the system mute itself.
    pub fn double(&mut self, sessions: &dyn Sessions) {
        let Some(app) = &self.target else {
            if self.volume {
                let last = match sessions.toggle_master_mute() {
                    Ok(muted) => LastLevel::Muted {
                        app: MASTER.into(),
                        muted,
                    },
                    Err(why) => LastLevel::Failed {
                        app: MASTER.into(),
                        why,
                    },
                };
                let ok = matches!(last, LastLevel::Muted { .. });
                self.last_level = Some(last);
                if ok {
                    self.master_card(sessions, knob_popup::MUTE);
                }
            }
            return;
        };
        let last = match sessions.toggle_mute(&app.id) {
            Ok(muted) => LastLevel::Muted {
                app: app.name.clone(),
                muted,
            },
            Err(why) => LastLevel::Failed {
                app: app.name.clone(),
                why,
            },
        };
        let ok = matches!(last, LastLevel::Muted { .. });
        self.last_level = Some(last);
        if ok {
            self.app_card(sessions, knob_popup::MUTE);
        }
    }

    /// A hold: back to master.
    pub fn hold(&mut self, sessions: &dyn Sessions) -> Tell {
        self.popup = Some((
            Card::Master {
                level: sessions.master().ok().map(|m| m.0),
                note: "held: master",
                muted: false,
            },
            knob_popup::MASTER,
        ));
        self.set(None)
    }

    /// Windows says session `id` expired or disconnected: if it was the target, back to master.
    pub fn gone(&mut self, id: &str) -> Tell {
        self.heard.forget(id);
        let Some(app) = self.target.as_ref().filter(|a| a.id == id) else {
            return Tell::Nothing;
        };
        let why = format!("{}'s sound ended", app.name);
        eprintln!("Foxglove: knob: {why}; back to master");
        self.popup = Some((
            Card::Stopped {
                name: app.name.clone(),
            },
            knob_popup::STOPPED,
        ));
        self.last_back = Some(why);
        self.set(None)
    }

    /// The keyboard dropped the hold (an endpoint change, a disconnect) or Foxglove gave it
    /// back: master, nothing to tell.
    pub fn dropped(&mut self) {
        self.set(None);
    }

    /// One knob event from the keyboard; what to tell it.
    pub fn event(&mut self, sessions: &dyn Sessions, kind: EventKind) -> Tell {
        match kind {
            EventKind::Turn(steps) => {
                self.turn(sessions, steps);
                Tell::Nothing
            }
            EventKind::Click => self.click(sessions),
            EventKind::Double => {
                self.double(sessions);
                Tell::Nothing
            }
            EventKind::Hold => self.hold(sessions),
            EventKind::HandedBack => {
                self.dropped();
                Tell::Nothing
            }
        }
    }

    /// Master's level or mute changed while the knob is held on master: Windows shows its own
    /// flyout, and Foxglove only the tab on top of it. Not while Foxglove holds master volume
    /// (#855): its own steps show its own card (and aren't reported here), and the tab would
    /// replace that card.
    pub fn master_changed(&mut self, held: bool) {
        if held && self.target.is_none() && !self.volume {
            self.popup = Some((Card::MasterTab, knob_popup::TAB));
        }
    }

    /// What the left screen's volume line says: the target's name, level and mute (`None`:
    /// master, the plain volume bar).
    pub fn panel_line(&self) -> Option<(String, u8, bool)> {
        let app = self.target.as_ref()?;
        let (level, muted) = self.level.unwrap_or((0, false));
        Some((app.name.clone(), level, muted))
    }

    /// Puts what this knows into the health view.
    fn note_health(&self) {
        let (target, last_level, last_back) = (
            self.target.as_ref().map(|a| a.name.clone()),
            self.last_level.clone(),
            self.last_back.clone(),
        );
        knob_health::note(|h| {
            h.target = target;
            h.last_level = last_level;
            h.last_back = last_back;
        });
    }
}

/// The name master's level goes by in the health view.
const MASTER: &str = "master volume";

/// Whether the keyboard on `link` has the knob (HELLO bit 31).
fn has_knob(link: &Link) -> bool {
    link.state()
        .connected
        .and_then(|c| c.hello)
        .is_some_and(|h| h.capabilities & capabilities::KNOB != 0)
}

/// Whether the keyboard on `link` lets Foxglove hold master volume (#855): HELLO v2's word 2,
/// bit 35 (`KNOB_VOLUME`). Firmware before it is never sent D7 16.
fn has_master_volume(link: &Link) -> bool {
    let Some(hello) = link.state().connected.and_then(|c| c.hello) else {
        return false;
    };
    foxglove_link::hello_v2::read(link, &hello).word2() & caps2::KNOB_VOLUME != 0
}

/// The report that puts `cycle`'s target on the keyboard, or hands the knob back.
fn report(held: bool, target: WireTarget) -> Vec<u8> {
    if held {
        wire::hold(target)
    } else {
        wire::release()
    }
}

enum Work {
    Link(Arc<Link>),
    Connected(Weak<Link>),
    Disconnected(Weak<Link>),
    Report(Weak<Link>, foxglove_link::link::Report),
    Sessions(Change),
    Pause,
    /// The keyboard let the knob go: Foxglove's hold lease lapsed (#847).
    Lapsed(Weak<Link>),
}

static WORK: Mutex<Option<mpsc::Sender<Work>>> = Mutex::new(None);
/// The link Foxglove holds the knob on, for [`stop`] to hand it back.
static HELD: Mutex<Option<Weak<Link>>> = Mutex::new(None);
/// The link Foxglove holds master volume on (#855), for [`stop`] to hand it back.
static VOLUME_HELD: Mutex<Option<Weak<Link>>> = Mutex::new(None);

fn held_link(link: Option<Weak<Link>>) -> Option<Weak<Link>> {
    std::mem::replace(&mut *HELD.lock().unwrap_or_else(|e| e.into_inner()), link)
}

fn volume_link(link: Option<Weak<Link>>) -> Option<Weak<Link>> {
    std::mem::replace(
        &mut *VOLUME_HELD.lock().unwrap_or_else(|e| e.into_inner()),
        link,
    )
}

/// Foxglove paused or resumed (#404): the knob goes back to the volume keys while paused.
pub fn pause_changed() {
    if let Some(tx) = WORK.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(Work::Pause);
    }
}

/// The keyboard on `link` let the knob go because Foxglove's hold lease lapsed (#847,
/// `hold_lease.rs`): master, and held again (while Foxglove runs unpaused).
pub fn holds_lapsed(link: Weak<Link>) {
    if let Some(tx) = WORK.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(Work::Lapsed(link));
    }
}

/// How long [`stop`] waits for the keyboard to take the knob back.
const RELEASE_REPLY: Duration = Duration::from_millis(300);

/// Foxglove quits: the knob is the volume keys again once the keyboard answers (master volume
/// first: Windows' own flyout again).
pub fn stop() {
    if let Some(link) = volume_link(None).and_then(|l| l.upgrade()) {
        let r = link.request(
            &wire::volume_hold(false),
            protocol::SETTINGS,
            protocol::settings::KNOB_VOLUME,
            RELEASE_REPLY,
        );
        if let Err(e) = r {
            eprintln!("Foxglove: knob: master volume not handed back to the keyboard: {e}");
        }
        knob_health::note(|h| h.master_volume = false);
    }
    if let Some(link) = held_link(None).and_then(|l| l.upgrade()) {
        let r = link.request(
            &wire::release(),
            protocol::SETTINGS,
            protocol::settings::KNOB,
            RELEASE_REPLY,
        );
        if let Err(e) = r {
            eprintln!("Foxglove: knob: not handed back to the keyboard: {e}");
        }
        knob_health::note(|h| h.held = false);
    }
}

/// Tells the keyboard on `link` the hold and target; notes it.
fn tell(link: &Weak<Link>, held: bool, target: WireTarget) {
    let Some(keyboard) = link.upgrade() else {
        return;
    };
    match keyboard.send(&report(held, target)) {
        Ok(()) => {
            held_link(held.then(|| link.clone()));
            knob_health::note(|h| h.held = held);
        }
        Err(e) => eprintln!("Foxglove: knob: can't tell the keyboard: {e}"),
    }
}

/// Tells the keyboard on `link` whether Foxglove holds master volume (#855); notes it in
/// `cycle` (only once the keyboard was told: until then it sends the keys).
fn tell_volume(link: &Weak<Link>, held: bool, cycle: &mut Cycle) {
    let Some(keyboard) = link.upgrade() else {
        return;
    };
    match keyboard.send(&wire::volume_hold(held)) {
        Ok(()) => {
            volume_link(held.then(|| link.clone()));
            cycle.volume = held;
            knob_health::note(|h| h.master_volume = held);
        }
        Err(e) => eprintln!("Foxglove: knob: can't tell the keyboard about master volume: {e}"),
    }
}

/// The keyboard dropped master volume, or the link went: master is its keys again.
fn volume_gone(cycle: &mut Cycle) {
    volume_link(None);
    cycle.volume = false;
    knob_health::note(|h| h.master_volume = false);
}

/// Starts the knob for every shared keyboard link. `FOXGLOVE_DISABLE=knob` leaves it out (the
/// knob is then the volume keys and mute, as without Foxglove).
pub fn start() {
    if super::repaint_trace::disabled("knob") {
        return;
    }
    let (tx, rx) = mpsc::channel::<Work>();
    *WORK.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
    let sessions = foxglove_panel::sessions::start({
        let tx = tx.clone();
        move |change| {
            let _ = tx.send(Work::Sessions(change));
        }
    });
    let links = super::link::subscribe_links({
        let tx = tx.clone();
        move |link| {
            let _ = tx.send(Work::Link(link));
        }
    });
    super::services::spawn_named("knob", move || {
        let _alive = links;
        let Some(sessions) = sessions else {
            eprintln!(
                "Foxglove: knob: Windows' audio sessions aren't available; the knob stays master"
            );
            return;
        };
        run(rx, tx, &*sessions);
    });
}

fn run(rx: mpsc::Receiver<Work>, tx: mpsc::Sender<Work>, sessions: &dyn Sessions) {
    let mut seen: Vec<(Weak<Link>, EventSubscription, EventSubscription)> = Vec::new();
    let mut cycle = Cycle::default();
    // The link the knob is held on now, and the last event's sequence number.
    let mut current: Option<Weak<Link>> = None;
    let mut last_seq: Option<u8> = None;
    // The keyboard on `current` lets Foxglove hold master volume (#855).
    let mut volume_ok = false;
    // While an app is the target: when to stop reading the peaks (#854).
    let mut listen_until: Option<Instant> = None;
    loop {
        let work = match listen_until {
            Some(until) => {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() || cycle.target.is_none() || super::pause::paused() {
                    listen_until = None;
                    continue;
                }
                match rx.recv_timeout(LISTEN_EVERY.min(left)) {
                    Ok(work) => work,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        cycle.listen(sessions);
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(work) => work,
                Err(_) => break,
            },
        };
        seen.retain(|(l, _, _)| l.strong_count() > 0);
        match work {
            Work::Link(link) => {
                let weak = Arc::downgrade(&link);
                if seen.iter().any(|(l, _, _)| Weak::ptr_eq(l, &weak)) {
                    continue;
                }
                // On the link's thread: only enqueue.
                let events = link.subscribe_events({
                    let tx = tx.clone();
                    let weak = weak.clone();
                    move |event| {
                        if let Event::State(state) = event {
                            let _ = tx.send(if state.connected.is_some() {
                                Work::Connected(weak.clone())
                            } else {
                                Work::Disconnected(weak.clone())
                            });
                        }
                    }
                });
                let reports = link.subscribe_reports({
                    let tx = tx.clone();
                    let weak = weak.clone();
                    move |_, report| {
                        // Knob events, and master volume dropped by the board (#855).
                        if wire::event(report).is_some()
                            || wire::volume(report).is_some_and(|v| v.dropped)
                        {
                            let _ = tx.send(Work::Report(weak.clone(), *report));
                        }
                    }
                });
                seen.push((weak, events, reports));
            }
            Work::Connected(weak) => {
                let Some(link) = weak.upgrade() else {
                    continue;
                };
                let has = has_knob(&link);
                let volume = has && has_master_volume(&link);
                drop(link);
                knob_health::note(|h| {
                    h.supported = Some(has);
                    h.master_volume_supported = has.then_some(volume);
                });
                if !has {
                    continue;
                }
                // A new connection: the keyboard starts unheld, on master, with its keys.
                cycle.dropped();
                volume_gone(&mut cycle);
                volume_ok = volume;
                last_seq = None;
                current = Some(weak.clone());
                if !super::pause::paused() {
                    tell(&weak, true, cycle.wire());
                    if volume_ok {
                        tell_volume(&weak, true, &mut cycle);
                    }
                }
            }
            Work::Disconnected(weak) => {
                if current.as_ref().is_some_and(|c| Weak::ptr_eq(c, &weak)) {
                    current = None;
                    held_link(None);
                    cycle.dropped();
                    volume_gone(&mut cycle);
                    volume_ok = false;
                    knob_health::note(|h| {
                        h.held = false;
                        h.supported = None;
                        h.master_volume_supported = None;
                    });
                }
            }
            Work::Report(weak, r) => {
                if !current.as_ref().is_some_and(|c| Weak::ptr_eq(c, &weak)) {
                    continue; // another keyboard's, or one Foxglove doesn't hold
                }
                if wire::volume(&r).is_some_and(|v| v.dropped) {
                    // An endpoint change: master is the keys again until the next connect.
                    volume_gone(&mut cycle);
                    continue;
                }
                let Some(ev) = wire::event(&r) else {
                    continue;
                };
                let lost = last_seq.map_or(0, |s| ev.seq.wrapping_sub(s).wrapping_sub(1));
                last_seq = Some(ev.seq);
                knob_health::note(|h| {
                    h.events += 1;
                    h.lost += u32::from(lost);
                });
                if super::pause::paused() {
                    continue;
                }
                if let Tell::Target(t) = cycle.event(sessions, ev.kind) {
                    tell(&weak, true, t);
                }
                listen_until = cycle.target.is_some().then(|| Instant::now() + LISTEN_FOR);
                if matches!(ev.kind, EventKind::HandedBack) {
                    held_link(None);
                    knob_health::note(|h| h.held = false);
                }
            }
            Work::Sessions(Change::Gone(id)) => {
                if let Tell::Target(t) = cycle.gone(&id) {
                    if let Some(link) = current.as_ref().filter(|_| !super::pause::paused()) {
                        tell(link, true, t);
                    }
                }
            }
            Work::Sessions(Change::Changed) => {}
            Work::Sessions(Change::Master) => {
                cycle.master_changed(current.is_some() && knob_health::now().held);
            }
            Work::Pause => {
                let Some(link) = current.as_ref() else {
                    continue;
                };
                cycle.dropped();
                let link = link.clone();
                tell(&link, !super::pause::paused(), cycle.wire());
                if volume_ok {
                    tell_volume(&link, !super::pause::paused(), &mut cycle);
                }
            }
            Work::Lapsed(weak) => {
                if !current.as_ref().is_some_and(|c| Weak::ptr_eq(c, &weak)) {
                    continue; // another keyboard's, or one Foxglove doesn't hold
                }
                // The keyboard is on master and unheld now, master volume its keys again: the same
                // as a fresh connect.
                cycle.dropped();
                volume_gone(&mut cycle);
                held_link(None);
                knob_health::note(|h| h.held = false);
                if !super::pause::paused() {
                    tell(&weak, true, cycle.wire());
                    if volume_ok {
                        tell_volume(&weak, true, &mut cycle);
                    }
                }
            }
        }
        cycle.note_health();
        if let Some((card, time)) = cycle.popup.take() {
            if !super::pause::paused() {
                knob_popup::show(card, time);
            }
        }
        publish_panel(cycle.panel_line());
    }
}

/// The left screen's volume line: the knob's target (`None`: master).
static PANEL: Mutex<Option<(String, u8, bool)>> = Mutex::new(None);

/// What the left screen's volume line names now (`screens.rs` reads it each frame).
pub fn panel_line() -> Option<(String, u8, bool)> {
    PANEL.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn publish_panel(line: Option<(String, u8, bool)>) {
    let changed = {
        let mut p = PANEL.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *p != line;
        *p = line;
        changed
    };
    if changed {
        super::screens::knob_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_panel::sessions::Session;
    use std::sync::Mutex;

    /// Windows' sessions, as a test lays them out: (id, name, level %, muted), in Windows' order;
    /// only the active ones are listed. Each plays sound (its peak is 0.5) unless it is silent.
    #[derive(Default)]
    struct FakeSessions {
        list: Mutex<Vec<(String, String, i32, bool)>>,
        steps: Mutex<Vec<(String, i32)>>,
        silent: Mutex<Vec<String>>,
        peak_reads: Mutex<u32>,
        /// The fake IAudioEndpointVolume (#855): master's level % and mute once Foxglove set
        /// them (`None`: 64 %, unmuted). Tests never touch the system's volume.
        master: Mutex<Option<(i32, bool)>>,
    }

    impl FakeSessions {
        fn with(apps: &[(&str, &str, i32)]) -> FakeSessions {
            let s = FakeSessions::default();
            *s.list.lock().unwrap() = apps
                .iter()
                .map(|(id, name, level)| (id.to_string(), name.to_string(), *level, false))
                .collect();
            s
        }

        /// Session `id` sends nothing (or starts playing: `false`).
        fn silent(&self, id: &str, silent: bool) {
            let mut s = self.silent.lock().unwrap();
            s.retain(|i| i != id);
            if silent {
                s.push(id.to_string());
            }
        }

        fn start(&self, id: &str, name: &str) {
            self.list
                .lock()
                .unwrap()
                .push((id.to_string(), name.to_string(), 50, false));
        }

        fn end(&self, id: &str) {
            self.list.lock().unwrap().retain(|(i, ..)| i != id);
        }

        fn level(&self, id: &str) -> i32 {
            self.list
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.0 == id)
                .unwrap()
                .2
        }
    }

    impl Sessions for FakeSessions {
        fn active(&self) -> Result<Vec<Session>, String> {
            Ok(self
                .list
                .lock()
                .unwrap()
                .iter()
                .map(|(id, name, ..)| Session {
                    id: id.clone(),
                    name: name.clone(),
                    icon: None,
                    pid: 1,
                })
                .collect())
        }

        fn peaks(&self) -> Result<Vec<(String, f32)>, String> {
            *self.peak_reads.lock().unwrap() += 1;
            let silent = self.silent.lock().unwrap();
            Ok(self
                .list
                .lock()
                .unwrap()
                .iter()
                .map(|(id, ..)| (id.clone(), if silent.contains(id) { 0.0 } else { 0.5 }))
                .collect())
        }

        fn step(&self, id: &str, steps: i32) -> Result<u8, String> {
            self.steps.lock().unwrap().push((id.to_string(), steps));
            let mut list = self.list.lock().unwrap();
            let s = list
                .iter_mut()
                .find(|s| s.0 == id)
                .ok_or_else(|| "the app's sound ended".to_string())?;
            let level = foxglove_panel::sessions::step_level(s.2 as f32 / 100.0, steps);
            s.2 = (level * 100.0).round() as i32;
            Ok(s.2 as u8)
        }

        fn toggle_mute(&self, id: &str) -> Result<bool, String> {
            let mut list = self.list.lock().unwrap();
            let s = list
                .iter_mut()
                .find(|s| s.0 == id)
                .ok_or_else(|| "the app's sound ended".to_string())?;
            s.3 = !s.3;
            Ok(s.3)
        }

        fn level(&self, id: &str) -> Result<(u8, bool), String> {
            let list = self.list.lock().unwrap();
            let s = list
                .iter()
                .find(|s| s.0 == id)
                .ok_or_else(|| "the app's sound ended".to_string())?;
            Ok((s.2 as u8, s.3))
        }

        fn master(&self) -> Result<(u8, bool), String> {
            let (level, muted) = self.master.lock().unwrap().unwrap_or((64, false));
            Ok((level as u8, muted))
        }

        fn step_master(&self, steps: i32) -> Result<u8, String> {
            let mut m = self.master.lock().unwrap();
            let (level, _) = m.unwrap_or((64, false));
            let new = foxglove_panel::sessions::step_level(level as f32 / 100.0, steps);
            let percent = (new * 100.0).round() as i32;
            *m = Some((percent, false)); // a step unmutes, as Windows' volume keys do
            Ok(percent as u8)
        }

        fn toggle_master_mute(&self) -> Result<bool, String> {
            let mut m = self.master.lock().unwrap();
            let (level, muted) = m.unwrap_or((64, false));
            *m = Some((level, !muted));
            Ok(!muted)
        }
    }

    fn popup(c: &mut Cycle) -> Option<(Card, Duration)> {
        c.popup.take()
    }

    #[test]
    fn each_moment_shows_the_storyboards_card() {
        // #719 option A: what shows, and for how long.
        let s = FakeSessions::with(&[
            ("a", "Spotify", 40),
            ("b", "Discord", 80),
            ("c", "Game", 60),
        ]);
        let mut c = Cycle::default();
        // 1. A click: the app, its place in the cycle and its level, 2 s.
        c.event(&s, EventKind::Click);
        assert_eq!(
            popup(&mut c),
            Some((
                Card::App {
                    name: "Spotify".into(),
                    place: Some((1, 3)),
                    level: Some(40),
                    muted: false
                },
                knob_popup::CLICK
            ))
        );
        // 2. A turn: the card with the new level, up until 1.5 s after the last detent.
        c.event(&s, EventKind::Turn(3));
        assert_eq!(
            popup(&mut c),
            Some((
                Card::App {
                    name: "Spotify".into(),
                    place: None,
                    level: Some(46),
                    muted: false
                },
                knob_popup::TURN
            ))
        );
        assert_eq!(c.panel_line(), Some(("Spotify".into(), 46, false)));
        // 5. A double click: muted, 2 s.
        c.event(&s, EventKind::Double);
        let (card, time) = popup(&mut c).unwrap();
        assert!(matches!(card, Card::App { muted: true, .. }));
        assert_eq!(time, knob_popup::MUTE);
        assert_eq!(c.panel_line(), Some(("Spotify".into(), 46, true)));
        // 3. Past the last app: the master card, back to master, 2 s.
        c.event(&s, EventKind::Click);
        c.event(&s, EventKind::Click);
        popup(&mut c);
        c.event(&s, EventKind::Click);
        assert_eq!(
            popup(&mut c),
            Some((
                Card::Master {
                    level: Some(64),
                    note: "back to master",
                    muted: false
                },
                knob_popup::MASTER
            ))
        );
        assert_eq!(c.panel_line(), None, "master: the plain volume bar");
        // 4. A hold: the master card, 2 s.
        c.event(&s, EventKind::Click);
        c.event(&s, EventKind::Hold);
        assert_eq!(
            popup(&mut c),
            Some((
                Card::Master {
                    level: Some(64),
                    note: "held: master",
                    muted: false
                },
                knob_popup::MASTER
            ))
        );
        // 6. The target's sound ends: "<app> stopped · knob: master", 3 s.
        c.event(&s, EventKind::Click);
        popup(&mut c);
        c.gone("a");
        assert_eq!(
            popup(&mut c),
            Some((
                Card::Stopped {
                    name: "Spotify".into()
                },
                knob_popup::STOPPED
            ))
        );
        // 7. Master's level changes while held on master: only the tab over Windows' flyout;
        // not held, or on an app, nothing.
        c.master_changed(true);
        assert_eq!(popup(&mut c), Some((Card::MasterTab, knob_popup::TAB)));
        c.master_changed(false);
        assert_eq!(popup(&mut c), None);
        c.event(&s, EventKind::Click);
        popup(&mut c);
        c.master_changed(true);
        assert_eq!(
            popup(&mut c),
            None,
            "never a second popup for an app's own turns"
        );
    }

    fn target(c: &Cycle) -> Option<&str> {
        c.target.as_ref().map(|a| a.name.as_str())
    }

    #[test]
    fn clicks_cycle_the_active_apps_in_windows_order_then_master() {
        let s = FakeSessions::with(&[
            ("a", "Spotify", 40),
            ("b", "Discord", 80),
            ("c", "Game", 60),
        ]);
        let mut c = Cycle::default();
        assert_eq!(c.event(&s, EventKind::Click), Tell::Target(WireTarget::App));
        assert_eq!(target(&c), Some("Spotify"));
        // App to app: the keyboard already sends the turns here.
        assert_eq!(c.event(&s, EventKind::Click), Tell::Nothing);
        assert_eq!(target(&c), Some("Discord"));
        c.event(&s, EventKind::Click);
        assert_eq!(target(&c), Some("Game"));
        // Past the last: master, and the keyboard is told (its turns are the volume keys again).
        assert_eq!(
            c.event(&s, EventKind::Click),
            Tell::Target(WireTarget::Master)
        );
        assert_eq!(target(&c), None);
        assert_eq!(c.event(&s, EventKind::Click), Tell::Target(WireTarget::App));
        assert_eq!(target(&c), Some("Spotify"));
    }

    #[test]
    fn nothing_playing_keeps_master() {
        let s = FakeSessions::default();
        let mut c = Cycle::default();
        assert_eq!(c.event(&s, EventKind::Click), Tell::Nothing);
        assert_eq!(target(&c), None);
    }

    #[test]
    fn turns_set_the_apps_level_2_percent_a_detent() {
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle::default();
        // On master a turn reaches the PC only by mistake: nothing is set.
        c.event(&s, EventKind::Turn(3));
        assert!(s.steps.lock().unwrap().is_empty());
        c.event(&s, EventKind::Click);
        c.event(&s, EventKind::Turn(3));
        assert_eq!(s.level("a"), 46);
        c.event(&s, EventKind::Turn(-1));
        assert_eq!(s.level("a"), 44);
        assert_eq!(
            c.last_level,
            Some(LastLevel::Set {
                app: "Spotify".into(),
                percent: 44
            })
        );
    }

    #[test]
    fn a_double_click_mutes_the_app_and_does_nothing_here_on_master() {
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle::default();
        assert_eq!(c.event(&s, EventKind::Double), Tell::Nothing);
        assert!(
            !s.list.lock().unwrap()[0].3,
            "master's mute is the keyboard's key"
        );
        c.event(&s, EventKind::Click);
        c.event(&s, EventKind::Double);
        assert!(s.list.lock().unwrap()[0].3);
        c.event(&s, EventKind::Double);
        assert!(!s.list.lock().unwrap()[0].3);
    }

    #[test]
    fn a_hold_goes_back_to_master() {
        let s = FakeSessions::with(&[("a", "Spotify", 40), ("b", "Discord", 80)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Click);
        c.event(&s, EventKind::Click);
        assert_eq!(
            c.event(&s, EventKind::Hold),
            Tell::Target(WireTarget::Master)
        );
        assert_eq!(target(&c), None);
        assert_eq!(
            c.event(&s, EventKind::Hold),
            Tell::Nothing,
            "already master"
        );
    }

    #[test]
    fn the_target_lives_as_long_as_its_session() {
        let s = FakeSessions::with(&[("a", "Spotify", 40), ("b", "Discord", 80)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Click);
        // Another app's session ending changes nothing.
        assert_eq!(c.gone("b"), Tell::Nothing);
        assert_eq!(target(&c), Some("Spotify"));
        // The target's: master, and the keyboard is told.
        s.end("a");
        assert_eq!(c.gone("a"), Tell::Target(WireTarget::Master));
        assert_eq!(target(&c), None);
        assert_eq!(c.last_back.as_deref(), Some("Spotify's sound ended"));
    }

    #[test]
    fn a_failed_level_is_a_failed_health_row() {
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Click);
        s.end("a"); // the session went before Windows' event reached the knob
        c.event(&s, EventKind::Turn(1));
        let rows = knob_health::rows(&knob_health::KnobHealth {
            last_level: c.last_level.clone(),
            ..Default::default()
        });
        let level = rows.iter().find(|r| r.item == "Knob · Last level").unwrap();
        assert_eq!(level.level, knob_health::Level::Fail);
        assert_eq!(
            level.detail,
            "couldn't set Spotify's level: the app's sound ended"
        );
    }

    #[test]
    fn a_click_after_the_target_ended_starts_from_the_first_app() {
        let s = FakeSessions::with(&[("a", "Spotify", 40), ("b", "Discord", 80)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Click);
        c.event(&s, EventKind::Click);
        s.end("b");
        c.event(&s, EventKind::Click);
        assert_eq!(target(&c), Some("Spotify"));
    }

    #[test]
    fn the_keyboard_dropping_the_hold_is_master() {
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Click);
        assert_eq!(c.event(&s, EventKind::HandedBack), Tell::Nothing);
        assert_eq!(target(&c), None);
    }

    fn names(c: &mut Cycle, s: &FakeSessions, clicks: usize) -> Vec<String> {
        (0..clicks)
            .map(|_| {
                c.event(s, EventKind::Click);
                target(c).unwrap_or("master").to_string()
            })
            .collect()
    }

    #[test]
    fn a_session_that_never_played_is_not_offered() {
        // #854: a background helper holds an active but silent stream.
        let s = FakeSessions::with(&[("bg", "LightStudio-background", 100), ("a", "Spotify", 40)]);
        s.silent("bg", true);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Click);
        assert_eq!(target(&c), Some("Spotify"));
        assert!(matches!(
            popup(&mut c),
            Some((
                Card::App {
                    place: Some((1, 1)),
                    ..
                },
                _
            ))
        ));
        assert_eq!(names(&mut c, &s, 2), ["master", "Spotify"]);
        // Only silence, ever: master, as when nothing plays.
        let quiet = FakeSessions::with(&[("bg", "LightStudio-background", 100)]);
        quiet.silent("bg", true);
        let mut c = Cycle::default();
        assert_eq!(c.event(&quiet, EventKind::Click), Tell::Nothing);
        assert_eq!(target(&c), None);
    }

    #[test]
    fn a_session_once_heard_stays_and_keeps_its_place() {
        let s = FakeSessions::with(&[("a", "Spotify", 40), ("b", "Discord", 80)]);
        s.silent("a", true);
        let mut c = Cycle::default();
        assert_eq!(names(&mut c, &s, 2), ["Discord", "master"]);
        // Spotify plays: it goes after Discord, in the order first heard, not Windows'.
        s.silent("a", false);
        assert_eq!(names(&mut c, &s, 3), ["Discord", "Spotify", "master"]);
        // Both quiet again (paused, or muted from the knob): still offered, same order.
        s.silent("a", true);
        s.silent("b", true);
        assert_eq!(names(&mut c, &s, 3), ["Discord", "Spotify", "master"]);
        // An ended session is forgotten; the rest keep their order.
        s.end("b");
        c.gone("b");
        assert!(!c.heard.has("b"));
        assert_eq!(names(&mut c, &s, 2), ["Spotify", "master"]);
    }

    #[test]
    fn listening_on_an_app_finds_a_new_player_for_the_next_click() {
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Click);
        s.start("g", "Game");
        // The service's bounded listening while an app is the target.
        c.listen(&s);
        s.silent("g", true); // between sounds when the click comes
        assert_eq!(names(&mut c, &s, 2), ["Game", "master"]);
    }

    #[test]
    fn peaks_are_read_only_on_a_click_and_not_on_master() {
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Turn(1));
        c.event(&s, EventKind::Double);
        c.event(&s, EventKind::Hold);
        c.gone("a");
        assert_eq!(*s.peak_reads.lock().unwrap(), 0);
        c.event(&s, EventKind::Click);
        assert_eq!(*s.peak_reads.lock().unwrap(), 1);
        c.event(&s, EventKind::Turn(1));
        assert_eq!(*s.peak_reads.lock().unwrap(), 1);
    }

    #[test]
    fn the_reports_are_the_firmwares() {
        assert_eq!(report(true, WireTarget::App), [0xD7, 0x14, 1, 1]);
        assert_eq!(report(true, WireTarget::Master), [0xD7, 0x14, 1, 0]);
        assert_eq!(report(false, WireTarget::Master), [0xD7, 0x14, 0, 0]);
        // #855: master volume, as tests/zmk-sim/knob sends it.
        assert_eq!(wire::volume_hold(true), [0xD7, 0x16, 1]);
        assert_eq!(wire::volume_hold(false), [0xD7, 0x16, 0]);
    }

    fn master_now(s: &FakeSessions) -> (u8, bool) {
        s.master().unwrap()
    }

    #[test]
    fn with_master_volume_held_master_turns_show_foxgloves_card() {
        // #855: the keyboard sends master's turns here; Foxglove sets the level (the fake
        // IAudioEndpointVolume) and shows the same card as an app's, not Windows' flyout.
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle {
            volume: true,
            ..Cycle::default()
        };
        assert_eq!(c.event(&s, EventKind::Turn(3)), Tell::Nothing);
        assert_eq!(master_now(&s), (70, false));
        assert_eq!(
            popup(&mut c),
            Some((
                Card::Master {
                    level: Some(70),
                    note: "",
                    muted: false
                },
                knob_popup::TURN
            ))
        );
        assert_eq!(
            c.last_level,
            Some(LastLevel::Set {
                app: "master volume".into(),
                percent: 70
            })
        );
        assert!(s.steps.lock().unwrap().is_empty(), "no app was touched");
        // A double click mutes master here (the keyboard tapped no mute key).
        c.event(&s, EventKind::Double);
        assert_eq!(master_now(&s), (70, true));
        assert_eq!(
            popup(&mut c),
            Some((
                Card::Master {
                    level: Some(70),
                    note: "",
                    muted: true
                },
                knob_popup::MUTE
            ))
        );
        // A turn unmutes, as Windows' volume keys do; the edges hold.
        c.event(&s, EventKind::Turn(-1));
        assert_eq!(master_now(&s), (68, false));
        c.event(&s, EventKind::Turn(127));
        assert_eq!(master_now(&s), (100, false));
        // Master's change events (from elsewhere) show no tab over Foxglove's card.
        popup(&mut c);
        c.master_changed(true);
        assert_eq!(popup(&mut c), None);
        // On an app the turns are the app's, master untouched.
        c.event(&s, EventKind::Click);
        c.event(&s, EventKind::Turn(1));
        assert_eq!(s.level("a"), 42);
        assert_eq!(master_now(&s), (100, false));
    }

    #[test]
    fn without_master_volume_master_is_the_keyboards_keys() {
        // Firmware before #855 (or Foxglove paused): the keyboard sends the volume keys and
        // mute itself, Windows shows its flyout and Foxglove only the tab.
        let s = FakeSessions::with(&[("a", "Spotify", 40)]);
        let mut c = Cycle::default();
        c.event(&s, EventKind::Turn(3));
        c.event(&s, EventKind::Double);
        assert_eq!(master_now(&s), (64, false), "master is untouched");
        assert_eq!(popup(&mut c), None);
        c.master_changed(true);
        assert_eq!(popup(&mut c), Some((Card::MasterTab, knob_popup::TAB)));
    }
}
