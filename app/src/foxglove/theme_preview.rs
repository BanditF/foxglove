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

mod art_flow;
mod device_rows;
mod device_thumbs;
mod keymap_session;
mod knob_mockup;
mod lamp_keyboard;
mod layout;
mod overlay;

thread_local! { static LEGACY: Cell<bool> = const { Cell::new(false) }; }
thread_local! {
    /// Where the pointer rests in the frames [`run_frames`] runs (a hover's tooltip), if set.
    static POINTER: Cell<Option<Pos2>> = const { Cell::new(None) };
}

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
            // Moved in once, then still, so a tooltip comes up.
            events: POINTER
                .get()
                .filter(|_| frame == 1)
                .map(|p| vec![egui::Event::PointerMoved(p)])
                .unwrap_or_default(),
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
                // The headings use the theme's fonts, which the "before #384" preview hasn't.
                let grid_heading = |ui: &mut egui::Ui, text: &str| {
                    if !legacy() {
                        crate::foxglove::settings_ui::grid_heading(ui, text);
                    }
                };
                // On or off, then #801: which keyboard it shows, the focus or a pin.
                if !legacy() {
                    crate::foxglove::overlay_shown::row(ui);
                    crate::foxglove::overlay_shown::mute_if_off(ui);
                    crate::foxglove::focus_pins::overlay_row(ui);
                }
                grid_heading(ui, "When it shows");
                for (label, value) in [("Display duration", 2000.0), ("Activation delay", 0.0)] {
                    ui.label(label);
                    let mut v: f64 = value;
                    ui.add_sized(
                        ui.available_size(),
                        egui::DragValue::new(&mut v).suffix(" ms"),
                    );
                    ui.end_row();
                }
                grid_heading(ui, "Appearance");
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
        if !legacy() {
            crate::foxglove::overlay_shown::mute_if_off(ui);
        }
        // The window theme is in "Appearance" since #390.
        let mut white = Color32::WHITE;
        ui.horizontal(|ui| {
            ui.label("Text colour");
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
                ui.label(theme::heading(ui, "Keyboard"));
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
                    let r = ui.button("Refresh");
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
            let _ = ui.button("Refresh");
        });
    });
    // The real Plugins page (#389), drawn from its preview sample.
    crate::foxglove::plugins_ui::group(ui);
}

/// The settings window as #385 draws it: the sidebar, the status strip and one page.
fn shell_window(ctx: &egui::Context, size: egui::Vec2) -> Option<Rect> {
    egui::Window::new("Foxglove")
        .fixed_pos(egui::pos2(16.0, 16.0))
        .default_size(size)
        .min_size(egui::vec2(720.0, 420.0))
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::ZERO))
        .collapsible(false)
        .resizable(true)
        .open(&mut true)
        .show(ctx, |ui| crate::foxglove::shell::frame(ui, sections))
        .map(|r| r.response.rect)
}

/// The settings window's sections, in the real window's order (ui_settings.rs).
fn sections(ui: &mut egui::Ui) {
    keypeek_groups(ui);
    crate::foxglove::profiles_ui::picker(ui);
    crate::foxglove::update::group(ui);
    crate::foxglove::follow_typing::group(ui);
    crate::foxglove::live_feed::group(ui);
    crate::foxglove::troubleshooting::group(ui);
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
    /// "Save battery" under the underglow controls.
    battery_dim: (
        crate::foxglove::battery_dim_ui::DimState,
        crate::foxglove::battery_dim_ui::Draft,
    ),
    pc: crate::foxglove::pc_lights_ui::View,
    wizard: crate::foxglove::pc_lights_add::Wizard,
    /// The colour calibrator, open as a dialog over the Lights page.
    calibrate: Option<(
        crate::foxglove::calibrate_ui::View,
        crate::foxglove::calibrate_ui::Wizard,
    )>,
    /// "Find my LEDs", open in place of the Lights page's cards.
    leds: Option<(
        crate::foxglove::led_map_ui::View,
        crate::foxglove::led_map::Wizard,
    )>,
    /// "Check LED map" (the dot test), open in place of the Lights page's cards.
    ledcheck: Option<(
        crate::foxglove::led_map_ui::CheckView,
        crate::foxglove::led_map_confirm::Confirm,
    )>,
    screens: Option<crate::foxglove::screens::Snapshot>,
    art: Option<(
        crate::foxglove::art_service::Snapshot,
        crate::foxglove::art_library::Library,
    )>,
    plugins: crate::foxglove::plugins_ui::View,
    /// The Scenes page (#487) and what's being dragged or typed on it.
    scenes: (
        crate::foxglove::scenes_ui::View,
        crate::foxglove::scenes_ui::Draft,
    ),
}

impl Samples {
    fn full() -> Samples {
        Samples {
            strip: sample_strip(),
            keyboard: crate::foxglove::keyboard_page::tests::sample(),
            lights: crate::foxglove::lights_page::tests::sample(),
            battery_dim: (
                crate::foxglove::battery_dim_ui::tests::sample(),
                Default::default(),
            ),
            pc: crate::foxglove::pc_lights_ui::tests::sample(),
            wizard: Default::default(),
            calibrate: None,
            leds: None,
            ledcheck: None,
            screens: Some(sample_screens()),
            art: Some(crate::foxglove::art_ui::tests::sample()),
            plugins: crate::foxglove::plugins_ui::tests::sample(),
            scenes: (
                crate::foxglove::scenes_ui::tests::sample(),
                Default::default(),
            ),
        }
    }

