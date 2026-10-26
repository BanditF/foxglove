//! Window-free previews of the Settings look (#384): egui runs headless, and a small CPU
//! rasterizer paints its meshes into a PNG. No window, no GPU.
//!
//! Set `FOXGLOVE_UI_PREVIEW_DIR` to a folder before `cargo test` to keep the PNGs
//! (`ui-384-before.png`, `ui-384-after-dark.png`, `ui-384-after-light.png`, and one
//! `ui-385-<page>.png` per page of the #385 shell, then each redesigned page's own, such as
//! `ui-386-keyboard.png`).

use std::cell::Cell;
use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, TextureId};

mod layout;
mod overlay;

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
fn render(
    ctx: &egui::Context,
    size: egui::Vec2,
    backdrop: Color32,
    mut draw: impl FnMut(&egui::Context),
) -> image::RgbaImage {
    render_ui(ctx, size, backdrop, |ui| draw(&ui.ctx().clone()))
}

/// [`render`] with the frame's root `Ui`, as a native window's viewport gets it.
fn render_ui(
    ctx: &egui::Context,
    size: egui::Vec2,
    backdrop: Color32,
    draw: impl FnMut(&mut egui::Ui),
) -> image::RgbaImage {
    let mut textures = HashMap::new();
    let (shapes, ppp) = run_frames(ctx, size, PPP, Some(&mut textures), draw);
    paint(ctx, size, backdrop, shapes, ppp, &textures)
}

/// Runs `draw` for a few frames at `ppp` pixels per point and returns the last frame's shapes.
/// With `textures`, keeps what egui sends for [`paint`]; without, drops it (a layout check).
fn run_frames(
    ctx: &egui::Context,
    size: egui::Vec2,
    ppp: f32,
    mut textures: Option<&mut HashMap<TextureId, Texture>>,
    mut draw: impl FnMut(&mut egui::Ui),
) -> (Vec<egui::epaint::ClippedShape>, f32) {
    let mut last = None;
    for frame in 0..8 {
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(frame as f64),
            predicted_dt: 1.0,
            focused: true,
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        let mut out = ctx.run_ui(raw, &mut draw);
        let Some(textures) = textures.as_deref_mut() else {
            out.textures_delta.clear();
            last = Some((out.shapes, out.pixels_per_point));
            continue;
        };
        for (id, delta) in out
            .textures_delta
            .set
            .iter()
            .flat_map(|(id, deltas)| deltas.iter().map(move |d| (id, d)))
        {
            let egui::ImageData::Color(image) = &delta.image;
            match delta.pos {
                None => {
                    textures.insert(
                        *id,
                        Texture {
                            size: image.size,
                            pixels: image.pixels.clone(),
                        },
                    );
                }
                Some([x0, y0]) => {
                    let t = textures.get_mut(id).expect("a patch for a known texture");
                    for y in 0..image.size[1] {
                        for x in 0..image.size[0] {
                            t.pixels[(y0 + y) * t.size[0] + x0 + x] =
                                image.pixels[y * image.size[0] + x];
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
    last.expect("frames ran")
}

/// Paints `shapes` over `backdrop` with the CPU rasterizer.
fn paint(
    ctx: &egui::Context,
    size: egui::Vec2,
    backdrop: Color32,
    shapes: Vec<egui::epaint::ClippedShape>,
    ppp: f32,
    textures: &HashMap<TextureId, Texture>,
) -> image::RgbaImage {
    let primitives = ctx.tessellate(shapes, ppp);

    let (w, h) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
    let mut px = vec![
        [
            backdrop.r() as f32,
            backdrop.g() as f32,
            backdrop.b() as f32,
            255.0
        ];
        w * h
    ];
    for prim in primitives {
        let egui::epaint::Primitive::Mesh(mesh) = prim.primitive else {
            continue;
        };
        let Some(tex) = textures.get(&mesh.texture_id) else {
            continue;
        };
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
        img.put_pixel(
            (i % w) as u32,
            (i / w) as u32,
            image::Rgba([p[0] as u8, p[1] as u8, p[2] as u8, 255]),
        );
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
    std::array::from_fn(|i| {
        (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
    })
}

fn fill_triangle(
    px: &mut [[f32; 4]],
    w: usize,
    h: usize,
    clip: Rect,
    ppp: f32,
    v: &[egui::epaint::Vertex; 3],
    tex: &Texture,
) {
    let p = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
    let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MAX, f32::min)
        .max(clip.min.x)
        .max(0.0)
        .floor() as usize;
    let max_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MIN, f32::max)
        .min(clip.max.x)
        .min(w as f32)
        .ceil() as usize;
    let min_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MAX, f32::min)
        .max(clip.min.y)
        .max(0.0)
        .floor() as usize;
    let max_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MIN, f32::max)
        .min(clip.max.y)
        .min(h as f32)
        .ceil() as usize;
    let colors = v.map(|v| v.color.to_array().map(|c| c as f32 / 255.0));
    for y in min_y..max_y {
        for x in min_x..max_x {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            if cx < clip.min.x || cx >= clip.max.x || cy < clip.min.y || cy >= clip.max.y {
                continue;
            }
            let edge =
                |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (cy - a.1) - (b.1 - a.1) * (cx - a.0);
            let w0 = edge(p[1], p[2]) / area;
            let w1 = edge(p[2], p[0]) / area;
            let w2 = edge(p[0], p[1]) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
            let t = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
            let texel = sample(tex, u, t);
            let src: [f32; 4] = std::array::from_fn(|i| {
                (w0 * colors[0][i] + w1 * colors[1][i] + w2 * colors[2][i]) * texel[i]
            });
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
    egui::Window::new("Foxglove Settings")
        .fixed_pos(egui::pos2(24.0, 24.0))
        .fixed_size(egui::vec2(450.0, 660.0))
        .collapsible(false)
        .resizable(false)
        .open(&mut true)
        .vscroll(true)
        .show(ctx, |ui| {
            keypeek_groups(ui);
            crate::foxglove::profiles_ui::picker(ui);
            crate::foxglove::update::group(ui);
            if !legacy {
                crate::foxglove::settings_ui::footer(ui);
                return;
            }
            // The footer before #384: one row, where the credit ran into the version.
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
                        egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                            .weak(),
                        "https://github.com/BanditF/foxglove/releases",
                    ));
                });
            });
        });
}

