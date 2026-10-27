//! "Find my LEDs" (#90 row G): where each LED of the keyboard sits, found once by the user.
//!
//! The wizard lights one LED at a time (bright white, the others off) with SET_FRAME, and the
//! user clicks where it shows on a drawing of the board ([`led_map_ui`](super::led_map_ui)). The
//! points go in the store's `[led_map]` section, the board profile's user override: each LED
//! the user saw, a point on its half's drawing in key units. A saved map marks the profile's
//! `led_map` checked, and the scenes' room map places each LED there (`scene_room`).
//!
//! The firmware's SET_EFFECT draws its LEDs from a table built into it (lamp_effect.c), with no
//! way for the PC to send positions yet (#550). So once a map is
//! saved, scenes drive the keyboard with frames, which take the room's positions, instead of
//! effects ([`effects_place_leds`]).
//!
//! While it runs the wizard holds the keyboard's lights under a high-priority lighting lease
//! ([`LEDMAP_CONN`]); a running scene's frames or effect are handed back under it, and when the
//! wizard ends (finish, cancel, page left, window closed, quit, pause or away) its frame goes
//! back and the lease ends, so the scene draws again or the lights from before come back.
//!
//! Nothing here polls. The worker thread sleeps on its channel: a step of the wizard, the
//! keyboard's own hand-back of the frame (D7 8C event: its 30 s timeout), a keyboard arriving,
//! or the end.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use digitalis::lamps::LampParts;
use foxglove_devices::{DeviceError, Lighting, Rgb};
use serde::{Deserialize, Serialize};

use super::boards::{Board, Drawing, LED_MAP_CHECKED, LED_MAP_SHIPPED};
use super::lighting_leases::{Arbiter, Lease, Priority};

/// The store section with every board's map.
pub const SECTION: &str = "led_map";

/// The lighting-lease connection the wizard holds the keyboard under (no plugin has it).
pub const LEDMAP_CONN: u64 = u64::MAX - 2;

/// The lit LED's colour: bright white.
pub const LIT: [u8; 3] = [255, 255, 255];

/// The store's `[led_map]`: each board's map, by profile id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub boards: BTreeMap<String, BoardLeds>,
}

/// One board's LEDs as the user found them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BoardLeds {
    /// The frame's lamps when they were found: a profile with another count ignores the map.
    pub lamps: u16,
    pub halves: u8,
    /// Each part's LEDs when they were found (#561): a profile that splits them otherwise
    /// ignores the map. Empty in maps saved before, which shared the lamps out evenly.
    #[serde(default)]
    pub parts: Vec<u16>,
    /// Each LED the user saw; one they couldn't see is left out.
    #[serde(default)]
    pub leds: Vec<Seen>,
}

/// Where one LED was seen: its lamp (SET_FRAME order) and a point on its half's drawing, in
/// key units from the half's top-left corner (the whole board's for a one-piece board).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub lamp: u16,
    pub x: f32,
    pub y: f32,
}

impl BoardLeds {
    /// One entry per lamp: where it was seen, or `None`.
    pub fn points(&self) -> Vec<Option<[f32; 2]>> {
        let mut out = vec![None; usize::from(self.lamps)];
        for s in &self.leds {
            if let Some(slot) = out.get_mut(usize::from(s.lamp)) {
                if s.x.is_finite() && s.y.is_finite() {
                    *slot = Some([s.x, s.y]);
                }
            }
        }
        out
    }
}

fn load() -> Saved {
    super::store::try_load(SECTION).unwrap_or_default()
}

/// The saved map for a board, if any.
pub fn saved(board: &str) -> Option<BoardLeds> {
    load().boards.remove(board)
}

/// Puts a board's saved map into its profile: the points, and `led_map` checked. A map found
/// with another lamp count (the profile changed since) is left out.
pub fn apply(board: &mut Board) {
    if let Some(leds) = saved(&board.id) {
        apply_leds(board, &leds);
    }
}

