//! The calibrator's tests (`calibrate_ui::tests`), and the samples the previews draw.

use super::*;
use foxglove_devices::bracket::{Answer, Bracket, Pick};
use foxglove_devices::calibration::Calibration;

pub fn case() -> Target {
    Target {
        key: "103c:84fd".into(),
        name: "HP OMEN 45L case lights".into(),
        kind: Kind::Lights {
            zones: [
                ("1", "Diamond display"),
                ("2", "Interior light"),
                ("5", "Front bottom fan"),
                ("6", "Front middle fan"),
                ("7", "Front top fan"),
            ]
            .map(|(i, n)| (i.to_string(), n.to_string()))
            .to_vec(),
        },
        default: Some(Calibration::default()),
        hint: None,
    }
}

pub fn corne() -> Target {
    Target {
        key: "1d50:615e".into(),
        name: "Eyelash Corne".into(),
        kind: Kind::Keyboard,
        default: None,
        hint: None,
    }
}

/// A strip with the catalog's SMD5050 hint (#473): FastLED's typical green and blue cut.
pub fn hinted_strip() -> Target {
    Target {
        key: "1a86:fe07".into(),
        name: "Desk strip".into(),
        kind: Kind::Lights {
            zones: vec![("1".into(), "Strip".into())],
        },
        default: None,
        hint: Some([1.0, 0.6902, 0.9412]),
    }
}

/// White matched with the light's blue down; red matched with the screen a little up.
pub fn sample_matches() -> Vec<Match> {
    vec![
        Match {
            color: [153; 3],
            light: [153, 147, 107],
            screen: [153; 3],
            zone: None,
        },
        Match {
            color: [153, 0, 0],
            light: [130, 0, 0],
            screen: [161, 0, 0],
            zone: None,
        },
    ]
}

/// The previews' calibrator on the 45L, at a colour, part-way through: two matches made, and
/// on blue the light down 30% and the screen up 5%.
pub fn sample(step: Step, master: bool) -> (View, Wizard) {
    let mut w = Wizard::new(case(), None);
    w.cal.matches = sample_matches();
    w.go_to(step, None);
    w.fresh = false;
    if step == Step::Blue {
        w.current.light = [0, 0, 107];
        w.current.screen = [0, 0, 161];
        w.ask_from_here();
    }
    if master {
        w.reference = Some(corne());
    }
    (
        View {
            others: vec![corne()],
            moving: false,
        },
        w,
    )
}

fn ui_run(v: &View, w: &mut Wizard, input: egui::RawInput) -> Output {
    let ctx = super::super::pc_lights_ui::tests::test_ctx();
    let mut out = Output::default();
    let mut first = ctx.run_ui(egui::RawInput::default(), |ui| {
        draw(ui, v, &mut w.clone());
    });
    first.textures_delta.clear();
    let mut frame = ctx.run_ui(input, |ui| out = draw(ui, v, w));
    frame.textures_delta.clear();
    out
}

#[test]
fn the_first_frame_shows_the_colour_and_a_quiet_frame_writes_nothing() {
    let (v, mut w) = sample(Step::White, false);
    w.fresh = true;
    let out = ui_run(&v, &mut w, egui::RawInput::default());
    assert!(out.show && out.show_reference && out.done.is_none());
    let out = ui_run(&v, &mut w, egui::RawInput::default());
    assert_eq!(out, Output::default(), "nothing moved: nothing is written");
}

#[test]
fn test_colours_start_mid_range_and_never_use_a_full_channel() {
    for step in Step::ALL {
        let c = step.color();
        assert!(c.iter().all(|v| *v < 255), "{step:?} {c:?}");
        assert!(c.iter().all(|v| *v <= TEST_LEVEL), "{step:?} {c:?}");
        let max = *c.iter().max().unwrap();
        assert!((115..=160).contains(&max), "{step:?}: about 45-60%");
    }
    // Every test colour is different.
    for (i, a) in Step::ALL.iter().enumerate() {
        for b in &Step::ALL[i + 1..] {
            assert_ne!(a.color(), b.color());
        }
    }
    // A picked colour comes down to the test level, keeping its hue.
    assert_eq!(test_color([255, 128, 0]), [153, 77, 0]);
    assert_eq!(test_color([100, 50, 20]), [100, 50, 20]);
    assert_eq!(test_color([255; 3]), [153; 3]);
}