/// KeyPeek's Connection, Overlay Appearance and Theme groups, with the same widgets and labels.
fn keypeek_groups(ui: &mut egui::Ui) {
    use crate::foxglove::settings_ui::section;
    section(ui, "Connection", |ui| {
        egui::Grid::new("connection_grid")
            .num_columns(2)
            .striped(true)
            .spacing([20.0, 10.0])
            .show(ui, |ui| {
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
        egui::Grid::new("appearance_grid")
            .num_columns(2)
            .striped(true)
            .spacing([20.0, 10.0])
            .show(ui, |ui| {
                ui.label("Alignment");
                egui::ComboBox::from_id_salt("position_combo")
                    .width(ui.available_width())
                    .selected_text(crate::settings::WindowPosition::BottomRight.to_string())
                    .show_ui(ui, |_| {});
                ui.end_row();
                // As overlay_window/foxglove_place.rs draws it, with a dragged place.
                ui.label("Position");
                ui.horizontal(|ui| {
                    ui.label("Where you dragged it");
                    let _ = ui.small_button("Reset");
                });
                ui.end_row();
                for (label, value, suffix) in [
                    ("Display duration", 2000.0, " ms"),
                    ("Activation delay", 0.0, " ms"),
                    ("Distance from screen edge", 10.0, " px"),
                    ("Key unit size", 60.0, " px"),
                ] {
                    ui.label(label);
                    let mut v: f64 = value;
                    ui.add_sized(
                        ui.available_size(),
                        egui::DragValue::new(&mut v).suffix(suffix),
                    );
                    ui.end_row();
                }
                // As overlay_window/ui_settings.rs draws them: the HUD at its 20% detent, the
                // key labels unlinked at 80%.
                crate::foxglove::overlay_opacity::rows(
                    ui,
                    &mut crate::settings::Settings {
                        opacity: 20,
                        label_opacity: Some(80),
                        ..Default::default()
                    },
                );
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
        // The window theme is in "Theme and updates" since #390.
        let mut white = Color32::WHITE;
        ui.horizontal(|ui| {
            ui.label("Font color");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.color_edit_button_srgba(&mut white);
            });
        });
        ui.add_space(4.0);
        let colors = crate::settings::ThemeSettings::default().layer_colors;
        // As in ui_settings.rs: one table when the card is too narrow for two (#390).
        let narrow = ui.available_width() < 560.0;
        let parts: Vec<std::ops::Range<usize>> = if narrow {
            std::iter::once(0..8).collect()
        } else {
            vec![0..4, 4..8]
        };
        ui.columns(parts.len(), |cols| {
            for (col, layers) in cols.iter_mut().zip(parts) {
                egui::Grid::new(("layers", layers.start))
                    .num_columns(4)
                    .spacing([12.0, 6.0])
                    .show(col, |ui| {
                        ui.weak("Show");
                        ui.weak("Hold");
                        ui.weak("Layer");
                        ui.label("");
                        ui.end_row();
                        for layer in layers {
                            ui.add(egui::Checkbox::without_text(&mut true));
                            ui.add(egui::Checkbox::without_text(&mut (layer != 3)));
                            ui.label(if layer == 7 {
                                "Other layers".to_string()
                            } else {
                                format!("Layer {layer}")
                            });
                            let mut c = crate::key_paint::to_egui_color(colors[layer]);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.color_edit_button_srgba(&mut c);
                                },
                            );
                            ui.end_row();
                        }
                    });
            }
        });
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
    let img = render(&ctx, egui::vec2(width, 750.0), backdrop, |ctx| {
        settings_window(ctx, legacy);
        if !legacy {
            widget_sheet(ctx, frame >= 2);
        }
        frame += 1;
    });
    LEGACY.set(false);
    img
}

/// Foxglove's own sections as stand-ins: the real ones need a keyboard (and would talk to it),
/// so these use the same widgets and labels with sample values.
fn foxglove_groups(ui: &mut egui::Ui) {
    use crate::foxglove::settings_ui::section;
    // The real Lights page (#387), drawn from its preview sample.
    section(ui, "Lights", crate::foxglove::lights_page::show);
    section(ui, "Other keyboards (VIA)", |ui| {
        ui.weak("QMK keyboards with VIA, like the Keychron K5 Pro, over USB. Foxglove only reads them: the keymap, layers and lights.");
        ui.checkbox(&mut true, "Use VIA keyboards");
        ui.horizontal(|ui| {
            let _ = ui.button("Add VIA definition (JSON)…");
            let _ = ui.button("Rescan");
        });
    });
    // The real Plugins page (#389), drawn from its preview sample.
    crate::foxglove::plugins_ui::group(ui);
}

/// The settings window as #385 draws it: the sidebar, the status strip and one page.
fn shell_window(ctx: &egui::Context, size: egui::Vec2) -> Option<Rect> {
    egui::Window::new("Foxglove Settings")
        .fixed_pos(egui::pos2(16.0, 16.0))
        .default_size(size)
        .min_size(egui::vec2(720.0, 420.0))
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::ZERO))
        .collapsible(false)
        .resizable(true)
        .open(&mut true)
        .show(ctx, |ui| {
            crate::foxglove::shell::frame(ui, sections)
        })
        .map(|r| r.response.rect)
}

