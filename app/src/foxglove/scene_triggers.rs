//! Scene triggers (#489): what starts a scene or draws over a running one, all riding events
//! Foxglove already gets; nothing here hooks Windows again or keeps a timer.
//!
//! - **Start with this profile** (`[scenes]`, profiled): a profile switch, by a program rule
//!   (`foreground.rs`, the foreground hook) or by hand, starts the scene when the new profile has
//!   it on, and stops a scene a profile started when it is off. A scene started by hand is never
//!   stopped by a switch; it takes the new profile's look. [`on_profile`] decides.
//! - **Ripple on key press:** the keyboard's key report (KeyPeek's reader, `keyboard.rs`) gives
//!   the key's row and column only, never what it types. Its place in the keyboard's layout
//!   becomes a point on the room map ([`key_point`]) and the running scene draws one ring from
//!   there ([`foxglove_scene::burst`]).
//! - **Sweep on layer change:** the `keyboard.layer` fact (the firmware's layer event) sends one
//!   sweep across the room in the new top layer's overlay colour.
//!
//! Ripples and sweeps show only while a scene runs. Nothing is logged about keys.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use foxglove_scene::burst::Shape;
use foxglove_scene::{Burst, DeviceKind, Effect, Point, Rect, Rgb, RoomMap, SceneTime};

use super::scene_looks::Chosen;

static RIPPLE: AtomicBool = AtomicBool::new(false);
static SWEEP: AtomicBool = AtomicBool::new(false);

/// The active profile's trigger boxes (the Scenes page and profile switches keep it current).
pub fn set_flags(c: &Chosen) {
    RIPPLE.store(c.ripple_keys, Ordering::Relaxed);
    SWEEP.store(c.sweep_layers, Ordering::Relaxed);
}

/// What a profile switch does to the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileAction {
    /// Start the new profile's scene.
    Start,
    /// Stop the scene a profile started.
    Stop,
    /// The running scene takes the new profile's look.
    Restyle,
    Nothing,
}

/// A profile switch: `running` a scene runs, `by_profile` a profile started it, `start` the new
/// profile's "Start with this profile".
pub fn on_profile(running: bool, by_profile: bool, start: bool) -> ProfileAction {
    match (running, start) {
        (false, true) => ProfileAction::Start,
        (false, false) => ProfileAction::Nothing,
        (true, false) if by_profile => ProfileAction::Stop,
        (true, _) => ProfileAction::Restyle,
    }
}

/// A key's place in the keyboard's layout, in key units (KeyPeek's layout).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyPlace {
    pub row: usize,
    pub col: usize,
    /// The key's centre.
    pub x: f32,
    pub y: f32,
}

static LAYOUT: Mutex<Vec<KeyPlace>> = Mutex::new(Vec::new());

/// The keyboard's layout (KeyPeek read it from the keyboard): where each key is.
pub fn set_layout(layout: &crate::protocols::KeyboardLayout) {
    let keys = layout
        .keys
        .iter()
        .map(|k| KeyPlace {
            row: k.row,
            col: k.col,
            x: k.x + k.w / 2.0,
            y: k.y + k.h / 2.0,
        })
        .collect();
    *LAYOUT.lock().unwrap_or_else(|e| e.into_inner()) = keys;
}

