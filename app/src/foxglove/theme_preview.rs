//! Window-free previews of the Settings look (#384): egui runs headless, and a small CPU
//! rasterizer paints its meshes into a PNG. No window, no GPU.
//!
//! Set `FOXGLOVE_UI_PREVIEW_DIR` to a folder before `cargo test` to keep the PNGs
//! (`ui-384-before.png`, `ui-384-after-dark.png`, `ui-384-after-light.png`).

use std::cell::Cell;
use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, TextureId};

thread_local! { static LEGACY: Cell<bool> = const { Cell::new(false) }; }

/// True while a "before #384" preview is drawn on this thread.
pub fn legacy() -> bool {
    LEGACY.get()
}

const PPP: f32 = 1.5;

struct Texture {
    size: [usize; 2],
    pixels: Vec<Color32>,
}

/// Runs `draw` for a few frames (so windows finish fading in and sizes settle) and paints the
/// last one over `backdrop`.
fn render(ctx: &egui::Context, size: egui::Vec2, backdrop: Color32, mut draw: impl FnMut(&egui::Context)) -> image::RgbaImage {
    let mut textures: HashMap<TextureId, Texture> = HashMap::new();
    let mut last = None;
    for frame in 0..8 {
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(frame as f64),
            predicted_dt: 1.0,
            focused: true,
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(PPP);
        let mut out = ctx.run_ui(raw, |ui| draw(&ui.ctx().clone()));
        for (id, delta) in out.textures_delta.set.iter().flat_map(|(id, deltas)| deltas.iter().map(move |d| (id, d))) {
            let egui::ImageData::Color(image) = &delta.image;
            match delta.pos {
                None => {
                    textures.insert(*id, Texture { size: image.size, pixels: image.pixels.clone() });
                }
                Some([x0, y0]) => {
                    let t = textures.get_mut(id).expect("a patch for a known texture");
                    for y in 0..image.size[1] {
                        for x in 0..image.size[0] {
                            t.pixels[(y0 + y) * t.size[0] + x0 + x] = image.pixels[y * image.size[0] + x];
                        }
                    }
                }
            }
        }
        for id in &out.textures_delta.free {
            textures.remove(id);
        }
        // Applied above; egui checks that deltas aren't dropped unhandled.
        out.textures_delta.clear();
        last = Some((out.shapes, out.pixels_per_point));
    }
    let (shapes, ppp) = last.expect("frames ran");
    let primitives = ctx.tessellate(shapes, ppp);

    let (w, h) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
    let mut px = vec![[backdrop.r() as f32, backdrop.g() as f32, backdrop.b() as f32, 255.0]; w * h];
    for prim in primitives {
        let egui::epaint::Primitive::Mesh(mesh) = prim.primitive else { continue };
        let Some(tex) = textures.get(&mesh.texture_id) else { continue };
        let clip = Rect::from_min_max(
            (prim.clip_rect.min.to_vec2() * ppp).to_pos2(),
            (prim.clip_rect.max.to_vec2() * ppp).to_pos2(),
        );
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [0, 1, 2].map(|i| mesh.vertices[tri[i] as usize]);
            fill_triangle(&mut px, w, h, clip, ppp, &v, tex);
        }
    }
    let mut img = image::RgbaImage::new(w as u32, h as u32);
    for (i, p) in px.iter().enumerate() {
        img.put_pixel((i % w) as u32, (i / w) as u32, image::Rgba([p[0] as u8, p[1] as u8, p[2] as u8, 255]));
    }
    img
}

