//! Wallpaper sync, stage 1 (#493, #377; the plan on #39): Foxglove tells each wallpaper that
//! asks for them (its `foxglove.json` `supports`, #562, docs/wallpaper-manifest.md) the
//! keyboard's layer, whether game mode is on, the running scene's clock, the live feed's port
//! and Windows' app mode (`windows_theme`, for the wallpapers' "Follow Windows" theme), through
//! Wallpaper Engine's own command line:
//!
//! ```text
//! wallpaper64.exe -control applyProperties -location <monitor> -properties RAW~({"foxglove_game":true,"foxglove_layer":"GAME","foxglove_scene":"v=1;..."})~END
//! ```
//!
//! The wallpaper gets them in `wallpaperPropertyListener.applyUserProperties`, the path the
//! editor uses, and draws its monitor's part of the scene from the shared epoch
//! (`foxglove_scene::wallpaper`; `shared/foxglove-sync.js` in bandit-wallpapers is one client).
//! Nothing is streamed: one push per change, only to the monitors that show such a wallpaper,
//! and only the properties it supports.
//!
//! - **When:** a scene starts, stops or is placed again; the layer or game mode changes; the
//!   wallpapers on screen change (a new wallpaper gets the current state). A push goes out only
//!   when a monitor's properties differ from what it was last sent, and one push runs at a time:
//!   changes that arrive meanwhile collapse to the newest.
//! - **Reloads:** Wallpaper Engine reloads a wallpaper from its saved properties, which pushes
//!   never write, so a reloaded wallpaper is sent everything again (`foxglove_theme` too): when
//!   its folder changes (installed again or updated in place: the target's [`Version`], held
//!   [`RELOAD_QUIET`] so the copy and Wallpaper Engine's reload finish first), and when a
//!   wallpaper connects to the live feed (it just loaded; the monitors sent something in the last
//!   [`RECONNECT_FRESH`] are not sent again).
//! - **Settled:** a layer change waits [`SETTLE`] for the next one before it is sent, so a layer
//!   held for a moment and let go (GAME+ for Win+Tab or Win+1 in a game, a thumb for NUMBER)
//!   sends nothing: each push starts a Wallpaper Engine process, and these landed just as it
//!   brought a wallpaper back from a full-screen game. Scene, live-feed and wallpaper changes
//!   go at once.
//! - **Full-screen programs (#586):** with one in front (a game, `SHQueryUserNotificationState`)
//!   a layer change waits [`GAME_SETTLE`], so GAME+ held for Win+Tab or Alt+Tab sends nothing.
//!   When the program leaves the front (the `app.foreground` fact, or any wake that finds it
//!   gone), Wallpaper Engine reloads the wallpaper it stopped or paused under it: nothing is sent
//!   for [`RELOAD_QUIET`] after that edge, then the newest state goes once, so no
//!   `-control` process lands in the reload. With Wallpaper Engine's "Other application
//!   fullscreen: Stop" (`playbackfullscreen: "stop"`) the reloaded wallpaper starts from its
//!   saved properties, which pushes never write, so everything is sent again then.
//! - **Health:** pushes per minute, duplicates suppressed, reload holds and the last push's
//!   reason (`wallpaper_sync_health`), also in each push's log line.
//! - **Facts only:** command lines are visible to other processes, so the properties are the
//!   layer's name, a bool and the scene's timing, colours and geometry. Never a token.
//! - **Optional:** without Wallpaper Engine, or while it isn't running, nothing is sent and no
//!   thread starts (its command line would start it). The worker starts the first time Wallpaper
//!   Engine shows a wallpaper that supports one of the properties.
//! - **Game mode:** a layer named GAME... is active (the GAME / GAME+ layers, docs/layers.md).
//! - **Which keyboard (#801):** the feed's (its pin, else the focus; `focus_pins`). The layer and
//!   game mode are the host-link keyboard's, so they go out only while it is that keyboard; for
//!   a VIA or LampArray keyboard the wallpapers hear no layer.
//!
//! Stage 2 (#499): the live socket (`live_feed`) carries key presses and, later, PC stats and
//! notifications. This path only tells the wallpapers its port (`foxglove_live`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use foxglove_scene::{Clock, Effect, Rect, RoomMap, WallpaperScene};

use super::wallpaper_leases::{Ended, Fail, Leases};
pub use super::wallpaper_manifest::Supports;
use super::wallpaper_sync_health::{self as health, Reason};

/// The reasons of the live leases, as a fact (#1120).
pub const PAUSED_BY: &str = "wallpaper_engine.paused_by";

/// The properties a wallpaper declares in its `project.json` to receive them (each one its
/// manifest's `supports` names: `layer`, `gamemode`, `clock`, `foxglove_live`).
pub const LAYER: &str = "foxglove_layer";
pub const GAME: &str = "foxglove_game";
pub const SCENE: &str = "foxglove_scene";
/// The live feed's port while it listens, 0 when it doesn't (#499): the wallpaper connects
/// when this changes, so it never retries on a timer. The pairing token never goes here.
pub const LIVE: &str = "foxglove_live";
/// Windows' app mode, `"light"` or `"dark"` (`""` when Windows doesn't say, or Foxglove has
/// gone): every wallpaper set to "Follow Windows" takes it, so two monitors never disagree.
pub const THEME: &str = "foxglove_theme";

/// How long one push may take before it is given up (and its process ended).
const PUSH_LIMIT: Duration = Duration::from_secs(5);
/// How long quitting waits for the wallpapers to be told Foxglove has gone.
const QUIT_LIMIT: Duration = Duration::from_secs(6);
/// A layer change waits this long for the next one before it is sent (a one-shot deadline per
/// change, never a timer).
pub const SETTLE: Duration = Duration::from_millis(400);
/// A run of layer changes is sent at the latest this long after its first.
pub const SETTLE_MAX: Duration = Duration::from_secs(2);
/// With a full-screen program in front, a layer change waits this long for the next (a layer
/// held to reach Tab or the Win key), and a run of them at most [`GAME_SETTLE_MAX`].
pub const GAME_SETTLE: Duration = Duration::from_secs(3);
pub const GAME_SETTLE_MAX: Duration = Duration::from_secs(6);
/// Nothing is sent this long after a full-screen program leaves the front: Wallpaper Engine
/// reloads (Stop) or resumes (Pause) the wallpaper then, and a `-control` command meanwhile is
/// ignored ("Windows is reentrant") or lands in the wallpaper's boot.
pub const RELOAD_QUIET: Duration = Duration::from_secs(2);
/// A wallpaper connecting to the live feed resends only to the monitors not sent anything this
/// recently (at Foxglove's start the wallpapers connect right after the first push).
pub const RECONNECT_FRESH: Duration = Duration::from_secs(3);

/// A wallpaper folder's contents as last seen: its newest file's write time and its number of
/// entries. Installing it again or updating it in place changes it.
pub type Version = (std::time::SystemTime, u64);

/// A running scene, as the wallpapers need it.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub id: String,
    pub effect: Effect,
    pub clock: Clock,
    pub room: RoomMap,
    /// Counts the scene's placements (start, devices placed again): the monitors' interface
    /// paths are read again on each.
    pub placement: u64,
}

/// What the wallpapers are told.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The highest active layer's name; `None` without a keyboard.
    pub layer: Option<String>,
    pub game: bool,
    pub scene: Option<Scene>,
    /// The live feed's port while it listens (`live_feed`).
    pub live: Option<u16>,
    /// Windows' app mode (`windows_theme`).
    pub theme: Option<super::windows_theme::AppMode>,
}

/// A layer name as a property: letters, digits, `+`, `-` and `_`, upper case, at most 16.
fn clean_layer(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_'))
        .take(16)
        .collect::<String>()
        .to_ascii_uppercase()
}

/// The highest active layer's name (`L<n>` when the keyboard didn't name it).
pub fn layer_name(active: &[u8], names: &[Option<String>]) -> Option<String> {
    let top = *active.iter().max()?;
    let named = names
        .get(usize::from(top))
        .and_then(|n| n.as_deref())
        .map(clean_layer)
        .filter(|n| !n.is_empty());
    Some(named.unwrap_or_else(|| format!("L{top}")))
}

/// Game mode: any active layer is named GAME... (GAME, GAME+).
pub fn game_mode(active: &[u8], names: &[Option<String>]) -> bool {
    active.iter().any(|&i| {
        names
            .get(usize::from(i))
            .and_then(|n| n.as_deref())
            .is_some_and(|n| clean_layer(n).starts_with("GAME"))
    })
}

/// One monitor's properties as compact JSON, only those its wallpaper supports: no spaces, and
/// no quotes inside the values, so it passes through `RAW~(...)~END` as typed.
pub fn properties(state: &State, monitor: Option<Rect>, supports: Supports) -> String {
    let mut out = serde_json::Map::new();
    if supports.game {
        out.insert(GAME.into(), state.game.into());
    }
    if supports.layer {
        out.insert(LAYER.into(), state.layer.clone().unwrap_or_default().into());
    }
    if supports.live {
        out.insert(LIVE.into(), state.live.unwrap_or(0).into());
    }
    if supports.theme {
        let mode = state
            .theme
            .map_or("", super::windows_theme::AppMode::property);
        out.insert(THEME.into(), mode.into());
    }
    if supports.clock {
        let scene = state.scene.as_ref().map_or_else(String::new, |s| {
            WallpaperScene::new(&s.id, &s.effect, &s.clock, s.room.bounds, monitor).encode()
        });
        out.insert(SCENE.into(), scene.into());
    }
    serde_json::Value::Object(out).to_string()
}

/// What quitting leaves: no layer, no game mode, no scene (the wallpaper runs on its own).
fn gone(supports: Supports) -> String {
    properties(&State::default(), None, supports)
}

/// The command line's arguments after the program, the properties last (they go in raw, as a
/// user types them in a terminal: Wallpaper Engine reads them between `RAW~(` and `)~END`).
pub fn arguments(location: &str, props: &str) -> (Vec<String>, String) {
    (
        vec![
            "-control".into(),
            "applyProperties".into(),
            "-location".into(),
            location.into(),
        ],
        format!("-properties RAW~({props})~END"),
    )
}

/// Wallpaper Engine's monitor key (`//?/DISPLAY#DEL40EA#...#{e6f07b5f-...}` in its config.json,
/// the monitor's device interface path) against Windows' own, ignoring case and slashes.
fn same_monitor(we_key: &str, interface: &str) -> bool {
    let norm = |s: &str| s.trim().replace('\\', "/").to_ascii_lowercase();
    !interface.trim().is_empty() && norm(we_key) == norm(interface)
}

