//! Screen glow on recorded synthetic frames: no screen is ever captured here.

use super::*;
use crate::room::{DeviceInfo, Layout, LightSpec, MonitorInfo};

const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 210, 40];
const BLUE: [u8; 3] = [20, 40, 230];
const YELLOW: [u8; 3] = [240, 220, 30];

/// One 1920 x 1080 screen at 96 DPI (508 x 286 mm), the keyboard's halves below it and the case
/// on its right.
fn room() -> RoomMap {
    let monitors = [MonitorInfo {
        id: r"\\.\DISPLAY1".into(),
        primary: true,
        rect_px: [0, 0, 1920, 1080],
        raw_dpi: Some(96),
        dpi: 96,
    }];
    let case = DeviceInfo {
        lights: vec![
            LightSpec::point("top", Some(Point::new(100.0, 40.0))),
            LightSpec::point("bottom", Some(Point::new(100.0, 430.0))),
        ],
        ..DeviceInfo::whole("case", "Case", DeviceKind::Case)
    };
    let devices = [
        DeviceInfo::whole("keyboard:left", "Left", DeviceKind::KeyboardLeft),
        DeviceInfo::whole("keyboard:right", "Right", DeviceKind::KeyboardRight),
        case,
    ];
    RoomMap::build(&monitors, &devices, &Layout::default())
}

/// A recorded frame: the screen in quarters, red top left, green top right, blue bottom left,
/// yellow bottom right (192 x 108, the capture's downscale of 1920 x 1080).
fn quarters() -> ScreenImage {
    let mut img = ScreenImage::filled(r"\\.\DISPLAY1", 192, 108, RED);
    img.paint((96, 0, 192, 54), GREEN);
    img.paint((0, 54, 96, 108), BLUE);
    img.paint((96, 54, 192, 108), YELLOW);
    img
}

fn close(a: Rgb, b: [u8; 3]) -> bool {
    a.0.iter()
        .zip(b)
        .all(|(x, y)| (i32::from(*x) - i32::from(y)).abs() <= 12)
}

fn light(f: &Frame, device: &str, light: &str) -> Rgb {
    f.lights
        .iter()
        .find(|l| l.device == device && l.light == light)
        .unwrap_or_else(|| panic!("{device}/{light} in {f:?}"))
        .colour
}

#[test]
fn edge_colours_map_to_the_room_maps_devices() {
    let room = room();
    let edges = [Edges::of(&quarters())];
    let mut glow = Glow::new(Smoothing::Quick);
    let f = glow.render(&room, &edges, SceneTime(0));
    // The halves sit below the screen: its bottom edge, the left half the blue part, the
    // right half the yellow part.
    assert!(close(light(&f, "keyboard:left", "all"), BLUE), "{f:?}");
    assert!(close(light(&f, "keyboard:right", "all"), YELLOW), "{f:?}");
    // The case stands to the right: its right edge, top to bottom.
    assert!(close(light(&f, "case", "top"), GREEN), "{f:?}");
    assert!(close(light(&f, "case", "bottom"), YELLOW), "{f:?}");
    // Which edge, for the map's lines.
    let left = room.device("keyboard:left").unwrap().lights[0].at;
    let spot = nearest_edge(&room, left).unwrap();
    assert_eq!(spot.edge, Edge::Bottom);
    assert_eq!(spot.monitor, r"monitor:\\.\DISPLAY1");
    assert!(spot.along < 0.5);
    let case = room.device("case").unwrap().lights[0].at;
    assert_eq!(nearest_edge(&room, case).unwrap().edge, Edge::Right);
}

#[test]
fn a_device_far_from_every_screen_still_follows_the_nearest() {
    let room = room();
    let far = Point::new(-5000.0, 3000.0);
    let spot = nearest_edge(&room, far).unwrap();
    assert_eq!(spot.monitor, r"monitor:\\.\DISPLAY1");
    assert!(colour_at(&room, &[Edges::of(&quarters())], far).is_some());
    // No screen in the room: nothing to follow.
    let empty = RoomMap::build(&[], &[], &Layout::default());
    assert!(nearest_edge(&empty, far).is_none() || empty.devices.is_empty());
}