    fn none() -> Samples {
        Samples {
            strip: Default::default(),
            keyboard: crate::foxglove::keyboard_page::tests::sample_none(),
            lights: crate::foxglove::lights_page::tests::sample_none(),
            battery_dim: (
                crate::foxglove::battery_dim_ui::DimState::NoKeyboard,
                Default::default(),
            ),
            pc: Default::default(),
            wizard: Default::default(),
            calibrate: None,
            leds: None,
            ledcheck: None,
            screens: None,
            art: None,
            plugins: crate::foxglove::plugins_ui::tests::sample(),
            scenes: (
                crate::foxglove::scenes_ui::tests::sample_none(),
                Default::default(),
            ),
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
        parts: crate::foxglove::fixtures::users_desk::board().parts(),
        switcher: Default::default(),
    }
}

fn shell_preview_with(
    page: crate::foxglove::shell::Page,
    samples: &Samples,
    height: f32,
) -> image::RgbaImage {
    shell_preview_in(page, samples, height, crate::foxglove::theme::Choice::Dark)
}

/// [`shell_preview_with`] in the theme `choice`.
fn shell_preview_in(
    page: crate::foxglove::shell::Page,
    samples: &Samples,
    height: f32,
    choice: crate::foxglove::theme::Choice,
) -> image::RgbaImage {
    let ctx = preview_ctx();
    crate::foxglove::theme::apply_choice(&ctx, choice);
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
    crate::foxglove::battery_dim_ui::PREVIEW.set(Some(samples.battery_dim));
    crate::foxglove::pc_lights_ui::PREVIEW.set(Some((samples.pc.clone(), samples.wizard.clone())));
    crate::foxglove::calibrate_ui::PREVIEW.set(samples.calibrate.clone());
    crate::foxglove::led_map_ui::PREVIEW.set(samples.leds.clone());
    crate::foxglove::led_map_ui::PREVIEW_CONFIRM.set(samples.ledcheck.clone());
    crate::foxglove::screens_ui::PREVIEW.set(Some(samples.screens.clone().unwrap_or_default()));
    crate::foxglove::art_ui::PREVIEW.set(Some(samples.art.clone().unwrap_or_default()));
    crate::foxglove::plugins_ui::PREVIEW.set(Some(samples.plugins.clone()));
    crate::foxglove::scenes_ui::PREVIEW.set(Some(samples.scenes.clone()));
}

fn clear_samples() {
    crate::foxglove::shell::PREVIEW_STRIP.set(None);
    crate::foxglove::keyboard_page::PREVIEW.set(None);
    crate::foxglove::lights_page::PREVIEW.set(None);
    crate::foxglove::battery_dim_ui::PREVIEW.set(None);
    crate::foxglove::pc_lights_ui::PREVIEW.set(None);
    crate::foxglove::calibrate_ui::PREVIEW.set(None);
    crate::foxglove::led_map_ui::PREVIEW.set(None);
    crate::foxglove::led_map_ui::PREVIEW_CONFIRM.set(None);
    crate::foxglove::screens_ui::PREVIEW.set(None);
    crate::foxglove::art_ui::PREVIEW.set(None);
    crate::foxglove::plugins_ui::PREVIEW.set(None);
    crate::foxglove::scenes_ui::PREVIEW.set(None);
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
        parts: crate::foxglove::fixtures::users_desk::board().parts(),
        switcher: Default::default(),
    };
    for h in &mut s.keyboard.halves {
        h.text = format!(
            "{} · e255d32 · built for the Eyelash Corne, dev profile",
            h.text
        );
    }
    s.keyboard.link.rows = vec![
        (
            "Left half".into(),
            "on USB and on Bluetooth slot 4 (another PC) at the same time".into(),
        ),
        (
            "Bluetooth to this PC".into(),
            "connected, but this PC is paired on slot 2 of 5".into(),
        ),
    ];
    s.keyboard.link.unplug_fix = Some(
        "Unplugging now leaves the keyboard unable to type - select Bluetooth slot 2: hold SYSTEM \
         (both middle thumbs) and Tab, then press the slot's key on the number row"
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

/// The Keyboard page in full (#534): the layer overlay's card (when it shows, appearance, layer
/// colours, keymap source) and Other keyboards below the keyboard's cards; and App without them.
#[test]
fn layer_overlay_card_renders() {
    use crate::foxglove::shell::Page;
    let keyboard = shell_preview_with(Page::Keyboard, &Samples::full(), 2300.0);
    let app = shell_preview_with(Page::Settings, &Samples::full(), 1100.0);
    keep("ui-ia-keyboard-full.png", &keyboard);
    keep("ui-ia-app.png", &app);
    assert_ne!(keyboard, app);
}

/// "Show layer overlay" on and off, dark and light: the Layer overlay card alone (its
/// appearance grid and layer colours, the keymap source hidden). Off mutes the rest of the card.
#[test]
fn layer_overlay_on_off_renders() {
    use crate::foxglove::theme::{Choice, DARK, LIGHT};
    let mut seen = Vec::new();
    for (choice, name, ground) in [
        (Choice::Dark, "dark", DARK.ground),
        (Choice::Light, "light", LIGHT.ground),
    ] {
        for on in [true, false] {
            crate::foxglove::overlay_shown::PREVIEW.set(Some(on));
            // One keyboard, connected, one layout: the card ends with its layer colours.
            crate::foxglove::settings_ui::keymap_choice(1, true, 1);
            let ctx = preview_ctx();
            crate::foxglove::theme::apply_choice(&ctx, choice);
            let img = render(&ctx, egui::vec2(720.0, 720.0), ground, |ctx| {
                egui::Area::new(egui::Id::new("overlay-card"))
                    .fixed_pos(egui::pos2(16.0, 10.0))
                    .show(ctx, |ui| {
                        ui.set_width(688.0);
                        keypeek_groups(ui);
                    });
            });
            crate::foxglove::settings_ui::keymap_choice(0, false, 0);
            keep(
                &format!(
                    "overlay-toggle-{}-{name}.png",
                    if on { "on" } else { "off" }
                ),
                &img,
            );
            seen.push(img);
        }
    }
    crate::foxglove::overlay_shown::PREVIEW.set(None);
    // Off looks different from on, in each theme.
    assert_ne!(seen[0], seen[1]);
    assert_ne!(seen[2], seen[3]);
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

/// #800: the keyboard switcher at the head of the status strip, and the pages following the
/// focus, dark and light: one keyboard (a flat chip, no caret), two keyboards (the clickable chip),
/// the list open, the Corne focused (the cards named for it) and the K5 Pro focused (its own
/// layout, muted cards with read-only chips).
#[test]
fn focus_switcher_renders() {
    use crate::foxglove::focus_ui::{tests as ft, Focused, Switcher, PREVIEW_OPEN};
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::Choice;
    let corne = Focused {
        name: "Eyelash Corne".into(),
        read_only: None,
    };
    let two = ft::sample(ft::SAMPLE_CORNE);
    let one = Switcher {
        rows: vec![two.rows[0].clone()],
    };
    let on_corne = |switcher: Switcher| {
        let mut s = Samples::full();
        s.strip.switcher = switcher;
        s.keyboard.focus = Some(corne.clone());
        s.keyboard.board.title = corne.name.clone();
        s.lights.focus = Some(corne.clone());
        s
    };
    let k5_switcher = ft::sample(ft::SAMPLE_K5);
    let k5 = Focused {
        name: "K5 Pro".into(),
        read_only: k5_switcher.focused().and_then(|r| r.read_only.clone()),
    };
    // No VIA definition is drawn yet (B2): the K5 Pro's layout is B1's guess.
    let layout = crate::foxglove::key_layout::resolve(None, &[], 0);
    let on_k5 = {
        let mut s = Samples::full();
        s.strip.switcher = k5_switcher.clone();
        let board = crate::foxglove::keyboard_page::read_only_board(&k5, layout.clone());
        let link = k5_switcher.focused().unwrap().detail.clone();
        s.keyboard = crate::foxglove::keyboard_page::View {
            devices: s.keyboard.devices.clone(),
            device_notice: true,
            ..crate::foxglove::keyboard_page::read_only(k5.clone(), board, link)
        };
        s.lights = crate::foxglove::lights_page::read_only_view(
            k5.clone(),
            crate::foxglove::light_zones::line(None, &layout),
        );
        s
    };
    for (choice, theme) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let strip_one = shell_preview_in(Page::Keyboard, &on_corne(one.clone()), 420.0, choice);
        let strip_two = shell_preview_in(Page::Keyboard, &on_corne(two.clone()), 420.0, choice);
        PREVIEW_OPEN.set(true);
        let open = shell_preview_in(Page::Keyboard, &on_corne(two.clone()), 420.0, choice);
        PREVIEW_OPEN.set(false);
        let corne_lights = shell_preview_in(Page::Lights, &on_corne(two.clone()), 760.0, choice);
        let k5_keyboard = shell_preview_in(Page::Keyboard, &on_k5, 900.0, choice);
        let k5_lights = shell_preview_in(Page::Lights, &on_k5, 760.0, choice);
        PREVIEW_OPEN.set(true);
        let k5_open = shell_preview_in(Page::Keyboard, &on_k5, 420.0, choice);
        PREVIEW_OPEN.set(false);
        assert_ne!(strip_one, strip_two, "{theme}: the caret and the outline");
        assert_ne!(strip_two, open, "{theme}: the list");
        assert_ne!(strip_two, k5_open, "{theme}: the K5 Pro in focus");
        keep(&format!("ui-800-strip-one-{theme}.png"), &strip_one);
        keep(&format!("ui-800-strip-two-{theme}.png"), &strip_two);
        keep(&format!("ui-800-list-open-{theme}.png"), &open);
        keep(&format!("ui-800-list-open-k5-{theme}.png"), &k5_open);
        keep(&format!("ui-800-lights-corne-{theme}.png"), &corne_lights);
        keep(&format!("ui-800-keyboard-k5-{theme}.png"), &k5_keyboard);
        keep(&format!("ui-800-lights-k5-{theme}.png"), &k5_lights);
    }
}

/// #796: the Keyboard page drawing from each layout source, each with its chip (the Corne from
/// its Studio layout with its turned thumbs, from its profile, and a board guessed from 36 keys),
/// and the zone drawing on the keyboard's layout.
#[test]
fn key_layout_renders() {
    use crate::foxglove::key_layout::{self, tests::corne_from_keyboard};
    use crate::foxglove::keyboard_page::{board, tests::sample_board, BoardView};
    let ground = Color32::from_rgb(0x09, 0x0a, 0x0c);
    // The screens stay dark: their textures are cached per thread, one context's at a time.
    let sample = || BoardView {
        panel: None,
        art: None,
        ..sample_board()
    };
    let draw = |v: BoardView| {
        render_ui(&preview_ctx(), egui::vec2(980.0, 360.0), ground, |ui| {
            board(ui, &v);
        })
    };
    let keyboard = draw(BoardView {
        layout: corne_from_keyboard(),
        ..sample()
    });
    let profile = draw(sample());
    let guessed = draw(BoardView {
        layout: key_layout::resolve(None, &[], 36),
        split: false,
        ..sample()
    });
    keep("ui-796-board-keyboard.png", &keyboard);
    keep("ui-796-board-profile.png", &profile);
    keep("ui-796-board-guessed.png", &guessed);
    assert_ne!(keyboard, profile, "the keyboard's own layout is drawn");
    assert_ne!(profile, guessed);
    let corne = crate::foxglove::fixtures::users_desk::board();
    let drawing = crate::foxglove::boards::drawing(Some(&corne));
    let zones = crate::foxglove::light_zones::from_profile(&corne, &drawing).unwrap();
    let line = crate::foxglove::light_zones::line(Some(&zones), &corne_from_keyboard());
    let zoned = render_ui(&preview_ctx(), egui::vec2(760.0, 420.0), ground, |ui| {
        crate::foxglove::theme::card(ui).show(ui, |ui| {
            crate::foxglove::zones_ui::keyboard(ui, &line, true);
        });
    });
    keep("ui-796-zones-keyboard.png", &zoned);
}

/// #797: keyboards drawn from their definitions, dark and light: a K5 Pro from its VIA
/// definition with a layout option on (Split Backspace), and the same board from a loaded QMK
/// layout, each with its chip.
#[test]
fn definition_layouts_render() {
    use crate::foxglove::key_layout::resolve;
    use crate::foxglove::keyboard_page::{board, tests::sample_board, BoardView};
    use crate::foxglove::theme::{Choice, DARK, LIGHT};
    use crate::foxglove::via_host::{layout_of, tests::k5_pro_opened};
    let (via, layout) = k5_pro_opened(1);
    let via = resolve(None, &[layout_of(&via, &layout).unwrap()], 0);
    let (mut qmk, layout) = k5_pro_opened(0);
    qmk.source = foxglove_via::Source::QmkLayout {
        layout: "LAYOUT_108_ansi".into(),
    };
    qmk.layout_options.clear();
    let qmk = resolve(None, &[layout_of(&qmk, &layout).unwrap()], 0);
    let mut shots = Vec::new();
    for (choice, theme, ground) in [
        (Choice::Dark, "dark", DARK.ground),
        (Choice::Light, "light", LIGHT.ground),
    ] {
        for (name, l) in [("via", &via), ("qmk", &qmk)] {
            let ctx = preview_ctx();
            crate::foxglove::theme::apply_choice(&ctx, choice);
            let v = BoardView {
                layout: l.clone(),
                split: false,
                panel: None,
                art: None,
                // A board drawn from its definition describes no screens (#822).
                screens: Vec::new(),
                ..sample_board()
            };
            let img = render_ui(&ctx, egui::vec2(980.0, 360.0), ground, |ui| {
                board(ui, &v);
            });
            keep(&format!("ui-797-{name}-{theme}.png"), &img);
            shots.push(img);
        }
    }
    assert_ne!(shots[0], shots[1], "the chips differ");
    assert_ne!(shots[0], shots[2], "dark and light");
}

/// #798: "Load a layout…", dark and light: a keyboard whose shape is guessed, with the link
/// after the muted note; the same keyboard drawn from a KLE file it loaded, with its chip and
/// "Forget this layout"; and a bad file's one line under the guess.
#[test]
fn loaded_layouts_render() {
    use crate::foxglove::key_layout::{of_keyboard, resolve, set_for_test};
    use crate::foxglove::keyboard_page::{board, tests::sample_board, BoardView};
    use crate::foxglove::layout_files;
    use crate::foxglove::theme::{Choice, DARK, LIGHT};
    const KB: (u16, u16) = (0xB398, 0x0036);
    const KLE: &str = r#"[{"name": "Split 36"},
        ["Q", "W", "E", "R", "T", {"x": 3}, "Y", "U", "I", "O", "P"],
        ["A", "S", "D", "F", "G", {"x": 3}, "H", "J", "K", "L", ";"],
        ["Z", "X", "C", "V", "B", {"x": 3}, "N", "M", ",", ".", "/"],
        [{"x": 2.5, "y": 0.2}, "Esc", "Tab", {"x": 4}, "Ent", "Bsp"],
        [{"r": 20, "rx": 4.6, "ry": 3.3}, "Spc"],
        [{"r": -20, "rx": 7.4, "ry": 3.3, "x": -1}, "Sft"]]"#;
    crate::foxglove::store::test_config_dir();
    let dir = crate::foxglove::store::test_config_dir()
        .parent()
        .unwrap()
        .join("layout-preview");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("split-36.json");
    std::fs::write(&path, KLE).unwrap();
    set_for_test(KB.0, KB.1, None);
    let guessed = resolve(None, &[], 36);
    layout_files::load(KB.0, KB.1, &path).unwrap();
    let loaded = of_keyboard(KB.0, KB.1, None);
    let view = |layout| BoardView {
        layout,
        split: false,
        panel: None,
        art: None,
        screens: Vec::new(),
        title: "Split 36".into(),
        layout_for: Some(KB),
        ..sample_board()
    };
    let mut shots = Vec::new();
    for (choice, theme, ground) in [
        (Choice::Dark, "dark", DARK.ground),
        (Choice::Light, "light", LIGHT.ground),
    ] {
        let shot =
            |name: &str, layout: &crate::foxglove::key_layout::KeyLayout, problem: Option<&str>| {
                let ctx = preview_ctx();
                crate::foxglove::theme::apply_choice(&ctx, choice);
                if let Some(m) = problem {
                    layout_files::show_problem(KB, m);
                }
                let v = view(layout.clone());
                let img = render_ui(&ctx, egui::vec2(980.0, 340.0), ground, |ui| {
                    board(ui, &v);
                });
                layout_files::show_problem((0, 0), "");
                keep(&format!("ui-798-{name}-{theme}.png"), &img);
                img
            };
        shots.push(shot("guessed", &guessed, None));
        shots.push(shot("kle", &loaded, None));
        shots.push(shot(
            "bad-file",
            &guessed,
            Some("Couldn't load notes.json: it isn't a VIA definition, a QMK keyboard.json / info.json or a KLE layout."),
        ));
    }
    assert_ne!(shots[0], shots[1], "the loaded layout is drawn");
    assert_ne!(shots[0], shots[2], "the bad file's line shows");
    assert_ne!(shots[0], shots[3], "dark and light");
}

/// #822: the board drawing's screens come from the board, dark and light: the Corne's two (as
/// before), none on a one-piece K5 Pro drawn from its VIA definition, and a one-piece board's
/// info and art screens where its profile puts them.
#[test]
fn screens_per_board_render() {
    use crate::foxglove::key_layout::{from_drawing, resolve, LayoutSource};
    use crate::foxglove::keyboard_page::{board, board_screens, tests::sample_board, BoardView};
    use crate::foxglove::parts::Parts;
    use crate::foxglove::screen_roles::of_board;
    use crate::foxglove::theme::{Choice, DARK, LIGHT};
    use crate::foxglove::via_host::{layout_of, tests::k5_pro_opened};
    let (via, layout) = k5_pro_opened(1);
    let via = resolve(None, &[layout_of(&via, &layout).unwrap()], 0);
    let plank = crate::foxglove::fixtures::second_desk::board();
    let mut shots = Vec::new();
    for (choice, theme, ground) in [
        (Choice::Dark, "dark", DARK.ground),
        (Choice::Light, "light", LIGHT.ground),
    ] {
        // The screens stay dark: their textures are cached per thread, one context's at a time.
        let views = [
            (
                "corne",
                BoardView {
                    panel: None,
                    art: None,
                    ..sample_board()
                },
            ),
            (
                "via",
                BoardView {
                    layout: via.clone(),
                    split: false,
                    panel: None,
                    art: None,
                    // No profile and nothing described: no screens.
                    screens: board_screens(&[], None, &Parts::of(None)),
                    ..sample_board()
                },
            ),
            (
                "plank",
                BoardView {
                    layout: from_drawing(
                        plank.drawing.as_ref().unwrap(),
                        false,
                        LayoutSource::Profile,
                    ),
                    split: false,
                    panel: None,
                    art: None,
                    left_label: "KEYBOARD · USB".into(),
                    screens: board_screens(&of_board(&plank), Some(&plank), &plank.parts()),
                    ..sample_board()
                },
            ),
        ];
        for (name, v) in views {
            let ctx = preview_ctx();
            crate::foxglove::theme::apply_choice(&ctx, choice);
            let img = render_ui(&ctx, egui::vec2(980.0, 360.0), ground, |ui| {
                board(ui, &v);
            });
            keep(&format!("ui-822-after-{name}-{theme}.png"), &img);
            shots.push(img);
        }
    }
    assert_ne!(shots[0], shots[1]);
    assert_ne!(shots[0], shots[3], "dark and light");
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
        hello: Some(foxglove_link::protocol::Hello {
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

/// The full samples with the bundled Corne art fitted to the art screen in `TEST_SCREENS`.
fn fitted_samples() -> Samples {
    use crate::foxglove::art_encode::{for_screen, ArtTarget};
    let target = ArtTarget::resolve();
    let mut samples = Samples::full();
    let (_, lib) = samples.art.as_mut().expect("art sample");
    for e in &mut lib.entries {
        let raw = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/lighting/art")
                .join(e.path.file_name().unwrap()),
        )
        .unwrap();
        let (bytes, fit) = for_screen(&raw, &target).unwrap();
        e.preview = crate::foxglove::art_preview::first_frame(&bytes)
            .map(std::sync::Arc::new)
            .map_err(str::to_owned);
        e.bytes = bytes.len();
        e.hash = crate::foxglove::art_library::blob_hash(&bytes);
        e.fit = fit;
    }
    samples
}

/// #833: the Screens page on a made-up board with an e-paper art screen (88x184, 1 s a frame),
/// "Animate on e-paper" off and on, and Rotation without Lights wake; beside it the Corne's
/// page, which a stored "on" leaves pixel for pixel as it was. Dark and light.
#[test]
fn screens_epaper_switch_renders() {
    use crate::foxglove::art_encode::TEST_SCREENS;
    use crate::foxglove::art_ui::{tests::epaper_screens, PREVIEW_ANIMATE_EPAPER};
    use crate::foxglove::fixtures::users_desk;
    use crate::foxglove::screen_roles::of_board;
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::Choice;
    let height = 900.0;
    for (choice, theme) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let corne = shell_preview_in(Page::Screens, &Samples::full(), height, choice);
        TEST_SCREENS.set(Some(of_board(&users_desk::board())));
        PREVIEW_ANIMATE_EPAPER.set(Some(true));
        let corne_on = shell_preview_in(Page::Screens, &Samples::full(), height, choice);
        keep(&format!("ui-833-corne-{theme}.png"), &corne_on);
        assert!(corne == corne_on, "the Corne's page is unchanged ({theme})");
        TEST_SCREENS.set(Some(epaper_screens()));
        let samples = fitted_samples();
        let mut shots = Vec::new();
        for (on, name) in [(false, "off"), (true, "on")] {
            PREVIEW_ANIMATE_EPAPER.set(Some(on));
            let img = shell_preview_in(Page::Screens, &samples, height, choice);
            keep(&format!("ui-833-epaper-{name}-{theme}.png"), &img);
            shots.push(img);
        }
        TEST_SCREENS.set(None);
        PREVIEW_ANIMATE_EPAPER.set(None);
        assert!(shots[0] != shots[1], "the switch shows its state ({theme})");
        assert!(
            shots[0] != corne,
            "e-paper's page differs from the Corne's ({theme})"
        );
    }
}

/// The Screens page on a made-up board with an 88x184 upright art screen (#340): the bundled
/// Corne art fitted to it (centred, marked "Not made for this board"), and a piece that can't
/// go on it listed with the reason. Beside it, the Corne's page as it was.
#[test]
fn screens_art_for_another_board_renders() {
    use crate::foxglove::art_encode::{DOES_NOT_FIT, TEST_SCREENS};
    use crate::foxglove::screen_roles::{Role, Screen};
    use crate::foxglove::shell::Page;
    let corne = shell_preview_with(Page::Screens, &Samples::full(), 1500.0);
    keep("ui-340-screens-corne.png", &corne);
    TEST_SCREENS.set(Some(vec![Screen {
        id: 0,
        role: Role::Art,
        name: "Art screen".into(),
        width: 88,
        height: 184,
        format: "1bit".into(),
        rotation: 0,
        refresh: Default::default(),
        min_frame_ms: 0,
        max_blob: None,
        slots: None,
    }]));
    let mut samples = fitted_samples();
    let (_, lib) = samples.art.as_mut().expect("art sample");
    lib.sources.push(crate::foxglove::art_library::Source {
        folder: "art-src/Bandit_wide".into(),
        blob: Err(format!(
            "{DOES_NOT_FIT}70412 bytes is more than a blob can hold"
        )),
        preview: None,
        fit: None,
    });
    let other = shell_preview_with(Page::Screens, &samples, 1500.0);
    // A board with art of both kinds: one piece made for it, so the fitted ones get a chip.
    let (_, lib) = samples.art.as_mut().expect("art sample");
    lib.entries[0].fit = crate::foxglove::art_encode::Fit::AsMade;
    let mixed = shell_preview_with(Page::Screens, &samples, 1500.0);
    TEST_SCREENS.set(None);
    keep("ui-340-screens-other-board.png", &other);
    keep("ui-340-screens-mixed.png", &mixed);
    assert_ne!(other, mixed);
    assert_ne!(corne, other);
    let lit = Color32::from_rgb(0xe8, 0xea, 0xe9);
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    assert!(count(&other, lit) > 300, "fitted art thumbnails");
}

/// The Screens page with the art playlist on (#522): its pieces in order (Rotation's piece now
/// in bold), the places lent to it, Add a piece…, and the keyboard card's lent-places line.
#[test]
fn screens_playlist_renders() {
    use crate::foxglove::art_cycle::{Item, Step};
    use crate::foxglove::shell::Page;
    let mut samples = Samples::full();
    let (s, _) = samples.art.as_mut().expect("art sample");
    s.cycle.playlist = true;
    s.cycle.places = vec![6, 7];
    s.cycle.items = vec![
        Item::File("art/fury.bin".into()),
        Item::File("art/hearth.bin".into()),
        Item::BuiltIn("Bandit_Emblem".into()),
        Item::Slot(2),
        Item::File("art/moon-garden.bin".into()),
        Item::File("art/constellations.bin".into()),
    ];
    s.cycle.position = 1;
    s.cycle.set_step(Step::Minutes, true);
    s.slots.push(foxglove_link::protocol::SlotInfo {
        slot: 6,
        used: true,
        writes: 14,
        len: 900,
        name: "moon-garden".into(),
    });
    if let Some(st) = s.status.as_mut() {
        st.flash_used |= 1 << 6;
    }
    let with = shell_preview_with(Page::Screens, &samples, 1400.0);
    let without = shell_preview_with(Page::Screens, &Samples::full(), 1400.0);
    keep("ui-522-playlist.png", &with);
    keep("ui-522-playlist-off.png", &without);
    assert_ne!(with, without, "the playlist shows");
}

/// The keyboard card after a connect that found kept places lost (#521): what changed, and
/// what Foxglove put back from this PC (the words `art_seen::restored_line` makes).
#[test]
fn screens_art_restored_renders() {
    use crate::foxglove::shell::Page;
    let mut samples = Samples::full();
    let (s, _) = samples.art.as_mut().expect("art sample");
    let line = crate::foxglove::art_seen::restored_line(
        &["fury".into(), "constellations".into()],
        &[("skyline".into(), "this PC has no copy of it".into())],
        3,
    );
    s.notice = Some(crate::foxglove::art_seen::with_restored(
        Some("No longer kept on the keyboard: fury, constellations, skyline.".into()),
        &line,
    ));
    let img = shell_preview_with(Page::Screens, &samples, 1300.0);
    keep("ui-521-restored.png", &img);
    assert_ne!(
        img,
        shell_preview_with(Page::Screens, &Samples::full(), 1300.0)
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

/// Sideloads (#381): developer mode on with an allowed and a not-yet-allowed unsigned plugin,
/// the same page with it off, and the approval card for an unsigned file under a catalog id.
#[test]
fn plugins_sideload_renders() {
    use crate::foxglove::plugins_ui::tests as plugins;
    use crate::foxglove::shell::Page;
    let base = shell_preview_with(Page::Plugins, &Samples::full(), 1150.0);
    for (name, view) in [
        ("ui-381-dev-on.png", plugins::sample_sideloads()),
        ("ui-381-dev-off.png", plugins::sample_dev_off()),
        ("ui-381-approval.png", plugins::sample_unsigned_approval()),
    ] {
        let samples = Samples {
            plugins: view,
            ..Samples::full()
        };
        let img = shell_preview_with(Page::Plugins, &samples, 1150.0);
        keep(name, &img);
        assert_ne!(img, base, "{name} differs from the plain page");
    }
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
    // The theme picker's selected pill sits in "Appearance".
    assert!(count(&img, DARK.selected_fill) > 100, "the Dark pill");
}

/// "Follow the keyboard I type on" (#802) on the App page, off and on, dark and light, each with
/// its ⓘ open.
#[test]
fn follow_typing_card_renders() {
    use crate::foxglove::follow_typing::hover;
    use crate::foxglove::keyboard_focus as kf;
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::{self, Choice};
    crate::foxglove::store::test_config_dir();
    kf::TEST_SECTION.set(Some("focus-follow-typing-preview"));
    let mut shots = Vec::new();
    for (choice, name) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        for on in [false, true] {
            kf::set_follow_typing(on);
            theme::PREVIEW_PINNED.set(Some(hover(on)));
            let img = shell_preview_in(Page::Settings, &Samples::full(), 1000.0, choice);
            theme::PREVIEW_PINNED.set(None);
            let state = if on { "on" } else { "off" };
            keep(&format!("ui-802-follow-typing-{state}-{name}.png"), &img);
            shots.push(img);
        }
    }
    kf::set_follow_typing(false);
    kf::TEST_SECTION.set(None);
    assert_ne!(shots[0], shots[1], "the switch shows its state");
    assert_ne!(shots[0], shots[2], "dark and light");
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

/// #670: the Lights page with `lights`, at the Lights page's wide size.
fn effect_preview(name: &str, lights: crate::foxglove::lights_page::View) -> image::RgbaImage {
    let mut samples = Samples::full();
    samples.lights = lights;
    let img = shell_preview_with(crate::foxglove::shell::Page::Lights, &samples, 980.0);
    if !name.is_empty() {
        keep(name, &img);
    }
    img
}

/// Pixels of a strong, bright green (hue 100-140°): the rainbow has them, the page's
/// swatches and theme don't.
fn rainbow_green(img: &image::RgbaImage) -> usize {
    img.pixels()
        .filter(|p| {
            let [r, g, b, _] = p.0;
            let hsv = egui::ecolor::Hsva::from(Color32::from_rgb(r, g, b));
            let hue = hsv.h * 360.0;
            hsv.s > 0.7 && hsv.v > 0.85 && (100.0..=140.0).contains(&hue)
        })
        .count()
}

/// #670: a colour effect (Breathe) shows the colour picker, brightness and speed.
#[test]
fn lights_colour_effect_renders() {
    use crate::foxglove::lights_page::tests::sample_effect;
    let img = effect_preview("ui-670-colour-effect.png", sample_effect(1, 26, 76));
    assert_eq!(rainbow_green(&img), 0, "no rainbow under a colour effect");
}

/// #670: a colour-free effect (Swirl) shows a rainbow under "Looks like" in place of the colour
/// picker, with no accent switch (it has no hue), and the halves say just "Swirl".
#[test]
fn lights_rainbow_effect_renders() {
    use crate::foxglove::lights_page::tests::sample_effect;
    let colour = effect_preview("", sample_effect(1, 26, 76));
    let img = effect_preview("ui-670-rainbow-effect.png", sample_effect(3, 26, 76));
    assert!(rainbow_green(&img) > 100, "the rainbow shows");
    assert_ne!(img, colour);
}

/// #670: the board shows Swirl while Foxglove saved Solid amber: the rainbow, and a line
/// naming the saved look.
#[test]
fn lights_board_differs_renders() {
    use crate::foxglove::lights_page::tests::{sample_board_differs, sample_effect};
    let plain = effect_preview("", sample_effect(3, 39, 70));
    let img = effect_preview("ui-670-board-differs.png", sample_board_differs());
    assert!(
        rainbow_green(&img) > 100,
        "the board's rainbow, not the saved amber"
    );
    assert_ne!(img, plain, "the saved look is named");
}

/// #626: the board draws Swirl in the chosen colour (amber): the colour controls and the
/// amber band, no rainbow; beside it the same Swirl from firmware that doesn't say (the rainbow).
#[test]
fn lights_colour_swirl_renders() {
    use crate::foxglove::lights_page::tests::{sample_colour_swirl, sample_effect};
    let rainbow = effect_preview("ui-626-swirl-before.png", sample_effect(3, 39, 70));
    let img = effect_preview("ui-626-swirl-colour.png", sample_colour_swirl(39, 70));
    let blue = effect_preview("ui-626-swirl-colour-blue.png", sample_colour_swirl(210, 80));
    assert!(rainbow_green(&rainbow) > 100, "older firmware: the rainbow");
    assert_eq!(rainbow_green(&img), 0, "a colour Swirl shows no rainbow");
    assert_eq!(rainbow_green(&blue), 0);
    assert_ne!(img, blue, "the band follows the colour");
}

/// #491: the owner line on the Lights page (a scene drives the lights, then Windows Dynamic
/// Lighting has them and the scene steps aside, in the warning colour), and the Scenes page's
/// status while the scene steps aside ("Paused for Windows Dynamic Lighting", amber, in place
/// of the green "Running"; the page's owner line shows only with no scene running).
#[test]
fn lights_owner_line_renders() {
    use crate::foxglove::lights_owner::{line, Who, PREVIEW};
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let near = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| {
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
                d(p.0[0], c.r()) + d(p.0[1], c.g()) + d(p.0[2], c.b()) < 24
            })
            .count()
    };
    let scene = Some("Bandit scanline");
    let page_with = |page, height, owner: Option<Who>, yielded: Option<&'static str>| {
        PREVIEW.set(owner.map(|w| line(&w, scene, Some("Eyelash Corne"))));
        let mut samples = Samples::full();
        samples.scenes.0.yielded_to = yielded;
        let img = shell_preview_with(page, &samples, height);
        PREVIEW.set(None);
        img
    };
    let plain = page_with(Page::Lights, 980.0, None, None);
    let lights = page_with(
        Page::Lights,
        980.0,
        Some(Who::Scene("Bandit scanline".into())),
        None,
    );
    let windows = page_with(Page::Lights, 980.0, Some(Who::Windows), None);
    let scenes_running = page_with(Page::Scenes, 1400.0, Some(Who::Windows), None);
    let scenes = page_with(
        Page::Scenes,
        1400.0,
        Some(Who::Windows),
        Some("Windows Dynamic Lighting"),
    );
    keep("ui-491-owner-lights-scene.png", &lights);
    keep("ui-491-owner-lights-windows.png", &windows);
    keep("ui-491-owner-scenes-windows.png", &scenes);
    assert_ne!(plain, lights, "the Lights page shows the line");
    assert_ne!(lights, windows, "the line follows the owner");
    assert!(
        near(&windows, DARK.warn) > near(&lights, DARK.warn),
        "the scene waiting for Windows is a warning"
    );
    assert_ne!(scenes_running, scenes, "the status says paused");
    assert!(
        near(&scenes, DARK.warn) > near(&scenes_running, DARK.warn),
        "paused is amber"
    );
    assert!(
        near(&scenes, DARK.ok) < near(&scenes_running, DARK.ok),
        "not the green Running"
    );
}

/// #749: Windows Dynamic Lighting holds the keyboard's lights. Only the Lights page's Keyboard
/// card says so: one amber line naming the Windows setting, then a small button that opens it.
/// With the lights free, it doesn't show. Saved as the review image.
#[test]
fn dynamic_lighting_notice_renders() {
    use crate::foxglove::lights_owner::{line, Who, PREVIEW};
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let near = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| {
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
                d(p.0[0], c.r()) + d(p.0[1], c.g()) + d(p.0[2], c.b()) < 24
            })
            .count()
    };
    let device = Some("Eyelash Corne");
    PREVIEW.set(Some(line(&Who::Windows, None, device)));
    let lights = shell_preview_with(Page::Lights, &Samples::full(), 980.0);
    PREVIEW.set(Some(line(&Who::Keyboard, None, device)));
    let lights_free = shell_preview_with(Page::Lights, &Samples::full(), 980.0);
    PREVIEW.set(None);
    keep("ui-749-dl-notice.png", &lights);
    assert_ne!(lights, lights_free, "the Lights page shows the notice");
    assert!(
        near(&lights, DARK.warn) > near(&lights_free, DARK.warn),
        "the notice is amber"
    );
}

