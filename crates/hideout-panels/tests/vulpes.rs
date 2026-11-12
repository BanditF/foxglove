// SPDX-License-Identifier: MIT
//! The Vulpes (AI) page and tile through the real panel host and renderer, window-free (#1117,
//! H4.3 Hideout side). The trees are Arete's own golden vector (`vectors/vulpes.json`, vendored,
//! see `vectors/SOURCE.md`): every state registers, draws its tone, meter, button action and
//! sections, and a button press becomes `action.invoke` or `ui.event`. A cold on-demand module's
//! tile reads "Not running". The last test writes the review images to `HIDEOUT_REVIEW_DIR`.
#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use egui::{epaint, Pos2, Rect, TextureId, Vec2};
use hideout_contract::messages::{Badge, UiBadge, Visibility};
use hideout_contract::module::{Context, Module};
use hideout_contract::panel::Register;
use hideout_panels::tray::TILE_NOT_RUNNING;
use hideout_panels::{
    page_sections_ui, panel_ui, tile_ui_at, HubCall, PanelHost, PanelKey, PanelState,
};
use serde_json::{json, Value};

const VECTOR: &str = include_str!("vectors/vulpes.json");
const MANIFEST: &str = include_str!("vectors/vulpes.module.toml");
const M: &str = "vulpes";

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

fn manifest() -> Module {
    Module::parse(MANIFEST, &Context::default()).expect("Arete's manifest parses")
}

/// Every node of a vector tree, depth first.
fn nodes(tree: &Value) -> Vec<&Value> {
    let mut out = vec![tree];
    for c in tree["children"].as_array().into_iter().flatten() {
        out.extend(nodes(c));
    }
    out
}

fn first<'a>(tree: &'a Value, kind: &str) -> Option<&'a Value> {
    nodes(tree).into_iter().find(|n| n["type"] == kind)
}

fn panel<'a>(state: &'a Value, id: &str) -> &'a Value {
    state["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap()
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

fn register(host: &mut PanelHost, state: &Value) {
    let v = vector();
    let declared: Vec<String> = serde_json::from_value(v["declared"].clone()).unwrap();
    let actions: Vec<String> = serde_json::from_value(v["actions"].clone()).unwrap();
    let params: Register =
        serde_json::from_value(json!({"instance": "i1", "panels": state["panels"]})).unwrap();
    let rejected = host.register_with(M, "AI", Some(&declared), Some(&actions), &params);
    assert!(rejected.is_empty(), "{}: {rejected:?}", state["state"]);
}

/// A host as the hub has it for Vulpes: the manifest, running, then Arete's `ui.register`.
fn host_in(state: &Value) -> PanelHost {
    let mut host = PanelHost::default();
    host.set_manifest(manifest());
    host.set_fact("module.vulpes.state", json!("running"));
    register(&mut host, state);
    host
}

/// A host whose module never ran: the manifest only, no facts, nothing registered.
fn cold_host() -> PanelHost {
    let mut host = PanelHost::default();
    host.set_manifest(manifest());
    host
}

fn tile_key() -> PanelKey {
    PanelKey::new(M, "tile")
}

fn page_key() -> PanelKey {
    PanelKey::new(M, "page")
}

fn has_fonts() -> bool {
    hideout_panels::render::test_fonts().is_some()
}

fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_fonts(hideout_panels::render::test_fonts().unwrap());
    ctx
}

fn raw(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 600.0))),
        events,
        ..Default::default()
    }
}

fn draw_tile(
    ctx: &egui::Context,
    host: &mut PanelHost,
    events: Vec<egui::Event>,
) -> hideout_panels::tray::TileReport {
    let mut r = None;
    let _ = ctx.run(raw(events), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            r = Some(tile_ui_at(host, ui, &tile_key(), Instant::now()));
        });
    });
    r.unwrap()
}

/// Draws the tile of `host` until it settles, then presses the node `id` with the mouse.
fn press_in_tile(host: &mut PanelHost, id: &str) -> hideout_panels::tray::TileReport {
    let ctx = ctx();
    let mut first = draw_tile(&ctx, host, vec![]);
    for _ in 0..2 {
        first = draw_tile(&ctx, host, vec![]);
    }
    // The node scope spans the row; the button sits at its left edge.
    let r = first.draw.rects[id];
    let at = Pos2::new(r.left() + 24.0, r.center().y);
    let click = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    draw_tile(&ctx, host, vec![egui::Event::PointerMoved(at), click(true)]);
    draw_tile(&ctx, host, vec![click(false)])
}

