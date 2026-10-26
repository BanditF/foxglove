//! Previews and window-free checks of the layer overlay's opacity (`foxglove::overlay_opacity`):
//! the overlay at several settings, the Settings rows with the 20% detent, and the slider driven
//! by simulated pointer and key events.

use super::{keep, preview_ctx, render, render_ui, PPP};
use crate::foxglove::overlay_opacity::{self, factors, label};
use egui::{Color32, Pos2, Rect};

/// The layer overlay over a wallpaper-like backdrop at: 100% (the look it always had), 60%, the
/// 20% detent, 20% with the key labels unlinked at 90%, and 0% while it's dragged (hidden, with
/// a faint outline).
#[test]
fn overlay_opacity_renders() {
    use crate::layout_key::{KeycodeKind, Label, LayoutKey};
    let ctx = preview_ctx();
    let settings = crate::settings::Settings::default();
    let unit = 48.0;
    let labels = ["Q", "W", "E", "R", "T", "Esc"];
    // (HUD, the labels' own, outline, caption)
    let rows: [(u8, Option<u8>, bool, &str); 5] = [
        (100, None, false, "100%"),
        (60, None, false, "60%"),
        (20, None, false, "20%"),
        (20, Some(90), false, "20% / labels 90%"),
        (0, None, true, "0%, dragged"),
    ];
    let left = 200.0;
    let size = egui::vec2(
        labels.len() as f32 * unit + left + 24.0,
        rows.len() as f32 * (unit + 16.0) + 16.0,
    );
    let img = render(&ctx, size, Color32::from_rgb(0x2a, 0x4a, 0x6a), |ctx| {
        egui::Area::new(egui::Id::new("opacity-preview"))
            .fixed_pos(Pos2::ZERO)
            .show(ctx, |ui| {
                // Stripes behind the keys, so the see-through shows.
                let full = Rect::from_min_size(Pos2::ZERO, size);
                for i in 0..((full.width() / 24.0) as usize + 1) {
                    if i % 2 == 0 {
                        let x = full.left() + i as f32 * 24.0;
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                egui::pos2(x, full.top()),
                                egui::pos2(x + 12.0, full.bottom()),
                            ),
                            0.0,
                            Color32::from_rgb(0xd8, 0xc0, 0x90),
                        );
                    }
                }
                for (row, (hud, own, outline, caption)) in rows.iter().enumerate() {
                    let top = full.top() + 16.0 + row as f32 * (unit + 16.0);
                    ui.painter().text(
                        egui::pos2(full.left() + 12.0, top + unit / 2.0),
                        egui::Align2::LEFT_CENTER,
                        *caption,
                        egui::FontId::proportional(16.0),
                        Color32::WHITE,
                    );
                    // As overlay_window/ui_overlay.rs draws it.
                    let (whole, face, text) = factors(*hud, label(*hud, *own));
                    let style = crate::key_paint::KeyPaintStyle::from_settings(&settings)
                        .with_unit(unit)
                        .with_opacity(face, text);
                    let mut keys = ui.new_child(egui::UiBuilder::new().max_rect(full));
                    let span = Rect::from_min_size(
                        egui::pos2(full.left() + left, top),
                        egui::vec2(labels.len() as f32 * unit, unit),
                    );
                    if *outline {
                        overlay_opacity::paint_outline(&keys, span);
                    }
                    keys.multiply_opacity(whole);
                    for (col, label) in labels.iter().enumerate() {
                        let rect = Rect::from_min_size(
                            egui::pos2(span.left() + col as f32 * unit, top),
                            egui::vec2(unit, unit),
                        );
                        let key = LayoutKey {
                            tap: Label::new(*label),
                            kind: KeycodeKind::Basic,
                            ..LayoutKey::default()
                        };
                        crate::key_paint::paint(
                            &keys,
                            rect,
                            0.0,
                            &crate::key_paint::KeyDisplay {
                                key: &key,
                                colors: style.colors_for(0, KeycodeKind::Basic, false, false),
                                hovered: false,
                                pressed: false,
                                shift_held: false,
                                ralt_held: false,
                            },
                            &style,
                        );
                    }
                }
            });
    });
    keep("ui-overlay-opacity.png", &img);
    // A key's face, low in its first key (below the label), and the backdrop between rows.
    let x = ((left + unit * 0.5) * PPP) as u32;
    let face = |row: f32| {
        let y = ((16.0 + row * (unit + 16.0) + unit * 0.85) * PPP) as u32;
        img.get_pixel(x, y).0
    };
    let backdrop = img.get_pixel(x, (8.0 * PPP) as u32).0;
    assert_ne!(face(0.0), face(2.0), "20% looks like 100%");
    assert_eq!(
        face(2.0),
        face(3.0),
        "the labels' own opacity changed the faces"
    );
    assert_eq!(face(4.0), backdrop, "0% still shows the keys");
}

