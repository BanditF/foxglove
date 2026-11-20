// SPDX-License-Identifier: MIT
//! Review images for the PC sensors page (OMEN sensors slice 3b): each state of the module's
//! own vectors (`tests/hideout-vectors/modules/sensors.json`, written by the sensors tests)
//! through the real renderer, window-free. Writes PNGs to `HIDEOUT_REVIEW_DIR`; without it they
//! go to the temp folder. Skipped without a system font.
#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{epaint, Pos2, Rect, TextureId, Vec2};
use hideout_contract::panel::Register;
use hideout_panels::{panel_ui, tile_ui, PanelHost, PanelKey};
use serde_json::{json, Value};

const VECTORS: &str = include_str!("../../../tests/hideout-vectors/modules/sensors.json");

fn rasterise(w: u32, h: u32, draw: impl FnMut(&egui::Context)) -> Option<Vec<u8>> {
    rasterise_with(w, h, &[vec![], vec![], vec![]], draw)
}

/// The test fonts, with a monospace face (Consolas) when the machine has one, as the hub has.
fn fonts() -> Option<egui::FontDefinitions> {
    let mut fonts = hideout_panels::render::test_fonts()?;
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    if let Ok(bytes) = std::fs::read(format!("{windir}\\Fonts\\consola.ttf")) {
        fonts.font_data.insert(
            "mono".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts.families.insert(
            egui::FontFamily::Monospace,
            vec!["mono".into(), "test".into()],
        );
    }
    Some(fonts)
}

/// Draws one frame per entry of `frames`, each with those input events, and rasterises the last.
fn rasterise_with(
    w: u32,
    h: u32,
    frames: &[Vec<egui::Event>],
    mut draw: impl FnMut(&egui::Context),
) -> Option<Vec<u8>> {
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts()?);
    ctx.set_visuals(egui::Visuals::dark());
    // The tile sheet draws one module's tile in many hosts, so they share one widget id.
    ctx.options_mut(|o| o.warn_on_id_clash = false);
    ctx.style_mut(|s| s.scroll_animation = egui::style::ScrollAnimation::none());
    let mut textures: HashMap<TextureId, epaint::ColorImage> = HashMap::new();
    let mut last = None;
    for events in frames {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(w as f32, h as f32),
            )),
            events: events.clone(),
            ..Default::default()
        };
        let out = ctx.run(raw, &mut draw);
        for (id, delta) in &out.textures_delta.set {
            let epaint::ImageData::Color(img) = &delta.image;
            match delta.pos {
                None => {
                    textures.insert(*id, (**img).clone());
                }
                Some([x0, y0]) => {
                    if let Some(t) = textures.get_mut(id) {
                        for y in 0..img.size[1] {
                            for x in 0..img.size[0] {
                                t.pixels[(y0 + y) * t.size[0] + x0 + x] =
                                    img.pixels[y * img.size[0] + x];
                            }
                        }
                    }
                }
            }
        }
        last = Some(ctx.tessellate(out.shapes, out.pixels_per_point));
    }
    let bg = egui::Visuals::dark().panel_fill.to_array();
    let mut px = vec![
        [
            bg[0] as f32 / 255.0,
            bg[1] as f32 / 255.0,
            bg[2] as f32 / 255.0,
            1.0
        ];
        (w * h) as usize
    ];
    for prim in last? {
        let epaint::Primitive::Mesh(mesh) = prim.primitive else {
            continue;
        };
        let Some(tex) = textures.get(&mesh.texture_id) else {
            continue;
        };
        let clip = prim.clip_rect;
        for tri in mesh.indices.chunks(3) {
            let v = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let xs = v.iter().map(|v| v.pos.x);
            let ys = v.iter().map(|v| v.pos.y);
            let minx = xs
                .clone()
                .fold(f32::MAX, f32::min)
                .max(clip.left())
                .max(0.0);
            let maxx = xs.fold(f32::MIN, f32::max).min(clip.right()).min(w as f32);
            let miny = ys.clone().fold(f32::MAX, f32::min).max(clip.top()).max(0.0);
            let maxy = ys.fold(f32::MIN, f32::max).min(clip.bottom()).min(h as f32);
            let area = (v[1].pos.x - v[0].pos.x) * (v[2].pos.y - v[0].pos.y)
                - (v[2].pos.x - v[0].pos.x) * (v[1].pos.y - v[0].pos.y);
            if area.abs() < 1e-6 {
                continue;
            }
            let mut y = miny.floor();
            while y < maxy {
                let mut x = minx.floor();
                while x < maxx {
                    let (cx, cy) = (x + 0.5, y + 0.5);
                    let e = |a: &epaint::Vertex, b: &epaint::Vertex| {
                        (b.pos.x - a.pos.x) * (cy - a.pos.y) - (cx - a.pos.x) * (b.pos.y - a.pos.y)
                    };
                    let (w0, w1, w2) = (
                        e(v[1], v[2]) / area,
                        e(v[2], v[0]) / area,
                        e(v[0], v[1]) / area,
                    );
                    if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                        let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                        let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                        let tx = ((u * tex.size[0] as f32) as usize).min(tex.size[0] - 1);
                        let ty = ((vv * tex.size[1] as f32) as usize).min(tex.size[1] - 1);
                        let t = tex.pixels[ty * tex.size[0] + tx].to_array();
                        let mut src = [0f32; 4];
                        for k in 0..4 {
                            let vc = w0 * v[0].color[k] as f32
                                + w1 * v[1].color[k] as f32
                                + w2 * v[2].color[k] as f32;
                            src[k] = vc / 255.0 * t[k] as f32 / 255.0;
                        }
                        let d = &mut px[(y as u32 * w + x as u32) as usize];
                        for k in 0..4 {
                            d[k] = src[k] + d[k] * (1.0 - src[3]);
                        }
                    }
                    x += 1.0;
                }
                y += 1.0;
            }
        }
    }
    Some(
        px.iter()
            .flat_map(|p| {
                [
                    (p[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (p[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (p[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                    255,
                ]
            })
            .collect(),
    )
}

fn save_png(name: &str, w: u32, h: u32, rgba: Vec<u8>) -> PathBuf {
    let dir = std::env::var_os("HIDEOUT_REVIEW_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("hideout-review"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    image::RgbaImage::from_raw(w, h, rgba)
        .unwrap()
        .save(&path)
        .unwrap();
    path
}

/// One state of the vectors as the hub holds it after `ui.register`.
fn host_for(state: &str) -> PanelHost {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    let st = doc["states"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["state"] == state)
        .unwrap_or_else(|| panic!("no vector state {state:?}"));
    let declared: Vec<String> = serde_json::from_value(doc["declared"].clone()).unwrap();
    let params: Register =
        serde_json::from_value(json!({"instance": "s1", "panels": st["panels"]})).unwrap();
    let mut host = PanelHost::default();
    let module = doc["module"].as_str().unwrap();
    let rej = host.register_with(module, "PC sensors", Some(&declared), Some(&[]), &params);
    assert!(rej.is_empty(), "{rej:?}");
    host
}

/// The states side by side, each in a column the width of the hub's page, captioned.
fn sheet(file: &str, states: &[&str]) {
    sheet_of(file, states, false)
}

/// The same for the tray tile (`tile`) or the page.
fn sheet_of(file: &str, states: &[&str], tile: bool) {
    let mut hosts: Vec<(&str, PanelHost)> = states.iter().map(|s| (*s, host_for(s))).collect();
    let module = serde_json::from_str::<Value>(VECTORS).unwrap()["module"]
        .as_str()
        .unwrap()
        .to_string();
    let col = if tile { 250.0f32 } else { 400.0f32 };
    let (w, h) = (
        (col as u32 + 12) * states.len() as u32 + 12,
        if tile { 200u32 } else { 800u32 },
    );
    let Some(px) = rasterise(w, h, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                for (cap, host) in hosts.iter_mut() {
                    ui.allocate_ui(Vec2::new(col, h as f32 - 20.0), |ui| {
                        ui.set_width(col);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(*cap).small().weak());
                            if tile {
                                let _ = tile_ui(host, ui, &PanelKey::new(&module, "tile"));
                            } else {
                                ui.heading("PC sensors");
                                let _ = panel_ui(host, ui, &PanelKey::new(&module, "page"));
                            }
                        });
                    });
                    ui.add_space(12.0);
                }
            });
        });
    }) else {
        eprintln!("no system font; review images skipped");
        return;
    };
    println!("{}", save_png(file, w, h, px).display());
}

#[test]
fn renders_the_sensors_page_for_review() {
    sheet(
        "ux-sensors3-page.png",
        &["Working", "Game mode on", "Reading"],
    );
    sheet(
        "ux-sensors3-edge.png",
        &[
            "Graphics card details missing",
            "Some details missing",
            "No sensors",
        ],
    );
    sheet(
        "ux-sensors3-more.png",
        &["Fan as percent", "Game mode not visible"],
    );
    sheet(
        "ux-sensors-temp.png",
        &["Working", "System temperature missing"],
    );
}

#[test]
fn renders_the_sensors_tile_for_review() {
    sheet_of(
        "ux-sensors3-tile.png",
        &[
            "Working",
            "Reading",
            "Graphics card details missing",
            "No sensors",
        ],
        true,
    );
}
