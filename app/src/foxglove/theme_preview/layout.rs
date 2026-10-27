//! The layout guard: every sidebar page, the tray panel and the wizards (Calibrate, Add a
//! device, Update firmware), laid out off-screen at the window's minimum size, its default
//! size and a wide one, at 100% and 150% scale. A case fails when two lines of text overlap,
//! text runs into a control it isn't part of, two controls overlap, a card overlaps another
//! card, something sticks out of its card, or text is cut off at the side.
//!
//! The checks read what egui reports, not pixels: each line of text from the frame's text
//! shapes (glyph to glyph), each control from the frame's widget rects, and each card from its
//! frame (the card surface with its line). The page's scroll area lays out the whole page
//! however short the window is, so the parts below the fold are checked too.
//!
//! With `FOXGLOVE_UI_PREVIEW_DIR` set, the Lights and Scenes pages are also kept at every size
//! and scale as `ui-layout-lights-<w>x<h>@<scale>.png` and `ui-scenes-layout-<w>x<h>@<scale>.png`.

use std::collections::HashMap;

use egui::epaint::{ClippedShape, Shape};
use egui::{Color32, Rect};

use super::{clear_samples, keep, paint, preview_ctx, run_frames, sections, show_samples, Samples};
use crate::foxglove::settings_window::{DEFAULT_SIZE, MIN_SIZE};
use crate::foxglove::shell::Page;
use crate::foxglove::theme::DARK;

/// Points two things may touch by before they count as overlapping (anti-aliased edges, the
/// pixel rounding of text at 150%).
const SLACK: f32 = 1.0;

const SIZES: [egui::Vec2; 3] = [MIN_SIZE, DEFAULT_SIZE, egui::vec2(1440.0, 900.0)];
/// 100%, the common 125% and 150% (the user's display).
const SCALES: [f32; 3] = [1.0, 1.25, 1.5];
/// A mark (a colour chip, a swatch, a dot) is a filled shape at most this big, in points².
const MARK_AREA: f32 = 2500.0;

/// One line of text as drawn: its words and the box from its first glyph to its last.
#[derive(Debug)]
struct Line {
    text: String,
    rect: Rect,
    clip: Rect,
}

/// What a frame drew, as the checks see it.
#[derive(Default)]
struct Drawn {
    lines: Vec<Line>,
    cards: Vec<(Rect, Rect)>,
    controls: Vec<Rect>,
    /// Small filled shapes: colour chips, swatches, status dots. Text must not run into them.
    marks: Vec<Rect>,
}

fn collect(shape: &Shape, clip: Rect, d: &mut Drawn) {
    match shape {
        Shape::Vec(v) => v.iter().for_each(|s| collect(s, clip, d)),
        Shape::Text(t) => {
            for row in &t.galley.rows {
                let glyphs: Vec<_> = row
                    .glyphs
                    .iter()
                    .filter(|g| !g.chr.is_whitespace())
                    .collect();
                let (Some(first), Some(last)) = (glyphs.first(), glyphs.last()) else {
                    continue;
                };
                let r = row.rect();
                let x = t.pos.x + row.pos.x;
                d.lines.push(Line {
                    text: row.text(),
                    rect: Rect::from_x_y_ranges(
                        x + first.pos.x..=x + last.pos.x + last.advance_width,
                        t.pos.y + r.top()..=t.pos.y + r.bottom(),
                    ),
                    clip,
                });
            }
        }
        Shape::Rect(r)
            if r.fill == DARK.surface && r.stroke.color == DARK.line && r.stroke.width == 1.0 =>
        {
            d.cards.push((r.rect, clip));
        }
        Shape::Rect(r) if r.fill.a() > 0 && r.rect.is_positive() && r.rect.area() <= MARK_AREA => {
            d.marks.push(r.rect);
        }
        Shape::Circle(c) if c.fill.a() > 0 && c.radius > 1.0 => {
            let r = Rect::from_center_size(c.center, egui::Vec2::splat(c.radius * 2.0));
            if r.area() <= MARK_AREA {
                d.marks.push(r);
            }
        }
        _ => {}
    }
}

