//! The keyboard's effect vectors (#490): the firmware renders SET_EFFECT on each half
//! (`boards/shields/nice_view_custom/widgets/lamp_effect.c`), and the ZMK simulator holds it to
//! these colours, which come from this crate's own effects at points of the room and moments of
//! the clock. The inputs are in the wire's units (whole mm, hundredths of a degree, tenths of a
//! mm), so both sides start from the same numbers.
//!
//! The file is generated: after a change to the effects, run
//! `FOXGLOVE_UPDATE_VECTORS=1 cargo test -p foxglove-scene --test keyboard_effect_vectors`
//! and let firmware CI's simulator check the C side against it.

use std::fmt::Write as _;
use std::path::PathBuf;

use foxglove_scene::{Effect, Point, Rect, Rgb, SceneTime};

const VECTORS: &str = "tests/zmk-sim/module/effect_vectors.inc";

/// One effect as the wire carries it.
struct Wire {
    kind: u8,
    period_ms: u32,
    angle_cdeg: u16,
    width_dmm: u16,
    origin: [i16; 2],
    colours: Vec<[u8; 3]>,
    room: [i16; 4],
}

impl Wire {
    fn effect(&self) -> Effect {
        let c = |i: usize| Rgb(self.colours[i]);
        let angle_deg = f32::from(self.angle_cdeg) / 100.0;
        let width_mm = f32::from(self.width_dmm) / 10.0;
        let period_ms = self.period_ms;
        match self.kind {
            1 => Effect::Scanline {
                colour: c(0),
                background: c(1),
                angle_deg,
                width_mm,
                period_ms,
            },
            2 => Effect::Sweep {
                stops: self.colours.iter().map(|&c| Rgb(c)).collect(),
                angle_deg,
                period_ms,
            },
            3 => Effect::Ripple {
                origin: Point::new(f32::from(self.origin[0]), f32::from(self.origin[1])),
                colour: c(0),
                background: c(1),
                width_mm,
                period_ms,
            },
            4 => Effect::Breathe {
                from: c(0),
                to: c(1),
                period_ms,
            },
            k => panic!("kind {k}"),
        }
    }

    fn bounds(&self) -> Rect {
        let [x, y, w, h] = self.room.map(f32::from);
        Rect::new(x, y, w, h)
    }
}

// The default room the firmware uses before a PLACE: the halves side by side, 80 mm apart.
const KEYBOARD: [i16; 4] = [0, 0, 328, 86];
// A desk under a 600 x 340 monitor, as the wallpaper vectors have it.
const DESK: [i16; 4] = [-150, 0, 900, 700];

fn cases() -> Vec<(&'static str, Wire)> {
    let orange = [0xf0, 0x8a, 0x3c];
    let night = [0x06, 0x08, 0x0e];
    vec![
        (
            "scanline left to right across the keyboard",
            Wire {
                kind: 1,
                period_ms: 3000,
                angle_cdeg: 0,
                width_dmm: 600,
                origin: [0, 0],
                colours: vec![orange, night],
                room: KEYBOARD,
            },
        ),
        (
            "scanline down the desk, from the monitor to the keys",
            Wire {
                kind: 1,
                period_ms: 4000,
                angle_cdeg: 9000,
                width_dmm: 800,
                origin: [0, 0],
                colours: vec![orange, night],
                room: DESK,
            },
        ),
        (
            "scanline on a slant, a fractional width",
            Wire {
                kind: 1,
                period_ms: 2500,
                angle_cdeg: 3050,
                width_dmm: 1255,
                origin: [0, 0],
                colours: vec![[0x00, 0xff, 0xc8], [0x20, 0x00, 0x40]],
                room: KEYBOARD,
            },
        ),
        (
            "scanline of width 0 draws nothing off its centre",
            Wire {
                kind: 1,
                period_ms: 1000,
                angle_cdeg: 18000,
                width_dmm: 0,
                origin: [0, 0],
                colours: vec![[0xff, 0xff, 0xff], [0x00, 0x00, 0x00]],
                room: KEYBOARD,
            },
        ),
        (
            "sweep of three stops, left to right",
            Wire {
                kind: 2,
                period_ms: 5000,
                angle_cdeg: 0,
                width_dmm: 0,
                origin: [0, 0],
                colours: vec![[0xff, 0x00, 0x00], [0x00, 0xff, 0x00], [0x00, 0x00, 0xff]],
                room: KEYBOARD,
            },
        ),
        (
            "sweep of four stops at 225 degrees on the desk",
            Wire {
                kind: 2,
                period_ms: 7000,
                angle_cdeg: 22500,
                width_dmm: 0,
                origin: [0, 0],
                colours: vec![orange, night, [0xff, 0xff, 0x00], [0x80, 0x00, 0xff]],
                room: DESK,
            },
        ),
        (
            "sweep of one stop is that colour",
            Wire {
                kind: 2,
                period_ms: 1000,
                angle_cdeg: 0,
                width_dmm: 0,
                origin: [0, 0],
                colours: vec![[0x12, 0x34, 0x56]],
                room: KEYBOARD,
            },
        ),
        (
            "ripple from the keyboard's middle",
            Wire {
                kind: 3,
                period_ms: 2000,
                angle_cdeg: 0,
                width_dmm: 400,
                origin: [164, 43],
                colours: vec![[0x00, 0x80, 0xff], [0x00, 0x00, 0x00]],
                room: KEYBOARD,
            },
        ),
        (
            "ripple from a corner of the desk",
            Wire {
                kind: 3,
                period_ms: 6000,
                angle_cdeg: 0,
                width_dmm: 1500,
                origin: [-150, 0],
                colours: vec![orange, night],
                room: DESK,
            },
        ),
        (
            "breathe between two colours",
            Wire {
                kind: 4,
                period_ms: 4000,
                angle_cdeg: 0,
                width_dmm: 0,
                origin: [0, 0],
                colours: vec![[0x10, 0x00, 0x30], [0xff, 0x60, 0x00]],
                room: KEYBOARD,
            },
        ),
        (
            "a still effect (period 0) stands at phase 0",
            Wire {
                kind: 4,
                period_ms: 0,
                angle_cdeg: 0,
                width_dmm: 0,
                origin: [0, 0],
                colours: vec![[0x10, 0x00, 0x30], [0xff, 0x60, 0x00]],
                room: KEYBOARD,
            },
        ),
    ]
}

