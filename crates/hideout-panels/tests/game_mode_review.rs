// SPDX-License-Identifier: MIT
//! Review images for #1234: Foxglove's Keyboard tile and page as the app's vectors give them
//! (game mode on the page only; the tile never changes for it), beside the AI tile's shipped
//! states, through the real renderer, window-free. Writes PNGs to `HIDEOUT_REVIEW_DIR`; without
//! it they go to the temp folder. Skipped without a system font.
#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use egui::{epaint, Pos2, Rect, TextureId, Vec2};
use hideout_contract::module::{Context, Module};
use hideout_contract::panel::Register;
use hideout_panels::{page_sections_ui, panel_ui, tile_ui_at, PanelHost, PanelKey};
use serde_json::{json, Value};

const VECTOR: &str = include_str!("vectors/vulpes.json");
const MANIFEST: &str = include_str!("vectors/vulpes.module.toml");
const FOX: &str = include_str!("../../../app/src/foxglove/hub_link/module.toml");
const FOX_VECTORS: &str = include_str!("../../../tests/hideout-vectors/modules/foxglove.json");

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

fn vector() -> Value {
    serde_json::from_str(VECTOR).unwrap()
}

fn state_named(name: &str) -> Value {
    vector()["states"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["state"] == name)
        .unwrap()
        .clone()
}

fn ai_host(state: Option<&str>) -> PanelHost {
    let mut host = PanelHost::default();
    host.set_manifest(Module::parse(MANIFEST, &Context::default()).unwrap());
    if let Some(name) = state {
        let v = vector();
        let s = state_named(name);
        host.set_fact("module.vulpes.state", json!("running"));
        let declared: Vec<String> = serde_json::from_value(v["declared"].clone()).unwrap();
        let actions: Vec<String> = serde_json::from_value(v["actions"].clone()).unwrap();
        let params: Register =
            serde_json::from_value(json!({"instance": "i1", "panels": s["panels"]})).unwrap();
        let rej = host.register_with("vulpes", "AI", Some(&declared), Some(&actions), &params);
        assert!(rej.is_empty(), "{rej:?}");
    }
    host
}

/// Foxglove's keyboard as the facts give it in a scenario: a state of the app's own vectors
/// (`tests/hideout-vectors/modules/foxglove.json`, written by `hub_panels`' tests), so the
/// images show what the app sends.
#[derive(Clone, Copy)]
enum Kb {
    Off,
    On,
    Away,
}

impl Kb {
    fn state(self) -> &'static str {
        match self {
            Kb::Off => "Game mode off",
            Kb::On => "Game mode on",
            Kb::Away => "Not connected",
        }
    }
}

fn fox_host(kb: Kb) -> PanelHost {
    let text = FOX
        .replace("@VERSION@", "0.2.0")
        .replace("@EXE@", "C:/x/foxglove.exe");
    let vectors: Value = serde_json::from_str(FOX_VECTORS).unwrap();
    let state = vectors["states"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["state"] == kb.state())
        .unwrap_or_else(|| panic!("no vector state {:?}", kb.state()));
    let mut host = PanelHost::default();
    host.set_manifest(Module::parse(&text, &Context::default()).expect("foxglove manifest"));
    host.set_fact("module.foxglove.state", json!("running"));
    let params: Register =
        serde_json::from_value(json!({"instance": "f1", "panels": state["panels"]})).unwrap();
    let declared: Vec<String> = serde_json::from_value(vectors["declared"].clone()).unwrap();
    let rej = host.register_with("foxglove", "Foxglove", Some(&declared), Some(&[]), &params);
    assert!(rej.is_empty(), "{rej:?}");
    host
}

