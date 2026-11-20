//! Window-free tests of the Lights page's live drawing.

use super::*;
use crate::foxglove::lights_live::{look, Report};
use foxglove_link::lights_owner::Owner;

pub(crate) fn half(on: bool, effect: u8, hue: u16, brightness: u8) -> RgbHalf {
    RgbHalf {
        on,
        hue,
        sat: 100,
        brightness,
        effect,
        speed: 3,
    }
}

pub(crate) fn rgb(left: RgbHalf, right: Option<RgbHalf>) -> RgbState {
    RgbState {
        left,
        right: right.map(|h| (h, 0)),
        colour_effects: Some(0x0B),
    }
}

fn report(state: Option<RgbState>) -> Report {
    Report {
        connected: true,
        rgb: state,
        dim: [100, 100],
        windows: None,
        right_apart: false,
    }
}

fn parts() -> Parts {
    crate::foxglove::fixtures::users_desk::board().parts()
}

pub(crate) fn layout() -> KeyLayout {
    crate::foxglove::key_layout::resolve(
        Some(&crate::foxglove::fixtures::users_desk::board()),
        &[],
        0,
    )
}

pub(crate) fn own_look(s: RgbState) -> Look {
    look(&report(Some(s)), None, None, None, None)
}

pub(crate) fn live_of(look: Look, rgb: Option<&RgbState>, animations: bool) -> Live {
    let (line, name) = words(&look, rgb, [100, 100], &parts(), animations);
    Live {
        look,
        layout: layout(),
        clock: None,
        line,
        name,
        animations,
    }
}

fn say(state: Option<RgbState>, owner: Option<Owner>) -> (String, String) {
    let l = look(&report(state), owner, None, None, None);
    words(&l, state.as_ref(), [100, 100], &parts(), true)
}

#[test]
fn the_line_and_name_for_each_state() {
    // Orange is hue 26; Swirl (3) is in colour with 0x0B, Spectrum (2) is its own colours.
    let swirl = rgb(half(true, 3, 26, 60), None);
    assert_eq!(
        say(Some(swirl), None),
        (
            "Swirl · Orange · 60%".into(),
            "Keyboard lights: Swirl, orange, 60 percent".into()
        )
    );
    let spectrum = rgb(half(true, 2, 26, 60), None);
    assert_eq!(
        say(Some(spectrum), None).0,
        "Spectrum · its own colours · 60%"
    );
    let off = rgb(half(false, 0, 0, 60), None);
    assert_eq!(
        say(Some(off), None),
        ("Lights off".into(), "Keyboard lights off".into())
    );
    let two = rgb(half(true, 1, 26, 60), Some(half(true, 0, 220, 40)));
    let (line, name) = say(Some(two), None);
    assert_eq!(
        line,
        "Left: Breathe · Orange · 60%   Right: Solid · Blue · 40%"
    );
    assert!(name.starts_with("Keyboard lights: left breathe, orange, 60 percent"));
    assert_eq!(
        say(Some(swirl), Some(Owner::Other(7))).0,
        "Another app sets these lights. Foxglove can't see what it shows."
    );
    let cannot = rgb(half(true, 7, 26, 60), None);
    assert_eq!(
        say(Some(cannot), None),
        (
            "Foxglove can't draw this effect here.".into(),
            "Keyboard lights: effect 7, orange".into()
        )
    );
    assert_eq!(say(None, None).0, "Reading the keyboard's lights…");
    let gone = look(
        &Report {
            connected: false,
            ..report(None)
        },
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        words(&gone, None, [100, 100], &parts(), true).0,
        "Keyboard not connected. Its lights show here when it's back."
    );
}

#[test]
fn a_moving_look_says_when_animation_effects_are_off() {
    let s = rgb(half(true, 3, 26, 60), None);
    let l = own_look(s);
    assert!(l.next_frame().is_some());
    let (line, _) = words(&l, Some(&s), [100, 100], &parts(), false);
    assert_eq!(
        line,
        "Swirl · Orange · 60% · still (Windows animation effects are off)"
    );
    // A still look has nothing to say about it.
    let solid = rgb(half(true, 0, 26, 60), None);
    let l = own_look(solid);
    assert!(!words(&l, Some(&solid), [100, 100], &parts(), false)
        .0
        .contains("still"));
}

#[test]
fn the_hover_adds_the_pace_line_only_for_moving_looks() {
    let moving = own_look(rgb(half(true, 3, 26, 60), None));
    let still = own_look(rgb(half(true, 0, 26, 60), None));
    assert!(about(&moving).ends_with("The pace matches the keyboard; the exact moment may differ."));
    assert_eq!(about(&still), ABOUT);
}

