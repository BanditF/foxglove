//! A running scene (#485): the room map, the shared clock and one effect, sent to the OMEN case
//! and the keyboard while the scheduler's gates are open: a scene runs, someone is present
//! (`away`) and Foxglove isn't paused (`pause`).
//!
//! Who holds the lights:
//! - The keyboard: the scene takes a low-priority lighting lease ([`SCENE_CONN`]), so a
//!   plugin's notification (a mic-mute lease) shows over it and the scene comes back after,
//!   pause gives the keyboard its own lights, and the scene's end restores the lights from
//!   before it (the lease arbiter's snapshot).
//! - With SET_FRAME (HELLO bit 14, #529) the keyboard gets frames: each half its own colour, or
//!   each LED its own once the board profile places the LEDs. The lease still holds the
//!   frame's mix as its colour, so the keyboard shows that whenever the frames are handed
//!   back. The frames are handed back (RELEASE) before anything else shows: a notification
//!   over the scene, pause, the user away, the scene's end and quitting. They come back on the
//!   next scene frame once the lease shows again, and after the keyboard's own hand-back (its
//!   30 s timeout under a still scene, seen as the D7 8C event), never on a timer. Firmware
//!   without the bit gets the one colour as before. The battery dim is the firmware's.
//! - The case: the Lights worker keeps each board's colours from before the scene and puts them
//!   back at its end; Identify still runs (the worker does one thing at a time, and frames that
//!   wait meanwhile collapse to the newest); colours the user picks meanwhile are what comes back.
//!
//! The Scenes page (#487) starts and stops scenes ([`start`], [`stop`]) and changes the look of
//! a running one ([`set_effect`]). The hidden demo for live checks still works too: launch
//! Foxglove with `FOXGLOVE_SCENE_DEMO=<seconds>` (5-600, any other value 30). A
//! scanline runs top to bottom through the room (the case's fans 7, 6, 5, then the keys) and
//! stops by itself after that long, putting the lights back. Quitting stops it too. Copy
//! diagnostics has a "Scene" line with what was sent.
//!
//! Threads: one control thread per scene (it owns the scheduler and takes the gate events),
//! the scheduler's frame thread while the gates are open, and one keyboard writer that sleeps
//! on a condition variable until a colour is due. None exists without a scene.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use foxglove_devices::{DeviceError, Lighting};
use foxglove_scene::scheduler::{Gate, Scheduler, MIN_INTERVAL};
use foxglove_scene::{Clock, Effect, Rgb};

use super::lighting_leases::{Arbiter, Lease, Priority};
use super::scene_output::{CaseTarget, Hsb, KeyboardFrame, Out, Outputs};

/// The lighting-lease connection a scene holds the keyboard under (no plugin has it: plugin
/// connections count up from 1).
pub const SCENE_CONN: u64 = u64::MAX - 1;

/// Launch with this set to run the demo scene for that many seconds.
pub const DEMO_ENV: &str = "FOXGLOVE_SCENE_DEMO";

/// How long a scene's end waits for each output to give the lights back.
const GIVE_BACK: Duration = Duration::from_secs(10);

/// The live check's scene: a Bandit-orange scanline down the room over a dim blue.
pub fn demo_effect() -> Effect {
    Effect::Scanline {
        colour: Rgb([0xf0, 0x8a, 0x3c]),
        background: Rgb([6, 8, 18]),
        angle_deg: 90.0,
        width_mm: 80.0,
        period_ms: 4000,
    }
}

/// How long the demo runs for an environment value.
pub fn demo_seconds(value: &str) -> Duration {
    let secs = value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|s| (5..=600).contains(s))
        .unwrap_or(30);
    Duration::from_secs(secs)
}

/// What a running scene's control thread hears.
#[derive(Debug, PartialEq)]
enum Ctl {
    Away(bool),
    Paused(bool),
    /// The lighting devices changed (or the user moved one): place them again.
    Devices,
    /// Another look for the running scene, and its name.
    Effect(String, Box<Effect>),
    /// A key was pressed at this row and column (#489): a ring from it.
    KeyRipple(usize, usize),
    /// The top layer changed (#489): a sweep in this colour.
    LayerSweep(Rgb),
    Stop,
}

struct Running {
    tx: mpsc::Sender<Ctl>,
    thread: JoinHandle<()>,
    name: String,
    /// The scene's clock, so the Scenes page's preview shows the phase the lights show.
    clock: Clock,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// The right-screen art the running scene shows (a wallpaper look's, #488): `None` while no
/// scene takes art (none runs, or it is ending), `Some(None)` for a scene without art.
static ART: Mutex<Option<Option<super::art_cycle::Item>>> = Mutex::new(None);

/// The running scene shows this right-screen art, or none. A no-op when no scene runs; the
/// scene's end puts back the art from before it (the art service's preset slot).
pub fn set_art(art: Option<super::art_cycle::Item>) {
    let mut cur = ART.lock().unwrap_or_else(|e| e.into_inner());
    let Some(shown) = cur.as_mut() else {
        return;
    };
    if *shown == art {
        return;
    }
    *shown = art.clone();
    if let Some(h) = super::art_service::handle() {
        h.preset_art(art);
    }
}

/// Repaints the Scenes page, if it was ever shown (a wallpaper change, for its "On screen").
pub fn repaint_page() {
    if let Some(ctx) = PAGE.get() {
        ctx.request_repaint();
    }
}

/// What diagnostics says about scenes.
#[derive(Clone, Debug, Default)]
struct Status {
    name: Option<String>,
    started: Option<Instant>,
    case_frames: u64,
    keyboard_frames: u64,
    /// The keyboard draws SET_FRAME frames (`Some(true)`), takes one colour (`Some(false)`), or
    /// hasn't been sent anything yet.
    keyboard_frames_mode: Option<bool>,
    /// The keyboard's own hand-backs (D7 8C events) seen.
    hand_backs: u64,
    last: Option<String>,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    name: None,
    started: None,
    case_frames: 0,
    keyboard_frames: 0,
    keyboard_frames_mode: None,
    hand_backs: 0,
    last: None,
});