#[test]
fn black_bars_are_skipped() {
    // A letterboxed film: black bars top and bottom (12 rows each), the picture red on top and
    // blue below; pillarboxed sides too (8 columns).
    let mut img = ScreenImage::filled(r"\\.\DISPLAY1", 192, 108, [0, 0, 0]);
    img.paint((8, 12, 184, 54), RED);
    img.paint((8, 54, 184, 96), BLUE);
    assert_eq!(picture(&img), (8, 12, 184, 96));
    let e = Edges::of(&img);
    assert!(e.top.iter().all(|c| close(*c, RED)), "{:?}", e.top);
    assert!(e.bottom.iter().all(|c| close(*c, BLUE)), "{:?}", e.bottom);
    // Without skipping, the top strip would be the bar's black.
    assert!(!close(e.top[0], [0, 0, 0]));
    // A fully black screen has no bars: it is all picture, and the lights go dark.
    let dark = ScreenImage::filled(r"\\.\DISPLAY1", 64, 36, [0, 0, 0]);
    assert_eq!(picture(&dark), (0, 0, 64, 36));
    assert!(Edges::of(&dark).top.iter().all(|c| *c == Rgb::BLACK));
    // A dark (not black) scene is no bar.
    let dim = ScreenImage::filled(r"\\.\DISPLAY1", 64, 36, [30, 30, 30]);
    assert_eq!(picture(&dim), (0, 0, 64, 36));
}

#[test]
fn the_strip_reaches_a_tenth_into_the_picture() {
    // A thin white frame of 4 rows on a 108-row picture: the 11-row strip averages it with the
    // red inside, so it is neither pure white nor pure red.
    let mut img = ScreenImage::filled(r"\\.\DISPLAY1", 192, 108, RED);
    img.paint((0, 0, 192, 4), [255, 255, 255]);
    let top = Edges::of(&img).top[5];
    assert!(!close(top, RED) && !close(top, [255, 255, 255]), "{top:?}");
}

/// The time a light takes to come within 10 % (in linear light) of a new colour.
fn settle_time(smoothing: Smoothing) -> i64 {
    let room = room();
    let red = [Edges::of(&ScreenImage::filled(
        r"\\.\DISPLAY1",
        64,
        36,
        RED,
    ))];
    let blue = [Edges::of(&ScreenImage::filled(
        r"\\.\DISPLAY1",
        64,
        36,
        BLUE,
    ))];
    let mut glow = Glow::new(smoothing);
    glow.render(&room, &red, SceneTime(0));
    let target = Rgb(BLUE).linear();
    let mut t = 0;
    loop {
        t += MIN_UPDATE_MS;
        let f = glow.render(&room, &blue, SceneTime(t));
        let c = light(&f, "keyboard:left", "all").linear();
        if c.iter().zip(target).all(|(a, b)| (a - b).abs() < 0.1) {
            return t;
        }
        assert!(t < 10_000, "{smoothing:?} never settles");
    }
}

#[test]
fn smoothing_quick_even_calm_settle_in_that_order() {
    let quick = settle_time(Smoothing::Quick);
    let even = settle_time(Smoothing::Even);
    let calm = settle_time(Smoothing::Calm);
    assert!(quick <= 100, "Quick follows every cut: {quick} ms");
    assert!((150..=450).contains(&even), "Even about 0.3 s: {even} ms");
    assert!((700..=1500).contains(&calm), "Calm about 1 s: {calm} ms");
    assert!(quick < even && even < calm);
    assert_eq!(Smoothing::default(), Smoothing::Even);
    assert_eq!(Smoothing::default_for(true), Smoothing::Even);
    assert_eq!(Smoothing::default_for(false), Smoothing::Calm);
}