/// The rectangle in the room of the monitor Wallpaper Engine calls `location`. `interfaces`
/// pairs each Windows display (`\\.\DISPLAY1`) with its interface path.
pub fn monitor_rect(
    location: &str,
    room: &RoomMap,
    interfaces: &[(String, String)],
) -> Option<Rect> {
    let (gdi, _) = interfaces
        .iter()
        .find(|(_, iface)| same_monitor(location, iface))?;
    room.device(&format!("monitor:{gdi}")).map(|p| p.rect)
}

/// Sends one monitor its properties: Wallpaper Engine's command line, or a fake in tests.
pub trait Apply: Send {
    /// Whether anything may be sent now (Wallpaper Engine runs).
    fn ready(&mut self) -> bool;
    fn apply(&mut self, location: &str, props: &str) -> Result<(), String>;
    /// Wallpaper Engine's `-control pause` or `-control play` (#1120): the local API's leases.
    fn control(&mut self, command: &str) -> Result<(), String>;
    /// Each Windows display's interface path, for [`monitor_rect`].
    fn interfaces(&mut self) -> Vec<(String, String)>;
}

/// One monitor showing a wallpaper that reads Foxglove's properties.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Wallpaper Engine's key for the monitor.
    pub location: String,
    /// The wallpaper's file (another wallpaper on the same monitor starts from its defaults, so
    /// it is sent again).
    pub file: String,
    /// What its manifest says it reads.
    pub supports: Supports,
    /// Its folder's contents ([`Version`]): when they change, Wallpaper Engine reloads the
    /// wallpaper from its saved properties, so it is sent again.
    pub version: Option<Version>,
}

/// What a monitor was last sent.
struct Sent {
    file: String,
    version: Option<Version>,
    props: String,
    supports: Supports,
    at: Instant,
}

/// What one sync did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    /// Monitors sent their properties.
    pub pushed: usize,
    /// Monitors that already had them: no push (a duplicate suppressed).
    pub unchanged: usize,
    /// The last push that failed, why.
    pub failed: Option<String>,
}

/// What has been sent, per monitor, and the pushes that keep it current.
pub struct Pusher<A: Apply> {
    apply: A,
    /// Per monitor: the wallpaper's file and folder version, the properties it was sent, what it
    /// supports and when.
    sent: BTreeMap<String, Sent>,
    /// The interface paths, read once per scene placement.
    interfaces: Option<Vec<(String, String)>>,
    placement_seen: Option<u64>,
}

impl<A: Apply> Pusher<A> {
    pub fn new(apply: A) -> Self {
        Pusher {
            apply,
            sent: BTreeMap::new(),
            interfaces: None,
            placement_seen: None,
        }
    }

    /// Brings every target up to `state`; returns how many monitors were sent something.
    #[cfg(test)]
    pub fn sync(&mut self, state: &State, targets: &[Target]) -> usize {
        self.sync_counted(state, targets).pushed
    }

    /// Forgets what was sent, so the next sync sends every monitor its properties (a wallpaper
    /// Wallpaper Engine reloaded starts from its saved ones).
    pub fn resend(&mut self) {
        self.sent.clear();
    }

    /// Forgets what was sent longer than `fresh` ago (a wallpaper connected to the live feed:
    /// it just loaded, on a monitor not known); returns how many monitors will be sent again.
    pub fn resend_stale(&mut self, fresh: Duration, now: Instant) -> usize {
        let before = self.sent.len();
        self.sent
            .retain(|_, s| now.saturating_duration_since(s.at) < fresh);
        before - self.sent.len()
    }

    /// [`Pusher::sync`], with the monitors that already had the properties and the last failure.
    pub fn sync_counted(&mut self, state: &State, targets: &[Target]) -> Synced {
        let mut out = Synced::default();
        self.sent
            .retain(|k, _| targets.iter().any(|t| t.location == *k));
        if targets.is_empty() {
            return out;
        }
        if !self.apply.ready() {
            // Everything goes again once it runs (it starts with the wallpapers' defaults).
            self.sent.clear();
            return out;
        }
        let placement = state.scene.as_ref().map(|s| s.placement);
        if placement != self.placement_seen {
            self.placement_seen = placement;
            self.interfaces = None;
        }
        for Target {
            location,
            file,
            supports,
            version,
        } in targets
        {
            let monitor = state.scene.as_ref().and_then(|s| {
                let ifaces = self
                    .interfaces
                    .get_or_insert_with(|| self.apply.interfaces());
                monitor_rect(location, &s.room, ifaces)
            });
            let props = properties(state, monitor, *supports);
            if self
                .sent
                .get(location)
                .is_some_and(|s| s.file == *file && s.version == *version && s.props == props)
            {
                out.unchanged += 1;
                continue;
            }
            match self.apply.apply(location, &props) {
                Ok(()) => {
                    self.sent.insert(
                        location.clone(),
                        Sent {
                            file: file.clone(),
                            version: *version,
                            props,
                            supports: *supports,
                            at: Instant::now(),
                        },
                    );
                    out.pushed += 1;
                }
                Err(e) => {
                    self.sent.remove(location);
                    eprintln!("Foxglove: wallpaper sync: can't reach {location}: {e}");
                    out.failed = Some(e);
                }
            }
        }
        out
    }

    /// The sender, for the lease commands that run between pushes (#1120).
    pub fn apply_mut(&mut self) -> &mut A {
        &mut self.apply
    }

    /// Quitting: the monitors that were sent something are told Foxglove has gone.
    pub fn quit(&mut self) {
        let told = std::mem::take(&mut self.sent);
        if told.is_empty() || !self.apply.ready() {
            return;
        }
        for (location, sent) in told {
            let props = gone(sent.supports);
            if sent.props != props {
                let _ = self.apply.apply(&location, &props);
            }
        }
    }
}

// ------------------------------------------------------------------------- the app's worker

enum Wake {
    /// A scene, the live feed or the wallpapers on screen changed: sent at once.
    Changed(Reason),
    /// The keyboard's layers changed: sent once they settle.
    Layer,
    /// The program in front changed (`app.foreground`): a full-screen one may have gone.
    Foreground,
    /// A wallpaper on screen was installed again or updated: sent once the copy settles.
    Reinstalled,
    /// A wallpaper connected to the live feed: it just loaded.
    Reconnected,
    /// A lease call of the local API (#1120), run between pushes.
    Control(Ctl),
    Quit(Sender<()>),
}

/// What a lease call needs from the worker (#1120, `wallpaper-engine.*`).
pub enum Ctl {
    Pause {
        conn: u64,
        reason: String,
        /// A paired wallpaper reports itself paused: the user's own pause.
        user_paused: bool,
        reply: Sender<Reply>,
    },
    Release {
        conn: u64,
        lease: u64,
        reply: Sender<Reply>,
    },
    Status(Sender<Reply>),
    /// A connection closed: its leases end.
    Closed(u64),
}

/// The worker's answer to a [`Ctl`].
pub enum Reply {
    Paused {
        lease: u64,
        paused: bool,
    },
    Released(Ended),
    Status {
        running: bool,
        leases: u32,
        paused_by_lease: bool,
    },
    Failed(Fail),
}

/// How long a layer change waits for the next, and a run of them at most; the same with a
/// full-screen program in front; and the quiet after one leaves.
#[derive(Clone, Copy, Debug)]
struct Settle {
    layer: Duration,
    max: Duration,
    game: Duration,
    game_max: Duration,
    reload: Duration,
    /// A live-feed connect resends to the monitors not sent anything this recently.
    fresh: Duration,
}

impl Settle {
    const APP: Settle = Settle {
        layer: SETTLE,
        max: SETTLE_MAX,
        game: GAME_SETTLE,
        game_max: GAME_SETTLE_MAX,
        reload: RELOAD_QUIET,
        fresh: RECONNECT_FRESH,
    };
}

/// Whether a full-screen program is in front, as last read, and its reader.
struct Screen<F: FnMut() -> bool> {
    read: F,
    full: bool,
}

impl<F: FnMut() -> bool> Screen<F> {
    fn new(mut read: F) -> Self {
        let full = read();
        Screen { read, full }
    }

    /// Reads it again; `true` when a full-screen program has just left the front.
    /// Whether a full-screen program is in front right now, without taking the edge.
    fn now(&mut self) -> bool {
        (self.read)()
    }

    fn left(&mut self) -> bool {
        let was = std::mem::replace(&mut self.full, (self.read)());
        was && !self.full
    }
}

/// What the worker does next.
enum Next {
    Sync {
        reason: Reason,
        /// A full-screen program left the front during the run (Wallpaper Engine reloaded).
        left_full: bool,
        /// A wallpaper connected to the live feed during the run.
        reconnected: bool,
    },
    Quit(Sender<()>),
    /// A lease call; the run in progress (if any) goes on after it.
    Control(Ctl, Option<Run>),
    /// Every sender has gone.
    Closed,
}

/// A run of changes being taken.
struct Run {
    reason: Reason,
    /// A layer change's settle deadline; `None` syncs once the queue is empty.
    deadline: Option<Instant>,
    /// The latest a layer run waits.
    cap: Instant,
    /// Nothing is sent before this (Wallpaper Engine's reload after a full-screen program).
    quiet_until: Option<Instant>,
    left_full: bool,
    /// The latest a reinstall's quiet holds the run.
    reinstall_cap: Option<Instant>,
    reconnected: bool,
}

impl Run {
    fn new(reason: Reason, deadline: Option<Instant>, cap: Instant) -> Run {
        Run {
            reason,
            deadline,
            cap,
            quiet_until: None,
            left_full: false,
            reinstall_cap: None,
            reconnected: false,
        }
    }

    /// When the run is sent if nothing else comes; `None`: once the queue is empty.
    fn due(&self) -> Option<Instant> {
        let settled = self.deadline.map(|d| d.min(self.cap));
        match (settled, self.quiet_until) {
            (Some(s), Some(q)) => Some(s.max(q)),
            (s, q) => s.or(q),
        }
    }

    fn done(&self) -> Next {
        Next::Sync {
            reason: self.reason,
            left_full: self.left_full,
            reconnected: self.reconnected,
        }
    }
}

/// Blocks for the next change, then takes the run it starts. A layer change moves the deadline
/// to `settle.layer` from now (`settle.game` with a full-screen program in front; never past the
/// run's cap); any other change clears the deadline, so the run syncs once the queue is empty; a
/// full-screen program leaving the front holds the run for `settle.reload` from then, whatever
/// else comes, and so does a wallpaper reinstalled (from its last change, at most
/// `settle.game_max` from its first); quitting wins. A foreground change that isn't such an edge sends nothing. The
/// waits are one-shot deadlines on the channel: nothing wakes without a change.
#[cfg(test)]
fn next<F: FnMut() -> bool>(rx: &Receiver<Wake>, settle: Settle, screen: &mut Screen<F>) -> Next {
    next_from(rx, settle, screen, None)
}