fn status() -> std::sync::MutexGuard<'static, Status> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// One line for Copy diagnostics.
pub fn summary() -> String {
    let s = status().clone();
    let keyboard = match s.keyboard_frames_mode {
        Some(true) => format!(
            "{} keyboard frames (SET_FRAME, {} hand-backs)",
            s.keyboard_frames, s.hand_backs
        ),
        _ => format!("{} keyboard colours", s.keyboard_frames),
    };
    let sent = format!("{} case frames, {keyboard}", s.case_frames);
    match (&s.name, s.started) {
        (Some(name), Some(at)) => format!("{name} running {} s, {sent}", at.elapsed().as_secs()),
        _ => match &s.last {
            Some(last) => format!("none running; last: {last}, {sent}"),
            None => "none".into(),
        },
    }
}

/// Starts the demo when Foxglove was launched for a live check (`FOXGLOVE_SCENE_DEMO`).
pub fn start_from_env() {
    let Some(value) = std::env::var_os(DEMO_ENV) else {
        return;
    };
    let limit = demo_seconds(&value.to_string_lossy());
    eprintln!(
        "Foxglove: {DEMO_ENV} set: the demo scene runs for {} s",
        limit.as_secs()
    );
    start("demo scanline", demo_effect(), Some(limit));
}

/// Starts a scene; false if one already runs. `limit` stops it after that long.
pub fn start(name: &str, effect: Effect, limit: Option<Duration>) -> bool {
    // A scene still giving the lights back would undo this one's: wait for its end.
    let Ok(_ending) = ENDING.try_lock() else {
        return false;
    };
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.as_ref().is_some_and(|r| !r.thread.is_finished()) {
        return false;
    }
    let (tx, rx) = mpsc::channel();
    let own = tx.clone();
    let shown = name.to_string();
    let name = name.to_string();
    let clock = Clock::starting_now();
    // Before the thread, whose end (even a failed start) takes it back.
    *ART.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
    let thread = super::services::spawn_named("scene-control", move || {
        control(&name, effect, limit, clock, &own, &rx)
    });
    *running = Some(Running {
        tx,
        thread,
        name: shown,
        clock,
    });
    true
}

/// The running scene's name and clock, or `None` when none runs (or it has just ended by
/// itself).
pub fn running() -> Option<(String, Clock)> {
    RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|r| !r.thread.is_finished())
        .map(|r| (r.name.clone(), r.clock))
}

/// Gives the running scene another look (a no-op when none runs).
pub fn set_effect(name: &str, effect: Effect) {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = running.as_mut().filter(|r| !r.thread.is_finished()) {
        r.name = name.to_string();
        let _ = r.tx.send(Ctl::Effect(name.to_string(), Box::new(effect)));
        status().name = Some(name.to_string());
    }
}

/// The settings window, so a scene's end repaints the Scenes page.
static PAGE: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

/// The Scenes page's context, for [`GiveBack`]'s repaint.
pub fn set_page(ctx: &egui::Context) {
    let _ = PAGE.set(ctx.clone());
}

/// Held while a scene ends, so a second `stop` (quitting while the Scenes page's Stop is still
/// giving the lights back) waits for the first instead of returning at once.
static ENDING: Mutex<()> = Mutex::new(());

/// Stops the running scene and waits until the lights are given back (bounded).
pub fn stop() {
    let _ending = ENDING.lock().unwrap_or_else(|e| e.into_inner());
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(r) = running {
        let _ = r.tx.send(Ctl::Stop);
        let _ = r.thread.join();
    }
}

fn tell(ctl: Ctl) {
    if let Some(r) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = r.tx.send(ctl);
    }
}

/// A key press for "Ripple on key press" (#489): its row and column only. A no-op when no
/// scene runs.
pub fn key_ripple(row: usize, col: usize) {
    tell(Ctl::KeyRipple(row, col));
}

/// A layer change for "Sweep on layer change" (#489), in that layer's colour.
pub fn layer_sweep(colour: Rgb) {
    tell(Ctl::LayerSweep(colour));
}

/// Foxglove paused or resumed (#404).
pub fn pause_changed(paused: bool) {
    tell(Ctl::Paused(paused));
}

/// The Lights worker found a different set of boards, or the user moved a device on the
/// Scenes page's room map.
pub fn devices_changed() {
    super::scene_room::changed();
    tell(Ctl::Devices);
}

/// The room as it is now, and the boards Foxglove drives in it.
fn gather() -> (foxglove_scene::RoomMap, Vec<CaseTarget>) {
    let cases = super::pc_lights::snapshot()
        .devices
        .into_iter()
        .filter(|r| matches!(r.control, foxglove_devices::pc_lighting::Control::Driver(_)))
        .map(|r| CaseTarget {
            key: r.key,
            zone_ids: r.zone_ids,
        })
        .collect();
    (super::scene_room::room(), cases)
}