fn apply_leds(board: &mut Board, leds: &BoardLeds) {
    let lighting = board
        .lighting
        .get_or_insert_with(|| super::boards::Lighting {
            lamps: leds.lamps,
            ..Default::default()
        });
    let split = |parts: &LampParts| parts.counts().map(|n| n as u16).collect::<Vec<u16>>();
    let parts_now = lighting.lamp_parts(board.halves).map(|p| split(&p));
    let other_split = !leds.parts.is_empty() && parts_now.as_ref() != Some(&leds.parts);
    if lighting.lamps != leds.lamps || board.halves != leds.halves || other_split {
        eprintln!(
            "Foxglove: the LED map for {} was found with {} LEDs ({:?}) on {} part(s); the profile now says {} ({:?}) on {}: find them again",
            board.id, leds.lamps, leds.parts, leds.halves, lighting.lamps, parts_now, board.halves
        );
        return;
    }
    lighting.led_points = leds.points();
    lighting.led_map = LED_MAP_CHECKED.into();
}

/// Saves a board's map and puts it into the connected keyboard's profile at once; the scenes'
/// room is built again.
pub fn save(board: &str, leds: BoardLeds) -> std::io::Result<()> {
    let mut all = load();
    all.boards.insert(board.to_string(), leds);
    super::store::save(SECTION, &all)?;
    super::boards::led_map_changed();
    super::scene_run::devices_changed();
    Ok(())
}

/// The connected keyboard's profile has a map from "Find my LEDs".
static PLACED: AtomicBool = AtomicBool::new(false);

/// The connected keyboard's profile changed (connected, or a map saved).
pub fn note_current(board: Option<&Board>) {
    PLACED.store(placed(board), Ordering::Relaxed);
}

/// Whether a profile places its LEDs itself: a map from "Find my LEDs" or the dot test, or a
/// key map scenes use (shipped or checked). Then the scene's positions are the map's, which
/// firmware without EFFECT LAMPS can't take, so scenes send frames there.
fn placed(board: Option<&Board>) -> bool {
    board
        .and_then(|b| b.lighting.as_ref())
        .is_some_and(|l| !l.led_points.is_empty() || (l.led_map_usable() && !l.led_keys.is_empty()))
}

/// Whether the firmware's own effects (SET_EFFECT) put the LEDs where they are: always with
/// firmware that takes the PC's points (`takes_points`, EFFECT LAMPS, HELLO bit 21, #550), since
/// the scene sends them. Without it, not once the profile places them (found, confirmed, or a
/// shipped map): that firmware places them by its built-in table, so scenes send frames, which
/// do. Never looks anything up (frame threads
/// ask).
pub fn effects_place_leds(takes_points: bool) -> bool {
    takes_points || !PLACED.load(Ordering::Relaxed)
}

/// Whether the connected keyboard's LEDs are placed (checked in its profile or found here), so
/// scenes light each one. Never looks for a keyboard (the UI thread asks).
pub fn checked(board: Option<&Board>) -> bool {
    board
        .and_then(|b| b.lighting.as_ref())
        .is_some_and(|l| l.led_map == LED_MAP_CHECKED)
}

/// How far the connected keyboard's LED map is known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapState {
    /// No map scenes use: each half is one light.
    #[default]
    Unknown,
    /// The profile ships a map no one has checked on a board: scenes use it, the dot test asks.
    Shipped,
    /// Checked on the board (or found with "Find my LEDs").
    Checked,
}

/// The connected keyboard's map state. Never looks for a keyboard (the UI thread asks).
pub fn state(board: Option<&Board>) -> MapState {
    match board.and_then(|b| b.lighting.as_ref()) {
        Some(l) if l.led_map == LED_MAP_CHECKED => MapState::Checked,
        Some(l) if l.led_map == LED_MAP_SHIPPED && !l.led_keys.is_empty() => MapState::Shipped,
        _ => MapState::Unknown,
    }
}

/// The wizard's state: which LED is lit, and what the user said about the ones before it.
#[derive(Clone, Debug, PartialEq)]
pub struct Wizard {
    /// The board profile's id, the map's key in the store.
    pub board: String,
    pub name: String,
    pub drawing: Drawing,
    /// 1 for a one-piece board, else 2.
    pub halves: u8,
    /// The board's LEDs by part, in frame order (#561).
    pub parts: LampParts,
    /// One answer per LED so far, in order: where it was seen, or `None` ("Can't see it").
    pub answers: Vec<Option<[f32; 2]>>,
    /// A problem to show (a failed save).
    pub note: Option<String>,
    /// Why it opened, when it's the dot test's fallback (none of the corrections fit).
    pub intro: Option<String>,
}