/// #749 b: Windows Dynamic Lighting holds the keyboard's lights and Foxglove draws through the
/// keyboard's own LampArray: the same amber notice and settings button, now saying Foxglove
/// draws through it while its window is in front. Saved as the review image.
#[test]
fn dynamic_lighting_through_renders() {
    use crate::foxglove::lights_owner::{line, line_through, Who, PREVIEW};
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let near = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| {
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
                d(p.0[0], c.r()) + d(p.0[1], c.g()) + d(p.0[2], c.b()) < 24
            })
            .count()
    };
    let device = Some("Eyelash Corne");
    PREVIEW.set(Some(line_through(&Who::Windows, None, device, Some(false))));
    let through = shell_preview_with(Page::Lights, &Samples::full(), 980.0);
    PREVIEW.set(Some(line(&Who::Windows, None, device)));
    let held = shell_preview_with(Page::Lights, &Samples::full(), 980.0);
    PREVIEW.set(Some(line(&Who::Keyboard, None, device)));
    let free = shell_preview_with(Page::Lights, &Samples::full(), 980.0);
    PREVIEW.set(None);
    keep("ui-749-dl-through.png", &through);
    assert_ne!(
        through, held,
        "the notice says Foxglove draws through Windows"
    );
    assert!(
        near(&through, DARK.warn) > near(&free, DARK.warn),
        "still the amber notice"
    );
}