/// [`next`], continuing the run `run` that a lease call interrupted.
fn next_from<F: FnMut() -> bool>(
    rx: &Receiver<Wake>,
    settle: Settle,
    screen: &mut Screen<F>,
    mut run: Option<Run>,
) -> Next {
    loop {
        let got = match &run {
            None => match rx.recv() {
                Ok(w) => w,
                Err(_) => return Next::Closed,
            },
            Some(r) => match r.due() {
                None => match rx.try_recv() {
                    Ok(w) => w,
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => return r.done(),
                },
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(w) => w,
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                        return r.done()
                    }
                },
            },
        };
        if let Wake::Quit(ack) = got {
            return Next::Quit(ack);
        }
        // A lease call goes first, ahead of any push; the run waits for it.
        if let Wake::Control(ctl) = got {
            return Next::Control(ctl, run);
        }
        let now = Instant::now();
        // Read on every wake, so the edge is seen even without a foreground event.
        if screen.left() {
            let r = run.get_or_insert_with(|| Run::new(Reason::Reload, None, now));
            r.quiet_until = Some(now + settle.reload);
            r.left_full = true;
            r.reason = Reason::Reload;
        }
        match got {
            Wake::Quit(_) | Wake::Control(_) => unreachable!("handled above"),
            Wake::Foreground => {}
            Wake::Layer => {
                let (wait, max) = if screen.full {
                    (settle.game, settle.game_max)
                } else {
                    (settle.layer, settle.max)
                };
                match &mut run {
                    None => run = Some(Run::new(Reason::Layer, Some(now + wait), now + max)),
                    // Once cleared, the deadline stays cleared for the rest of the run.
                    Some(r) => r.deadline = r.deadline.map(|_| now + wait),
                }
            }
            Wake::Changed(reason) => match &mut run {
                None => run = Some(Run::new(reason, None, now)),
                Some(r) => {
                    r.deadline = None;
                    if !r.left_full && r.reinstall_cap.is_none() {
                        r.reason = reason;
                    }
                }
            },
            Wake::Reinstalled => {
                let r = run.get_or_insert_with(|| Run::new(Reason::Reinstalled, None, now));
                r.deadline = None;
                let cap = *r.reinstall_cap.get_or_insert(now + settle.game_max);
                let until = (now + settle.reload).min(cap);
                r.quiet_until = Some(r.quiet_until.map_or(until, |q| q.max(until)));
                if !r.left_full {
                    r.reason = Reason::Reinstalled;
                }
            }
            Wake::Reconnected => {
                let r = run.get_or_insert_with(|| Run::new(Reason::Reconnect, None, now));
                r.deadline = None;
                r.reconnected = true;
            }
        }
    }
}

#[derive(Default)]
struct Shared {
    state: State,
    /// The monitors showing a wallpaper that reads Foxglove's properties.
    targets: Vec<Target>,
    /// Each wallpaper file's folder version as last seen ([`reinstalled`]).
    versions: BTreeMap<String, Option<Version>>,
    /// The worker's channel, with the worker's number (a dead worker is forgotten by number).
    wake: Option<(u64, Sender<Wake>)>,
    workers: u64,
    /// Foxglove is quitting: no worker starts again.
    quitting: bool,
    placements: u64,
    /// Wallpaper Engine stops wallpapers under a full-screen program (`playbackfullscreen:
    /// "stop"`): the reloaded one starts from its saved properties.
    reloads: bool,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    let mut g = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Shared::default))
}

fn changed(reason: Reason, f: impl FnOnce(&mut Shared)) {
    let wake = with(|s| {
        f(s);
        s.wake.clone()
    });
    if let Some((worker, w)) = wake {
        if w.send(Wake::Changed(reason)).is_err() {
            forget(worker);
        }
    }
}

/// The worker `worker` ended (a panic): forget it, so the next wallpapers on screen start
/// another one instead of every change going nowhere. A newer worker is kept.
fn forget(worker: u64) {
    let forgot = with(|s| {
        let dead = s.wake.as_ref().is_some_and(|(n, _)| *n == worker);
        if dead {
            s.wake = None;
        }
        dead
    });
    if forgot {
        eprintln!("Foxglove: wallpaper sync: the worker stopped; the next wallpaper change starts it again");
    }
}

/// A scene started (`scene_run`): its id, effect, clock and room.
pub fn scene_started(id: &str, effect: &Effect, clock: Clock, room: RoomMap) {
    eprintln!(
        "Foxglove: wallpaper sync: scene {id} on epoch {} ms",
        clock.epoch_unix_ms()
    );
    changed(Reason::Scene, |s| {
        s.placements += 1;
        s.state.scene = Some(Scene {
            id: id.to_string(),
            effect: effect.clone(),
            clock,
            room,
            placement: s.placements,
        });
    });
}

/// The running scene placed its devices again.
pub fn scene_room(room: RoomMap) {
    changed(Reason::Scene, |s| {
        s.placements += 1;
        if let Some(scene) = s.state.scene.as_mut() {
            scene.room = room;
            scene.placement = s.placements;
        }
    });
}

/// The live feed started listening on `port`, or stopped (`None`).
pub fn live(port: Option<u16>) {
    changed(Reason::Live, |s| s.state.live = port);
}

/// A wallpaper connected to the live feed (`live_feed`): it has just loaded, from its saved
/// properties, so the monitors not sent anything in the last [`RECONNECT_FRESH`] are sent
/// everything again. Which monitor it is on isn't known.
pub fn reconnected() {
    wake(Wake::Reconnected);
}

fn wake(w: Wake) {
    if let Some((worker, tx)) = with(|s| s.wake.clone()) {
        if tx.send(w).is_err() {
            forget(worker);
        }
    }
}

/// Whether a wallpaper in `now` was seen before (`seen`, per file, kept while it is off screen
/// or its folder is half copied) with other folder contents: installed again or updated in
/// place. `seen` takes the new versions.
fn reinstalled(seen: &mut BTreeMap<String, Option<Version>>, now: &[Target]) -> bool {
    let mut again = false;
    for t in now {
        if let Some(old) = seen.insert(t.file.clone(), t.version) {
            again |= old != t.version;
        }
    }
    again
}

/// Windows' app mode now (`windows_theme`, at start and on each change).
pub fn theme(mode: Option<super::windows_theme::AppMode>) {
    if with(|s| s.state.theme) != mode {
        changed(Reason::Theme, |s| s.state.theme = mode);
    }
}

/// The scene ended: the wallpapers go back to their own clocks.
pub fn scene_stopped() {
    changed(Reason::Scene, |s| s.state.scene = None);
}

/// The wallpapers on screen, from Wallpaper Engine's config (`wallpaper.rs`, on each change):
/// `targets` are the monitors showing a wallpaper that reads Foxglove's properties, empty
/// without Wallpaper Engine or while it isn't running. `reloads`: its config stops wallpapers
/// under a full-screen program (`playbackfullscreen: "stop"`). The first non-empty list starts
/// the worker.
pub fn wallpapers(targets: Vec<Target>, reloads: bool) {
    let start = with(|s| s.wake.is_none() && !s.quitting && !targets.is_empty());
    if start {
        start_worker();
    }
    let again = with(|s| {
        let again = reinstalled(&mut s.versions, &targets);
        s.targets = targets;
        s.reloads = reloads;
        again
    });
    // A reinstall waits for the copy and Wallpaper Engine's reload; anything else goes at once.
    wake(if again {
        Wake::Reinstalled
    } else {
        Wake::Changed(Reason::Wallpapers)
    });
}

/// Quitting: tells the wallpapers Foxglove has gone (bounded), so they don't keep showing a
/// layer that no longer is.
pub fn quit() {
    let Some((_, wake)) = with(|s| {
        s.quitting = true;
        s.wake.take()
    }) else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    if wake.send(Wake::Quit(tx)).is_ok() {
        let _ = rx.recv_timeout(QUIT_LIMIT);
    }
}

fn start_worker() {
    let (tx, rx) = mpsc::channel::<Wake>();
    // Seed the layer from the facts the keyboard already reported, then follow its changes.
    let facts = super::devices::bus().facts();
    let layers = facts.subscribe("keyboard.layers");
    set_layers(match facts.get("keyboard.layers") {
        Some(foxglove_devices::facts::Value::Bytes(b)) => Some(b),
        _ => None,
    });
    with(|s| {
        s.workers += 1;
        s.wake = Some((s.workers, tx.clone()));
    });
    // The program in front (`foreground.rs`): a full-screen one leaving starts Wallpaper
    // Engine's reload (#586).
    let fronts = facts.subscribe("app.foreground");
    let forward = tx.clone();
    super::services::spawn_named("wallpaper-sync-foreground", move || {
        for _ in fronts {
            if forward.send(Wake::Foreground).is_err() {
                break;
            }
        }
    });
    let forward = tx.clone();
    super::services::spawn_named("wallpaper-sync-layers", move || {
        for change in layers {
            set_layers(match change.value {
                Some(foxglove_devices::facts::Value::Bytes(b)) => Some(b),
                _ => None,
            });
            if forward.send(Wake::Layer).is_err() {
                break;
            }
        }
    });
    // The feed's keyboard changed (#801): its layer, or none.
    let feed = facts.subscribe("focus.feed");
    let forward = tx.clone();
    super::services::spawn_named("wallpaper-sync-keyboard", move || {
        for _ in feed {
            let facts = super::devices::bus().facts();
            set_layers(match facts.get("keyboard.layers") {
                Some(foxglove_devices::facts::Value::Bytes(b)) => Some(b),
                _ => None,
            });
            if forward.send(Wake::Layer).is_err() {
                break;
            }
        }
    });
    super::services::spawn_named("wallpaper-sync", move || {
        run(
            &rx,
            Cli::default(),
            || with(|s| (s.state.clone(), s.targets.clone(), s.reloads)),
            Settle::APP,
            super::live_feed::fullscreen_now,
        );
    });
    let _ = tx.send(Wake::Changed(Reason::Start));
}

fn set_layers(active: Option<Vec<u8>>) {
    // Another keyboard than the host link's in the feed: its layers aren't these.
    let active = active.filter(|_| super::focus_pins::feed_is_host_link());
    let names = super::layer_names::remembered();
    with(|s| {
        s.state.layer = active.as_deref().and_then(|a| layer_name(a, &names));
        s.state.game = active.as_deref().is_some_and(|a| game_mode(a, &names));
    });
}

