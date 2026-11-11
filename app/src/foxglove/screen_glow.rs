//! Screen glow (#496, `docs/screen-ambient-ux.md`): an effect on the Scenes page whose lights take
//! the colours at the edges of the screens. The maths is `foxglove_scene::screen` (edge regions,
//! black bars, the nearest edge on the room map, smoothing and the flash limit); this is the
//! app's half:
//!
//! - **The yes:** reading the whole screen needs the user's click on "Allow and start", kept in
//!   the store's `[screen_glow]` (not profiled) until "Stop allowing". Without it nothing starts
//!   Screen glow: `scene_run::start` and `set_effect` refuse it (a profile, the last look after a
//!   restart, the Lights page, the local API and plugins all go through them), and the Scenes
//!   page says so ([`NEEDS_YES`]).
//! - **The gates** ([`Gates`]): frames are read only while Screen glow runs, is allowed, and
//!   nothing holds it: Pause Foxglove, the display off, the PC locked, a full-screen game, another
//!   app (or Windows) holding the lights, or capture failing. Each comes from an event the scene's
//!   control thread already hears (pause, `away`'s WTS and display-power callbacks, the foreground
//!   program changing, the lights owner's report, `WM_DISPLAYCHANGE`); none is polled. A gate
//!   stops the capture threads (joins them), not only what is sent.
//! - **Capture** ([`Reader`]): one thread per screen while the gates are open, each blocked in
//!   DXGI's `AcquireNextFrame` (`screen_capture`), which returns when the screen changes. After a
//!   frame it waits out the rest of [`MIN_GAP`] (stop-aware), so at most 20 pictures a second are
//!   read, and the scene sends at most 20 updates a second (`Glow::due`). The 1 s wait limit is
//!   only the fallback that lets a stop through on a still screen.
//! - **Privacy:** a picture is downscaled on the GPU, turned into edge colours here and dropped.
//!   Only those colours go anywhere (to the lights); no frame leaves the process and nothing is
//!   stored or logged.
//! - **Always visible:** while frames are read (and only then) the tray icon shows the eye, the
//!   tooltip says "Screen glow on" and the tray panel has its row ([`status`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use foxglove_scene::screen::{Edges, ScreenImage, Smoothing, MIN_UPDATE_MS};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------- the words

/// The effect's name: the Effects list, the scene's name, the tray.
pub const NAME: &str = "Screen glow";
pub const DESCRIPTION: &str =
    "Your lights take the colours at the edges of your screens, like a TV's ambient light.";
pub const ASK_TITLE: &str = "Let Foxglove read your screen?";
pub const ASK_BODY: &str = "To colour your lights, Foxglove looks at everything on your screens, up to 20 times a second, while Screen glow runs.";
pub const ASK_POINTS: [&str; 3] = [
    "The picture never leaves Foxglove and is never saved. Only the edge colours go to the lights.",
    "It pauses while a game is full screen, the PC is locked or the display is off.",
    "While it runs, the tray icon shows an eye. Stop it there or here at any time.",
];
pub const ALLOW: &str = "Allow and start";
pub const NOT_NOW: &str = "Not now";
pub const ASK_FOOTNOTE: &str =
    "Foxglove remembers your answer. You can take it back under this effect.";
pub const SMOOTHING: &str = "Smoothing";
pub const SMOOTHING_HINT: &str =
    "Quick follows every cut. Calm drifts between colours and never flickers.";
pub const ALLOWED_LINE: &str = "Foxglove may read your screen for Screen glow.";
pub const STOP_ALLOWING: &str = "Stop allowing";
pub const NOT_ALLOWED_LINE: &str = "Foxglove asks before reading your screen.";
pub const ROOM_STOPPED: &str =
    "Not running. Screen glow shows your screen's colours here once it runs.";
pub const ROOM_RUNNING: &str = "Drag a device to change which edge it follows.";
pub const RUNNING: &str = "Running: Screen glow · reading your screen";
pub const NEEDS_YES: &str = "Screen glow didn't start: it needs your yes. Pick it and press Start.";
pub const NO_SCREEN: &str = "Foxglove can't see a screen yet.";
pub const TRAY_ROW_LINE: &str = "Reading your screen for the lights";
pub const TRAY_STOP: &str = "Stop";
pub const LIGHTS_SCENE: &str = "Scene: Screen glow";
pub const NOTIFY_TITLE: &str = "Screen glow is on.";
pub const NOTIFY_BODY: &str =
    "Foxglove is reading your screen for the lights. Stop it from the tray.";