/// The settings window's sections, in the real window's order (ui_settings.rs).
fn sections(ui: &mut egui::Ui) {
    keypeek_groups(ui);
    crate::foxglove::profiles_ui::picker(ui);
    crate::foxglove::update::group(ui);
    foxglove_groups(ui);
    crate::foxglove::settings_ui::footer(ui);
}

fn shell_preview(page: crate::foxglove::shell::Page) -> image::RgbaImage {
    shell_preview_with(page, &Samples::full(), 660.0)
}

/// What the previews' live parts show: with a keyboard, or with none.
struct Samples {
    strip: crate::foxglove::shell::StripState,
    keyboard: crate::foxglove::keyboard_page::View,
    lights: crate::foxglove::lights_page::View,
    pc: crate::foxglove::pc_lights_ui::View,
    wizard: crate::foxglove::pc_lights_add::Wizard,
    /// The colour calibrator, open in place of the PC card.
    calibrate: Option<(
        crate::foxglove::calibrate_ui::View,
        crate::foxglove::calibrate_ui::Wizard,
    )>,
    screens: Option<crate::foxglove::screens::Snapshot>,
    art: Option<(
        crate::foxglove::art_service::Snapshot,
        crate::foxglove::art_library::Library,
    )>,
    plugins: crate::foxglove::plugins_ui::View,
}

impl Samples {
    fn full() -> Samples {
        Samples {
            strip: sample_strip(),
            keyboard: crate::foxglove::keyboard_page::tests::sample(),
            lights: crate::foxglove::lights_page::tests::sample(),
            pc: crate::foxglove::pc_lights_ui::tests::sample(),
            wizard: Default::default(),
            calibrate: None,
            screens: Some(sample_screens()),
            art: Some(crate::foxglove::art_ui::tests::sample()),
            plugins: crate::foxglove::plugins_ui::tests::sample(),
        }
    }

    fn none() -> Samples {
        Samples {
            strip: Default::default(),
            keyboard: crate::foxglove::keyboard_page::tests::sample_none(),
            lights: crate::foxglove::lights_page::tests::sample_none(),
            pc: Default::default(),
            wizard: Default::default(),
            calibrate: None,
            screens: None,
            art: None,
            plugins: crate::foxglove::plugins_ui::tests::sample(),
        }
    }
}

/// The strip the previews show: both halves seen, the right one not answering yet.
fn sample_strip() -> crate::foxglove::shell::StripState {
    crate::foxglove::shell::StripState {
        keyboard: true,
        usb: Some(true),
        left_build: Some(Ok("0.1.0 · e255d32".into())),
        right_build: Some(Err("no reply from the keyboard".into())),
        right_battery: Some(61),
        left_battery: Some(95),
        profile: Some(0),
        this_pc: Some(0),
        unplug_warning: None,
        host_link: Some("USB"),
    }
}

fn shell_preview_with(
    page: crate::foxglove::shell::Page,
    samples: &Samples,
    height: f32,
) -> image::RgbaImage {
    let ctx = preview_ctx();
    show_samples(page, samples);
    let size = egui::vec2(1072.0, height + 40.0);
    let img = render(&ctx, size, Color32::from_rgb(0x09, 0x0a, 0x0c), |ctx| {
        shell_window(ctx, egui::vec2(1040.0, height));
    });
    clear_samples();
    img
}