#[test]
fn a_frame_is_asked_only_when_nobody_would_miss_it() {
    let moving = own_look(rgb(half(true, 3, 26, 60), None));
    let still = own_look(rgb(half(true, 0, 26, 60), None));
    assert_eq!(
        frame_request(&moving, true, false, false, true),
        Some(Duration::from_millis(50))
    );
    for (front, min, away, anim) in [
        (false, false, false, true),
        (true, true, false, true),
        (true, false, true, true),
        (true, false, false, false),
    ] {
        assert_eq!(frame_request(&moving, front, min, away, anim), None);
    }
    assert_eq!(frame_request(&still, true, false, false, true), None);
}

fn run(ctx: &egui::Context, live: &Live, focused: bool, minimised: bool) {
    let viewport = egui::ViewportInfo {
        minimized: Some(minimised),
        ..Default::default()
    };
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 400.0),
        )),
        focused,
        ..Default::default()
    };
    raw.viewports.insert(
        crate::foxglove::overlay_viewport::overlay_viewport_id(),
        viewport,
    );
    ctx.run_ui(raw, |ui| show(ui, live)).textures_delta.clear();
}

fn repaints(live: &Live, focused: bool, minimised: bool) -> u32 {
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    REPAINTS.set(0);
    run(&ctx, live, focused, minimised);
    REPAINTS.get()
}

#[test]
fn the_page_asks_for_frames_only_when_a_moving_look_is_in_front() {
    let s = rgb(half(true, 3, 26, 60), None);
    let moving = live_of(own_look(s), Some(&s), true);
    assert!(repaints(&moving, true, false) > 0);
    assert_eq!(repaints(&moving, false, false), 0, "behind another window");
    assert_eq!(repaints(&moving, true, true), 0, "minimised");
    let off = live_of(own_look(s), Some(&s), false);
    assert_eq!(repaints(&off, true, false), 0, "animation effects off");
    let solid = rgb(half(true, 0, 26, 60), None);
    let still = live_of(own_look(solid), Some(&solid), true);
    assert_eq!(repaints(&still, true, false), 0, "a still look");
}

#[test]
fn the_moment_moves_the_drawing_but_not_its_name() {
    let s = rgb(half(true, 3, 26, 60), None);
    let live = live_of(own_look(s), Some(&s), true);
    let at = |ms| live.look.colours(&live.layout, scene_time(&live, ms));
    assert_ne!(at(0), at(1500), "an effect moves");
    let again = live_of(own_look(s), Some(&s), true);
    assert_eq!(live.name, again.name);
    // Animation effects off: the same first frame whenever.
    let still = live_of(own_look(s), Some(&s), false);
    let still_at = |ms| still.look.colours(&still.layout, scene_time(&still, ms));
    assert_eq!(still_at(0), still_at(1500));
}

#[test]
fn behind_another_window_the_last_frame_is_held() {
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    let id = egui::Id::new("held");
    let a: Keys = vec![vec![Some(Color32::RED)]];
    let b: Keys = vec![vec![Some(Color32::BLUE)]];
    assert_eq!(held(&ctx, id, true, a.clone()), a);
    assert_eq!(
        held(&ctx, id, false, b.clone()),
        a,
        "behind: the last drawn"
    );
    assert_eq!(held(&ctx, id, true, b.clone()), b, "back in front: now");
}

#[test]
fn a_dim_key_still_reads_as_lit() {
    // The curve lifts a mid colour and keeps the brightest channel's order.
    let mid = lit(Color32::from_rgb(200, 100, 0));
    assert!(mid.r() > 200 && mid.g() > 100 && mid.r() > mid.g());
    let dim = lit(Color32::from_rgb(10, 5, 0));
    assert!(dim.r() >= FLOOR);
    assert_eq!(lit(Color32::BLACK), Color32::from_gray(FLOOR));
}

#[test]
fn the_line_says_when_a_pick_is_not_applied_or_a_half_is_unheard() {
    let (line, name) = not_applied("Solid · Blue · 60%".into(), "Keyboard lights: solid".into());
    assert_eq!(line, "Solid · Blue · 60% · not applied yet");
    assert!(name.ends_with(", not applied yet"));
    let solid = rgb(half(true, 0, 26, 80), None);
    let l = own_look(solid);
    let (line, name) = words_with(&l, Some(&solid), [100, 100], &parts(), true, true);
    assert!(line.contains("Right: no report yet"), "{line}");
    assert!(name.contains("no report yet"), "{name}");
    let (line, _) = words_with(&l, Some(&solid), [100, 100], &parts(), true, false);
    assert!(!line.contains("no report yet"), "{line}");
}