/// The Scenes page (#487): the user's room with a scene running (wide and at the window's
/// default size), a scene being dragged, and an empty room with nothing running.
#[test]
fn scenes_page_renders() {
    use crate::foxglove::scenes_ui::{tests, Draft, View};
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let with = shell_preview_with(Page::Scenes, &Samples::full(), 1000.0);
    let without = shell_preview_with(Page::Scenes, &Samples::none(), 1000.0);
    keep("ui-scenes.png", &with);
    keep("ui-scenes-none.png", &without);
    // A sweep with a palette, and a device being dragged (the case, 300 mm to the left).
    let mut s = Samples::full();
    s.scenes = (
        View {
            look: crate::foxglove::scene_looks::built_in()
                .into_iter()
                .find(|p| p.name == "Steel sweep")
                .unwrap()
                .look,
            running: None,
            ..tests::sample()
        },
        Draft {
            name: "Desk".into(),
            drag: Some((
                crate::foxglove::scene_room::tests::SAMPLE_CASE.into(),
                foxglove_scene::Point::new(-300.0, 0.0),
            )),
            ..Draft::default()
        },
    );
    let dragging = shell_preview_with(Page::Scenes, &s, 1000.0);
    keep("ui-scenes-sweep-drag.png", &dragging);
    // The orange of Stop's neighbour pills and the scanline; Start is orange with nothing running.
    assert!(count(&with, DARK.accent) > 200, "orange");
    assert!(count(&without, DARK.accent) > 200, "the Start button");
    assert!(count(&with, DARK.surface) > (with.width() * with.height()) as usize / 20);
    assert_ne!(with, without);
    assert_ne!(with, dragging);
}

/// A wallpaper pack's looks on the Scenes page (#488, #562; the fake third-party pack): one
/// running with "Art screen too" and its wallpaper on screen, and one picked with nothing
/// running, at the window's default size (one column).
#[test]
fn scenes_wallpaper_presets_render() {
    use crate::foxglove::scene_looks::scenes;
    use crate::foxglove::scenes_ui::{tests, View};
    use crate::foxglove::shell::Page;
    let look = |id: &str| {
        scenes()
            .looks
            .iter()
            .find(|w| w.wallpaper == id)
            .unwrap()
            .preset
            .clone()
    };
    let mut s = Samples::full();
    let reef = look("tidepool.reef");
    s.scenes.0 = View {
        look: reef.look.clone(),
        running: Some(reef.name.clone()),
        with_art: true,
        on_screen: vec!["tidepool.reef".into()],
        t: foxglove_scene::SceneTime(600),
        ..tests::sample()
    };
    let running = shell_preview_with(Page::Scenes, &s, 1100.0);
    keep("ui-scenes-presets-running.png", &running);
    let beam = look("tidepool.lighthouse");
    s.scenes.0 = View {
        look: beam.look.clone(),
        on_screen: vec!["tidepool.lighthouse".into(), "tidepool.reef".into()],
        ..tests::sample_none()
    };
    let picked = shell_preview_with(Page::Scenes, &s, 1100.0);
    keep("ui-scenes-presets-picked.png", &picked);
    assert_ne!(running, picked);
}

/// #40: the "Wallpaper" palette picked for a sweep, running, wide; and a drift with it at the
/// window's default size (one column). Under test the wallpaper on screen is a sunset picture.
#[test]
fn scenes_wallpaper_palette_renders() {
    use crate::foxglove::scene_looks::{Direction, Kind, Look, WALLPAPER};
    use crate::foxglove::scenes_ui::{tests, View};
    use crate::foxglove::shell::Page;
    let mut s = Samples::full();
    s.scenes.0 = View {
        look: Look {
            kind: Kind::Sweep,
            palette: WALLPAPER.into(),
            direction: Direction::Right,
            speed: 4,
            ..Look::default()
        },
        running: Some("Custom scene".into()),
        t: foxglove_scene::SceneTime(900),
        ..tests::sample()
    };
    let sweep = shell_preview_with(Page::Scenes, &s, 1200.0);
    keep("ui-40-scenes-wallpaper-sweep.png", &sweep);
    s.scenes.0 = View {
        look: Look {
            kind: Kind::Drift,
            palette: WALLPAPER.into(),
            width_mm: 250,
            ..Look::default()
        },
        ..tests::sample_none()
    };
    let drift = shell_preview_with(Page::Scenes, &s, 1000.0);
    keep("ui-40-scenes-wallpaper-drift.png", &drift);
    assert_ne!(sweep, drift);
}

/// The Triggers card (#489) under Effect: the Game profile starting its scene with key
/// ripples on, and the base settings with every box off.
#[test]
fn scenes_triggers_render() {
    use crate::foxglove::shell::Page;
    let on = shell_preview_with(Page::Scenes, &Samples::full(), 1200.0);
    let off = shell_preview_with(Page::Scenes, &Samples::none(), 1200.0);
    keep("ui-scenes-triggers.png", &on);
    keep("ui-scenes-triggers-none.png", &off);
    assert_ne!(on, off);
}

/// The review sheet for #488: each wallpaper look found (in tests, the fake pack) rendered
/// from the real effect code over a room (a monitor above the keyboard), six moments of one
/// cycle per row, and a GIF of
/// each look's cycle. Written only with FOXGLOVE_UI_PREVIEW_DIR set.
#[test]
fn scenes_wallpaper_preset_sheet() {
    use crate::foxglove::scene_looks::scenes;
    use foxglove_scene::{Point, Rgb, SceneTime};
    let Some(dir) = std::env::var_os("FOXGLOVE_UI_PREVIEW_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let room = crate::foxglove::scene_room::tests::sample_room();
    let bounds = room.bounds;
    let (w, h) = (
        160u32,
        (160.0 * bounds.h / bounds.w).round().max(40.0) as u32,
    );
    let frame = |effect: &foxglove_scene::Effect, t: SceneTime| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            let at = Point::new(
                bounds.x + bounds.w * (x as f32 + 0.5) / w as f32,
                bounds.y + bounds.h * (y as f32 + 0.5) / h as f32,
            );
            let Rgb([r, g, b]) = effect.colour(at, t, &bounds);
            image::Rgba([r, g, b, 255])
        })
    };
    let looks = &scenes().looks;
    let gap = 6u32;
    let cols = 6u32;
    let mut sheet = image::RgbaImage::from_pixel(
        cols * (w + gap) + gap,
        looks.len() as u32 * (h + gap) + gap,
        image::Rgba([0x09, 0x0a, 0x0c, 255]),
    );
    for (row, wl) in looks.iter().enumerate() {
        let effect = wl.preset.look.effect(&room);
        // One whole cycle (Drift's is four of its speed's periods).
        let speed_ms = i64::from(crate::foxglove::scene_looks::period_ms(
            wl.preset.look.speed,
        ));
        let period = match wl.preset.look.kind {
            crate::foxglove::scene_looks::Kind::Drift => speed_ms * 4,
            _ => speed_ms,
        };
        for c in 0..cols {
            let t = SceneTime(period * i64::from(c) / i64::from(cols));
            let img = frame(&effect, t);
            image::imageops::overlay(
                &mut sheet,
                &img,
                i64::from(gap + c * (w + gap)),
                i64::from(gap + row as u32 * (h + gap)),
            );
        }
        let path = dir.join(format!("ui-scenes-presets-{}.gif", wl.wallpaper));
        let file = std::fs::File::create(&path).expect("create the gif");
        let mut gif = image::codecs::gif::GifEncoder::new_with_speed(file, 10);
        gif.set_repeat(image::codecs::gif::Repeat::Infinite)
            .expect("loop the gif");
        let n = 16;
        let delay = image::Delay::from_numer_denom_ms((period / n) as u32, 1);
        for k in 0..n {
            let img = frame(&effect, SceneTime(period * k / n));
            gif.encode_frame(image::Frame::from_parts(img, 0, 0, delay))
                .expect("a gif frame");
        }
    }
    keep("ui-scenes-presets-sheet.png", &sheet);
}

/// The preview asks for frames only while the Scenes page is drawn (#487): another page, with
/// the same scene chosen, asks for none.
#[test]
fn the_scenes_preview_repaints_only_on_its_page() {
    use crate::foxglove::scenes_ui::REPAINTS;
    use crate::foxglove::shell::Page;
    REPAINTS.set(0);
    let _ = shell_preview_with(Page::Lights, &Samples::full(), 600.0);
    let _ = shell_preview_with(Page::Keyboard, &Samples::full(), 600.0);
    assert_eq!(REPAINTS.get(), 0, "a hidden page asks for no frames");
    let _ = shell_preview_with(Page::Scenes, &Samples::full(), 600.0);
    assert!(REPAINTS.get() > 0, "the visible page animates");
}

/// "Save battery: dim lights on battery" on the Lights page: closed (the default), with the
/// levels open, with the right half low, and switched off.
#[test]
fn battery_dim_renders() {
    use crate::foxglove::battery_dim_ui::{tests, DimState, Draft};
    use crate::foxglove::shell::Page;
    use foxglove_link::protocol::BatteryDimConfig;
    let open = Draft {
        config: None,
        open: true,
    };
    let off = Draft {
        config: Some(BatteryDimConfig {
            enabled: false,
            ..Default::default()
        }),
        open: false,
    };
    let mut seen: Vec<image::RgbaImage> = Vec::new();
    for (name, state, draft) in [
        ("ui-battery-dim.png", tests::sample(), Draft::default()),
        ("ui-battery-dim-adjust.png", tests::sample(), open),
        (
            "ui-battery-dim-low.png",
            tests::sample_low(),
            Draft::default(),
        ),
        ("ui-battery-dim-off.png", tests::sample(), off),
        (
            "ui-battery-dim-old-firmware.png",
            DimState::NotSupported,
            Draft::default(),
        ),
    ] {
        let mut samples = Samples::full();
        samples.battery_dim = (state, draft);
        let img = shell_preview_with(Page::Lights, &samples, 980.0);
        keep(name, &img);
        assert!(
            seen.iter().all(|other| *other != img),
            "{name} looks like another"
        );
        seen.push(img);
    }
}