/// The tray row as a screen reader reads it.
pub const TRAY_ROW_SPOKEN: &str = "Screen glow, reading your screen for the lights, Stop button";

/// A smoothing pill's label.
pub fn smoothing_label(s: Smoothing) -> &'static str {
    match s {
        Smoothing::Quick => "Quick",
        Smoothing::Even => "Even",
        Smoothing::Calm => "Calm",
    }
}

/// The tooltip's status line while frames are read.
pub fn tooltip_status(connected: bool) -> &'static str {
    if connected {
        "Connected · Screen glow on"
    } else {
        "Disconnected · Screen glow on"
    }
}

// ---------------------------------------------------------------------------- the yes

/// The store section with the user's yes (not profiled: it is about this PC).
pub const SECTION: &str = "screen_glow";

/// `[screen_glow]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stored {
    /// The user clicked "Allow and start" and hasn't clicked "Stop allowing" since.
    pub allowed: bool,
}

/// 0 = not read yet, 1 = no, 2 = yes.
static ALLOWED: AtomicU8 = AtomicU8::new(0);

/// Whether the user said yes (read once from the store, then kept here).
pub fn allowed() -> bool {
    match ALLOWED.load(Ordering::SeqCst) {
        0 => {
            let yes = super::store::try_load::<Stored>(SECTION).is_some_and(|s| s.allowed);
            ALLOWED.store(if yes { 2 } else { 1 }, Ordering::SeqCst);
            yes
        }
        v => v == 2,
    }
}

fn set_allowed(yes: bool) {
    ALLOWED.store(if yes { 2 } else { 1 }, Ordering::SeqCst);
    if let Err(e) = super::store::save(SECTION, &Stored { allowed: yes }) {
        eprintln!("Foxglove: can't save [{SECTION}]: {e}");
    }
    eprintln!(
        "Foxglove: screen glow {}",
        if yes {
            "allowed (Allow and start)"
        } else {
            "no longer allowed (Stop allowing)"
        }
    );
    // A running scene's reader looks again: without the yes it stops reading at once.
    super::scene_run::screen_allowed_changed();
}

/// "Allow and start": the only way the yes is given.
pub fn allow() {
    REFUSED.store(false, Ordering::SeqCst);
    set_allowed(true);
}

/// "Stop allowing": forgets the yes. The caller stops a running Screen glow.
pub fn forget() {
    set_allowed(false);
}

/// A start without the yes was refused (a profile, the last look, the API): the Scenes page says
/// [`NEEDS_YES`] until the next start or the yes.
static REFUSED: AtomicBool = AtomicBool::new(false);

pub fn refused() -> bool {
    REFUSED.load(Ordering::SeqCst)
}

/// Called by `scene_run` when it refuses Screen glow, or starts any scene.
pub fn set_refused(on: bool) {
    REFUSED.store(on, Ordering::SeqCst);
}

/// Whether Windows' "Animation effects" is on: off, a new Screen glow starts on Calm.
#[cfg(all(windows, not(test)))]
pub fn animations_on() -> bool {
    use windows::core::BOOL;
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut on = BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer given.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(std::ptr::from_mut(&mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on.as_bool()
}

#[cfg(any(not(windows), test))]
pub fn animations_on() -> bool {
    true
}

/// The smoothing a look runs with: the user's pick, else Even (Calm with animations off).
pub fn smoothing_for(picked: Option<Smoothing>) -> Smoothing {
    picked.unwrap_or_else(|| Smoothing::default_for(animations_on()))
}

// ---------------------------------------------------------------------------- the gates

/// What holds a running Screen glow: no frames are read meanwhile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    Paused,
    DisplayOff,
    Locked,
    Game,
    /// Another app, or Windows Dynamic Lighting, has the lights (#491).
    Yielded,
    /// Capture failed (remote desktop, a GPU or driver change); tried again when the display
    /// changes.
    Failed,
}

impl Hold {
    /// The Scenes page's status line and the tray panel row's words. `who` names whoever has
    /// the lights ([`Hold::Yielded`]).
    pub fn words(self, who: Option<&str>) -> String {
        match self {
            Hold::Game => "Paused while a game is full screen".into(),
            Hold::Locked => "Paused while the PC is locked".into(),
            Hold::DisplayOff => "Paused while the display is off".into(),
            Hold::Paused => "Paused: Foxglove is paused. Resume it from the tray.".into(),
            Hold::Yielded => format!("Paused for {}", who.unwrap_or("another app")),
            Hold::Failed => {
                "Can't read the screen right now. Foxglove tries again when the display changes."
                    .into()
            }
        }
    }

    /// For Copy diagnostics.
    pub fn short(self) -> &'static str {
        match self {
            Hold::Paused => "paused",
            Hold::DisplayOff => "display off",
            Hold::Locked => "locked",
            Hold::Game => "full-screen game",
            Hold::Yielded => "lights held by another app",
            Hold::Failed => "capture failed",
        }
    }
}