#[test]
fn aretes_manifest_is_an_on_demand_adopted_module_for_hub_1_1() {
    let m = manifest();
    assert_eq!(m.id, M);
    assert_eq!(m.kind, hideout_contract::module::Kind::Adopted);
    assert_eq!(m.hub, "1.1");
    assert_eq!(m.activation_on, ["open", "call"]);
    let panels: Vec<&str> = m.panels.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(panels, ["tile", "page"]);
    let actions: Vec<&str> = m.actions.iter().map(|a| a.decl.id.as_str()).collect();
    assert_eq!(actions, ["free-gpu", "release-gpu-reservation"]);
}

/// Every state of Arete's vector registers cleanly and its tile draws the tree's value with
/// its `tone`, the meter with its words, and the button. The tile's accessible name carries the
/// meter's words.
#[test]
fn every_vulpes_state_registers_and_its_tile_draws_tone_meter_and_button() {
    if !has_fonts() {
        return;
    }
    let v = vector();
    let states = v["states"].as_array().unwrap();
    assert_eq!(states.len(), 13);
    for s in states {
        let name = s["state"].as_str().unwrap();
        let tree = &panel(s, "tile")["tree"];
        assert!(nodes(tree).len() <= 12, "{name}: tile node cap");
        let mut host = host_in(s);
        let r = draw_tile(&ctx(), &mut host, vec![]);
        assert!(!r.draw.panicked, "{name}");
        let value = first(tree, "value").unwrap();
        let glance = r.draw.glance.clone().unwrap();
        assert_eq!(
            glance.text,
            value["props"]["value"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            glance.tone.as_deref(),
            value["props"]["tone"].as_str(),
            "{name}: tone"
        );
        if let Some(meter) = first(tree, "meter") {
            let words = meter["props"]["label"].as_str().unwrap();
            assert!(r.draw.rects.contains_key("meter"), "{name}: meter drawn");
            assert!(r.draw.texts.iter().any(|t| t == words), "{name}: {words}");
        } else {
            assert!(!r.draw.rects.contains_key("meter"), "{name}");
        }
        match first(tree, "button") {
            Some(_) => assert!(r.draw.rects.contains_key("go"), "{name}: button drawn"),
            None => assert!(!r.draw.rects.contains_key("go"), "{name}"),
        }
    }
}

/// A button with an `action` is the hub's own `action.invoke` (no `ui.event`); a button without
/// one ("Try again") is a `ui.event` click on its node.
#[test]
fn the_tile_button_invokes_the_action_or_sends_a_click() {
    if !has_fonts() {
        return;
    }
    for (state, action) in [
        ("idle", "free-gpu"),
        ("reserved", "release-gpu-reservation"),
    ] {
        let mut host = host_in(&state_named(state));
        let r = press_in_tile(&mut host, "go");
        assert!(
            !r.open,
            "{state}: the button's press does not open the page"
        );
        assert_eq!(
            host.take_hub_calls(),
            [HubCall::Invoke {
                module: M.into(),
                action: action.into()
            }],
            "{state}"
        );
        assert!(host.take_events().is_empty(), "{state}: no ui.event");
    }
    let mut host = host_in(&state_named("error"));
    let r = press_in_tile(&mut host, "go");
    assert!(!r.open);
    assert!(host.take_hub_calls().is_empty());
    let events = host.take_events();
    assert_eq!(events.len(), 1, "{events:?}");
    // A press on the tile outside its button opens the page (5.1).
    let mut host = host_in(&state_named("idle"));
    let r = press_in_tile(&mut host, "glance");
    assert!(r.open);
    assert!(host.take_hub_calls().is_empty());
}

/// The page: the Now, Full UI and About sections draw their children with tone and meter, and
/// the hub's Keys section follows for the two actions.
#[test]
fn the_page_draws_sections_with_tone_meter_button_and_markdown() {
    if !has_fonts() {
        return;
    }
    let v = vector();
    for s in v["states"].as_array().unwrap() {
        let name = s["state"].as_str().unwrap();
        let tree = &panel(s, "page")["tree"];
        let mut host = host_in(s);
        let ctx = ctx();
        let mut report = None;
        let _ = ctx.run(raw(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                report = Some(panel_ui(&mut host, ui, &page_key()));
                page_sections_ui(&mut host, ui, &page_key());
            });
        });
        let r = report.unwrap();
        assert!(!r.panicked, "{name}");
        for n in nodes(tree) {
            let id = n["id"].as_str().unwrap();
            assert!(r.rects.contains_key(id), "{name}: {id} drawn");
        }
        let value = first(tree, "value").unwrap();
        assert_eq!(
            r.glance.unwrap().tone.as_deref(),
            value["props"]["tone"].as_str(),
            "{name}"
        );
    }
}