#[test]
fn at_most_twenty_updates_a_second() {
    let mut glow = Glow::new(Smoothing::Even);
    let room = room();
    let mut sent = 0;
    // The scene's frame thread ticks at 30 a second for a second.
    for i in 0..30 {
        let t = SceneTime(i * 1000 / 30);
        if glow.due(t) {
            glow.render(&room, &[], t);
            sent += 1;
        }
    }
    assert!(
        sent <= MAX_UPDATES_PER_S as usize,
        "{sent} updates in a second"
    );
    assert!(sent >= 14, "still follows the screen: {sent}");
}

/// Swings between dark and bright in the shown colours, with the times they happened.
fn swings(colours: &[(i64, Rgb)]) -> Vec<i64> {
    let mut out = Vec::new();
    let (mut dir, mut extreme) = (0i8, luminance(colours[0].1));
    for &(t, c) in &colours[1..] {
        let y = luminance(c);
        let d = y - extreme;
        let way = if d > 0.0 { 1 } else { -1 };
        if dir == 0 {
            if d.abs() >= SWING {
                dir = way;
                extreme = y;
            }
        } else if way == dir {
            if (way > 0 && y > extreme) || (way < 0 && y < extreme) {
                extreme = y;
            }
        } else if d.abs() >= SWING {
            out.push(t);
            dir = way;
            extreme = y;
        }
    }
    out
}

#[test]
fn a_strobe_never_flashes_more_than_three_times_a_second() {
    let room = room();
    let white = [Edges::of(&ScreenImage::filled(
        r"\\.\DISPLAY1",
        64,
        36,
        [255; 3],
    ))];
    let black = [Edges::of(&ScreenImage::filled(
        r"\\.\DISPLAY1",
        64,
        36,
        [0; 3],
    ))];
    for smoothing in Smoothing::ALL {
        let mut glow = Glow::new(smoothing);
        let mut shown = Vec::new();
        // A 10 Hz strobe for 5 s, sampled at the 20 updates a second.
        for i in 0..100 {
            let t = i * MIN_UPDATE_MS;
            let edges = if (t / 50) % 2 == 0 { &white } else { &black };
            let f = glow.render(&room, edges, SceneTime(t));
            shown.push((t, light(&f, "keyboard:left", "all")));
        }
        let s = swings(&shown);
        for (i, &at) in s.iter().enumerate() {
            let in_second = s[i..].iter().take_while(|&&b| b - at < 1000).count();
            assert!(
                in_second <= MAX_SWINGS,
                "{smoothing:?}: {in_second} swings within a second from {at} ms ({s:?})"
            );
        }
        if smoothing == Smoothing::Quick {
            // It still moves: a steady pulse, not a frozen light.
            assert!(s.len() >= 6, "{smoothing:?}: {s:?}");
        }
    }
}

#[test]
fn a_slow_change_passes_the_flash_limit_untouched() {
    let mut limit = FlashLimit::default();
    // A fade from black to white over two seconds, then back.
    for i in 0..=40 {
        let v = (i * 255 / 40) as u8;
        assert_eq!(limit.apply(Rgb([v; 3]), i * 50), Rgb([v; 3]));
    }
    for i in 0..=40 {
        let v = 255 - (i * 255 / 40) as u8;
        assert_eq!(limit.apply(Rgb([v; 3]), 2050 + i * 50), Rgb([v; 3]));
    }
    assert!(limit.recent() <= 1);
}

#[test]
fn a_screen_not_read_keeps_the_lights_where_they_were() {
    let room = room();
    let mut glow = Glow::new(Smoothing::Quick);
    let f = glow.render(&room, &[Edges::of(&quarters())], SceneTime(0));
    let before = light(&f, "keyboard:left", "all");
    // Another monitor's edges only (the first screen wasn't read this time).
    let other = Edges {
        monitor: r"\\.\DISPLAY9".into(),
        ..Edges::of(&quarters())
    };
    let f = glow.render(&room, &[other], SceneTime(500));
    assert_eq!(light(&f, "keyboard:left", "all"), before);
}