/// A headless context with the app's fonts and the dark theme.
fn preview_ctx() -> egui::Context {
    crate::foxglove::store::test_config_dir();
    let ctx = egui::Context::default();
    // The plugin icons are SVGs, as in the app.
    egui_extras::install_image_loaders(&ctx);
    let mut fonts = egui::FontDefinitions::default();
    crate::platform::add_phosphor_to_fonts(&mut fonts);
    crate::foxglove::theme::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    crate::foxglove::theme::apply_choice(&ctx, crate::foxglove::theme::Choice::Dark);
    ctx
}

/// The pages' live parts show `samples` on this thread, on `page`.
fn show_samples(page: crate::foxglove::shell::Page, samples: &Samples) {
    use crate::foxglove::shell;
    shell::preview_page(page);
    shell::PREVIEW_STRIP.set(Some(samples.strip.clone()));
    crate::foxglove::keyboard_page::PREVIEW.set(Some(samples.keyboard.clone()));
    crate::foxglove::lights_page::PREVIEW.set(Some(samples.lights.clone()));
    crate::foxglove::pc_lights_ui::PREVIEW.set(Some((samples.pc.clone(), samples.wizard.clone())));
    crate::foxglove::calibrate_ui::PREVIEW.set(samples.calibrate.clone());
    crate::foxglove::screens_ui::PREVIEW.set(Some(samples.screens.clone().unwrap_or_default()));
    crate::foxglove::art_ui::PREVIEW.set(Some(samples.art.clone().unwrap_or_default()));
    crate::foxglove::plugins_ui::PREVIEW.set(Some(samples.plugins.clone()));
}

fn clear_samples() {
    crate::foxglove::shell::PREVIEW_STRIP.set(None);
    crate::foxglove::keyboard_page::PREVIEW.set(None);
    crate::foxglove::lights_page::PREVIEW.set(None);
    crate::foxglove::pc_lights_ui::PREVIEW.set(None);
    crate::foxglove::calibrate_ui::PREVIEW.set(None);
    crate::foxglove::screens_ui::PREVIEW.set(None);
    crate::foxglove::art_ui::PREVIEW.set(None);
    crate::foxglove::plugins_ui::PREVIEW.set(None);
}

/// The settings window's width after a few frames of `samples`, at the window's minimum size.
fn window_width(ctx: &egui::Context, page: crate::foxglove::shell::Page, samples: &Samples) -> f32 {
    show_samples(page, samples);
    let mut rect = None;
    for frame in 0..6 {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 900.0))),
            time: Some(frame as f64),
            predicted_dt: 1.0,
            focused: true,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            rect = shell_window(ui.ctx(), egui::vec2(720.0, 600.0));
        });
        out.textures_delta.clear(); // nothing is painted here
    }
    clear_samples();
    rect.expect("the window was drawn").width()
}

/// What a keyboard on USB and Bluetooth at once shows, worded as long as it gets: every chip
/// of the strip, the unplug warning, and long rows on the Keyboard page.
fn long_samples() -> Samples {
    let mut s = Samples::full();
    s.strip = crate::foxglove::shell::StripState {
        keyboard: true,
        usb: Some(false),
        left_build: Some(Ok("0.1.0 (dev) · e255d32".into())),
        right_build: Some(Err("no reply from the keyboard".into())),
        right_battery: Some(100),
        left_battery: Some(100),
        profile: Some(3),
        this_pc: Some(1),
        unplug_warning: Some("Unplugging now leaves the keyboard unable to type".into()),
        host_link: Some("USB"),
    };
    for h in &mut s.keyboard.halves {
        h.text = format!(
            "{} · e255d32 · built for the Eyelash Corne, dev profile",
            h.text
        );
    }
    s.keyboard.link.rows = vec![
        (
            "Left half",
            "on USB and on Bluetooth profile 4 (another PC) at the same time".into(),
        ),
        (
            "Bluetooth to this PC",
            "connected, but this PC is paired on profile 2 of 5".into(),
        ),
    ];
    s.keyboard.link.unplug_fix = Some(
        "Unplugging now leaves the keyboard unable to type - select profile 2: hold SYSTEM \
         (both middle thumbs) and Tab, then press the profile's key on the number row"
            .into(),
    );
    s
}

/// A keyboard connecting (or replugged) must not widen the settings window: long chips and rows
/// wrap or are cut inside it. Before the fix the window grew by the overflow on reconnect.
#[test]
fn a_reconnect_does_not_widen_the_window() {
    use crate::foxglove::shell::Page;
    for page in [Page::Keyboard, Page::Settings, Page::Screens] {
        let ctx = preview_ctx();
        let before = window_width(&ctx, page, &Samples::none());
        let after = window_width(&ctx, page, &long_samples());
        assert!(
            after <= before + 0.5,
            "{page:?}: the window grew from {before} to {after} points when the keyboard connected"
        );
    }
}