/// An on-demand module that is not running draws "Not running", never what it last showed: not
/// registered yet, and registered then gone idle (the supervisor publishes no state for idle).
#[test]
fn a_cold_on_demand_tile_reads_not_running_never_cached_data() {
    if !has_fonts() {
        return;
    }
    let (a, b) = TILE_NOT_RUNNING;
    let cold_words = format!("AI, {a}. {b}");
    let mut cold = cold_host();
    let r = draw_tile(&ctx(), &mut cold, vec![]);
    assert_eq!(r.phase.unwrap().words, cold_words);
    assert!(r.draw.glance.is_none(), "no body");

    // It ran, drew a held GPU, then exited cleanly: its numbers are not shown.
    let mut was = host_in(&state_named("held-chat"));
    let live = draw_tile(&ctx(), &mut was, vec![]);
    assert_eq!(live.draw.glance.unwrap().text, "Chat: daily");
    was.set_fact("module.vulpes.state", Value::Null);
    was.module_gone(M, "stopped");
    let r = draw_tile(&ctx(), &mut was, vec![]);
    assert_eq!(r.phase.unwrap().words, cold_words);
    assert!(r.draw.texts.is_empty(), "{:?}", r.draw.texts);

    // A module that starts at enable keeps its old words.
    let mut svc = PanelHost::default();
    let text = MANIFEST.replace("on = [\"open\", \"call\"]", "on = [\"enable\"]");
    svc.set_manifest(Module::parse(&text, &Context::default()).unwrap());
    let r = draw_tile(&ctx(), &mut svc, vec![]);
    assert!(!r.phase.unwrap().words.contains(a));
}

/// The page opening sends `visible` to the module, also when the module registers after the
/// page is already on screen (the cold start): the hub's every-frame `set_visible` does it.
/// A failed action's `ui.badge` attention reaches the page panel and clears.
#[test]
fn the_page_opening_sends_visible_and_the_badge_lands_on_the_page() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest());
    host.set_visible(&[page_key()]);
    assert_eq!(host.take_opened(), [M], "the open activation");
    assert!(host.take_visibility().is_empty(), "nobody to tell yet");
    // Arete starts and registers; the next frame's set_visible tells it.
    register(&mut host, &state_named("error"));
    host.set_visible(&[page_key()]);
    let vis = host.take_visibility();
    assert!(
        vis.iter()
            .any(|(m, u)| m == M && u.panel == "page" && u.state == Visibility::Visible),
        "{vis:?}"
    );
    assert!(
        host.take_opened().is_empty(),
        "once per time it comes on screen"
    );

    let badge = |b| UiBadge {
        panel: "page".into(),
        badge: b,
        count: None,
    };
    host.set_badge(M, &badge(Badge::Attention));
    assert_eq!(host.panel(&page_key()).unwrap().badge, Badge::Attention);
    host.set_badge(M, &badge(Badge::None));
    assert_eq!(host.panel(&page_key()).unwrap().badge, Badge::None);
    assert_eq!(host.panel(&page_key()).unwrap().state, PanelState::Live);
}

// ---- Review images (HIDEOUT_REVIEW_DIR) ------------------------------------------------------

/// Tiles of every state of the vector and the cold tile, and four pages, drawn by the real
/// renderer and rasterised here. Names: `vulpes-tiles.png`, `vulpes-page-<state>.png`.
#[test]
fn renders_the_vulpes_page_and_tile_states_for_review() {
    let v = vector();
    let states = v["states"].as_array().unwrap();
    let mut hosts: Vec<(String, PanelHost)> = vec![("not running (cold)".into(), cold_host())];
    for s in states {
        hosts.push((s["state"].as_str().unwrap().to_string(), host_in(s)));
    }
    let (w, h) = (4 * 250 + 20, 4 * 215 + 20);
    let now = Instant::now();
    let Some(px) = rasterise(w, h, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 6.0);
                for (name, host) in hosts.iter_mut() {
                    ui.allocate_ui(Vec2::new(240.0, 205.0), |ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(name.as_str()).small().weak());
                            let _ = tile_ui_at(host, ui, &tile_key(), now);
                        });
                    });
                }
            });
        });
    }) else {
        eprintln!("no system font; review images skipped");
        return;
    };
    println!("{}", save_png("vulpes-tiles.png", w, h, px).display());

    for name in [
        "idle-web-running",
        "held-chat",
        "reserved-open-failed",
        "error",
    ] {
        let mut host = host_in(&state_named(name));
        let (w, h) = (620u32, 520u32);
        let px = rasterise(w, h, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("AI");
                panel_ui(&mut host, ui, &page_key());
                page_sections_ui(&mut host, ui, &page_key());
            });
        })
        .unwrap();
        println!(
            "{}",
            save_png(&format!("vulpes-page-{name}.png"), w, h, px).display()
        );
    }
}
