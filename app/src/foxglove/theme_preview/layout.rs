//! The layout guard: every sidebar page, the tray panel and the wizards (Calibrate, Add a
//! device, Update keyboard), laid out off-screen at the window's minimum size, its default
//! size and a wide one, at 100% and 150% scale. A case fails when two lines of text overlap,
//! text runs into a control it isn't part of, two controls overlap, a card overlaps another
//! card, something sticks out of its card, or text is cut off at the side.
//!
//! The checks read what egui reports, not pixels: each line of text from the frame's text
//! shapes (glyph to glyph), each control from the frame's widget rects, and each card from its
//! frame (the card surface with its line). The page's scroll area lays out the whole page
//! however short the window is, so the parts below the fold are checked too.
//!
//! With `FOXGLOVE_UI_PREVIEW_DIR` set, the Lights page is also kept at every size and scale as
//! `ui-layout-lights-<w>x<h>@<scale>.png`.

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
const SCALES: [f32; 2] = [1.0, 1.5];

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
        for c in &d.controls {
            if overlap(a.rect, *c).is_some() && !inside(a.rect, *c) && !inside(*c, a.rect) {
                out.push(format!(
                    "text over a control: {:?} {} and {}",
                    a.text,
                    fmt(a.rect),
                    fmt(*c)
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
        for (b, _) in &d.cards[i + 1..] {
            if overlap(*a, *b).is_some() && !inside(*a, *b) && !inside(*b, *a) {
                out.push(format!("card over a card: {} and {}", fmt(*a), fmt(*b)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// One case: `draw` on a fresh context at `size` and `ppp`; the problems, prefixed with `name`.
/// Keeps a PNG named `png` when the preview folder is set.
fn check(
    name: &str,
    size: egui::Vec2,
    ppp: f32,
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
    let found = problems(&drawn(&ctx, &shapes));
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
            out.extend(check(name, size, ppp, png.as_deref(), |ui| {
                crate::foxglove::shell::frame(ui, sections)
            }));
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
        let png = (page == Page::Lights).then_some("ui-layout-lights");
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
        found.extend(page_cases(&format!("Update keyboard: {name}"), Page::Keyboard, &s, None));
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
            found.extend(check(name, size, ppp, None, |ui| {
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

/// The guard itself catches what the user saw: two cards on top of each other, and a label
/// and its value drawn in the same place.
#[test]
fn the_guard_catches_overlaps() {
    let found = check("probe", egui::vec2(400.0, 300.0), 1.0, None, |ui| {
        let r = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(200.0, 80.0));
        for at in [r, r.translate(egui::vec2(120.0, 40.0))] {
            ui.put(at, |ui: &mut egui::Ui| {
                crate::foxglove::theme::card(ui)
                    .show(ui, |ui| ui.label("A card"))
                    .response
            });
        }
        ui.put(
            Rect::from_min_size(egui::pos2(20.0, 200.0), egui::vec2(100.0, 20.0)),
            egui::Label::new("Left"),
        );
        ui.put(
            Rect::from_min_size(egui::pos2(24.0, 200.0), egui::vec2(100.0, 20.0)),
            egui::Label::new("on · Solid"),
        );
    });
    assert!(
        found.iter().any(|p| p.contains("card over a card")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|p| p.contains("text over text")),
        "{found:#?}"
    );
}