/// The LED the wizard lights now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Its lamp in the frame.
    pub lamp: usize,
    /// 0 left (or the whole one-piece board), 1 right.
    pub half: usize,
    /// Its place on its half's strip, from 0.
    pub index: usize,
}

impl Wizard {
    /// Starts on a board profile: every LED of the frame (as many as the board has, #561), the
    /// left half's first.
    pub fn new(board: &Board) -> Wizard {
        let halves = if board.halves == 1 { 1 } else { 2 };
        let mut profile = board.clone();
        profile.halves = halves;
        let parts = super::devices::lamp_parts(Some(&profile));
        Wizard {
            board: board.id.clone(),
            name: board.name.clone(),
            drawing: super::boards::drawing(Some(board)),
            halves,
            parts,
            answers: Vec::new(),
            note: None,
            intro: None,
        }
    }

    pub fn total(&self) -> usize {
        self.parts.total()
    }

    /// The part a frame lamp is on (0 for one past the last).
    pub fn part_of(&self, lamp: usize) -> usize {
        self.parts.part_of(lamp).map_or(0, |(p, _)| p)
    }

    /// The LED lit now; `None` once every one is answered.
    pub fn step(&self) -> Option<Step> {
        let lamp = self.answers.len();
        let (half, index) = self.parts.part_of(lamp)?;
        Some(Step { lamp, half, index })
    }

    /// The user clicked where the lit LED is (a point on its half, key units).
    pub fn place(&mut self, at: [f32; 2]) {
        if self.step().is_some() && at.iter().all(|v| v.is_finite()) {
            self.answers.push(Some(at));
        }
    }

    /// "Can't see it".
    pub fn skip(&mut self) {
        if self.step().is_some() {
            self.answers.push(None);
        }
    }

    /// Back: the LED before is lit again and its answer goes.
    pub fn back(&mut self) {
        self.answers.pop();
    }

    pub fn done(&self) -> bool {
        self.step().is_none()
    }

    /// How many LEDs were seen so far.
    pub fn seen(&self) -> usize {
        self.answers.iter().filter(|a| a.is_some()).count()
    }

    /// The frame for now: the lit LED white, every other one off; all off once done.
    pub fn frame(&self) -> Vec<Rgb> {
        let mut frame = vec![Rgb { r: 0, g: 0, b: 0 }; self.total()];
        if let Some(s) = self.step() {
            let [r, g, b] = LIT;
            frame[s.lamp] = Rgb { r, g, b };
        }
        frame
    }

    /// The map to save: every LED seen so far.
    pub fn result(&self) -> BoardLeds {
        BoardLeds {
            lamps: u16::try_from(self.total()).unwrap_or(u16::MAX),
            halves: self.halves,
            parts: self.parts.counts().map(|n| n as u16).collect(),
            leds: self
                .answers
                .iter()
                .enumerate()
                .filter_map(|(i, a)| {
                    let [x, y] = (*a)?;
                    Some(Seen {
                        lamp: u16::try_from(i).ok()?,
                        x: (x * 1000.0).round() / 1000.0,
                        y: (y * 1000.0).round() / 1000.0,
                    })
                })
                .collect(),
        }
    }
}

/// Takes the keyboard's lights for the wizard: a high-priority lease over whatever holds them,
/// so a scene's frames or effect are handed back first and nothing else draws while it runs.
/// Returns its id.
///
/// The lease's colour is the keyboard's own lights from before (the user's, not a scene's):
/// taking them sends the keyboard nothing new when they were on (no colour change racing the
/// first frames to the right half), a hand-back mid-way (the dot stopped, the keyboard's
/// timeout) shows the user's lights rather than a dim board (dark only if they were off), and
/// ending it puts back no more than the speed.
///
/// The wizard's frames stay its own (it doesn't register them with `draws_frames`, which keeps
/// one drawer a device: the scene's, so the scene is asked to draw again after). So a plugin
/// notification that starts mid-wizard shows under the lit LED until the wizard ends; the
/// wizard is short and the user is watching it.
pub fn take(a: &mut Arbiter, device: &str, light: &dyn Lighting) -> Result<u64, DeviceError> {
    let before = a.lights_before(device, light)?;
    a.add(
        Lease {
            id: 0,
            conn: LEDMAP_CONN,
            device: device.to_string(),
            priority: Priority::High,
            hue: before.hue,
            sat: before.sat,
            // A lease's lights are on: lights that were off stay dark at no brightness.
            brightness: if before.on { before.brightness } else { 0 },
            effect: before.effect.id,
        },
        light,
    )
}