fn sample(tex: &Texture, u: f32, v: f32) -> [f32; 4] {
    // Bilinear, in the texture's own (premultiplied, gamma) space, like egui's glow painter.
    let (tw, th) = (tex.size[0] as f32, tex.size[1] as f32);
    let x = (u * tw - 0.5).clamp(0.0, tw - 1.0);
    let y = (v * th - 0.5).clamp(0.0, th - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(tex.size[0] - 1), (y0 + 1).min(tex.size[1] - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| tex.pixels[y * tex.size[0] + x].to_array().map(|c| c as f32);
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
}

fn fill_triangle(px: &mut [[f32; 4]], w: usize, h: usize, clip: Rect, ppp: f32, v: &[egui::epaint::Vertex; 3], tex: &Texture) {
    let p = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
    let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = p.iter().map(|q| q.0).fold(f32::MAX, f32::min).max(clip.min.x).max(0.0).floor() as usize;
    let max_x = p.iter().map(|q| q.0).fold(f32::MIN, f32::max).min(clip.max.x).min(w as f32).ceil() as usize;
    let min_y = p.iter().map(|q| q.1).fold(f32::MAX, f32::min).max(clip.min.y).max(0.0).floor() as usize;
    let max_y = p.iter().map(|q| q.1).fold(f32::MIN, f32::max).min(clip.max.y).min(h as f32).ceil() as usize;
    let colors = v.map(|v| v.color.to_array().map(|c| c as f32 / 255.0));
    for y in min_y..max_y {
        for x in min_x..max_x {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            if cx < clip.min.x || cx >= clip.max.x || cy < clip.min.y || cy >= clip.max.y {
                continue;
            }
            let edge = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (cy - a.1) - (b.1 - a.1) * (cx - a.0);
            let w0 = edge(p[1], p[2]) / area;
            let w1 = edge(p[2], p[0]) / area;
            let w2 = edge(p[0], p[1]) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
            let t = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
            let texel = sample(tex, u, t);
            let src: [f32; 4] =
                std::array::from_fn(|i| (w0 * colors[0][i] + w1 * colors[1][i] + w2 * colors[2][i]) * texel[i]);
            let dst = &mut px[y * w + x];
            let keep = 1.0 - src[3] / 255.0;
            for (d, s) in dst.iter_mut().zip(src).take(3) {
                *d = (s + *d * keep).clamp(0.0, 255.0);
            }
        }
    }
}

/// The Settings window as it opens with no keyboard connected: KeyPeek's Connection, Overlay
/// Appearance and Theme groups drawn with the same widgets and labels, then Foxglove's own
/// Profile and Updates groups and the footer. The real window needs the app (tray icon,
/// connection), so this stands in for it.
fn settings_window(ctx: &egui::Context, legacy: bool) {
    use crate::foxglove::settings_ui::section;
    egui::Window::new("Foxglove Settings")
        .fixed_pos(egui::pos2(24.0, 24.0))
        .fixed_size(egui::vec2(450.0, 600.0))
        .collapsible(false)
        .resizable(false)
        .open(&mut true)
        .vscroll(true)
        .show(ctx, |ui| {
            section(ui, "Connection", |ui| {
                egui::Grid::new("connection_grid").num_columns(2).striped(true).spacing([20.0, 10.0]).show(ui, |ui| {
                    ui.label("Device");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("device_combo")
                            .width(ui.available_width() - 108.0)
                            .selected_text("Eyelash Corne (USB)")
                            .show_ui(ui, |_| {});
                        ui.add_sized([100.0, 20.0], egui::Button::new("Connect"));
                    });
                    ui.end_row();
                    ui.label("Layout");
                    ui.add_enabled_ui(false, |ui| {
                        egui::ComboBox::from_id_salt("layout_combo")
                            .width(ui.available_width() - 108.0)
                            .selected_text("Connect to device first")
                            .show_ui(ui, |_| {});
                    });
                    ui.end_row();
                });
            });
            section(ui, "Overlay Appearance", |ui| {
                egui::Grid::new("appearance_grid").num_columns(2).striped(true).spacing([20.0, 10.0]).show(ui, |ui| {
                    ui.label("Alignment");
                    egui::ComboBox::from_id_salt("position_combo")
                        .width(ui.available_width())
                        .selected_text(crate::settings::WindowPosition::BottomRight.to_string())
                        .show_ui(ui, |_| {});
                    ui.end_row();
                    for (label, value, suffix) in [
                        ("Display duration", 2000.0, " ms"),
                        ("Activation delay", 0.0, " ms"),
                        ("Distance from screen edge", 10.0, " px"),
                        ("Key unit size", 60.0, " px"),
                    ] {
                        ui.label(label);
                        let mut v: f64 = value;
                        ui.add_sized(ui.available_size(), egui::DragValue::new(&mut v).suffix(suffix));
                        ui.end_row();
                    }
                    ui.label("Auto-fit long labels");
                    ui.checkbox(&mut true, "Fit long labels to available space");
                    ui.end_row();
                    ui.label("Legend mode");
                    egui::ComboBox::from_id_salt("legend_mode_combo")
                        .width(ui.available_width())
                        .selected_text(crate::settings::LegendMode::Stacked.to_string())
                        .show_ui(ui, |_| {});
                    ui.end_row();
                });
            });
            section(ui, "Theme", |ui| {
                if !legacy {
                    crate::foxglove::theme::picker(ui);
                }
                let mut white = Color32::WHITE;
                ui.horizontal(|ui| {
                    ui.label("Font color");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.color_edit_button_srgba(&mut white);
                    });
                });
                ui.add_space(4.0);
                let colors = crate::settings::ThemeSettings::default().layer_colors;
                ui.columns(2, |cols| {
                    for (col, layers) in cols.iter_mut().zip([0..4, 4..8]) {
                        egui::Grid::new(("layers", layers.start)).num_columns(4).spacing([12.0, 6.0]).show(col, |ui| {
                            ui.weak("Show");
                            ui.weak("Hold");
                            ui.weak("Layer");
                            ui.label("");
                            ui.end_row();
                            for layer in layers {
                                ui.add(egui::Checkbox::without_text(&mut true));
                                ui.add(egui::Checkbox::without_text(&mut (layer != 3)));
                                ui.label(if layer == 7 { "Other layers".to_string() } else { format!("Layer {layer}") });
                                let mut c = crate::key_paint::to_egui_color(colors[layer]);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.color_edit_button_srgba(&mut c);
                                });
                                ui.end_row();
                            }
                        });
                    }
                });
            });
            crate::foxglove::profiles_ui::picker(ui);
            crate::foxglove::update::group(ui);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::Hyperlink::from_label_and_url(
                    egui::RichText::new("github.com/BanditF/foxglove").weak(),
                    "https://github.com/BanditF/foxglove",
                ));
                ui.add(egui::Hyperlink::from_label_and_url(
                    egui::RichText::new("based on KeyPeek by Stephan Rumswinkel").weak(),
                    "https://github.com/srwi/keypeek",
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Hyperlink::from_label_and_url(
                        egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).weak(),
                        "https://github.com/BanditF/foxglove/releases",
                    ));
                });
            });
            if !legacy {
                crate::foxglove::theme::credits(ui);
            }
        });
}

