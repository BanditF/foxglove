// SPDX-License-Identifier: MIT
//! Window-free tests of the hub window with tabs: visibility emission, keyboard switching and
//! restore through the real frame code, and the headless PNGs for review (`HIDEOUT_REVIEW_DIR`).

use super::*;
use eframe::egui::{epaint, Event, RawInput, TextureId};
use hideout_contract::messages::{Badge, UiBadge, Visibility};
use hideout_panels::feed::{channel, Inbound, Outbound, RouterEnd};
use std::collections::HashMap;
use std::path::PathBuf;

struct FakePage(&'static str, &'static str);

impl Page for FakePage {
    fn id(&self) -> &str {
        self.0
    }
    fn title(&self) -> &str {
        self.1
    }
    fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        ui.label(egui::RichText::new(self.1).size(20.0).color(c(t.text)));
        ui.label(egui::RichText::new("A hub page.").color(c(t.text_muted)));
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hideout-shell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn hub_in(dir: &std::path::Path, feed: Option<Box<dyn PanelFeed>>) -> Hub {
    let root = dir.as_os_str().to_owned();
    let paths = HubPaths::resolve(|k| (k == "HIDEOUT_CONFIG_DIR").then(|| root.clone())).unwrap();
    let shared = Arc::new(Shared {
        paths,
        prefs: Mutex::new(Prefs::default()),
        paused: AtomicBool::new(false),
        hidden: AtomicBool::new(false),
    });
    let pages: Vec<Box<dyn Page>> = vec![
        Box::new(FakePage("modules", "Modules")),
        Box::new(FakePage("settings", "Settings")),
    ];
    Hub::new(shared, pages, feed, None)
}

fn register(module: &str, name: &str, panel: &str, title: &str) -> Inbound {
    let params = serde_json::from_value(serde_json::json!({"instance": "i1", "panels": [
        {"id": panel, "slot": "page", "title": title, "icon": "x",
         "tree": {"type": "column", "id": "root", "children": [
            {"type": "label", "id": "l", "props": {"text": format!("{title} page from {name}.")}}]}}]}))
    .unwrap();
    Inbound::Register {
        call: 1,
        module: module.into(),
        name: name.into(),
        declared: None,
        actions: None,
        params,
    }
}

fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    if let Some(f) = hideout_panels::render::test_fonts() {
        ctx.set_fonts(f);
    }
    set_theme(&ctx, ThemeChoice::Dark);
    ctx
}

fn input(w: f32, h: f32, events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(w, h),
        )),
        events,
        ..Default::default()
    }
}

