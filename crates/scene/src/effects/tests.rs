use super::*;
use crate::clock::Clock;
use crate::room::{DeviceKind, Placed, PlacedLight};

const RED: Rgb = Rgb([255, 0, 0]);
const BLUE: Rgb = Rgb([0, 0, 255]);
const PINE: Rgb = Rgb([20, 90, 60]);

/// A 1000 x 500 mm room: a strip across the middle (five lights, left to right) and a
/// column down the middle (top, bottom).
fn room() -> RoomMap {
    let lights = |pts: &[(f32, f32)]| -> Vec<PlacedLight> {
        pts.iter()
            .enumerate()
            .map(|(i, &(x, y))| PlacedLight::point(i.to_string(), Point::new(x, y)))
            .collect()
    };
    RoomMap {
        devices: vec![
            Placed {
                id: "monitor:x".into(),
                name: "x".into(),
                kind: DeviceKind::Monitor,
                rect: Rect::new(0.0, 0.0, 1000.0, 200.0),
                lights: vec![],
            },
            Placed {
                id: "strip".into(),
                name: "strip".into(),
                kind: DeviceKind::Other,
                rect: Rect::new(0.0, 240.0, 1000.0, 20.0),
                lights: lights(&[
                    (0.0, 250.0),
                    (250.0, 250.0),
                    (500.0, 250.0),
                    (750.0, 250.0),
                    (1000.0, 250.0),
                ]),
            },
            Placed {
                id: "column".into(),
                name: "column".into(),
                kind: DeviceKind::Other,
                rect: Rect::new(490.0, 0.0, 20.0, 500.0),
                lights: lights(&[(500.0, 0.0), (500.0, 500.0)]),
            },
        ],
        bounds: Rect::new(0.0, 0.0, 1000.0, 500.0),
    }
}

fn effects() -> Vec<(&'static str, Effect)> {
    vec![
        (
            "scanline",
            Effect::Scanline {
                colour: RED,
                background: PINE,
                angle_deg: 90.0,
                width_mm: 200.0,
                period_ms: 4000,
            },
        ),
        (
            "sweep",
            Effect::Sweep {
                stops: vec![RED, BLUE, PINE],
                angle_deg: 0.0,
                period_ms: 3000,
            },
        ),
        (
            "ripple",
            Effect::Ripple {
                origin: Point::new(500.0, 500.0),
                colour: BLUE,
                background: Rgb::BLACK,
                width_mm: 300.0,
                period_ms: 2000,
            },
        ),
        (
            "breathe",
            Effect::Breathe {
                from: PINE,
                to: Rgb::WHITE,
                period_ms: 4000,
            },
        ),
        (
            "gradient",
            Effect::Gradient {
                stops: vec![RED, BLUE],
                angle_deg: 0.0,
            },
        ),
        (
            "noise",
            Effect::Noise {
                stops: vec![Rgb::BLACK, PINE, Rgb::WHITE],
                scale_mm: 300.0,
                period_ms: 5000,
                seed: 7,
            },
        ),
    ]
}

