//! The Keyboard card's effect while Windows Dynamic Lighting holds the keyboard (#768): Breathe,
//! Spectrum and Swirl run as a scene look through the scene engine (its renderer, its tick and
//! its LampArray writer), drawn on the keyboard's own LampArray. The keyboard firmware draws
//! these effects itself over its link; while Windows holds its lights that link shows nothing,
//! so Foxglove draws them through Windows instead, on the scene clock.
//!
//! - Only the keyboard: the card's room is the keyboard's parts from the room map, over their
//!   own bounds ([`card_room`]); no case, other LampArray, wallpaper or right-screen art takes
//!   part, and the host link gets nothing (the output's "held" flag stays set).
//! - The look ([`scene_look`]): Breathe is the chosen colour fading to dark and back; Spectrum
//!   is the hue wheel turning on every lamp together; Swirl the hue wheel across the keyboard,
//!   or, on firmware that draws Swirl in the chosen colour (RGB_STATE byte 29, #626), the
//!   firmware's band of hues around it. Each at about the firmware's pace for the speed
//!   ([`period_ms`]). Solid has nothing to animate: it stays the Lights worker's steady colour
//!   (#764), as lights off does.
//! - The keyboard's colour calibration and lamp places are the scene path's (#765,
//!   `scene_run::keyboard_lamps`), so the card's effect and a scene's draw the same lamps alike.
//! - A running scene comes first: it already draws the held keyboard through Windows (#761).
//!   Its start stops the card's look ([`scene_starting`]), and its end brings it back
//!   ([`scene_ended`]) while Windows still holds the keyboard.
//! - Pause and away close the run's gates and let the LampArray go, as a scene's do; resuming
//!   or coming back draws again. Windows letting go of the keyboard ([`release`], from the
//!   owner report) ends it: the keyboard's own link has the pick, since every pick still goes
//!   over it.
//! - No new timer: the run is a scene scheduler (frames only while its gates are open) and the
//!   scene LampArray writer; both end with it. Nothing exists while the card shows no effect.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use foxglove_scene::scheduler::{Gate, Scheduler, MIN_INTERVAL};
use foxglove_scene::{Clock, Effect, Rect, Rgb, RoomMap};

use super::pc_lights::{hsv_rgb, KeyboardLights};
use super::scene_lamps::{LampWriter, Opener};
use super::scene_output::{KeyboardLamps, Out, Outputs, KEYBOARD};

/// The firmware's colour Swirl: degrees of hue either side of the chosen colour
/// (`CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND`'s default).
const SWIRL_BAND: u16 = 30;

/// The firmware's colour Swirl: how much dimmer the wave's trough is, in percent
/// (`CONFIG_FOXGLOVE_COLOUR_SWIRL_DEPTH`'s default).
const SWIRL_DEPTH: u8 = 40;

/// ZMK's underglow tick: each effect steps once per this many milliseconds.
const ZMK_TICK_MS: u32 = 50;

/// The Keyboard card's pick as the scene engine draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardLook {
    pub lights: KeyboardLights,
    /// 1-5, as the keyboard reports it.
    pub speed: u8,
    /// The keyboard draws this effect in the chosen colour (RGB_STATE byte 29): Swirl as a
    /// band of hues around it rather than the whole wheel.
    pub in_colour: bool,
}

/// How the look spreads over the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spread {
    /// Every lamp shows the same colour at once (Breathe, Spectrum).
    Together,
    /// The colours run across the keyboard, left to right (Swirl).
    Across,
}

/// The card's look as a scene look.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneLook {
    pub effect: Effect,
    pub spread: Spread,
}

/// One turn of the effect at `speed` (1-5; anything else is taken as 3), in milliseconds:
/// about the keyboard's own pace. ZMK steps every 50 ms: Spectrum turns the hue by the speed in
/// degrees, Swirl by twice that, and Breathe runs a 2400-step cycle by ten times the speed.
pub fn period_ms(effect: u8, speed: u8) -> u32 {
    let speed = u32::from(if (1..=5).contains(&speed) { speed } else { 3 });
    let steps = match effect {
        1 => 2400 / 10,
        2 => 360,
        3 => 360 / 2,
        _ => return 0,
    };
    steps * ZMK_TICK_MS / speed
}