#[test]
fn these_match_keeps_both_sides_and_moves_on() {
    let mut w = Wizard::new(case(), None);
    assert_eq!(w.current, Match::plain(Step::White.color()));
    w.current.light[2] = 107;
    w.current.screen[2] = 161;
    w.confirm();
    assert_eq!(w.cal.matches.len(), 1);
    assert_eq!(
        (w.cal.matches[0].light, w.cal.matches[0].screen),
        ([153, 153, 107], [153, 153, 161])
    );
    assert_eq!(w.next_unmatched(), Some(Step::Grey));
    // Grey starts from what the white match says, not from scratch.
    w.go_to(Step::Grey, None);
    assert!(
        w.current.light[2] < 115 && w.current.screen[2] > 115,
        "{:?}",
        w.current
    );
    // Confirming the same colour again replaces its match.
    w.go_to(Step::White, None);
    assert_eq!(w.current.light, [153, 153, 107], "its saved match");
    w.current.light[2] = 120;
    w.confirm();
    assert_eq!(w.cal.matches.len(), 1);
    assert_eq!(w.cal.matches[0].light[2], 120);
    // A zone's match is its own.
    w.zone = Some("5".into());
    w.load_current();
    w.confirm();
    assert_eq!(w.cal.matches.len(), 2);
    assert_eq!(w.cal.matches[1].zone.as_deref(), Some("5"));
}

/// A model's hint starts the light side of its first match (the screen as is) and nothing
/// after: once a colour is matched the next ones (and a zone without matches of its own)
/// follow the matches. Without a hint the first match starts as is.
#[test]
fn a_hint_starts_the_first_match_only() {
    let mut w = Wizard::new(hinted_strip(), None);
    let white = Step::White.color();
    assert_eq!(w.current, Match::hinted(white, [1.0, 0.6902, 0.9412]));
    assert!(w.current.light[1] < 153 && w.current.light[2] < 153);
    assert_eq!(w.current.screen, white);
    assert!(!w.unconfirmed(), "the hinted start isn't an adjustment");
    // Confirmed as the user left it: grey follows that match, not the hint.
    w.current.light = [153, 140, 150];
    w.confirm();
    w.go_to(Step::Grey, None);
    let grey = Step::Grey.color();
    assert_ne!(w.current, Match::hinted(grey, [1.0, 0.6902, 0.9412]));
    assert!(w.current.light[1] > 100, "{:?}", w.current);
    // A zone with no matches of its own follows the device's, not the hint.
    w.zone = Some("1".into());
    w.go_to(Step::White, None);
    assert_eq!(w.current.light, [153, 140, 150]);
    assert_eq!(w.current.zone.as_deref(), Some("1"));
    // Start over goes back to the hint.
    w.start_over();
    assert_eq!(w.current, Match::hinted(white, [1.0, 0.6902, 0.9412]));
    // No hint: as is.
    assert_eq!(Wizard::new(case(), None).current, Match::plain(white));
}

#[test]
fn each_side_has_its_own_values_and_reset() {
    let (v, mut w) = sample(Step::Blue, false);
    assert_eq!(match_text(&w.current), "Light: B 70 · Screen: B +5");
    let (_, white) = sample(Step::White, false);
    assert_eq!(
        match_text(&white.current),
        "Light: R 100 G 96 B 70 · Screen: as is"
    );
    let text = shown(&v, &mut w);
    assert!(text.contains("Light: B 70 · Screen: B +5"), "{text}");
    assert!(text.contains("ADJUST") && text.contains("Screen") && text.contains("Light"));
    w.reset_side(Side::Screen);
    assert_eq!(w.current.screen, [0, 0, 153]);
    assert_eq!(w.current.light, [0, 0, 107], "the light stays");
    w.reset_side(Side::Light);
    assert_eq!(w.current.light, [0, 0, 153]);
}

