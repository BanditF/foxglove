use super::*;
use crate::effects::{Effect, FrameLight};
use crate::room::{DeviceInfo, DeviceKind, Layout, LightSpec, MonitorInfo, Placed, PlacedLight};
use crate::space::{Point, Rect};

/// The OMEN 45L as the catalog has it: the diamond, the interior light (ambient) and the three
/// front fans, beside a 27" monitor and the Corne's halves.
fn room() -> RoomMap {
    let zone = |id: &str, x: f32, y: f32| LightSpec::point(id, Some(Point::new(x, y)));
    let case = DeviceInfo {
        id: "case".into(),
        name: "HP OMEN 45L case lights".into(),
        kind: DeviceKind::Case,
        size: Some(Point::new(205.0, 475.0)),
        lights: vec![
            zone("1", 102.0, 55.0),
            zone("2", 120.0, 250.0).with_role(LightRole::Ambient),
            zone("5", 102.0, 405.0),
            zone("6", 102.0, 285.0),
            zone("7", 102.0, 165.0),
        ],
    };
    let monitor = MonitorInfo {
        id: r"\\.\DISPLAY1".into(),
        primary: true,
        rect_px: [0, 0, 2560, 1440],
        raw_dpi: Some(109),
        dpi: 96,
    };
    let devices = [
        DeviceInfo::whole("keyboard:left", "left", DeviceKind::KeyboardLeft),
        DeviceInfo::whole("keyboard:right", "right", DeviceKind::KeyboardRight),
        case,
    ];
    RoomMap::build(&[monitor], &devices, &Layout::default())
}

/// The Bandit HUD's scanline: Bandit orange down the room over night blue.
fn scanline(width_mm: f32, period_ms: u32) -> Effect {
    Effect::Scanline {
        colour: Rgb([255, 106, 0]),
        background: Rgb([8, 12, 40]),
        angle_deg: 90.0,
        width_mm,
        period_ms,
    }
}

fn colour(f: &Frame, light: &str) -> Rgb {
    f.lights
        .iter()
        .find(|l| l.device == "case" && l.light == light)
        .map(|l| l.colour)
        .unwrap_or_else(|| panic!("case {light}"))
}

/// The largest change of any channel between two colours.
fn step(a: Rgb, b: Rgb) -> u8 {
    (0..3).map(|i| a.0[i].abs_diff(b.0[i])).max().unwrap_or(0)
}

#[test]
fn the_interior_light_never_pings_when_a_scanline_passes() {
    let room = room();
    // The page's widths and speeds: narrow and fast to wide and slow.
    for (width, period) in [(20.0, 1_000), (60.0, 3_000), (80.0, 4_000), (250.0, 9_000)] {
        let effect = scanline(width, period);
        let mut ambient = Ambient::new();
        let (mut fan_step, mut amb_step) = (0u8, 0u8);
        let (mut amb_min, mut amb_max) = (u8::MAX, 0u8);
        let mut prev: Option<Frame> = None;
        // Three cycles at 30 frames a second.
        for k in 0..(3 * period / 33) {
            let mut f = effect.render(&room, SceneTime(i64::from(k) * 33));
            ambient.apply(&mut f, &room);
            if let Some(p) = &prev {
                for fan in ["5", "6", "7"] {
                    fan_step = fan_step.max(step(colour(p, fan), colour(&f, fan)));
                }
                amb_step = amb_step.max(step(colour(p, "2"), colour(&f, "2")));
            }
            // After the first cycle (from a standing start), it swells and fades.
            if k >= period / 33 {
                let g = colour(&f, "2").0[1];
                amb_min = amb_min.min(g);
                amb_max = amb_max.max(g);
            }
            prev = Some(f);
        }
        // A wide, slow band moves gently over the fans too; a narrow one pings them.
        assert!(
            width > 100.0 || fan_step >= 40,
            "{width} mm / {period} ms: the fans ping ({fan_step})"
        );
        assert!(
            amb_step <= 8,
            "{width} mm / {period} ms: the interior light eases ({amb_step} per frame)"
        );
        assert!(
            width < 60.0 || amb_max - amb_min >= 8,
            "{width} mm / {period} ms: it still fades in and out ({amb_min}..{amb_max})"
        );
    }
}

/// One device, two point lights and an ambient one, for frames made by hand.
fn bench() -> RoomMap {
    let at = Point::new(10.0, 10.0);
    let mut amb = PlacedLight::point("amb", at);
    amb.role = LightRole::Ambient;
    RoomMap {
        devices: vec![Placed {
            id: "d".into(),
            name: "d".into(),
            kind: DeviceKind::Other,
            rect: Rect::new(0.0, 0.0, 20.0, 20.0),
            lights: vec![
                PlacedLight::point("a", at),
                PlacedLight::point("b", at),
                amb,
            ],
        }],
        bounds: Rect::new(0.0, 0.0, 20.0, 20.0),
    }
}