#[test]
fn the_effect_is_a_still_grey_before_anything_is_read() {
    let e = crate::Effect::Screen {
        smoothing: Smoothing::Calm,
    };
    let room = room();
    let a = e.render(&room, SceneTime(0));
    let b = e.render(&room, SceneTime(5000));
    assert_eq!(a.lights, b.lights);
    assert!(a
        .lights
        .iter()
        .all(|l| l.colour == crate::effects::SCREEN_IDLE));
    // Saved looks keep the smoothing.
    let text = serde_json::to_string(&e).unwrap();
    assert!(
        text.contains("\"screen\"") && text.contains("calm"),
        "{text}"
    );
    assert_eq!(serde_json::from_str::<crate::Effect>(&text).unwrap(), e);
}

// ------------------------------------------------------------------ handing back (#1085)

fn filled(c: [u8; 3]) -> [Edges; 1] {
    [Edges::of(&ScreenImage::filled(r"\\.\DISPLAY1", 64, 36, c))]
}

fn key(device: &str, light: &str) -> LightKey {
    (device.into(), light.into())
}

/// Linear-light distance between two colours (the largest channel's).
fn apart(a: Rgb, b: Rgb) -> f32 {
    let (a, b) = (a.linear(), b.linear());
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
}

/// A glow settled on a red screen by `t` ms.
fn settled_on_red(smoothing: Smoothing, t: i64) -> (RoomMap, Glow) {
    let room = room();
    let mut glow = Glow::new(smoothing);
    let mut at = 0;
    while at <= t {
        glow.render(&room, &filled(RED), SceneTime(at));
        at += MIN_UPDATE_MS;
    }
    (room, glow)
}

#[test]
fn the_fade_curve_eases_in_and_out() {
    assert_eq!(fade_curve(0.0), 0.0);
    assert_eq!(fade_curve(1.0), 1.0);
    assert!((fade_curve(0.5) - 0.5).abs() < 1e-6);
    assert_eq!(fade_curve(-1.0), 0.0);
    assert_eq!(fade_curve(7.0), 1.0);
    assert_eq!(fade_curve(f32::NAN), 1.0);
    let steps: Vec<f32> = (0..=20).map(|i| fade_curve(i as f32 / 20.0)).collect();
    assert!(steps.windows(2).all(|w| w[1] >= w[0]), "never turns back");
    // Slow at both ends, quickest in the middle.
    let first = steps[1] - steps[0];
    let middle = steps[11] - steps[10];
    let last = steps[20] - steps[19];
    assert!(first < middle / 4.0 && last < middle / 4.0, "{steps:?}");
}