/// One scene, start to end, on its control thread.
fn control(
    name: &str,
    effect: Effect,
    limit: Option<Duration>,
    clock: Clock,
    own: &mpsc::Sender<Ctl>,
    rx: &mpsc::Receiver<Ctl>,
) {
    {
        let mut s = status();
        *s = Status {
            name: Some(name.to_string()),
            started: Some(Instant::now()),
            ..Status::default()
        };
    }
    // Declared first, so dropped last: after the frame thread and the keyboard writer are
    // gone, even when something here panics.
    let mut give_back = GiveBack {
        name: name.to_string(),
        ended: "it failed",
    };
    super::pc_lights::send(super::pc_lights::Msg::SceneStart);
    let (room, cases) = gather();
    eprintln!("Foxglove: scene {name} started; room: {}", room.summary());
    super::wallpaper_sync::scene_started(name, &effect, clock, room.clone());
    // The room as last placed, for the wallpapers when the look changes.
    let mut placed = room.clone();
    let outputs = Arc::new(Mutex::new(Outputs::new(effect, room, cases)));
    let again = outputs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keyboard_again();
    let keyboard = KeyboardOut::spawn(Box::new(AppKeyboard::new(again.clone())));
    let frames = outputs.clone();
    let mailbox = keyboard.mailbox.clone();
    let scheduler = Scheduler::new(clock, MIN_INTERVAL, move |t| {
        let outs = frames.lock().unwrap_or_else(|e| e.into_inner()).frame(t);
        send(outs, &mailbox);
    });
    let away_tx = own.clone();
    let away = super::away::subscribe(move |a| {
        let _ = away_tx.send(Ctl::Away(a));
    });
    scheduler.set(Gate::Away, super::away::away());
    scheduler.set(Gate::Paused, super::pause::paused());
    scheduler.set(Gate::Scene, true);
    let ended = pump(rx, limit.map(|l| Instant::now() + l), |ctl| match ctl {
        Ctl::Away(a) => {
            if !a {
                // Before the gate opens, so the first frame back draws the keyboard.
                again.store(true, Ordering::Relaxed);
            }
            scheduler.set(Gate::Away, a);
            if a {
                // Nothing draws while away: the keyboard shows the lease's colour, not a
                // frame that stopped.
                keyboard.mailbox.release();
            }
        }
        Ctl::Paused(p) => {
            // Pausing hands the frames back through the lease arbiter (the keyboard's own
            // lights); resuming draws again.
            if !p {
                again.store(true, Ordering::Relaxed);
            }
            scheduler.set(Gate::Paused, p);
        }
        Ctl::Devices => {
            let (room, cases) = gather();
            super::wallpaper_sync::scene_room(room.clone());
            placed = room.clone();
            outputs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_room(room, cases);
        }
        Ctl::KeyRipple(row, col) => {
            let mut o = outputs.lock().unwrap_or_else(|e| e.into_inner());
            let keys = super::scene_triggers::keys();
            let ring =
                super::scene_triggers::ripple(&keys, row, col, o.room(), o.effect(), clock.now());
            if let Some(ring) = ring {
                o.burst(ring);
            }
        }
        Ctl::LayerSweep(colour) => {
            let sweep = super::scene_triggers::sweep(colour, clock.now());
            outputs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .burst(sweep);
        }
        Ctl::Effect(name, effect) => {
            // The wallpapers draw the new look on the same clock (#493).
            super::wallpaper_sync::scene_started(&name, &effect, clock, placed.clone());
            outputs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_effect(*effect);
        }
        Ctl::Stop => {}
    });
    give_back.ended = ended;
    drop(away);
    // Joins the frame thread: nothing draws after this.
    drop(scheduler);
    // Closes the keyboard writer, which gives the keyboard back.
    drop(keyboard);
}

/// A scene's end: the keyboard's lease and the case's colours back, however it ended.
struct GiveBack {
    name: String,
    ended: &'static str,
}

impl Drop for GiveBack {
    fn drop(&mut self) {
        // Idempotent: nothing to end when the keyboard writer already gave it back.
        super::api::end_leases(|l| l.conn == SCENE_CONN);
        super::wallpaper_sync::scene_stopped();
        // The art from before the scene, when it showed a wallpaper's.
        let art = ART.lock().unwrap_or_else(|e| e.into_inner()).take();
        if matches!(art, Some(Some(_))) {
            if let Some(h) = super::art_service::handle() {
                h.preset_art(None);
            }
        }
        let case_back = super::pc_lights::end_scene(GIVE_BACK);
        eprintln!(
            "Foxglove: scene {} ended ({}){}",
            self.name,
            self.ended,
            if case_back {
                ""
            } else {
                "; the case's colours were still being put back"
            }
        );
        {
            let mut s = status();
            s.last = Some(format!("{}, {}", self.name, self.ended));
            s.name = None;
            s.started = None;
        }
        // The Scenes page shows the end, however it came.
        if let Some(ctx) = PAGE.get() {
            ctx.request_repaint();
        }
    }
}

/// Sends a frame's writes: the case's to the Lights worker, the keyboard's to its writer.
fn send(outs: Vec<Out>, keyboard: &Mailbox) {
    for out in outs {
        match out {
            Out::Case { key, colors } => {
                status().case_frames += 1;
                super::pc_lights::send(super::pc_lights::Msg::SceneFrame { key, colors });
            }
            Out::Keyboard(f) => keyboard.post(f),
        }
    }
}

/// Hands each control event to `on` until a stop, the time limit or every sender gone; says
/// which ended it.
fn pump(rx: &mpsc::Receiver<Ctl>, until: Option<Instant>, mut on: impl FnMut(Ctl)) -> &'static str {
    loop {
        let next = match until {
            Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match next {
            Ok(Ctl::Stop) => return "stopped",
            Ok(ctl) => on(ctl),
            Err(RecvTimeoutError::Timeout) => return "its time was up",
            Err(RecvTimeoutError::Disconnected) => return "Foxglove stopped it",
        }
    }
}

/// What the keyboard writer does next.
#[derive(Debug, PartialEq)]
enum Job {
    Show(KeyboardFrame),
    /// Hand the frames back (the user is away).
    Release,
}

#[derive(Default)]
struct Slot {
    frame: Option<KeyboardFrame>,
    release: bool,
    closed: bool,
}

