//! Slice L2 (#1300's drawing on the Lights page): the keyboard drawing at the top of the
//! Keyboard card, dark and light. The page with Swirl running (`ui-lights-live-page-*`), one
//! drawing per state (`ui-lights-live-states-*`) and the same states in a narrow card
//! (`ui-lights-live-narrow-*`). Kept with `FOXGLOVE_UI_PREVIEW_DIR` set.

use super::{keep, preview_ctx, render_ui, shell_preview_in, Samples};
use crate::foxglove::lights_live::{look, Report, SceneRun};
use crate::foxglove::lights_live_ui::tests::{half, layout, live_of, rgb};
use crate::foxglove::lights_live_ui::{self, Live, NOW};
use crate::foxglove::shell::Page;
use crate::foxglove::theme::{self, Choice, DARK, LIGHT};
use foxglove_link::lights_owner::Owner;
use foxglove_link::protocol::RgbState;

fn report(state: Option<RgbState>, connected: bool) -> Report {
    Report {
        connected,
        rgb: state,
        dim: [100, 100],
        windows: None,
        right_apart: false,
    }
}

/// One row per state: its caption and the drawing the page would build.
fn states() -> Vec<(&'static str, Live)> {
    let swirl = rgb(half(true, 3, 26, 60), None);
    let spectrum = rgb(half(true, 2, 0, 60), None);
    let two = rgb(half(true, 1, 26, 60), Some(half(true, 0, 220, 40)));
    let solid = rgb(half(true, 0, 26, 80), None);
    let off = rgb(half(false, 0, 0, 60), None);
    let cannot = rgb(half(true, 7, 26, 60), None);
    let own = |s: RgbState| {
        live_of(
            look(&report(Some(s), true), None, None, None, None),
            Some(&s),
            true,
        )
    };
    let mut v = vec![
        ("Swirl in orange (moving)", own(swirl)),
        ("Spectrum, its own colours (moving)", own(spectrum)),
        ("Halves differ", own(two)),
        ("Solid", own(solid)),
        ("Lights off", own(off)),
        ("An effect Foxglove cannot draw", own(cannot)),
    ];
    let l = look(
        &report(Some(swirl), true),
        Some(Owner::Other(7)),
        None,
        None,
        None,
    );
    v.push((
        "Another app sets the lights",
        live_of(l, Some(&swirl), true),
    ));
    let l = look(&report(Some(swirl), true), None, None, None, None);
    v.push((
        "Windows animation effects off",
        live_of(l, Some(&swirl), false),
    ));
    let lease = crate::foxglove::lighting_leases::Lease {
        id: 1,
        conn: 9,
        device: "kb".into(),
        priority: crate::foxglove::lighting_leases::Priority::High,
        hue: 0,
        sat: 100,
        brightness: 100,
        effect: "0".into(),
    };
    let l = look(
        &report(Some(swirl), true),
        None,
        Some(("Mic muted", &lease)),
        None,
        None,
    );
    v.push((
        "A plugin's lease shows over the lights",
        live_of(l, Some(&swirl), true),
    ));
    let mut windows = report(Some(swirl), true);
    windows.windows = Some(crate::foxglove::scene_card::CardLook {
        lights: crate::foxglove::pc_lights::KeyboardLights {
            hue: 26,
            sat: 100,
            brightness: 60,
            effect: 3,
        },
        speed: 3,
        in_colour: true,
    });
    let l = look(&windows, Some(Owner::LampArrayUsb), None, None, None);
    v.push((
        "Foxglove draws through Windows",
        live_of(l, Some(&swirl), true),
    ));
    let l = look(&report(Some(swirl), true), None, None, Some(&scene()), None);
    v.push((
        "A scene runs (preview room is empty, so outlines)",
        live_of(l, Some(&swirl), true),
    ));
    // A pick sent (Solid orange -> Swirl): drawn at once, "Sending…" until the keyboard reports.
    let sent = crate::foxglove::lights_draft::shown(
        &crate::foxglove::lights_page::Draft::default(),
        &solid,
        Some(&crate::foxglove::lights_draft::Sent {
            change: crate::foxglove::lights_draft::Change::Effect(3),
            at: std::time::Instant::now(),
        }),
    )
    .expect("a change");
    let drafted = RgbState {
        left: sent.left.expect("left"),
        ..solid
    };
    let l = look(&report(Some(solid), true), None, None, None, Some(&sent));
    v.push((
        "Effect picked, keyboard not reported yet (Sending…)",
        live_of(l, Some(&drafted), true),
    ));
    // Right half's lights apart from the left's, nothing heard from it: outlines only.
    let mut apart = report(Some(solid), true);
    apart.right_apart = true;
    let l = look(&apart, None, None, None, None);
    v.push((
        "Right follows left is off, right half not reported",
        live_of(l, Some(&solid), true),
    ));
    let l = look(&report(None, true), None, None, None, None);
    v.push(("Reading the keyboard", live_of(l, None, true)));
    let l = look(&report(None, false), None, None, None, None);
    v.push(("Keyboard not connected", live_of(l, None, true)));
    v
}

fn scene() -> SceneRun {
    SceneRun {
        name: "Hearth".into(),
        effect: crate::foxglove::scene_run::demo_effect(),
        room: foxglove_scene::RoomMap::default(),
    }
}

fn sheet(choice: Choice, width: f32, tag: &str) -> image::RgbaImage {
    let ctx = preview_ctx();
    theme::apply_choice(&ctx, choice);
    let p = if choice == Choice::Dark {
        *DARK
    } else {
        *LIGHT
    };
    let rows = states();
    let height = 40.0 + rows.len() as f32 * 190.0;
    let _ = tag;
    render_ui(&ctx, egui::vec2(width, height), p.ground, |ui| {
        egui::Frame::new()
            .fill(p.ground)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                theme::card(ui).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    for (caption, live) in &rows {
                        ui.label(egui::RichText::new(*caption).color(p.muted).size(11.5));
                        ui.push_id(*caption, |ui| lights_live_ui::show(ui, live));
                        ui.add_space(10.0);
                    }
                });
            });
    })
}

#[test]
fn lights_live_renders() {
    NOW.set(Some(1_234));
    let mut images = Vec::new();
    for (choice, theme_name) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let mut s = Samples::full();
        let swirl = rgb(half(true, 3, 26, 60), None);
        s.lights.state = Some(swirl);
        s.lights.live = Some(live_of(
            look(&report(Some(swirl), true), None, None, None, None),
            Some(&swirl),
            true,
        ));
        let page = shell_preview_in(Page::Lights, &s, 900.0, choice);
        keep(&format!("ui-lights-live-page-{theme_name}.png"), &page);
        let states = sheet(choice, 900.0, "states");
        keep(&format!("ui-lights-live-states-{theme_name}.png"), &states);
        let narrow = sheet(choice, 420.0, "narrow");
        keep(&format!("ui-lights-live-narrow-{theme_name}.png"), &narrow);
        images.push((page, states));
    }
    NOW.set(None);
    assert_ne!(images[0].0, images[1].0, "dark and light differ");
    assert_ne!(images[0].1, images[1].1, "dark and light differ");
    let _ = layout();
}