/// Gives the lights back: the wizard's frame first (the arbiter doesn't know of it), then the
/// lease ends and what is under it shows again: a scene is asked for its frame or effect, or
/// the lights from before come back.
pub fn give_back(a: &mut Arbiter, id: u64, light: &dyn Lighting) {
    if let Err(e) = light.release() {
        eprintln!("Foxglove: handing the LED finder's frame back: {e}");
    }
    for device in a.end(|l| l.id == id && l.conn == LEDMAP_CONN) {
        if let Err(e) = a.apply(&device, light) {
            eprintln!("Foxglove: giving back {device}'s lights after the LED finder: {e}");
        }
    }
}

/// What the wizard's worker hears.
#[derive(Debug, PartialEq)]
enum Job {
    /// Show this frame (a step of the wizard).
    Show(Vec<Rgb>),
    /// Run these frames one after another, `every` apart, `rounds` times, then hand the
    /// frame back so the keyboard's own lights show (the dot test).
    Play(Play),
    /// Send the frame again (the keyboard handed it back, or came back).
    Again,
    End,
}

/// A dot run: the frames in order and their pace.
#[derive(Clone, Debug, PartialEq)]
pub struct Play {
    pub frames: Vec<Vec<Rgb>>,
    pub every: std::time::Duration,
    pub rounds: u32,
}

/// How the worker is getting on, for the page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The lights are held and the last frame went out.
    pub shown: Option<Vec<Rgb>>,
    /// Something went wrong (no keyboard, refused).
    pub error: Option<String>,
    /// The dot test: the step lit now; `None` before it starts and once its rounds are done.
    pub step: Option<usize>,
    /// The dot test ran all its rounds and stopped; its frame went back to the keyboard, so
    /// the keyboard's own lights show until it runs again.
    pub played: bool,
    /// A part of the board that doesn't draw the frames (a split's other half with no link):
    /// what to tell the user.
    pub warn: Option<String>,
}

/// What the keyboard said about a frame it was shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// How long it holds a frame with no other before it hands its lights back (its timeout);
    /// `None` when it never does or doesn't say.
    pub hold: Option<Duration>,
    /// A part that doesn't draw the frame: the words for the page.
    pub missing: Option<String>,
}

/// A held frame goes again this long before the keyboard's own hold would run out.
const KEEP_MARGIN: Duration = Duration::from_secs(5);

/// When a frame shown now has to go again so the keyboard keeps it: a little before its hold
/// runs out, and never sooner than half of it.
fn keep_alive(hold: Duration) -> Duration {
    hold.saturating_sub(KEEP_MARGIN).max(hold / 2)
}

/// Where the wizard's frames go: the app's keyboard, or a fake in tests.
pub trait Target: Send {
    /// Takes the lights; an error ends the worker. `again` asks for the frame again (the
    /// keyboard handed it back); it may be called on any thread and only flags work.
    fn take(&mut self, again: Again) -> Result<(), String>;
    /// Shows a frame and waits for the keyboard's answer (so one frame is in flight at a time).
    fn show(&mut self, frame: &[Rgb]) -> Result<Shown, String>;
    /// Hands the frame back but keeps the lights (the dot stopped): the keyboard's own lights
    /// show, and nothing else draws until the next frame or the end.
    fn rest(&mut self);
    fn give_back(&mut self);
}

/// Asks the worker to send its frame again.
pub type Again = Arc<dyn Fn() + Send + Sync>;

/// The dot test's clock, the worker's only timer: the wall clock in the app, a hand-wound one in
/// tests (so they never sleep or race the machine's load).
trait DotClock: Send {
    fn now(&self) -> Instant;
    /// The next job, or `Timeout` once `until` has come.
    fn recv_until(&self, rx: &mpsc::Receiver<Job>, until: Instant)
        -> Result<Job, RecvTimeoutError>;
}

/// Real time: the worker sleeps on its channel until the next step is due.
struct WallClock;

impl DotClock for WallClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn recv_until(
        &self,
        rx: &mpsc::Receiver<Job>,
        until: Instant,
    ) -> Result<Job, RecvTimeoutError> {
        rx.recv_timeout(until.saturating_duration_since(Instant::now()))
    }
}