/// A full-screen game: each light fades over 1 s from its screen colour to its own, along the
/// curve, ignoring the screen meanwhile; a light whose own colour isn't known keeps its colour.
#[test]
fn a_game_fades_the_lights_to_their_own_colours_in_one_second() {
    let (room, mut glow) = settled_on_red(Smoothing::Even, 3000);
    let t0 = 3050;
    let own = BTreeMap::from([
        (key("keyboard:left", "all"), Rgb(BLUE)),
        (key("case", "top"), Rgb(GREEN)),
        (key("case", "bottom"), Rgb(GREEN)),
    ]);
    glow.hand_back(HandBack::Game, own, SceneTime(t0));
    assert!(glow.handing_back() && !glow.settled());
    assert_eq!(
        glow.fade_starts(SceneTime(t0)),
        None,
        "a game doesn't hold first"
    );
    let at = |glow: &mut Glow, ms: i64| {
        // The screen turns yellow meanwhile: ignored while handing back.
        glow.render(&room, &filled(YELLOW), SceneTime(t0 + ms))
    };
    let f = at(&mut glow, 0);
    assert!(
        close(light(&f, "keyboard:left", "all"), RED),
        "starts where it was"
    );
    let f = at(&mut glow, 500);
    let mid = light(&f, "keyboard:left", "all").linear();
    let (r, b) = (Rgb(RED).linear(), Rgb(BLUE).linear());
    for i in 0..3 {
        let half = (r[i] + b[i]) / 2.0;
        assert!(
            (mid[i] - half).abs() < 0.03,
            "half way in linear light at 0.5 s: {mid:?}"
        );
    }
    assert!(!glow.handed_back(SceneTime(t0 + 500)));
    let f = at(&mut glow, 1000);
    assert_eq!(light(&f, "keyboard:left", "all"), Rgb(BLUE));
    assert_eq!(light(&f, "case", "top"), Rgb(GREEN));
    assert!(
        close(light(&f, "keyboard:right", "all"), RED),
        "no own colour: kept"
    );
    assert!(glow.handed_back(SceneTime(t0 + 1000)));
    // A second hand-back while handing back changes nothing.
    glow.hand_back(HandBack::Failed, BTreeMap::new(), SceneTime(t0 + 1000));
    assert_eq!(glow.fade_starts(SceneTime(t0 + 1000)), None);
}

/// Capture failing: the last colours hold for [`FAIL_HOLD_MS`], then fade over 3 s.
#[test]
fn a_failure_holds_then_fades_in_three_seconds() {
    let (room, mut glow) = settled_on_red(Smoothing::Calm, 4000);
    let t0 = 4050;
    let own = BTreeMap::from([(key("keyboard:left", "all"), Rgb(BLUE))]);
    glow.hand_back(HandBack::Failed, own, SceneTime(t0));
    let before = light(
        &glow.render(&room, &[], SceneTime(t0)),
        "keyboard:left",
        "all",
    );
    assert_eq!(
        glow.fade_starts(SceneTime(t0 + 10)),
        Some(SceneTime(t0 + FAIL_HOLD_MS))
    );
    for ms in [500, 1000, 1999] {
        let f = glow.render(&room, &[], SceneTime(t0 + ms));
        assert_eq!(light(&f, "keyboard:left", "all"), before, "held at {ms} ms");
    }
    let start = t0 + FAIL_HOLD_MS;
    assert_eq!(glow.fade_starts(SceneTime(start)), None);
    let f = glow.render(&room, &[], SceneTime(start + 1500));
    let mid = light(&f, "keyboard:left", "all");
    assert!(
        apart(mid, before) > 0.2 && apart(mid, Rgb(BLUE)) > 0.2,
        "{mid:?}"
    );
    assert!(!glow.handed_back(SceneTime(start + 2999)));
    let f = glow.render(&room, &[], SceneTime(start + FAIL_FADE_MS));
    assert_eq!(light(&f, "keyboard:left", "all"), Rgb(BLUE));
    assert!(glow.handed_back(SceneTime(start + FAIL_FADE_MS)));
}

/// The hold ends: the lights ease back to the screen from their own colours at the
/// smoothing's pace, never in one jump, however long the hold was.
#[test]
fn after_a_hold_the_lights_ease_back_to_the_screen() {
    for smoothing in Smoothing::ALL {
        let (room, mut glow) = settled_on_red(smoothing, 3000);
        let own = BTreeMap::from([(key("keyboard:left", "all"), Rgb(BLUE))]);
        glow.hand_back(HandBack::Game, own, SceneTime(3050));
        glow.render(&room, &[], SceneTime(4050));
        assert!(glow.handed_back(SceneTime(4050)));
        // A minute later the game ends and the screen is green.
        glow.resume();
        assert!(!glow.handing_back());
        let mut t = 64_050;
        let f = glow.render(&room, &filled(GREEN), SceneTime(t));
        let first = light(&f, "keyboard:left", "all");
        assert!(
            apart(first, Rgb(BLUE)) <= apart(Rgb(BLUE), Rgb(GREEN)) * 0.97,
            "{smoothing:?}: one step at most from its own colour: {first:?}"
        );
        if smoothing != Smoothing::Quick {
            assert!(
                apart(first, Rgb(GREEN)) > 0.1,
                "{smoothing:?}: no jump to the screen: {first:?}"
            );
        }
        let mut c = first;
        while apart(c, Rgb(GREEN)) > 0.01 {
            t += MIN_UPDATE_MS;
            c = light(
                &glow.render(&room, &filled(GREEN), SceneTime(t)),
                "keyboard:left",
                "all",
            );
            assert!(t < 64_050 + 10_000, "{smoothing:?} never got back");
        }
    }
}