fn keep(name: &str, img: &image::RgbaImage) {
    if let Some(dir) = std::env::var_os("FOXGLOVE_UI_PREVIEW_DIR") {
        let path = std::path::Path::new(&dir).join(name);
        img.save(&path).expect("save the preview");
        eprintln!("Foxglove: preview saved to {}", path.display());
    }
}

#[test]
fn shell_pages_render() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let mut seen: Vec<image::RgbaImage> = Vec::new();
    for page in Page::ALL {
        let img = shell_preview(page);
        keep(&format!("ui-385-{}.png", page.label().to_lowercase()), &img);
        let total = (img.width() * img.height()) as usize;
        // The sidebar's well, the page's ground and the orange nav dot all show up.
        assert!(count(&img, DARK.well) > total / 20, "{page:?}: sidebar");
        assert!(count(&img, DARK.ground) > total / 20, "{page:?}: page");
        assert!(count(&img, DARK.accent) > 20, "{page:?}: orange");
        assert!(
            seen.iter().all(|other| *other != img),
            "{page:?} looks like another page"
        );
        seen.push(img);
    }
}

/// The Keyboard page (#386) with a keyboard and without one.
#[test]
fn keyboard_page_renders() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let with = shell_preview_with(Page::Keyboard, &Samples::full(), 1060.0);
    let without = shell_preview_with(Page::Keyboard, &Samples::none(), 1060.0);
    keep("ui-386-keyboard.png", &with);
    keep("ui-386-keyboard-none.png", &without);
    // The same page with the board drawn from its profile (#392).
    keep("ui-392-board.png", &with);
    keep("ui-392-board-none.png", &without);
    for img in [&with, &without] {
        // The cards' surface and the board's orange (screen rings, underglow) show up.
        assert!(
            count(img, DARK.surface) > (img.width() * img.height()) as usize / 20,
            "cards"
        );
        assert!(count(img, DARK.accent) > 500, "the board's orange");
    }
    // The right half's "no reply yet" is amber on the page as well as in the strip.
    assert!(count(&with, DARK.warn) > count(&without, DARK.warn));
}

/// The left panel as the preview shows it: a keyboard that answered HELLO, a frame of the
/// panel drawn from a pattern, and a game in front.
pub(crate) fn sample_screens() -> crate::foxglove::screens::Snapshot {
    let (width, height) = (64, 52);
    let stride = width / 8;
    let mut bits = vec![0u8; stride * height];
    let mut set = |x: usize, y: usize| bits[y * stride + x / 8] |= 0x80 >> (x % 8);
    // A frame, a clock-like bar and a progress line, so the preview isn't blank.
    for x in 0..width {
        set(x, 0);
        set(x, height - 1);
        if x < 40 {
            set(x, 40);
        }
    }
    for y in 0..height {
        set(0, y);
        set(width - 1, y);
    }
    for y in 10..26 {
        for x in (8..56).filter(|x| (x / 6) % 2 == 0) {
            set(x, y);
        }
    }
    crate::foxglove::screens::Snapshot {
        product: "Eyelash Corne".into(),
        hello: Some(digitalis::protocol::Hello {
            version: 3,
            right_link: 2,
            right_battery: Some(61),
            ..Default::default()
        }),
        frame: Some(crate::foxglove::screens::Frame {
            width,
            height,
            stride,
            bits,
        }),
        foreground: Some("eldenring.exe".into()),
        screens: Vec::new(),
    }
}

/// The Screens page (#388) with a keyboard and without one.
#[test]
fn screens_page_renders() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let with = shell_preview_with(Page::Screens, &Samples::full(), 1000.0);
    let without = shell_preview_with(Page::Screens, &Samples::none(), 660.0);
    keep("ui-388-screens.png", &with);
    keep("ui-388-screens-none.png", &without);
    // The art tiles are drawn in the right screen's two tones.
    let lit = Color32::from_rgb(0xe8, 0xea, 0xe9);
    assert!(count(&with, lit) > 300, "art thumbnails");
    assert!(
        count(&with, DARK.surface) > count(&without, DARK.surface),
        "two cards"
    );
}

/// The Plugins page (#389): the installed and catalog cards, three to a row.
#[test]
fn plugins_page_renders() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let img = shell_preview_with(Page::Plugins, &Samples::full(), 900.0);
    keep("ui-389-plugins.png", &img);
    // The cards' surface and their outlines fill the page.
    assert!(
        count(&img, DARK.surface) > (img.width() * img.height()) as usize / 12,
        "cards"
    );
    assert!(count(&img, DARK.line) > 2000, "card outlines");
}

/// The Settings page (#390): the layer overlay's card on the left, the other cards on the right.
#[test]
fn settings_page_renders() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let img = shell_preview_with(Page::Settings, &Samples::full(), 1000.0);
    keep("ui-390-settings.png", &img);
    assert!(
        count(&img, DARK.surface) > (img.width() * img.height()) as usize / 10,
        "cards"
    );
    // The theme picker's selected pill sits in "Theme and updates".
    assert!(count(&img, DARK.selected_fill) > 100, "the Dark pill");
}