/// The worker that holds the lights while the wizard runs.
pub struct Worker {
    tx: mpsc::Sender<Job>,
    thread: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<Status>>,
}

/// The last worker's thread, joined by the next one before it takes the lights, so an old
/// give-back can't hand the new wizard's frame back.
static LAST: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// The worker running now, for the keyboard's arrival.
static CURRENT: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);

impl Worker {
    /// Starts the worker; `wake` repaints the page when its status changes.
    pub fn spawn(target: Box<dyn Target>, wake: impl Fn() + Send + 'static) -> Worker {
        Worker::spawn_on(target, Box::new(WallClock), wake)
    }

    /// [`Worker::spawn`] on its own clock.
    fn spawn_on(
        target: Box<dyn Target>,
        clock: Box<dyn DotClock>,
        wake: impl Fn() + Send + 'static,
    ) -> Worker {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let st = status.clone();
        let own = Mutex::new(tx.clone());
        let again: Again = Arc::new(move || {
            let _ = own
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send(Job::Again);
        });
        let before = LAST.lock().unwrap_or_else(|e| e.into_inner()).take();
        let thread = super::services::spawn_named("led-map", move || {
            if let Some(t) = before {
                let _ = t.join();
            }
            run(target, &*clock, &rx, again, &st, &wake);
        });
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
        Worker {
            tx,
            thread: Some(thread),
            status,
        }
    }

    pub fn show(&self, frame: Vec<Rgb>) {
        let _ = self.tx.send(Job::Show(frame));
    }

    /// Runs the dot test's frames (replacing a run or a frame before).
    pub fn play(&self, play: Play) {
        let _ = self.tx.send(Job::Play(play));
    }

    /// Ends this worker and waits for its give-back (tests: no shared [`LAST`]).
    #[cfg(test)]
    fn end_and_wait(mut self) {
        let _ = self.tx.send(Job::End);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Worker {
    /// The wizard ended (finished, cancelled, or dropped): the worker gives the lights back on
    /// its own thread, and the next worker (or [`wait_for_last`]) waits for it.
    fn drop(&mut self) {
        let _ = self.tx.send(Job::End);
        if let Some(t) = self.thread.take() {
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
        }
    }
}

/// Waits until the last worker has given the lights back (quitting). Bounded by the worker's
/// own sends and the lease's readback.
pub fn wait_for_last() {
    let last = LAST.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(t) = last {
        let _ = t.join();
    }
}

/// A keyboard connected: the wizard's LED is lit again on it.
pub fn keyboard_connected() {
    if let Some(tx) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(Job::Again);
    }
}

/// The newest of the jobs waiting: a newer frame or run replaces an older one, an End wins.
fn newest(job: Job, next: Job) -> Job {
    match (job, next) {
        (Job::End, _) | (_, Job::End) => Job::End,
        (_, Job::Show(f)) => Job::Show(f),
        (_, Job::Play(p)) => Job::Play(p),
        (Job::Show(f), Job::Again) => Job::Show(f),
        (Job::Play(p), Job::Again) => Job::Play(p),
        (Job::Again, Job::Again) => Job::Again,
    }
}

/// A dot run under way: which step, and when the next one is due.
struct Running {
    play: Play,
    at: usize,
    next: Instant,
}

impl Running {
    /// The frame for the next step; `None` once every round is done. Its time on the keyboard
    /// starts when the keyboard has it ([`run`] sets `next`), so a slow send never shortens a
    /// step and the dot never rushes to catch up.
    fn advance(&mut self) -> Option<Vec<Rgb>> {
        let n = self.play.frames.len().max(1);
        self.at += 1;
        if self.at >= n * self.play.rounds.max(1) as usize {
            return None;
        }
        self.play.frames.get(self.at % n).cloned()
    }