#[test]
fn the_screen_swatch_draws_the_screen_side() {
    let (v, mut w) = sample(Step::Blue, false);
    let screen = Color32::from_rgb(0, 0, 161);
    let drawn = fills(&v, &mut w);
    assert!(drawn.contains(&screen), "{drawn:?}");
    // Against another device, the screen isn't the reference: no screen swatch.
    let (v, mut w) = sample(Step::Blue, true);
    assert!(!fills(&v, &mut w).contains(&screen));
}

#[test]
fn the_matches_list_has_redo_and_remove() {
    let (v, mut w) = sample(Step::Blue, false);
    let text = shown(&v, &mut w);
    assert!(text.contains("MATCHES") && text.contains("Redo") && text.contains("Remove"));
    assert!(text.contains("Light: R 85 · Screen: R +5"), "{text}");
    w.redo(1);
    assert_eq!(
        (w.step, w.current.clone()),
        (Step::Red, w.cal.matches[1].clone())
    );
    w.remove(0);
    assert_eq!(w.cal.matches.len(), 1);
    assert_eq!(w.cal.matches[0].color, [153, 0, 0]);
    // No words that claim a colour is right or wrong.
    for word in ["true", "accurate", "correct", "can't do"] {
        assert!(!text.to_lowercase().contains(word), "{word}: {text}");
    }
}

#[test]
fn save_keeps_the_matches_and_nothing_forgets() {
    let mut w = Wizard::new(case(), None);
    assert_eq!(w.result(), None, "the model's default, no matches");
    w.confirm();
    let saved = w.result().unwrap();
    assert_eq!(saved.matches.len(), 1);
    assert_eq!(saved.name, "HP OMEN 45L case lights");
    w.start_over();
    assert_eq!(w.result(), None);
    assert_eq!(w.cal, start(&case()));
    // A calibration from before matches keeps its values, with no matches.
    let old = DeviceCalibration {
        device: Calibration {
            gain: [1.0, 0.9, 0.7],
            ..Default::default()
        },
        ..Default::default()
    };
    let w = Wizard::new(case(), Some(old.clone()));
    assert_eq!(w.result().unwrap().device, old.device);
    assert!(w.result().unwrap().matches.is_empty());
}

#[test]
fn save_takes_an_adjusted_colour_as_a_match() {
    let mut w = Wizard::new(case(), None);
    assert!(!w.unconfirmed());
    w.current.screen[2] = 161;
    assert!(w.unconfirmed());
    w.confirm();
    assert!(!w.unconfirmed(), "confirmed");
}

#[test]
fn while_matching_the_device_gets_the_light_value_as_it_is() {
    let (_, w) = sample(Step::Blue, false);
    let preview = w.previewing();
    assert!(preview.matches.is_empty());
    assert_eq!(preview.device, w.cal.device);
}

#[test]
fn a_picked_colour_opens_at_the_test_level() {
    let mut w = Wizard::new(case(), None);
    w.go_to(Step::White, Some(test_color([255, 128, 0])));
    assert_eq!(w.color(), [153, 77, 0]);
    assert_eq!(w.current.color, [153, 77, 0]);
    w.confirm();
    assert_eq!(w.next_unmatched(), Some(Step::Grey));
    // A picked colour that is a test colour is that step.
    w.go_to(Step::White, Some([0, 0, 153]));
    assert_eq!((w.step, w.custom), (Step::Blue, None));
}

#[test]
fn the_first_device_picked_to_compare_with_is_captured_and_put_back() {
    let mut w = Wizard::new(case(), None);
    let picked = Output {
        reference_changed: Some(None),
        show_reference: true,
        ..Default::default()
    };
    w.reference = Some(corne());
    assert!(!act(&mut w, picked));
    // No keyboard in the tests: captured as unknown, which restore turns into white.
    assert_eq!(w.reference_before, Before::Unknown);
}