/// Settings → Layer overlay's two opacity rows: the HUD at the 20% detent (the nub under the
/// rail) with the labels following it, then the labels unlinked at 80%.
#[test]
fn opacity_rows_render() {
    use crate::foxglove::theme::DARK;
    let ctx = preview_ctx();
    let states = [
        (20u8, None, "Labels follow the HUD"),
        (20, Some(80), "Labels unlinked"),
    ];
    let size = egui::vec2(470.0, 196.0);
    let img = render_ui(&ctx, size, DARK.ground, |ui| {
        egui::Frame::new().inner_margin(16.0).show(ui, |ui| {
            for (n, (hud, own, title)) in states.iter().enumerate() {
                ui.strong(*title);
                egui::Grid::new(("opacity-rows", n))
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        overlay_opacity::rows(
                            ui,
                            &mut crate::settings::Settings {
                                opacity: *hud,
                                label_opacity: *own,
                                ..Default::default()
                            },
                        );
                    });
                ui.add_space(12.0);
            }
        });
    });
    keep("ui-overlay-opacity-rows.png", &img);
}

/// Runs one frame of a lone opacity slider at the top left with `events`.
fn slider_frame(ctx: &egui::Context, value: &mut u8, events: Vec<egui::Event>, t: f64) {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 100.0))),
        time: Some(t),
        events,
        focused: true,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| {
        egui::Area::new(egui::Id::new("opacity-slider"))
            .fixed_pos(Pos2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.spacing_mut().window_margin = egui::Margin::ZERO;
                overlay_opacity::slider(ui, value, "test");
            });
    });
    out.textures_delta.clear();
}

/// The slider's x for `percent`, as egui lays out its rail.
fn slider_x(ctx: &egui::Context, percent: f32) -> f32 {
    let style = ctx.global_style();
    let width = style.spacing.slider_width;
    let radius = style.spacing.interact_size.y / 2.5;
    let radius = match style.visuals.handle_shape {
        egui::style::HandleShape::Circle => radius,
        egui::style::HandleShape::Rect { aspect_ratio } => radius * aspect_ratio,
    };
    radius + (width - 2.0 * radius) * percent / 100.0
}

/// A press, a move to `to` percent, and a release on the slider, with `modifiers` held.
fn drag_slider(ctx: &egui::Context, value: &mut u8, from: f32, to: f32, alt: bool) -> u8 {
    // egui's clock only goes forward, across the drags of one test.
    let t = ctx.input(|i| i.time) + 1.0;
    let y = ctx.global_style().spacing.interact_size.y / 2.0;
    let modifiers = egui::Modifiers {
        alt,
        ..Default::default()
    };
    let at = |p: f32| egui::pos2(slider_x(ctx, p), y);
    let button = |p: f32, pressed: bool| egui::Event::PointerButton {
        pos: at(p),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    // A new area lays itself out unseen first; then the pointer comes in and presses.
    slider_frame(ctx, value, vec![], t);
    slider_frame(
        ctx,
        value,
        vec![egui::Event::PointerMoved(at(from))],
        t + 0.05,
    );
    slider_frame(ctx, value, vec![button(from, true)], t + 0.1);
    slider_frame(
        ctx,
        value,
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(at(to)),
        ],
        t + 0.2,
    );
    let during = *value;
    slider_frame(ctx, value, vec![button(to, false)], t + 0.3);
    during
}

#[test]
fn dragging_near_twenty_rests_on_it_and_alt_passes() {
    let ctx = preview_ctx();
    let mut value = 60u8;
    let during = drag_slider(&ctx, &mut value, 60.0, 22.0, false);
    assert_eq!((during, value), (20, 20), "rests on the detent");

    let mut value = 60u8;
    drag_slider(&ctx, &mut value, 60.0, 22.0, true);
    assert!(value.abs_diff(22) <= 1, "Alt passes it: {value}");

    let mut value = 60u8;
    drag_slider(&ctx, &mut value, 60.0, 0.0, false);
    assert_eq!(value, 0, "all the way to zero");

    let mut value = 60u8;
    drag_slider(&ctx, &mut value, 60.0, 40.0, false);
    assert!(value.abs_diff(40) <= 1, "far from it: no pull ({value})");
}

#[test]
fn arrow_keys_step_one_past_the_detent() {
    let ctx = preview_ctx();
    let mut value = 60u8;
    drag_slider(&ctx, &mut value, 60.0, 21.0, false);
    assert_eq!(value, 20);
    // The click left the slider focused: arrows step 1% and don't snap back.
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    slider_frame(&ctx, &mut value, vec![key(egui::Key::ArrowLeft)], 10.0);
    assert_eq!(value, 19);
    slider_frame(&ctx, &mut value, vec![key(egui::Key::ArrowRight)], 10.1);
    slider_frame(&ctx, &mut value, vec![key(egui::Key::ArrowRight)], 10.2);
    assert_eq!(value, 21);
}