    fn step(&self) -> usize {
        self.at % self.play.frames.len().max(1)
    }
}

/// The worker's loop. Its only timers are the dot's next step and the held frame's keep-alive,
/// both started by a frame going out and stopped by the end, a new job, or (the dot) its rounds;
/// otherwise it sleeps on its channel.
fn run(
    mut target: Box<dyn Target>,
    clock: &dyn DotClock,
    rx: &mpsc::Receiver<Job>,
    again: Again,
    status: &Mutex<Status>,
    wake: &dyn Fn(),
) {
    let set = |f: &dyn Fn(&mut Status)| {
        f(&mut status.lock().unwrap_or_else(|e| e.into_inner()));
        wake();
    };
    // The frame the keyboard should show; `None` before the first and while resting.
    let mut frame: Option<Vec<Rgb>> = None;
    let mut taken = false;
    // The dot test's clock: the worker sleeps until its next step is due (or a job comes); it
    // stops after its rounds, so nothing runs on once the test is left alone.
    let mut running: Option<Running> = None;
    // When the held frame goes again so the keyboard doesn't time out and hand the lights back
    // mid-way (its hold, from its answer); only while a frame is held.
    let mut keep: Option<Instant> = None;
    loop {
        let due = match (&running, keep) {
            (Some(r), Some(k)) => Some(r.next.min(k)),
            (Some(r), None) => Some(r.next),
            (None, k) => k,
        };
        let first = match due {
            Some(until) => match clock.recv_until(rx, until) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(job) => Some(job),
                Err(_) => break,
            },
        };
        // The dot's step the page shows once the keyboard has this frame (`Some(None)`: none).
        let mut step: Option<Option<usize>> = None;
        match first {
            // A deadline came: the dot's next step, else the held frame's keep-alive (sent
            // again below as it is).
            None => {
                let now = clock.now();
                if let Some(r) = running.as_mut().filter(|r| r.next <= now) {
                    match r.advance() {
                        Some(f) => {
                            step = Some(Some(r.step()));
                            frame = Some(f);
                        }
                        None => {
                            // Done: the keyboard's own lights come back (not a dark board) and
                            // the page offers Run again.
                            running = None;
                            frame = None;
                            keep = None;
                            if taken {
                                target.rest();
                            }
                            set(&|s| {
                                s.step = None;
                                s.played = true;
                            });
                            continue;
                        }
                    }
                }
            }
            Some(first) => {
                let mut job = first;
                while let Ok(next) = rx.try_recv() {
                    job = newest(job, next);
                }
                match job {
                    Job::End => break,
                    Job::Show(f) => {
                        running = None;
                        frame = Some(f);
                        step = Some(None);
                        set(&|s| s.played = false);
                    }
                    Job::Play(p) => {
                        frame = p.frames.first().cloned();
                        running = frame.as_ref().map(|_| Running {
                            next: clock.now() + p.every,
                            play: p,
                            at: 0,
                        });
                        step = Some(running.as_ref().map(|_| 0));
                        set(&|s| s.played = false);
                    }
                    Job::Again => {}
                }
            }
        }
        let Some(f) = &frame else { continue };
        if !taken {
            if let Err(e) = target.take(again.clone()) {
                set(&|s| s.error = Some(e.clone()));
                break;
            }
            taken = true;
        }
        let sent = target.show(f);
        // The step's time on the keyboard starts now that the keyboard has it (or refused it:
        // the dot goes on, and the page says what went wrong).
        let now = clock.now();
        if let (Some(r), Some(_)) = (running.as_mut(), step) {
            r.next = now + r.play.every;
        }
        match sent {
            Ok(shown) => {
                keep = shown.hold.map(|h| now + keep_alive(h));
                set(&|s| {
                    s.shown = Some(f.clone());
                    s.error = None;
                    s.warn = shown.missing.clone();
                    if let Some(at) = step {
                        s.step = at;
                    }
                });
            }
            Err(e) => {
                // No keep-alive for a frame the keyboard didn't take (gone, not answering): a
                // keyboard arriving or handing its lights back (Again) sends it again, or the
                // dot's next step does. Else a past deadline would send it over and over.
                keep = None;
                set(&|s| {
                    s.error = Some(e.clone());
                    if let Some(at) = step {
                        s.step = at;
                    }
                });
            }
        }
    }
    if taken {
        target.give_back();
    }
}