/// The card's pick as a scene look, or `None` when it has nothing to animate: Solid (the
/// Lights worker shows it steady) or an effect Foxglove doesn't number.
pub fn scene_look(look: &CardLook) -> Option<SceneLook> {
    let l = &look.lights;
    let colour = |hue: u16, value: u8| Rgb(hsv_rgb(hue % 360, l.sat, value));
    let wheel = || -> Vec<Rgb> { (0..6).map(|i| colour(i * 60, l.brightness)).collect() };
    // The firmware's colour band (colour_effects.c): the chosen hue less the band at the
    // trough (dimmer by the depth), more at the crest, back and forth along the path.
    let band = || -> Vec<Rgb> {
        let h = l.hue % 360 + 360;
        let b = u16::from(l.brightness);
        let level = |pct: u16| (b * pct / 100) as u8;
        let trough = 100 - u16::from(SWIRL_DEPTH);
        let middle = 100 - u16::from(SWIRL_DEPTH) / 2;
        vec![
            colour(h - SWIRL_BAND, level(trough)),
            colour(h, level(middle)),
            colour(h + SWIRL_BAND, l.brightness),
            colour(h, level(middle)),
        ]
    };
    let period_ms = period_ms(l.effect, look.speed);
    let sweep = |stops: Vec<Rgb>, spread| SceneLook {
        effect: Effect::Sweep {
            stops,
            angle_deg: 0.0,
            period_ms,
        },
        spread,
    };
    Some(match l.effect {
        1 => SceneLook {
            effect: Effect::Breathe {
                from: colour(l.hue, l.brightness),
                to: Rgb::BLACK,
                period_ms,
            },
            spread: Spread::Together,
        },
        2 if look.in_colour => sweep(band(), Spread::Together),
        2 => sweep(wheel(), Spread::Together),
        3 if look.in_colour => sweep(band(), Spread::Across),
        3 => sweep(wheel(), Spread::Across),
        _ => return None,
    })
}

/// The card's room: the keyboard's parts out of `room`, over their own bounds, so a Swirl runs
/// across the keyboard alone. [`Spread::Together`] gives the bounds no width: a sweep then
/// stands at one point of its palette on every lamp (`along` of a room with no length is 0).
/// `None` without the keyboard in the room.
pub fn card_room(room: &RoomMap, spread: Spread) -> Option<RoomMap> {
    let devices: Vec<_> = room
        .devices
        .iter()
        .filter(|d| d.id.starts_with(KEYBOARD) && !d.lights.is_empty())
        .cloned()
        .collect();
    let (first, rest) = devices.split_first()?;
    let r = rest.iter().fold(first.rect, |r, d| r.union(&d.rect));
    let bounds = match spread {
        Spread::Across => r,
        Spread::Together => Rect::new(r.x, r.y, 0.0, r.h),
    };
    Some(RoomMap { devices, bounds })
}

/// What the run hears.
#[derive(Debug)]
enum Ctl {
    Away(bool),
    Paused(bool),
    /// The lighting devices changed, or a keyboard connected: place and pair it again.
    Devices,
    Look(Box<SceneLook>),
    Stop,
}

/// What the run reads from the app: the room, the keyboard's own LampArray and the LampArray
/// opener. Fakes in tests.
pub struct Env {
    pub room: Box<dyn Fn() -> RoomMap + Send>,
    pub lamps: Box<dyn Fn() -> Option<KeyboardLamps> + Send>,
    pub opener: Box<dyn Opener>,
    pub status: super::scene_lamps::Status,
    /// Windows changed whether Foxglove may drive the keyboard's LampArray.
    pub notify: Option<super::scene_lamps::Notify>,
}

impl Env {
    /// The app's: the room map, the paired LampArray with the keyboard's calibration, Windows.
    fn app() -> Env {
        Env {
            room: Box::new(super::scene_room::room),
            lamps: Box::new(super::scene_run::keyboard_lamps),
            opener: Box::<super::scene_lamps::WindowsLamps>::default(),
            status: LAMPS.clone(),
            notify: Some(Arc::new(|_: &str| {
                super::lights_owner::retry("Windows changed the Keyboard card's LampArray");
            })),
        }
    }
}