/// A frame at `t` with the two point lights' colours and the ambient one at their average.
fn frame(room: &RoomMap, t: i64, a: Rgb, b: Rgb) -> Frame {
    let light = |id: &str, colour| FrameLight {
        device: "d".into(),
        light: id.into(),
        colour,
    };
    let mut f = Frame {
        t: SceneTime(t),
        lights: vec![light("a", a), light("b", b), light("amb", Rgb::BLACK)],
    };
    group(&mut f, room);
    f
}

/// The ambient light's Oklab lightness after easing up to `t`, one frame every `dt`.
fn run(
    room: &RoomMap,
    amb: &mut Ambient,
    from: i64,
    to: i64,
    dt: i64,
    a: Rgb,
    b: Rgb,
) -> (f32, Frame) {
    let mut t = from;
    let mut last = None;
    while t <= to {
        let mut f = frame(room, t, a, b);
        amb.apply(&mut f, room);
        last = Some(f);
        t += dt;
    }
    let f = last.expect("a frame");
    (colour_of(&f, "amb").lab()[0], f)
}

fn colour_of(f: &Frame, light: &str) -> Rgb {
    f.lights
        .iter()
        .find(|l| l.light == light)
        .map(|l| l.colour)
        .unwrap_or_default()
}

#[test]
fn it_follows_the_group_average_with_the_time_constants() {
    let room = bench();
    let (w, k) = (Rgb::WHITE, Rgb::BLACK);
    // Its target is the group's average in linear light: half of white's light.
    let half = Rgb::average([w, k]).unwrap();
    assert_eq!(colour_of(&frame(&room, 0, w, k), "amb"), half);
    // Eased in Oklab lightness, from black's (0).
    let target = half.lab()[0];

    for dt in [10, 33, 100] {
        let mut amb = Ambient::new();
        // A standing start at black.
        run(&room, &mut amb, 0, 0, dt, k, k);
        // One attack time constant later: 63 % of the way up.
        let ms = ATTACK_MS as i64;
        let (up, _) = run(&room, &mut amb, dt, ms - ms % dt, dt, w, k);
        let ends = (ms - ms % dt) as f32;
        let want = target * (1.0 - (-ends / ATTACK_MS).exp());
        assert!((up - want).abs() < 0.01, "{dt} ms frames: {up} vs {want}");
        // Then the group goes dark: one decay time constant later, 37 % is left.
        let start = ms - ms % dt;
        let ds = DECAY_MS as i64;
        let (down, _) = run(&room, &mut amb, start + dt, start + ds - ds % dt, dt, k, k);
        let want = up * (-((ds - ds % dt) as f32) / DECAY_MS).exp();
        assert!(
            (down - want).abs() < 0.01,
            "{dt} ms frames: {down} vs {want}"
        );
    }
    // Its attack is quicker than its decay.
    const { assert!(ATTACK_MS < DECAY_MS) };
}

#[test]
fn a_gap_or_a_jump_back_starts_at_the_target() {
    let room = bench();
    let (w, k) = (Rgb::WHITE, Rgb::BLACK);
    let mut amb = Ambient::new();
    run(&room, &mut amb, 0, 0, 33, k, k);
    // Paused for longer than the gap: no slow fade from a colour long gone.
    let (after_gap, _) = run(&room, &mut amb, 10_000, 10_000, 33, w, w);
    assert!((after_gap - 1.0).abs() < 1e-3, "{after_gap}");
    let (back, _) = run(&room, &mut amb, 0, 0, 33, k, k);
    assert!(back.abs() < 1e-3, "earlier than the last frame: {back}");
    // Point lights pass through untouched.
    let (_, f) = run(&room, &mut amb, 33, 33, 33, w, k);
    assert_eq!(colour_of(&f, "a"), w);
    assert_eq!(colour_of(&f, "b"), k);
    amb.reset();
    assert_eq!(amb, Ambient::new());
}

#[test]
fn an_ambient_light_alone_shows_its_area_not_its_point() {
    let mut room = bench();
    room.devices[0]
        .lights
        .retain(|l| l.role == LightRole::Ambient);
    // A hard edge: left half red, right half blue, the light itself at the left.
    room.devices[0].lights[0].at = Point::new(1.0, 10.0);
    let effect = Effect::Gradient {
        stops: vec![
            Rgb([255, 0, 0]),
            Rgb([255, 0, 0]),
            Rgb([0, 0, 255]),
            Rgb([0, 0, 255]),
        ],
        angle_deg: 0.0,
    };
    let f = effect.render(&room, SceneTime(0));
    let c = colour_of(&f, "amb");
    assert!(c.0[0] > 100 && c.0[2] > 100, "both halves show: {c:?}");
    // With point lights beside it, it is their average instead.
    let room = self::room();
    let f = scanline(80.0, 4_000).render(&room, SceneTime(1_000));
    let points = ["1", "5", "6", "7"].map(|z| colour(&f, z));
    assert_eq!(Some(colour(&f, "2")), Rgb::average(points));
}
