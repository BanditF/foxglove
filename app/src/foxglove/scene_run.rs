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
//! - With SET_EFFECT (HELLO bit 19, #542) the keyboard draws the scene itself: PLACE (the room
//!   and both halves, mm) and EFFECT go once, with the effect's clock set from the scene's
//!   epoch at the moment it is sent, so the halves, the case and the wallpapers share a phase.
//!   No keyboard frames go out while it runs. It is sent on start, on a look change, after the
//!   room changes, after a reconnect or HELLO, and when the scene's lease shows again; it is
//!   handed back wherever frames are (`Lighting::release` ends both), so the rules are:
//!
//!   | Event | The keyboard's effect |
//!   |---|---|
//!   | Stop (Scenes page, a profile without the scene, the time limit) | released: the lights from before the scene |
//!   | Pause | released; resuming sends it again |
//!   | Away (locked, display off) | released; coming back sends it again |
//!   | A notification lease (mic mute) | released under it; sent again after |
//!   | Profile switch keeping the scene | the new look replaces it, no release in between |
//!   | Quit | released, unless "Keep running on the keyboard" ([`KeyboardKeep`], off by default) is on and the effect is showing: it stays until Foxglove next finds the keyboard (or changes its lights), which hands it back |
//!
//!   Scenes that don't map (gradient, noise), and ripples or sweeps over a scene (#489), use
//!   frames while they show. Copy diagnostics says which path the keyboard got.
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

use digitalis::protocol::MmRect;
use foxglove_devices::{DeviceError, Lighting};
use foxglove_scene::scheduler::{Gate, Scheduler, MIN_INTERVAL};
use foxglove_scene::{Clock, Effect, Rgb};
use serde::{Deserialize, Serialize};

use super::lighting_leases::{Arbiter, Lease, Priority};
use super::scene_lamps::{LampStatus, LampWriter, Poster};
use super::scene_output::{
    CaseTarget, Hsb, KeyboardEffect, KeyboardFrame, LampTarget, Out, Outputs,
};

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

/// The store section for [`KeyboardKeep`] (not profiled: it is about quitting).
pub const KEEP_SECTION: &str = "scene_keyboard";

/// `[scene_keyboard]`: what quitting does to a scene's effect on the keyboard (#542).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardKeep {
    /// "Keep running on the keyboard": quitting leaves the scene's effect running on it. Off by
    /// default: quitting gives the keyboard its own lights back.
    pub on_quit: bool,
    /// Quitting left an effect running: Foxglove hands it back when it next finds the keyboard.
    pub left_running: bool,
}

/// The keep setting as stored (defaults when the store can't be read).
pub fn keep() -> KeyboardKeep {
    super::store::try_load(KEEP_SECTION).unwrap_or_default()
}

fn save_keep(k: KeyboardKeep) {
    if let Err(e) = super::store::save(KEEP_SECTION, &k) {
        eprintln!("Foxglove: can't save [{KEEP_SECTION}]: {e}");
    }
}

/// The Scenes page's "Keep running on the keyboard" box.
pub fn set_keep_on_quit(on: bool) {
    save_keep(KeyboardKeep {
        on_quit: on,
        ..keep()
    });
}

/// Before Foxglove changes the keyboard's lights itself (the Lights page): an effect a quit
/// left running would hide the change, so it is handed back first. Once: the mark goes with it.
pub fn release_left_effect(kb: &digitalis::devices::DigitalisDevice) {
    let k = keep();
    if !k.left_running || running().is_some() || QUITTING.load(Ordering::Relaxed) {
        return;
    }
    if kb.effects() {
        if let Err(e) = kb.release_effect() {
            eprintln!("Foxglove: handing back the effect left on the keyboard: {e}");
            return;
        }
    }
    save_keep(KeyboardKeep {
        left_running: false,
        ..k
    });
}

/// Set by [`quit`]: the scene ends because Foxglove quits.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Foxglove quits: the running scene ends as [`stop`] does, except that the keyboard keeps an
/// effect it is showing when the user ticked "Keep running on the keyboard".
pub fn quit() {
    end(true);
}

/// A keyboard connected (the device bus's Connected, after the lease arbiter's own reconnect):
/// a running scene sends the keyboard its effect or frame again, and looks for it at once even
/// if it was missing a moment ago. With no scene, an effect a quit left running is handed back:
/// Foxglove runs again and manages the lights.
pub fn keyboard_connected() {
    // "Find my LEDs" lights its LED again on the keyboard that came back.
    super::led_map::keyboard_connected();
    if running().is_some() {
        ARRIVED.store(true, Ordering::Relaxed);
        tell(Ctl::KeyboardArrived);
        return;
    }
    if QUITTING.load(Ordering::Relaxed) || !keep().left_running {
        return;
    }
    if let Some(kb) = super::devices::device() {
        release_left_effect(&kb);
    }
}

