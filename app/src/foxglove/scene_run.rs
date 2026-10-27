//! A running scene (#485): the room map, the shared clock and one effect, sent to the OMEN case
//! and the keyboard while the scheduler's gates are open: a scene runs, someone is present
//! (`away`) and Foxglove isn't paused (`pause`).
//!
//! Who holds the lights:
//! - The keyboard: the scene takes a low-priority lighting lease ([`SCENE_CONN`]), so a
//!   plugin's notification (a mic-mute lease) shows over it and the scene comes back after,
//!   pause gives the keyboard its own lights, and the scene's end restores the lights from
//!   before it (the lease arbiter's snapshot).
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

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use foxglove_devices::{DeviceError, Lighting};
use foxglove_scene::scheduler::{Gate, Scheduler, MIN_INTERVAL};
use foxglove_scene::{Clock, Effect, Rgb};

use super::lighting_leases::{Arbiter, Lease, Priority};
use super::scene_output::{CaseTarget, Hsb, Out, Outputs};

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

/// What diagnostics says about scenes.
#[derive(Clone, Debug, Default)]
struct Status {
    name: Option<String>,
    started: Option<Instant>,
    case_frames: u64,
    keyboard_frames: u64,
    last: Option<String>,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    name: None,
    started: None,
    case_frames: 0,
    keyboard_frames: 0,
    last: None,
});