#[test]
fn cancel_puts_back_what_was_saved() {
    let key = "calibrate-ui-cancel";
    let saved = DeviceCalibration {
        matches: sample_matches(),
        ..Default::default()
    };
    calibration::global().set_device(key, Some(saved.clone()));
    let mut target = case();
    target.key = key.into();
    // Before there is a device or page: show and restore only queue (no worker runs).
    let mut w = Wizard::new(target, Some(saved.clone()));
    w.cal.matches.clear();
    let preview = Output {
        show: true,
        ..Default::default()
    };
    assert!(!act(&mut w, preview));
    assert!(calibration::global()
        .device(key)
        .unwrap()
        .matches
        .is_empty());
    let cancel = Output {
        done: Some(Done::Cancel),
        ..Default::default()
    };
    assert!(act(&mut w, cancel));
    assert_eq!(calibration::global().device(key), Some(saved));
    calibration::global().set_device(key, None);
}

#[test]
fn copy_and_import_round_trip() {
    let (_, w) = sample(Step::Grey, false);
    let text = calibration::export(&w.target.key, &w.cal).unwrap();
    let (_, cal) = calibration::import(&text).unwrap();
    assert_eq!(cal.matches, w.cal.matches);
    assert!(calibration::import("not a calibration").is_err());
}

#[test]
fn every_colour_draws_and_zones_show_for_the_case() {
    for step in Step::ALL {
        let (v, mut w) = sample(step, false);
        let text = shown(&v, &mut w);
        assert!(text.contains(step.name()), "{step:?}: {text}");
        // An unmatched colour asks; a matched one (White and Red here) says so.
        let asks = text.contains("They match") && text.contains("A is closer");
        assert!(asks != text.contains("Ask again"), "{step:?}: {text}");
        assert!(text.contains("Save"));
        assert!(text.contains("Front top fan") && text.contains("All zones"));
    }
    let (v, mut w) = sample(Step::Blue, true);
    let text = shown(&v, &mut w);
    assert!(text.contains("On the Eyelash Corne"), "{text}");
    let mut keyboard = Wizard::new(corne(), None);
    let text = shown(&View::default(), &mut keyboard);
    assert!(!text.contains("All zones"), "the keyboard has one colour");
    let moving = View {
        moving: true,
        ..View::default()
    };
    assert!(shown(&moving, &mut keyboard).contains("Solid"));
}

/// A user who sees exactly: answers every choice for `w`'s side by which candidate is nearer
/// `target` in Oklab, until the colour is matched. Returns the choices it took.
pub fn answer_toward(w: &mut Wizard, target: [u8; 3]) -> u8 {
    use foxglove_devices::oklab;
    let t = oklab::from_srgb(target);
    let mut n = 0;
    while let Some((a, b)) = w.ask.as_ref().and_then(Bracket::pair) {
        let da = oklab::distance(oklab::from_srgb(a), t);
        let db = oklab::distance(oklab::from_srgb(b), t);
        let pick = if da <= db { Pick::A } else { Pick::B };
        n += 1;
        if w.choose(Answer::Closer(pick)) {
            break;
        }
        assert!(n <= foxglove_devices::bracket::MAX_ROUNDS);
    }
    n
}