/// What a keyboard's answer to a frame means for the wizard: how long it holds the frame, and
/// whether a split board's other half draws it (`parts`: the board's parts).
fn shown(reply: &digitalis::protocol::FrameReply, parts: usize) -> Shown {
    use digitalis::protocol::FrameRight;
    let missing = match (parts > 1, reply.right) {
        (false, _) | (true, FrameRight::Draws) => None,
        (true, FrameRight::NoLink) => Some(
            "The other half isn't linked to this one yet, so its LEDs keep their own colour. \
             Press a key on it to wake it."
                .to_string(),
        ),
        (true, FrameRight::Unsupported) => Some(
            "The other half's firmware can't light one LED at a time: update it to check its LEDs."
                .to_string(),
        ),
    };
    Shown {
        hold: (reply.timeout_s > 0).then(|| Duration::from_secs(reply.timeout_s.into())),
        missing,
    }
}

/// The app's keyboard under the wizard's lease.
#[derive(Default)]
pub struct AppKeyboard {
    lease: Option<u64>,
    /// The keyboard the lease was taken on: a rest touches only it.
    device: Option<String>,
    again: Option<Again>,
    /// The hand-back watch on the keyboard's link.
    tapped: Option<(
        Arc<digitalis::link::Link>,
        digitalis::link::EventSubscription,
    )>,
}

impl AppKeyboard {
    /// The keyboard, with the hand-back watch on its link (again after a reconnect).
    fn keyboard(&mut self) -> Result<Arc<digitalis::devices::DigitalisDevice>, String> {
        let kb = super::devices::device().ok_or("No keyboard with Foxglove's firmware answers.")?;
        if !kb.frames() {
            return Err("This keyboard's firmware can't light one LED at a time.".into());
        }
        let same = self
            .tapped
            .as_ref()
            .is_some_and(|(l, _)| Arc::ptr_eq(l, kb.link()));
        if let (false, Some(again)) = (same, self.again.clone()) {
            let watch = kb.link().subscribe_reports(move |_, r| {
                if digitalis::protocol::is_frame_event(r) {
                    again();
                }
            });
            self.tapped = Some((kb.link().clone(), watch));
        }
        Ok(kb)
    }
}

impl Target for AppKeyboard {
    fn take(&mut self, again: Again) -> Result<(), String> {
        if super::pause::paused() {
            return Err(super::pause::REFUSED.into());
        }
        self.again = Some(again);
        let kb = self.keyboard()?;
        use foxglove_devices::Device as _;
        let device = kb.info().id.to_string();
        let light = kb.lighting().ok_or("This keyboard has no lights.")?;
        let id = super::lighting_leases::with(|a| take(a, &device, light)).map_err(|e| {
            // A lease added whose lights then didn't take would hold them with no one to end
            // it: only this wizard takes leases on LEDMAP_CONN, and it holds none yet.
            super::api::end_leases(|l| l.conn == LEDMAP_CONN);
            format!("The keyboard didn't hand over its lights: {e}")
        })?;
        self.lease = Some(id);
        self.device = Some(device);
        Ok(())
    }

    fn show(&mut self, frame: &[Rgb]) -> Result<Shown, String> {
        let kb = self.keyboard()?;
        let reply = kb
            .show_frame(frame)
            .map_err(|e| format!("The keyboard didn't light the LED: {e}"))?;
        Ok(shown(&reply, kb.lamp_parts().len()))
    }

    fn rest(&mut self) {
        // The frame only: the lease stays, so nothing else draws meanwhile. Only on the keyboard
        // the lease is on (another one plugged in meanwhile has no frame of ours).
        use foxglove_devices::Device as _;
        let ours = |kb: &Arc<digitalis::devices::DigitalisDevice>| {
            self.device.as_ref() == Some(&kb.info().id.to_string())
        };
        if let Some(kb) = super::devices::device().filter(ours) {
            if let Err(e) = kb.release_frame() {
                eprintln!("Foxglove: handing the LED map's frame back after the dot: {e}");
            }
        }
    }

    fn give_back(&mut self) {
        self.tapped = None;
        let Some(id) = self.lease.take() else {
            return;
        };
        let kb = super::devices::device();
        use foxglove_devices::Device as _;
        match kb.as_ref().and_then(|k| k.lighting()) {
            Some(light) => super::lighting_leases::with(|a| give_back(a, id, light)),
            // Gone: the lease still ends, so nothing waits on it; the keyboard hands its frame
            // back on its own (the unplug, or its timeout).
            None => super::api::end_leases(|l| l.id == id && l.conn == LEDMAP_CONN),
        }
    }
}

#[cfg(test)]
#[path = "led_map_tests.rs"]
mod tests;