/// The theme's widgets in their states, for the review (only drawn on the "after" previews).
fn widget_sheet(ctx: &egui::Context, focus: bool) {
    use crate::foxglove::theme;
    egui::Window::new("Widgets")
        .fixed_pos(egui::pos2(510.0, 24.0))
        .fixed_size(egui::vec2(320.0, 400.0))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("FOXGLOVE").font(theme::hud(22.0)).color(theme::palette(ui).text_strong));
            ui.label(theme::caps_label(ui, "Steel for good"));
            ui.add_space(8.0);
            theme::picker(ui);
            theme::card(ui).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(theme::heading(ui, "Underglow"));
                ui.label(theme::caps_label(ui, "Effect"));
                ui.horizontal(|ui| {
                    for (i, e) in ["Solid", "Breathe", "Spectrum", "Swirl"].iter().enumerate() {
                        theme::pill(ui, i == 0, e);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Brightness");
                    ui.add(egui::Slider::new(&mut 80.0, 0.0..=100.0).suffix("%"));
                });
                ui.checkbox(&mut true, "Right half's lights follow the left");
                ui.checkbox(&mut false, "Follow the Windows accent colour");
                ui.horizontal(|ui| {
                    let r = ui.button("Ask the keyboard again");
                    if focus {
                        r.request_focus();
                    }
                    ui.add(theme::primary_button(ui, "Apply colour"));
                });
                ui.label(egui::RichText::new("0.1.0 (dev) · e255d32").font(theme::mono(12.5)));
                let p = theme::palette(ui);
                ui.horizontal(|ui| {
                    ui.colored_label(p.ok, "Left on USB");
                    ui.colored_label(p.warn, "Right no reply yet");
                    ui.colored_label(p.danger, "Reset");
                });
                ui.hyperlink_to("Foxglove releases on GitHub", "https://github.com/BanditF/foxglove/releases");
            });
            ui.add_space(6.0);
            ui.weak("Above: a focused button (orange ring), the theme picker, pills, the slider and checkboxes.");
        });
}

fn preview(legacy: bool, choice: crate::foxglove::theme::Choice) -> image::RgbaImage {
    crate::foxglove::store::test_config_dir();
    LEGACY.set(legacy);
    let ctx = egui::Context::default();
    let mut fonts = egui::FontDefinitions::default();
    crate::platform::add_phosphor_to_fonts(&mut fonts);
    if !legacy {
        crate::foxglove::theme::add_fonts(&mut fonts);
    }
    ctx.set_fonts(fonts);
    if !legacy {
        crate::foxglove::theme::apply_choice(&ctx, choice);
    }
    let width = if legacy { 500.0 } else { 860.0 };
    // The desktop behind, dimmed the way the overlay dims it while Settings is open.
    let backdrop = Color32::from_rgb(0x09, 0x0a, 0x0c);
    let mut frame = 0;
    let img = render(&ctx, egui::vec2(width, 690.0), backdrop, |ctx| {
        settings_window(ctx, legacy);
        if !legacy {
            widget_sheet(ctx, frame >= 2);
        }
        frame += 1;
    });
    LEGACY.set(false);
    img
}

fn keep(name: &str, img: &image::RgbaImage) {
    if let Some(dir) = std::env::var_os("FOXGLOVE_UI_PREVIEW_DIR") {
        let path = std::path::Path::new(&dir).join(name);
        img.save(&path).expect("save the preview");
        eprintln!("Foxglove: preview saved to {}", path.display());
    }
}

#[test]
fn settings_preview_renders_in_the_theme() {
    use crate::foxglove::theme::{Choice, DARK, LIGHT};
    let before = preview(true, Choice::Dark);
    let dark = preview(false, Choice::Dark);
    let light = preview(false, Choice::Light);
    keep("ui-384-before.png", &before);
    keep("ui-384-after-dark.png", &dark);
    keep("ui-384-after-light.png", &light);

    let count = |img: &image::RgbaImage, c: Color32| img.pixels().filter(|p| p.0[..3] == [c.r(), c.g(), c.b()]).count();
    let total = (dark.width() * dark.height()) as usize;
    // The window's ground, the cards and the orange all show up; before, none of them did.
    assert!(count(&dark, DARK.ground) > total / 50, "dark ground");
    assert!(count(&dark, DARK.surface) > total / 50, "dark cards");
    assert!(count(&dark, DARK.accent) > 50, "orange");
    assert!(count(&light, LIGHT.ground) > total / 50, "light ground");
    assert_eq!(count(&before, DARK.ground), 0);
}