/// Points in the room: on each half and between them for the keyboard, across the desk else.
fn points(room: [i16; 4]) -> Vec<[f32; 2]> {
    if room == KEYBOARD {
        vec![
            [9.5, 19.0],
            [28.5, 57.25],
            [62.0, 30.5],
            [114.25, 76.5],
            [164.0, 43.0],
            [213.5, 76.5],
            [266.0, 30.5],
            [318.5, 57.25],
        ]
    } else {
        vec![
            [300.0, 150.0],
            [-100.0, 600.0],
            [150.0, 520.0],
            [450.0, 545.5],
            [700.0, 20.0],
            [233.25, 412.75],
        ]
    }
}

/// Moments on the scene clock, a few periods in and far along.
const TIMES: [i64; 6] = [0, 250, 1_337, 2_999, 61_234, 1_000_000_007];

fn generate() -> String {
    let mut out = String::new();
    out.push_str(
        "/* Generated by crates/scene/tests/keyboard_effect_vectors.rs: do not edit. After a change\n \
         * to crates/scene's effects, run\n \
         * FOXGLOVE_UPDATE_VECTORS=1 cargo test -p foxglove-scene --test keyboard_effect_vectors\n \
         * Each: kind, colours used, period ms, angle (0.01 deg), width (0.1 mm), origin x, y,\n \
         * colours, room x, y, w, h (mm), point x, y (mm), scene time ms, the colour crates/scene\n \
         * draws there. */\n",
    );
    for (name, w) in cases() {
        writeln!(out, "/* {name} */").unwrap();
        let effect = w.effect();
        let bounds = w.bounds();
        let mut colours = w.colours.clone();
        colours.resize(4, [0, 0, 0]);
        let rgb = colours
            .iter()
            .map(|c| format!("{{{}, {}, {}}}", c[0], c[1], c[2]))
            .collect::<Vec<_>>()
            .join(", ");
        for p in points(w.room) {
            for t in TIMES {
                let c = effect.colour(Point::new(p[0], p[1]), SceneTime(t), &bounds);
                writeln!(
                    out,
                    "{{{}, {}, {}, {}, {}, {{{}, {}}}, {{{rgb}}}, {{{}, {}, {}, {}}}, {:.2}f, {:.2}f, \
                     {t}ULL, {{{}, {}, {}}}}},",
                    w.kind,
                    w.colours.len(),
                    w.period_ms,
                    w.angle_cdeg,
                    w.width_dmm,
                    w.origin[0],
                    w.origin[1],
                    w.room[0],
                    w.room[1],
                    w.room[2],
                    w.room[3],
                    p[0],
                    p[1],
                    c.0[0],
                    c.0[1],
                    c.0[2],
                )
                .unwrap();
            }
        }
    }
    out
}

#[test]
fn the_firmware_vectors_are_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
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
        "{VECTORS} is out of date: run FOXGLOVE_UPDATE_VECTORS=1 cargo test -p foxglove-scene \
         --test keyboard_effect_vectors"
    );
}

#[test]
fn the_vectors_cover_every_effect_the_keyboard_draws() {
    let text = generate();
    for kind in 1..=4 {
        assert!(
            text.lines().any(|l| l.starts_with(&format!("{{{kind}, "))),
            "kind {kind}"
        );
    }
    // Not all one colour: the band, the sweep and the ring move.
    let distinct: std::collections::BTreeSet<&str> = text
        .lines()
        .filter_map(|l| l.rsplit_once("ULL, ").map(|(_, c)| c))
        .collect();
    assert!(distinct.len() > 40, "{}", distinct.len());
}