/// Everything that decides whether frames are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    /// A scene runs, and its look is Screen glow.
    pub on: bool,
    pub allowed: bool,
    pub paused: bool,
    pub display_off: bool,
    pub locked: bool,
    pub game: bool,
    pub yielded: bool,
    pub failed: bool,
}

impl Gates {
    /// What holds it, the first in the order the user would look for it.
    pub fn hold(&self) -> Option<Hold> {
        [
            (self.paused, Hold::Paused),
            (self.display_off, Hold::DisplayOff),
            (self.locked, Hold::Locked),
            (self.game, Hold::Game),
            (self.yielded, Hold::Yielded),
            (self.failed, Hold::Failed),
        ]
        .into_iter()
        .find_map(|(on, h)| on.then_some(h))
    }

    /// Frames are read.
    pub fn reads(&self) -> bool {
        self.on && self.allowed && self.hold().is_none()
    }

    /// The scene itself stops drawing (the scheduler's Held gate): a game full screen or capture
    /// failing. The lights get their own colours back meanwhile. The other holds are the
    /// scene's own gates already (pause, away, the lights owner).
    pub fn scene_held(&self) -> bool {
        self.on && (self.game || self.failed)
    }
}

// ---------------------------------------------------------------------------- capture

/// One screen's frames.
pub trait Capture: Send {
    /// The next picture of the screen, waiting at most `wait` for it to change: `Ok(None)` when
    /// it didn't. An error ends this capture (the gate's "capture failed").
    fn next(&mut self, wait: Duration) -> Result<Option<ScreenImage>, String>;
}

/// Opens one [`Capture`] per screen.
pub type Opener = Arc<dyn Fn() -> Result<Vec<Box<dyn Capture>>, String> + Send + Sync>;

/// The real screens (DXGI Desktop Duplication). Tests never call it: under `cfg(test)` it fails.
pub fn opener() -> Opener {
    Arc::new(super::screen_capture::open)
}

/// The shortest gap between two pictures of one screen.
pub const MIN_GAP: Duration = Duration::from_millis(MIN_UPDATE_MS as u64);
/// How long one wait for a changed screen lasts before a stop is looked at: the fallback only.
pub const WAIT: Duration = Duration::from_secs(1);

/// The newest edge colours of each screen, shared with the scene's outputs. Only colours: the
/// pictures they came from are gone.
#[derive(Debug, Default)]
pub struct Feed {
    edges: Mutex<BTreeMap<String, Edges>>,
    pictures: AtomicU64,
}

impl Feed {
    pub fn put(&self, e: Edges) {
        self.pictures.fetch_add(1, Ordering::Relaxed);
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(e.monitor.clone(), e);
    }