fn drawn(ctx: &egui::Context, shapes: &[ClippedShape]) -> Drawn {
    let mut d = Drawn::default();
    for s in shapes {
        collect(&s.shape, s.clip_rect, &mut d);
    }
    // Controls: whatever takes a click or a drag (buttons, pills, checkboxes, sliders, text
    // fields, selectable labels). Containers that take a drag hold their contents, which the
    // checks allow.
    d.controls = ctx.viewport(|v| {
        v.prev_pass
            .widgets
            .layers()
            .flat_map(|(_, w)| w.iter())
            .filter(|w| (w.sense.senses_click() || w.sense.senses_drag()) && w.rect.is_positive())
            .map(|w| w.rect)
            .collect()
    });
    d
}

/// The overlap of `a` and `b`, each shrunk by the slack; `None` when they only touch.
fn overlap(a: Rect, b: Rect) -> Option<Rect> {
    let i = a.shrink(SLACK / 2.0).intersect(b.shrink(SLACK / 2.0));
    i.is_positive().then_some(i)
}

fn inside(inner: Rect, outer: Rect) -> bool {
    outer.expand(SLACK).contains_rect(inner)
}

/// `a` and `b` are the same box, give or take two points each way (a label's widget rect
/// around its glyphs); a control the size of a short word drawn over it is not.
fn same(a: Rect, b: Rect) -> bool {
    inside(a, b.expand(2.0)) && inside(b, a.expand(2.0))
}

fn fmt(r: Rect) -> String {
    format!(
        "[{:.0},{:.0} {:.0}x{:.0}]",
        r.min.x,
        r.min.y,
        r.width(),
        r.height()
    )
}