fn status() -> std::sync::MutexGuard<'static, Status> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// One line for Copy diagnostics.
pub fn summary() -> String {
    let s = status().clone();
    let sent = format!(
        "{} case frames, {} keyboard colours",
        s.case_frames, s.keyboard_frames
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
    let keyboard = KeyboardOut::spawn(Box::new(AppKeyboard::default()));
    let (room, cases) = gather();
    eprintln!("Foxglove: scene {name} started; room: {}", room.summary());
    super::wallpaper_sync::scene_started(name, &effect, clock, room.clone());
    // The room as last placed, for the wallpapers when the look changes.
    let mut placed = room.clone();
    let outputs = Arc::new(Mutex::new(Outputs::new(effect, room, cases)));
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
        Ctl::Away(a) => scheduler.set(Gate::Away, a),
        Ctl::Paused(p) => scheduler.set(Gate::Paused, p),
        Ctl::Devices => {
            let (room, cases) = gather();
            super::wallpaper_sync::scene_room(room.clone());
            placed = room.clone();
            outputs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_room(room, cases);
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
            Out::Keyboard(c) => keyboard.post(c),
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

/// The keyboard's newest colour, waiting to be written: one in flight, newest wins.
#[derive(Default)]
struct Mailbox {
    slot: Mutex<(Option<Hsb>, bool)>,
    wake: Condvar,
}

impl Mailbox {
    fn post(&self, c: Hsb) {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).0 = Some(c);
        self.wake.notify_one();
    }

    fn close(&self) {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        self.wake.notify_one();
    }

    /// The next colour, or `None` once closed (a colour waiting then is dropped).
    fn next(&self) -> Option<Hsb> {
        let guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let mut guard = self
            .wake
            .wait_while(guard, |(c, closed)| c.is_none() && !*closed)
            .unwrap_or_else(|e| e.into_inner());
        if guard.1 {
            return None;
        }
        guard.0.take()
    }
}

/// Where the keyboard's colours go: the app's lease-held keyboard, or a fake in tests.
trait KeyboardLights: Send {
    fn show(&mut self, c: Hsb);
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
            while let Some(c) = mb.next() {
                lights.show(c);
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
}

impl KeyboardLease {
    /// The first colour takes the lights (a low lease: its snapshot, on, solid); later ones
    /// change the lease in place and, while it is the one showing and Foxglove isn't paused,
    /// send only the colour (no readback: a frame every 100 ms).
    fn show(
        &mut self,
        a: &mut Arbiter,
        device: &str,
        light: &dyn Lighting,
        c: Hsb,
    ) -> Result<(), DeviceError> {
        if let Some(id) = self.id {
            match a.update(id, SCENE_CONN, c.hue, c.sat, c.brightness, "0") {
                Ok(Some(_)) => return light.set_color(c.hue, c.sat, c.brightness),
                Ok(None) => return Ok(()),
                // Gone (the keyboard was another one): take it again.
                Err(_) => self.id = None,
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
        self.id = Some(a.add(lease, light)?);
        Ok(())
    }
}

/// The host-link keyboard under the scene's lease.
#[derive(Default)]
struct AppKeyboard {
    lease: KeyboardLease,
    /// No keyboard was found: don't look again before this (frames keep coming).
    next_look: Option<Instant>,
    device: Option<String>,
    error_logged: bool,
}

/// How long a missing keyboard is left before frames look for it again.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

impl KeyboardLights for AppKeyboard {
    fn show(&mut self, c: Hsb) {
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
        let shown = super::lighting_leases::with(|a| self.lease.show(a, &device, light, c));
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

    fn give_back(&mut self) {
        if self.device.is_some() {
            super::api::end_leases(|l| l.conn == SCENE_CONN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_devices::{Effect as LightEffect, LightingState, Zone};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A keyboard's lights in memory, counting colour writes.
    struct Fake {
        state: Mutex<LightingState>,
        colours: AtomicU32,
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
            }
        }

        fn hsb(&self) -> (bool, u16, u8, u8, String) {
            let s = self.state.lock().unwrap();
            (s.on, s.hue, s.sat, s.brightness, s.effect.id.clone())
        }
    }

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
    }

    fn c(hue: u16) -> Hsb {
        Hsb {
            hue,
            sat: 100,
            brightness: 90,
        }
    }

    #[test]
    fn the_scene_takes_the_keyboard_a_notification_wins_and_the_end_restores() {
        let kb = Fake::new();
        let before = kb.hsb();
        let mut a = Arbiter::default();
        let mut scene = KeyboardLease::default();
        scene.show(&mut a, "kb", &kb, c(30)).unwrap();
        assert_eq!(
            kb.hsb(),
            (true, 30, 100, 90, "0".into()),
            "on, solid, the frame"
        );
        let writes = kb.colours.load(Ordering::SeqCst);
        scene.show(&mut a, "kb", &kb, c(40)).unwrap();
        assert_eq!(
            kb.colours.load(Ordering::SeqCst),
            writes + 1,
            "a frame is one colour write"
        );
        // A plugin's mute lease (high) shows over the scene; frames meanwhile write nothing.
        let mute = Lease {
            id: 0,
            conn: 7,
            device: "kb".into(),
            priority: Priority::High,
            hue: 0,
            sat: 100,
            brightness: 100,
            effect: "0".into(),
        };
        let mute = a.add(mute, &kb).unwrap();
        let writes = kb.colours.load(Ordering::SeqCst);
        scene.show(&mut a, "kb", &kb, c(50)).unwrap();
        assert_eq!(kb.colours.load(Ordering::SeqCst), writes);
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
        scene.show(&mut a, "kb", &kb, c(60)).unwrap();
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
        scene.show(&mut a, "kb", &kb, c(70)).unwrap();
        assert_eq!(kb.hsb().1, 70);
    }

    /// Records what the writer thread was given.
    struct Recorded(Arc<Mutex<Vec<Option<Hsb>>>>);

    impl KeyboardLights for Recorded {
        fn show(&mut self, c: Hsb) {
            self.0.lock().unwrap().push(Some(c));
            // A slow keyboard: colours posted meanwhile collapse to the newest.
            std::thread::sleep(Duration::from_millis(30));
        }
        fn give_back(&mut self) {
            self.0.lock().unwrap().push(None);
        }
    }

    #[test]
    fn a_busy_keyboard_gets_the_newest_colour_and_the_end_gives_back() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let out = KeyboardOut::spawn(Box::new(Recorded(seen.clone())));
        out.mailbox.post(c(1));
        // Wait (bounded) until the first is being written, then post a burst.
        let until = Instant::now() + Duration::from_secs(5);
        while seen.lock().unwrap().is_empty() && Instant::now() < until {
            std::thread::yield_now();
        }
        for h in 2..=9 {
            out.mailbox.post(c(h));
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