/// A warmer white than the screen's: the light side's choices (two warm or cool, two green or
/// pink, then two brightness) find it within one just-noticeable difference in six choices,
/// and it's kept as White's match, with the next colour offered.
#[test]
fn choices_find_the_match_and_keep_it() {
    use foxglove_devices::{bracket::MAX_ROUNDS, oklab};
    let mut w = Wizard::new(case(), None);
    let b = w.ask.clone().expect("White asks");
    assert_eq!((b.rounds(), b.planned()), (0, 6));
    assert!(b.planned() <= MAX_ROUNDS);
    // The device shows A until B is picked to look at.
    let (a, bb) = b.pair().unwrap();
    assert_eq!(w.lit(), a);
    w.showing = Pick::B;
    assert_eq!(w.lit(), bb);
    let text = shown(&View::default(), &mut w);
    for t in [
        "Choice 1 of 6 · warm or cool",
        "DEVICE SHOWS",
        "Which looks closer to your screen?",
        "A is closer",
        "B is closer",
        "They match",
    ] {
        assert!(text.contains(t), "{t}: {text}");
    }
    let white = oklab::from_srgb([153; 3]);
    let target = oklab::to_srgb([white[0], white[1] - 0.01, white[2] + 0.03]);
    assert_eq!(answer_toward(&mut w, target), 6);
    // Skin and Warm: hue and strength three each, then brightness twice.
    for step in [Step::Skin, Step::Warm] {
        let mut other = Wizard::new(case(), None);
        other.go_to(step, None);
        assert_eq!(other.ask.as_ref().unwrap().planned(), 8, "{step:?}");
    }
    assert!(w.ask.is_none() && w.matched(Step::White.color()));
    assert_eq!(w.cal.matches.len(), 1);
    assert_eq!(w.cal.matches[0].light, w.current.light);
    assert_eq!(w.cal.matches[0].screen, [153; 3], "the screen side as is");
    let off = oklab::delta_e(w.current.light, target);
    assert!(
        off < oklab::JND,
        "{:?} vs {target:?}: {off}",
        w.current.light
    );
    assert!(!w.unconfirmed(), "kept: Save has nothing more to take");
    assert_eq!(w.lit(), w.current.light, "the device shows the match");
    let note = w.note.clone().unwrap();
    assert!(
        note.starts_with("Matched: White · 6 choices · moved ΔE 0.0"),
        "{note}"
    );
    assert!(note.contains(" · within ΔE 0.01"), "{note}");
    assert!(w.finished.as_ref().unwrap().left() < oklab::JND);
    let text = shown(&View::default(), &mut w);
    for t in [
        "✓ Matched after 6 choices",
        "Next: Grey",
        "Ask again",
        "White ✓",
    ] {
        assert!(text.contains(t), "{t}: {text}");
    }
    assert!(!text.contains("A is closer"));
    // The result is in what Save keeps.
    assert_eq!(w.result().unwrap().matches, w.cal.matches);
    // Ask again starts from the match.
    w.ask_from_here();
    assert_eq!(w.ask.as_ref().unwrap().start(), w.current.light);
}

/// They match on White's warm or cool skips green or pink and goes on to brightness; they match
/// there ends the colour and keeps it where the choices stand.
#[test]
fn they_match_skips_to_brightness_then_ends_the_colour() {
    let mut w = Wizard::new(case(), None);
    assert!(!w.choose(Answer::Closer(Pick::B)));
    let moved = w.current.light;
    assert_ne!(moved, [153; 3], "a pick moves the light half a step");
    assert!(w.unconfirmed(), "Save would take it as it stands");
    assert!(!w.choose(Answer::Same), "still asking: brightness");
    assert_eq!(w.current.light, moved);
    assert!(w.cal.matches.is_empty());
    let text = shown(&View::default(), &mut w);
    assert!(text.contains("Choice 3 of 4 · brightness"), "{text}");
    assert!(w.choose(Answer::Same));
    assert_eq!(w.cal.matches[0].light, moved);
    assert_eq!(w.finished.as_ref().unwrap().rounds(), 3);
    assert!(w.note.clone().unwrap().contains("· 3 choices ·"));
}

/// Red has only brightness: they match ends the colour at once.
#[test]
fn they_match_on_a_one_channel_colour_ends_it() {
    let mut w = Wizard::new(case(), None);
    w.go_to(Step::Red, None);
    assert!(w.choose(Answer::Same));
    assert!(w.matched(Step::Red.color()));
    assert_eq!(w.finished.as_ref().unwrap().rounds(), 1);
}