fn key(k: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn visibility(router: &RouterEnd) -> Vec<(String, Visibility)> {
    router
        .take()
        .into_iter()
        .filter_map(|o| match o {
            Outbound::Visibility { visibility, .. } => Some((visibility.panel, visibility.state)),
            _ => None,
        })
        .collect()
}

#[test]
fn visibility_follows_switch_close_and_the_tray() {
    let dir = temp_dir("vis");
    let (router, feed) = channel();
    let mut hub = hub_in(&dir, Some(Box::new(feed)));
    let ctx = ctx();
    router.push(register("mod.chat", "Chat", "chat", "Chat"));
    router.push(register("mod.art", "Art", "art", "Art"));
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    let first = visibility(&router);
    assert!(
        first.contains(&("chat".into(), Visibility::Hidden)),
        "{first:?}"
    );
    assert!(first.contains(&("art".into(), Visibility::Hidden)));
    hub.open(TabId::panel("mod.chat", "chat"));
    hub.open(TabId::panel("mod.art", "art"));
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert_eq!(visibility(&router), [("art".into(), Visibility::Visible)]);
    // Ctrl+Shift+Tab: back to Chat (strip order: Modules, Chat, Art).
    let ev = vec![key(Key::Tab, Modifiers::COMMAND | Modifiers::SHIFT)];
    let _ = ctx.run(input(1100.0, 640.0, ev), |c| hub.frame(c));
    let v = visibility(&router);
    assert!(v.contains(&("chat".into(), Visibility::Visible)), "{v:?}");
    assert!(v.contains(&("art".into(), Visibility::Hidden)));
    assert_eq!(hub.tabs.active(), Some(1));
    // Ctrl+W closes Chat; Art is to its right and comes up.
    let _ = ctx.run(
        input(1100.0, 640.0, vec![key(Key::W, Modifiers::COMMAND)]),
        |c| hub.frame(c),
    );
    let v = visibility(&router);
    assert!(v.contains(&("chat".into(), Visibility::Closed)), "{v:?}");
    assert!(v.contains(&("art".into(), Visibility::Visible)));
    // Closed to the tray: hidden. Back: visible. No repeats when nothing changed.
    hub.shared.hidden.store(true, Ordering::Release);
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert_eq!(visibility(&router), [("art".into(), Visibility::Hidden)]);
    hub.shared.hidden.store(false, Ordering::Release);
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert_eq!(visibility(&router), [("art".into(), Visibility::Visible)]);
    // Ctrl+Shift+T brings Chat back at its old place.
    let ev = vec![key(Key::T, Modifiers::COMMAND | Modifiers::SHIFT)];
    let _ = ctx.run(input(1100.0, 640.0, ev), |c| hub.frame(c));
    assert_eq!(
        hub.tabs.active_id(),
        Some(&TabId::panel("mod.chat", "chat"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn order_is_saved_in_the_hub_folder_and_restored() {
    let dir = temp_dir("restore");
    let ctx = ctx();
    {
        let mut hub = hub_in(&dir, None);
        assert_eq!(
            hub.tabs.active_id(),
            Some(&TabId::page("modules")),
            "first start"
        );
        hub.open(TabId::page("settings"));
        hub.panels
            .register("mod.chat", "Chat", None, &params("chat", "Chat"));
        hub.open(TabId::panel("mod.chat", "chat"));
        hub.tabs.move_tab(2, 0);
        hub.tabs.activate(2);
        let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    }
    assert!(dir.join("tabs.toml").exists());
    let hub = hub_in(&dir, None);
    let ids: Vec<&TabId> = hub.tabs.tabs().iter().map(|t| &t.id).collect();
    assert_eq!(
        ids,
        [
            &TabId::panel("mod.chat", "chat"),
            &TabId::page("modules"),
            &TabId::page("settings")
        ]
    );
    assert_eq!(hub.tabs.active(), Some(2));
    // Before Chat registers again it keeps its name and reads "Starting".
    let l = hub.look(0);
    assert_eq!(l.title, "Chat");
    assert_eq!(l.state, TabState::Busy("Starting".into()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dirty_tab_asks_and_keep_open_is_the_default() {
    let dir = temp_dir("dirty");
    let ctx = ctx();
    let mut hub = hub_in(&dir, None);
    hub.panels
        .register("mod.scenes", "Scenes", None, &params("scenes", "Scenes"));
    hub.open(TabId::panel("mod.scenes", "scenes"));
    hub.panels.set_badge(
        "mod.scenes",
        &UiBadge {
            panel: "scenes".into(),
            badge: Badge::Dirty,
            count: None,
        },
    );
    let _ = ctx.run(
        input(1100.0, 640.0, vec![key(Key::W, Modifiers::COMMAND)]),
        |c| hub.frame(c),
    );
    assert!(hub.asking.is_some() && hub.tabs.len() == 2);
    let _ = ctx.run(
        input(1100.0, 640.0, vec![key(Key::Escape, Modifiers::NONE)]),
        |c| hub.frame(c),
    );
    assert!(
        hub.asking.is_none() && hub.tabs.len() == 2,
        "Esc keeps it open"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn params(panel: &str, title: &str) -> hideout_contract::panel::Register {
    match register("m", title, panel, title) {
        Inbound::Register { params, .. } => params,
        _ => unreachable!(),
    }
}

fn badge(hub: &mut Hub, module: &str, panel: &str, b: Badge, count: Option<u32>) {
    hub.panels.set_badge(
        module,
        &UiBadge {
            panel: panel.into(),
            badge: b,
            count,
        },
    );
}

/// The many-tabs hub of the mockups: every badge and state.
fn mock_hub(dir: &std::path::Path) -> Hub {
    let mut hub = hub_in(dir, None);
    for (m, p, title) in [
        ("mod.keyboard", "keyboard", "Keyboard"),
        ("mod.lights", "lights", "Lights"),
        ("mod.scenes", "scenes", "Scenes"),
        ("mod.screens", "screens", "Screens"),
        ("mod.art", "art", "Art"),
        ("mod.chat", "chat", "Chat"),
        ("mod.images", "images", "Images"),
    ] {
        hub.panels.register(m, title, None, &params(p, title));
        hub.open(TabId::panel(m, p));
    }
    hub.tabs.close_now(&[TabId::page("modules")]);
    badge(&mut hub, "mod.chat", "chat", Badge::Count, Some(3));
    badge(&mut hub, "mod.scenes", "scenes", Badge::Dirty, None);
    badge(&mut hub, "mod.lights", "lights", Badge::Attention, None);
    hub.panels.module_gone("mod.screens", "updating");
    hub.panels.module_gone("mod.art", "crash-stopped");
    let images = hub
        .tabs
        .position(&TabId::panel("mod.images", "images"))
        .unwrap();
    hub.tabs.pop_out(images);
    hub
}

/// Draws a few frames of `hub` and rasterises the last one (premultiplied, nearest texel).
fn render(hub: &mut Hub, w: u32, h: u32, setup: impl Fn(&mut Hub)) -> Option<Vec<u8>> {
    rasterise(w, h, |c| {
        setup(hub);
        hub.frame(c)
    })
}

/// Draws a few frames of `draw` on the hub's dark theme and fonts and rasterises the last one.
fn rasterise(w: u32, h: u32, mut draw: impl FnMut(&egui::Context)) -> Option<Vec<u8>> {
    hideout_panels::render::test_fonts()?;
    let ctx = egui::Context::default();
    // The shell's own fonts (Segoe UI plus Segoe UI Symbol for the marks), as the hub draws.
    install_fonts(&ctx);
    set_theme(&ctx, ThemeChoice::Dark);
    let mut textures: HashMap<TextureId, epaint::ColorImage> = HashMap::new();
    let mut last = None;
    for _ in 0..3 {
        let out = ctx.run(input(w as f32, h as f32, vec![]), &mut draw);
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
    let mut px = vec![[0f32; 4]; (w * h) as usize];
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
            let (minx, maxx) = (
                v.iter()
                    .map(|v| v.pos.x)
                    .fold(f32::MAX, f32::min)
                    .max(clip.left())
                    .max(0.0),
                v.iter()
                    .map(|v| v.pos.x)
                    .fold(f32::MIN, f32::max)
                    .min(clip.right())
                    .min(w as f32),
            );
            let (miny, maxy) = (
                v.iter()
                    .map(|v| v.pos.y)
                    .fold(f32::MAX, f32::min)
                    .max(clip.top())
                    .max(0.0),
                v.iter()
                    .map(|v| v.pos.y)
                    .fold(f32::MIN, f32::max)
                    .min(clip.bottom())
                    .min(h as f32),
            );
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
                    if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 && cx >= clip.left() && cy >= clip.top()
                    {
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

fn save_png(name: &str, w: u32, h: u32, rgba: Vec<u8>) {
    let dir = std::env::var_os("HIDEOUT_REVIEW_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("hideout-review"));
    std::fs::create_dir_all(&dir).unwrap();
    let img = image::RgbaImage::from_raw(w, h, rgba).unwrap();
    img.save(dir.join(name)).unwrap();
}

#[test]
fn renders_the_three_review_states() {
    // 1. Many tabs, every badge and state, Chat active (count 3).
    let dir = temp_dir("png1");
    let mut hub = mock_hub(&dir);
    let chat = hub
        .tabs
        .position(&TabId::panel("mod.chat", "chat"))
        .unwrap();
    hub.tabs.activate(chat);
    let Some(rgba) = render(&mut hub, 1280, 520, |_| {}) else {
        eprintln!("no system font; render test skipped");
        return;
    };
    // The active tab's top edge is the accent.
    assert!(rgba.chunks(4).any(|p| p[..3] == [0xff, 0x8a, 0x2a]));
    save_png("built-strip-states.png", 1280, 520, rgba);

    // 2. Overflow: twelve tabs in a narrow window, the last one active (kept in view).
    let dir = temp_dir("png2");
    let mut hub = mock_hub(&dir);
    for (m, p, title) in [
        ("mod.notes", "notes", "Notes"),
        ("mod.macros", "macros", "Macros"),
        ("mod.sensors", "sensors", "Sensors"),
        ("mod.layouts", "layouts", "Layouts with a long title"),
    ] {
        hub.panels.register(m, title, None, &params(p, title));
        let last = hub.tabs.len().saturating_sub(1);
        hub.tabs.activate(last);
        hub.open(TabId::panel(m, p));
    }
    hub.open(TabId::page("settings"));
    let rgba = render(&mut hub, 1000, 520, |_| {}).unwrap();
    assert!(
        hub.strip.scroll > 0.0,
        "the active tab was scrolled into view"
    );
    save_png("built-overflow.png", 1000, 520, rgba);

    // 3. A stopped module active, a popped-out tab, and closing Scenes with unsaved work.
    let dir = temp_dir("png3");
    let mut hub = mock_hub(&dir);
    let art = hub.tabs.position(&TabId::panel("mod.art", "art")).unwrap();
    hub.tabs.activate(art);
    hub.asking = Some(vec![TabId::panel("mod.scenes", "scenes")]);
    let rgba = render(&mut hub, 1280, 520, |_| {}).unwrap();
    assert!(hub.asking.is_some(), "the ask stays until answered");
    save_png("built-stopped-popout-close.png", 1280, 520, rgba);
}

/// The idle hub draws once and then asks for nothing: no repaint, no repaint-after. A frame
/// that asked again would keep eframe out of `ControlFlow::Wait` (#935, ported from #962).
#[test]
fn an_idle_frame_requests_no_repaint() {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    set_theme(&ctx, ThemeChoice::System);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    ctx.set_request_repaint_callback(move |info| log.lock().unwrap().push(info.delay));
    let mut hub = hub_in(&temp_dir("idle"), None);
    let input = || RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            placement::DEFAULT_SIZE.into(),
        )),
        ..Default::default()
    };
    // egui settles layout and fonts over the first passes.
    for _ in 0..4 {
        let _ = ctx.run(input(), |ctx| hub.frame(ctx));
    }
    asked.lock().unwrap().clear();
    for pass in 0..3 {
        let out = ctx.run(input(), |ctx| hub.frame(ctx));
        let vp = &out.viewport_output[&egui::ViewportId::ROOT];
        assert_eq!(
            vp.repaint_delay,
            std::time::Duration::MAX,
            "idle pass {pass} asked for a repaint"
        );
    }
    assert!(
        asked.lock().unwrap().is_empty(),
        "{:?}",
        asked.lock().unwrap()
    );
}

/// Closed to the tray, a frame asks for nothing either: a hidden window gets no frame for it,
/// and only `wake::Gate` would keep eframe's Poll from spinning (#935).
#[test]
fn a_hidden_frame_requests_no_repaint() {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    set_theme(&ctx, ThemeChoice::System);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    ctx.set_request_repaint_callback(move |info| log.lock().unwrap().push(info.delay));
    let mut hub = hub_in(&temp_dir("hidden-idle"), None);
    let input = || RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            placement::DEFAULT_SIZE.into(),
        )),
        ..Default::default()
    };
    for _ in 0..4 {
        let _ = ctx.run(input(), |ctx| hub.frame(ctx));
    }
    hub.shared.hidden.store(true, Ordering::Release);
    // The first hidden pass sends the visibility change; after that nothing.
    let _ = ctx.run(input(), |ctx| hub.frame(ctx));
    asked.lock().unwrap().clear();
    for pass in 0..3 {
        let out = ctx.run(input(), |ctx| hub.frame(ctx));
        let vp = &out.viewport_output[&egui::ViewportId::ROOT];
        assert_eq!(
            vp.repaint_delay,
            std::time::Duration::MAX,
            "hidden pass {pass} asked for a repaint"
        );
    }
    assert!(
        asked.lock().unwrap().is_empty(),
        "{:?}",
        asked.lock().unwrap()
    );
}

/// The tray panel's tiles for the review (H2.8): every module in tests/hideout-vectors/modules,
/// each state as a tile, plus the first module stopped after its crashes and restarting, and the
/// first state's page of the module whose vector file sorts first. The hub names no module: it
/// draws whatever vectors are there.
#[test]
fn renders_the_module_tiles_for_review() {
    use hideout_contract::panel::Register;
    use hideout_panels::{panel_ui, tile_ui, PanelHost, PanelKey};
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors/modules");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut host = PanelHost::default();
    let mut rows: Vec<Vec<PanelKey>> = Vec::new();
    let mut page: Option<(PanelKey, String)> = None;
    for f in &files {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let module = doc["module"].as_str().unwrap();
        let name = doc["name"].as_str().unwrap();
        let mut row = Vec::new();
        for (i, st) in doc["states"].as_array().unwrap().iter().enumerate() {
            // One host entry per state: the module id with the state's index.
            let id = format!("{module}#{i}");
            let params: Register = serde_json::from_value(
                serde_json::json!({"instance": "i1", "panels": st["panels"]}),
            )
            .unwrap();
            host.register(&id, name, None, &params);
            row.push(PanelKey::new(&id, "tile"));
            if row.len() == 4 {
                rows.push(std::mem::take(&mut row));
            }
            if page.is_none() && st["panels"].to_string().contains("\"connect\"") {
                page = Some((PanelKey::new(&id, "page"), name.to_string()));
            }
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    // The supervisor's states on the first module's first tile (hub-drawn, §7.8).
    if let Some(first) = rows.first().and_then(|r| r.first()).cloned() {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
        let params: Register = serde_json::from_value(
            serde_json::json!({"instance": "i1", "panels": doc["states"][0]["panels"]}),
        )
        .unwrap();
        let mut extra = Vec::new();
        for state in ["restarting", "crash-stopped"] {
            let id = format!("{}#{state}", first.module);
            host.register(&id, doc["name"].as_str().unwrap(), None, &params);
            host.module_gone(&id, state);
            extra.push(PanelKey::new(&id, "tile"));
        }
        rows.push(extra);
    }
    let (w, h) = (1040u32, 150 * rows.len() as u32 + 40);
    let Some(rgba) = rasterise(w, h, |c| {
        egui::CentralPanel::default().show(c, |ui| {
            for row in &rows {
                ui.horizontal_top(|ui| {
                    for key in row {
                        tile_ui(&mut host, ui, key);
                    }
                });
                ui.add_space(8.0);
            }
        });
    }) else {
        eprintln!("no system font; review render skipped");
        return;
    };
    save_png("built-module-tiles.png", w, h, rgba);
    if let Some((key, _)) = page {
        let (w, h) = (720u32, 900u32);
        let rgba = rasterise(w, h, |c| {
            egui::CentralPanel::default().show(c, |ui| {
                panel_ui(&mut host, ui, &key);
            });
        })
        .unwrap();
        save_png("built-module-page.png", w, h, rgba);
    }
}

/// The modules' manifests next to their vectors (#991): every `crates/*/module.toml` whose id
/// has a vectors file, with that file's first state registered. The hub names none of them.
fn review_modules() -> (hideout_panels::PanelHost, Vec<String>) {
    use hideout_contract::module::{Context, Module};
    use hideout_contract::panel::Register;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut states = HashMap::new();
    for e in std::fs::read_dir(root.join("tests/hideout-vectors/modules")).unwrap() {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
        states.insert(doc["module"].as_str().unwrap().to_string(), doc);
    }
    let mut host = hideout_panels::PanelHost::default();
    let mut ids = Vec::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .unwrap()
        .map(|e| e.unwrap().path().join("module.toml"))
        .filter(|p| p.exists())
        .collect();
    crates.sort();
    for p in crates {
        let Ok(m) = Module::parse(&std::fs::read_to_string(&p).unwrap(), &Context::default())
        else {
            continue;
        };
        let Some(doc) = states.get(&m.id) else {
            continue;
        };
        let params: Register = serde_json::from_value(
            serde_json::json!({"instance": "i1", "panels": doc["states"][0]["panels"]}),
        )
        .unwrap();
        let declared: Vec<String> = m.panels.iter().map(|p| p.id.clone()).collect();
        let actions: Vec<String> = m.actions.iter().map(|a| a.decl.id.clone()).collect();
        host.register_with(&m.id, &m.name, Some(&declared), Some(&actions), &params);
        ids.push(m.id.clone());
        host.set_manifest(m);
    }
    (host, ids)
}

/// The tray panel for the review (#991): the default tiles; every tile with a restarting and a
/// stopped one; and one page with the hub's Keys and Settings sections.
#[test]
fn renders_the_tray_panel_for_review() {
    use hideout_panels::{page_sections_ui, panel_ui, tray_panel_ui, TrayLayout};
    use std::time::{Duration, Instant};
    let (mut host, ids) = review_modules();
    if ids.is_empty() {
        eprintln!("no module vectors; tray review skipped");
        return;
    }
    let now = Instant::now();
    let draw_tray = |host: &mut hideout_panels::PanelHost, name: &str, h: u32| {
        let mut layout = TrayLayout::default();
        let mut editing = false;
        let w = 520u32;
        let Some(rgba) = rasterise(w, h, |c| {
            egui::CentralPanel::default().show(c, |ui| {
                tray_panel_ui(host, ui, &mut layout, &mut editing, now);
            });
        }) else {
            return false;
        };
        save_png(name, w, h, rgba);
        true
    };
    // 1. Default tiles: only those with no `when` (no app seen yet).
    if !draw_tray(&mut host, "built-tray-default.png", 260) {
        eprintln!("no system font; tray review skipped");
        return;
    }
    // 2. Every app seen: all tiles; the last module restarting, the one before it stopped.
    let whens: Vec<String> = host
        .manifests()
        .flat_map(|m| m.panels.iter().filter_map(|p| p.when.clone()))
        .collect();
    for w in whens {
        host.set_fact(w.trim_start_matches('!'), serde_json::json!(true));
    }
    let n = ids.len();
    if n >= 2 {
        let restarting = &ids[n - 1];
        host.module_gone_at(restarting, "restarting", now - Duration::from_secs(12));
        host.set_fact(
            &format!("module.{restarting}.restart"),
            serde_json::json!({"delay_s": 8, "crash": 2, "of": 5}),
        );
        host.set_restart_since(restarting, now - Duration::from_millis(3500));
        host.set_fact(
            &format!("module.{restarting}.state"),
            serde_json::json!("restarting"),
        );
        let stopped = &ids[n - 2];
        host.set_fact(
            &format!("module.{stopped}.state"),
            serde_json::json!("crash-stopped"),
        );
    }
    draw_tray(&mut host, "built-tray-all.png", 600);
    // 3. Pages with the hub's sections: the first module with actions (Keys, one key bound)
    // and the first with settings (Settings), side by side.
    let first = |host: &hideout_panels::PanelHost,
                 f: &dyn Fn(&hideout_contract::module::Module) -> bool| {
        ids.iter()
            .find(|id| host.manifest(id).is_some_and(f))
            .cloned()
    };
    let keys = first(&host, &|m| !m.actions.is_empty());
    let settings = first(&host, &|m| !m.settings.is_empty());
    let mut pages = Vec::new();
    for id in [keys, settings].into_iter().flatten() {
        let first_action = host
            .manifest(&id)
            .and_then(|m| m.actions.first().map(|a| a.decl.id.clone()));
        if let Some(a) = first_action {
            host.set_key(&id, &a, Some("F13".into()));
        }
        host.set_fact(&format!("module.{id}.state"), serde_json::json!("running"));
        if let Some(page) = host.navigate(&id, "page") {
            pages.push(page);
        }
    }
    let (w, h) = (1300u32, 760u32);
    let rgba = rasterise(w, h, |c| {
        egui::CentralPanel::default().show(c, |ui| {
            ui.columns(pages.len().max(1), |cols| {
                for (ui, page) in cols.iter_mut().zip(&pages) {
                    ui.push_id(&page.module, |ui| {
                        panel_ui(&mut host, ui, page);
                        page_sections_ui(&mut host, ui, page);
                    });
                }
            });
        });
    })
    .unwrap();
    save_png("built-page-keys-settings.png", w, h, rgba);
}