/// The Lights page's "Other devices" card (#423): the 45L with colours, the cooler and the
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
    // The second desk (#557): another vendor's case, a lightbar and the one-piece Plank 3.
    let mut second = Samples::full();
    second.pc = crate::foxglove::pc_lights_ui::tests::sample_second_desk();
    let second = shell_preview_with(Page::Lights, &second, 1180.0);
    keep("ui-557-pc-lights-second-desk.png", &second);
    assert!(
        count(&second, DARK.warn) < count(&with, DARK.warn),
        "no vendor's note"
    );
    assert_ne!(second, with);
    keep("ui-423-pc-lights.png", &with);
    keep("ui-423-pc-lights-fresh.png", &fresh);
    keep("ui-423-pc-lights-none.png", &none);
    // Light Studio's note is in the warning colour; the fresh card has none.
    assert!(count(&with, DARK.warn) > 100, "the Light Studio note");
    assert!(count(&fresh, DARK.warn) < count(&with, DARK.warn));
    assert_ne!(with, fresh);
    assert_ne!(fresh, none);
}

/// #611's zone drawing and Check zones (docs/lighting-zones.md sections 2 to 4): the Corne's
/// zones shown, the case's and the mouse's, a per-key board's, a keyboard whose zones are
/// unknown, the probe mid-way with keys picked and at its end, and the Scenes page's words.
#[test]
fn zones_render() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::zones_ui::PREVIEW_OPEN;
    let mut shots = Vec::new();
    let open = |keys: Vec<String>| PREVIEW_OPEN.set(keys);
    open(vec![crate::foxglove::lights_page::ZONES_KEY.into()]);
    let corne = shell_preview_with(Page::Lights, &Samples::full(), 1500.0);
    keep("ui-611-zones-corne.png", &corne);
    shots.push(corne);
    // The Hub's cut on #775: rows with named pickers have no Show zones; the strip keeps it.
    let mut s = Samples::full();
    s.pc.pc
        .devices
        .push(crate::foxglove::pc_lights_ui::tests::strip_row());
    open(s.pc.pc.devices.iter().map(|r| r.key.clone()).collect());
    let devices = shell_preview_with(Page::Lights, &s, 1700.0);
    keep("ui-611-zones-devices.png", &devices);
    shots.push(devices);
    open(vec![crate::foxglove::lights_page::ZONES_KEY.into()]);
    let mut s = Samples::full();
    let plank = crate::foxglove::fixtures::second_desk::board();
    s.lights.parts = plank.parts();
    s.lights.zones = Some(crate::foxglove::lights_page::tests::zone_line_of(&plank));
    if let Some(st) = s.lights.state.as_mut() {
        st.right = None;
    }
    let per_key = shell_preview_with(Page::Lights, &s, 1500.0);
    keep("ui-611-zones-per-key.png", &per_key);
    shots.push(per_key);
    open(vec![]);
    let mut s = Samples::full();
    s.lights = crate::foxglove::lights_page::tests::sample_zones_unknown();
    let unknown = shell_preview_with(Page::Lights, &s, 1180.0);
    keep("ui-611-zones-unknown.png", &unknown);
    shots.push(unknown);
    // The probe: lamp 10 on the left half lit, two keys picked; then the end.
    let mut s = Samples::full();
    let (v, mut w) = crate::foxglove::led_map_ui::tests::sample(9);
    w.toggle(9);
    w.toggle(10);
    s.leds = Some((v, w));
    let probe = shell_preview_with(Page::Lights, &s, 900.0);
    keep("ui-611-check-zones.png", &probe);
    shots.push(probe);
    let mut s = Samples::full();
    s.leds = Some(crate::foxglove::led_map_ui::tests::sample(42));
    let done = shell_preview_with(Page::Lights, &s, 900.0);
    keep("ui-611-check-zones-done.png", &done);
    shots.push(done);
    let mut s = Samples::full();
    s.scenes.0.keyboard_frames = true;
    let scenes = shell_preview_with(Page::Scenes, &s, 1000.0);
    keep("ui-611-scenes-zones.png", &scenes);
    shots.push(scenes);
    for (i, a) in shots.iter().enumerate() {
        for b in &shots[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

/// "Find my LEDs" (#90 row G): the Keyboard card's entry, the first LED while it lights, one
/// mid-way on each half with the LEDs placed so far, the end with Save, and the Scenes page's
/// prompt while the LEDs aren't placed.
#[test]
fn led_map_renders() {
    use crate::foxglove::led_map_ui::tests::sample;
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let entry = shell_preview_with(Page::Lights, &Samples::full(), 1180.0);
    keep("ui-ledmap-entry.png", &entry);
    let mut shots = Vec::new();
    for (name, answered, lit) in [
        ("first", 0, false),
        ("left", 9, true),
        ("right", 30, true),
        ("done", 42, true),
    ] {
        let mut s = Samples::full();
        let (mut v, w) = sample(answered);
        v.lit = lit;
        s.leds = Some((v, w));
        let img = shell_preview_with(Page::Lights, &s, 900.0);
        keep(&format!("ui-ledmap-{name}.png"), &img);
        shots.push(img);
    }
    // Check zones (#611): the keys each lamp lit fill in as it goes on.
    for pair in shots.windows(2) {
        assert_ne!(pair[0], pair[1]);
    }
    let mut s = Samples::full();
    s.scenes.0.keyboard_frames = true;
    s.scenes.0.leds_placed = false;
    let prompt = shell_preview_with(Page::Scenes, &s, 1000.0);
    keep("ui-ledmap-scenes-prompt.png", &prompt);
    let placed = shell_preview_with(Page::Scenes, &Samples::full(), 1000.0);
    assert!(
        count(&prompt, DARK.warn) > count(&placed, DARK.warn),
        "the prompt"
    );
}

/// "Check LED map" (guess and confirm): the dot on the shipped Corne map, after a correction,
/// once the dot stopped, the Lights entry for a shipped map, and the Scenes prompt for it.
#[test]
fn led_map_confirm_renders() {
    use crate::foxglove::led_map_confirm::Answer;
    use crate::foxglove::led_map_ui::tests::check_sample;
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let mut shots = Vec::new();
    for (name, step, fixes, played) in [
        ("shipped", Some(4), vec![], false),
        (
            "fixed",
            Some(12),
            vec![Answer::Mirrored, Answer::Backwards],
            false,
        ),
        (
            "stopped",
            None,
            vec![Answer::Mirrored, Answer::Backwards, Answer::RowsFlipped],
            true,
        ),
    ] {
        let mut s = Samples::full();
        let (mut v, c) = check_sample(step, &fixes);
        v.played = played;
        s.ledcheck = Some((v, c));
        let img = shell_preview_with(Page::Lights, &s, 900.0);
        keep(&format!("ui-ledmap-confirm-{name}.png"), &img);
        shots.push(img);
    }
    for pair in shots.windows(2) {
        assert_ne!(pair[0], pair[1]);
    }
    // The dot (white, ringed) shows while it runs, not once it stopped.
    let white = |i: &image::RgbaImage| count(i, Color32::WHITE);
    assert!(white(&shots[0]) > white(&shots[2]), "the dot");
    let entry = shell_preview_with(Page::Lights, &Samples::full(), 1180.0);
    keep("ui-ledmap-confirm-entry.png", &entry);
    let mut s = Samples::full();
    s.scenes.0.keyboard_frames = true;
    s.scenes.0.leds_placed = false;
    s.scenes.0.leds_shipped = true;
    let shipped = shell_preview_with(Page::Scenes, &s, 1000.0);
    keep("ui-ledmap-confirm-scenes.png", &shipped);
    s.scenes.0.leds_shipped = false;
    let unknown = shell_preview_with(Page::Scenes, &s, 1000.0);
    assert!(
        count(&unknown, DARK.warn) > count(&shipped, DARK.warn),
        "a shipped map is a quiet note, not a warning"
    );
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
        let (v, w) = sample(step, false);
        let screen = w.current.screen;
        s.calibrate = Some((v, w));
        let img = shell_preview_with(Page::Lights, &s, 1180.0);
        keep(
            &format!("ui-calibrate-{}.png", step.name().to_lowercase()),
            &img,
        );
        // The screen's swatch is the colour through the screen side, large.
        assert!(count(&img, screen) > 5000, "{step:?} swatch");
        shots.push(img);
    }
    let mut master = Samples::full();
    master.calibrate = Some(sample(Step::Blue, true));
    let master = shell_preview_with(Page::Lights, &master, 1180.0);
    keep("ui-calibrate-master.png", &master);
    // These match is the orange primary button; every colour looks different.
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

/// A strip whose model class has a catalog hint (#473): its first match, White, opens its
/// choices from the light already warmed by the hint (green and blue down), the screen as is.
#[test]
fn calibrate_hinted_renders() {
    use crate::foxglove::calibrate_ui::{
        tests::{corne, hinted_strip},
        View, Wizard,
    };
    use crate::foxglove::shell::Page;
    let mut w = Wizard::new(hinted_strip(), None);
    w.fresh = false;
    let sent = w.ask.as_ref().and_then(|b| b.pair()).unwrap().0;
    assert_eq!(w.ask.as_ref().unwrap().start(), w.current.light);
    let mut s = Samples::full();
    s.calibrate = Some((
        View {
            others: vec![corne()],
            moving: false,
        },
        w,
    ));
    let img = shell_preview_with(Page::Lights, &s, 1180.0);
    keep("ui-calibrate-hinted.png", &img);
    assert!(
        img.pixels().filter(|p| p.0[..3] == sent).count() > 2000,
        "the sent swatch draws candidate A around the hinted light {sent:?}"
    );
}

/// The calibrator's "Adjust" switch on each side, side by side in one image: blue with the light
/// down 30% and the screen up 5%, both kept. On the light side the device's A and B sit beside
/// the screen's swatch; on the screen side the screen draws both.
#[test]
fn calibrate_two_way_renders() {
    use crate::foxglove::calibrate_ui::{tests::sample, Step};
    use crate::foxglove::shell::Page;
    use foxglove_devices::calibration::Side;
    let mut shots = Vec::new();
    for side in [Side::Light, Side::Screen] {
        let mut s = Samples::full();
        let (v, mut w) = sample(Step::Blue, false);
        w.set_side(side);
        s.calibrate = Some((v, w));
        shots.push(shell_preview_with(Page::Lights, &s, 1180.0));
    }
    assert_ne!(shots[0], shots[1], "the switch shows which side moves");
    let (a, b) = (&shots[0], &shots[1]);
    let gap = 24;
    let mut both = image::RgbaImage::from_pixel(
        a.width() + gap + b.width(),
        a.height().max(b.height()),
        image::Rgba([0, 0, 0, 255]),
    );
    image::imageops::overlay(&mut both, a, 0, 0);
    image::imageops::overlay(&mut both, b, i64::from(a.width() + gap), 0);
    keep("ui-calibrate-2way.png", &both);
    // The light side: the screen's swatch draws blue up 5%. The screen side: both candidates
    // around it, large.
    let n = |img: &image::RgbaImage, c: [u8; 3]| img.pixels().filter(|p| p.0[..3] == c).count();
    assert!(n(a, [0, 0, 161]) > 5000);
    let (_, mut w) = sample(Step::Blue, false);
    w.set_side(Side::Screen);
    let (ca, cb) = w.ask.as_ref().and_then(|b| b.pair()).unwrap();
    assert!(n(b, ca) > 5000 && n(b, cb) > 5000, "{ca:?} {cb:?}");
}

/// The A/B choices (#474) on Grey against the screen: a step part-way (two choices made, the
/// device showing B), then the finished state once the choices settle.
#[test]
fn calibrate_ab_renders() {
    use crate::foxglove::calibrate_ui::tests::{answer_toward, sample};
    use crate::foxglove::calibrate_ui::Step;
    use crate::foxglove::shell::Page;
    use foxglove_devices::bracket::{Answer, Pick};
    use foxglove_devices::oklab;
    let n = |img: &image::RgbaImage, c: [u8; 3]| img.pixels().filter(|p| p.0[..3] == c).count();
    let (v, mut w) = sample(Step::Grey, false);
    // Grey starts from the White match's warm light: the user picks the cooler A, then the
    // pinker B.
    w.choose(Answer::Closer(Pick::A));
    w.choose(Answer::Closer(Pick::B));
    w.showing = Pick::B;
    let (a, b) = w.ask.as_ref().and_then(|b| b.pair()).unwrap();
    let mut s = Samples::full();
    s.calibrate = Some((v.clone(), w.clone()));
    let step = shell_preview_with(Page::Lights, &s, 1180.0);
    keep("ui-calibrate-ab-step.png", &step);
    assert!(
        n(&step, a) > 1500 && n(&step, b) > 1500,
        "A and B both drawn"
    );
    let grey = oklab::from_srgb(Step::Grey.color());
    answer_toward(&mut w, oklab::to_srgb([grey[0], grey[1], grey[2] + 0.04]));
    assert!(w.ask.is_none() && w.finished.is_some());
    let light = w.current.light;
    s.calibrate = Some((v, w));
    let done = shell_preview_with(Page::Lights, &s, 1180.0);
    keep("ui-calibrate-ab-done.png", &done);
    assert!(
        n(&done, light) > 3000,
        "the sent swatch draws the match {light:?}"
    );
    assert_ne!(step, done);
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

/// #691: the Lights page with Dynamic Lighting devices driven like any light: the mouse's two
/// lamps (logo, accent) on the user's desk, and the second desk's 12-lamp lightbar while
/// Windows keeps it from Foxglove. Review images `ui-691-lights-desk.png` and
/// `ui-691-lights-second-desk.png`.
#[test]
fn lamp_array_cards_render() {
    use crate::foxglove::shell::Page;
    let mut second = Samples::full();
    second.pc = crate::foxglove::pc_lights_ui::tests::sample_second_desk();
    let desk = shell_preview_with(Page::Lights, &Samples::full(), 1900.0);
    let other = shell_preview_with(Page::Lights, &second, 1900.0);
    keep("ui-691-lights-desk.png", &desk);
    keep("ui-691-lights-second-desk.png", &other);
    assert_ne!(desk, other);
}

/// #505: the Keyboard page's Batteries card. Review image `ui-505-hidpp-batteries.png` is the
/// default: "Read Logitech batteries" off (unticked), no HID++ rows. The "on" example
/// `ui-505-hidpp-batteries-on-example.png` has it ticked, a mouse read over HID++ among the
/// other devices.
#[test]
fn hidpp_batteries_render() {
    use crate::foxglove::shell::Page;
    let defaults = Samples::full();
    assert!(!defaults.keyboard.logitech, "the default sample has it off");
    let mut on = Samples::full();
    on.keyboard.logitech = true;
    on.keyboard.devices = crate::foxglove::peripherals::tests::sample_hidpp();
    let on = shell_preview_with(Page::Keyboard, &on, 1300.0);
    let off = shell_preview_with(Page::Keyboard, &defaults, 1300.0);
    keep("ui-505-hidpp-batteries.png", &off);
    keep("ui-505-hidpp-batteries-on-example.png", &on);
    assert_ne!(on, off);
}

/// #471: the Lights page with "Use OpenRGB's devices" on: a memory module (a picker per LED)
/// and a 40-LED fan hub (one All LEDs row) through OpenRGB, the host and port under the
/// toggle. Review image `ui-471-openrgb-lights.png`; `ui-471-openrgb-off.png` is the toggle off.
#[test]
fn openrgb_cards_render() {
    use crate::foxglove::shell::Page;
    let mut on = Samples::full();
    on.pc = crate::foxglove::pc_lights_ui::tests::sample_openrgb();
    let mut off = Samples::full();
    off.pc = crate::foxglove::pc_lights_ui::tests::sample_second_desk();
    let on = shell_preview_with(Page::Lights, &on, 2300.0);
    let off = shell_preview_with(Page::Lights, &off, 1900.0);
    keep("ui-471-openrgb-lights.png", &on);
    keep("ui-471-openrgb-off.png", &off);
    assert_ne!(on, off);
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

/// The other devices' batteries (#504): the Keyboard page's Batteries card with a mouse and a
/// low controller, the same card with none, and the tray panel's Devices part.
#[test]
fn peripheral_batteries_render() {
    use crate::foxglove::shell::Page;
    use crate::foxglove::theme::DARK;
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    let with = shell_preview_with(Page::Keyboard, &Samples::full(), 1300.0);
    keep("ui-batteries-1-keyboard.png", &with);
    let mut none = Samples::full();
    none.keyboard.devices.clear();
    let without = shell_preview_with(Page::Keyboard, &none, 1300.0);
    keep("ui-batteries-2-none.png", &without);
    // The low controller's level is amber.
    assert!(count(&with, DARK.warn) > count(&without, DARK.warn));

    let tray = |v: &crate::foxglove::tray_panel::View| {
        let ctx = preview_ctx();
        let size = egui::vec2(crate::foxglove::tray_panel::WIDTH + 40.0, 820.0);
        render(&ctx, size, Color32::from_rgb(0x09, 0x0a, 0x0c), |ctx| {
            egui::Area::new(egui::Id::new("tray-panel-preview"))
                .fixed_pos(egui::pos2(20.0, 20.0))
                .show(ctx, |ui| {
                    crate::foxglove::tray_panel::panel(
                        ui,
                        v,
                        &mut Default::default(),
                        &mut Vec::new(),
                    );
                });
        })
    };
    let v = crate::foxglove::tray_panel::tests::sample();
    let img = tray(&v);
    keep("ui-batteries-3-tray.png", &img);
    let bare = tray(&crate::foxglove::tray_panel::View {
        devices: Vec::new(),
        ..v
    });
    assert_ne!(img, bare, "the Devices part shows only with a device");
    assert!(count(&img, DARK.warn) > count(&bare, DARK.warn));
}

/// A 1-bit panel as the keyboard gets it, enlarged 6×, one pixel a square.
fn panel_image(p: &foxglove_panel::render::Panel) -> image::RgbaImage {
    let scale = 6;
    let (w, h) = (p.width as u32, p.height as u32);
    let mut img = image::RgbaImage::from_pixel(
        w * scale + 24,
        h * scale + 24,
        image::Rgba([0x09, 0x0a, 0x0c, 0xff]),
    );
    for y in 0..p.height {
        for x in 0..p.width {
            let on = p.bits[y * p.stride + x / 8] & (0x80 >> (x % 8)) != 0;
            let c = if on {
                [0xe8, 0xea, 0xe9, 0xff]
            } else {
                [0, 0, 0, 0xff]
            };
            for dy in 0..scale {
                for dx in 0..scale {
                    img.put_pixel(
                        12 + x as u32 * scale + dx,
                        12 + y as u32 * scale + dy,
                        image::Rgba(c),
                    );
                }
            }
        }
    }
    img
}

/// Notifications on the left screen (#507): the app's name alone (the default), an app whose
/// text the user allowed, and the Screens page's Notifications card.
#[test]
fn left_screen_notify_render() {
    use crate::foxglove::notifications::{NotifySettings, State};
    use foxglove_panel::render::{draw_notice, Notice, Panel};
    let panel = |n: &Notice| {
        let mut p = Panel::new(64, 52);
        draw_notice(&mut p, n);
        (panel_image(&p), p.bits)
    };
    let (name, name_bits) = panel(&Notice {
        app: "Microsoft Teams".into(),
        more: 2,
        ..Notice::default()
    });
    keep("left-screen-notify-1-name.png", &name);
    let (text, text_bits) = panel(&Notice {
        app: "Discord".into(),
        title: Some("Raid tonight".into()),
        body: Some("Bring the good keyboard and a charger for the right half".into()),
        more: 0,
    });
    keep("left-screen-notify-2-text.png", &text);
    assert_ne!(name_bits, text_bits);

    let ctx = preview_ctx();
    let mut settings = NotifySettings {
        enabled: true,
        light_cue: true,
        ..NotifySettings::default()
    };
    settings.text_apps.insert("Discord".into());
    let recent = [
        "Microsoft Teams".to_string(),
        "Discord".into(),
        "Outlook".into(),
    ];
    let img = render_ui(
        &ctx,
        egui::vec2(460.0, 420.0),
        Color32::from_rgb(0x09, 0x0a, 0x0c),
        |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.label(crate::foxglove::theme::heading(ui, "Notifications"));
                    ui.add_space(6.0);
                    crate::foxglove::notifications_ui::contents(
                        ui,
                        &mut settings,
                        &State::Listening { by_event: false },
                        &recent,
                    );
                });
        },
    );
    keep("left-screen-notify-3-settings.png", &img);

    // Without access: the one hint and its button.
    let ctx = preview_ctx();
    let mut settings = NotifySettings {
        enabled: true,
        ..NotifySettings::default()
    };
    let img = render_ui(
        &ctx,
        egui::vec2(460.0, 300.0),
        Color32::from_rgb(0x09, 0x0a, 0x0c),
        |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.label(crate::foxglove::theme::heading(ui, "Notifications"));
                    ui.add_space(6.0);
                    crate::foxglove::notifications_ui::contents(
                        ui,
                        &mut settings,
                        &State::NoAccess,
                        &[],
                    );
                });
        },
    );
    keep("left-screen-notify-4-no-access.png", &img);
}

