//! A per-key scene on a zone board, end to end (#611): the Corne's profile as it ships (6 column
//! zones per half, 15 lamps per half that light nothing, #608), the room the scenes build from it
//! (scene_room: one light per zone at its keys' centre), the frame the scene output makes
//! (scene_output::keyboard_frame) and the SET_FRAME reports the app sends for it (Foxglove link's
//! builder). Each zone's lamp must carry the scene's colour at the zone's centre, and no dead lamp
//! may be lit. The ZMK simulator (tests/zmk-sim/host-zones) plays the same reports through the
//! real host_frames.c and holds what both halves draw to the same colours.
//!
//! The file is generated: after a change to the profile's map, the room, the scene output or
//! the effects, run
//! `FOXGLOVE_UPDATE_VECTORS=1 cargo test --manifest-path app/Cargo.toml keyboard_zone`
//! and let firmware CI's simulator check the C side against it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use foxglove_scene::{Effect, MonitorInfo, Point, Rgb, RoomMap, SceneTime};

use super::boards::Board;
use super::scene_output::keyboard_frame;
use super::scene_room::KEY_MM;

const VECTORS: &str = "tests/zmk-sim/module/zone_vectors.inc";

/// SET_FRAME reports per frame of the Corne's 42 lamps (9 lamps a report).
const REPORTS: usize = 5;

const BLACK: Rgb = Rgb([0, 0, 0]);

/// The Corne's profile as it ships: its zone map, not a test's.
fn board() -> Board {
    super::fixtures::users_desk::board()
}

/// The user's desk: a 1440p monitor and the Corne below it.
fn room(board: &Board) -> RoomMap {
    let devices = super::scene_room::devices(Some(board), &[], &BTreeMap::new());
    let monitors = [MonitorInfo {
        id: r"\\.\DISPLAY1".into(),
        primary: true,
        rect_px: [0, 0, 2560, 1440],
        raw_dpi: Some(109),
        dpi: 96,
    }];
    RoomMap::build(&monitors, &devices, &Default::default())
}

/// Per-key looks with a colour that changes across the board, at one moment each.
fn looks() -> Vec<(&'static str, Effect, i64)> {
    let stops = vec![Rgb([255, 0, 0]), Rgb([0, 255, 0]), Rgb([0, 0, 255])];
    vec![
        (
            "gradient left to right",
            Effect::Gradient {
                stops: stops.clone(),
                angle_deg: 0.0,
            },
            0,
        ),
        (
            "gradient top to bottom",
            Effect::Gradient {
                stops: stops.clone(),
                angle_deg: 90.0,
            },
            0,
        ),
        (
            "sweep, a third of the way round",
            Effect::Sweep {
                stops: stops.clone(),
                angle_deg: 0.0,
                period_ms: 6000,
            },
            2000,
        ),
        (
            "sweep, two thirds of the way round",
            Effect::Sweep {
                stops,
                angle_deg: 0.0,
                period_ms: 6000,
            },
            4000,
        ),
    ]
}

/// What each lamp should show, worked out from the profile's map alone (not from the room's
/// lights): a zone's lamp the effect's colour at the centre of its keys, a dead lamp black.
fn expected(board: &Board, room: &RoomMap, effect: &Effect, t: SceneTime) -> Vec<Rgb> {
    let l = board.lighting.as_ref().expect("lighting");
    let parts = l.lamp_parts(board.halves).expect("parts");
    let drawing = super::boards::drawing(Some(board));
    let mut out = Vec::new();
    for (p, id) in ["keyboard:left", "keyboard:right"].iter().enumerate() {
        let half = room.devices.iter().find(|d| d.id == *id).expect("a half");
        let keys = drawing.keys(p == 1);
        for lamp in l.part_keys(&parts, p).expect("a map per half") {
            if lamp.dead() {
                out.push(BLACK);
                continue;
            }
            let n = lamp.keys().len() as f32;
            let (sx, sy) = lamp.keys().iter().fold((0.0, 0.0), |(x, y), &k| {
                let c = keys[usize::from(k)].center();
                (x + c.x, y + c.y)
            });
            let at = Point::new(half.rect.x + sx / n * KEY_MM, half.rect.y + sy / n * KEY_MM);
            out.push(effect.colour(at, t, &room.bounds));
        }
    }
    out
}