/// The Settings window as a native window of its own shows it: the shell edge to edge in the
/// client area. Windows draws the real title bar (dark with the dark theme); a stand-in sits
/// above the client area here, so the review sees the whole window.
#[test]
fn settings_window_renders_as_a_native_window() {
    use crate::foxglove::settings_window::{DEFAULT_SIZE, TITLE};
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let ctx = preview_ctx();
    show_samples(Page::Settings, &Samples::full());
    const TITLE_BAR: f32 = 32.0;
    let bar = Color32::from_rgb(0x20, 0x20, 0x20);
    let size = DEFAULT_SIZE + egui::vec2(0.0, TITLE_BAR);
    let img = render_ui(&ctx, size, bar, |ui| {
        egui::Panel::top("native-title-bar")
            .exact_size(TITLE_BAR)
            .frame(
                egui::Frame::new()
                    .fill(bar)
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new(TITLE).color(Color32::from_gray(0xd0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 28.0;
                        for glyph in [
                            egui_phosphor::regular::X,
                            egui_phosphor::regular::SQUARE,
                            egui_phosphor::regular::MINUS,
                        ] {
                            ui.label(egui::RichText::new(glyph).color(Color32::from_gray(0xd0)));
                        }
                    });
                });
            });
        crate::foxglove::shell::frame(ui, sections);
    });
    clear_samples();
    keep("ui-settings-native-window.png", &img);
    let count = |c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let total = (img.width() * img.height()) as usize;
    // The shell reaches the window's edges: sidebar well and page ground, no egui window frame.
    assert!(count(DARK.well) > total / 20, "sidebar");
    assert!(count(DARK.ground) > total / 20, "page");
    let left_edge = img.get_pixel(0, img.height() / 2).0;
    assert_eq!(
        left_edge[..3],
        [DARK.well.r(), DARK.well.g(), DARK.well.b()],
        "the sidebar starts at the window's left edge"
    );
}