/// Red lights one channel: three choices of brightness, and green and blue stay off in every
/// candidate.
#[test]
fn a_one_channel_colour_asks_only_its_brightness() {
    let mut w = Wizard::new(case(), None);
    w.go_to(Step::Red, None);
    let b = w.ask.clone().unwrap();
    assert_eq!(b.planned(), 3);
    let text = shown(&View::default(), &mut w);
    assert!(text.contains("Choice 1 of 3 · brightness"), "{text}");
    let mut n = 0;
    while let Some((a, bb)) = w.ask.as_ref().and_then(Bracket::pair) {
        assert_eq!((a[1], a[2], bb[1], bb[2]), (0, 0, 0, 0));
        n += 1;
        w.choose(Answer::Closer(Pick::A));
    }
    assert_eq!(n, 3);
    assert!(w.current.light[0] < 153 && w.current.light[1..] == [0, 0]);
}

/// The screen side: both candidates are drawn on the screen and the device keeps the light.
#[test]
fn the_screen_side_draws_both_candidates() {
    let (v, mut w) = sample(Step::Grey, false);
    let light = w.current.light;
    w.set_side(Side::Screen);
    let (a, b) = w.ask.as_ref().unwrap().pair().unwrap();
    assert_eq!(w.lit(), light, "the device shows the match's light");
    let drawn = fills(&v, &mut w);
    for c in [a, b] {
        assert!(
            drawn.contains(&Color32::from_rgb(c[0], c[1], c[2])),
            "{c:?}"
        );
    }
    let text = shown(&v, &mut w);
    assert!(
        text.contains("Which swatch looks closer to this device?"),
        "{text}"
    );
    assert!(text.contains("Screen A") && text.contains("Screen B"));
    assert!(!text.contains("DEVICE SHOWS"));
    answer_toward(&mut w, [120, 115, 110]);
    assert_eq!(w.current.light, light, "the light side stays");
    assert_ne!(w.current.screen, Step::Grey.color());
}

/// The hinted strip's first choices start from the hint (#473), and against another device the
/// question names it.
#[test]
fn choices_start_from_the_hint_and_name_the_other_device() {
    let mut w = Wizard::new(hinted_strip(), None);
    let hinted = Match::hinted(Step::White.color(), [1.0, 0.6902, 0.9412]).light;
    assert_eq!(w.ask.as_ref().unwrap().start(), hinted);
    w.reference = Some(corne());
    let text = shown(&View::default(), &mut w);
    assert!(
        text.contains("Which looks closer to the Eyelash Corne?"),
        "{text}"
    );
}

/// Draws the calibrator headless and returns its text.
fn shown(v: &View, w: &mut Wizard) -> String {
    let mut text = String::new();
    for clipped in &frame(v, w) {
        collect(&clipped.shape, &mut text);
    }
    text
}

/// Draws the calibrator headless and returns its filled rectangles' colours.
fn fills(v: &View, w: &mut Wizard) -> Vec<Color32> {
    let mut out = Vec::new();
    for clipped in &frame(v, w) {
        rects(&clipped.shape, &mut out);
    }
    out
}

fn frame(v: &View, w: &mut Wizard) -> Vec<egui::epaint::ClippedShape> {
    let ctx = super::super::pc_lights_ui::tests::test_ctx();
    let mut first = ctx.run_ui(egui::RawInput::default(), |ui| {
        draw(ui, v, &mut w.clone());
    });
    first.textures_delta.clear();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        draw(ui, v, w);
    });
    out.textures_delta.clear();
    out.shapes
}

fn rects(shape: &egui::Shape, out: &mut Vec<Color32>) {
    match shape {
        egui::Shape::Rect(r) => out.push(r.fill),
        egui::Shape::Vec(v) => v.iter().for_each(|s| rects(s, out)),
        _ => {}
    }
}

fn collect(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
        _ => {}
    }
}