/// The worker: waits for a change and the run it starts ([`next`]), then pushes the newest
/// state from `read` (the state, the targets, and whether Wallpaper Engine stops wallpapers
/// under a full-screen program). `full` says whether a full-screen program is in front.
fn run(
    rx: &Receiver<Wake>,
    apply: impl Apply,
    read: impl Fn() -> (State, Vec<Target>, bool),
    settle: Settle,
    full: impl FnMut() -> bool,
) {
    let mut pusher = Pusher::new(apply);
    let mut screen = Screen::new(full);
    let mut last_scene: Option<(String, i64)> = None;
    // A reload under Stop that hasn't been answered yet (the program came back first).
    let mut resend = false;
    let mut leases = Leases::default();
    // A run a lease call interrupted, and the quiet after a play.
    let mut carry: Option<Run> = None;
    loop {
        let (mut reason, left_full, reconnected) =
            match next_from(rx, settle, &mut screen, carry.take()) {
                Next::Control(ctl, run) => {
                    carry = run;
                    let full = screen.now();
                    if serve(ctl, &mut leases, pusher.apply_mut(), full) {
                        // Wallpaper Engine resumes: a `-control` command meanwhile lands in it.
                        let now = Instant::now();
                        let r =
                            carry.get_or_insert_with(|| Run::new(Reason::Wallpapers, None, now));
                        let until = now + settle.reload;
                        r.quiet_until = Some(r.quiet_until.map_or(until, |q| q.max(until)));
                    }
                    announce(&leases);
                    continue;
                }
                Next::Sync {
                    reason,
                    left_full,
                    reconnected,
                } => (reason, left_full, reconnected),
                Next::Quit(ack) => {
                    // Play first (when it is ours to play), then tell the wallpapers we've gone.
                    let full = screen.now();
                    if leases.end_all(pusher.apply_mut(), full).is_some() {
                        announce(&leases);
                    }
                    pusher.quit();
                    let _ = ack.send(());
                    return;
                }
                Next::Closed => return,
            };
        let (state, targets, reloads) = read();
        resend |= left_full && reloads;
        if leases.paused_by_lease() {
            // Paused for a lease: the newest state goes once it plays (the sent state is kept).
            continue;
        }
        let resent = resend && !screen.full;
        if resent {
            // The reloaded wallpaper starts from its saved properties: everything goes once.
            pusher.resend();
            resend = false;
            reason = Reason::Reload;
        } else if reconnected && pusher.resend_stale(settle.fresh, Instant::now()) > 0 {
            // A wallpaper just loaded and connected: it starts from its saved properties.
            reason = Reason::Reconnect;
        }
        let synced = pusher.sync_counted(&state, &targets);
        let pushed = synced.pushed;
        let (minute, suppressed) = {
            let mut h = (0, 0);
            health::note(|s| {
                if left_full {
                    s.reload(resent);
                }
                s.synced(reason, pushed, synced.unchanged, Instant::now());
                if let Some(e) = synced.failed.clone() {
                    s.failed(e);
                }
                h = (s.per_minute(Instant::now()), s.suppressed);
            });
            h
        };
        // Every push starts a Wallpaper Engine process: each one shows in the log with why it
        // went, so a flicker can be lined up with them.
        let why = format!(
            "{}; {minute} in the last minute, {suppressed} duplicate(s) suppressed",
            reason.label()
        );
        let scene = state
            .scene
            .as_ref()
            .map(|s| (s.id.clone(), s.clock.epoch_unix_ms()));
        if pushed > 0 && scene != last_scene {
            match &scene {
                Some((id, epoch)) => eprintln!(
                    "Foxglove: wallpaper sync: scene {id} (epoch {epoch}) sent to {pushed} monitor(s) ({why})"
                ),
                None => eprintln!(
                    "Foxglove: wallpaper sync: no scene, sent to {pushed} monitor(s) ({why})"
                ),
            }
            last_scene = scene;
        } else if pushed > 0 {
            eprintln!(
                "Foxglove: wallpaper sync: sent to {pushed} monitor(s): layer {}, game mode {}, Windows {} ({why})",
                state.layer.as_deref().unwrap_or("none"),
                if state.game { "on" } else { "off" },
                state
                    .theme
                    .map_or("unknown", super::windows_theme::AppMode::property)
            );
        }
    }
}

/// Runs one lease call; `true` when it sent `play`.
fn serve(ctl: Ctl, leases: &mut Leases, apply: &mut impl Apply, full: bool) -> bool {
    let reply = |to: Sender<Reply>, r: Reply| {
        let _ = to.send(r);
    };
    match ctl {
        Ctl::Pause {
            conn,
            reason,
            user_paused,
            reply: to,
        } => {
            let r = match leases.pause(apply, conn, &reason, user_paused) {
                Ok((lease, paused)) => Reply::Paused { lease, paused },
                Err(e) => Reply::Failed(e),
            };
            reply(to, r);
            false
        }
        Ctl::Release {
            conn,
            lease,
            reply: to,
        } => match leases.release(apply, conn, lease, full) {
            Ok(ended) => {
                reply(to, Reply::Released(ended));
                ended.played
            }
            Err(e) => {
                reply(to, Reply::Failed(e));
                false
            }
        },
        Ctl::Status(to) => {
            let running = apply.ready();
            reply(
                to,
                Reply::Status {
                    running,
                    leases: leases.count(),
                    paused_by_lease: leases.paused_by_lease(),
                },
            );
            false
        }
        Ctl::Closed(conn) => leases
            .drop_conn(apply, conn, full)
            .is_some_and(|ended| ended.played),
    }
}

/// The `wallpaper_engine.paused_by` fact and the health counts, after the leases changed.
fn announce(leases: &Leases) {
    let facts = super::devices::bus().facts();
    match leases.reasons() {
        r if r.is_empty() => {
            facts.remove(PAUSED_BY);
        }
        r => facts.set(PAUSED_BY, foxglove_devices::facts::Value::Text(r)),
    }
    health::note(|h| {
        h.lease_pauses = leases.paused;
        h.lease_plays = leases.played;
        h.lease_dropped = leases.dropped;
    });
}

/// How long a lease call waits for the worker, which may be in a push (5 s each).
const CONTROL_WAIT: Duration = Duration::from_secs(20);

/// A lease call of the local API (#1120): `make` builds the request around the reply channel.
/// `Pause` (`start`) starts the worker when Wallpaper Engine runs and none does; the others
/// need one.
fn control(make: impl FnOnce(Sender<Reply>) -> Ctl, start: bool) -> Result<Reply, Fail> {
    if start {
        if super::wallpaper_engine::cli_exe(&super::wallpaper_engine::presence()).is_none() {
            return Err(Fail::NotInstalled);
        }
        if !super::wallpaper_engine::running_now() {
            return Err(Fail::NotRunning);
        }
        if with(|s| s.wake.is_none() && !s.quitting) {
            start_worker();
        }
    }
    let Some((_, tx)) = with(|s| s.wake.clone()) else {
        return Err(if start {
            Fail::NotRunning
        } else {
            Fail::Unknown
        });
    };
    let (reply, answer) = mpsc::channel();
    tx.send(Wake::Control(make(reply)))
        .map_err(|_| Fail::NotRunning)?;
    answer
        .recv_timeout(CONTROL_WAIT)
        .map_err(|_| Fail::Command("Wallpaper Engine did not answer in time".into()))
}

/// `wallpaper-engine.pause`: a lease; the first one pauses Wallpaper Engine.
pub fn pause(conn: u64, reason: &str) -> Result<(u64, bool), Fail> {
    let user_paused = super::live_feed::wallpaper_paused();
    let reason = reason.to_string();
    let ask = |reply| Ctl::Pause {
        conn,
        reason,
        user_paused,
        reply,
    };
    match control(ask, true)? {
        Reply::Paused { lease, paused } => Ok((lease, paused)),
        Reply::Failed(e) => Err(e),
        _ => Err(Fail::Unknown),
    }
}

/// `wallpaper-engine.release`: ends a lease; the last one plays.
pub fn release(conn: u64, lease: u64) -> Result<Ended, Fail> {
    match control(|reply| Ctl::Release { conn, lease, reply }, false)? {
        Reply::Released(e) => Ok(e),
        Reply::Failed(e) => Err(e),
        _ => Err(Fail::Unknown),
    }
}

/// `wallpaper-engine.status`: no worker means no leases.
pub fn status() -> (bool, u32, bool) {
    match control(Ctl::Status, false) {
        Ok(Reply::Status {
            running,
            leases,
            paused_by_lease,
        }) => (running, leases, paused_by_lease),
        _ => (super::wallpaper_engine::running_now(), 0, false),
    }
}

/// A connection of the local API closed: its leases end (no answer is waited for).
pub fn connection_closed(conn: u64) {
    wake(Wake::Control(Ctl::Closed(conn)));
}

/// Wallpaper Engine's command line.
#[derive(Default)]
struct Cli {
    exe: Option<PathBuf>,
}

impl Apply for Cli {
    fn ready(&mut self) -> bool {
        if !super::wallpaper_engine::running_now() {
            return false;
        }
        if self.exe.is_none() {
            self.exe = super::wallpaper_engine::cli_exe(&super::wallpaper_engine::presence());
        }
        self.exe.is_some()
    }

    fn apply(&mut self, location: &str, props: &str) -> Result<(), String> {
        // Checked again before every push: if Wallpaper Engine quit meanwhile, its command
        // line would start it.
        if !super::wallpaper_engine::running_now() {
            return Err("Wallpaper Engine isn't running".into());
        }
        let exe = self.exe.clone().ok_or("no Wallpaper Engine program")?;
        let (args, raw) = arguments(location, props);
        run_cli(&exe, &args, &raw)
    }

    fn control(&mut self, command: &str) -> Result<(), String> {
        // Only these two; the same running check as a push.
        if !matches!(command, "pause" | "play") {
            return Err(format!("not a control command: {command}"));
        }
        if !super::wallpaper_engine::running_now() {
            return Err("Wallpaper Engine isn't running".into());
        }
        let exe = self.exe.clone().ok_or("no Wallpaper Engine program")?;
        run_cli(&exe, &["-control".into(), command.into()], "")
    }

    fn interfaces(&mut self) -> Vec<(String, String)> {
        interfaces()
    }
}

#[cfg(windows)]
fn run_cli(exe: &std::path::Path, args: &[String], raw: &str) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{WaitForSingleObject, CREATE_NO_WINDOW};

    let mut command = std::process::Command::new(exe);
    command.args(args);
    if !raw.is_empty() {
        command.raw_arg(raw);
    }
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map_err(|e| e.to_string())?;
    // Waits on the process handle (an event), bounded.
    let handle = HANDLE(child.as_raw_handle());
    // SAFETY: the child's process handle, alive while `child` is.
    let waited = unsafe { WaitForSingleObject(handle, PUSH_LIMIT.as_millis() as u32) };
    if waited != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("no answer in {} s", PUSH_LIMIT.as_secs()));
    }
    let _ = child.wait();
    Ok(())
}