/// The keyboard's newest frame, waiting to be written: one in flight, newest wins.
#[derive(Default)]
struct Mailbox {
    slot: Mutex<Slot>,
    wake: Condvar,
}

impl Mailbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn post(&self, f: KeyboardFrame) {
        self.lock().frame = Some(f);
        self.wake.notify_one();
    }

    /// Hands the frames back; a frame still waiting is dropped (nothing draws meanwhile).
    fn release(&self) {
        let mut slot = self.lock();
        slot.frame = None;
        slot.release = true;
        drop(slot);
        self.wake.notify_one();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_one();
    }

    /// The next job, a hand-back before a frame; `None` once closed (a frame waiting then is
    /// dropped: the scene's end gives the lights back).
    fn next(&self) -> Option<Job> {
        let guard = self.lock();
        let mut slot = self
            .wake
            .wait_while(guard, |s| s.frame.is_none() && !s.release && !s.closed)
            .unwrap_or_else(|e| e.into_inner());
        if slot.closed {
            return None;
        }
        if std::mem::take(&mut slot.release) {
            return Some(Job::Release);
        }
        slot.frame.take().map(Job::Show)
    }
}

/// Where the keyboard's frames go: the app's lease-held keyboard, or a fake in tests.
trait KeyboardLights: Send {
    fn show(&mut self, f: &KeyboardFrame);
    /// Hand the frames back; the lease's colour shows.
    fn release_frames(&mut self);
    /// The scene ended: the lights from before it.
    fn give_back(&mut self);
}

/// The keyboard's writer thread; dropping it closes the mailbox and waits for the lights to
/// be given back.
struct KeyboardOut {
    mailbox: Arc<Mailbox>,
    thread: Option<JoinHandle<()>>,
}

impl KeyboardOut {
    fn spawn(mut lights: Box<dyn KeyboardLights>) -> Self {
        let mailbox = Arc::new(Mailbox::default());
        let mb = mailbox.clone();
        let thread = super::services::spawn_named("scene-keyboard", move || {
            while let Some(job) = mb.next() {
                match job {
                    Job::Show(f) => lights.show(&f),
                    Job::Release => lights.release_frames(),
                }
            }
            lights.give_back();
        });
        KeyboardOut {
            mailbox,
            thread: Some(thread),
        }
    }
}