/// The PC load layout (#506): the 1-bit panel as the keyboard gets it (enlarged 6×, one pixel a
/// square), its first second (values not known yet), and the Change… list with it chosen.
#[test]
fn left_screen_stats_render() {
    use foxglove_panel::render::{draw_stats, Panel};
    use foxglove_panel::stats::Stats;
    let panel = |stats: &Stats| {
        let mut p = Panel::new(64, 52);
        draw_stats(&mut p, stats);
        (panel_image(&p), p.bits)
    };
    let (shown, bits) = panel(&crate::foxglove::screens_ui::sample_stats());
    keep("left-screen-stats-1-panel.png", &shown);
    let (starting, empty_bits) = panel(&Stats::default());
    keep("left-screen-stats-2-first-second.png", &starting);
    assert_ne!(bits, empty_bits);

    let ctx = preview_ctx();
    let img = render_ui(
        &ctx,
        egui::vec2(460.0, 440.0),
        Color32::from_rgb(0x09, 0x0a, 0x0c),
        |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.label(crate::foxglove::theme::heading(ui, "Info screen: Change…"));
                    ui.add_space(6.0);
                    crate::foxglove::screens_ui::left_choices(
                        ui,
                        crate::foxglove::screens_ui::LeftChoice::Stats,
                    );
                });
        },
    );
    keep("left-screen-stats-3-picker.png", &img);
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

/// The App page's Keyboard health card (#705): a keyboard with a failing right screen and a slow
/// relay, and no keyboard.
#[test]
fn health_card_renders() {
    use crate::foxglove::theme::DARK;
    // A context each: render_ui's texture map starts empty, so the font atlas must too.
    let card = |view: crate::foxglove::health::View| {
        render_ui(
            &preview_ctx(),
            egui::vec2(640.0, 440.0),
            DARK.ground,
            move |ui| {
                egui::Frame::new()
                    .fill(DARK.ground)
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        crate::foxglove::health::draw(ui, &view);
                    });
            },
        )
    };
    let with = card(crate::foxglove::health::tests::sample());
    let without = card(crate::foxglove::health::tests::sample_none());
    keep("ui-705-health.png", &with);
    keep("ui-705-health-none.png", &without);
    let count = |img: &image::RgbaImage, c: Color32| {
        img.pixels()
            .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
            .count()
    };
    assert!(count(&with, DARK.danger) > 20, "a Failed label");
    assert!(count(&with, DARK.warn) > 20, "a Warning label");
    assert_eq!(count(&without, DARK.danger), 0);
}

/// The settings' "Other keyboards (VIA)" group (#74): off (the default), and on with generic
/// discovery and one board per definition source.
#[test]
fn via_settings_render() {
    use crate::foxglove::theme::DARK;
    use crate::foxglove::via_host::{tests::sample_found, ViaSettings};
    let group = |s: ViaSettings, found: Vec<crate::foxglove::via_host::Found>| {
        render_ui(
            &preview_ctx(),
            egui::vec2(760.0, 520.0),
            DARK.ground,
            move |ui| {
                egui::Frame::new()
                    .fill(DARK.ground)
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        crate::foxglove::via_ui::preview(ui, s.clone(), &found);
                    });
            },
        )
    };
    let off = group(ViaSettings::default(), vec![]);
    let on = group(
        ViaSettings {
            enabled: true,
            generic: true,
            ..Default::default()
        },
        sample_found(),
    );
    keep("ui-74-via-off.png", &off);
    keep("ui-74-via-on.png", &on);
    let lit = |img: &image::RgbaImage| {
        let g = [DARK.ground.r(), DARK.ground.g(), DARK.ground.b()];
        img.pixels().filter(|p| p.0[..3] != g).count()
    };
    assert!(lit(&on) > lit(&off), "the boards add lines");
}

/// The ⓘ text the disclosure preview pins open ("Right follows left"); the tooltip test
/// lists it.
pub(super) const PINNED: &str =
    "On: the right half's lights do what the left half's do. Saved on the keyboard.";