/// Where the key at `row`, `col` is on the room map: on its half (the keys left of the layout's
/// middle on the left half), else on the whole keyboard; `None` without a keyboard in the room
/// or a key the layout doesn't have.
pub fn key_point(keys: &[KeyPlace], row: usize, col: usize, room: &RoomMap) -> Option<Point> {
    let key = keys.iter().find(|k| k.row == row && k.col == col)?;
    let rect = |kind: DeviceKind| room.devices.iter().find(|d| d.kind == kind).map(|d| d.rect);
    let span = |ks: &mut dyn Iterator<Item = &KeyPlace>| {
        ks.fold(None, |acc: Option<(f32, f32, f32, f32)>, k| {
            Some(match acc {
                None => (k.x, k.y, k.x, k.y),
                Some((x0, y0, x1, y1)) => (x0.min(k.x), y0.min(k.y), x1.max(k.x), y1.max(k.y)),
            })
        })
    };
    let (x0, _, x1, _) = span(&mut keys.iter())?;
    let middle = (x0 + x1) / 2.0;
    let left = key.x < middle;
    let half = if left {
        rect(DeviceKind::KeyboardLeft)
    } else {
        rect(DeviceKind::KeyboardRight)
    };
    let (target, (kx0, ky0, kx1, ky1)) = match half {
        Some(r) => (
            r,
            span(&mut keys.iter().filter(|k| (k.x < middle) == left))?,
        ),
        None => (rect(DeviceKind::Keyboard)?, span(&mut keys.iter())?),
    };
    let fit = |v: f32, lo: f32, hi: f32| if hi > lo { (v - lo) / (hi - lo) } else { 0.5 };
    Some(place(&target, fit(key.x, kx0, kx1), fit(key.y, ky0, ky1)))
}

/// A point inside `r`, `fx` and `fy` from 0 to 1, kept off the very edge.
fn place(r: &Rect, fx: f32, fy: f32) -> Point {
    let inset = |f: f32| 0.08 + 0.84 * f.clamp(0.0, 1.0);
    Point::new(r.x + r.w * inset(fx), r.y + r.h * inset(fy))
}

/// A ring's crossing time and width.
pub const RIPPLE_MS: u32 = 1200;
pub const RIPPLE_WIDTH_MM: f32 = 60.0;
/// A sweep's crossing time and width.
pub const SWEEP_MS: u32 = 900;
pub const SWEEP_WIDTH_MM: f32 = 150.0;

/// The ring a key press sends at scene time `now`, in the scene's colour; `None` for a key the
/// room has no place for.
pub fn ripple(
    keys: &[KeyPlace],
    row: usize,
    col: usize,
    room: &RoomMap,
    effect: &Effect,
    now: SceneTime,
) -> Option<Burst> {
    Some(Burst {
        shape: Shape::Ring {
            origin: key_point(keys, row, col, room)?,
        },
        colour: effect_colour(effect),
        start: now,
        duration_ms: RIPPLE_MS,
        width_mm: RIPPLE_WIDTH_MM,
    })
}

/// The sweep a layer change sends at `now`: left to right across the room.
pub fn sweep(colour: Rgb, now: SceneTime) -> Burst {
    Burst {
        shape: Shape::Sweep { angle_deg: 0.0 },
        colour,
        start: now,
        duration_ms: SWEEP_MS,
        width_mm: SWEEP_WIDTH_MM,
    }
}