/// Set by [`keyboard_connected`], taken by the keyboard writer: look for the keyboard now.
static ARRIVED: AtomicBool = AtomicBool::new(false);

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
    /// A keyboard connected: send it the scene again.
    KeyboardArrived,
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
    /// Effects sent to the keyboard (SET_EFFECT).
    keyboard_effects: u64,
    /// The path the keyboard got last, or `None` before anything was sent.
    keyboard_path: Option<KeyboardPath>,
    /// The keyboard's own hand-backs (D7 8C events) seen.
    hand_backs: u64,
    last: Option<String>,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    name: None,
    started: None,
    case_frames: 0,
    keyboard_frames: 0,
    keyboard_effects: 0,
    keyboard_path: None,
    hand_backs: 0,
    last: None,
});

/// How the keyboard shows the scene (#542).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyboardPath {
    /// It draws the effect itself (SET_EFFECT).
    Effect,
    /// SET_FRAME frames.
    Frames,
    /// One `&rgb_ug` colour.
    OneColour,
}

/// The running scene's Dynamic Lighting devices (#503), for the Scenes page and diagnostics.
static LAMPS: std::sync::LazyLock<super::scene_lamps::Status> =
    std::sync::LazyLock::new(super::scene_lamps::Status::default);

/// What the running scene's Dynamic Lighting devices are doing.
pub fn lamps() -> LampStatus {
    LAMPS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn status() -> std::sync::MutexGuard<'static, Status> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// One line for Copy diagnostics.
pub fn summary() -> String {
    let s = status().clone();
    let keyboard = match s.keyboard_path {
        Some(KeyboardPath::Effect) => format!(
            "keyboard: effect (SET_EFFECT, {} sent; {} frames)",
            s.keyboard_effects, s.keyboard_frames
        ),
        Some(KeyboardPath::Frames) => format!(
            "keyboard: frames ({} SET_FRAME, {} hand-backs; {} effects)",
            s.keyboard_frames, s.hand_backs, s.keyboard_effects
        ),
        _ => format!("keyboard: one colour ({} colours)", s.keyboard_frames),
    };
    let l = lamps();
    let sent = format!(
        "{} case frames, {keyboard}, Dynamic Lighting: {} frames, {} showing, {} waiting (package identity: {})",
        s.case_frames,
        l.frames,
        l.showing.len(),
        l.waiting.len(),
        if super::scene_lamps::identity() { "yes" } else { "no" }
    );
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
    end(false);
}

/// Ends the running scene; `quitting` is read by its end after any stop still giving the
/// lights back has finished (so a quit never keeps an effect a Stop is releasing).
fn end(quitting: bool) {
    let _ending = ENDING.lock().unwrap_or_else(|e| e.into_inner());
    if quitting {
        QUITTING.store(true, Ordering::Relaxed);
    }
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

/// The keyboard's LEDs by part now: its board profile's (#561).
fn keyboard_parts() -> digitalis::lamps::LampParts {
    super::boards::lamp_parts(super::boards::current().as_ref())
}

/// The room as it is now, the boards Foxglove drives in it and its Dynamic Lighting devices.
fn gather() -> (foxglove_scene::RoomMap, Vec<CaseTarget>, Vec<LampTarget>) {
    use foxglove_devices::pc_lighting::Control;
    let rows = super::pc_lights::snapshot().devices;
    let cases = rows
        .iter()
        .filter(|r| matches!(r.control, Control::Driver(_)))
        .map(|r| CaseTarget {
            key: r.key.clone(),
            zone_ids: r.zone_ids.clone(),
        })
        .collect();
    let lamps = rows.iter().filter_map(lamp_target).collect();
    (super::scene_room::room(), cases, lamps)
}

/// A Lights page row as a Dynamic Lighting output: any LampArray, by its own attributes.
fn lamp_target(r: &super::pc_lights::Row) -> Option<LampTarget> {
    (r.control == foxglove_devices::pc_lighting::Control::LampArray).then(|| LampTarget {
        key: r.key.clone(),
        lamps: r.caps.count as usize,
        min_interval: r
            .caps
            .fps
            .filter(|f| *f > 0)
            .map(|f| Duration::from_micros(1_000_000 / u64::from(f))),
    })
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
    let (room, cases, lamp_targets) = gather();
    eprintln!("Foxglove: scene {name} started; room: {}", room.summary());
    super::wallpaper_sync::scene_started(name, &effect, clock, room.clone());
    // The room as last placed, for the wallpapers when the look changes.
    let mut placed = room.clone();
    let outputs = Arc::new(Mutex::new(Outputs::new(effect, room, cases)));
    {
        let mut o = outputs.lock().unwrap_or_else(|e| e.into_inner());
        o.set_keyboard_parts(keyboard_parts());
        o.set_lamps(lamp_targets);
    }
    // Declared after `give_back`, so dropped before it (a panic too): lets every Dynamic
    // Lighting device go before the end is reported. Package identity is read here once, off
    // the UI thread, for the Scenes page.
    let _ = super::scene_lamps::identity();
    let lamp_writer = LampWriter::spawn(
        Box::<super::scene_lamps::WindowsLamps>::default(),
        LAMPS.clone(),
    );
    let lamp_poster = lamp_writer.poster();
    let (again, effects) = {
        let o = outputs.lock().unwrap_or_else(|e| e.into_inner());
        (o.keyboard_again(), o.keyboard_effects())
    };
    // What the keyboard found last says (the writer corrects it), so the first send is
    // already the effect rather than a frame.
    effects.store(
        super::devices::effects()
            && super::led_map::effects_place_leds(super::devices::effect_lamps()),
        Ordering::Relaxed,
    );
    let keyboard = KeyboardOut::spawn(Box::new(AppKeyboard::new(again.clone(), effects, clock)));
    let frames = outputs.clone();
    let mailbox = keyboard.mailbox.clone();
    let scheduler = Scheduler::new(clock, MIN_INTERVAL, move |t| {
        let outs = frames.lock().unwrap_or_else(|e| e.into_inner()).frame(t);
        send(outs, &mailbox, &lamp_poster);
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
                // Before the gate opens, so the first frame back draws the keyboard and the
                // Dynamic Lighting devices (let go while away) even when nothing changed.
                again.store(true, Ordering::Relaxed);
                redraw_lamps(&outputs);
            }
            scheduler.set(Gate::Away, a);
            if a {
                // Nothing draws while away: the keyboard shows the lease's colour, not a
                // frame that stopped, and Dynamic Lighting devices Windows' own lights.
                keyboard.mailbox.release();
                lamp_writer.release();
            }
        }
        Ctl::Paused(p) => {
            // Pausing hands the frames back through the lease arbiter (the keyboard's own
            // lights); resuming draws again.
            if !p {
                again.store(true, Ordering::Relaxed);
                redraw_lamps(&outputs);
            }
            scheduler.set(Gate::Paused, p);
            if p {
                // Paused: Windows' own lights on Dynamic Lighting devices.
                lamp_writer.release();
            }
        }
        Ctl::Devices => {
            let (room, cases, lamp_targets) = gather();
            super::wallpaper_sync::scene_room(room.clone());
            placed = room.clone();
            lamp_writer.devices_changed();
            let mut o = outputs.lock().unwrap_or_else(|e| e.into_inner());
            o.set_keyboard_parts(keyboard_parts());
            o.set_room(room, cases);
            o.set_lamps(lamp_targets);
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
        Ctl::KeyboardArrived => again.store(true, Ordering::Relaxed),
        Ctl::Stop => {}
    });
    give_back.ended = ended;
    drop(away);
    // Joins the frame thread: nothing draws after this.
    drop(scheduler);
    // Closes the keyboard writer, which gives the keyboard back.
    drop(keyboard);
    // Lets every Dynamic Lighting device go: Windows' own lighting comes back.
    drop(lamp_writer);
}

/// Dynamic Lighting devices let go while paused or away get their next frame in full.
fn redraw_lamps(outputs: &Mutex<Outputs>) {
    outputs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .redraw_lamps();
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

/// Sends a frame's writes: the case's to the Lights worker, the keyboard's to its writer, the
/// Dynamic Lighting devices' to theirs.
fn send(outs: Vec<Out>, keyboard: &Mailbox, lamps: &Poster) {
    for out in outs {
        match out {
            Out::Case { key, colors } => {
                status().case_frames += 1;
                super::pc_lights::send(super::pc_lights::Msg::SceneFrame { key, colors });
            }
            Out::Keyboard(f) => keyboard.post(Draw::Frame(f)),
            Out::KeyboardEffect(e) => keyboard.post(Draw::Effect(e)),
            Out::Lamps { key, colors } => lamps.post(key, colors),
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

/// What the keyboard is sent: a frame, or the effect it draws itself.
#[derive(Clone, Debug, PartialEq)]
enum Draw {
    Frame(KeyboardFrame),
    Effect(KeyboardEffect),
}

/// What the keyboard writer does next.
#[derive(Debug, PartialEq)]
enum Job {
    Show(Draw),
    /// Hand the frames (or the effect) back (the user is away).
    Release,
}

#[derive(Default)]
struct Slot {
    frame: Option<Draw>,
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

    fn post(&self, f: Draw) {
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
    /// The keyboard draws the scene itself from now on (SET_EFFECT).
    fn effect(&mut self, e: &KeyboardEffect);
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
                    Job::Show(Draw::Frame(f)) => lights.show(&f),
                    Job::Show(Draw::Effect(e)) => lights.effect(&e),
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
    /// Effects that failed in a row.
    effect_errors: u32,
    /// The scene's effect is what the keyboard shows now (the last thing sent was it, and the
    /// lease shows): what "Keep running on the keyboard" keeps on quit.
    effect_on: bool,
    /// The LED points (EFFECT LAMPS, #550) this keyboard last took: sent again only when they
    /// change, or after a reconnect (the keyboard forgets them on a reboot).
    lamps_sent: Option<Option<Vec<Vec<[u16; 2]>>>>,
}

/// Where the keyboard's effect goes: the host-link keyboard, or a fake in tests.
trait EffectTarget {
    /// EFFECT PLACE: the room's bounds and each half's place in it.
    fn place(&self, room: MmRect, left: MmRect, right: MmRect) -> Result<(), DeviceError>;
    /// EFFECT, with the keyboard's colour calibration on its colours.
    fn run(&self, e: &digitalis::protocol::Effect) -> Result<(), DeviceError>;
    /// EFFECT LAMPS: where each LED of `half` sits; empty puts the half back on the firmware's
    /// table. Unsupported on firmware without HELLO bit 21 (#550).
    fn lamps(&self, half: u8, points: &[[u16; 2]]) -> Result<(), DeviceError>;
}

impl EffectTarget for digitalis::devices::DigitalisDevice {
    fn place(&self, room: MmRect, left: MmRect, right: MmRect) -> Result<(), DeviceError> {
        self.place_effect(room, left, right).map(|_| ())
    }
    fn run(&self, e: &digitalis::protocol::Effect) -> Result<(), DeviceError> {
        self.show_effect(e).map(|_| ())
    }
    fn lamps(&self, half: u8, points: &[[u16; 2]]) -> Result<(), DeviceError> {
        self.place_effect_lamps(half, points).map(|_| ())
    }
}

/// EFFECT LAMPS for every part: the room's points, or the firmware's table again without
/// them. Firmware that takes no points keeps its table (the scene then uses frames for a map
/// from "Find my LEDs", led_map::effects_place_leds).
fn send_lamps(target: &dyn EffectTarget, e: &KeyboardEffect) -> Result<(), DeviceError> {
    for part in 0..e.parts.len() {
        let points = e
            .lamps
            .as_ref()
            .and_then(|l| l.get(part))
            .map_or(&[][..], |p| &p[..]);
        let Ok(part) = u8::try_from(part) else { break };
        match target.lamps(part, points) {
            Ok(()) | Err(DeviceError::Unsupported) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// What sending the effect came to.
#[derive(Debug, PartialEq, Eq)]
enum EffectSent {
    /// It runs.
    Done,
    /// The lease doesn't show now (a notification, pause): sent again when it does.
    Waiting,
    /// The keyboard takes no effects after all (refused, or failed [`FRAME_ERRORS`] times in a
    /// row): frames from now on.
    Frames,
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
    ) -> Result<bool, DeviceError> {
        let draws = self.frames != Some(false);
        if self.hold(a, device, light, f.mix, draws, again)? {
            self.effect_on = false;
            self.draw(a, device, light, f)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The effect for a keyboard that draws it itself: the lease holds its colours' mix (what
    /// shows when it is handed back), and while the lease shows, LAMPS, PLACE then EFFECT go
    /// out with the clock read just before. LAMPS and PLACE go each time, since a reboot
    /// forgets them.
    #[allow(clippy::too_many_arguments)]
    fn show_effect(
        &mut self,
        a: &mut Arbiter,
        device: &str,
        light: &dyn Lighting,
        target: &dyn EffectTarget,
        e: &KeyboardEffect,
        clock: &Clock,
        again: &Arc<AtomicBool>,
    ) -> Result<EffectSent, DeviceError> {
        let shows = match self.hold(a, device, light, e.mix, true, again) {
            Ok(shows) => shows,
            // The lease itself failed (the keyboard didn't answer): bounded like the sends.
            Err(err) => return self.failed(err),
        };
        if !shows {
            self.effect_on = false;
            return Ok(EffectSent::Waiting);
        }
        let lamps = if self.lamps_sent.as_ref() == Some(&e.lamps) {
            Ok(())
        } else {
            send_lamps(target, e).inspect(|()| self.lamps_sent = Some(e.lamps.clone()))
        };
        let (left, right) = e.place();
        let sent = lamps
            .and_then(|()| target.place(e.room, left, right))
            .and_then(|()| target.run(&e.at(clock.now())));
        match sent {
            Ok(()) => {
                self.effect_errors = 0;
                self.effect_on = true;
                a.frame_sent(device);
                status().keyboard_path = Some(KeyboardPath::Effect);
                Ok(EffectSent::Done)
            }
            Err(DeviceError::Unsupported) => {
                self.effect_on = false;
                Ok(EffectSent::Frames)
            }
            Err(err) => {
                // It may run although the reply was lost: hand it back when needed.
                a.frame_sent(device);
                self.effect_on = false;
                self.failed(err)
            }
        }
    }

    /// One more failed effect: the error, or frames after [`FRAME_ERRORS`] in a row.
    fn failed(&mut self, err: DeviceError) -> Result<EffectSent, DeviceError> {
        self.effect_errors += 1;
        if self.effect_errors < FRAME_ERRORS {
            return Err(err);
        }
        eprintln!("Foxglove: {FRAME_ERRORS} scene effects failed ({err}); frames for the keyboard until it connects again");
        self.effect_errors = 0;
        Ok(EffectSent::Frames)
    }

    /// Takes the lights (a low lease with `mix` as its colour: its snapshot, on, solid), or
    /// changes the lease in place; true while it is the one showing and Foxglove isn't paused.
    /// `draws`: the lease draws frames or an effect over its colour, which the arbiter hands
    /// back before anything else shows and asks for again (`again`) when the lease shows again.
    fn hold(
        &mut self,
        a: &mut Arbiter,
        device: &str,
        light: &dyn Lighting,
        c: Hsb,
        draws: bool,
        again: &Arc<AtomicBool>,
    ) -> Result<bool, DeviceError> {
        if let Some(id) = self.id {
            match a.update(id, SCENE_CONN, c.hue, c.sat, c.brightness, "0") {
                Ok(shows) => return Ok(shows.is_some()),
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
        if draws {
            let again = again.clone();
            a.draws_frames(id, move || again.store(true, Ordering::Relaxed));
        }
        Ok(a.shows(id))
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
                    status().keyboard_path = Some(KeyboardPath::Frames);
                    return Ok(());
                }
                Err(DeviceError::Unsupported) => {
                    // Firmware without SET_FRAME: one colour from now on.
                    self.frames = Some(false);
                    a.no_frames(device);
                    status().keyboard_path = Some(KeyboardPath::OneColour);
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
                    status().keyboard_path = Some(KeyboardPath::OneColour);
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
    /// The scene's flag for the keyboard drawing its effect itself ([`Outputs::keyboard_effects`]).
    effects: Arc<AtomicBool>,
    /// The keyboard refused effects (or they kept failing): frames until another keyboard.
    effects_failed: bool,
    /// The scene's clock, read just before an effect goes.
    clock: Clock,
    /// The "left running" mark of a quit before was looked at (once per scene).
    left_checked: bool,
    /// The link whose hand-back events [`hand_back_tap`] watches, and the watch.
    tapped: Option<(
        Arc<digitalis::link::Link>,
        digitalis::link::EventSubscription,
    )>,
}

impl AppKeyboard {
    fn new(again: Arc<AtomicBool>, effects: Arc<AtomicBool>, clock: Clock) -> Self {
        AppKeyboard {
            lease: KeyboardLease::default(),
            next_look: None,
            device: None,
            error_logged: false,
            again,
            effects,
            effects_failed: false,
            clock,
            left_checked: false,
            tapped: None,
        }
    }

    /// The keyboard to draw on, with its device id; takes care of another keyboard (the old
    /// one's lease ends) and of the hand-back watch. `None` while there is none.
    fn keyboard(&mut self) -> Option<(Arc<digitalis::devices::DigitalisDevice>, String)> {
        self.take_arrival();
        if self.next_look.is_some_and(|t| Instant::now() < t) {
            return None;
        }
        let Some(kb) = super::devices::device() else {
            self.next_look = Some(Instant::now() + LOOK_AGAIN);
            return None;
        };
        self.next_look = None;
        use foxglove_devices::Device as _;
        let device = kb.info().id.to_string();
        if self.device.as_ref().is_some_and(|d| *d != device) {
            // Another keyboard: the old one's lease ends, its lights come back.
            super::api::end_leases(|l| l.conn == SCENE_CONN);
            self.lease = KeyboardLease::default();
            self.effects_failed = false;
        }
        self.device = Some(device.clone());
        self.tap(&kb);
        // The firmware may have changed under the same id (a flash): the scene follows it.
        // A map from "Find my LEDs" needs frames unless the firmware takes the LEDs' points
        // for its effects (EFFECT LAMPS, #550; led_map::effects_place_leds).
        let effects = kb.effects()
            && !self.effects_failed
            && super::led_map::effects_place_leds(kb.effect_lamps());
        if self.effects.swap(effects, Ordering::Relaxed) != effects {
            self.again.store(true, Ordering::Relaxed);
        }
        Some((kb, device))
    }

    /// A keyboard connected ([`keyboard_connected`]): look now, and one whose effects failed
    /// gets them again.
    fn take_arrival(&mut self) {
        if ARRIVED.swap(false, Ordering::Relaxed) {
            self.next_look = None;
            self.effects_failed = false;
            self.lease.lamps_sent = None; // it may have rebooted: its LED points again
        }
    }

    /// Something of this scene now shows on the keyboard: an effect a quit left there is gone
    /// (replaced), so its mark goes.
    fn drew(&mut self) {
        if std::mem::replace(&mut self.left_checked, true) {
            return;
        }
        let k = keep();
        if k.left_running {
            save_keep(KeyboardKeep {
                left_running: false,
                ..k
            });
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

/// Whether a scene's end leaves its effect on the keyboard: only on quit, with "Keep running
/// on the keyboard" ticked, and while the effect is what the keyboard shows (not frames, not
/// handed back for away, pause or a notification).
fn keeps_on_quit(quitting: bool, keep: KeyboardKeep, effect_on: bool) -> bool {
    quitting && keep.on_quit && effect_on
}

/// How long a missing keyboard is left before frames look for it again.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

impl KeyboardLights for AppKeyboard {
    fn show(&mut self, f: &KeyboardFrame) {
        let Some((kb, device)) = self.keyboard() else {
            return;
        };
        use foxglove_devices::Device as _;
        let Some(light) = kb.lighting() else {
            return;
        };
        let again = self.again.clone();
        let shown = super::lighting_leases::with(|a| self.lease.show(a, &device, light, f, &again));
        match shown {
            Ok(drawn) => {
                status().keyboard_frames += 1;
                self.error_logged = false;
                if drawn {
                    self.drew();
                }
            }
            Err(e) if !std::mem::replace(&mut self.error_logged, true) => {
                eprintln!("Foxglove: scene colour to the keyboard: {e}");
            }
            Err(_) => {}
        }
    }

    fn effect(&mut self, e: &KeyboardEffect) {
        let Some((kb, device)) = self.keyboard() else {
            return;
        };
        if !self.effects.load(Ordering::Relaxed) {
            // Not this keyboard (keyboard() asked for frames instead).
            return;
        }
        use foxglove_devices::Device as _;
        let Some(light) = kb.lighting() else {
            return;
        };
        let again = self.again.clone();
        let clock = self.clock;
        let sent = super::lighting_leases::with(|a| {
            self.lease
                .show_effect(a, &device, light, &*kb, e, &clock, &again)
        });
        match sent {
            Ok(EffectSent::Done) => {
                status().keyboard_effects += 1;
                self.error_logged = false;
                self.drew();
            }
            Ok(EffectSent::Waiting) => {}
            Ok(EffectSent::Frames) => {
                self.effects_failed = true;
                self.effects.store(false, Ordering::Relaxed);
                self.again.store(true, Ordering::Relaxed);
            }
            Err(err) => {
                if !std::mem::replace(&mut self.error_logged, true) {
                    eprintln!("Foxglove: scene effect to the keyboard: {err}");
                }
                // Tried again on the keyboard's next turn (bounded by FRAME_ERRORS).
                self.again.store(true, Ordering::Relaxed);
            }
        }
    }

    fn release_frames(&mut self) {
        self.lease.effect_on = false;
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
        // Quitting with "Keep running on the keyboard": an effect showing stays. The arbiter
        // forgets it, so ending the lease restores the lights under it (hidden while it runs)
        // without handing it back; the mark hands it back when Foxglove next finds the keyboard.
        if let Some(device) = &self.device {
            let k = keep();
            let lease = self.lease.id;
            // Not under a notification or pause, which handed it back already.
            let showing = self.lease.effect_on
                && super::lighting_leases::with(|a| {
                    a.holds(device) && lease.is_some_and(|id| a.shows(id))
                });
            if keeps_on_quit(QUITTING.load(Ordering::Relaxed), k, showing) {
                super::lighting_leases::with(|a| a.no_frames(device));
                save_keep(KeyboardKeep {
                    left_running: true,
                    ..k
                });
                eprintln!("Foxglove: quitting; the scene's effect keeps running on the keyboard");
            }
        }
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
        /// It draws SET_EFFECT effects (HELLO bit 19).
        effects: bool,
        /// The effect running, if any (a frame or a release ends it).
        effect: Mutex<Option<digitalis::protocol::Effect>>,
        /// Every PLACE and EFFECT, in order ('P', 'E').
        effect_ops: Mutex<Vec<char>>,
        /// Effects to fail (no answer) before taking them.
        effect_failing: AtomicU32,
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
                effects: false,
                effect: Mutex::new(None),
                effect_ops: Mutex::new(Vec::new()),
                effect_failing: AtomicU32::new(0),
            }
        }

        fn with_effects() -> Fake {
            Fake {
                frames: true,
                effects: true,
                ..Fake::new()
            }
        }

        fn running(&self) -> Option<digitalis::protocol::Effect> {
            self.effect.lock().unwrap().clone()
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

    /// The test keyboard's LEDs per half (its frame is two of them).
    const HALF: usize = 21;

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
            // Frames draw over an effect: SET_FRAME ends it (#541).
            *self.effect.lock().unwrap() = None;
            Ok(())
        }
        fn release(&self) -> foxglove_devices::Result<()> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            *self.frame.lock().unwrap() = None;
            // DigitalisDevice's release ends an effect too.
            *self.effect.lock().unwrap() = None;
            Ok(())
        }
    }

    impl EffectTarget for Fake {
        fn place(&self, _: MmRect, _: MmRect, _: MmRect) -> Result<(), DeviceError> {
            if !self.effects {
                return Err(DeviceError::Unsupported);
            }
            self.effect_ops.lock().unwrap().push('P');
            Ok(())
        }
        fn run(&self, e: &digitalis::protocol::Effect) -> Result<(), DeviceError> {
            if !self.effects {
                return Err(DeviceError::Unsupported);
            }
            if self
                .effect_failing
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(DeviceError::Timeout);
            }
            self.effect_ops.lock().unwrap().push('E');
            *self.frame.lock().unwrap() = None;
            *self.effect.lock().unwrap() = Some(e.clone());
            Ok(())
        }
        fn lamps(&self, half: u8, points: &[[u16; 2]]) -> Result<(), DeviceError> {
            if !self.effects {
                return Err(DeviceError::Unsupported);
            }
            if !points.is_empty() {
                self.effect_ops
                    .lock()
                    .unwrap()
                    .push(if half == 0 { 'l' } else { 'r' });
            }
            Ok(())
        }
    }

    /// A 4 s scanline on both halves, as `scene_output::keyboard_effect` makes it.
    fn scan_effect(mix: Hsb) -> KeyboardEffect {
        KeyboardEffect {
            effect: digitalis::protocol::Effect {
                kind: digitalis::protocol::EffectKind::Scanline,
                colours: vec![RED, BLUE],
                clock_ms: 0,
                period_ms: 4000,
                angle_deg: 90.0,
                width_mm: 80.0,
                origin_mm: [0, 0],
            },
            room: [0, 0, 600, 700],
            parts: vec![[100, 500, 124, 86], [304, 500, 124, 86]],
            mix,
            lamps: None,
        }
    }

    /// The SET_EFFECT path (#542) through the lease contract: sent once with its clock from the
    /// scene's epoch, handed back under a notification, pause and away, sent again when the
    /// lease shows again, and released at the end.
    #[test]
    fn an_effect_runs_on_the_keyboard_and_gives_way_like_frames() {
        let kb = Fake::with_effects();
        let before = kb.hsb();
        let again = Arc::new(AtomicBool::new(false));
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        // A clock whose epoch was 10.5 s ago: the effect starts 2.5 s into its 4 s period.
        let clock = Clock::aligned(foxglove_scene::clock::unix_ms_now() - 10_500);
        let e = scan_effect(c(30));
        let sent = scene
            .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
            .unwrap();
        assert_eq!(sent, EffectSent::Done);
        assert_eq!(
            *kb.effect_ops.lock().unwrap(),
            ['P', 'E'],
            "placed, then the effect"
        );
        // With the user's LED map (#550), both halves' points go first, every time.
        let mut mapped = scan_effect(c(30));
        mapped.lamps = Some(vec![vec![[100, 200]; HALF], vec![[900, 200]; HALF]]);
        kb.effect_ops.lock().unwrap().clear();
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &mapped, &clock, &again)
            .unwrap();
        assert_eq!(*kb.effect_ops.lock().unwrap(), ['l', 'r', 'P', 'E']);
        // The same points again: only PLACE and the effect.
        kb.effect_ops.lock().unwrap().clear();
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &mapped, &clock, &again)
            .unwrap();
        assert_eq!(*kb.effect_ops.lock().unwrap(), ['P', 'E']);
        kb.effect_ops.lock().unwrap().clear();
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
            .unwrap();
        let running = kb.running().expect("the effect runs");
        assert!(
            (2500..2700).contains(&running.clock_ms),
            "on the scene's clock: {}",
            running.clock_ms
        );
        assert_eq!(
            kb.hsb(),
            (true, 30, 100, 90, "0".into()),
            "the lease holds the mix"
        );
        assert!(scene.effect_on);

        assert!(a.holds("kb"));
        // A notification: the effect is handed back before its colour shows.
        let mute = a.add(mute_lease(), &kb).unwrap();
        assert!(!a.holds("kb"), "nothing of the scene's left to keep");
        assert_eq!(Fake::count(&kb.releases), 1);
        assert_eq!(kb.running(), None);
        assert_eq!(kb.hsb().1, 0, "the mute's red, visible");
        // Sent again while the notification shows: nothing goes out.
        let ops = kb.effect_ops.lock().unwrap().len();
        let sent = scene
            .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
            .unwrap();
        assert_eq!(
            sent,
            EffectSent::Waiting,
            "nothing drawn: a quit's mark stays"
        );
        assert_eq!(kb.effect_ops.lock().unwrap().len(), ops);
        assert!(!scene.effect_on);
        // Unmuted: the scene is asked for its effect again.
        for d in a.end(|l| l.id == mute) {
            a.apply(&d, &kb).unwrap();
        }
        assert!(again.swap(false, Ordering::SeqCst), "send it again");
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
            .unwrap();
        assert!(kb.running().is_some());

        // Paused: released, the keyboard's own lights; resumed: asked again.
        for d in a.set_paused(true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(Fake::count(&kb.releases), 2);
        assert_eq!((kb.running(), kb.hsb()), (None, before.clone()));
        for d in a.set_paused(false) {
            a.apply(&d, &kb).unwrap();
        }
        assert!(again.swap(false, Ordering::SeqCst));
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
            .unwrap();

        // Away: released with the lease kept; its colour shows.
        a.release_frames("kb", &kb);
        assert_eq!(Fake::count(&kb.releases), 3);
        assert_eq!((kb.running(), kb.hsb().1), (None, 30));

        // A ripple over the scene goes as frames, which take over from the effect.
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
            .unwrap();
        scene
            .show(&mut a, "kb", &kb, &halves(RED, BLUE, c(30)), &again)
            .unwrap();
        assert_eq!(kb.running(), None);
        assert_eq!(kb.showing(), Some((RED, BLUE)));
        assert!(!scene.effect_on);

        // The end (Stop): released, then the lights from before.
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
            .unwrap();
        for d in a.end(|l| l.conn == SCENE_CONN) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!((kb.running(), kb.hsb()), (None, before));
    }

    /// A keyboard that connects is looked for at once, even inside the wait after it was
    /// missing, and gets effects again after they failed.
    #[test]
    fn a_connect_ends_the_wait_for_a_missing_keyboard() {
        let mut kb = AppKeyboard::new(Arc::default(), Arc::default(), Clock::starting_now());
        kb.next_look = Some(Instant::now() + Duration::from_secs(60));
        kb.effects_failed = true;
        kb.take_arrival();
        assert!(kb.next_look.is_some(), "no connect: the wait stays");
        ARRIVED.store(true, Ordering::Relaxed);
        kb.take_arrival();
        assert_eq!((kb.next_look, kb.effects_failed), (None, false));
        assert!(!ARRIVED.load(Ordering::Relaxed));
    }

    /// Quitting with "Keep running on the keyboard" leaves the effect: the arbiter forgets it,
    /// so the lease's end restores the lights under it without a release.
    #[test]
    fn keep_on_quit_leaves_the_effect_and_only_then() {
        let off = KeyboardKeep::default();
        let on = KeyboardKeep {
            on_quit: true,
            ..off
        };
        assert!(!keeps_on_quit(true, off, true), "off by default");
        assert!(keeps_on_quit(true, on, true));
        assert!(!keeps_on_quit(false, on, true), "Stop always releases");
        assert!(
            !keeps_on_quit(true, on, false),
            "frames or handed back: nothing to keep"
        );

        let kb = Fake::with_effects();
        let before = kb.hsb();
        let again = Arc::new(AtomicBool::new(false));
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        let clock = Clock::starting_now();
        scene
            .show_effect(&mut a, "kb", &kb, &kb, &scan_effect(c(30)), &clock, &again)
            .unwrap();
        // What give_back does on a keeping quit.
        a.no_frames("kb");
        for d in a.end(|l| l.conn == SCENE_CONN) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(Fake::count(&kb.releases), 0, "not released");
        assert!(kb.running().is_some(), "still running");
        assert_eq!(kb.hsb(), before, "the lights under it are the user's again");
    }

    #[test]
    fn effects_fall_back_to_frames_when_refused_or_failing() {
        // Refused (firmware without bit 19): frames from now on, nothing held.
        let kb = Fake::with_frames();
        let again = Arc::new(AtomicBool::new(false));
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        let clock = Clock::starting_now();
        let e = scan_effect(c(30));
        assert_eq!(
            scene
                .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
                .unwrap(),
            EffectSent::Frames
        );
        assert_eq!(Fake::count(&kb.releases), 0);
        // Failing: errors, then frames after FRAME_ERRORS in a row.
        let kb = Fake::with_effects();
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        kb.effect_failing.store(FRAME_ERRORS, Ordering::SeqCst);
        for i in 1..FRAME_ERRORS {
            let r = scene.show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again);
            assert!(r.is_err(), "effect {i} failed");
        }
        assert_eq!(
            scene
                .show_effect(&mut a, "kb", &kb, &kb, &e, &clock, &again)
                .unwrap(),
            EffectSent::Frames
        );
        // A failed send may have run: the end still hands it back.
        for d in a.end(|l| l.conn == SCENE_CONN) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(Fake::count(&kb.releases), 1);
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
        mb.post(Draw::Frame(one(c(1))));
        mb.release();
        assert_eq!(
            mb.next(),
            Some(Job::Release),
            "away drops the frame waiting"
        );
        mb.release();
        mb.post(Draw::Frame(one(c(2))));
        assert_eq!(mb.next(), Some(Job::Release));
        assert_eq!(mb.next(), Some(Job::Show(Draw::Frame(one(c(2))))));
        // Newest wins between a frame and an effect too.
        mb.post(Draw::Frame(one(c(4))));
        mb.post(Draw::Effect(scan_effect(c(5))));
        assert_eq!(mb.next(), Some(Job::Show(Draw::Effect(scan_effect(c(5))))));
        mb.post(Draw::Frame(one(c(3))));
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
        fn effect(&mut self, e: &KeyboardEffect) {
            self.0.lock().unwrap().push(Some(e.mix));
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
        fn effect(&mut self, _: &KeyboardEffect) {}
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
        out.mailbox.post(Draw::Frame(one(c(1))));
        // Wait (bounded) until the first is being written, then post a burst.
        let until = Instant::now() + Duration::from_secs(5);
        while seen.lock().unwrap().is_empty() && Instant::now() < until {
            std::thread::yield_now();
        }
        for h in 2..=9 {
            out.mailbox.post(Draw::Frame(one(c(h))));
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