/// The disclosure widgets (#793), dark and light: flat status chips with and without a dot, the
/// one clickable chip (outlined), the ⓘ after controls (one shown as Enter shows it), the info
/// bars and the expanders, closed and open.
#[test]
fn disclosure_widgets_render() {
    use crate::foxglove::settings_ui::{expander, expander_with, set_expander_open};
    use crate::foxglove::theme::{self, Choice, Dot, Level, DARK, LIGHT};
    let sheet = |choice: Choice| {
        let ctx = preview_ctx();
        theme::apply_choice(&ctx, choice);
        let p = if choice == Choice::Dark {
            *DARK
        } else {
            *LIGHT
        };
        set_expander_open(&ctx, "preview-adjust", true);
        theme::PREVIEW_PINNED.set(Some(PINNED.to_owned()));
        let img = render_ui(&ctx, egui::vec2(640.0, 600.0), p.ground, move |ui| {
            egui::Frame::new()
                .fill(p.ground)
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    theme::card(ui)
                        .inner_margin(egui::Margin::symmetric(18, 14))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.label(theme::heading(ui, "Disclosure widgets"));
                            ui.add_space(8.0);
                            ui.label(theme::caps_label(ui, "Status chips · read-only, flat"));
                            ui.horizontal_wrapped(|ui| {
                                theme::hint_chip(ui, "6 zones per half", Some("6 lighting zones per half, by column"), None);
                                theme::hint_chip(ui, "Lights now: Foxglove", None, Some(Dot::Ok));
                                theme::hint_chip(ui, "in front only", None, Some(Dot::Muted));
                                theme::hint_chip(ui, "off in Windows", None, Some(Dot::Warn));
                            });
                            ui.add_space(8.0);
                            ui.label(theme::caps_label(ui, "Clickable chip · outlined"));
                            ui.horizontal(|ui| {
                                theme::click_chip(ui, "Corne · focus", Some("Pick the keyboard the pages show"), Some(Dot::Ok));
                                ui.label(egui::RichText::new("beside a pill:").color(p.muted));
                                theme::pill(ui, true, "Solid");
                            });
                            ui.add_space(10.0);
                            ui.label(theme::caps_label(ui, "Info bars · fill only, never collapsed"));
                            theme::info_bar(
                                ui,
                                Level::Warn,
                                "Windows has the keyboard's lights · Aurora waits",
                                "Windows Dynamic Lighting holds the keyboard's lights.",
                                Some("Open Dynamic Lighting settings"),
                            );
                            ui.add_space(4.0);
                            theme::info_bar(ui, Level::Danger, "The keyboard didn't answer", "No reply in 2 s.", Some("Retry"));
                            ui.add_space(4.0);
                            theme::info_bar(ui, Level::Info, "Needs an RGB key in the keymap", "The controls need an RGB key (&rgb_ug) somewhere in the keymap.", None);
                            ui.add_space(10.0);
                            ui.label(theme::caps_label(ui, "Expanders · closed by default"));
                            expander_with(
                                ui,
                                "preview-more",
                                "More sources",
                                "OpenRGB: on",
                                |ui| {
                                    theme::hint_chip(ui, "OpenRGB not connected", None, Some(Dot::Warn));
                                },
                                |ui| ui.label("Use OpenRGB's devices"),
                            );
                            expander(ui, "preview-adjust", "Adjust levels…", "100% · 80% · 12%", |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("On battery");
                                    ui.add(egui::Slider::new(&mut 80.0, 0.0..=100.0).suffix("%"));
                                });
                            });
                            ui.add_space(10.0);
                            ui.label(theme::caps_label(ui, "Info dot on controls"));
                            ui.horizontal(|ui| {
                                let c = ui.checkbox(&mut true, "Follow Windows accent");
                                theme::with_info(ui, c, "Follow the Windows accent colour: the underglow takes the accent colour's hue.");
                            });
                            ui.horizontal(|ui| {
                                let c = ui.checkbox(&mut true, "Right follows left");
                                theme::with_info(ui, c, PINNED);
                                ui.label(egui::RichText::new("← focused, Enter pressed").color(p.muted));
                            });
                            ui.add_space(70.0);
                        });
                });
        });
        theme::PREVIEW_PINNED.set(None);
        img
    };
    let dark = sheet(Choice::Dark);
    let light = sheet(Choice::Light);
    keep("ui-793-widgets-dark.png", &dark);
    keep("ui-793-widgets-light.png", &light);
    assert_ne!(dark, light);
}

/// The Lights page's Keyboard card alone (#794), `width` points wide, in `choice`, with the
/// owner line `owner` and the battery dim `dim`; and the card's height in points.
fn keyboard_card(
    choice: crate::foxglove::theme::Choice,
    lights: &crate::foxglove::lights_page::View,
    owner: Option<crate::foxglove::lights_owner::Line>,
    dim: crate::foxglove::battery_dim_ui::DimState,
    width: f32,
) -> (image::RgbaImage, f32) {
    use crate::foxglove::theme::{self, Choice, DARK, LIGHT};
    let ctx = preview_ctx();
    theme::apply_choice(&ctx, choice);
    let p = if choice == Choice::Dark {
        *DARK
    } else {
        *LIGHT
    };
    crate::foxglove::lights_owner::PREVIEW.set(owner);
    crate::foxglove::battery_dim_ui::PREVIEW.set(Some((dim, Default::default())));
    let height = std::cell::Cell::new(0.0f32);
    let pad = 16.0;
    let img = render_ui(
        &ctx,
        egui::vec2(width + 2.0 * pad, 1100.0),
        p.ground,
        |ui| {
            egui::Frame::new()
                .fill(p.ground)
                .inner_margin(egui::Margin::same(pad as i8))
                .show(ui, |ui| {
                    ui.set_width(width);
                    let mut draft = crate::foxglove::lights_page::Draft::default();
                    draft.settle(lights.state.as_ref());
                    let r = ui
                        .scope(|ui| {
                            crate::foxglove::lights_page::draw(ui, lights, &mut draft);
                        })
                        .response
                        .rect;
                    // The card, without the gap the page leaves under it.
                    height.set(r.height() - 10.0);
                });
        },
    );
    crate::foxglove::lights_owner::PREVIEW.set(None);
    crate::foxglove::battery_dim_ui::PREVIEW.set(None);
    let h = height.get();
    let px = |v: f32| (v * PPP).round() as u32;
    let crop =
        image::imageops::crop_imm(&img, 0, 0, img.width(), px(h + 2.0 * pad).min(img.height()))
            .to_image();
    (crop, h)
}

/// #794: the Keyboard card at 980 points, dark and light, with Foxglove driving the lights;
/// Windows Dynamic Lighting holding them (a scene waiting) and Foxglove drawing through it
/// (amber, in one short bar); zones unknown; and older firmware (the follow and battery
/// switches greyed with a short chip).
#[test]
fn lights_keyboard_card_1105_renders() {
    use crate::foxglove::battery_dim_ui::{tests as dim, DimState};
    use crate::foxglove::keyboard_settings::RgbSync;
    use crate::foxglove::lights_owner::{line, line_through, Who};
    use crate::foxglove::lights_page::tests::{sample, sample_zones_unknown};
    use crate::foxglove::theme::Choice;
    let corne = Some("Eyelash Corne");
    let foxglove = || Some(line(&Who::Foxglove, None, corne));
    let width = 980.0;
    let (dark, h_dark) = keyboard_card(Choice::Dark, &sample(), foxglove(), dim::sample(), width);
    let (light, _) = keyboard_card(Choice::Light, &sample(), foxglove(), dim::sample(), width);
    let windows_line = Some(line(&Who::Windows, Some("Bandit scanline"), corne));
    let (windows, h_windows) = keyboard_card(
        Choice::Dark,
        &sample(),
        windows_line.clone(),
        dim::sample(),
        width,
    );
    let (windows_light, _) =
        keyboard_card(Choice::Light, &sample(), windows_line, dim::sample(), width);
    let through_line = Some(line_through(&Who::Windows, None, corne, Some(false)));
    let (through, _) = keyboard_card(Choice::Dark, &sample(), through_line, dim::sample(), width);
    let (unknown, _) = keyboard_card(
        Choice::Dark,
        &sample_zones_unknown(),
        foxglove(),
        dim::sample(),
        width,
    );
    let old = crate::foxglove::lights_page::View {
        sync: RgbSync::NotSupported,
        ..sample()
    };
    let (old_firmware, _) = keyboard_card(
        Choice::Dark,
        &old,
        foxglove(),
        DimState::NotSupported,
        width,
    );
    eprintln!("Foxglove: #794 Keyboard card at {width} pt: {h_dark} pt tall, {h_windows} pt with Windows holding the lights");
    keep("ui-1105-lights-keyboard.png", &dark);
    keep("ui-1105-lights-keyboard-light.png", &light);
    keep("ui-1105-lights-keyboard-windows.png", &windows);
    keep("ui-1105-lights-keyboard-windows-light.png", &windows_light);
    keep("ui-1105-lights-keyboard-through.png", &through);
    keep("ui-1105-lights-keyboard-unknown-zones.png", &unknown);
    keep("ui-1105-lights-keyboard-old-firmware.png", &old_firmware);
    assert_ne!(dark, light);
    assert_ne!(dark, windows);
    // Shorter than before the declutter, measured at 980 points on origin/main cf7d6a90's card
    // with the same samples: 323.1 points, 362.8 with Windows holding the lights.
    assert!(
        h_dark < 323.0,
        "the card is {h_dark} points tall, not shorter"
    );
    assert!(
        h_windows < 362.0,
        "with Windows: {h_windows} points, not shorter"
    );
    // Amber stays on screen while Windows holds the lights, drawn through it or not.
    let near_warn = |img: &image::RgbaImage| {
        let c = crate::foxglove::theme::DARK.warn;
        img.pixels()
            .filter(|p| {
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
                d(p.0[0], c.r()) + d(p.0[1], c.g()) + d(p.0[2], c.b()) < 24
            })
            .count()
    };
    assert!(near_warn(&windows) > near_warn(&dark) + 20, "the amber dot");
    assert!(near_warn(&through) > near_warn(&dark) + 20, "the amber dot");
}

/// The Lights page's Other devices card alone (#795), `width` points wide, in `choice`, with
/// the expanders `open` open; and the card's height in points.
fn other_devices_card(
    choice: crate::foxglove::theme::Choice,
    pc: &crate::foxglove::pc_lights_ui::View,
    open: &[String],
    width: f32,
) -> (image::RgbaImage, f32) {
    use crate::foxglove::pc_lights_add::Wizard;
    use crate::foxglove::pc_lights_ui::{draw, Draft};
    use crate::foxglove::theme::{self, Choice, DARK, LIGHT};
    let ctx = preview_ctx();
    theme::apply_choice(&ctx, choice);
    for key in open {
        crate::foxglove::settings_ui::set_expander_open(&ctx, key, true);
    }
    let p = if choice == Choice::Dark {
        *DARK
    } else {
        *LIGHT
    };
    let height = std::cell::Cell::new(0.0f32);
    let pad = 16.0;
    let img = render_ui(
        &ctx,
        egui::vec2(width + 2.0 * pad, 2400.0),
        p.ground,
        |ui| {
            egui::Frame::new()
                .fill(p.ground)
                .inner_margin(egui::Margin::same(pad as i8))
                .show(ui, |ui| {
                    ui.set_width(width);
                    let r = ui
                        .scope(|ui| {
                            draw(ui, pc, &mut Draft::default(), &mut Wizard::default());
                        })
                        .response
                        .rect;
                    height.set(r.height());
                });
        },
    );
    let h = height.get();
    let px = |v: f32| (v * PPP).round() as u32;
    let crop =
        image::imageops::crop_imm(&img, 0, 0, img.width(), px(h + 2.0 * pad).min(img.height()))
            .to_image();
    (crop, h)
}

/// A3b (#795 follow-up): the worker's last word next to what was clicked, not a loose line at
/// the card's foot. A confirmation is a fading chip on the row (here the case's "Colours
/// set"), a problem an amber bar under the row, a note a muted bar; the card-wide one (a
/// catalog file that can't be saved) at the card's top, Add a device's on More sources, the keyboard card's own on the keyboard card.
#[test]
fn lights_results_a3b_render() {
    use crate::foxglove::pc_lights::{Place, Status, Tone};
    use crate::foxglove::pc_lights_ui::tests::sample;
    use crate::foxglove::theme::Choice;
    let width = 980.0;
    let none: Vec<String> = Vec::new();
    let with = |place: Place, tone: Tone, text: &str| {
        // That result alone (the sample's own chip on the case's row aside).
        let mut v = sample();
        v.pc.results.clear();
        v.pc.said(Status::new(place, tone, text));
        v
    };
    let case = Place::Row("103c:84fd".into());
    let mouse = Place::Row(sample().pc.devices[2].key.clone());
    let (done, _) = other_devices_card(Choice::Dark, &sample(), &none, width);
    let (done_light, _) = other_devices_card(Choice::Light, &sample(), &none, width);
    let problem = with(
        case.clone(),
        Tone::Problem,
        "HP OMEN 45L case lights: not connected",
    );
    let (problem, _) = other_devices_card(Choice::Dark, &problem, &none, width);
    let note = with(
        mouse,
        Tone::Note,
        "Match keyboard: the keyboard plays Swirl, which G502 Proteus Spectrum can't match \
         from Foxglove, so it keeps its colours",
    );
    let (note, _) = other_devices_card(Choice::Dark, &note, &none, width);
    // The only card-wide result: your own catalog file can't be saved (a pause goes where it
    // was clicked).
    let card = with(
        Place::Card,
        Tone::Problem,
        r"Can't save C:\Users\you\AppData\Roaming\Foxglove\lighting-catalog.local.toml: Access is denied. (os error 5)",
    );
    let (card, _) = other_devices_card(Choice::Dark, &card, &none, width);
    let sources = with(
        Place::Sources,
        Tone::Done,
        "Desk strip: saved as your own device",
    );
    let (sources, _) = other_devices_card(Choice::Dark, &sources, &none, width);
    // The keyboard card's own pick drawn through Windows (#749 b).
    let through = with(
        Place::Keyboard,
        Tone::Done,
        "Eyelash Corne: shown through Windows Dynamic Lighting",
    );
    crate::foxglove::pc_lights_ui::PREVIEW.set(Some((through, Default::default())));
    let corne = Some("Eyelash Corne");
    let owner = Some(crate::foxglove::lights_owner::line(
        &crate::foxglove::lights_owner::Who::Foxglove,
        None,
        corne,
    ));
    let (keyboard, _) = keyboard_card(
        Choice::Dark,
        &crate::foxglove::lights_page::tests::sample(),
        owner,
        crate::foxglove::battery_dim_ui::tests::sample(),
        width,
    );
    crate::foxglove::pc_lights_ui::PREVIEW.set(None);
    keep("ui-a3b-row-done.png", &done);
    keep("ui-a3b-row-done-light.png", &done_light);
    keep("ui-a3b-row-problem.png", &problem);
    keep("ui-a3b-row-note.png", &note);
    keep("ui-a3b-card-problem.png", &card);
    keep("ui-a3b-sources-done.png", &sources);
    keep("ui-a3b-keyboard-done.png", &keyboard);
    assert_ne!(done, problem);
    assert_ne!(problem, note);
}