#[cfg(not(windows))]
fn run_cli(_exe: &std::path::Path, _args: &[String], _raw: &str) -> Result<(), String> {
    Err("Wallpaper Engine runs on Windows only".into())
}

/// Each Windows display (`\\.\DISPLAY1`) with its monitor's interface path, the key Wallpaper
/// Engine uses in its config.
#[cfg(windows)]
fn interfaces() -> Vec<(String, String)> {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};
    use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;

    let text = |w: &[u16]| {
        let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..n])
    };
    super::scene_room::monitors()
        .into_iter()
        .filter_map(|m| {
            let name: Vec<u16> = m.id.encode_utf16().chain(std::iter::once(0)).collect();
            let mut dd = DISPLAY_DEVICEW {
                cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            // SAFETY: a NUL-terminated name and a sized, writable DISPLAY_DEVICEW.
            let ok = unsafe {
                EnumDisplayDevicesW(
                    PCWSTR(name.as_ptr()),
                    0,
                    &mut dd,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            };
            ok.as_bool()
                .then(|| (m.id.clone(), text(&dd.DeviceID)))
                .filter(|(_, iface)| !iface.is_empty())
        })
        .collect()
}

#[cfg(not(windows))]
fn interfaces() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::windows_theme::AppMode;
    use foxglove_scene::{Layout, MonitorInfo};

    fn names() -> Vec<Option<String>> {
        ["QWERTY", "NUMBER", "SYMBOL", "GAME", "GAME+", "FUNCTION"]
            .iter()
            .map(|n| Some((*n).to_string()))
            .collect()
    }

    fn room() -> RoomMap {
        let m = |id: &str, x: i32, primary: bool| MonitorInfo {
            id: id.into(),
            primary,
            rect_px: [x, 0, x + 2560, 1440],
            raw_dpi: Some(109),
            dpi: 96,
        };
        RoomMap::build(
            &[m(r"\\.\DISPLAY1", 0, true), m(r"\\.\DISPLAY2", 2560, false)],
            &[],
            &Layout::default(),
        )
    }

    const WE_LEFT: &str =
        "//?/DISPLAY#SAM0E3C#5&26cfee32&1&UID8449#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    const WE_RIGHT: &str =
        "//?/DISPLAY#DEL40EA#5&26cfee32&1&UID8451#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

    fn ifaces() -> Vec<(String, String)> {
        vec![
            (
                r"\\.\DISPLAY1".into(),
                r"\\?\DISPLAY#SAM0E3C#5&26cfee32&1&UID8449#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
                    .into(),
            ),
            (
                r"\\.\DISPLAY2".into(),
                r"\\?\DISPLAY#DEL40EA#5&26cfee32&1&UID8451#{E6F07B5F-EE97-4A90-B076-33F57BF4EAA7}"
                    .into(),
            ),
        ]
    }

    fn scene() -> Scene {
        Scene {
            id: "demo scanline".into(),
            effect: super::super::scene_run::demo_effect(),
            clock: Clock::aligned(1_761_000_000_000),
            room: room(),
            placement: 1,
        }
    }

    /// Records every push; `running` plays Wallpaper Engine being up.
    #[derive(Default)]
    struct Fake {
        running: bool,
        pushes: Vec<(String, String)>,
        interface_reads: usize,
        fail: bool,
        controls: Vec<String>,
    }

    impl Apply for &mut Fake {
        fn ready(&mut self) -> bool {
            self.running
        }
        fn apply(&mut self, location: &str, props: &str) -> Result<(), String> {
            if self.fail {
                return Err("refused".into());
            }
            self.pushes.push((location.into(), props.into()));
            Ok(())
        }
        fn control(&mut self, command: &str) -> Result<(), String> {
            self.controls.push(command.into());
            Ok(())
        }
        fn interfaces(&mut self) -> Vec<(String, String)> {
            self.interface_reads += 1;
            ifaces()
        }
    }

    #[test]
    fn layer_names_and_game_mode_come_from_the_active_layers() {
        let n = names();
        assert_eq!(layer_name(&[0], &n).as_deref(), Some("QWERTY"));
        assert_eq!(layer_name(&[0, 3, 4], &n).as_deref(), Some("GAME+"));
        assert_eq!(layer_name(&[0, 9], &n).as_deref(), Some("L9"));
        assert_eq!(layer_name(&[], &n), None);
        assert!(game_mode(&[0, 3], &n));
        assert!(game_mode(&[0, 3, 4, 5], &n));
        assert!(!game_mode(&[0, 1, 2], &n));
        assert!(!game_mode(&[3], &[]));
        let odd = vec![Some(" nav \"layer\" ".to_string())];
        assert_eq!(layer_name(&[0], &odd).as_deref(), Some("NAV_LAYER"));
    }

    #[test]
    fn the_command_line_is_compact_json_between_the_raw_markers() {
        let state = State {
            layer: Some("GAME".into()),
            game: true,
            scene: None,
            live: Some(8768),
            theme: Some(AppMode::Dark),
        };
        let props = properties(&state, None, Supports::ALL);
        assert_eq!(
            props,
            r#"{"foxglove_game":true,"foxglove_layer":"GAME","foxglove_live":8768,"foxglove_scene":"","foxglove_theme":"dark"}"#
        );
        // A wallpaper gets only what its manifest says it reads.
        let live_only = Supports {
            live: true,
            ..Supports::default()
        };
        assert_eq!(
            properties(&state, None, live_only),
            r#"{"foxglove_live":8768}"#
        );
        assert_eq!(
            properties(
                &state,
                None,
                Supports {
                    clock: true,
                    layer: true,
                    ..Supports::default()
                }
            ),
            r#"{"foxglove_layer":"GAME","foxglove_scene":""}"#
        );
        let (args, raw) = arguments(WE_LEFT, &props);
        assert_eq!(args, ["-control", "applyProperties", "-location", WE_LEFT]);
        assert_eq!(raw, format!("-properties RAW~({props})~END"));
        assert_eq!(raw.matches(' ').count(), 1, "one space, after -properties");
        assert_eq!(
            gone(Supports::ALL),
            r#"{"foxglove_game":false,"foxglove_layer":"","foxglove_live":0,"foxglove_scene":"","foxglove_theme":""}"#
        );
    }

    /// "Follow Windows" (`foxglove_theme`): every monitor showing a wallpaper that reads it gets
    /// the same mode, again on each change, and "" once Foxglove has gone (the wallpaper then
    /// falls back to its own reading).
    #[test]
    fn every_monitor_hears_the_same_windows_app_mode() {
        let theme_only = Supports {
            theme: true,
            ..Supports::default()
        };
        assert_eq!(theme_only, Supports::from_names(&["theme"]));
        let targets = vec![
            Target {
                location: WE_LEFT.into(),
                file: "hud/index.html".into(),
                supports: Supports::ALL,
                version: None,
            },
            Target {
                location: WE_RIGHT.into(),
                file: "live-board/index.html".into(),
                supports: theme_only,
                version: None,
            },
        ];
        let mut fake = Fake {
            running: true,
            ..Fake::default()
        };
        let mut state = State {
            theme: Some(AppMode::Light),
            ..State::default()
        };
        {
            let mut p = Pusher::new(&mut fake);
            assert_eq!(p.sync(&state, &targets), 2);
            assert_eq!(p.sync(&state, &targets), 0, "no change, no push");
            state.theme = Some(AppMode::Dark);
            assert_eq!(p.sync(&state, &targets), 2);
            p.quit();
        }
        let modes = |props: &str| {
            let v: serde_json::Value = serde_json::from_str(props).unwrap();
            v[THEME].as_str().unwrap().to_string()
        };
        let sent: Vec<(String, String)> = fake
            .pushes
            .iter()
            .map(|(at, props)| (at.clone(), modes(props)))
            .collect();
        assert_eq!(sent.len(), 6);
        assert_eq!(
            sent[..4],
            [
                (WE_LEFT.to_string(), "light".to_string()),
                (WE_RIGHT.to_string(), "light".to_string()),
                (WE_LEFT.to_string(), "dark".to_string()),
                (WE_RIGHT.to_string(), "dark".to_string()),
            ]
        );
        // Quitting tells both (in the monitors' order).
        let mut gone: Vec<&(String, String)> = sent[4..].iter().collect();
        gone.sort();
        assert_eq!(
            gone,
            [
                &(WE_RIGHT.to_string(), String::new()),
                &(WE_LEFT.to_string(), String::new()),
            ]
        );
        // Windows doesn't say: "" (the wallpaper's own fallback).
        assert_eq!(
            properties(&State::default(), None, theme_only),
            r#"{"foxglove_theme":""}"#
        );
    }

    #[test]
    fn each_monitor_gets_its_own_rectangle_in_the_room() {
        let r = room();
        let left = monitor_rect(WE_LEFT, &r, &ifaces()).unwrap();
        let right = monitor_rect(WE_RIGHT, &r, &ifaces()).unwrap();
        assert_eq!(left, r.device(r"monitor:\\.\DISPLAY1").unwrap().rect);
        assert!(right.x >= left.x + left.w - 0.5, "{left:?} {right:?}");
        assert_eq!(monitor_rect("Monitor0", &r, &ifaces()), None);
        let mut state = State {
            scene: Some(scene()),
            ..State::default()
        };
        state.layer = Some("QWERTY".into());
        let a = properties(&state, Some(left), Supports::ALL);
        let b = properties(&state, Some(right), Supports::ALL);
        assert_ne!(a, b);
        assert!(
            a.contains("fx=scanline;e=1761000000000;p=4000;a=90;w=80"),
            "{a}"
        );
        assert!(a.contains(";mon=0,0,"), "{a}");
        assert!(!a.contains(' '));
    }

    fn target(location: &str, file: &str) -> Target {
        Target {
            location: location.into(),
            file: file.into(),
            supports: Supports::ALL,
            version: None,
        }
    }

    #[test]
    fn pushes_go_only_on_change_and_only_while_wallpaper_engine_runs() {
        let mut fake = Fake::default();
        let reef = "C:/WE/projects/myprojects/reef/index.html";
        let targets = vec![target(WE_LEFT, reef), target(WE_RIGHT, reef)];
        let mut state = State::default();
        {
            let mut p = Pusher::new(&mut fake);
            // Not running: nothing (its command line would start it).
            assert_eq!(p.sync(&state, &targets), 0);
        }
        assert!(fake.pushes.is_empty());
        fake.running = true;
        let mut p = Pusher::new(&mut fake);
        assert_eq!(p.sync(&state, &targets), 2, "the first sync tells both");
        assert_eq!(p.sync(&state, &targets), 0, "nothing changed");
        state.layer = Some("NUMBER".into());
        assert_eq!(p.sync(&state, &targets), 2);
        assert_eq!(
            p.sync(&state, &targets[..1]),
            0,
            "a monitor going adds nothing"
        );
        state.scene = Some(scene());
        assert_eq!(p.sync(&state, &targets), 2, "a scene start");
        assert_eq!(p.sync(&state, &targets), 0);
        // Another wallpaper on one monitor starts from its defaults: it is sent again.
        let lighthouse = "D:/Lib/steamapps/workshop/content/431960/777/scene.pkg";
        let swapped = vec![targets[0].clone(), target(WE_RIGHT, lighthouse)];
        assert_eq!(p.sync(&state, &swapped), 1, "a swapped wallpaper");
        assert_eq!(p.sync(&state, &swapped), 0);
        // The devices placed again: the interface paths are read again.
        state.scene.as_mut().unwrap().placement = 2;
        assert_eq!(p.sync(&state, &swapped), 0, "same room, nothing to send");
        state.scene = None;
        assert_eq!(p.sync(&state, &targets), 2, "a scene stop");
        assert_eq!(
            p.sync(&state, &[]),
            0,
            "no wallpaper that reads them: nothing"
        );
        p.quit();
        drop(p);
        assert_eq!(fake.pushes.len(), 9);
        assert_eq!(
            fake.interface_reads, 2,
            "interfaces read once per scene placement"
        );
        let scene_push = &fake.pushes[4].1;
        assert!(scene_push.contains("v=1;id=demo-scanline;fx=scanline"));
        assert!(fake.pushes[7].1.contains(r#""foxglove_scene":"""#));
    }

    #[test]
    fn quitting_clears_what_was_sent_and_failures_are_sent_again() {
        let mut fake = Fake {
            running: true,
            ..Fake::default()
        };
        let targets = vec![target(WE_LEFT, "reef")];
        let state = State {
            layer: Some("GAME".into()),
            game: true,
            scene: Some(scene()),
            live: None,
            theme: None,
        };
        {
            let mut p = Pusher::new(&mut fake);
            p.sync(&state, &targets);
            p.quit();
            p.quit();
        }
        assert_eq!(fake.pushes.len(), 2);
        assert_eq!(fake.pushes[1].1, gone(Supports::ALL));
        fake.pushes.clear();
        fake.fail = true;
        let mut p = Pusher::new(&mut fake);
        assert_eq!(p.sync(&state, &targets), 0);
        p.apply.fail = false;
        assert_eq!(
            p.sync(&state, &targets),
            1,
            "a failed push goes again on the next change"
        );
        // Wallpaper Engine quit and came back: everything goes again.
        p.apply.running = false;
        assert_eq!(p.sync(&state, &targets), 0);
        p.apply.running = true;
        assert_eq!(p.sync(&state, &targets), 1);
    }

    /// A third-party wallpaper that asks for the live feed's port and the scene clock (#562): it
    /// hears the port when the feed starts and 0 when Foxglove goes, and never the layer.
    #[test]
    fn a_third_party_wallpaper_pairs_with_the_live_feed() {
        use super::super::wallpaper_manifest::tests::fake_manifests;
        let reef = fake_manifests().remove(0);
        let mut fake = Fake {
            running: true,
            ..Fake::default()
        };
        let targets = vec![Target {
            location: WE_LEFT.into(),
            file: "C:/WE/projects/myprojects/reef/index.html".into(),
            supports: reef.supports,
            version: None,
        }];
        let mut state = State::default();
        {
            let mut p = Pusher::new(&mut fake);
            p.sync(&state, &targets);
            state.live = Some(8768);
            p.sync(&state, &targets);
            state.layer = Some("GAME".into());
            assert_eq!(p.sync(&state, &targets), 0, "it doesn't read the layer");
            p.quit();
        }
        let sent: Vec<&str> = fake.pushes.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(
            sent,
            [
                r#"{"foxglove_live":0,"foxglove_scene":""}"#,
                r#"{"foxglove_live":8768,"foxglove_scene":""}"#,
                r#"{"foxglove_live":0,"foxglove_scene":""}"#
            ]
        );
    }

    /// No full-screen program in front.
    fn windowed() -> Screen<fn() -> bool> {
        Screen::<fn() -> bool>::new(|| false)
    }

    fn quick() -> Settle {
        Settle {
            layer: Duration::from_millis(150),
            max: Duration::from_secs(5),
            ..Settle::APP
        }
    }

    /// Long enough that a test only sees it pass if the change went at once.
    fn slow() -> Settle {
        Settle {
            layer: Duration::from_secs(60),
            max: Duration::from_secs(60),
            ..Settle::APP
        }
    }

    #[test]
    fn a_run_of_layer_changes_settles_into_one_sync() {
        let (tx, rx) = mpsc::channel();
        for _ in 0..3 {
            tx.send(Wake::Layer).unwrap();
        }
        let t = Instant::now();
        assert!(matches!(
            next(&rx, quick(), &mut windowed()),
            Next::Sync { .. }
        ));
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(150), "{took:?}");
        assert!(rx.try_recv().is_err(), "the whole run is taken at once");
        // A scene, the live feed or the wallpapers changing goes at once, even mid-run, and a
        // layer change after it doesn't hold it back.
        tx.send(Wake::Layer).unwrap();
        tx.send(Wake::Changed(Reason::Scene)).unwrap();
        tx.send(Wake::Layer).unwrap();
        let t = Instant::now();
        assert!(matches!(
            next(&rx, slow(), &mut windowed()),
            Next::Sync { .. }
        ));
        assert!(t.elapsed() < Duration::from_secs(30));
        assert!(rx.try_recv().is_err());
        // Quitting wins over a run.
        let (ack, _acked) = mpsc::channel();
        tx.send(Wake::Layer).unwrap();
        tx.send(Wake::Quit(ack)).unwrap();
        assert!(matches!(next(&rx, slow(), &mut windowed()), Next::Quit(_)));
        drop(tx);
        assert!(matches!(next(&rx, quick(), &mut windowed()), Next::Closed));
    }

    #[test]
    fn a_run_that_keeps_going_is_sent_at_its_cap() {
        use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
        let (tx, rx) = mpsc::channel();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let typing = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                // Layers changing for up to 5 s, faster than they settle.
                let end = Instant::now() + Duration::from_secs(5);
                while Instant::now() < end && !stop.load(Relaxed) {
                    if tx.send(Wake::Layer).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
        };
        let settle = Settle {
            layer: Duration::from_millis(100),
            max: Duration::from_millis(300),
            ..Settle::APP
        };
        let t = Instant::now();
        assert!(matches!(
            next(&rx, settle, &mut windowed()),
            Next::Sync { .. }
        ));
        let took = t.elapsed();
        stop.store(true, Relaxed);
        typing.join().unwrap();
        assert!(took >= Duration::from_millis(300), "{took:?}");
        assert!(took < Duration::from_secs(4), "{took:?}");
    }

    /// Wallpaper Engine's command line as a channel.
    struct Wire(Sender<String>);

    impl Apply for Wire {
        fn ready(&mut self) -> bool {
            true
        }
        fn apply(&mut self, _location: &str, props: &str) -> Result<(), String> {
            let _ = self.0.send(props.to_string());
            Ok(())
        }
        fn control(&mut self, command: &str) -> Result<(), String> {
            let _ = self.0.send(format!("control {command}"));
            Ok(())
        }
        fn interfaces(&mut self) -> Vec<(String, String)> {
            ifaces()
        }
    }

    /// The user's report: in a game, Win+Tab (GAME+ held for Tab) and back. Wallpaper Engine
    /// hears nothing for a layer held and let go; a layer that stays is sent once.
    #[test]
    fn a_layer_held_for_the_win_key_sends_nothing() {
        let state = std::sync::Arc::new(Mutex::new(State {
            layer: Some("GAME".into()),
            game: true,
            ..State::default()
        }));
        let (pushes_tx, pushes) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        // Settles long enough that a stalled test thread can't split a hold in two.
        let settle = Settle {
            layer: Duration::from_millis(800),
            max: Duration::from_secs(10),
            ..Settle::APP
        };
        let worker = {
            let state = state.clone();
            std::thread::spawn(move || {
                let read = move || {
                    let s = state.lock().unwrap().clone();
                    (
                        s,
                        vec![target(WE_LEFT, "hud"), target(WE_RIGHT, "stars")],
                        false,
                    )
                };
                run(&rx, Wire(pushes_tx), read, settle, || false);
            })
        };
        let set = |layer: &str| state.lock().unwrap().layer = Some(layer.into());
        let wait = Duration::from_secs(5);
        tx.send(Wake::Changed(Reason::Scene)).unwrap();
        for _ in 0..2 {
            let first = pushes.recv_timeout(wait).expect("the first state goes out");
            assert!(first.contains(r#""foxglove_layer":"GAME""#), "{first}");
        }

        // Hold GAME+ for Tab, then let go: the same as before, so nothing is sent.
        set("GAME+");
        tx.send(Wake::Layer).unwrap();
        set("GAME");
        tx.send(Wake::Layer).unwrap();
        assert!(
            pushes.recv_timeout(Duration::from_secs(2)).is_err(),
            "Wallpaper Engine hears nothing"
        );

        // A layer that stays goes out once per monitor.
        set("GAME+");
        tx.send(Wake::Layer).unwrap();
        for _ in 0..2 {
            let held = pushes.recv_timeout(wait).expect("a settled layer goes out");
            assert!(held.contains(r#""foxglove_layer":"GAME+""#), "{held}");
        }
        assert!(pushes.recv_timeout(Duration::from_millis(300)).is_err());

        let (ack, acked) = mpsc::channel();
        tx.send(Wake::Quit(ack)).unwrap();
        acked.recv_timeout(wait).expect("quit answers");
        assert_eq!(pushes.recv_timeout(wait).unwrap(), gone(Supports::ALL));
        worker.join().unwrap();
    }

    /// Wallpaper Engine's command line as a shared log; a push waits at `gate` while it is shut.
    #[derive(Clone)]
    struct Logged {
        log: std::sync::Arc<Mutex<Vec<String>>>,
        gate: std::sync::Arc<(Mutex<bool>, std::sync::Condvar)>,
        pushing: Sender<()>,
    }

    impl Logged {
        fn new(open: bool) -> (Logged, Receiver<()>) {
            let (pushing, seen) = mpsc::channel();
            let one = Logged {
                log: Default::default(),
                gate: std::sync::Arc::new((Mutex::new(open), std::sync::Condvar::new())),
                pushing,
            };
            (one, seen)
        }
        fn open(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Apply for Logged {
        fn ready(&mut self) -> bool {
            true
        }
        fn apply(&mut self, _: &str, props: &str) -> Result<(), String> {
            self.log.lock().unwrap().push("apply start".into());
            let _ = self.pushing.send(());
            let mut open = self.gate.0.lock().unwrap();
            while !*open {
                open = self.gate.1.wait(open).unwrap();
            }
            drop(open);
            let what = if props.contains("\"B\"") {
                "B"
            } else {
                "other"
            };
            self.log.lock().unwrap().push(format!("apply end {what}"));
            Ok(())
        }
        fn control(&mut self, command: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(command.to_string());
            Ok(())
        }
        fn interfaces(&mut self) -> Vec<(String, String)> {
            ifaces()
        }
    }

    fn pause_ctl(conn: u64, user_paused: bool) -> (Wake, Receiver<Reply>) {
        let (reply, got) = mpsc::channel();
        let ctl = Ctl::Pause {
            conn,
            reason: "model loaded".into(),
            user_paused,
            reply,
        };
        (Wake::Control(ctl), got)
    }

    fn release_ctl(conn: u64, lease: u64) -> (Wake, Receiver<Reply>) {
        let (reply, got) = mpsc::channel();
        (Wake::Control(Ctl::Release { conn, lease, reply }), got)
    }

    fn layer_state(layer: &str) -> std::sync::Arc<Mutex<State>> {
        std::sync::Arc::new(Mutex::new(State {
            layer: Some(layer.into()),
            ..State::default()
        }))
    }

    type Flag = std::sync::Arc<std::sync::atomic::AtomicBool>;

    fn spawn_worker(
        apply: Logged,
        state: &std::sync::Arc<Mutex<State>>,
        full: &Flag,
    ) -> (Sender<Wake>, std::thread::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let (state, full) = (state.clone(), full.clone());
        let worker = std::thread::spawn(move || {
            let read = move || {
                (
                    state.lock().unwrap().clone(),
                    vec![target(WE_LEFT, "hud")],
                    false,
                )
            };
            run(&rx, apply, read, gaming(), screen(&full));
        });
        (tx, worker)
    }

    fn quit(tx: &Sender<Wake>, worker: std::thread::JoinHandle<()>) {
        let (ack, acked) = mpsc::channel();
        tx.send(Wake::Quit(ack)).unwrap();
        acked
            .recv_timeout(Duration::from_secs(5))
            .expect("quit answers");
        worker.join().unwrap();
    }

    /// #1120: a pause asked for during a push waits for it (one Wallpaper Engine process at a
    /// time); pushes during the pause hold, and the newest goes once, after the play and the
    /// reload quiet.
    #[test]
    fn a_pause_during_a_push_is_serialised_and_pushes_wait_for_the_play() {
        let wait = Duration::from_secs(5);
        let (apply, pushing) = Logged::new(false);
        let (state, full) = (layer_state("A"), Flag::default());
        let (tx, worker) = spawn_worker(apply.clone(), &state, &full);
        tx.send(Wake::Changed(Reason::Scene)).unwrap();
        pushing.recv_timeout(wait).expect("the push starts");

        let (pause, paused) = pause_ctl(1, false);
        tx.send(pause).unwrap();
        assert!(
            paused.recv_timeout(Duration::from_millis(300)).is_err(),
            "the pause waits while the push runs"
        );
        assert_eq!(apply.log(), ["apply start"], "no second process meanwhile");
        apply.open();
        assert!(matches!(
            paused.recv_timeout(wait).unwrap(),
            Reply::Paused { paused: true, .. }
        ));
        assert_eq!(apply.log(), ["apply start", "apply end other", "pause"]);

        // A change while paused: held, not sent.
        state.lock().unwrap().layer = Some("B".into());
        tx.send(Wake::Changed(Reason::Layer)).unwrap();
        assert!(pushing.recv_timeout(Duration::from_millis(400)).is_err());

        let (release, released) = release_ctl(1, 1);
        tx.send(release).unwrap();
        assert!(matches!(
            released.recv_timeout(wait).unwrap(),
            Reply::Released(Ended { played: true, .. })
        ));
        pushing
            .recv_timeout(wait)
            .expect("the newest state goes after the play");
        quit(&tx, worker);
        assert_eq!(
            apply.log()[3..],
            [
                "play",
                "apply start",
                "apply end B",
                "apply start",
                "apply end other"
            ],
            "play, the held push once, then the quit's `gone`"
        );
    }

    /// #1120: a connection that ends ends its lease, but with a full-screen program in front the
    /// play is left to Wallpaper Engine.
    #[test]
    fn a_closed_connection_ends_its_lease_and_a_full_screen_program_defers_the_play() {
        let wait = Duration::from_secs(5);
        let (apply, _pushing) = Logged::new(true);
        let (state, full) = (layer_state("A"), Flag::default());
        let (tx, worker) = spawn_worker(apply.clone(), &state, &full);
        for (conn, in_front) in [(1, false), (2, true)] {
            full.store(in_front, std::sync::atomic::Ordering::SeqCst);
            let (pause, paused) = pause_ctl(conn, false);
            tx.send(pause).unwrap();
            paused.recv_timeout(wait).unwrap();
            tx.send(Wake::Control(Ctl::Closed(conn))).unwrap();
            let (status, answer) = mpsc::channel();
            tx.send(Wake::Control(Ctl::Status(status))).unwrap();
            assert!(matches!(
                answer.recv_timeout(wait).unwrap(),
                Reply::Status {
                    leases: 0,
                    paused_by_lease: false,
                    ..
                }
            ));
        }
        quit(&tx, worker);
        assert_eq!(
            apply.log(),
            ["pause", "play", "pause"],
            "the second play is Wallpaper Engine's"
        );
    }

    /// #1120: quitting with a lease plays first (when it is ours to play), then says `gone`.
    #[test]
    fn quitting_with_a_lease_plays_then_says_gone() {
        let wait = Duration::from_secs(5);
        let (apply, pushing) = Logged::new(true);
        let (state, full) = (layer_state("A"), Flag::default());
        let (tx, worker) = spawn_worker(apply.clone(), &state, &full);
        tx.send(Wake::Changed(Reason::Scene)).unwrap();
        pushing.recv_timeout(wait).unwrap();
        let (pause, paused) = pause_ctl(1, false);
        tx.send(pause).unwrap();
        paused.recv_timeout(wait).unwrap();
        quit(&tx, worker);
        assert_eq!(
            apply.log(),
            [
                "apply start",
                "apply end other",
                "pause",
                "play",
                "apply start",
                "apply end other"
            ]
        );
    }

    /// #1120: the user's own pause is reported, not played over.
    #[test]
    fn the_users_own_pause_is_not_played_over_by_the_worker() {
        let wait = Duration::from_secs(5);
        let (apply, _pushing) = Logged::new(true);
        let (state, full) = (layer_state("A"), Flag::default());
        let (tx, worker) = spawn_worker(apply.clone(), &state, &full);
        let (pause, paused) = pause_ctl(1, true);
        tx.send(pause).unwrap();
        assert!(matches!(
            paused.recv_timeout(wait).unwrap(),
            Reply::Paused { paused: false, .. }
        ));
        let (release, released) = release_ctl(1, 1);
        tx.send(release).unwrap();
        match released.recv_timeout(wait).unwrap() {
            Reply::Released(e) => assert_eq!((e.played, e.why), (false, Some("user-paused"))),
            _ => panic!("a release answers Released"),
        }
        quit(&tx, worker);
        assert!(
            apply.log().is_empty(),
            "no pause, no play: {:?}",
            apply.log()
        );
    }

    /// A full-screen program in front, as a switch the test flips.
    fn screen(
        full: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> impl FnMut() -> bool + Send + 'static {
        let full = full.clone();
        move || full.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Settles for the full-screen tests: the reload quiet is long enough that a stalled test
    /// thread can't end it early, and short enough to wait for.
    fn gaming() -> Settle {
        Settle {
            layer: Duration::from_millis(150),
            max: Duration::from_secs(5),
            game: Duration::from_millis(1500),
            game_max: Duration::from_secs(10),
            reload: Duration::from_millis(900),
            fresh: RECONNECT_FRESH,
        }
    }

    /// A foreground change that isn't a full-screen program leaving sends nothing; one that is
    /// holds the run for the reload, even against a change that would go at once.
    #[test]
    fn a_full_screen_program_leaving_holds_the_run_for_the_reload() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let full = std::sync::Arc::new(AtomicBool::new(false));
        let mut sc = Screen::new(screen(&full));
        let (tx, rx) = mpsc::channel();
        // Into a game: no sync for that alone.
        full.store(true, SeqCst);
        tx.send(Wake::Foreground).unwrap();
        tx.send(Wake::Changed(Reason::Live)).unwrap();
        assert!(matches!(
            next(&rx, gaming(), &mut sc),
            Next::Sync {
                reason: Reason::Live,
                left_full: false,
                ..
            }
        ));
        assert!(sc.full);
        // The game leaves the front, and a wallpaper change comes during the reload.
        full.store(false, SeqCst);
        let t = Instant::now();
        tx.send(Wake::Foreground).unwrap();
        tx.send(Wake::Changed(Reason::Wallpapers)).unwrap();
        let got = next(&rx, gaming(), &mut sc);
        let took = t.elapsed();
        assert!(
            matches!(
                got,
                Next::Sync {
                    reason: Reason::Reload,
                    left_full: true,
                    ..
                }
            ),
            "the reload's run"
        );
        assert!(
            took >= Duration::from_millis(900),
            "held for the reload: {took:?}"
        );
        // The edge is seen without a foreground event too (a layer wake reads it).
        sc.full = true; // in a game again, as last read
        full.store(false, SeqCst);
        tx.send(Wake::Layer).unwrap();
        assert!(matches!(
            next(&rx, gaming(), &mut sc),
            Next::Sync {
                left_full: true,
                ..
            }
        ));
    }

    /// #586: GAME+ held in a full-screen game for the Win key or Alt+Tab, the game leaving the
    /// front, GAME again: Wallpaper Engine hears nothing (with Pause it kept the wallpaper).
    /// With Stop it reloaded the wallpaper from its saved properties: once the reload's quiet is
    /// over, each monitor gets the state once, and none during it. The health notes both.
    #[test]
    fn leaving_a_full_screen_game_sends_nothing_into_the_reload() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        for stops in [false, true] {
            let state = std::sync::Arc::new(Mutex::new(State {
                layer: Some("GAME".into()),
                game: true,
                ..State::default()
            }));
            let full = std::sync::Arc::new(AtomicBool::new(true));
            let (pushes_tx, pushes) = mpsc::channel();
            let (tx, rx) = mpsc::channel();
            let before = health::now();
            let worker = {
                let state = state.clone();
                let full = screen(&full);
                std::thread::spawn(move || {
                    let read = move || {
                        let s = state.lock().unwrap().clone();
                        (
                            s,
                            vec![target(WE_LEFT, "hud"), target(WE_RIGHT, "stars")],
                            stops,
                        )
                    };
                    run(&rx, Wire(pushes_tx), read, gaming(), full);
                })
            };
            let set = |layer: &str| state.lock().unwrap().layer = Some(layer.into());
            let wait = Duration::from_secs(5);
            tx.send(Wake::Changed(Reason::Start)).unwrap();
            for _ in 0..2 {
                let first = pushes.recv_timeout(wait).unwrap();
                assert!(first.contains(r#""foxglove_layer":"GAME""#), "{first}");
            }

            // In the game: GAME+ for the Win key; the game leaves the front; GAME+ let go.
            set("GAME+");
            tx.send(Wake::Layer).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            full.store(false, SeqCst);
            let left = Instant::now();
            tx.send(Wake::Foreground).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            set("GAME");
            tx.send(Wake::Layer).unwrap();

            if stops {
                for _ in 0..2 {
                    let p = pushes
                        .recv_timeout(wait)
                        .expect("the reloaded wallpaper gets the state");
                    assert!(
                        left.elapsed() >= Duration::from_millis(900),
                        "not inside the reload"
                    );
                    assert!(p.contains(r#""foxglove_layer":"GAME""#), "{p}");
                }
            }
            assert!(
                pushes.recv_timeout(Duration::from_millis(1500)).is_err(),
                "nothing else (stops: {stops})"
            );
            let (ack, acked) = mpsc::channel();
            tx.send(Wake::Quit(ack)).unwrap();
            acked.recv_timeout(wait).expect("quit answers");
            worker.join().unwrap();

            let after = health::now();
            assert!(
                after.held_for_reload > before.held_for_reload,
                "the reload hold is noted"
            );
            if stops {
                assert!(after.resent_after_reload > before.resent_after_reload);
            } else {
                assert!(
                    after.suppressed >= before.suppressed + 2,
                    "GAME again: a duplicate on each monitor"
                );
            }
        }
    }

    /// With Stop, a game that comes back before the reload's quiet is over leaves its wallpaper
    /// stopped again: the resend waits for the next sync without a full-screen program.
    #[test]
    fn the_reload_resend_waits_while_the_game_is_back() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let full = std::sync::Arc::new(AtomicBool::new(true));
        let (pushes_tx, pushes) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let worker = {
            let full = screen(&full);
            std::thread::spawn(move || {
                let read = || {
                    let s = State {
                        layer: Some("GAME".into()),
                        game: true,
                        ..State::default()
                    };
                    (s, vec![target(WE_LEFT, "hud")], true)
                };
                run(&rx, Wire(pushes_tx), read, gaming(), full);
            })
        };
        let wait = Duration::from_secs(5);
        tx.send(Wake::Changed(Reason::Start)).unwrap();
        pushes.recv_timeout(wait).unwrap();
        full.store(false, SeqCst);
        tx.send(Wake::Foreground).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        full.store(true, SeqCst);
        tx.send(Wake::Foreground).unwrap();
        assert!(
            pushes.recv_timeout(Duration::from_millis(2000)).is_err(),
            "the game is back: its wallpaper is stopped"
        );
        full.store(false, SeqCst);
        tx.send(Wake::Foreground).unwrap();
        let p = pushes
            .recv_timeout(wait)
            .expect("sent once the game has gone");
        assert!(p.contains(r#""foxglove_game":true"#), "{p}");
        let (ack, acked) = mpsc::channel();
        tx.send(Wake::Quit(ack)).unwrap();
        acked.recv_timeout(wait).unwrap();
        worker.join().unwrap();
    }

    /// A worker that died is forgotten, so the next wallpapers start another; a newer one is
    /// kept even if a late change still finds the old channel closed.
    #[test]
    fn a_dead_worker_is_forgotten_by_number() {
        let (old, _) = mpsc::channel::<Wake>();
        let (new, _keep) = mpsc::channel::<Wake>();
        with(|s| s.wake = Some((7, old)));
        forget(7);
        assert!(with(|s| s.wake.is_none()));
        with(|s| s.wake = Some((8, new)));
        forget(7);
        assert!(with(|s| s.wake.as_ref().is_some_and(|(n, _)| *n == 8)));
        with(|s| s.wake = None);
    }

    fn version(secs: u64, entries: u64) -> Option<Version> {
        Some((
            std::time::UNIX_EPOCH + Duration::from_secs(1_762_000_000 + secs),
            entries,
        ))
    }

    fn theme_of(props: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(props).unwrap();
        v[THEME].as_str().unwrap_or("missing").to_string()
    }

    /// The bug: Live Board was installed again after Foxglove's only theme push, so Wallpaper
    /// Engine reloaded it from its saved properties (`foxglove_theme` ""), and the same file
    /// with the same properties counted as already sent. A changed folder is sent again, with
    /// the theme; the other monitor isn't.
    #[test]
    fn a_reinstalled_wallpaper_is_sent_the_theme_again() {
        let mut fake = Fake {
            running: true,
            ..Fake::default()
        };
        let state = State {
            theme: Some(AppMode::Dark),
            ..State::default()
        };
        let board = |v| Target {
            version: v,
            ..target(
                WE_RIGHT,
                "C:/WE/projects/myprojects/bandit-live-board/index.html",
            )
        };
        let hud = target(WE_LEFT, "C:/WE/projects/myprojects/bandit-hud/index.html");
        {
            let mut p = Pusher::new(&mut fake);
            assert_eq!(p.sync(&state, &[hud.clone(), board(version(0, 14))]), 2);
            assert_eq!(p.sync(&state, &[hud.clone(), board(version(0, 14))]), 0);
            assert_eq!(
                p.sync(&state, &[hud.clone(), board(version(65, 14))]),
                1,
                "installed again: only its monitor"
            );
            assert_eq!(p.sync(&state, &[hud.clone(), board(version(65, 14))]), 0);
            assert_eq!(
                p.sync(&state, &[hud.clone(), board(version(65, 15))]),
                1,
                "a file added in place"
            );
        }
        assert_eq!(fake.pushes.len(), 4);
        for (at, props) in &fake.pushes[2..] {
            assert_eq!(at, WE_RIGHT);
            assert_eq!(theme_of(props), "dark", "{props}");
        }

        // Seen per file, so a folder that vanished mid-copy and came back still counts.
        let mut seen = BTreeMap::new();
        assert!(
            !reinstalled(&mut seen, &[board(version(0, 14))]),
            "first sight"
        );
        assert!(!reinstalled(&mut seen, &[board(version(0, 14))]));
        assert!(!reinstalled(&mut seen, &[]), "removed by the install");
        assert!(
            reinstalled(&mut seen, &[board(version(65, 14))]),
            "copied anew"
        );
        assert!(!reinstalled(&mut seen, &[board(version(65, 14))]));
    }

    /// A reinstall waits for the copy and Wallpaper Engine's reload: held `reload` from its last
    /// change (a plain wallpaper change meanwhile doesn't release it), and never past its cap.
    #[test]
    fn a_reinstall_is_held_until_the_copy_settles() {
        let settle = Settle {
            reload: Duration::from_millis(400),
            game_max: Duration::from_millis(1500),
            ..slow()
        };
        let (tx, rx) = mpsc::channel();
        tx.send(Wake::Reinstalled).unwrap();
        tx.send(Wake::Changed(Reason::Wallpapers)).unwrap();
        let t = Instant::now();
        assert!(matches!(
            next(&rx, settle, &mut windowed()),
            Next::Sync {
                reason: Reason::Reinstalled,
                ..
            }
        ));
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(400), "{took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");

        // A copy that keeps writing is sent at the cap.
        let (tx, rx) = mpsc::channel();
        let copying = std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(4);
            while Instant::now() < end {
                if tx.send(Wake::Reinstalled).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let t = Instant::now();
        assert!(matches!(
            next(&rx, settle, &mut windowed()),
            Next::Sync { .. }
        ));
        let took = t.elapsed();
        drop(rx);
        copying.join().unwrap();
        assert!(took >= Duration::from_millis(1500), "{took:?}");
        assert!(took < Duration::from_millis(3500), "{took:?}");
    }

    /// A wallpaper connecting to the live feed has just loaded from its saved properties: every
    /// monitor not sent anything in the last `fresh` is sent everything again, the theme too.
    /// Right after a push (Foxglove's start) a connect sends nothing.
    #[test]
    fn a_live_feed_connect_resends_the_theme() {
        let settle = Settle {
            fresh: Duration::from_millis(600),
            ..slow()
        };
        let (pushes_tx, pushes) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let read = || {
                let s = State {
                    live: Some(8768),
                    theme: Some(AppMode::Dark),
                    ..State::default()
                };
                (
                    s,
                    vec![target(WE_LEFT, "hud"), target(WE_RIGHT, "board")],
                    false,
                )
            };
            run(&rx, Wire(pushes_tx), read, settle, || false);
        });
        let wait = Duration::from_secs(5);
        tx.send(Wake::Changed(Reason::Start)).unwrap();
        for _ in 0..2 {
            assert_eq!(theme_of(&pushes.recv_timeout(wait).unwrap()), "dark");
        }
        // The wallpapers connect right after the first push: nothing again.
        tx.send(Wake::Reconnected).unwrap();
        assert!(pushes.recv_timeout(Duration::from_millis(300)).is_err());
        // Later, one reloads and connects: both monitors (which one isn't known).
        std::thread::sleep(Duration::from_millis(700));
        let before = health::now();
        tx.send(Wake::Reconnected).unwrap();
        for _ in 0..2 {
            let p = pushes.recv_timeout(wait).expect("sent again on connect");
            assert_eq!(theme_of(&p), "dark", "{p}");
            assert!(p.contains(r#""foxglove_live":8768"#), "{p}");
        }
        assert!(pushes.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(health::now().pushes >= before.pushes + 2);
        let (ack, acked) = mpsc::channel();
        tx.send(Wake::Quit(ack)).unwrap();
        acked.recv_timeout(wait).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn only_monitors_sent_long_enough_ago_are_forgotten() {
        let mut fake = Fake {
            running: true,
            ..Fake::default()
        };
        let mut p = Pusher::new(&mut fake);
        let state = State::default();
        p.sync(&state, &[target(WE_LEFT, "hud"), target(WE_RIGHT, "board")]);
        let now = Instant::now();
        assert_eq!(p.resend_stale(Duration::from_secs(60), now), 0);
        assert_eq!(p.resend_stale(Duration::ZERO, now), 2);
        assert_eq!(
            p.sync(&state, &[target(WE_LEFT, "hud"), target(WE_RIGHT, "board")]),
            2
        );
    }
}