/// The keys as last read.
pub fn keys() -> Vec<KeyPlace> {
    LAYOUT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The scene's own colour, for its ripples: a one-colour effect's colour, a palette's first.
pub fn effect_colour(e: &Effect) -> Rgb {
    match e {
        Effect::Scanline { colour, .. } | Effect::Ripple { colour, .. } => *colour,
        Effect::Breathe { to, .. } => *to,
        Effect::Sweep { stops, .. }
        | Effect::Gradient { stops, .. }
        | Effect::Noise { stops, .. } => stops.first().copied().unwrap_or(Rgb([255, 255, 255])),
    }
}

/// A layer's overlay colour, as bright as a light can show it (the overlay's colours are dim
/// fills): its hue at full value.
pub fn bright(r: u8, g: u8, b: u8) -> Rgb {
    let max = r.max(g).max(b);
    if max == 0 {
        return Rgb([255, 255, 255]);
    }
    let k = 255.0 / f32::from(max);
    let s = |c: u8| (f32::from(c) * k).round().min(255.0) as u8;
    Rgb([s(r), s(g), s(b)])
}

/// A key report from the keyboard (KeyPeek's reader thread): a press sends a ripple when the
/// box is on and a scene runs. Row and column only.
pub fn key(row: usize, col: usize, pressed: bool) {
    if pressed && RIPPLE.load(Ordering::Relaxed) {
        super::scene_run::key_ripple(row, col);
    }
}

/// Follows the top layer (the `keyboard.layer` fact) for "Sweep on layer change". One thread,
/// blocked on the fact's changes.
pub fn start() {
    let facts = super::devices::bus().facts();
    let changes = facts.subscribe("keyboard.layer");
    let mut last = match facts.get("keyboard.layer") {
        Some(foxglove_devices::facts::Value::Int(l)) => Some(l),
        _ => None,
    };
    super::services::spawn_named("scene-triggers", move || {
        for change in changes {
            let now = match change.value {
                Some(foxglove_devices::facts::Value::Int(l)) => Some(l),
                _ => None,
            };
            let (Some(layer), true) = (now, now != last) else {
                last = now;
                continue;
            };
            // The first layer after a (re)connect is no change.
            let first = last.is_none();
            last = now;
            if first || !SWEEP.load(Ordering::Relaxed) || super::scene_run::running().is_none() {
                continue;
            }
            let theme = crate::settings::Settings::load()
                .map(|s| s.theme)
                .unwrap_or_default();
            let c = theme.layer_color(u8::try_from(layer).unwrap_or(u8::MAX));
            super::scene_run::layer_sweep(bright(c.r, c.g, c.b));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::foreground::decide;
    use crate::foxglove::store::Rule;
    use foxglove_scene::{DeviceInfo, Layout, SceneTime};

    #[test]
    fn a_profile_switch_starts_stops_or_restyles() {
        use ProfileAction::*;
        assert_eq!(on_profile(false, false, true), Start);
        assert_eq!(on_profile(false, false, false), Nothing);
        assert_eq!(
            on_profile(true, true, false),
            Stop,
            "a profile's scene ends"
        );
        assert_eq!(
            on_profile(true, false, false),
            Restyle,
            "a hand-started one stays"
        );
        assert_eq!(on_profile(true, true, true), Restyle);
        assert_eq!(on_profile(true, false, true), Restyle);
    }

    /// Fake foreground events through the profile rules: the game's program starts its
    /// profile's scene, leaving it stops that scene, and a scene started by hand survives.
    #[test]
    fn switching_programs_switches_scenes() {
        let rules = vec![Rule {
            program: "eldenring.exe".into(),
            profile: "Game".into(),
        }];
        // Which profiles have "Start with this profile" on.
        let starts = |profile: &Option<String>| profile.as_deref() == Some("Game");
        let mut active: Option<String> = None;
        let mut by_rule = false;
        let (mut running, mut by_profile) = (false, false);
        let mut step = |program: &str, running: &mut bool, by_profile: &mut bool| {
            let next = decide(&rules, program, active.as_deref(), by_rule)?;
            by_rule = next.is_some();
            active = next;
            let action = on_profile(*running, *by_profile, starts(&active));
            match action {
                ProfileAction::Start => (*running, *by_profile) = (true, true),
                ProfileAction::Stop => (*running, *by_profile) = (false, false),
                _ => {}
            }
            Some(action)
        };
        assert_eq!(step("explorer.exe", &mut running, &mut by_profile), None);
        assert_eq!(
            step("eldenring.exe", &mut running, &mut by_profile),
            Some(ProfileAction::Start)
        );
        assert!(running && by_profile);
        assert_eq!(
            step("code.exe", &mut running, &mut by_profile),
            Some(ProfileAction::Stop)
        );
        assert!(!running);
        // Started by hand on the base settings: the game restyles it, leaving keeps it.
        (running, by_profile) = (true, false);
        assert_eq!(
            step("eldenring.exe", &mut running, &mut by_profile),
            Some(ProfileAction::Restyle)
        );
        assert_eq!(
            step("code.exe", &mut running, &mut by_profile),
            Some(ProfileAction::Restyle)
        );
        assert!(running);
    }

    /// A Corne-like layout: three columns by two rows on each half, a gap between.
    fn corne() -> Vec<KeyPlace> {
        let mut keys = Vec::new();
        for row in 0..2 {
            for col in 0..6 {
                let x = col as f32 + if col >= 3 { 2.0 } else { 0.0 } + 0.5;
                keys.push(KeyPlace {
                    row,
                    col,
                    x,
                    y: row as f32 + 0.5,
                });
            }
        }
        keys
    }

    fn halves() -> RoomMap {
        let devices = [
            DeviceInfo::whole("keyboard:left", "L", DeviceKind::KeyboardLeft),
            DeviceInfo::whole("keyboard:right", "R", DeviceKind::KeyboardRight),
        ];
        RoomMap::build(&[], &devices, &Layout::default())
    }

    #[test]
    fn a_key_is_placed_on_its_half() {
        let room = halves();
        let left = room.devices[0].rect;
        let right = room.devices[1].rect;
        let keys = corne();
        let tl = key_point(&keys, 0, 0, &room).unwrap();
        assert!(
            (tl.x - (left.x + left.w * 0.08)).abs() < 0.01,
            "{tl:?} {left:?}"
        );
        assert!((tl.y - (left.y + left.h * 0.08)).abs() < 0.01);
        let br = key_point(&keys, 1, 5, &room).unwrap();
        assert!(
            (br.x - (right.x + right.w * 0.92)).abs() < 0.01,
            "{br:?} {right:?}"
        );
        assert!((br.y - (right.y + right.h * 0.92)).abs() < 0.01);
        assert_eq!(key_point(&keys, 9, 9, &room), None, "not in the layout");
        assert_eq!(
            key_point(&keys, 0, 0, &RoomMap::default()),
            None,
            "no keyboard"
        );
        // One keyboard rather than halves: the whole layout over it.
        let one = RoomMap::build(
            &[],
            &[DeviceInfo::whole("keyboard", "K", DeviceKind::Keyboard)],
            &Layout::default(),
        );
        let whole = one.devices[0].rect;
        let p = key_point(&keys, 1, 5, &one).unwrap();
        assert!((p.x - (whole.x + whole.w * 0.92)).abs() < 0.01);
    }

    #[test]
    fn a_key_press_sends_a_ring_of_the_scenes_colour_from_its_key() {
        let room = halves();
        let effect = Effect::Sweep {
            stops: vec![Rgb([1, 2, 3]), Rgb([9, 9, 9])],
            angle_deg: 0.0,
            period_ms: 1000,
        };
        let ring = ripple(&corne(), 1, 4, &room, &effect, SceneTime(500)).unwrap();
        assert_eq!(ring.colour, Rgb([1, 2, 3]), "a palette's first colour");
        assert_eq!(ring.start, SceneTime(500));
        let at = key_point(&corne(), 1, 4, &room).unwrap();
        assert_eq!(ring.shape, Shape::Ring { origin: at });
        assert!(ring.level(at, SceneTime(500), &room.bounds) > 0.99);
        assert!(ripple(&corne(), 7, 7, &room, &effect, SceneTime(0)).is_none());
        let s = sweep(Rgb([0, 255, 0]), SceneTime(0));
        assert_eq!(s.shape, Shape::Sweep { angle_deg: 0.0 });
    }

    #[test]
    fn layer_colours_are_brightened_and_flags_follow_the_profile() {
        assert_eq!(bright(80, 140, 115), Rgb([146, 255, 209]));
        assert_eq!(bright(0, 0, 0), Rgb([255, 255, 255]));
        set_flags(&Chosen {
            ripple_keys: true,
            ..Chosen::default()
        });
        assert!(RIPPLE.load(Ordering::Relaxed) && !SWEEP.load(Ordering::Relaxed));
        set_flags(&Chosen::default());
        assert!(!RIPPLE.load(Ordering::Relaxed));
    }
}