/// The card's LampArray writes, for diagnostics.
static LAMPS: std::sync::LazyLock<super::scene_lamps::Status> =
    std::sync::LazyLock::new(super::scene_lamps::Status::default);

/// One run of the card's look, start to end, on its own thread: the scene output for the
/// keyboard alone, the scene scheduler for its frames and the scene LampArray writer.
fn control(look: SceneLook, env: Env, own: &mpsc::Sender<Ctl>, rx: &mpsc::Receiver<Ctl>) {
    let place = |look: &SceneLook| card_room(&(env.room)(), look.spread).unwrap_or_default();
    let mut outputs = Outputs::new(look.effect.clone(), place(&look), Vec::new());
    outputs.set_keyboard_lamps((env.lamps)());
    // Windows holds the keyboard: nothing on its host link, its LampArray only.
    outputs.keyboard_held().store(true, Ordering::Relaxed);
    let outputs = Arc::new(Mutex::new(outputs));
    fn lock(o: &Mutex<Outputs>) -> std::sync::MutexGuard<'_, Outputs> {
        o.lock().unwrap_or_else(|e| e.into_inner())
    }
    let writer = LampWriter::spawn(env.opener, env.status, env.notify);
    let poster = writer.poster();
    let frames = outputs.clone();
    let scheduler = Scheduler::new(Clock::starting_now(), MIN_INTERVAL, move |t| {
        let outs = frames.lock().unwrap_or_else(|e| e.into_inner()).frame(t);
        for out in outs {
            if let Out::Lamps { key, colors } = out {
                poster.post(key, colors);
            }
        }
    });
    let away_tx = own.clone();
    let away = super::away::subscribe(move |a| {
        let _ = away_tx.send(Ctl::Away(a));
    });
    scheduler.set(Gate::Away, super::away::away());
    scheduler.set(Gate::Paused, super::pause::paused());
    scheduler.set(Gate::Scene, true);
    let mut spread = look.spread;
    eprintln!("Foxglove: the Keyboard card's effect runs through Windows Dynamic Lighting");
    while let Ok(ctl) = rx.recv() {
        match ctl {
            Ctl::Away(gone) | Ctl::Paused(gone) => {
                let gate = if matches!(ctl, Ctl::Away(_)) {
                    Gate::Away
                } else {
                    Gate::Paused
                };
                if !gone {
                    // Let go meanwhile: the first frame back draws it in full.
                    lock(&outputs).redraw_lamps();
                }
                scheduler.set(gate, gone);
                if gone {
                    // Windows' own lights while paused or away.
                    writer.release();
                }
            }
            Ctl::Devices => {
                writer.devices_changed();
                let room = card_room(&(env.room)(), spread).unwrap_or_default();
                let mut o = lock(&outputs);
                o.set_room(room, Vec::new());
                o.set_keyboard_lamps((env.lamps)());
            }
            Ctl::Look(look) => {
                let mut o = lock(&outputs);
                if look.spread != spread {
                    spread = look.spread;
                    o.set_room(place(&look), Vec::new());
                }
                o.set_effect(look.effect);
            }
            Ctl::Stop => break,
        }
    }
    drop(away);
    // Joins the frame thread: nothing draws after this.
    drop(scheduler);
    // Lets the LampArray go.
    drop(writer);
    eprintln!("Foxglove: the Keyboard card's effect through Windows ended");
}

/// What the card's look does on each event: pure, so the rules are tested without threads.
#[derive(Debug, Default)]
struct Plan {
    /// The look the card asks for while Windows holds the keyboard.
    wanted: Option<SceneLook>,
    /// Its run is going.
    running: bool,
}

#[derive(Debug, PartialEq)]
enum Step {
    Nothing,
    Start(SceneLook),
    Change(SceneLook),
    /// Stop the run; `wait` until its LampArray is let go.
    Stop {
        wait: bool,
    },
}

impl Plan {
    /// The card picked `look` (`None`: nothing to animate) while Windows holds the keyboard;
    /// `scene` says a scene runs, which comes first.
    fn show(&mut self, look: Option<SceneLook>, scene: bool) -> Step {
        self.wanted = look.clone();
        match (look, self.running) {
            // A steady pick follows at once: wait for the effect's last frame to go.
            (None, true) => self.stop(true),
            (None, false) => Step::Nothing,
            (Some(_), _) if scene => Step::Nothing,
            (Some(l), true) => Step::Change(l),
            (Some(l), false) => {
                self.running = true;
                Step::Start(l)
            }
        }
    }