/// Where the Settings window opens, drawn from the real placement logic on this PC's two
/// displays: where it was closed while both are there, and centred on the main display once the
/// second one is gone.
#[test]
fn settings_window_placement_renders() {
    use crate::foxglove::settings_window::{restore, tests::two_displays, Saved};
    use crate::foxglove::theme::DARK;
    let ctx = preview_ctx();
    let displays = two_displays();
    let saved = Saved {
        monitor: displays[1].id.clone(),
        // In points of the 150 % display: 1560 x 990 pixels there, 1040 x 660 on the main one.
        place: Some([293.0, 134.0, 1040.0, 660.0]),
        rect: None,
    };
    let scale = 0.12;
    let panel_h = 2088.0 * scale + 56.0;
    let size = egui::vec2(6400.0 * scale + 48.0, panel_h * 2.0 + 16.0);
    let cases: [(&str, &[crate::monitors::Monitor]); 2] = [
        ("Both displays: it opens where it was closed", &displays[..]),
        (
            "Display 2 unplugged: centred on the main display",
            &displays[..1],
        ),
    ];
    let img = render_ui(&ctx, size, DARK.ground, |ui| {
        let painter = ui.painter().clone();
        for (i, (caption, monitors)) in cases.iter().enumerate() {
            let top = 12.0 + i as f32 * panel_h;
            painter.text(
                egui::pos2(24.0, top),
                egui::Align2::LEFT_TOP,
                *caption,
                crate::foxglove::theme::body(15.0),
                DARK.text_strong,
            );
            let origin = egui::pos2(24.0, top + 32.0);
            let to_rect = |r: [i32; 4]| {
                Rect::from_min_max(
                    origin + egui::vec2(r[0] as f32, r[1] as f32) * scale,
                    origin + egui::vec2(r[2] as f32, r[3] as f32) * scale,
                )
            };
            for (n, m) in displays.iter().enumerate() {
                let here = monitors.iter().any(|x| x.id == m.id);
                let rect = to_rect(m.work_area);
                painter.rect(
                    rect,
                    4.0,
                    if here { DARK.surface } else { DARK.ground },
                    egui::Stroke::new(1.0, if here { DARK.muted } else { DARK.well }),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    rect.left_bottom() + egui::vec2(8.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                    if here {
                        format!("Display {}", n + 1)
                    } else {
                        format!("Display {} (gone)", n + 1)
                    },
                    crate::foxglove::theme::mono(12.0),
                    DARK.muted,
                );
            }
            if let Some(r) = restore(&saved, monitors, [1040.0, 660.0], "") {
                let rect = to_rect(r);
                painter.rect(
                    rect,
                    3.0,
                    DARK.accent.gamma_multiply(0.35),
                    egui::Stroke::new(2.0, DARK.accent),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Settings",
                    crate::foxglove::theme::body(13.0),
                    DARK.text_strong,
                );
            }
        }
    });
    keep("ui-settings-window-placement.png", &img);
    assert!(
        img.pixels()
            .any(|p| p.0[..3] == [DARK.accent.r(), DARK.accent.g(), DARK.accent.b()]),
        "the window's outline"
    );
}

/// The tray panel (#391), drawn off-screen the way it opens next to the tray icon.
#[test]
fn tray_panel_renders() {
    use crate::foxglove::theme::DARK;
    crate::foxglove::store::test_config_dir();
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    let mut fonts = egui::FontDefinitions::default();
    crate::platform::add_phosphor_to_fonts(&mut fonts);
    crate::foxglove::theme::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    crate::foxglove::theme::apply_choice(&ctx, crate::foxglove::theme::Choice::Dark);
    let v = crate::foxglove::tray_panel::tests::sample();
    let size = egui::vec2(crate::foxglove::tray_panel::WIDTH + 40.0, 720.0);
    let img = render(&ctx, size, Color32::from_rgb(0x09, 0x0a, 0x0c), |ctx| {
        egui::Area::new(egui::Id::new("tray-panel-preview"))
            .fixed_pos(egui::pos2(20.0, 20.0))
            .show(ctx, |ui| {
                crate::foxglove::tray_panel::panel(
                    ui,
                    &v,
                    &mut Default::default(),
                    &mut Vec::new(),
                );
            });
    });
    keep("ui-391-tray.png", &img);
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let total = (img.width() * img.height()) as usize;
    assert!(count(&img, DARK.ground) > total / 4, "the panel's ground");
    assert!(count(&img, DARK.selected_fill) > 200, "the selected pills");

    // Paused (#404): "Resume Foxglove" is a selected pill, with the note under the header.
    let paused = crate::foxglove::tray_panel::while_paused(crate::foxglove::tray_panel::View {
        paused: true,
        ..v.clone()
    });
    // A fresh context: `render` keeps the textures it was sent, starting with the fonts.
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    let mut fonts = egui::FontDefinitions::default();
    crate::platform::add_phosphor_to_fonts(&mut fonts);
    crate::foxglove::theme::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    crate::foxglove::theme::apply_choice(&ctx, crate::foxglove::theme::Choice::Dark);
    let img_paused = render(&ctx, size, Color32::from_rgb(0x09, 0x0a, 0x0c), |ctx| {
        egui::Area::new(egui::Id::new("tray-panel-preview"))
            .fixed_pos(egui::pos2(20.0, 20.0))
            .show(ctx, |ui| {
                crate::foxglove::tray_panel::panel(
                    ui,
                    &paused,
                    &mut Default::default(),
                    &mut Vec::new(),
                );
            });
    });
    keep("tray-panel-paused.png", &img_paused);
    assert!(count(&img_paused, DARK.warn) > 50, "the paused note");
    assert!(
        count(&img_paused, DARK.selected_fill) > count(&img, DARK.selected_fill),
        "the resume pill"
    );
}

/// The Lights page (#387) with a keyboard and without one.
#[test]
fn lights_page_renders() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let with = shell_preview_with(Page::Lights, &Samples::full(), 760.0);
    let without = shell_preview_with(Page::Lights, &Samples::none(), 760.0);
    keep("ui-387-lights.png", &with);
    keep("ui-387-lights-none.png", &without);
    // The Bandit orange swatch, the selected pill and Apply colour; dimmer with no keyboard.
    assert!(count(&with, DARK.accent) > 400, "orange");
    assert!(count(&with, DARK.selected_fill) > 200, "a selected pill");
    assert!(count(&with, DARK.accent) > count(&without, DARK.accent));
    assert_ne!(with, without);
}

/// The Lights page's "PC & peripherals" card (#423): the 45L with colours, the cooler and the
/// G502 with Light Studio running; then a fresh 45L matching the keyboard; then none found.
#[test]
fn pc_lights_render() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let mut fresh = Samples::full();
    fresh.pc = crate::foxglove::pc_lights_ui::tests::sample_fresh();
    let with = shell_preview_with(Page::Lights, &Samples::full(), 1180.0);
    let fresh = shell_preview_with(Page::Lights, &fresh, 1180.0);
    let none = shell_preview_with(Page::Lights, &Samples::none(), 1180.0);
    keep("ui-423-pc-lights.png", &with);
    keep("ui-423-pc-lights-fresh.png", &fresh);
    keep("ui-423-pc-lights-none.png", &none);
    // Light Studio's note is in the warning colour; the fresh card has none.
    assert!(count(&with, DARK.warn) > 100, "the Light Studio note");
    assert!(count(&fresh, DARK.warn) < count(&with, DARK.warn));
    assert_ne!(with, fresh);
    assert_ne!(fresh, none);
}