#[test]
fn renders_the_game_mode_proposal_for_review() {
    let now = Instant::now();
    // Rows: (caption, keyboard, AI state or None for cold).
    let rows: Vec<(&str, Kb, Option<&str>)> = vec![
        (
            "1  game mode off, chat holds the GPU",
            Kb::Off,
            Some("held-chat"),
        ),
        (
            "2  game mode on, chat holds it: tile unchanged",
            Kb::On,
            Some("held-chat"),
        ),
        (
            "3  game mode on, user pressed Free GPU",
            Kb::On,
            Some("freeing"),
        ),
        (
            "4  game mode on, reserved for gaming",
            Kb::On,
            Some("reserved"),
        ),
        (
            "5  game over: game mode off, still reserved",
            Kb::Off,
            Some("reserved"),
        ),
        ("6  game mode on, AI not running (cold)", Kb::On, None),
        ("7  keyboard away, reserved", Kb::Away, Some("reserved")),
        (
            "8  game mode on, freeing failed",
            Kb::On,
            Some("free-failed"),
        ),
    ];
    let mut hosts: Vec<(&str, PanelHost, PanelHost)> = rows
        .into_iter()
        .map(|(c, kb, ai)| (c, fox_host(kb), ai_host(ai)))
        .collect();
    let (w, h) = (1060u32, hosts.len() as u32 * 205 + 20);
    let Some(px) = rasterise(w, h, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            for (cap, fox, ai) in hosts.iter_mut() {
                ui.horizontal(|ui| {
                    ui.allocate_ui(Vec2::new(250.0, 195.0), |ui| {
                        ui.set_width(250.0);
                        ui.label(egui::RichText::new(*cap).small().weak());
                    });
                    ui.allocate_ui(Vec2::new(380.0, 195.0), |ui| {
                        ui.set_width(380.0);
                        let _ = tile_ui_at(fox, ui, &PanelKey::new("foxglove", "keyboard"), now);
                    });
                    ui.allocate_ui(Vec2::new(380.0, 195.0), |ui| {
                        ui.set_width(380.0);
                        let _ = tile_ui_at(ai, ui, &PanelKey::new("vulpes", "tile"), now);
                    });
                });
                ui.add_space(4.0);
            }
        });
    }) else {
        eprintln!("no system font; review images skipped");
        return;
    };
    println!("{}", save_png("game-mode-tiles.png", w, h, px).display());

    for (name, kb) in [("on", Kb::On), ("off", Kb::Off), ("away", Kb::Away)] {
        let mut host = fox_host(kb);
        let (w, h) = (620u32, 520u32);
        let px = rasterise(w, h, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Foxglove");
                let key = PanelKey::new("foxglove", "page");
                panel_ui(&mut host, ui, &key);
                page_sections_ui(&mut host, ui, &key);
            });
        })
        .unwrap();
        println!(
            "{}",
            save_png(&format!("foxglove-page-game-{name}.png"), w, h, px).display()
        );
    }
}

/// Review images for the Wallpaper Engine allow list (S5b): the app's own vector states named
/// "Allow list: ...", the Foxglove page, light, at 720 px and at a narrow 380 px.
#[test]
fn renders_the_wallpaper_engine_section_for_review() {
    let vectors: Value = serde_json::from_str(FOX_VECTORS).unwrap();
    let text = FOX
        .replace("@VERSION@", "0.2.0")
        .replace("@EXE@", "C:/x/foxglove.exe");
    for (n, st) in vectors["states"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["state"].as_str().unwrap().starts_with("Allow list"))
        .enumerate()
    {
        let mut host = PanelHost::default();
        host.set_manifest(Module::parse(&text, &Context::default()).unwrap());
        host.set_fact("module.foxglove.state", json!("running"));
        let params: Register =
            serde_json::from_value(json!({"instance": "f1", "panels": st["panels"]})).unwrap();
        let declared: Vec<String> = serde_json::from_value(vectors["declared"].clone()).unwrap();
        let rej = host.register_with("foxglove", "Foxglove", Some(&declared), Some(&[]), &params);
        assert!(rej.is_empty(), "{rej:?}");
        for (size, w) in [("wide", 720u32), ("narrow", 380u32)] {
            let h = 1500u32;
            let Some(px) = rasterise(w, h, |ctx| {
                ctx.set_visuals(egui::Visuals::light());
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let key = PanelKey::new("foxglove", "page");
                        panel_ui(&mut host, ui, &key);
                        page_sections_ui(&mut host, ui, &key);
                    });
                });
            }) else {
                eprintln!("no system font; review images skipped");
                return;
            };
            println!(
                "{}",
                save_png(&format!("we-s5b-{:02}-{size}-light.png", n + 1), w, h, px).display()
            );
        }
    }
}