impl Drop for KeyboardOut {
    fn drop(&mut self) {
        self.mailbox.close();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The scene's lease on one keyboard.
#[derive(Default)]
struct KeyboardLease {
    id: Option<u64>,
    /// Whether this keyboard draws frames: unknown until the first one is tried.
    frames: Option<bool>,
    /// The colour last written without frames.
    sent_mix: Option<Hsb>,
    /// Frames that failed in a row (the keyboard refused or didn't answer).
    frame_errors: u32,
}

/// After this many failed frames in a row (a second of them) the scene sends the keyboard one
/// colour instead, until it next takes the keyboard.
const FRAME_ERRORS: u32 = 10;

impl KeyboardLease {
    /// The first frame takes the lights (a low lease with the frame's mix as its colour: its
    /// snapshot, on, solid); later ones change the lease in place. While it is the one showing
    /// and Foxglove isn't paused, the frame goes out: SET_FRAME (no readback, one frame in
    /// flight), or the mix when it changed on firmware without frames. `again` is the scene's
    /// redraw flag, set when the lease shows again after its frames were handed back.
    fn show(
        &mut self,
        a: &mut Arbiter,
        device: &str,
        light: &dyn Lighting,
        f: &KeyboardFrame,
        again: &Arc<AtomicBool>,
    ) -> Result<(), DeviceError> {
        let c = f.mix;
        if let Some(id) = self.id {
            match a.update(id, SCENE_CONN, c.hue, c.sat, c.brightness, "0") {
                Ok(Some(_)) => return self.draw(a, device, light, f),
                Ok(None) => return Ok(()),
                // Gone (the keyboard was another one): take it again.
                Err(_) => *self = KeyboardLease::default(),
            }
        }
        let lease = Lease {
            id: 0,
            conn: SCENE_CONN,
            device: device.to_string(),
            priority: Priority::Low,
            hue: c.hue,
            sat: c.sat,
            brightness: c.brightness,
            effect: "0".into(),
        };
        let id = a.add(lease, light)?;
        self.id = Some(id);
        // The arbiter just showed the mix (when the lease is the one showing).
        self.sent_mix = Some(c);
        if self.frames != Some(false) {
            let again = again.clone();
            a.draws_frames(id, move || again.store(true, Ordering::Relaxed));
        }
        if a.shows(id) {
            return self.draw(a, device, light, f);
        }
        Ok(())
    }

    /// Sends the frame to a keyboard the lease shows on.
    fn draw(
        &mut self,
        a: &mut Arbiter,
        device: &str,
        light: &dyn Lighting,
        f: &KeyboardFrame,
    ) -> Result<(), DeviceError> {
        if self.frames != Some(false) {
            let lamps: Vec<foxglove_devices::Rgb> = f
                .lamps
                .iter()
                .map(|c| foxglove_devices::Rgb {
                    r: c.0[0],
                    g: c.0[1],
                    b: c.0[2],
                })
                .collect();
            match light.set_frame(&lamps) {
                Ok(()) => {
                    self.frames = Some(true);
                    self.frame_errors = 0;
                    a.frame_sent(device);
                    status().keyboard_frames_mode = Some(true);
                    return Ok(());
                }
                Err(DeviceError::Unsupported) => {
                    // Firmware without SET_FRAME: one colour from now on.
                    self.frames = Some(false);
                    a.no_frames(device);
                    status().keyboard_frames_mode = Some(false);
                }
                Err(e) => {
                    // It may have shown before the reply was lost: hand it back when needed.
                    a.frame_sent(device);
                    self.frame_errors += 1;
                    if self.frame_errors < FRAME_ERRORS {
                        return Err(e);
                    }
                    eprintln!("Foxglove: {FRAME_ERRORS} scene frames failed ({e}); one colour for the keyboard until the scene takes it again");
                    // Hands back whatever shows and puts the lease's colour on.
                    a.release_frames(device, light);
                    a.no_frames(device);
                    self.frames = Some(false);
                    self.sent_mix = Some(f.mix);
                    status().keyboard_frames_mode = Some(false);
                    return Ok(());
                }
            }
        }
        if self.sent_mix == Some(f.mix) {
            return Ok(());
        }
        light.set_color(f.mix.hue, f.mix.sat, f.mix.brightness)?;
        self.sent_mix = Some(f.mix);
        Ok(())
    }
}

/// The keyboard's own hand-back of the frames (D7 8C event: its timeout, an unplug, an output
/// change): the scene sends its frame again on its next tick. Runs on the link's reader thread,
/// so it only sets the flag.
fn hand_back_tap(again: Arc<AtomicBool>) -> impl Fn(u64, &digitalis::link::Report) + Send {
    move |_, r| {
        if digitalis::protocol::is_frame_event(r) {
            again.store(true, Ordering::Relaxed);
            status().hand_backs += 1;
        }
    }
}

/// The host-link keyboard under the scene's lease.
struct AppKeyboard {
    lease: KeyboardLease,
    /// No keyboard was found: don't look again before this (frames keep coming).
    next_look: Option<Instant>,
    device: Option<String>,
    error_logged: bool,
    /// The scene's flag for sending the keyboard its frame again.
    again: Arc<AtomicBool>,
    /// The link whose hand-back events [`hand_back_tap`] watches, and the watch.
    tapped: Option<(
        Arc<digitalis::link::Link>,
        digitalis::link::EventSubscription,
    )>,
}

impl AppKeyboard {
    fn new(again: Arc<AtomicBool>) -> Self {
        AppKeyboard {
            lease: KeyboardLease::default(),
            next_look: None,
            device: None,
            error_logged: false,
            again,
            tapped: None,
        }
    }

    /// Watches this keyboard's link for the frames' hand-back, once per link.
    fn tap(&mut self, kb: &digitalis::devices::DigitalisDevice) {
        if !kb.frames() {
            self.tapped = None;
            return;
        }
        if self
            .tapped
            .as_ref()
            .is_some_and(|(l, _)| Arc::ptr_eq(l, kb.link()))
        {
            return;
        }
        let watch = kb
            .link()
            .subscribe_reports(hand_back_tap(self.again.clone()));
        self.tapped = Some((kb.link().clone(), watch));
    }
}

/// How long a missing keyboard is left before frames look for it again.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

impl KeyboardLights for AppKeyboard {
    fn show(&mut self, f: &KeyboardFrame) {
        if self.next_look.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let Some(kb) = super::devices::device() else {
            self.next_look = Some(Instant::now() + LOOK_AGAIN);
            return;
        };
        self.next_look = None;
        use foxglove_devices::Device as _;
        let Some(light) = kb.lighting() else {
            return;
        };
        let device = kb.info().id.to_string();
        if self.device.as_ref().is_some_and(|d| *d != device) {
            // Another keyboard: the old one's lease ends, its lights come back.
            super::api::end_leases(|l| l.conn == SCENE_CONN);
            self.lease = KeyboardLease::default();
        }
        self.device = Some(device.clone());
        self.tap(&kb);
        let again = self.again.clone();
        let shown = super::lighting_leases::with(|a| self.lease.show(a, &device, light, f, &again));
        match shown {
            Ok(()) => {
                status().keyboard_frames += 1;
                self.error_logged = false;
            }
            Err(e) if !std::mem::replace(&mut self.error_logged, true) => {
                eprintln!("Foxglove: scene colour to the keyboard: {e}");
            }
            Err(_) => {}
        }
    }

    fn release_frames(&mut self) {
        let Some(device) = &self.device else {
            return;
        };
        let Some(kb) = super::devices::device() else {
            return;
        };
        use foxglove_devices::Device as _;
        if let Some(light) = kb.lighting() {
            super::lighting_leases::with(|a| a.release_frames(device, light));
        }
    }

    fn give_back(&mut self) {
        // Ending the lease hands the frames back before the lights from before come back.
        if self.device.is_some() {
            super::api::end_leases(|l| l.conn == SCENE_CONN);
        }
        self.tapped = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_devices::{Effect as LightEffect, LightingState, Zone};
    use foxglove_scene::Rgb as SceneRgb;
    use std::sync::atomic::AtomicU32;

    /// A keyboard's lights in memory, counting colour writes; with `frames`, it takes SET_FRAME
    /// frames, which hide its colour until released.
    struct Fake {
        state: Mutex<LightingState>,
        colours: AtomicU32,
        frames: bool,
        /// The frame showing over the colour, if any.
        frame: Mutex<Option<Vec<foxglove_devices::Rgb>>>,
        frames_sent: AtomicU32,
        releases: AtomicU32,
        /// Frames to fail (no answer) before taking them.
        failing: AtomicU32,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                state: Mutex::new(LightingState {
                    on: true,
                    hue: 200,
                    sat: 40,
                    brightness: 60,
                    effect: LightEffect {
                        id: "3".into(),
                        params: vec![],
                    },
                    speed: 2,
                    settled: true,
                }),
                colours: AtomicU32::new(0),
                frames: false,
                frame: Mutex::new(None),
                frames_sent: AtomicU32::new(0),
                releases: AtomicU32::new(0),
                failing: AtomicU32::new(0),
            }
        }

        fn with_frames() -> Fake {
            Fake {
                frames: true,
                ..Fake::new()
            }
        }

        fn hsb(&self) -> (bool, u16, u8, u8, String) {
            let s = self.state.lock().unwrap();
            (s.on, s.hue, s.sat, s.brightness, s.effect.id.clone())
        }

        /// The frame showing: its left and right halves' first LEDs.
        fn showing(&self) -> Option<([u8; 3], [u8; 3])> {
            self.frame.lock().unwrap().as_ref().map(|f| {
                let rgb = |c: &foxglove_devices::Rgb| [c.r, c.g, c.b];
                (rgb(&f[0]), rgb(&f[HALF]))
            })
        }

        fn count(n: &AtomicU32) -> u32 {
            n.load(Ordering::SeqCst)
        }
    }

    const HALF: usize = super::super::scene_output::HALF_LAMPS;

    impl Lighting for Fake {
        fn layout(&self) -> foxglove_devices::Result<Vec<Zone>> {
            Ok(vec![])
        }
        fn state(&self) -> foxglove_devices::Result<LightingState> {
            Ok(self.state.lock().unwrap().clone())
        }
        fn set_on(&self, on: bool) -> foxglove_devices::Result<()> {
            self.state.lock().unwrap().on = on;
            Ok(())
        }
        fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> foxglove_devices::Result<()> {
            self.colours.fetch_add(1, Ordering::SeqCst);
            let mut s = self.state.lock().unwrap();
            (s.hue, s.sat, s.brightness) = (hue, sat, brightness);
            Ok(())
        }
        fn set_effect(&self, effect: LightEffect) -> foxglove_devices::Result<()> {
            self.state.lock().unwrap().effect = effect;
            Ok(())
        }
        fn set_speed(&self, speed: u8) -> foxglove_devices::Result<()> {
            self.state.lock().unwrap().speed = speed;
            Ok(())
        }
        fn set_frame(&self, frame: &[foxglove_devices::Rgb]) -> foxglove_devices::Result<()> {
            if !self.frames {
                return Err(DeviceError::Unsupported);
            }
            assert_eq!(frame.len(), 2 * HALF, "both halves, every LED");
            if self
                .failing
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(DeviceError::Timeout);
            }
            self.frames_sent.fetch_add(1, Ordering::SeqCst);
            *self.frame.lock().unwrap() = Some(frame.to_vec());
            Ok(())
        }
        fn release(&self) -> foxglove_devices::Result<()> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            *self.frame.lock().unwrap() = None;
            Ok(())
        }
    }

    fn c(hue: u16) -> Hsb {
        Hsb {
            hue,
            sat: 100,
            brightness: 90,
        }
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    /// A frame with each half one colour, and `mix` as its one colour.
    fn halves(left: [u8; 3], right: [u8; 3], mix: Hsb) -> KeyboardFrame {
        let mut lamps = vec![SceneRgb(left); HALF];
        lamps.extend([SceneRgb(right); HALF]);
        KeyboardFrame { lamps, mix }
    }

    /// One colour everywhere.
    fn one(h: Hsb) -> KeyboardFrame {
        halves([h.hue as u8, 0, 0], [h.hue as u8, 0, 0], h)
    }

    fn mute_lease() -> Lease {
        Lease {
            id: 0,
            conn: 7,
            device: "kb".into(),
            priority: Priority::High,
            hue: 0,
            sat: 100,
            brightness: 100,
            effect: "0".into(),
        }
    }

    #[test]
    fn the_scene_takes_the_keyboard_a_notification_wins_and_the_end_restores() {
        let kb = Fake::new();
        let before = kb.hsb();
        let again = Arc::new(AtomicBool::new(false));
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        scene.show(&mut a, "kb", &kb, &one(c(30)), &again).unwrap();
        assert_eq!(
            kb.hsb(),
            (true, 30, 100, 90, "0".into()),
            "on, solid, the frame"
        );
        let writes = Fake::count(&kb.colours);
        scene.show(&mut a, "kb", &kb, &one(c(40)), &again).unwrap();
        assert_eq!(
            Fake::count(&kb.colours),
            writes + 1,
            "a frame is one colour write"
        );
        // Without SET_FRAME, a frame whose one colour didn't change writes nothing.
        let other_lamps = halves(RED, BLUE, c(40));
        scene.show(&mut a, "kb", &kb, &other_lamps, &again).unwrap();
        assert_eq!(Fake::count(&kb.colours), writes + 1);
        // A plugin's mute lease (high) shows over the scene; frames meanwhile write nothing.
        let mute = a.add(mute_lease(), &kb).unwrap();
        let writes = Fake::count(&kb.colours);
        scene.show(&mut a, "kb", &kb, &one(c(50)), &again).unwrap();
        assert_eq!(Fake::count(&kb.colours), writes);
        assert_eq!(kb.hsb().1, 0, "the mute's red");
        // Unmuted: the scene's newest colour, and frames go on.
        for d in a.end(|l| l.id == mute) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(kb.hsb().1, 50);
        // Paused: the keyboard's own lights, and frames write nothing.
        for d in a.set_paused(true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(kb.hsb(), before);
        scene.show(&mut a, "kb", &kb, &one(c(60)), &again).unwrap();
        assert_eq!(kb.hsb(), before);
        for d in a.set_paused(false) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(kb.hsb().1, 60);
        // The scene ends: the lights from before it.
        for d in a.end(|l| l.conn == SCENE_CONN) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(kb.hsb(), before);
        // A lease that went (another keyboard) is taken again.
        scene.show(&mut a, "kb", &kb, &one(c(70)), &again).unwrap();
        assert_eq!(kb.hsb().1, 70);
        // Never a frame, never a hand-back, never a redraw request on firmware without them.
        assert_eq!(Fake::count(&kb.frames_sent), 0);
        assert_eq!(Fake::count(&kb.releases), 0);
        assert!(!again.load(Ordering::SeqCst));
    }

    #[test]
    fn with_set_frame_each_half_gets_its_colour_and_frames_give_way() {
        let kb = Fake::with_frames();
        let before = kb.hsb();
        let again = Arc::new(AtomicBool::new(false));
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        scene
            .show(&mut a, "kb", &kb, &halves(RED, BLUE, c(30)), &again)
            .unwrap();
        // Taken: the mix is the lease's colour (what shows whenever the frames are handed
        // back), and the frame draws over it.
        assert_eq!(kb.hsb(), (true, 30, 100, 90, "0".into()));
        assert_eq!(kb.showing(), Some((RED, BLUE)), "each half its own colour");
        let writes = Fake::count(&kb.colours);
        scene
            .show(&mut a, "kb", &kb, &halves(BLUE, RED, c(40)), &again)
            .unwrap();
        assert_eq!(kb.showing(), Some((BLUE, RED)));
        assert_eq!(Fake::count(&kb.colours), writes, "frames, no colour writes");
        assert_eq!(Fake::count(&kb.frames_sent), 2);

        // A notification: the frames are handed back before its colour shows.
        let mute = a.add(mute_lease(), &kb).unwrap();
        assert_eq!(Fake::count(&kb.releases), 1);
        assert_eq!(kb.showing(), None);
        assert_eq!(kb.hsb().1, 0, "the mute's red, visible");
        scene
            .show(&mut a, "kb", &kb, &halves(RED, RED, c(50)), &again)
            .unwrap();
        assert_eq!(Fake::count(&kb.frames_sent), 2, "no frames under it");
        assert!(!again.load(Ordering::SeqCst));
        // Unmuted: the scene's colour, and the scene is asked for its frame again.
        for d in a.end(|l| l.id == mute) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(kb.hsb().1, 50);
        assert!(again.swap(false, Ordering::SeqCst), "draw again");
        scene
            .show(&mut a, "kb", &kb, &halves(RED, RED, c(50)), &again)
            .unwrap();
        assert_eq!(kb.showing(), Some((RED, RED)));

        // Paused: handed back, the keyboard's own lights, nothing drawn until resumed.
        for d in a.set_paused(true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(Fake::count(&kb.releases), 2);
        assert_eq!((kb.showing(), kb.hsb()), (None, before.clone()));
        let sent = Fake::count(&kb.frames_sent);
        scene
            .show(&mut a, "kb", &kb, &halves(BLUE, BLUE, c(60)), &again)
            .unwrap();
        assert_eq!(Fake::count(&kb.frames_sent), sent);
        for d in a.set_paused(false) {
            a.apply(&d, &kb).unwrap();
        }
        assert!(again.swap(false, Ordering::SeqCst));
        scene
            .show(&mut a, "kb", &kb, &halves(BLUE, RED, c(60)), &again)
            .unwrap();
        assert_eq!(kb.showing(), Some((BLUE, RED)));

        // A new look: frames only, so the keyboard's own colour is still the last one shown.
        scene
            .show(&mut a, "kb", &kb, &halves(RED, RED, c(65)), &again)
            .unwrap();
        assert_eq!(kb.hsb().1, 60);
        // Away: handed back with the lease kept; the lease's current colour shows.
        a.release_frames("kb", &kb);
        assert_eq!(Fake::count(&kb.releases), 3);
        assert_eq!((kb.showing(), kb.hsb().1), (None, 65));
        a.release_frames("kb", &kb);
        assert_eq!(Fake::count(&kb.releases), 3, "nothing to hand back");
        // Back: the next frame takes the lights again.
        scene
            .show(&mut a, "kb", &kb, &halves(RED, BLUE, c(70)), &again)
            .unwrap();
        assert_eq!(kb.showing(), Some((RED, BLUE)));

        // The keyboard reconnected (an unplug hands frames back): draw again.
        a.connected("kb", &kb).unwrap();
        assert!(again.swap(false, Ordering::SeqCst));

        // The end (Stop, quit, the limit): handed back, then the lights from before.
        for d in a.end(|l| l.conn == SCENE_CONN) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(Fake::count(&kb.releases), 4);
        assert_eq!((kb.showing(), kb.hsb()), (None, before));
    }

    #[test]
    fn frames_that_keep_failing_fall_back_to_one_colour() {
        let kb = Fake::with_frames();
        let again = Arc::new(AtomicBool::new(false));
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        scene.show(&mut a, "kb", &kb, &one(c(30)), &again).unwrap();
        kb.failing.store(FRAME_ERRORS, Ordering::SeqCst);
        for i in 1..FRAME_ERRORS {
            let r = scene.show(&mut a, "kb", &kb, &one(c(30 + i as u16)), &again);
            assert!(r.is_err(), "frame {i} failed");
        }
        // The last one: handed back, the lease's colour on, one colour from now on.
        scene.show(&mut a, "kb", &kb, &one(c(50)), &again).unwrap();
        assert_eq!(Fake::count(&kb.releases), 1);
        assert_eq!((kb.showing(), kb.hsb().1), (None, 50));
        let sent = Fake::count(&kb.frames_sent);
        scene.show(&mut a, "kb", &kb, &one(c(55)), &again).unwrap();
        assert_eq!(Fake::count(&kb.frames_sent), sent);
        assert_eq!(kb.hsb().1, 55);
        // And a notification no longer sends RELEASE.
        a.add(mute_lease(), &kb).unwrap();
        assert_eq!(Fake::count(&kb.releases), 1);
    }

    #[test]
    fn the_keyboards_own_hand_back_asks_for_the_frame_again() {
        let again = Arc::new(AtomicBool::new(false));
        let tap = hand_back_tap(again.clone());
        let mut r = [0u8; digitalis::protocol::REPORT];
        // A frame's own reply is not a hand-back.
        r[..7].copy_from_slice(&[0xD7, 0x8C, 0, 3, 30, 1, 0]);
        tap(1, &r);
        assert!(!again.load(Ordering::SeqCst));
        // The timeout's hand-back event.
        r[..7].copy_from_slice(&[0xD7, 0x8C, 0, 0, 30, 1, 1]);
        tap(1, &r);
        assert!(again.load(Ordering::SeqCst));
    }

    #[test]
    fn the_mailbox_hands_back_before_drawing_and_drops_a_waiting_frame() {
        let mb = Mailbox::default();
        mb.post(one(c(1)));
        mb.release();
        assert_eq!(
            mb.next(),
            Some(Job::Release),
            "away drops the frame waiting"
        );
        mb.release();
        mb.post(one(c(2)));
        assert_eq!(mb.next(), Some(Job::Release));
        assert_eq!(mb.next(), Some(Job::Show(one(c(2)))));
        mb.post(one(c(3)));
        mb.close();
        assert_eq!(mb.next(), None);
    }

    /// Records what the writer thread was given.
    struct Recorded(Arc<Mutex<Vec<Option<Hsb>>>>);

    impl KeyboardLights for Recorded {
        fn show(&mut self, f: &KeyboardFrame) {
            self.0.lock().unwrap().push(Some(f.mix));
            // A slow keyboard: frames posted meanwhile collapse to the newest.
            std::thread::sleep(Duration::from_millis(30));
        }
        fn release_frames(&mut self) {}
        fn give_back(&mut self) {
            self.0.lock().unwrap().push(None);
        }
    }

    /// Counts the writer's hand-backs.
    struct Releases(Arc<AtomicU32>);

    impl KeyboardLights for Releases {
        fn show(&mut self, _: &KeyboardFrame) {}
        fn release_frames(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn give_back(&mut self) {}
    }

    #[test]
    fn away_reaches_the_keyboard_writer() {
        let n = Arc::new(AtomicU32::new(0));
        let out = KeyboardOut::spawn(Box::new(Releases(n.clone())));
        out.mailbox.release();
        let until = Instant::now() + Duration::from_secs(5);
        while n.load(Ordering::SeqCst) == 0 && Instant::now() < until {
            std::thread::yield_now();
        }
        drop(out);
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_busy_keyboard_gets_the_newest_colour_and_the_end_gives_back() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let out = KeyboardOut::spawn(Box::new(Recorded(seen.clone())));
        out.mailbox.post(one(c(1)));
        // Wait (bounded) until the first is being written, then post a burst.
        let until = Instant::now() + Duration::from_secs(5);
        while seen.lock().unwrap().is_empty() && Instant::now() < until {
            std::thread::yield_now();
        }
        for h in 2..=9 {
            out.mailbox.post(one(c(h)));
        }
        // Wait (bounded) for the newest.
        while !seen.lock().unwrap().contains(&Some(c(9))) && Instant::now() < until {
            std::thread::yield_now();
        }
        drop(out);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.first(), Some(&Some(c(1))));
        assert!(seen.len() <= 4, "a burst collapses: {seen:?}");
        assert_eq!(&seen[seen.len() - 2..], [Some(c(9)), None], "{seen:?}");
    }

    #[test]
    fn gate_events_reach_the_scheduler_and_stop_or_the_limit_ends_the_scene() {
        let frames = Arc::new(AtomicU32::new(0));
        let f = frames.clone();
        let sched = Scheduler::new(Clock::starting_now(), MIN_INTERVAL, move |_| {
            f.fetch_add(1, Ordering::SeqCst);
        });
        sched.set(Gate::Scene, true);
        assert!(sched.running());
        let (tx, rx) = mpsc::channel();
        for ctl in [
            Ctl::Away(true),
            Ctl::Away(false),
            Ctl::Paused(true),
            Ctl::Stop,
        ] {
            tx.send(ctl).unwrap();
        }
        let mut seen = Vec::new();
        let ended = pump(&rx, None, |ctl| {
            match ctl {
                Ctl::Away(a) => sched.set(Gate::Away, a),
                Ctl::Paused(p) => sched.set(Gate::Paused, p),
                _ => {}
            }
            seen.push(sched.running());
        });
        assert_eq!(ended, "stopped");
        assert_eq!(
            seen,
            [false, true, false],
            "away stops, back resumes, pause stops"
        );
        assert!(!sched.running());
        // Nothing comes: the limit ends it.
        let started = Instant::now();
        let ended = pump(
            &rx,
            Some(Instant::now() + Duration::from_millis(50)),
            |_| {},
        );
        assert_eq!(ended, "its time was up");
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(tx);
        assert_eq!(pump(&rx, None, |_| {}), "Foxglove stopped it");
    }

    /// A Start while a stop still gives the lights back is refused: the old scene's end would
    /// undo the new one's lease and case hold.
    #[test]
    fn no_start_while_a_scene_is_still_ending() {
        let _ending = ENDING.lock().unwrap();
        assert!(!start("test", demo_effect(), Some(Duration::from_secs(5))));
        assert!(running().is_none());
    }

    #[test]
    fn the_demo_is_a_scanline_down_the_room_for_a_bounded_time() {
        assert_eq!(demo_seconds("20"), Duration::from_secs(20));
        assert_eq!(demo_seconds("1"), Duration::from_secs(30), "too short");
        assert_eq!(demo_seconds("100000"), Duration::from_secs(30), "too long");
        assert_eq!(demo_seconds("yes"), Duration::from_secs(30));
        assert!(matches!(
            demo_effect(),
            Effect::Scanline { angle_deg, .. } if angle_deg == 90.0
        ));
        assert_eq!(summary(), "none", "nothing ran in tests");
    }
}