/// #795: the Other devices card at 980 points with 1, 3 (a vendor app running: the conflict)
/// and 7 devices, OpenRGB on and not answering (amber), dark and light; and the 45L (a device
/// with zones) and More sources opened.
#[test]
fn lights_other_devices_1105_renders() {
    use crate::foxglove::pc_lights_ui::tests::{
        sample, sample_one, sample_openrgb_down, sample_seven,
    };
    use crate::foxglove::theme::Choice;
    let width = 980.0;
    let none: Vec<String> = Vec::new();
    let (one, h_one) = other_devices_card(Choice::Dark, &sample_one(), &none, width);
    let (one_light, _) = other_devices_card(Choice::Light, &sample_one(), &none, width);
    let (three, h_three) = other_devices_card(Choice::Dark, &sample(), &none, width);
    let (three_light, _) = other_devices_card(Choice::Light, &sample(), &none, width);
    let (seven, h_seven) = other_devices_card(Choice::Dark, &sample_seven(), &none, width);
    let (down, h_down) = other_devices_card(Choice::Dark, &sample_openrgb_down(), &none, width);
    let (down_light, _) = other_devices_card(Choice::Light, &sample_openrgb_down(), &none, width);
    let open = vec![
        format!("pc-device:{}", sample().pc.devices[0].key),
        "pc-more-sources".to_string(),
    ];
    let (expanded, h_expanded) = other_devices_card(Choice::Dark, &sample(), &open, width);
    let (expanded_light, _) = other_devices_card(Choice::Light, &sample(), &open, width);
    eprintln!(
        "Foxglove: #795 Other devices at {width} pt: 1 device {h_one} pt, 3 devices and a \
         conflict {h_three} pt, 7 devices {h_seven} pt, OpenRGB down {h_down} pt, the 45L and \
         More sources open {h_expanded} pt"
    );
    keep("ui-1105-lights-other-1.png", &one);
    keep("ui-1105-lights-other-1-light.png", &one_light);
    keep("ui-1105-lights-other-3.png", &three);
    keep("ui-1105-lights-other-3-light.png", &three_light);
    keep("ui-1105-lights-other-7.png", &seven);
    keep("ui-1105-lights-other-openrgb-down.png", &down);
    keep("ui-1105-lights-other-openrgb-down-light.png", &down_light);
    keep("ui-1105-lights-other-expanded.png", &expanded);
    keep("ui-1105-lights-other-expanded-light.png", &expanded_light);
    assert_ne!(one, one_light);
    assert_ne!(three, expanded);
    // Shorter than before the declutter, measured at 980 points on the A2 base (509a2375) with
    // the same samples: 364.3 points with 1 device, 703.6 with 3 and a conflict, 1452.7 with 7,
    // 619.8 with OpenRGB down.
    for (what, h, before) in [
        ("1 device", h_one, 364.0),
        ("3 devices", h_three, 703.0),
        ("7 devices", h_seven, 1452.0),
        ("OpenRGB down", h_down, 619.0),
    ] {
        assert!(h < before, "{what}: {h} points, not shorter than {before}");
    }
    // Amber stays on screen: the conflict bar, and OpenRGB's chip on the closed More sources.
    let near_warn = |img: &image::RgbaImage| {
        let c = crate::foxglove::theme::DARK.warn;
        img.pixels()
            .filter(|p| {
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
                d(p.0[0], c.r()) + d(p.0[1], c.g()) + d(p.0[2], c.b()) < 24
            })
            .count()
    };
    let (calm, _) = other_devices_card(
        Choice::Dark,
        &crate::foxglove::pc_lights_ui::tests::sample_second_desk(),
        &none,
        width,
    );
    assert!(
        near_warn(&down) > near_warn(&calm) + 20,
        "OpenRGB's amber dot"
    );
    assert!(
        near_warn(&three) > near_warn(&one) + 20,
        "the conflict's amber dot"
    );
}

/// Settings > Live wallpaper with "Show notifications on the wallpaper when it's on screen" and
/// its hover, in the dark and light themes.
#[test]
fn live_wallpaper_notify_row_renders() {
    use crate::foxglove::live_feed::{notify_row, EXPLAINER, NOTIFY_HOVER};
    use crate::foxglove::theme::{Choice, DARK, LIGHT};
    for (choice, name, ground) in [
        (Choice::Dark, "dark", DARK.ground),
        (Choice::Light, "light", LIGHT.ground),
    ] {
        let ctx = preview_ctx();
        crate::foxglove::theme::apply_choice(&ctx, choice);
        let img = render(&ctx, egui::vec2(760.0, 420.0), ground, |ctx| {
            egui::Area::new(egui::Id::new("notify-settings"))
                .fixed_pos(egui::pos2(20.0, 14.0))
                .show(ctx, |ui| {
                ui.set_width(720.0);
                crate::foxglove::settings_ui::section(ui, "Live wallpaper", |ui| {
                    ui.checkbox(&mut true, "Live keyboard feed for wallpapers");
                    ui.weak(EXPLAINER);
                    ui.weak(
                        "Paste it into the wallpaper's \"Foxglove pairing token\" in Wallpaper Engine.",
                    );
                    let row = notify_row(ui, &mut true);
                    ui.weak("Listening on 127.0.0.1:8768: 1 wallpaper connected");
                    // The hover, drawn where egui puts it.
                    egui::Area::new(egui::Id::new("notify-hover"))
                        .fixed_pos(row.rect.left_bottom() + egui::vec2(24.0, 30.0))
                        .order(egui::Order::Tooltip)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.label(NOTIFY_HOVER);
                            });
                        });
                });
            });
        });
        keep(&format!("ui-notify-route-settings-{name}.png"), &img);
        let n = img
            .pixels()
            .filter(|p| p.0[..3] == [ground.r(), ground.g(), ground.b()])
            .count();
        assert!(n > 1000, "{name}: the theme's ground shows");
    }
}

/// #801: "Keyboard: Follow focus ▾" in the layer overlay's card and the Live wallpaper card, dark
/// and light: both following the focus (the Corne), the overlay's list open, and the overlay
/// pinned to the Corne while the focus is on the K5 Pro (the feed, following, says it gets the
/// K5 Pro's name and shape only).
#[test]
fn focus_pins_render() {
    use crate::foxglove::focus_pins::{feed_row, overlay_row, PinView, PREVIEW, PREVIEW_OPEN};
    use crate::foxglove::keyboard_focus::View;
    use crate::foxglove::live_feed::EXPLAINER;
    use crate::foxglove::settings_ui::{grid_heading, section};
    use crate::foxglove::theme::{Choice, DARK, LIGHT};
    let keyboards = vec![
        ("corne".to_string(), "Eyelash Corne".to_string(), true),
        ("k5".to_string(), "K5 Pro".to_string(), true),
    ];
    let row = |pinned: Option<&str>, limited: bool| PinView {
        pinned: pinned.map(String::from),
        keyboards: keyboards.clone(),
        limited,
    };
    let follow = (row(None, false), row(None, false));
    let pinned = (row(Some("corne"), false), row(None, true));
    let cards = |ui: &mut egui::Ui| {
        ui.set_width(720.0);
        section(ui, "Overlay Appearance", |ui| {
            egui::Grid::new("appearance_grid")
                .num_columns(2)
                .striped(true)
                .spacing([20.0, 10.0])
                .show(ui, |ui| {
                    overlay_row(ui);
                    grid_heading(ui, "When it shows");
                    ui.label("Display duration");
                    ui.label("2000 ms");
                    ui.end_row();
                });
        });
        section(ui, "Live wallpaper", |ui| {
            ui.checkbox(&mut true, "Live keyboard feed for wallpapers");
            ui.weak(EXPLAINER);
            ui.weak(
                "Paste it into the wallpaper's \"Foxglove pairing token\" in Wallpaper Engine.",
            );
            feed_row(ui);
            ui.weak("Listening on 127.0.0.1:8768: 1 wallpaper connected");
        });
    };
    for (choice, theme, ground) in [
        (Choice::Dark, "dark", DARK.ground),
        (Choice::Light, "light", LIGHT.ground),
    ] {
        let shot = |pins: &(PinView, PinView), open: Option<View>| {
            PREVIEW.with_borrow_mut(|p| *p = Some(pins.clone()));
            PREVIEW_OPEN.set(open);
            let ctx = preview_ctx();
            crate::foxglove::theme::apply_choice(&ctx, choice);
            let img = render(&ctx, egui::vec2(760.0, 520.0), ground, |ctx| {
                egui::Area::new(egui::Id::new("focus-pins"))
                    .fixed_pos(egui::pos2(20.0, 14.0))
                    .show(ctx, cards);
            });
            PREVIEW.with_borrow_mut(|p| *p = None);
            PREVIEW_OPEN.set(None);
            img
        };
        let follows = shot(&follow, None);
        let open = shot(&follow, Some(View::Overlay));
        let pin = shot(&pinned, None);
        assert_ne!(follows, open, "{theme}: the list");
        assert_ne!(follows, pin, "{theme}: the pin and the feed's note");
        keep(&format!("ui-801-pins-follow-{theme}.png"), &follows);
        keep(&format!("ui-801-pins-list-open-{theme}.png"), &open);
        keep(&format!("ui-801-pins-pinned-{theme}.png"), &pin);
    }
}

/// #801: the tray panel's title names the focused keyboard (a long name is cut short, its
/// whole name on hover), dark and light.
#[test]
fn tray_title_names_the_focus_renders() {
    use crate::foxglove::theme::Choice;
    for (choice, theme) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let shot = |v: &crate::foxglove::tray_panel::View| {
            let ctx = preview_ctx();
            crate::foxglove::theme::apply_choice(&ctx, choice);
            let size = egui::vec2(crate::foxglove::tray_panel::WIDTH + 40.0, 260.0);
            render(&ctx, size, Color32::from_rgb(0x09, 0x0a, 0x0c), |ctx| {
                egui::Area::new(egui::Id::new("tray-panel-preview"))
                    .fixed_pos(egui::pos2(20.0, 20.0))
                    .show(ctx, |ui| {
                        crate::foxglove::tray_panel::panel(
                            ui,
                            v,
                            &mut Default::default(),
                            &mut Vec::new(),
                        );
                    });
            })
        };
        let v = crate::foxglove::tray_panel::tests::sample();
        let corne = shot(&v);
        let long = shot(&crate::foxglove::tray_panel::View {
            keyboard: Some("Keychron K5 Pro ANSI Knob (wired)".into()),
            ..v.clone()
        });
        let none = shot(&crate::foxglove::tray_panel::View {
            keyboard: None,
            ..v
        });
        assert_ne!(corne, none, "{theme}: the name in the title");
        assert_ne!(corne, long);
        keep(&format!("ui-801-tray-title-{theme}.png"), &corne);
        keep(&format!("ui-801-tray-title-long-{theme}.png"), &long);
    }
}

/// #801: the Scenes room with two keyboards (the Corne and a K5 Pro beside it), the focused one
/// ringed: the Corne, then the K5 Pro.
#[test]
fn scenes_room_two_keyboards_renders() {
    use crate::foxglove::scene_room::{
        focused_ids,
        tests::{sample_room_two_keyboards, two_keyboards},
    };
    use crate::foxglove::shell::Page;
    let room = sample_room_two_keyboards();
    let [corne, k5] = two_keyboards().try_into().unwrap();
    let on = |focused: Vec<String>| {
        let mut s = Samples::full();
        s.scenes.0.room = room.clone();
        s.scenes.0.focused = focused;
        shell_preview_with(Page::Scenes, &s, 1000.0)
    };
    let plain = on(Vec::new());
    let on_corne = on(focused_ids(&room, Some(&corne)));
    let on_k5 = on(focused_ids(&room, Some(&k5)));
    assert_ne!(plain, on_corne, "the ring");
    assert_ne!(on_corne, on_k5, "the ring moves with the focus");
    keep("ui-801-scenes-room-corne.png", &on_corne);
    keep("ui-801-scenes-room-k5.png", &on_k5);
}