    fn stop(&mut self, wait: bool) -> Step {
        if !std::mem::take(&mut self.running) {
            return Step::Nothing;
        }
        Step::Stop { wait }
    }

    /// A scene starts: it draws the keyboard, the card's look waits for its end.
    fn scene_starting(&mut self) -> Step {
        self.stop(true)
    }

    /// A scene ended: the card's look again, unless Foxglove quits.
    fn scene_ended(&mut self, quitting: bool) -> Step {
        match &self.wanted {
            Some(l) if !quitting && !self.running => {
                self.running = true;
                Step::Start(l.clone())
            }
            _ => Step::Nothing,
        }
    }

    /// Windows let go of the keyboard (the link has the pick), or Foxglove quits (`wait`).
    fn release(&mut self, wait: bool) -> Step {
        self.wanted = None;
        self.stop(wait)
    }

    /// The run ended by itself (it can't, short of a panic): start afresh next time.
    fn ended(&mut self) {
        self.running = false;
    }
}

struct Run {
    tx: mpsc::Sender<Ctl>,
    thread: JoinHandle<()>,
}

struct Card {
    plan: Plan,
    run: Option<Run>,
}

static CARD: Mutex<Card> = Mutex::new(Card {
    plan: Plan {
        wanted: None,
        running: false,
    },
    run: None,
});

fn card() -> std::sync::MutexGuard<'static, Card> {
    CARD.lock().unwrap_or_else(|e| e.into_inner())
}

/// Carries out a step. A stop that waits joins the run outside the lock (bounded: the frame
/// thread and the LampArray writer end with it).
fn act(step: Step) {
    let mut c = card();
    if c.run.as_ref().is_some_and(|r| r.thread.is_finished()) {
        c.run = None;
        if matches!(step, Step::Change(_)) {
            c.plan.ended();
        }
    }
    match step {
        Step::Nothing => {}
        Step::Start(look) | Step::Change(look) if c.run.is_none() => {
            c.plan.running = true;
            let (tx, rx) = mpsc::channel();
            let own = tx.clone();
            let env = Env::app();
            let thread = super::services::spawn_named("keyboard-card-look", move || {
                control(look, env, &own, &rx)
            });
            c.run = Some(Run { tx, thread });
        }
        Step::Start(look) | Step::Change(look) => {
            if let Some(r) = &c.run {
                let _ = r.tx.send(Ctl::Look(Box::new(look)));
            }
        }
        Step::Stop { wait } => {
            let Some(r) = c.run.take() else {
                return;
            };
            let _ = r.tx.send(Ctl::Stop);
            drop(c);
            if wait {
                let _ = r.thread.join();
            }
        }
    }
}

/// The Keyboard card's pick while Windows Dynamic Lighting holds the keyboard (#768): true when
/// the scene engine draws it (an effect: now, or after a running scene), false when the Lights
/// worker's steady colour should (Solid, lights off, no keyboard in the room or no LampArray
/// paired). A steady pick stops the effect first, so its last frame can't land after the
/// colour. Nothing while paused: Foxglove leaves the lights alone then.
pub fn show(look: Option<CardLook>) -> bool {
    if super::pause::paused() {
        return false;
    }
    let look = look.as_ref().and_then(scene_look).filter(|l| {
        card_room(&super::scene_room::room(), l.spread).is_some()
            && super::scene_run::keyboard_lamps().is_some()
    });
    let taken = look.is_some();
    let scene = super::scene_run::running().is_some();
    let step = card().plan.show(look, scene);
    act(step);
    taken
}

/// A scene is starting: it draws the held keyboard, so the card's look stops (and waits).
pub fn scene_starting() {
    let step = card().plan.scene_starting();
    act(step);
}

/// A scene ended (`quitting`: because Foxglove quits): the card's look comes back while Windows
/// still holds the keyboard.
pub fn scene_ended(quitting: bool) {
    let step = card().plan.scene_ended(quitting);
    act(step);
}

