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

/// The most an eased ambient light may change in one 33 ms frame, in any channel.
const MAX_STEP: u8 = 12;

/// The Scenes page's "Night" background.
const NIGHT: Rgb = Rgb([6, 8, 18]);

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
        // At most one attack step: 33 ms of a 600 ms time constant, about 5 % of the way
        // from night to full orange (a ping is the whole way in a frame or two).
        assert!(
            amb_step <= MAX_STEP,
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

/// A frame at `t` with the two point lights' colours and the ambient one at their peak.
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
fn it_follows_the_group_peak_with_the_time_constants() {
    let room = bench();
    let (w, k) = (Rgb::WHITE, Rgb::BLACK);
    // Its target is the group's peak, channel by channel: one lit light is enough.
    assert_eq!(colour_of(&frame(&room, 0, w, k), "amb"), w);
    let (a, b) = (Rgb([200, 40, 0]), Rgb([30, 90, 10]));
    assert_eq!(colour_of(&frame(&room, 0, a, b), "amb"), Rgb([200, 90, 10]));
    // Eased in Oklab lightness, from black's (0).
    let target = w.lab()[0];

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
    // With point lights beside it, it is their peak instead.
    let room = self::room();
    let f = scanline(80.0, 4_000).render(&room, SceneTime(1_000));
    let points = ["1", "5", "6", "7"].map(|z| colour(&f, z));
    assert_eq!(Some(colour(&f, "2")), peak(points));
}

/// #546: the user's scene, a narrow orange scanline down the room over "Night" at speed 3,
/// lights one fan at a time. The interior light glows while the band is in the case, as
/// brightly as a lit fan, swells and fades smoothly, and never drops below the background.
#[test]
fn a_narrow_scanline_makes_the_interior_light_glow_through_the_case() {
    let room = room();
    let period = 7_000;
    let effect = Effect::Scanline {
        colour: Rgb([229, 115, 43]),
        background: NIGHT,
        angle_deg: 90.0,
        width_mm: 108.0,
        period_ms: period,
    };
    let (mut ambient, mut averaged) = (Ambient::new(), Ambient::new());
    let (mut amb_step, mut fan_max, mut amb_max, mut amb_min) = (0u8, 0u8, 0u8, u8::MAX);
    let mut avg_max = 0u8;
    let mut prev: Option<Rgb> = None;
    for k in 0..(3 * period / 33) {
        let mut f = effect.render(&room, SceneTime(i64::from(k) * 33));
        // What it was before #546: the eased average of the case's lights.
        let mut avg = f.clone();
        let points = ["1", "5", "6", "7"].map(|z| colour(&avg, z));
        for l in avg.lights.iter_mut().filter(|l| l.light == "2") {
            l.colour = Rgb::average(points).unwrap();
        }
        averaged.apply(&mut avg, &room);
        ambient.apply(&mut f, &room);
        let amb = colour(&f, "2");
        if let Some(p) = prev {
            amb_step = amb_step.max(step(p, amb));
        }
        prev = Some(amb);
        if k >= period / 33 {
            for fan in ["1", "5", "6", "7"] {
                fan_max = fan_max.max(colour(&f, fan).0[0]);
            }
            amb_max = amb_max.max(amb.0[0]);
            avg_max = avg_max.max(colour(&avg, "2").0[0]);
            amb_min = amb_min.min(amb.0[0]);
            for i in 0..3 {
                assert!(
                    amb.0[i] >= NIGHT.0[i],
                    "never below the background: {amb:?}"
                );
            }
        }
    }
    assert!(fan_max >= 200, "the band lights the fans ({fan_max})");
    assert!(
        u32::from(amb_max) * 2 >= u32::from(fan_max),
        "it glows half as bright as a lit fan or more ({amb_max} of {fan_max})"
    );
    // In light, three times the average's glow or more (152 against 81 in sRGB red).
    let light = |v: u8| Rgb([v, 0, 0]).linear()[0];
    assert!(
        light(amb_max) >= 3.0 * light(avg_max),
        "far brighter than the average, which read as off ({amb_max} vs {avg_max})"
    );
    assert!(
        amb_min <= 40,
        "and fades once the band leaves the case ({amb_min})"
    );
    assert!(
        amb_step <= MAX_STEP,
        "smoothly, without a ping ({amb_step} per frame)"
    );
}
