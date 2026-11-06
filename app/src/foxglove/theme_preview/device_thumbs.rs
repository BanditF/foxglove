//! Preview sheets of the device thumbnails (#803), dark and light: every shape lit, off and
//! unknown (`ui-803-shapes-{dark,light}.png`), and each fixture desk's devices
//! (`ui-803-fixtures-{dark,light}.png`). Kept with `FOXGLOVE_UI_PREVIEW_DIR` set.

use super::{keep, preview_ctx, render_ui};
use crate::foxglove::device_thumb::tests::{fixture_thumbs, rainbow, sample};
use crate::foxglove::device_thumb::{self, Shape, Thumb, Tint, ROW, SMALL};
use crate::foxglove::theme::{self, Choice, DARK, LIGHT};

/// One sheet: a row per thumbnail, its label, then lit, off, unknown, lit in the battery list's
/// size and lit at twice the row size (for a closer look).
fn sheet(choice: Choice, title: &str, rows: &[(String, Thumb, usize)]) -> image::RgbaImage {
    let ctx = preview_ctx();
    theme::apply_choice(&ctx, choice);
    let p = if choice == Choice::Dark {
        *DARK
    } else {
        *LIGHT
    };
    let height = 140.0 + rows.len() as f32 * 92.0;
    render_ui(&ctx, egui::vec2(920.0, height), p.ground, |ui| {
        egui::Frame::new()
            .fill(p.ground)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                theme::card(ui).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(theme::heading(ui, title));
                    ui.add_space(6.0);
                    egui::Grid::new("thumbs")
                        .num_columns(6)
                        .spacing([18.0, 10.0])
                        .min_row_height(ROW.y * 2.0)
                        .show(ui, |ui| {
                            for head in ["", "LIT", "OFF", "UNKNOWN", "LIT 44×30", "LIT ×2"] {
                                ui.label(theme::caps_label(ui, head));
                            }
                            ui.end_row();
                            for (label, thumb, lights) in rows {
                                ui.label(label.as_str());
                                let with = |tint: Tint| Thumb {
                                    tint: Some(tint),
                                    ..thumb.clone()
                                };
                                device_thumb::show(ui, ROW, &with(rainbow(*lights)));
                                device_thumb::show(ui, ROW, &with(Tint::Off));
                                device_thumb::show(ui, ROW, &with(Tint::Unknown));
                                device_thumb::show(ui, SMALL, &with(rainbow(*lights)));
                                device_thumb::show(ui, ROW * 2.0, &with(rainbow(*lights)));
                                ui.end_row();
                            }
                        });
                });
            });
    })
}

/// Every shape, the keyboard from a Studio layout and from a profile.
fn shape_rows() -> Vec<(String, Thumb, usize)> {
    let corne_profile = crate::foxglove::key_layout::resolve(
        Some(&crate::foxglove::fixtures::users_desk::board()),
        &[],
        42,
    );
    let mut rows = Vec::new();
    for shape in Shape::ALL {
        let thumb = sample(shape);
        let (label, lights) = match shape {
            Shape::Keyboard => ("Keyboard · profile (one piece)".to_string(), 61),
            Shape::Split => ("Split · ZMK Studio layout".to_string(), 42),
            Shape::Case => ("Case · 4 zones, one ambient".to_string(), 4),
            Shape::Strip => ("Strip · 12 lamps in a row".to_string(), 12),
            Shape::Mouse => ("Mouse · 2 lamps placed".to_string(), 2),
            Shape::Lamps => ("Lamps only · 9 placed".to_string(), 9),
            s => (s.label().to_string(), 3),
        };
        assert_eq!(thumb.shape(), shape, "{label}");
        rows.push((label, thumb, lights));
        if shape == Shape::Split {
            rows.push((
                "Split · board profile".into(),
                Thumb::keyboard(corne_profile.clone(), Tint::Off),
                42,
            ));
        }
        if shape == Shape::Keyboard {
            rows.push((
                "Keyboard · no layout known".into(),
                Thumb {
                    catalog_shape: Some(foxglove_devices::pc_lighting::Shape::Keyboard),
                    ..Default::default()
                },
                48,
            ));
        }
    }
    rows
}

#[test]
fn device_thumbs_render() {
    let shapes = shape_rows();
    let fixtures: Vec<(String, Thumb, usize)> = fixture_thumbs(rainbow)
        .into_iter()
        .map(|(name, thumb, shape)| {
            let lights = match thumb.tint.as_ref() {
                Some(Tint::Lit(cs)) => cs.len(),
                _ => 1,
            };
            (format!("{name} · {}", shape.label()), thumb, lights)
        })
        .collect();
    let mut images = Vec::new();
    for (choice, suffix) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let a = sheet(choice, "Device thumbnails · every shape", &shapes);
        let b = sheet(choice, "Device thumbnails · the fixture desks", &fixtures);
        keep(&format!("ui-803-shapes-{suffix}.png"), &a);
        keep(&format!("ui-803-fixtures-{suffix}.png"), &b);
        images.push((a, b));
    }
    assert_ne!(images[0].0, images[1].0, "dark and light differ");
    assert_ne!(images[0].0, images[0].1);
    // The rainbow's red key shows: the thumbnails are tinted.
    let red = images[0]
        .0
        .pixels()
        .filter(|p| p.0[0] > 200 && p.0[1] < 120 && p.0[2] < 120)
        .count();
    assert!(red > 20, "{red} red pixels");
}