/// The colour calibrator on the Lights page: the device list with "Calibrate…", then each of
/// the five steps on the 45L (against the screen, and the blue step against the keyboard).
#[test]
fn calibrate_renders() {
    use crate::foxglove::calibrate_ui::{tests::sample, Step};
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: [u8; 3]| img.pixels().filter(|p| p.0[..3] == c).count();
    let list = shell_preview_with(Page::Lights, &Samples::full(), 1180.0);
    keep("ui-calibrate-devices.png", &list);
    let mut shots = Vec::new();
    for step in Step::ALL {
        let mut s = Samples::full();
        s.calibrate = Some(sample(step, false));
        let img = shell_preview_with(Page::Lights, &s, 1180.0);
        keep(
            &format!("ui-calibrate-{}.png", step.name().to_lowercase()),
            &img,
        );
        // The screen's swatch is the step's own colour, large.
        assert!(count(&img, step.color()) > 5000, "{step:?} swatch");
        shots.push(img);
    }
    let mut master = Samples::full();
    master.calibrate = Some(sample(Step::Blue, true));
    let master = shell_preview_with(Page::Lights, &master, 1180.0);
    keep("ui-calibrate-master.png", &master);
    // Next/Save is the orange primary button; every step looks different.
    assert!(
        count(
            &shots[0],
            [DARK.accent.r(), DARK.accent.g(), DARK.accent.b()]
        ) > 200
    );
    for pair in shots.windows(2) {
        assert_ne!(pair[0], pair[1]);
    }
    assert_ne!(master, shots[3]);
}

/// What each device can do, as chips (the keyboard from HELLO, the case and cooler from the
/// catalog, the mouse from its LampArray attributes), and "Add a device…": picking an unknown
/// device, then naming its zones.
#[test]
fn pc_lights_caps_and_add_render() {
    use crate::foxglove::pc_lights_add::{tests::sample_zones, Step, Wizard};
    use crate::foxglove::shell::Page;
    let mut pick = Samples::full();
    pick.wizard = Wizard {
        step: Step::Pick,
        ..Wizard::default()
    };
    let mut zones = Samples::full();
    zones.wizard = sample_zones();
    let caps = shell_preview_with(Page::Lights, &Samples::full(), 1400.0);
    let pick = shell_preview_with(Page::Lights, &pick, 1400.0);
    let zones = shell_preview_with(Page::Lights, &zones, 1400.0);
    keep("ui-423-caps.png", &caps);
    keep("ui-423-add-pick.png", &pick);
    keep("ui-423-add-zones.png", &zones);
    assert_ne!(caps, pick);
    assert_ne!(pick, zones);
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

    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let total = (dark.width() * dark.height()) as usize;
    // The window's ground, the cards and the orange all show up; before, none of them did.
    assert!(count(&dark, DARK.ground) > total / 50, "dark ground");
    assert!(count(&dark, DARK.surface) > total / 50, "dark cards");
    assert!(count(&dark, DARK.accent) > 50, "orange");
    assert!(count(&light, LIGHT.ground) > total / 50, "light ground");
    assert_eq!(count(&before, DARK.ground), 0);
}

/// The status strip with "Copy diagnostics" live (#371), and the report it copies.
#[test]
fn copy_diagnostics_renders() {
    use crate::foxglove::shell::Page;
    let img = shell_preview_with(Page::Keyboard, &Samples::full(), 660.0);
    keep("ui-371-copy-diagnostics.png", &img);
    if let Some(dir) = std::env::var_os("FOXGLOVE_UI_PREVIEW_DIR") {
        let p = crate::foxglove::diagnostics::sample_personal();
        let text =
            crate::foxglove::diagnostics::report(&crate::foxglove::diagnostics::sample(), &p);
        std::fs::write(
            std::path::Path::new(&dir).join("ui-371-diagnostics-sample.txt"),
            text,
        )
        .expect("save the sample");
    }
}

/// The Keyboard page's Firmware card with the low-battery switch (#372).
#[test]
fn battery_notice_switch_renders() {
    use crate::foxglove::shell::Page;
    let img = shell_preview_with(Page::Keyboard, &Samples::full(), 1060.0);
    keep("ui-372-battery-notice.png", &img);
}

/// The sidebar's Bandit fox mark, whole and enlarged (it matches the tray glyph).
#[test]
fn sidebar_fox_renders() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let img = shell_preview_with(Page::Keyboard, &Samples::full(), 660.0);
    keep("ui-sidebar-fox.png", &img);
    let top = image::imageops::crop_imm(&img, 0, 0, 480, 270).to_image();
    let zoom =
        image::imageops::resize(&top, 480 * 3, 270 * 3, image::imageops::FilterType::Nearest);
    keep("ui-sidebar-fox-zoom.png", &zoom);
    // The fox's orange fills more than the old arrowhead's outline did.
    let orange = top
        .pixels()
        .filter(|p| p.0[..3] == [DARK.accent.r(), DARK.accent.g(), DARK.accent.b()])
        .count();
    assert!(orange > 250, "{orange}");
}