/// The frame the scene output makes for `effect` at `t`, as SET_FRAME colours.
fn frame(board: &Board, room: &RoomMap, effect: &Effect, t: SceneTime) -> Vec<Rgb> {
    let parts = super::boards::lamp_parts(Some(board));
    let rendered = effect.render(room, t);
    let lights: Vec<(&str, &str, Rgb)> = rendered
        .lights
        .iter()
        .filter(|l| l.device.starts_with("keyboard"))
        .map(|l| (l.device.as_str(), l.light.as_str(), l.colour))
        .collect();
    keyboard_frame(&lights, &parts).expect("both halves").lamps
}

fn bytes(b: &[u8]) -> String {
    b.iter().map(u8::to_string).collect::<Vec<_>>().join(", ")
}

fn generate() -> String {
    let board = board();
    let room = room(&board);
    let mut out = String::new();
    out.push_str(
        "/* Generated by app/src/foxglove/keyboard_zone_vectors.rs: do not edit. After a change\n \
         * to the board profile's LED map, the room, the scene output or the effects, run\n \
         * FOXGLOVE_UPDATE_VECTORS=1 cargo test --manifest-path app/Cargo.toml keyboard_zone\n \
         * Each: the look, the SET_FRAME reports the app sends for it (32 bytes each), and the\n \
         * colour each lamp must show (left 0-20, right 21-41): its zone's, or black. */\n",
    );
    for (name, effect, t) in looks() {
        let t = SceneTime(t);
        let lamps = frame(&board, &room, &effect, t);
        let colours: Vec<[u8; 3]> = lamps.iter().map(|c| c.0).collect();
        let reports = foxglove_link::protocol::frame(0, &colours);
        assert_eq!(reports.len(), REPORTS, "{name}");
        let reports = reports
            .iter()
            .map(|r| {
                let mut r = r.clone();
                r.resize(32, 0);
                format!("{{{}}}", bytes(&r))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let want = lamps
            .iter()
            .map(|c| format!("{{{}}}", bytes(&c.0)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "{{\"{name}\", {{{reports}}}, {{{want}}}}},").unwrap();
    }
    out
}

#[test]
fn keyboard_zone_vectors_are_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(VECTORS);
    let want = generate();
    if std::env::var_os("FOXGLOVE_UPDATE_VECTORS").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        have == want,
        "{VECTORS} is out of date: run FOXGLOVE_UPDATE_VECTORS=1 cargo test --manifest-path \
         app/Cargo.toml keyboard_zone"
    );
}

/// Every zone's lamp carries the scene's colour at the zone's centre, and the lamps that light
/// nothing stay black, for every look (#611's acceptance: "the frame equals the expected
/// per-zone colours, and no frame writes dead lamps").
#[test]
fn keyboard_zone_frames_carry_each_zones_colour_and_no_dead_lamp() {
    let board = board();
    let room = room(&board);
    let zones = super::light_zones::from_profile(&board, &super::boards::drawing(Some(&board)))
        .expect("the profile's zones");
    assert_eq!(
        (zones.live(), zones.dead.clone()),
        (vec![6, 6], vec![15, 15])
    );
    for (name, effect, t) in looks() {
        let t = SceneTime(t);
        let got = frame(&board, &room, &effect, t);
        let want = expected(&board, &room, &effect, t);
        assert_eq!(got.len(), 42, "{name}");
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert!(
                g.0.iter().zip(w.0).all(|(a, b)| a.abs_diff(b) <= 1),
                "{name}: lamp {i} is {g:?}, its zone's colour is {w:?}"
            );
        }
        let dead = want.iter().filter(|c| **c == BLACK).count();
        assert_eq!(
            dead, 30,
            "{name}: the 30 dead lamps are black, the 12 zones lit"
        );
    }
}

/// A gradient across the board gives each column zone its own colour on both halves: the frame
/// is per zone, not one colour per half.
#[test]
fn keyboard_zone_frames_differ_zone_by_zone() {
    let board = board();
    let room = room(&board);
    let (_, effect, t) = looks().remove(0);
    let got = frame(&board, &room, &effect, SceneTime(t));
    for half in [&got[0..6], &got[21..27]] {
        let distinct: std::collections::BTreeSet<[u8; 3]> = half.iter().map(|c| c.0).collect();
        assert_eq!(distinct.len(), 6, "{half:?}");
    }
}