/// Everything wrong with one frame's layout, in words.
fn problems(d: &Drawn) -> Vec<String> {
    let mut out = Vec::new();
    for (i, a) in d.lines.iter().enumerate() {
        // Cut off at the side (a short window may cut the page at the bottom: it scrolls).
        let visible = a.rect.y_range().intersection(a.clip.y_range()).span() > 0.0;
        if visible
            && (a.rect.left() < a.clip.left() - SLACK || a.rect.right() > a.clip.right() + SLACK)
        {
            out.push(format!(
                "text cut off at the side: {:?} {}",
                a.text,
                fmt(a.rect)
            ));
        }
        for b in &d.lines[i + 1..] {
            if overlap(a.rect, b.rect).is_some() {
                out.push(format!(
                    "text over text: {:?} {} and {:?} {}",
                    a.text,
                    fmt(a.rect),
                    b.text,
                    fmt(b.rect)
                ));
            }
        }
        // A line is fine inside a control (a button's label, a selectable label), and a control
        // the size of the line is the line's own (a link); anything else touching it isn't.
        for c in &d.controls {
            if overlap(a.rect, *c).is_some() && !inside(a.rect, *c) && !same(*c, a.rect) {
                out.push(format!(
                    "text over a control: {:?} {} and {}",
                    a.text,
                    fmt(a.rect),
                    fmt(*c)
                ));
            }
        }
        for m in &d.marks {
            if overlap(a.rect, *m).is_some() && !inside(a.rect, *m) {
                out.push(format!(
                    "text over a mark: {:?} {} and {}",
                    a.text,
                    fmt(a.rect),
                    fmt(*m)
                ));
            }
        }
        for (card, _) in &d.cards {
            if overlap(a.rect, *card).is_some() && !inside(a.rect, *card) {
                out.push(format!(
                    "text across a card's edge: {:?} {} card {}",
                    a.text,
                    fmt(a.rect),
                    fmt(*card)
                ));
            }
        }
    }
    for (i, a) in d.controls.iter().enumerate() {
        for b in &d.controls[i + 1..] {
            if overlap(*a, *b).is_some() && !inside(*a, *b) && !inside(*b, *a) {
                out.push(format!(
                    "control over a control: {} and {}",
                    fmt(*a),
                    fmt(*b)
                ));
            }
        }
        for (card, _) in &d.cards {
            if overlap(*a, *card).is_some() && !inside(*a, *card) && !inside(*card, *a) {
                out.push(format!(
                    "control across a card's edge: {} card {}",
                    fmt(*a),
                    fmt(*card)
                ));
            }
        }
    }
    for (i, (a, clip)) in d.cards.iter().enumerate() {
        if a.left() < clip.left() - SLACK || a.right() > clip.right() + SLACK {
            out.push(format!(
                "card cut off at the side: {} in {}",
                fmt(*a),
                fmt(*clip)
            ));
        }
        // No card sits inside another: any overlap is one drawn over the other.
        for (b, _) in &d.cards[i + 1..] {
            if overlap(*a, *b).is_some() {
                out.push(format!("card over a card: {} and {}", fmt(*a), fmt(*b)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// One case: `draw` on a fresh context at `size` and `ppp`; the problems, prefixed with `name`.
/// With `cards`, finding no card at all is a problem too (the card checks would see nothing if
/// the cards' look changed). Keeps a PNG named `png` when the preview folder is set.
fn check(
    name: &str,
    size: egui::Vec2,
    ppp: f32,
    cards: bool,
    png: Option<&str>,
    mut draw: impl FnMut(&mut egui::Ui),
) -> Vec<String> {
    let ctx = preview_ctx();
    let want_png = png.is_some() && std::env::var_os("FOXGLOVE_UI_PREVIEW_DIR").is_some();
    let mut textures = HashMap::new();
    let (shapes, out_ppp) = run_frames(
        &ctx,
        size,
        ppp,
        want_png.then_some(&mut textures),
        &mut draw,
    );
    let d = drawn(&ctx, &shapes);
    let mut found = problems(&d);
    if cards && d.cards.is_empty() {
        found.push("no cards found: did the cards' look change? (`collect`)".into());
    }
    if let (true, Some(png)) = (want_png, png) {
        let img = paint(
            &ctx,
            size,
            Color32::from_rgb(0x09, 0x0a, 0x0c),
            shapes,
            out_ppp,
            &textures,
        );
        keep(png, &img);
    }
    found
        .into_iter()
        .map(|p| format!("{name} at {}x{} @{}%: {p}", size.x, size.y, ppp * 100.0))
        .collect()
}

/// A page of the settings window, as the native window draws it, at every size and scale.
fn page_cases(name: &str, page: Page, samples: &Samples, png: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for size in SIZES {
        for ppp in SCALES {
            show_samples(page, samples);
            let png = png.map(|p| format!("{p}-{}x{}@{}.png", size.x, size.y, ppp * 100.0));
            let found = check(name, size, ppp, true, png.as_deref(), |ui| {
                crate::foxglove::shell::frame(ui, sections)
            });
            out.extend(found);
            clear_samples();
        }
    }
    out
}

fn report(found: &[String]) {
    assert!(
        found.is_empty(),
        "{} layout problems:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn pages_lay_out_without_overlaps() {
    let mut found = Vec::new();
    for page in Page::ALL {
        let label = page.label();
        let png = match page {
            Page::Lights => Some("ui-layout-lights"),
            Page::Scenes => Some("ui-scenes-layout"),
            _ => None,
        };
        found.extend(page_cases(label, page, &Samples::full(), png));
        found.extend(page_cases(
            &format!("{label} (no keyboard)"),
            page,
            &Samples::none(),
            None,
        ));
        found.extend(page_cases(
            &format!("{label} (long texts)"),
            page,
            &super::long_samples(),
            None,
        ));
    }
    report(&found);
}

#[test]
fn wizards_lay_out_without_overlaps() {
    use crate::foxglove::firmware_update::View as Update;
    let mut found = Vec::new();
    // Calibrate: each step, and the blue step against the keyboard.
    for step in crate::foxglove::calibrate_ui::Step::ALL {
        let mut s = Samples::full();
        s.calibrate = Some(crate::foxglove::calibrate_ui::tests::sample(step, false));
        found.extend(page_cases(
            &format!("Calibrate {step:?}"),
            Page::Lights,
            &s,
            None,
        ));
    }
    let mut s = Samples::full();
    s.calibrate = Some(crate::foxglove::calibrate_ui::tests::sample(
        crate::foxglove::calibrate_ui::Step::Blue,
        true,
    ));
    found.extend(page_cases(
        "Calibrate against the keyboard",
        Page::Lights,
        &s,
        None,
    ));
    // Find my LEDs: the first LED, one on each half, the end; and the Scenes page's prompt.
    for answered in [0, 9, 30, 42] {
        let mut s = Samples::full();
        s.leds = Some(crate::foxglove::led_map_ui::tests::sample(answered));
        found.extend(page_cases(
            &format!("Find my LEDs after {answered}"),
            Page::Lights,
            &s,
            None,
        ));
    }
    let mut s = Samples::full();
    s.scenes.0.keyboard_frames = true;
    s.scenes.0.leds_placed = false;
    found.extend(page_cases(
        "Scenes, LEDs not placed",
        Page::Scenes,
        &s,
        None,
    ));
    s.scenes.0.leds_shipped = true;
    found.extend(page_cases(
        "Scenes, shipped LED map",
        Page::Scenes,
        &s,
        None,
    ));
    // Check LED map: the dot on the shipped map, after three corrections.
    for fixes in [
        vec![],
        vec![crate::foxglove::led_map_confirm::Answer::Mirrored; 3],
    ] {
        let mut s = Samples::full();
        s.ledcheck = Some(crate::foxglove::led_map_ui::tests::check_sample(
            Some(2),
            &fixes,
        ));
        found.extend(page_cases(
            &format!("Check LED map after {} fixes", fixes.len()),
            Page::Lights,
            &s,
            None,
        ));
    }
    // Add a device: pick, set up, name the zones, review.
    {
        use crate::foxglove::pc_lights_add::{tests::sample_zones, Step, Wizard};
        let pick = Wizard {
            step: Step::Pick,
            ..Wizard::default()
        };
        for (name, wizard) in [
            ("Pick", pick),
            (
                "Setup",
                Wizard {
                    step: Step::Setup,
                    ..sample_zones()
                },
            ),
            ("Zones", sample_zones()),
            (
                "Review",
                Wizard {
                    step: Step::Review,
                    ..sample_zones()
                },
            ),
        ] {
            let mut s = Samples::full();
            s.wizard = wizard;
            found.extend(page_cases(
                &format!("Add a device: {name}"),
                Page::Lights,
                &s,
                None,
            ));
        }
    }
    // Update keyboard: each of its steps on the Keyboard page's Firmware card.
    let steps = || {
        vec![
            "Left half: copied, restarted on 0.2.0 dev 1a2b3c4".to_string(),
            "Right half: waiting for its drive to appear (double-tap the reset button)".into(),
        ]
    };
    for (name, view) in [
        ("Checking", Update::Checking),
        ("Confirm", crate::foxglove::firmware_update::tests::sample()),
        (
            "Confirm an older version",
            Update::Confirm {
                lines: vec!["Left: 0.2.0 dev 1a2b3c4 -> 0.1.0 dev e255d32".into()],
                notes: vec!["This is an older version than the keyboard runs.".into()],
                downgrade: true,
                allow_downgrade: false,
            },
        ),
        (
            "Refused",
            Update::Refused(
                "The folder has no firmware for the Eyelash Corne (looked for eyelash_corne_left and \
                 eyelash_corne_right .uf2 files)"
                    .into(),
            ),
        ),
        ("Running", Update::Running(steps())),
        ("Done", Update::Done(steps())),
        (
            "Failed",
            Update::Failed {
                steps: steps(),
                message: "The right half's drive didn't appear within 60 seconds.".into(),
                recovery: "The left half runs the new firmware and still types. Double-tap the right \
                           half's reset button, then choose Update keyboard again."
                    .into(),
            },
        ),
    ] {
        let mut s = Samples::full();
        s.keyboard.update = view;
        found.extend(page_cases(&format!("Update firmware: {name}"), Page::Keyboard, &s, None));
    }
    report(&found);
}

#[test]
fn tray_panel_lays_out_without_overlaps() {
    let v = crate::foxglove::tray_panel::tests::sample();
    let paused = crate::foxglove::tray_panel::while_paused(crate::foxglove::tray_panel::View {
        paused: true,
        ..v.clone()
    });
    let size = egui::vec2(crate::foxglove::tray_panel::WIDTH + 40.0, 760.0);
    let mut found = Vec::new();
    for (name, view) in [("Tray panel", &v), ("Tray panel (paused)", &paused)] {
        for ppp in SCALES {
            found.extend(check(name, size, ppp, false, None, |ui| {
                egui::Area::new(egui::Id::new("tray-panel-preview"))
                    .fixed_pos(egui::pos2(20.0, 20.0))
                    .show(ui.ctx(), |ui| {
                        crate::foxglove::tray_panel::panel(
                            ui,
                            view,
                            &mut Default::default(),
                            &mut Vec::new(),
                        );
                    });
            }));
        }
    }
    report(&found);
}

/// A probe's drawing.
type Draw<'a> = dyn Fn(&mut egui::Ui) + 'a;

/// The guard itself catches each kind of problem, starting with what the user saw: two cards on
/// top of each other, and a label and its value drawn in the same place.
#[test]
fn the_guard_catches_overlaps() {
    let probe = |draw: &Draw<'_>| {
        check("probe", egui::vec2(400.0, 300.0), 1.0, false, None, |ui| {
            draw(ui)
        })
    };
    let at =
        |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
    let card = |ui: &mut egui::Ui, r: Rect, text: &str| {
        ui.put(r, |ui: &mut egui::Ui| {
            crate::foxglove::theme::card(ui)
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    ui.label(text)
                })
                .response
        });
    };
    let cases: [(&str, &Draw<'_>); 8] = [
        ("card over a card", &|ui| {
            card(ui, at(20.0, 20.0, 200.0, 80.0), "A card");
            card(ui, at(140.0, 60.0, 200.0, 80.0), "Another");
        }),
        ("card over a card", &|ui| {
            card(ui, at(20.0, 20.0, 300.0, 200.0), "A card");
            card(ui, at(60.0, 80.0, 120.0, 60.0), "Inside it");
        }),
        ("text over text", &|ui| {
            ui.put(at(20.0, 200.0, 100.0, 20.0), egui::Label::new("Left"));
            ui.put(at(24.0, 200.0, 100.0, 20.0), egui::Label::new("on · Solid"));
        }),
        ("text over a control", &|ui| {
            ui.put(
                at(20.0, 20.0, 300.0, 20.0),
                egui::Label::new("A long line of text here"),
            );
            ui.put(at(80.0, 18.0, 60.0, 24.0), egui::Button::new("OK"));
        }),
        ("control over a control", &|ui| {
            ui.put(at(20.0, 20.0, 80.0, 24.0), egui::Button::new("One"));
            ui.put(at(70.0, 20.0, 80.0, 24.0), egui::Button::new("Two"));
        }),
        ("text over a mark", &|ui| {
            ui.put(
                at(20.0, 20.0, 60.0, 20.0),
                egui::Label::new("on · Solid · 80%"),
            );
            ui.painter()
                .rect_filled(at(40.0, 22.0, 28.0, 14.0), 3.0, Color32::WHITE);
        }),
        ("across a card's edge", &|ui| {
            card(ui, at(20.0, 20.0, 160.0, 80.0), "A card");
            ui.put(at(150.0, 50.0, 80.0, 24.0), egui::Button::new("Apply"));
        }),
        ("cut off at the side", &|ui| {
            ui.set_clip_rect(at(0.0, 0.0, 120.0, 300.0));
            ui.put(
                at(80.0, 20.0, 200.0, 20.0),
                egui::Label::new("Cut off here"),
            );
        }),
    ];
    for (want, draw) in cases {
        let found = probe(draw);
        assert!(found.iter().any(|p| p.contains(want)), "{want}: {found:#?}");
    }
    // A page whose cards aren't recognised fails, rather than passing every card check.
    let blind = check("probe", egui::vec2(400.0, 300.0), 1.0, true, None, |ui| {
        ui.label("No card here");
    });
    assert!(
        blind.iter().any(|p| p.contains("no cards found")),
        "{blind:#?}"
    );
    // And a clean layout passes: a card with a row of controls inside it.
    let clean = probe(&|ui| {
        ui.put(at(20.0, 20.0, 300.0, 120.0), |ui: &mut egui::Ui| {
            crate::foxglove::theme::card(ui)
                .show(ui, |ui| {
                    ui.label("Underglow");
                    ui.horizontal(|ui| {
                        let _ = ui.button("On");
                        let _ = ui.button("Off");
                    });
                })
                .response
        });
    });
    assert!(clean.is_empty(), "{clean:#?}");
}