/// Windows let go of the keyboard's lights (the owner report): the card's look ends without
/// waiting (this runs on the link's thread); the keyboard's own link has the pick.
pub fn release() {
    let step = card().plan.release(false);
    act(step);
}

/// Foxglove quits: the card's look ends and lets the LampArray go.
pub fn quit() {
    let step = card().plan.release(true);
    act(step);
}

fn tell(ctl: Ctl) {
    if let Some(r) = &card().run {
        let _ = r.tx.send(ctl);
    }
}

/// Foxglove paused or resumed.
pub fn pause_changed(paused: bool) {
    tell(Ctl::Paused(paused));
}

/// The lighting devices changed, the room moved or a keyboard connected.
pub fn devices_changed() {
    tell(Ctl::Devices);
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_scene::{DeviceInfo, DeviceKind, MonitorInfo, SceneTime};
    use std::time::Duration;

    fn look(effect: u8, in_colour: bool) -> CardLook {
        CardLook {
            lights: KeyboardLights {
                hue: 30,
                sat: 100,
                brightness: 80,
                effect,
            },
            speed: 3,
            in_colour,
        }
    }

    /// The desk: one monitor, both halves of a split keyboard and a case.
    fn room() -> RoomMap {
        let monitors = [MonitorInfo {
            id: r"\\.\DISPLAY1".into(),
            primary: true,
            rect_px: [0, 0, 2560, 1440],
            raw_dpi: Some(109),
            dpi: 96,
        }];
        let devices = [
            DeviceInfo::whole("keyboard:left", "Keyboard left", DeviceKind::KeyboardLeft),
            DeviceInfo::whole(
                "keyboard:right",
                "Keyboard right",
                DeviceKind::KeyboardRight,
            ),
            DeviceInfo::whole("light:103c:84fd", "Case", DeviceKind::Case),
        ];
        RoomMap::build(&monitors, &devices, &Default::default())
    }

    /// 42 lamps in a row across the keyboard, as Windows gives a split's LampArray.
    fn lamps() -> KeyboardLamps {
        let at: Vec<[f32; 2]> = (0..42).map(|i| [i as f32 * 7.0, 30.0]).collect();
        KeyboardLamps::new("1d50:615e".into(), Some(([294.0, 60.0], &at[..])))
    }

    /// The keyboard LampArray writes for `look` from `from` to `to` ms, as the run makes them.
    fn writes(look: &SceneLook, from: i64, to: i64) -> Vec<Vec<[u8; 3]>> {
        let mut o = Outputs::new(
            look.effect.clone(),
            card_room(&room(), look.spread).unwrap(),
            Vec::new(),
        );
        o.set_keyboard_lamps(Some(lamps()));
        o.keyboard_held().store(true, Ordering::Relaxed);
        let mut out = Vec::new();
        for ms in (from..to).step_by(10) {
            for w in o.frame(SceneTime(ms)) {
                match w {
                    Out::Lamps { key, colors } => {
                        assert_eq!(key, "1d50:615e");
                        out.push(colors);
                    }
                    other => panic!("only the keyboard's LampArray: {other:?}"),
                }
            }
        }
        out
    }

    /// Solid has nothing to animate; Breathe, Spectrum and Swirl each map to a scene look at
    /// about the keyboard's pace; Swirl in the chosen colour is the firmware's band.
    #[test]
    fn the_cards_effects_map_onto_scene_looks() {
        assert_eq!(scene_look(&look(0, false)), None);
        assert_eq!(scene_look(&look(9, false)), None);
        let breathe = scene_look(&look(1, false)).unwrap();
        assert_eq!(breathe.spread, Spread::Together);
        assert!(
            matches!(breathe.effect, Effect::Breathe { to, period_ms: 4000, .. } if to == Rgb::BLACK),
            "{breathe:?}"
        );
        let spectrum = scene_look(&look(2, false)).unwrap();
        assert_eq!(spectrum.spread, Spread::Together);
        let Effect::Sweep {
            stops,
            period_ms: turn,
            ..
        } = &spectrum.effect
        else {
            panic!("{spectrum:?}");
        };
        assert_eq!(*turn, 6000);
        assert_eq!(stops.len(), 6, "the hue wheel");
        assert_eq!(stops[0], Rgb(hsv_rgb(0, 100, 80)));
        let swirl = scene_look(&look(3, false)).unwrap();
        assert_eq!(swirl.spread, Spread::Across);
        assert!(
            matches!(&swirl.effect, Effect::Sweep { stops, period_ms: 3000, .. } if stops.len() == 6)
        );
        let band = scene_look(&look(3, true)).unwrap();
        let Effect::Sweep { stops, .. } = &band.effect else {
            panic!("{band:?}");
        };
        assert_eq!(
            stops,
            &vec![
                Rgb(hsv_rgb(0, 100, 48)),
                Rgb(hsv_rgb(30, 100, 64)),
                Rgb(hsv_rgb(60, 100, 80)),
                Rgb(hsv_rgb(30, 100, 64)),
            ],
            "30 degrees either side, 40 % dimmer at the trough"
        );
        // Faster at a higher speed; an unknown speed is the middle one.
        assert_eq!(period_ms(2, 1), 18_000);
        assert_eq!(period_ms(2, 5), 3600);
        assert_eq!(period_ms(1, 0), period_ms(1, 3));
        assert_eq!(period_ms(0, 3), 0);
    }

    /// The card's room is the keyboard alone; Together has no width, so every lamp shows the
    /// same point of the palette; no keyboard, no room.
    #[test]
    fn the_cards_room_is_the_keyboard_alone() {
        let across = card_room(&room(), Spread::Across).unwrap();
        assert_eq!(across.devices.len(), 2);
        assert!(across.devices.iter().all(|d| d.id.starts_with(KEYBOARD)));
        assert!(across.bounds.w > 100.0);
        let together = card_room(&room(), Spread::Together).unwrap();
        assert_eq!(together.bounds.w, 0.0);
        let no_keyboard = RoomMap::build(&[], &[], &Default::default());
        assert_eq!(card_room(&no_keyboard, Spread::Across), None);
    }

    /// Through the scene output: Spectrum turns every lamp's hue together, Swirl runs the
    /// wheel across the keyboard, Breathe fades the chosen colour; all only on the keyboard's
    /// LampArray, paced and only on change.
    #[test]
    fn the_effects_animate_on_the_keyboards_lamparray() {
        let spectrum = writes(&scene_look(&look(2, false)).unwrap(), 0, 3000);
        assert!(spectrum.len() > 20, "{}", spectrum.len());
        assert!(spectrum.iter().all(|c| c.len() == 42));
        assert!(
            spectrum.iter().all(|c| c.iter().all(|l| *l == c[0])),
            "every lamp together"
        );
        assert_ne!(spectrum[0][0], spectrum[spectrum.len() - 1][0], "it turns");
        let swirl = writes(&scene_look(&look(3, false)).unwrap(), 0, 1000);
        assert!(swirl.len() > 10);
        assert!(swirl.iter().all(|c| c[0] != c[20]), "across the keyboard");
        assert_ne!(swirl[0], swirl[swirl.len() - 1], "it turns");
        let breathe = writes(&scene_look(&look(1, false)).unwrap(), 0, 4000);
        let peak = Rgb(hsv_rgb(30, 100, 80)).0;
        assert_eq!(breathe[0][0], peak, "from the chosen colour");
        assert!(
            breathe
                .iter()
                .any(|c| c[0].iter().map(|&v| u32::from(v)).sum::<u32>() < 30),
            "to dark"
        );
    }

    /// The rules: an effect starts the run and a later one changes it; a steady pick stops it
    /// and waits; a scene stops it and its end brings it back, but not on quitting; a pick
    /// during a scene waits for its end; Windows letting go ends it without waiting and
    /// forgets the look.
    #[test]
    fn the_plan_gives_way_to_scenes_and_ends_on_release() {
        let a = scene_look(&look(2, false)).unwrap();
        let b = scene_look(&look(3, false)).unwrap();
        let mut p = Plan::default();
        assert_eq!(p.show(None, false), Step::Nothing);
        assert_eq!(p.show(Some(a.clone()), false), Step::Start(a.clone()));
        assert_eq!(p.show(Some(b.clone()), false), Step::Change(b.clone()));
        assert_eq!(p.show(None, false), Step::Stop { wait: true });
        assert_eq!(p.show(Some(a.clone()), false), Step::Start(a.clone()));
        assert_eq!(p.scene_starting(), Step::Stop { wait: true });
        assert_eq!(p.scene_starting(), Step::Nothing);
        assert_eq!(
            p.show(Some(b.clone()), true),
            Step::Nothing,
            "the scene first"
        );
        assert_eq!(p.scene_ended(false), Step::Start(b.clone()));
        assert_eq!(p.scene_ended(false), Step::Nothing, "already running");
        assert_eq!(p.scene_starting(), Step::Stop { wait: true });
        assert_eq!(p.scene_ended(true), Step::Nothing, "not on quitting");
        assert_eq!(p.scene_ended(false), Step::Start(b.clone()));
        assert_eq!(p.release(false), Step::Stop { wait: false });
        assert_eq!(p.scene_ended(false), Step::Nothing, "forgotten");
        assert_eq!(p.show(None, true), Step::Nothing);
    }

    /// A fake LampArray: each write goes to the test; dropping it is the let-go.
    struct Fake {
        writes: mpsc::Sender<Option<Vec<[u8; 3]>>>,
    }

    impl super::super::scene_lamps::LampDevice for Fake {
        fn available(&self) -> bool {
            true
        }
        fn set(&self, colours: &[[u8; 3]]) -> Result<(), String> {
            let _ = self.writes.send(Some(colours.to_vec()));
            Ok(())
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = self.writes.send(None);
        }
    }

    struct FakeOpener(mpsc::Sender<Option<Vec<[u8; 3]>>>);

    impl Opener for FakeOpener {
        fn open(
            &mut self,
            key: &str,
            _: super::super::scene_lamps::Changed,
        ) -> Vec<Box<dyn super::super::scene_lamps::LampDevice>> {
            assert_eq!(key, "1d50:615e");
            vec![Box::new(Fake {
                writes: self.0.clone(),
            })]
        }
    }

    /// The run: frames on the scene tick reach the keyboard's LampArray; pause lets it go and
    /// writes nothing until resumed; away the same; a new look keeps the run; the stop lets it
    /// go. Window-free: a fake LampArray and the test's room.
    #[test]
    fn the_run_draws_gives_way_to_pause_and_away_and_lets_go() {
        let (wtx, writes) = mpsc::channel();
        let env = Env {
            room: Box::new(room),
            lamps: Box::new(|| Some(lamps())),
            opener: Box::new(FakeOpener(wtx)),
            status: Arc::default(),
            notify: None,
        };
        let (tx, rx) = mpsc::channel();
        let own = tx.clone();
        let first = scene_look(&look(2, false)).unwrap();
        let run = std::thread::spawn(move || control(first, env, &own, &rx));
        let wait = Duration::from_secs(5);
        let next = || writes.recv_timeout(wait).expect("a write or a let-go");
        // Drawing: two different frames in a row.
        let a = next().expect("a frame");
        let b = (0..100)
            .map(|_| next().expect("a frame"))
            .find(|f| *f != a)
            .expect("the effect moves");
        assert_eq!(b.len(), 42);
        // Paused: let go, then nothing until resumed.
        tx.send(Ctl::Paused(true)).unwrap();
        while next().is_some() {}
        assert!(writes.recv_timeout(Duration::from_millis(300)).is_err());
        tx.send(Ctl::Paused(false)).unwrap();
        assert!(next().is_some(), "resumed: drawn again");
        // Away: the same.
        tx.send(Ctl::Away(true)).unwrap();
        while next().is_some() {}
        assert!(writes.recv_timeout(Duration::from_millis(300)).is_err());
        tx.send(Ctl::Away(false)).unwrap();
        assert!(next().is_some(), "back: drawn again");
        // A new look in the same run, across the keyboard.
        tx.send(Ctl::Look(Box::new(scene_look(&look(3, false)).unwrap())))
            .unwrap();
        let across = (0..100)
            .map(|_| next())
            .find(|f| f.as_ref().is_some_and(|c| c[0] != c[20]));
        assert!(across.is_some(), "Swirl runs across");
        tx.send(Ctl::Stop).unwrap();
        run.join().unwrap();
        let rest: Vec<_> = writes.try_iter().collect();
        assert_eq!(rest.last(), Some(&None), "let go at the end");
    }
}