/// The flash limit holds during a fade too: a fade right after a strobe waits for it.
#[test]
fn a_fade_keeps_the_flash_limit() {
    let room = room();
    let mut glow = Glow::new(Smoothing::Quick);
    let mut shown = Vec::new();
    // A 10 Hz strobe for a second, ending dark ...
    let mut t = 0;
    while t < 1000 {
        let edges = if (t / 50) % 2 == 0 {
            filled([255; 3])
        } else {
            filled([0; 3])
        };
        let f = glow.render(&room, &edges, SceneTime(t));
        shown.push((t, light(&f, "keyboard:left", "all")));
        t += MIN_UPDATE_MS;
    }
    // ... then a game: the own colour is white, the other way.
    let own = BTreeMap::from([(key("keyboard:left", "all"), Rgb([255; 3]))]);
    glow.hand_back(HandBack::Game, own, SceneTime(t));
    let end = t + GAME_FADE_MS;
    loop {
        let f = glow.render(&room, &[], SceneTime(t));
        shown.push((t, light(&f, "keyboard:left", "all")));
        if glow.handed_back(SceneTime(t)) {
            break;
        }
        t += MIN_UPDATE_MS;
        assert!(t < end + 2000, "the fade ends once the limit lets it");
    }
    let s = swings(&shown);
    for (i, &at) in s.iter().enumerate() {
        let in_second = s[i..].iter().take_while(|&&b| b - at < 1000).count();
        assert!(
            in_second <= MAX_SWINGS,
            "{in_second} swings from {at} ms ({s:?})"
        );
    }
    assert_eq!(shown.last().unwrap().1, Rgb([255; 3]));
}

/// #1085: a still picture settles (nothing more to send, the frame thread may sleep); a new
/// one doesn't until the lights reach it. A step after a long gap is one ordinary step.
#[test]
fn a_still_screen_settles() {
    let room = room();
    let mut glow = Glow::new(Smoothing::Even);
    let red = filled(RED);
    glow.render(&room, &red, SceneTime(0));
    assert!(glow.settled(), "the first picture shows at once");
    let mut t = 0;
    for _ in 0..3 {
        t += MIN_UPDATE_MS;
        glow.render(&room, &red, SceneTime(t));
        assert!(glow.settled());
    }
    // Ten seconds still, then blue: one step of Even, not the whole way.
    t += 10_000;
    let f = glow.render(&room, &filled(BLUE), SceneTime(t));
    assert!(!glow.settled());
    let k = 1.0 - (-(MAX_STEP_MS as f32) / (Smoothing::Even.settle_ms() / 3.0)).exp();
    let c = light(&f, "keyboard:left", "all").linear();
    let (r, b) = (Rgb(RED).linear(), Rgb(BLUE).linear());
    for i in 0..3 {
        let want = r[i] + (b[i] - r[i]) * k;
        assert!((c[i] - want).abs() < 0.01, "{c:?} vs {want}");
    }
    let mut n = 0;
    while !glow.settled() {
        t += MIN_UPDATE_MS;
        glow.render(&room, &filled(BLUE), SceneTime(t));
        n += 1;
        assert!(n < 100, "Even settles in a few seconds");
    }
    assert_eq!(glow.shown()[&key("keyboard:left", "all")], Rgb(BLUE));
}