fn hex(f: &Frame) -> String {
    f.lights
        .iter()
        .map(|l| {
            format!(
                "{:02x}{:02x}{:02x}",
                l.colour.0[0], l.colour.0[1], l.colour.0[2]
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Each effect at fixed times: the strip's five lights, then the column's two. Noise is
/// tied to `noise` 0.9 (pinned by the lockfile): a new version may move its field.
const GOLDEN: &[(&str, i64, &str)] = &[
    (
        "scanline",
        0,
        "145a3c 145a3c 145a3c 145a3c 145a3c 145a3c 145a3c",
    ),
    (
        "scanline",
        1000,
        "145a3c 145a3c 145a3c 145a3c 145a3c 4e5b39 145a3c",
    ),
    (
        "scanline",
        2500,
        "2a5a3b 2a5a3b 2a5a3b 2a5a3b 2a5a3b 145a3c 145a3c",
    ),
    (
        "sweep",
        0,
        "ff0000 5147d2 00529f 655a37 ff0000 00529f 00529f",
    ),
    (
        "sweep",
        1000,
        "145a3c cd4322 8c53a2 0040cf 145a3c 8c53a2 8c53a2",
    ),
    (
        "sweep",
        2500,
        "8c53a2 0040cf 145a3c cd4322 8c53a2 145a3c 145a3c",
    ),
    (
        "ripple",
        0,
        "000000 000000 000000 000000 000000 000000 000000",
    ),
    (
        "ripple",
        1000,
        "000000 0000ff 00001c 0000ff 000000 000000 000000",
    ),
    (
        "ripple",
        2500,
        "000000 000000 000000 000000 000000 000000 000020",
    ),
    (
        "breathe",
        0,
        "145a3c 145a3c 145a3c 145a3c 145a3c 145a3c 145a3c",
    ),
    (
        "breathe",
        1000,
        "8baa98 8baa98 8baa98 8baa98 8baa98 8baa98 8baa98",
    ),
    (
        "breathe",
        2500,
        "dce5e0 dce5e0 dce5e0 dce5e0 dce5e0 dce5e0 dce5e0",
    ),
    (
        "gradient",
        0,
        "ff0000 c6496d 8c53a2 5147d2 0000ff 8c53a2 8c53a2",
    ),
    (
        "gradient",
        1000,
        "ff0000 c6496d 8c53a2 5147d2 0000ff 8c53a2 8c53a2",
    ),
    (
        "gradient",
        2500,
        "ff0000 c6496d 8c53a2 5147d2 0000ff 8c53a2 8c53a2",
    ),
    (
        "noise",
        0,
        "3d7157 226144 628c75 032113 083321 0d452d 0f4c32",
    ),
    (
        "noise",
        1000,
        "5e8972 175c3e 5f8a73 02170c 031e11 062f1d 0c402a",
    ),
    (
        "noise",
        2500,
        "286548 0e472e 41745a 042315 011209 02190e 4a7b61",
    ),
];

#[test]
fn golden_frames() {
    let room = room();
    let mut actual = Vec::new();
    for (name, effect) in effects() {
        for t in [0, 1000, 2500] {
            let f = effect.render(&room, SceneTime(t));
            assert_eq!(f.lights.len(), 7, "monitors have no lights");
            actual.push((name, t, hex(&f)));
        }
    }
    let printed: Vec<String> = actual
        .iter()
        .map(|(n, t, h)| format!("    (\"{n}\", {t}, \"{h}\"),"))
        .collect();
    let golden: Vec<(&str, i64, String)> = GOLDEN
        .iter()
        .map(|(n, t, h)| (*n, *t, h.to_string()))
        .collect();
    assert_eq!(golden, actual, "golden frames:\n{}", printed.join("\n"));
}

#[test]
fn the_scanline_is_where_the_clock_says() {
    let e = Effect::Scanline {
        colour: RED,
        background: PINE,
        angle_deg: 90.0,
        width_mm: 200.0,
        period_ms: 4000,
    };
    let b = room().bounds;
    // A quarter period in: the band's centre is a quarter of the way along 500 + 200 mm,
    // less half a width: y = 75 mm. Full colour there, background a width away.
    assert_eq!(band_centre(SceneTime(1000), 4000, 500.0, 200.0), 75.0);
    assert_eq!(e.colour(Point::new(300.0, 75.0), SceneTime(1000), &b), RED);
    assert_eq!(
        e.colour(Point::new(300.0, 300.0), SceneTime(1000), &b),
        PINE
    );
    // Two hosts on one epoch draw the same frame at the same moment, whatever came before.
    let ours = Clock::aligned(1_761_000_000_000);
    let wallpaper = Clock::aligned(ours.epoch_unix_ms());
    let now = 1_761_000_001_000;
    let _ = e.render(&room(), ours.at(now + 777));
    assert_eq!(
        e.render(&room(), ours.at(now)),
        e.render(&room(), wallpaper.at(now))
    );
    assert_eq!(ours.at(now), SceneTime(1000));
}

#[test]
fn breathe_and_gradient_hit_their_ends() {
    let b = room().bounds;
    let breathe = Effect::Breathe {
        from: PINE,
        to: Rgb::WHITE,
        period_ms: 4000,
    };
    assert_eq!(breathe.colour(Point::default(), SceneTime(0), &b), PINE);
    assert_eq!(
        breathe.colour(Point::default(), SceneTime(2000), &b),
        Rgb::WHITE
    );
    let g = Effect::Gradient {
        stops: vec![RED, BLUE],
        angle_deg: 0.0,
    };
    assert_eq!(g.colour(Point::new(0.0, 0.0), SceneTime(0), &b), RED);
    assert_eq!(g.colour(Point::new(1000.0, 0.0), SceneTime(99), &b), BLUE);
}

#[test]
fn effects_survive_odd_parameters_and_round_trip() {
    let b = room().bounds;
    let odd = [
        Effect::Scanline {
            colour: RED,
            background: PINE,
            angle_deg: f32::NAN,
            width_mm: -5.0,
            period_ms: 0,
        },
        Effect::Sweep {
            stops: vec![],
            angle_deg: 45.0,
            period_ms: 0,
        },
        Effect::Noise {
            stops: vec![RED],
            scale_mm: 0.0,
            period_ms: 0,
            seed: 0,
        },
    ];
    for e in &odd {
        let _ = e.colour(Point::new(1.0, 2.0), SceneTime(-5), &b);
        let _ = e.colour(Point::new(1.0, 2.0), SceneTime(5), &Rect::default());
    }
    for (_, e) in effects() {
        let text = toml::to_string(&e).unwrap();
        assert_eq!(toml::from_str::<Effect>(&text).unwrap(), e, "{text}");
    }
}
