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
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use foxglove_devices::{DeviceError, Lighting, Rgb};
use serde::{Deserialize, Serialize};

use super::boards::{Board, Drawing, LED_MAP_CHECKED};
use super::lighting_leases::{Arbiter, Lease, Priority};

/// The store section with every board's map.
pub const SECTION: &str = "led_map";

/// The lighting-lease connection the wizard holds the keyboard under (no plugin has it).
pub const LEDMAP_CONN: u64 = u64::MAX - 2;

/// The lit LED's colour: bright white.
pub const LIT: [u8; 3] = [255, 255, 255];

/// Lamps a frame has when the profile doesn't say (the Corne's 21 a half).
const DEFAULT_LAMPS: u16 = 42;

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
    if lighting.lamps != leds.lamps || board.halves != leds.halves {
        eprintln!(
            "Foxglove: the LED map for {} was found with {} LEDs on {} half(s); the profile now says {} on {}: find them again",
            board.id, leds.lamps, leds.halves, lighting.lamps, board.halves
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

/// Whether a profile has a map from "Find my LEDs".
fn placed(board: Option<&Board>) -> bool {
    board
        .and_then(|b| b.lighting.as_ref())
        .is_some_and(|l| !l.led_points.is_empty())
}

/// Whether the firmware's own effects (SET_EFFECT) put the LEDs where they are. Not once the
/// user found them: the firmware places them by its built-in table and takes no positions from
/// the PC yet, so scenes send frames, which do. Never looks anything up (frame threads ask).
pub fn effects_place_leds() -> bool {
    !PLACED.load(Ordering::Relaxed)
}

/// Whether the connected keyboard's LEDs are placed (checked in its profile or found here), so
/// scenes light each one. Never looks for a keyboard (the UI thread asks).
pub fn checked(board: Option<&Board>) -> bool {
    board
        .and_then(|b| b.lighting.as_ref())
        .is_some_and(|l| l.led_map == LED_MAP_CHECKED)
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
    pub per_half: usize,
    /// One answer per LED so far, in order: where it was seen, or `None` ("Can't see it").
    pub answers: Vec<Option<[f32; 2]>>,
    /// A problem to show (a failed save).
    pub note: Option<String>,
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
    /// Starts on a board profile: every LED of the frame, the left half's first.
    pub fn new(board: &Board) -> Wizard {
        let halves = if board.halves == 1 { 1 } else { 2 };
        let lamps = board.lighting.as_ref().map_or(DEFAULT_LAMPS, |l| l.lamps);
        Wizard {
            board: board.id.clone(),
            name: board.name.clone(),
            drawing: board.drawing.clone().unwrap_or_default(),
            halves,
            per_half: usize::from(lamps) / usize::from(halves),
            answers: Vec::new(),
            note: None,
        }
    }

    pub fn total(&self) -> usize {
        self.per_half * usize::from(self.halves)
    }

    /// The LED lit now; `None` once every one is answered.
    pub fn step(&self) -> Option<Step> {
        let lamp = self.answers.len();
        (lamp < self.total()).then(|| Step {
            lamp,
            half: lamp / self.per_half.max(1),
            index: lamp % self.per_half.max(1),
        })
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

/// Takes the keyboard's lights for the wizard: a high-priority lease (dim white, solid) over
/// whatever holds them, so a scene's frames or effect are handed back first. Returns its id.
///
/// The wizard's frames stay its own (it doesn't register them with `draws_frames`, which keeps
/// one drawer a device: the scene's, so the scene is asked to draw again after). So a plugin
/// notification that starts mid-wizard shows under the lit LED until the wizard ends; the
/// wizard is short and the user is watching it.
pub fn take(a: &mut Arbiter, device: &str, light: &dyn Lighting) -> Result<u64, DeviceError> {
    a.add(
        Lease {
            id: 0,
            conn: LEDMAP_CONN,
            device: device.to_string(),
            priority: Priority::High,
            hue: 0,
            sat: 0,
            brightness: 10,
            effect: "0".into(),
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
    /// Send the frame again (the keyboard handed it back, or came back).
    Again,
    End,
}

/// How the worker is getting on, for the page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The lights are held and the last frame went out.
    pub shown: Option<Vec<Rgb>>,
    /// Something went wrong (no keyboard, refused).
    pub error: Option<String>,
}

/// Where the wizard's frames go: the app's keyboard, or a fake in tests.
pub trait Target: Send {
    /// Takes the lights; an error ends the worker. `again` asks for the frame again (the
    /// keyboard handed it back); it may be called on any thread and only flags work.
    fn take(&mut self, again: Again) -> Result<(), String>;
    fn show(&mut self, frame: &[Rgb]) -> Result<(), String>;
    fn give_back(&mut self);
}

/// Asks the worker to send its frame again.
pub type Again = Arc<dyn Fn() + Send + Sync>;

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
            run(target, &rx, again, &st, &wake);
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

/// The newest of the jobs waiting: a newer frame replaces an older one, an End wins.
fn newest(job: Job, next: Job) -> Job {
    match (job, next) {
        (Job::End, _) | (_, Job::End) => Job::End,
        (_, Job::Show(f)) => Job::Show(f),
        (Job::Show(f), Job::Again) => Job::Show(f),
        (Job::Again, Job::Again) => Job::Again,
    }
}

fn run(
    mut target: Box<dyn Target>,
    rx: &mpsc::Receiver<Job>,
    again: Again,
    status: &Mutex<Status>,
    wake: &dyn Fn(),
) {
    let set = |f: &dyn Fn(&mut Status)| {
        f(&mut status.lock().unwrap_or_else(|e| e.into_inner()));
        wake();
    };
    let mut frame: Option<Vec<Rgb>> = None;
    let mut taken = false;
    while let Ok(first) = rx.recv() {
        let mut job = first;
        while let Ok(next) = rx.try_recv() {
            job = newest(job, next);
        }
        match job {
            Job::End => break,
            Job::Show(f) => frame = Some(f),
            Job::Again => {}
        }
        let Some(f) = &frame else { continue };
        if !taken {
            if let Err(e) = target.take(again.clone()) {
                set(&|s| s.error = Some(e.clone()));
                break;
            }
            taken = true;
        }
        match target.show(f) {
            Ok(()) => set(&|s| {
                s.shown = Some(f.clone());
                s.error = None;
            }),
            Err(e) => set(&|s| s.error = Some(e.clone())),
        }
    }
    if taken {
        target.give_back();
    }
}

/// The app's keyboard under the wizard's lease.
#[derive(Default)]
pub struct AppKeyboard {
    lease: Option<u64>,
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
        let id = super::lighting_leases::with(|a| take(a, &device, light))
            .map_err(|e| format!("The keyboard didn't hand over its lights: {e}"))?;
        self.lease = Some(id);
        Ok(())
    }

    fn show(&mut self, frame: &[Rgb]) -> Result<(), String> {
        let kb = self.keyboard()?;
        use foxglove_devices::Device as _;
        let light = kb.lighting().ok_or("This keyboard has no lights.")?;
        light
            .set_frame(frame)
            .map_err(|e| format!("The keyboard didn't light the LED: {e}"))
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