    /// Every screen's newest edges.
    pub fn latest(&self) -> Vec<Edges> {
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Pictures read so far.
    pub fn pictures(&self) -> u64 {
        self.pictures.load(Ordering::Relaxed)
    }

    fn clear(&self) {
        self.edges.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Set once to stop every capture thread; their waits wake at once.
#[derive(Default)]
struct Stop {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Stop {
    fn set(&self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.notify_all();
    }

    fn is_set(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits `d` unless stopped first; true when stopped.
    fn wait(&self, d: Duration) -> bool {
        let g = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self
            .wake
            .wait_timeout_while(g, d, |s| !*s)
            .unwrap_or_else(|e| e.into_inner());
        *g
    }
}

struct Run {
    stop: Arc<Stop>,
    threads: Vec<JoinHandle<()>>,
}

/// Runs the capture threads while the gates are open. Owned by a running scene's control
/// thread; each gate change is one [`Reader::set`].
pub struct Reader {
    gates: Gates,
    run: Option<Run>,
    open: Opener,
    feed: Arc<Feed>,
    /// Told (from a capture thread) when capture failed; the control thread then sets
    /// `failed`.
    on_fail: Arc<dyn Fn() + Send + Sync>,
    /// The app's reader publishes its state for the tray and the page ([`status`]); a test's
    /// doesn't.
    publish: bool,
}

impl Reader {
    pub fn new(
        open: Opener,
        feed: Arc<Feed>,
        on_fail: Arc<dyn Fn() + Send + Sync>,
        publish: bool,
    ) -> Reader {
        if publish {
            *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&feed);
        }
        Reader {
            gates: Gates::default(),
            run: None,
            open,
            feed,
            on_fail,
            publish,
        }
    }

    pub fn gates(&self) -> Gates {
        self.gates
    }

    /// Whether the capture threads exist now.
    #[cfg(test)]
    pub fn reading(&self) -> bool {
        self.run.is_some()
    }

    /// Changes the gates; starts or stops capture to match (a stop joins the threads first).
    pub fn set(&mut self, change: impl FnOnce(&mut Gates)) {
        let before = self.gates;
        change(&mut self.gates);
        if !self.gates.on {
            // Not Screen glow (any more): nothing to hold, nothing failed.
            self.gates.failed = false;
        }
        match (self.gates.reads(), self.run.is_some()) {
            (true, false) => self.start(),
            (false, true) => self.stop(),
            _ => {}
        }
        if before.hold() != self.gates.hold() {
            if let Some(h) = self.gates.hold().filter(|_| self.gates.on) {
                eprintln!("Foxglove: screen glow held: {}", h.short());
            } else if before.on && self.gates.on {
                eprintln!("Foxglove: screen glow reads again");
            }
        }
        self.publish();
    }

    fn start(&mut self) {
        let stop = Arc::new(Stop::default());
        let wait = stop.clone();
        let open = self.open.clone();
        let feed = self.feed.clone();
        let on_fail = self.on_fail.clone();
        // Opening (making the GPU devices) happens off the control thread too.
        let first = std::thread::Builder::new()
            .name("screen-glow".into())
            .spawn(move || {
                let caps = match open() {
                    Ok(c) if !c.is_empty() => c,
                    Ok(_) => {
                        eprintln!("Foxglove: screen glow: no screen to read");
                        on_fail();
                        return;
                    }
                    Err(e) => {
                        eprintln!("Foxglove: screen glow can't read the screen: {e}");
                        on_fail();
                        return;
                    }
                };
                let failed = Arc::new(AtomicBool::new(false));
                let handles: Vec<JoinHandle<()>> = caps
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, cap)| {
                        let (wait, feed, on_fail, failed) =
                            (wait.clone(), feed.clone(), on_fail.clone(), failed.clone());
                        std::thread::Builder::new()
                            .name(format!("screen-glow-{i}"))
                            .spawn(move || capture_loop(cap, &wait, &feed, &*on_fail, &failed))
                            .ok()
                    })
                    .collect();
                for h in handles {
                    let _ = h.join();
                }
            });
        match first {
            Ok(t) => {
                self.run = Some(Run {
                    stop,
                    threads: vec![t],
                })
            }
            Err(e) => eprintln!("Foxglove: no thread for screen glow: {e}"),
        }
    }

    fn stop(&mut self) {
        if let Some(run) = self.run.take() {
            run.stop.set();
            for t in run.threads {
                let _ = t.join();
            }
        }
        // Nothing read is kept once reading stops.
        self.feed.clear();
    }

    fn publish(&self) {
        if !self.publish {
            return;
        }
        let mut s = STATUS.lock().unwrap_or_else(|e| e.into_inner());
        let on = self.gates.on;
        if on && s.since.is_none() {
            s.since = Some(Instant::now());
            s.pictures_at_start = self.feed.pictures();
        }
        if !on {
            s.since = None;
        }
        s.on = on;
        s.reading = self.run.is_some();
        s.hold = self.gates.hold().filter(|_| on);
        drop(s);
        super::tray_panel::wake();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop();
        self.gates.on = false;
        self.publish();
    }
}

/// One screen's capture: a picture when it changes, at most one per [`MIN_GAP`], until a stop or
/// an error.
fn capture_loop(
    mut cap: Box<dyn Capture>,
    stop: &Stop,
    feed: &Feed,
    on_fail: &(dyn Fn() + Send + Sync),
    failed: &AtomicBool,
) {
    loop {
        if stop.is_set() {
            return;
        }
        match cap.next(WAIT) {
            Ok(Some(img)) => {
                feed.put(Edges::of(&img));
                drop(img);
                // The rest of the gap, woken at once by a stop. Pictures that change meanwhile
                // are merged by Windows into the next one.
                if stop.wait(MIN_GAP) {
                    return;
                }
            }
            Ok(None) => {}
            Err(e) => {
                // One report for all the screens' threads.
                if !failed.swap(true, Ordering::SeqCst) {
                    eprintln!("Foxglove: screen glow: capture stopped: {e}");
                    on_fail();
                }
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------- what shows

/// What the tray, the tray panel and the Scenes page show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Screen glow runs (reading or held).
    pub on: bool,
    /// Frames are read now: the eye shows.
    pub reading: bool,
    pub hold: Option<Hold>,
    since: Option<Instant>,
    pictures_at_start: u64,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    on: false,
    reading: false,
    hold: None,
    since: None,
    pictures_at_start: 0,
});

/// The running scene's feed, for the Scenes page's live edge strips (gone with the scene).
static LIVE: Mutex<std::sync::Weak<Feed>> = Mutex::new(std::sync::Weak::new());

/// The screens' newest edge colours while Screen glow reads; none otherwise.
pub fn live_edges() -> Vec<Edges> {
    if !status().reading {
        return Vec::new();
    }
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .upgrade()
        .map(|f| f.latest())
        .unwrap_or_default()
}

/// Updates the scene sent so far (for Copy diagnostics).
static UPDATES: AtomicU64 = AtomicU64::new(0);

/// The scene sent screen colours to the lights once.
pub fn sent() {
    UPDATES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
thread_local! {
    /// The previews' and tests' status, so they never depend on a running scene.
    pub static PREVIEW: std::cell::Cell<Option<Status>> = const { std::cell::Cell::new(None) };
}

/// The state now.
pub fn status() -> Status {
    #[cfg(test)]
    if let Some(s) = PREVIEW.get() {
        return s;
    }
    *STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A status for previews and tests.
#[cfg(test)]
pub fn preview_status(reading: bool, hold: Option<Hold>) -> Status {
    Status {
        on: true,
        reading,
        hold,
        since: None,
        pictures_at_start: 0,
    }
}

/// The diagnostics line: "Screen glow: allowed/not allowed · running/stopped/held ({gate}) ·
/// {n} updates/s".
pub fn diagnostics() -> String {
    let s = status();
    let state = match (s.on, s.hold) {
        (false, _) => "stopped".to_string(),
        (true, Some(h)) => format!("held ({})", h.short()),
        (true, None) => "running".to_string(),
    };
    let rate = s
        .since
        .map(|t| t.elapsed().as_secs_f64())
        .filter(|secs| *secs >= 1.0)
        .map_or(0.0, |secs| UPDATES.load(Ordering::Relaxed) as f64 / secs);
    diagnostics_line(allowed(), &state, rate)
}

pub fn diagnostics_line(allowed: bool, state: &str, per_s: f64) -> String {
    format!(
        "Screen glow: {} · {state} · {per_s:.0} updates/s",
        if allowed { "allowed" } else { "not allowed" }
    )
}

/// A scene started: the update count starts again.
pub fn scene_started() {
    UPDATES.store(0, Ordering::Relaxed);
}

/// Screen glow started without a click while Foxglove's window is closed (a profile switch, the
/// last look after a restart): one Windows notification says so.
pub fn notify_started_unseen() {
    super::notice::toast(&super::notice::Note {
        title: NOTIFY_TITLE.into(),
        body: NOTIFY_BODY.into(),
        level: super::live_wire::Level::Info,
    });
}

/// Whether a full-screen game is in front: Direct3D exclusive full screen or presentation mode.
/// `QUNS_BUSY` is left out on purpose: Windows reports it for any full-screen window, a browser's
/// full-screen video too, and video keeps glowing (the design's pick). Borderless-window games
/// aren't seen (the guide says so).
#[cfg(all(windows, not(test)))]
pub fn game_now() -> bool {
    use windows::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    // SAFETY: no arguments; it returns the state by value.
    unsafe { SHQueryUserNotificationState() }
        .is_ok_and(|s| matches!(s, QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE))
}

#[cfg(test)]
pub static FAKE_GAME: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub fn game_now() -> bool {
    FAKE_GAME.load(Ordering::SeqCst)
}

#[cfg(all(not(windows), not(test)))]
pub fn game_now() -> bool {
    false
}

#[cfg(test)]
mod tests;
