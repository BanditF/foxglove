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
    /// Each hub page its own glyph (#953 F4): Settings the gear, the others the module icon.
    fn icon(&self) -> &str {
        if self.0 == "settings" {
            "gear"
        } else {
            "module"
        }
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
        flyout_click: Mutex::new(None),
        start_normally: None,
        quit: QuitAsk::default(),
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
fn rasterise(w: u32, h: u32, draw: impl FnMut(&egui::Context)) -> Option<Vec<u8>> {
    rasterise_events(w, h, Vec::new(), draw)
}

/// [`rasterise`] with input: `events[k]` goes to frame `k` (of at least three).
fn rasterise_events(
    w: u32,
    h: u32,
    mut events: Vec<Vec<Event>>,
    mut draw: impl FnMut(&egui::Context),
) -> Option<Vec<u8>> {
    hideout_panels::render::test_fonts()?;
    let ctx = egui::Context::default();
    // The shell's own fonts (Segoe UI plus Segoe UI Symbol for the marks), as the hub draws.
    install_fonts(&ctx);
    set_theme(&ctx, ThemeChoice::Dark);
    let mut textures: HashMap<TextureId, epaint::ColorImage> = HashMap::new();
    let mut last = None;
    let n = events.len().max(3);
    events.resize(n, Vec::new());
    for ev in events {
        let out = ctx.run(input(w as f32, h as f32, ev), &mut draw);
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

/// Closed to the tray with live modules (#1000): a module restarting (its countdown, its state
/// facts) wakes the hub once per router event, and the hidden frames that brings ask for no
/// repaint and no repaint-after. Only a shown page or the open flyout may count down.
#[test]
fn a_hidden_hub_with_a_restarting_module_requests_no_repaint() {
    let ctx = ctx();
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (router, mut feed) = channel();
    let w = wakes.clone();
    let wake_ctx = ctx.clone();
    feed.set_waker(Box::new(move || {
        w.fetch_add(1, Ordering::Relaxed);
        wake_ctx.request_repaint();
    }));
    let mut hub = hub_in(&temp_dir("hidden-restart"), Some(Box::new(feed)));
    router.push(register("mod.chat", "Chat", "chat", "Chat"));
    for _ in 0..4 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    hub.open(TabId::panel("mod.chat", "chat"));
    for _ in 0..4 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    hub.shared.hidden.store(true, Ordering::Release);
    for _ in 0..2 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    let before = wakes.load(Ordering::Relaxed);
    // The supervisor's restart while hidden: state, restart countdown, then running again.
    let fact = |name: &str, value: serde_json::Value| Inbound::Fact {
        name: name.into(),
        value,
    };
    router.push(fact(
        "module.mod.chat.state",
        serde_json::json!("restarting"),
    ));
    router.push(fact(
        "module.mod.chat.restart",
        serde_json::json!({"delay_s": 8, "crash": 2, "of": 5}),
    ));
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        before + 2,
        "one wake per event"
    );
    // The frame the wake brings takes the events in (egui's own second frame of a wake).
    quiet_frame(&ctx, &mut hub, vec![]);
    for pass in 0..4 {
        assert!(
            !quiet_frame(&ctx, &mut hub, vec![]),
            "hidden pass {pass} with a restarting module asked for a repaint"
        );
    }
    router.push(fact("module.mod.chat.state", serde_json::json!("running")));
    quiet_frame(&ctx, &mut hub, vec![]);
    for pass in 0..3 {
        assert!(
            !quiet_frame(&ctx, &mut hub, vec![]),
            "hidden pass {pass} after the restart asked for a repaint"
        );
    }
}

/// The footprint gate's case (#1000), window-free: a hidden hub with no flyout and a mock feed.
/// Each loop wake eframe would get is egui's repaint callback (its `RequestRepaint` user event).
/// A burst of feed events while hidden is one wake, and the hidden frames that follow ask for no
/// repaint: the loop then waits until the next event, so it stays under 1 wake/s.
#[test]
fn a_hidden_hub_with_a_busy_feed_wakes_the_loop_once_per_burst() {
    let ctx = ctx();
    let loop_wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lw = loop_wakes.clone();
    ctx.set_request_repaint_callback(move |_| {
        lw.fetch_add(1, Ordering::Relaxed);
    });
    let (router, mut feed) = channel();
    let wake_ctx = ctx.clone();
    feed.set_waker(Box::new(move || wake_ctx.request_repaint()));
    let mut hub = hub_in(&temp_dir("hidden-burst"), Some(Box::new(feed)));
    router.push(register("mod.chat", "Chat", "chat", "Chat"));
    for _ in 0..4 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    hub.shared.hidden.store(true, Ordering::Release);
    for _ in 0..3 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    assert!(hub.flyout.is_none(), "no flyout in this case");
    for round in 0..3 {
        let before = loop_wakes.load(Ordering::Relaxed);
        for i in 0..20 {
            router.push(Inbound::Fact {
                name: format!("apps.seen.tool{i}"),
                value: serde_json::json!(round),
            });
        }
        assert_eq!(
            loop_wakes.load(Ordering::Relaxed),
            before + 1,
            "round {round}: 20 feed events while hidden are one loop wake"
        );
        // The frame the wake brings drains the feed; the hidden hub then asks for nothing.
        quiet_frame(&ctx, &mut hub, vec![]);
        for pass in 0..3 {
            assert!(
                !quiet_frame(&ctx, &mut hub, vec![]),
                "round {round}: hidden pass {pass} asked for a repaint"
            );
        }
    }
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
        for state in ["restarting", "crash-stopped", "revoked"] {
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
    let settings = first(&host, &|m| {
        m.settings
            .iter()
            .any(|s| s.kind == hideout_contract::module::ModuleSettingKind::File)
    });
    let secrets = first(&host, &|m| !m.secrets.is_empty());
    // Choose file shows with a picker; the review never opens a real one.
    host.picker = Some(hideout_panels::model::Picker(Box::new(|_| None)));
    // Secrets answer as not saved yet (the masked field).
    if let Some(id) = &secrets {
        host.secrets_result(id, Default::default());
    }
    let mut pages = Vec::new();
    for id in [keys, settings, secrets].into_iter().flatten() {
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
    let (w, h) = (1600u32, 760u32);
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

/// A frame with no input on the hub's own context, returning whether it asked for a repaint.
fn quiet_frame(ctx: &egui::Context, hub: &mut Hub, events: Vec<Event>) -> bool {
    let out = ctx.run(input(960.0, 640.0, events), |ctx| hub.frame(ctx));
    out.viewport_output[&egui::ViewportId::ROOT].repaint_delay != std::time::Duration::MAX
}

fn tray_click(hub: &Hub) {
    let icon = placement::Rect {
        x: 1800,
        y: 1044,
        w: 24,
        h: 24,
    };
    *hub.shared.flyout_click.lock().unwrap() = Some((icon, std::time::Instant::now()));
}

/// The tray flyout with the main window hidden (#991, #935): a tray click opens it in the next
/// frame, it then settles (no repaint asked, so `wake::Gate` stops running frames), Esc closes
/// it and the frame after that drops it; a click while it is open closes it.
#[test]
fn the_flyout_opens_settles_and_closes_while_the_hub_is_hidden() {
    let ctx = ctx();
    let mut hub = hub_in(&temp_dir("flyout"), None);
    hub.shared.hidden.store(true, Ordering::Release);
    for _ in 0..3 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    assert!(hub.flyout.is_none());
    tray_click(&hub);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(hub.flyout.is_some(), "a tray click opens the flyout");
    // Its first frames look for its window (none here) and then show it where it is.
    for _ in 0..6 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    for pass in 0..3 {
        assert!(
            !quiet_frame(&ctx, &mut hub, vec![]),
            "open flyout pass {pass} asked for a repaint"
        );
    }
    assert!(hub.flyout.is_some(), "it stays open without input");
    quiet_frame(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(hub.flyout.is_none(), "Esc closes it");
    for _ in 0..2 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    assert!(
        !quiet_frame(&ctx, &mut hub, vec![]),
        "the closed flyout leaves a hidden hub that asks for nothing"
    );
    // A click opens it, the next click closes it.
    tray_click(&hub);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(hub.flyout.is_some());
    tray_click(&hub);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(hub.flyout.is_none());
    // Closed by focus loss a moment ago: that click was the same click, so it stays closed.
    hub.flyout_closed_by_focus = Some(std::time::Instant::now());
    tray_click(&hub);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(hub.flyout.is_none());
}

/// #1019: the main window closed to the tray, then a tray click. The frame that hid the window
/// asked egui for a repaint (`CancelClose`) that eframe can never answer, as a hidden window gets
/// no `RedrawRequested`; egui then holds that request and calls eframe's repaint callback only for
/// a sooner one, so the click's own `request_repaint` never reaches eframe and its flow stays
/// `Wait`. On main the gate ran a hidden frame only for `Poll`: the flyout stayed open and never
/// drawn. Now the click kicks the first frame, and from there the flyout's own requests reach
/// eframe: it is placed and shown, then settles and the loop waits again.
#[test]
fn a_tray_click_draws_the_flyout_while_the_main_window_is_hidden() {
    use std::sync::atomic::AtomicUsize;
    use winit::event_loop::ControlFlow;
    let ctx = ctx();
    // eframe's side, in miniature: each callback schedules a frame (its flow turns `Poll`).
    let asked = Arc::new(AtomicUsize::new(0));
    let a = asked.clone();
    ctx.set_request_repaint_callback(move |_| {
        a.fetch_add(1, Ordering::AcqRel);
    });
    let mut hub = hub_in(&temp_dir("flyout-hidden-root"), None);
    for _ in 0..3 {
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    // The close-to-tray frame: CancelClose asks for a repaint, and the window hides.
    let _ = ctx.run(input(960.0, 640.0, vec![]), |ctx| {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        hub.shared.hidden.store(true, Ordering::Release);
        hub.frame(ctx)
    });
    // eframe took that request; the hidden window's redraw never comes, so no frame follows.
    let mut seen = asked.load(Ordering::Acquire);
    let icon = placement::Rect {
        x: 1800,
        y: 1044,
        w: 24,
        h: 24,
    };
    on_tray(tray::Command::Flyout(icon), &hub.shared, &ctx);
    assert_eq!(
        asked.load(Ordering::Acquire),
        seen,
        "egui passes the click's repaint on (the stale request is gone?)"
    );
    // The gate's turns after the click (the flyout is live from the click on; read as a literal
    // because other tests' frames flip the process-wide flag).
    let mut frames = 0;
    let mut streak = 0;
    let mut kicked = flyout::take_kick();
    for _ in 0..crate::wake::HIDDEN_STREAK * 2 {
        let now = asked.load(Ordering::Acquire);
        let flow = if now > seen {
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        };
        seen = now;
        if kicked {
            streak = 0;
        }
        if !crate::wake::hidden_pass(flow, false, crate::wake::side_frames(true, streak), kicked) {
            break;
        }
        kicked = false;
        streak += 1;
        frames += 1;
        quiet_frame(&ctx, &mut hub, vec![]);
    }
    assert!(
        frames > 0,
        "no frame ran after the tray click with the main window hidden"
    );
    let fly = hub
        .flyout
        .as_ref()
        .expect("the tray click opens the flyout");
    assert!(
        fly.placed.is_some(),
        "the flyout was placed and shown in {frames} frames"
    );
    assert!(
        frames < crate::wake::HIDDEN_STREAK as usize,
        "the flyout settles: {frames} hidden frames"
    );
    // Esc closes it (an event on its window: a frame), and the hidden hub asks for nothing.
    quiet_frame(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(hub.flyout.is_none());
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(!quiet_frame(&ctx, &mut hub, vec![]));
    assert!(!flyout::take_kick(), "one kick per click");
}

/// A frame whose flyout window says it lacks the focus (Windows refused the hand-off).
fn unfocused_frame(ctx: &egui::Context, hub: &mut Hub) -> bool {
    let mut raw = input(960.0, 640.0, vec![]);
    raw.viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(false);
    let out = ctx.run(raw, |ctx| hub.frame(ctx));
    out.viewport_output[&egui::ViewportId::ROOT].repaint_delay != std::time::Duration::MAX
}

/// A flyout shown without ever getting the focus (#1000) can't close on focus loss: it asks for
/// one look when the grace ends and closes then, so the hidden hub goes back to waiting.
#[test]
fn a_flyout_that_never_got_the_focus_closes() {
    let ctx = ctx();
    let mut hub = hub_in(&temp_dir("flyout-nofocus"), None);
    hub.shared.hidden.store(true, Ordering::Release);
    tray_click(&hub);
    for _ in 0..8 {
        unfocused_frame(&ctx, &mut hub);
    }
    let shown = hub.flyout.as_ref().and_then(|f| f.shown_at);
    assert!(shown.is_some(), "the flyout is up and shown");
    // Within the grace: it stays, with one look asked for (a repaint-after, not a loop).
    assert!(unfocused_frame(&ctx, &mut hub), "it asks for the look");
    assert!(hub.flyout.is_some());
    // The grace is over: it closes, and the hidden hub asks for nothing after that.
    if let Some(f) = hub.flyout.as_mut() {
        f.shown_at = Some(std::time::Instant::now() - flyout::FOCUS_GRACE);
    }
    unfocused_frame(&ctx, &mut hub);
    assert!(
        hub.flyout.is_none(),
        "a flyout that never got the focus closes"
    );
    for _ in 0..2 {
        unfocused_frame(&ctx, &mut hub);
    }
    assert!(!unfocused_frame(&ctx, &mut hub));
    assert!(
        hub.flyout_closed_by_focus.is_none(),
        "not a focus-loss toggle"
    );
}

/// An open flyout that has the focus settles: frames after the first few ask for nothing, so
/// `wake::Gate` stops running hidden frames (#1000).
#[test]
fn an_idle_open_flyout_settles_to_no_repaint() {
    let ctx = ctx();
    let mut hub = hub_in(&temp_dir("flyout-idle"), None);
    hub.shared.hidden.store(true, Ordering::Release);
    tray_click(&hub);
    let focused = |ctx: &egui::Context, hub: &mut Hub| {
        let mut raw = input(960.0, 640.0, vec![]);
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(true);
        let out = ctx.run(raw, |ctx| hub.frame(ctx));
        out.viewport_output[&egui::ViewportId::ROOT].repaint_delay != std::time::Duration::MAX
    };
    for _ in 0..8 {
        focused(&ctx, &mut hub);
    }
    for pass in 0..crate::wake::HIDDEN_STREAK {
        assert!(
            !focused(&ctx, &mut hub),
            "idle flyout pass {pass} asked for a repaint"
        );
    }
    assert!(hub.flyout.is_some(), "a focused flyout stays open");
}

/// The tile layout (order, hidden, shown) lives in `hideout.toml` `[tray]` and comes back with
/// the next hub; other keys stay.
#[test]
fn the_tile_layout_is_saved_in_hideout_toml() {
    let dir = temp_dir("tray-layout");
    let hub = {
        let mut hub = hub_in(&dir, None);
        hub.tray_layout.order = vec!["b.m/tile".into(), "a.m/tile".into()];
        hub.tray_layout.hidden.insert("c.m/tile".into());
        hub.tray_layout.shown.insert("d.m/tile".into());
        hub.save_tray_layout();
        hub
    };
    let file = hub.shared.paths.prefs_file();
    let (prefs, err) = Prefs::load(&file);
    assert_eq!(err, None);
    assert_eq!(prefs.tray.order, ["b.m/tile", "a.m/tile"]);
    assert_eq!(prefs.tray.hidden, ["c.m/tile"]);
    assert_eq!(prefs.tray.shown, ["d.m/tile"]);
    let shared = Arc::new(Shared {
        paths: hub.shared.paths.clone(),
        prefs: Mutex::new(prefs),
        paused: AtomicBool::new(false),
        hidden: AtomicBool::new(false),
        flyout_click: Mutex::new(None),
        start_normally: None,
        quit: QuitAsk::default(),
    });
    let again = Hub::new(shared, Vec::new(), None, None);
    assert_eq!(again.tray_layout, hub.tray_layout);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The flyout for the review (#991): the default tiles, the same with the keyboard focus on the
/// first tile (as it opens), all five once their apps were seen, and Edit tiles, each at the
/// size the flyout opens with. Each module's first action has a key bound, so its button shows
/// the key chip.
#[test]
fn renders_the_flyout_for_review() {
    let (mut host, ids) = review_modules();
    if ids.is_empty() {
        eprintln!("no module vectors; flyout review skipped");
        return;
    }
    let bound: Vec<(String, String)> = host
        .manifests()
        .filter_map(|m| m.actions.first().map(|a| (m.id.clone(), a.decl.id.clone())))
        .collect();
    for (i, (m, a)) in bound.iter().enumerate() {
        host.set_key(m, a, Some(format!("F{}", 13 + i)));
    }
    let t = theme::tokens(true);
    let w = flyout::WIDTH as u32;
    let draw = |host: &mut hideout_panels::PanelHost, name: &str, editing: bool, focus: bool| {
        let mut layout = TrayLayout::default();
        // Measure first, then draw at the height the flyout would take.
        let mut height = flyout::FIRST_HEIGHT;
        let mut ed = editing;
        rasterise(w, 900, |c| {
            egui::CentralPanel::default()
                .frame(flyout_frame(&t))
                .show(c, |ui| {
                    height = flyout_panel(ui, host, &mut layout, &mut ed, &t).1
                });
        })?;
        let h = height.ceil() as u32;
        let mut ed = editing;
        let rgba = rasterise(w, h, |c| {
            if focus {
                hideout_panels::tray::focus_first_tile(c, host, &layout, true);
            }
            egui::CentralPanel::default()
                .frame(flyout_frame(&t))
                .show(c, |ui| flyout_panel(ui, host, &mut layout, &mut ed, &t));
        })?;
        save_png(name, w, h, rgba);
        Some(())
    };
    if draw(&mut host, "built-flyout-default.png", false, false).is_none() {
        eprintln!("no system font; flyout review skipped");
        return;
    }
    draw(&mut host, "built-flyout-keyboard.png", false, true);
    let whens: Vec<String> = host
        .manifests()
        .flat_map(|m| m.panels.iter().filter_map(|p| p.when.clone()))
        .collect();
    for w in whens {
        host.set_fact(w.trim_start_matches('!'), serde_json::json!(true));
    }
    for id in &ids {
        host.set_fact(&format!("module.{id}.state"), serde_json::json!("running"));
    }
    draw(&mut host, "built-flyout-all.png", false, false);
    draw(&mut host, "built-flyout-edit.png", true, false);
}

// ---- The supervisor's surfaces (#983): crash bar, stopped card, Couldn't start ----

use std::time::Duration;

/// What the ladder's buttons asked of the supervisor.
#[derive(Clone, Default)]
struct Asked(Arc<Mutex<Vec<String>>>);

impl TabHooks for Asked {
    fn start_again(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("start-again {m}"));
    }
    fn start_with_defaults(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("defaults {m}"));
    }
    fn go_back(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("go-back {m}"));
    }
    fn turn_off(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("turn-off {m}"));
    }
    fn check_for_updates(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("check {m}"));
    }
}

const SUP: &str = "mod.art";

/// Art's manifest: its `name` is what every supervisor surface says.
fn art_manifest() -> hideout_contract::module::Module {
    let toml = format!(
        r#"
id = "{SUP}"
name = "Art"
version = "0.5.0"
kind = "service"
family = "third-party"
hub = "1.0"
run = ["art.exe"]
description = "Art."
"#
    );
    hideout_contract::module::Module::parse(&toml, &Default::default()).expect("manifest parses")
}

/// One module page, Art, active, with these facts (`null` leaves one out). Registered as the
/// hub's feed registers it, under the module's id only; the manifest gives the name (#1013).
fn supervised(dir: &std::path::Path, facts: &[(&str, serde_json::Value)]) -> (Hub, Asked) {
    let mut hub = hub_in(dir, None);
    let asked = Asked::default();
    hub.hooks = Some(Box::new(asked.clone()));
    hub.panels.register(SUP, SUP, None, &params("art", "Art"));
    hub.panels.set_manifest(art_manifest());
    hub.open(TabId::panel(SUP, "art"));
    for (k, v) in facts {
        hub.panels.set_fact(&format!("module.{SUP}.{k}"), v.clone());
    }
    (hub, asked)
}

fn restarting(dir: &std::path::Path, delay: u64, crash: u64, ago: Duration) -> (Hub, Asked) {
    let (mut hub, asked) = supervised(
        dir,
        &[
            (
                "restart",
                serde_json::json!({"delay_s": delay, "crash": crash, "of": 5}),
            ),
            // The frozen reason: never drawn for `restarting`.
            (
                "reason",
                serde_json::json!("Art stopped unexpectedly. Restarting in 4 s (1 of 5)."),
            ),
            ("state", serde_json::json!("restarting")),
        ],
    );
    hub.panels
        .set_restart_since(SUP, std::time::Instant::now() - ago);
    (hub, asked)
}

fn stopped(dir: &std::path::Path, step: &str, previous: Option<&str>) -> (Hub, Asked) {
    supervised(
        dir,
        &[
            ("state", serde_json::json!("crash-stopped")),
            (
                "reason",
                serde_json::json!("Art crashed 5 times in 10 minutes."),
            ),
            ("recovery", serde_json::json!(step)),
            (
                "previous",
                previous.map_or(serde_json::Value::Null, |v| serde_json::json!(v)),
            ),
        ],
    )
}

/// A note in a publisher's own words, 300 characters with a bidi override, a newline and a
/// zero-width character (the cleaning rule's test input, #1026).
fn long_note() -> String {
    let mut n = String::from(
        "This build can send your screen\u{202e}captures\u{202c} to a test server.\n\n\
         Please update to 0.5.1.\u{200b} https://example.test/notes ",
    );
    while n.chars().count() < 300 {
        n.push_str("More words from the publisher. ");
    }
    n
}

/// Art 0.5.0 withdrawn (state `revoked`), with the router's fact, a check status and the Go
/// back version.
fn withdrawn(
    dir: &std::path::Path,
    reason: &str,
    check: Option<&str>,
    previous: Option<&str>,
) -> (Hub, Asked) {
    supervised(
        dir,
        &[
            ("state", serde_json::json!("revoked")),
            ("reason", serde_json::json!(words::WITHDRAWN_LINE)),
            (
                "revoked",
                serde_json::json!({"version": "0.5.0", "reason": reason}),
            ),
            (
                "check",
                check.map_or(serde_json::Value::Null, |v| serde_json::json!(v)),
            ),
            (
                "previous",
                previous.map_or(serde_json::Value::Null, |v| serde_json::json!(v)),
            ),
        ],
    )
}

fn launch_failed(dir: &std::path::Path, reason: &str) -> (Hub, Asked) {
    supervised(
        dir,
        &[
            ("state", serde_json::json!("install-failed")),
            ("reason", serde_json::json!(reason)),
            ("failed", serde_json::json!("launch")),
        ],
    )
}

/// Every text the frame drew.
fn drawn(out: &egui::FullOutput) -> Vec<String> {
    fn walk(s: &epaint::Shape, v: &mut Vec<String>) {
        match s {
            epaint::Shape::Text(t) => v.push(t.galley.text().to_string()),
            epaint::Shape::Vec(all) => all.iter().for_each(|s| walk(s, v)),
            _ => {}
        }
    }
    let mut v = Vec::new();
    for c in &out.shapes {
        walk(&c.shape, &mut v);
    }
    v
}

/// Where a drawn text starts (x).
fn text_x(out: &egui::FullOutput, text: &str) -> Option<f32> {
    fn walk(s: &epaint::Shape, text: &str) -> Option<f32> {
        match s {
            epaint::Shape::Text(t) if t.galley.text() == text => Some(t.pos.x),
            epaint::Shape::Vec(all) => all.iter().find_map(|s| walk(s, text)),
            _ => None,
        }
    }
    out.shapes.iter().find_map(|c| walk(&c.shape, text))
}

/// A few frames; the last one's output.
fn frames(ctx: &egui::Context, hub: &mut Hub, events: Vec<Event>) -> egui::FullOutput {
    for _ in 0..2 {
        let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    }
    ctx.run(input(1100.0, 640.0, events), |c| hub.frame(c))
}

fn repaint_delay(out: &egui::FullOutput) -> Duration {
    out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
}

#[test]
fn the_crash_bar_counts_down_from_the_restart_fact() {
    let ctx = ctx();
    let dir = temp_dir("bar");
    // 8 s, 2.5 s ago: 5.5 s left, rounded up.
    let (mut hub, _) = restarting(&dir, 8, 2, Duration::from_millis(2500));
    let out = frames(&ctx, &mut hub, vec![]);
    let t = drawn(&out);
    assert!(
        t.iter()
            .any(|s| s == "Art stopped unexpectedly. Restarting in 6 s (2 of 5)."),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == words::PLACE_KEPT));
    assert!(t.iter().any(|s| s == words::OPEN_LOG));
    assert!(!t.iter().any(|s| s.contains("Restart now")));
    // Neither the frozen reason nor the greyed banner is drawn.
    assert!(!t.iter().any(|s| s.contains("in 4 s (1 of 5)")), "{t:?}");
    assert!(!t.iter().any(|s| s.starts_with("Art: ")), "{t:?}");
    // Repainted once a second while shown.
    assert!(repaint_delay(&out) <= Duration::from_secs(1));

    // At 0: "Restarting…", and no more repaints.
    let (mut hub, _) = restarting(&dir, 2, 3, Duration::from_secs(5));
    let out = frames(&ctx, &mut hub, vec![]);
    assert!(
        drawn(&out)
            .iter()
            .any(|s| s == "Art stopped unexpectedly. Restarting\u{2026} (3 of 5)."),
        "{:?}",
        drawn(&out)
    );
    assert_eq!(repaint_delay(&out), Duration::MAX);

    // Never past 60 s.
    let (mut hub, _) = restarting(&dir, 300, 1, Duration::ZERO);
    let out = frames(&ctx, &mut hub, vec![]);
    assert!(drawn(&out).iter().any(|s| s.contains("Restarting in 60 s")));

    // Closed to the tray: the countdown asks for no frame (#935, #967).
    let (mut hub, _) = restarting(&dir, 8, 2, Duration::ZERO);
    hub.shared.hidden.store(true, Ordering::Release);
    let out = frames(&ctx, &mut hub, vec![]);
    assert_eq!(repaint_delay(&out), Duration::MAX);

    // Back: the bar goes, with no message.
    let (mut hub, _) = restarting(&dir, 8, 2, Duration::ZERO);
    let _ = frames(&ctx, &mut hub, vec![]);
    hub.panels
        .set_fact(&format!("module.{SUP}.restart"), serde_json::Value::Null);
    hub.panels
        .set_fact(&format!("module.{SUP}.state"), serde_json::json!("running"));
    // Registered again under its id: the manifest's name still wins.
    hub.panels.register(SUP, SUP, None, &params("art", "Art"));
    let out = frames(&ctx, &mut hub, vec![]);
    let t = drawn(&out);
    assert!(
        !t.iter()
            .any(|s| s.contains("stopped unexpectedly") || s.contains("Restarting")),
        "{t:?}"
    );
    assert_eq!(repaint_delay(&out), Duration::MAX);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_crash_bar_is_announced_once_and_the_countdown_never() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("live");
    let (mut hub, _) = restarting(&dir, 8, 2, Duration::ZERO);
    let live = |out: &egui::FullOutput| -> Vec<String> {
        out.platform_output
            .accesskit_update
            .as_ref()
            .map(|u| {
                u.nodes
                    .iter()
                    .filter(|(_, n)| n.live().is_some_and(|l| l != egui::accesskit::Live::Off))
                    .map(|(_, n)| n.label().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let first = live(&frames(&ctx, &mut hub, vec![]));
    assert_eq!(
        first,
        vec!["Art stopped unexpectedly, restarting".to_string()]
    );
    // Two seconds on: the countdown moved, the live region's words didn't.
    hub.panels
        .set_restart_since(SUP, std::time::Instant::now() - Duration::from_secs(2));
    let later = live(&frames(&ctx, &mut hub, vec![]));
    assert_eq!(later, first);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_stopped_card_offers_one_step_and_its_button_has_the_focus() {
    let ctx = ctx();
    let dir = temp_dir("card");
    for (step, prev, main, line, asked) in [
        ("start-again", None, "Start again", None, "start-again"),
        (
            "defaults",
            None,
            "Start with default settings",
            Some("Starting again didn't help last time."),
            "defaults",
        ),
        (
            "go-back",
            Some("0.4.2"),
            "Go back to 0.4.2",
            Some("Art 0.4.2 worked before."),
            "go-back",
        ),
        ("last", None, "Turn off Art", None, "turn-off"),
    ] {
        let (mut hub, rec) = stopped(&dir, step, prev);
        let out = frames(&ctx, &mut hub, vec![]);
        let t = drawn(&out);
        assert!(
            t.iter().any(|s| s == "\u{26A0} Stopped after 5 crashes"),
            "{t:?}"
        );
        assert!(t.iter().any(|s| s == "Art crashed 5 times in 10 minutes."));
        assert!(t.iter().any(|s| s == main), "{step}: {t:?}");
        if let Some(l) = line {
            assert!(t.iter().any(|s| s == l), "{step}: {t:?}");
        }
        // One main button: the other steps aren't offered.
        for other in ["Start again", "Start with default settings", "Turn off Art"] {
            if other != main {
                assert!(!t.iter().any(|s| s == other), "{step} offers {other}");
            }
        }
        // Copy diagnostics is last.
        let copy = text_x(&out, "Copy diagnostics").unwrap();
        assert!(copy > text_x(&out, main).unwrap(), "{step}");
        // Esc does nothing: the main button keeps the focus, and Enter presses it.
        let _ = frames(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
        assert!(rec.0.lock().unwrap().is_empty());
        assert_eq!(hub.tabs.len(), 2, "Esc closed something");
        let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(
            *rec.0.lock().unwrap(),
            vec![format!("{asked} {SUP}")],
            "{step}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_launch_failure_reads_couldnt_start() {
    let ctx = ctx();
    let dir = temp_dir("couldnt");
    let (mut hub, rec) = launch_failed(&dir, words::DIDNT_START);
    let t = drawn(&frames(&ctx, &mut hub, vec![]));
    assert!(
        t.iter().any(|s| s == "\u{26A0} Couldn't start Art"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == words::DIDNT_START));
    let i = hub.tabs.position(&TabId::panel(SUP, "art")).unwrap();
    assert_eq!(
        hub.look(i).state,
        TabState::Stopped("Couldn't start Art".into())
    );
    assert!(rec.0.lock().unwrap().is_empty());
    // Without the fact it is the install failure's words.
    hub.panels
        .set_fact(&format!("module.{SUP}.failed"), serde_json::Value::Null);
    assert_eq!(
        hub.look(i).state,
        TabState::Stopped("Couldn't install or update".into())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Never shown" (docs/hideout-supervisor-ux.md) over every string the supervisor's surfaces
/// draw: words.rs, `state_words`, the tab lines, and what the frames really drew in each state.
/// No module id, state id, error id, pid or hash.
#[test]
fn no_drawn_string_shows_an_id() {
    let ctx = ctx();
    let dir = temp_dir("never");
    let mut all = words::every_string();
    for s in [
        "starting",
        "updating",
        "restarting",
        "crash-stopped",
        "crash-looped",
        "waiting",
        "needs-hub",
        "install-failed",
        "off",
        "running",
        "revoked",
    ] {
        all.push(hideout_panels::state_words(s).into());
    }
    let mut hubs = vec![
        withdrawn(&dir, "", None, Some("0.4.2")).0,
        withdrawn(&dir, "", Some("checking"), None).0,
        withdrawn(&dir, "", Some("none"), None).0,
        withdrawn(&dir, "", Some("failed"), None).0,
        restarting(&dir, 8, 2, Duration::from_secs(1)).0,
        restarting(&dir, 1, 4, Duration::from_secs(9)).0,
        launch_failed(&dir, words::FILES_DONT_MATCH).0,
    ];
    for (step, prev) in [
        ("start-again", None),
        ("defaults", None),
        ("go-back", Some("0.4.2")),
        ("last", None),
    ] {
        hubs.push(stopped(&dir, step, prev).0);
    }
    for mut hub in hubs {
        all.extend(drawn(&frames(&ctx, &mut hub, vec![])));
        for i in 0..hub.tabs.len() {
            let l = hub.look(i);
            all.push(l.tooltip());
            all.push(l.accessible_name());
        }
    }
    let mut never: Vec<&str> = words::NEVER_SHOWN.to_vec();
    never.extend([SUP, "go-back", "start-again", "recovery", "State:"]);
    for s in &all {
        for n in &never {
            assert!(
                !s.to_lowercase().contains(&n.to_lowercase()),
                "{s:?} shows {n:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The withdrawn card (#1026, docs/hideout-revoked-ux.md): title with the version, Hideout's
/// line, the publisher's note cleaned and capped, Check for updates focused (Enter presses
/// it), Go back only with a version, Turn off, Copy diagnostics last.
#[test]
fn the_withdrawn_card_offers_its_ways_out() {
    let ctx = ctx();
    let dir = temp_dir("withdrawn");
    let note = long_note();
    let (mut hub, rec) = withdrawn(&dir, &note, None, Some("0.4.2"));
    let out = frames(&ctx, &mut hub, vec![]);
    let t = drawn(&out);
    assert!(
        t.iter().any(|s| s == "\u{26A0} Art 0.5.0 was withdrawn"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == words::WITHDRAWN_LINE));
    assert!(t.iter().any(|s| s == words::PUBLISHERS_NOTE));
    let shown = t
        .iter()
        .find(|s| s.starts_with('\u{201C}'))
        .expect("the note is drawn");
    let inner = shown
        .trim_start_matches('\u{201C}')
        .trim_end_matches('\u{201D}');
    assert_eq!(inner, words::clean_note(&note));
    assert!(inner.chars().count() <= words::NOTE_MAX && inner.ends_with('\u{2026}'));
    assert!(!inner.contains('\u{202e}') && !inner.contains('\n') && !inner.contains('\u{200b}'));
    for b in [
        "Check for updates",
        "Go back to 0.4.2",
        "Turn off Art",
        "Copy diagnostics",
    ] {
        assert!(t.iter().any(|s| s == b), "{b}: {t:?}");
    }
    // Never Start again or Turn on.
    assert!(
        !t.iter().any(|s| s == "Start again" || s == "Turn on"),
        "{t:?}"
    );
    let x = |s| text_x(&out, s).unwrap();
    assert!(x("Check for updates") < x("Go back to 0.4.2"));
    assert!(x("Go back to 0.4.2") < x("Turn off Art"));
    // The tab: the red mark and "withdrawn by its publisher".
    let art = hub.tabs.position(&TabId::panel(SUP, "art")).unwrap();
    assert!(hub.look(art).tooltip().contains(words::WITHDRAWN_TAB));
    assert_eq!(hub.look(art).shown(), crate::tabs::Shown::Stopped);
    // Esc does nothing; Enter presses the focused Check for updates.
    let _ = frames(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(rec.0.lock().unwrap().is_empty());
    let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(*rec.0.lock().unwrap(), [format!("check {SUP}")]);

    // No note: the note lines are left out. No previous version: no Go back.
    let (mut hub, _) = withdrawn(&dir, " \u{200b}\n ", None, None);
    let t = drawn(&frames(&ctx, &mut hub, vec![]));
    assert!(!t.iter().any(|s| s == words::PUBLISHERS_NOTE), "{t:?}");
    assert!(!t.iter().any(|s| s.starts_with("Go back")), "{t:?}");

    // Checking: the button reads Checking… and the status line names the source.
    let (mut hub, rec) = withdrawn(&dir, "", Some("checking"), None);
    let t = drawn(&frames(
        &ctx,
        &mut hub,
        vec![key(Key::Enter, Modifiers::NONE)],
    ));
    assert!(t.iter().any(|s| s == words::CHECKING), "{t:?}");
    assert!(
        t.iter().any(|s| s == "Checking Art's source\u{2026}"),
        "{t:?}"
    );
    assert!(!t.iter().any(|s| s == "Check for updates"));
    assert!(rec.0.lock().unwrap().is_empty(), "pressed while checking");
    for (check, line) in [
        ("none", words::NOTHING_NEWER),
        ("failed", words::COULDNT_CHECK),
    ] {
        let (mut hub, _) = withdrawn(&dir, "", Some(check), None);
        let t = drawn(&frames(&ctx, &mut hub, vec![]));
        assert!(t.iter().any(|s| s == line), "{check}: {t:?}");
    }

    // Turned off: "Off · 0.5.0 was withdrawn" and Check for updates, never Turn on.
    let (mut hub, rec) = withdrawn(&dir, &note, None, None);
    hub.panels
        .set_fact(&format!("module.{SUP}.state"), serde_json::json!("off"));
    let t = drawn(&frames(&ctx, &mut hub, vec![]));
    assert!(
        t.iter().any(|s| s == "Off \u{b7} 0.5.0 was withdrawn"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == "Check for updates"));
    assert!(
        !t.iter()
            .any(|s| s == "Turn on" || s == words::PUBLISHERS_NOTE),
        "{t:?}"
    );
    let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(*rec.0.lock().unwrap(), [format!("check {SUP}")]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The note is on the card only: never in the tab's tooltip or accessible name, and never in
/// the live announcement.
#[test]
fn the_publishers_note_stays_on_the_card() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("withdrawn-note");
    let (mut hub, _) = withdrawn(&dir, "Secret words", None, None);
    let out = frames(&ctx, &mut hub, vec![]);
    let art = hub.tabs.position(&TabId::panel(SUP, "art")).unwrap();
    let l = hub.look(art);
    assert!(!l.tooltip().contains("Secret") && !l.accessible_name().contains("Secret"));
    let live: Vec<String> = out
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|u| u.nodes.iter())
        .filter(|(_, n)| n.live().is_some_and(|l| l != egui::accesskit::Live::Off))
        .filter_map(|(_, n)| n.label().map(String::from))
        .collect();
    assert!(
        live.iter()
            .any(|s| s == "Art 0.5.0 was withdrawn by its publisher and stopped"),
        "{live:?}"
    );
    assert!(live.iter().all(|s| !s.contains("Secret")), "{live:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The check ends: Check for updates has the focus again (the disabled "Checking…" button
/// dropped it), on the card and on the Off card; never when the user moved the focus to another
/// button meanwhile (#1048 review nit 1).
#[test]
fn check_for_updates_takes_the_focus_back_when_the_check_ends() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("withdrawn-refocus");
    let check = format!("module.{SUP}.check");
    for off in [false, true] {
        let (mut hub, _) = withdrawn(&dir, "", None, None);
        if off {
            hub.panels
                .set_fact(&format!("module.{SUP}.state"), serde_json::json!("off"));
        }
        let out = frames(&ctx, &mut hub, vec![]);
        assert_eq!(
            focused_label(&out).as_deref(),
            Some("Check for updates"),
            "off {off}"
        );
        hub.panels.set_fact(&check, serde_json::json!("checking"));
        let out = frames(&ctx, &mut hub, vec![]);
        assert_ne!(
            focused_label(&out).as_deref(),
            Some("Check for updates"),
            "off {off}"
        );
        hub.panels.set_fact(&check, serde_json::json!("none"));
        let out = frames(&ctx, &mut hub, vec![]);
        assert_eq!(
            focused_label(&out).as_deref(),
            Some("Check for updates"),
            "off {off}"
        );
    }

    // Moved to Turn off while checking: it stays there when the check ends.
    let (mut hub, _) = withdrawn(&dir, "", None, None);
    let _ = frames(&ctx, &mut hub, vec![]);
    hub.panels.set_fact(&check, serde_json::json!("checking"));
    let mut out = frames(&ctx, &mut hub, vec![]);
    for _ in 0..40 {
        if focused_label(&out).as_deref() == Some("Turn off Art") {
            break;
        }
        out = frames(&ctx, &mut hub, vec![key(Key::Tab, Modifiers::NONE)]);
    }
    assert_eq!(focused_label(&out).as_deref(), Some("Turn off Art"));
    hub.panels.set_fact(&check, serde_json::json!("failed"));
    let out = frames(&ctx, &mut hub, vec![]);
    assert_eq!(focused_label(&out).as_deref(), Some("Turn off Art"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The withdrawn, Off and stopped cards are regions named by their titles (#1048 review nit 2).
#[test]
fn the_cards_are_regions_named_by_their_titles() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("card-regions");
    let regions = |out: &egui::FullOutput| -> Vec<String> {
        out.platform_output
            .accesskit_update
            .iter()
            .flat_map(|u| u.nodes.iter())
            .filter(|(_, n)| n.role() == egui::accesskit::Role::Region)
            .filter_map(|(_, n)| n.label().map(String::from))
            .collect()
    };
    let (mut hub, _) = withdrawn(&dir, "Secret words", None, None);
    let r = regions(&frames(&ctx, &mut hub, vec![]));
    assert!(r.iter().any(|s| s == "Art 0.5.0 was withdrawn"), "{r:?}");
    assert!(r.iter().all(|s| !s.contains("Secret")), "{r:?}");
    hub.panels
        .set_fact(&format!("module.{SUP}.state"), serde_json::json!("off"));
    let r = regions(&frames(&ctx, &mut hub, vec![]));
    assert!(
        r.iter().any(|s| s == "Off \u{b7} 0.5.0 was withdrawn"),
        "{r:?}"
    );
    let (mut hub, _) = stopped(&dir, "start-again", None);
    let r = regions(&frames(&ctx, &mut hub, vec![]));
    assert!(r.iter().any(|s| s == words::STOPPED), "{r:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The tray tile's withdrawn words are the supervisor's: hideout-panels can't depend on the
/// supervisor, so this keeps the two from drifting apart (#1048 review nit 3).
#[test]
fn the_tray_tile_says_the_supervisors_withdrawn_words() {
    assert_eq!(hideout_panels::tray::TILE_WITHDRAWN, words::TILE_WITHDRAWN);
}

/// Review renders (#1026): the withdrawn card with a long note, with none and no previous
/// version, while checking, after nothing newer, and turned off.
#[test]
fn renders_the_withdrawn_states_for_review() {
    let dir = temp_dir("png-withdrawn");
    let note = long_note();
    type Make<'a> = Box<dyn Fn() -> Hub + 'a>;
    let shots: Vec<(&str, Make)> = vec![
        (
            "built-withdrawn-card.png",
            Box::new(|| withdrawn(&dir, &note, None, Some("0.4.2")).0),
        ),
        (
            "built-withdrawn-no-note-no-previous.png",
            Box::new(|| withdrawn(&dir, "", None, None).0),
        ),
        (
            "built-withdrawn-checking.png",
            Box::new(|| withdrawn(&dir, &note, Some("checking"), Some("0.4.2")).0),
        ),
        (
            "built-withdrawn-nothing-newer.png",
            Box::new(|| withdrawn(&dir, &note, Some("none"), Some("0.4.2")).0),
        ),
        (
            "built-withdrawn-off.png",
            Box::new(|| {
                let (mut hub, _) = withdrawn(&dir, &note, None, None);
                hub.panels
                    .set_fact(&format!("module.{SUP}.state"), serde_json::json!("off"));
                hub
            }),
        ),
    ];
    for (name, make) in shots {
        let mut hub = make();
        let Some(rgba) = render(&mut hub, 1100, 520, |_| {}) else {
            eprintln!("no system font; withdrawn review skipped");
            return;
        };
        save_png(name, 1100, 520, rgba);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review renders (#983): the restarting page, the stopped card at each ladder step and the
/// no-previous-version last step, Couldn't start, and the tray glyphs.
#[test]
fn renders_the_supervisor_states_for_review() {
    let dir = temp_dir("png-sup");
    type Make<'a> = Box<dyn Fn() -> Hub + 'a>;
    let shots: Vec<(&str, Make)> = vec![
        (
            "built-sup-restarting.png",
            Box::new(|| restarting(&dir, 8, 2, Duration::from_millis(3500)).0),
        ),
        (
            "built-sup-restarting-0s.png",
            Box::new(|| restarting(&dir, 1, 4, Duration::from_secs(3)).0),
        ),
        (
            "built-sup-stopped-1-start-again.png",
            Box::new(|| stopped(&dir, "start-again", Some("0.4.2")).0),
        ),
        (
            "built-sup-stopped-2-defaults.png",
            Box::new(|| stopped(&dir, "defaults", Some("0.4.2")).0),
        ),
        (
            "built-sup-stopped-3-go-back.png",
            Box::new(|| stopped(&dir, "go-back", Some("0.4.2")).0),
        ),
        (
            "built-sup-stopped-3-last-no-previous.png",
            Box::new(|| stopped(&dir, "last", None).0),
        ),
        (
            "built-sup-couldnt-start.png",
            Box::new(|| launch_failed(&dir, words::FILES_DONT_MATCH).0),
        ),
    ];
    for (name, make) in shots {
        let mut hub = make();
        let Some(rgba) = render(&mut hub, 1100, 520, |_| {}) else {
            eprintln!("no system font; supervisor review skipped");
            return;
        };
        save_png(name, 1100, 520, rgba);
    }
    // The tray icon at 4x: normal, a red dot (stopped, needs a newer Hideout), amber (safe
    // mode), paused.
    let states = [
        crate::icon::State::Normal,
        crate::icon::State::NeedsYou,
        crate::icon::State::SafeMode,
        crate::icon::State::Paused,
    ];
    let (s, pad) = (128u32, 16u32);
    let w = states.len() as u32 * (s + pad) + pad;
    let h = s + 2 * pad;
    let mut img = vec![0u8; (w * h * 4) as usize];
    for p in img.chunks_mut(4) {
        p.copy_from_slice(&[0x20, 0x1d, 0x1b, 0xff]);
    }
    for (i, st) in states.iter().enumerate() {
        let g = crate::icon::rgba(s, *st);
        let x0 = pad + i as u32 * (s + pad);
        for y in 0..s {
            for x in 0..s {
                let src = &g[((y * s + x) * 4) as usize..][..4];
                let a = src[3] as u32;
                let d = &mut img[(((y + pad) * w + x0 + x) * 4) as usize..][..4];
                for k in 0..3 {
                    d[k] = ((src[k] as u32 * a + d[k] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
    }
    save_png("built-sup-tray-icons.png", w, h, img);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Esc belongs to the palette (and the close dialog) when one is open over a stopped card: it
/// closes the palette, the card's button doesn't take the focus, and Enter presses nothing.
#[test]
fn esc_over_a_stopped_card_closes_the_palette_first() {
    let ctx = ctx();
    let dir = temp_dir("card-palette");
    let (mut hub, rec) = stopped(&dir, "last", None);
    let _ = frames(&ctx, &mut hub, vec![]);
    hub.palette.toggle();
    assert!(hub.palette.open);
    let _ = frames(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(!hub.palette.open, "Esc didn't close the palette");
    assert!(rec.0.lock().unwrap().is_empty());
    // The close dialog too: Esc keeps the tab open and presses no card button.
    hub.asking = Some(vec![TabId::panel(SUP, "art")]);
    hub.panels.set_badge(
        SUP,
        &UiBadge {
            panel: "art".into(),
            badge: Badge::Dirty,
            count: None,
        },
    );
    let _ = frames(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(hub.asking.is_none(), "Esc didn't answer the dialog");
    assert_eq!(hub.tabs.len(), 2);
    assert!(
        rec.0.lock().unwrap().is_empty(),
        "{:?}",
        rec.0.lock().unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Couldn't start has one main button, Try again, with the stopped card's focus and Esc rule.
#[test]
fn couldnt_start_focuses_try_again() {
    let ctx = ctx();
    let dir = temp_dir("couldnt-focus");
    let (mut hub, rec) = launch_failed(&dir, words::FILES_DONT_MATCH);
    let out = frames(&ctx, &mut hub, vec![]);
    assert!(text_x(&out, "Try again").unwrap() < text_x(&out, "Open log").unwrap());
    let _ = frames(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(rec.0.lock().unwrap().is_empty());
    let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(*rec.0.lock().unwrap(), vec![format!("start-again {SUP}")]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Going to another page and back focuses the card's main button again.
#[test]
fn coming_back_to_a_stopped_page_focuses_its_button_again() {
    let ctx = ctx();
    let dir = temp_dir("card-return");
    let (mut hub, rec) = stopped(&dir, "start-again", None);
    let _ = frames(&ctx, &mut hub, vec![]);
    // The focus moves off the button (Tab), then the user goes to Modules and back.
    let _ = frames(&ctx, &mut hub, vec![key(Key::Tab, Modifiers::NONE)]);
    hub.open(TabId::page("modules"));
    let _ = frames(&ctx, &mut hub, vec![]);
    hub.open(TabId::panel(SUP, "art"));
    let _ = frames(&ctx, &mut hub, vec![]);
    let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(*rec.0.lock().unwrap(), vec![format!("start-again {SUP}")]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_restarting_tab_says_which_crash_and_the_sidebar_marks_a_stopped_one() {
    let ctx = ctx();
    let dir = temp_dir("tab-words");
    let (hub, _) = restarting(&dir, 8, 2, Duration::ZERO);
    let i = hub.tabs.position(&TabId::panel(SUP, "art")).unwrap();
    assert_eq!(hub.look(i).tooltip(), "Art\nRestarting\u{2026} (2 of 5)");
    // The sidebar entry of a stopped module carries the red mark and its words.
    ctx.enable_accesskit();
    let (mut hub, _) = stopped(&dir, "start-again", None);
    hub.open(TabId::page("modules"));
    let out = frames(&ctx, &mut hub, vec![]);
    assert!(drawn(&out).iter().filter(|s| *s == "\u{26A0}").count() >= 2);
    let names: Vec<String> = out
        .platform_output
        .accesskit_update
        .unwrap()
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().map(String::from))
        .collect();
    assert!(
        names.iter().any(|n| n == "Art, stopped after 5 crashes"),
        "{names:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_toast_icon_is_a_png_of_the_glyph() {
    let bytes = crate::icon::png(64, crate::icon::State::Normal);
    let img = image::load_from_memory(&bytes).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (64, 64));
    assert_eq!(
        img.into_raw(),
        crate::icon::rgba(64, crate::icon::State::Normal)
    );
}
/// Lab capture (#1002), window-free. With the switch on and nothing asked, a frame carries no
/// read-back and asks for no repaint (the capture costs nothing while unused); an asked capture
/// adds exactly one read-back to that one frame; a flyout read-back asked while the flyout is
/// closed is dropped, not kept for later.
#[test]
fn a_capture_adds_one_readback_only_to_the_frame_that_answers_it() {
    use crate::capture::{Job, Slot};
    let ctx = ctx();
    let mut hub = hub_in(&temp_dir("cap"), None);
    let slot = Arc::new(Slot::default());
    hub.capture = Some(slot.clone());
    let readbacks = |out: &egui::FullOutput| {
        out.shapes
            .iter()
            .filter(|s| matches!(s.shape, epaint::Shape::Callback(_)))
            .count()
    };
    let run = |hub: &mut Hub| ctx.run(input(800.0, 600.0, vec![]), |c| hub.frame(c));
    for _ in 0..4 {
        let _ = run(&mut hub);
    }
    let out = run(&mut hub);
    assert_eq!(readbacks(&out), 0);
    let vp = &out.viewport_output[&egui::ViewportId::ROOT];
    assert_eq!(vp.repaint_delay, std::time::Duration::MAX);

    let (tx, rx) = std::sync::mpsc::channel();
    slot.put(Job {
        main: true,
        flyout: true,
        popouts: 0,
        shots: tx,
    });
    assert_eq!(
        readbacks(&run(&mut hub)),
        1,
        "the flyout is closed: main only"
    );
    assert_eq!(
        readbacks(&run(&mut hub)),
        0,
        "a later frame reads nothing back"
    );
    // Headless frames paint no callbacks; the frame holds the only sender left, in its shape.
    drop(rx);
}

/// The read-back's PNG writer on a real hub frame: rendered headless (as the review images
/// are), passed through `capture::write_png`, read back from disk unchanged and opaque.
#[test]
fn a_rendered_frame_goes_through_the_capture_png_writer() {
    let dir = temp_dir("cappng");
    let mut hub = mock_hub(&dir);
    let (w, h) = (960u32, 540u32);
    let Some(rgba) = render(&mut hub, w, h, |_| {}) else {
        eprintln!("no system font; capture png test skipped");
        return;
    };
    let pixels = rgba
        .chunks(4)
        .map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    let image = egui::ColorImage::new([w as usize, h as usize], pixels);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("capture.png");
    crate::capture::write_png(&out, &image).unwrap();
    assert!(!dir.join("capture.png.tmp").exists());
    let back = image::open(&out).unwrap().to_rgba8();
    assert_eq!(back.dimensions(), (w, h));
    for (a, b) in back.pixels().zip(rgba.chunks(4)) {
        assert_eq!(a.0, [b[0], b[1], b[2], 255]);
    }
    // The hub's accent survives the trip.
    assert!(back.pixels().any(|p| p.0[..3] == [0xff, 0x8a, 0x2a]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[path = "popout_tests.rs"]
mod popouts;

/// Lab control (#1003) through the real hub frame, window-free: `state` lists the tabs and the
/// active page as drawn, `key ctrl+tab` goes through the hub's own shortcut code, and
/// `focus <tab>` (an AccessKit click on the tab) switches back.
#[test]
fn lab_control_drives_the_tabs_through_the_hub_frame() {
    use crate::lab_control::{self, Lab};
    struct Host(egui::Context);
    impl lab_control::Host for Host {
        fn wake(&self) {
            self.0.request_repaint();
        }
        fn arm(&self) {
            self.0.enable_accesskit();
        }
        fn windows(&self) -> (bool, bool, bool) {
            (true, false, false)
        }
        fn post_tray(&self, _: u32) -> Result<(), String> {
            Err("no tray in tests".into())
        }
        fn close_main(&self) -> Result<(), String> {
            Ok(())
        }
        fn foreground(&self) -> &'static str {
            "main"
        }
        fn menu(&self) -> Option<Vec<lab_control::MenuItem>> {
            None
        }
        fn menu_key(&self, _: u16) -> Result<(), String> {
            Ok(())
        }
        fn menu_pick(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
    }
    let ctx = ctx();
    let lab = Arc::new(Lab::new(flyout_id()));
    ctx.add_plugin(lab_control::Plugin::new(lab.clone(), || (true, false)));
    let dir = temp_dir("lab-control");
    let mut hub = hub_in(&dir, None);
    hub.open(TabId::page("settings"));
    let host = Arc::new(Host(ctx.clone()));
    let pass = |hub: &mut Hub| {
        let _ = ctx.run(input(960.0, 640.0, vec![]), |ctx| hub.frame(ctx));
    };
    // One request on a worker thread (as the pipe thread does) while this thread runs the hub's
    // passes.
    let ask = |hub: &mut Hub, line: &str| -> serde_json::Value {
        let worker = {
            let (lab, host, line) = (lab.clone(), host.clone(), line.to_string());
            std::thread::spawn(move || lab_control::answer(&line, &lab, &*host))
        };
        while !worker.is_finished() {
            pass(hub);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        pass(hub);
        let r = worker.join().unwrap();
        assert!(lab_control::answer_ok(&r), "{line}: {r}");
        serde_json::from_str(r.trim()).unwrap()
    };
    ask(&mut hub, "state");
    let s = ask(&mut hub, "state");
    let tabs: Vec<&str> = s["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(tabs.len(), 2, "{s}");
    assert!(
        s["active_page"].as_str().unwrap().starts_with("Settings"),
        "{s}"
    );
    ask(&mut hub, "key ctrl+tab");
    let s = ask(&mut hub, "state");
    assert!(
        s["active_page"].as_str().unwrap().starts_with("Modules"),
        "{s}"
    );
    ask(&mut hub, "focus Settings");
    let s = ask(&mut hub, "state");
    assert!(
        s["active_page"].as_str().unwrap().starts_with("Settings"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Modules page's rows (#983, #1029) through the real hub frame, for review: every row
/// state the supervisor UX plan's column M names with its one main button, the needs-you rows
/// first and Off last; then the same hub in safe mode with the banner. No drawn id.
#[test]
fn renders_the_modules_page_for_review() {
    use crate::modules_list::{order, page_ui, row, Installed, Row};
    struct ModulesPage(Vec<Row>, bool);
    impl Page for ModulesPage {
        fn id(&self) -> &str {
            "hub.modules"
        }
        fn title(&self) -> &str {
            "Modules"
        }
        fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens) {
            page_ui(
                ui,
                t,
                &self.0,
                self.1.then_some(hideout_supervisor::words::SAFE_MODE_LINE),
            );
        }
    }
    let facts: HashMap<String, serde_json::Value> = [
        ("module.m.beat.state", serde_json::json!("restarting")),
        ("module.m.chat.state", serde_json::json!("crash-stopped")),
        ("module.m.chat.recovery", serde_json::json!("defaults")),
        ("module.m.dock.state", serde_json::json!("install-failed")),
        ("module.m.dock.failed", serde_json::json!("launch")),
        ("module.m.lamp.state", serde_json::json!("starting")),
        ("module.m.art.state", serde_json::json!("running")),
        ("module.m.kiln.state", serde_json::json!("needs-hub")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let fact = |k: &str| facts.get(k).cloned();
    let m = |id: &str, name: &str, version: &str, enabled| Installed {
        id: id.into(),
        name: name.into(),
        version: version.into(),
        enabled,
    };
    let rows: Vec<Row> = [
        (m("m.art", "Art", "0.5.0", true), None),
        (m("m.beat", "Beat", "1.2.0", true), Some((4, 2))),
        (m("m.chat", "Chat", "0.3.1", true), None),
        (m("m.dock", "Dock", "2.0.0", true), None),
        (m("m.lamp", "Lamp", "0.9.4", true), None),
        (m("m.notes", "Notes", "0.1.0", false), None),
        (m("m.kiln", "Kiln", "1.0.0", true), None),
    ]
    .iter()
    .map(|(i, r)| row(i, &fact, *r))
    .collect();
    let mut rows = rows;
    order(&mut rows);
    for r in &rows {
        let mut drawn = vec![r.state.clone(), r.label.clone()];
        drawn.extend(r.primary.iter().map(|p| p.label.clone()));
        for s in &drawn {
            for never in hideout_supervisor::words::NEVER_SHOWN {
                assert!(!s.contains(never), "{never} drawn: {s}");
            }
            assert!(!s.contains(&r.id), "{} drawn", r.id);
        }
    }
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["Chat", "Dock", "Kiln", "Art", "Beat", "Lamp", "Notes"]
    );
    // Safe mode: nothing started, every enabled module "On · not started in safe mode".
    let safe_facts = |k: &str| (k == "hub.safe_mode").then_some(serde_json::Value::Bool(true));
    let safe_rows: Vec<Row> = [
        m("m.art", "Art", "0.5.0", true),
        m("m.beat", "Beat", "1.2.0", true),
        m("m.notes", "Notes", "0.1.0", false),
    ]
    .iter()
    .map(|i| row(i, &safe_facts, None))
    .collect();
    let dir = temp_dir("png-modules");
    let root = dir.as_os_str().to_owned();
    let paths = HubPaths::resolve(|k| (k == "HIDEOUT_CONFIG_DIR").then(|| root.clone())).unwrap();
    let shared = Arc::new(Shared {
        paths,
        prefs: Mutex::new(Prefs::default()),
        paused: AtomicBool::new(false),
        hidden: AtomicBool::new(false),
        flyout_click: Mutex::new(None),
        start_normally: None,
        quit: QuitAsk::default(),
    });
    let pages: Vec<Box<dyn Page>> = vec![Box::new(ModulesPage(rows, false))];
    let mut hub = Hub::new(shared.clone(), pages, None, None);
    let Some(rgba) = render(&mut hub, 1000, 720, |_| {}) else {
        eprintln!("no system font; render test skipped");
        return;
    };
    save_png("built-modules-page.png", 1000, 720, rgba);
    let pages: Vec<Box<dyn Page>> = vec![Box::new(ModulesPage(safe_rows, true))];
    let mut hub = Hub::new(shared, pages, None, None);
    if let Some(rgba) = render(&mut hub, 1000, 640, |_| {}) {
        save_png("built-modules-safe-mode.png", 1000, 640, rgba);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Finding the module that breaks Hideout (#1033): a search whose view the test sets, and
/// whose buttons it records.
#[derive(Default)]
struct FakeFinder {
    view: Mutex<Option<hideout_supervisor::bisect::View>>,
    acts: Mutex<Vec<hideout_supervisor::bisect::Act>>,
}

impl Finder for FakeFinder {
    fn view(&self) -> Option<hideout_supervisor::bisect::View> {
        self.view.lock().unwrap().clone()
    }
    fn act(&self, act: hideout_supervisor::bisect::Act) {
        self.acts.lock().unwrap().push(act);
    }
    fn diagnostics(&self) -> String {
        "diagnostics".into()
    }
    fn set_waker(&self, _: Box<dyn Fn() + Send + Sync>) {}
}

/// Six modules, A–Z; `steps` moves the search on (`'f'` fine, `'c'` a close and the next
/// round, `'x'` a close only, `'q'` a clean quit, `'u'` an end it can't read).
fn search_after(steps: &str) -> hideout_supervisor::bisect::Search {
    use hideout_supervisor::bisect::{entries, Candidate, Exit, Search};
    let c: Vec<Candidate> = ["Art", "Beat", "Chat", "Dock", "Echo", "Fern"]
        .iter()
        .map(|n| Candidate {
            id: format!("m.{}", n.to_lowercase()),
            name: n.to_string(),
            needs: vec![],
        })
        .collect();
    let mut s = Search::new(entries(&c)).unwrap();
    for step in steps.chars() {
        match step {
            'f' => {
                s.fine(true);
            }
            'c' => {
                s.after_exit(Exit::Unclean);
                s.start_next();
            }
            'x' => s.after_exit(Exit::Unclean),
            'q' => s.after_exit(Exit::Clean),
            'u' => s.after_exit(Exit::Unknown),
            _ => unreachable!(),
        }
    }
    s
}

fn fake_finder(
    s: &hideout_supervisor::bisect::Search,
    seq: u64,
    previous: Option<&str>,
) -> Arc<FakeFinder> {
    let left =
        (s.phase == hideout_supervisor::bisect::Phase::Running).then_some(Duration::from_secs(239));
    let mut v = hideout_supervisor::bisect::view(s, left, previous);
    v.seq = seq;
    Arc::new(FakeFinder {
        view: Mutex::new(Some(v)),
        ..Default::default()
    })
}

/// The accessible tree's focused node's label.
fn focused_label(out: &egui::FullOutput) -> Option<String> {
    let u = out.platform_output.accesskit_update.as_ref()?;
    u.nodes
        .iter()
        .find(|(id, _)| *id == u.focus)
        .and_then(|(_, n)| n.label().map(String::from))
}

/// The bar is a landmark region named for the search and first in tab order (before the
/// sidebar); its words are said once in the polite live region; Tab then Enter is It's fine;
/// a round's bar takes no focus by itself (#1033, "Keyboard and screen reader").
#[test]
fn the_search_bar_is_a_landmark_first_in_tab_order_and_said_once() {
    use hideout_supervisor::bisect::{words as w, Act};
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("bisect-bar");
    let mut hub = hub_in(&dir, None);
    let f = fake_finder(&search_after("f"), 7, None);
    hub.search = Some(f.clone());
    let out = frames(&ctx, &mut hub, vec![]);
    let nodes = &out.platform_output.accesskit_update.as_ref().unwrap().nodes;
    assert!(
        nodes.iter().any(|(_, n)| n.role() == egui::accesskit::Role::Region
            && n.label() == Some(w::REGION)),
        "no landmark region"
    );
    let live: Vec<String> = nodes
        .iter()
        .filter(|(_, n)| n.live() == Some(egui::accesskit::Live::Polite))
        .filter_map(|(_, n)| n.label().map(String::from))
        .collect();
    assert_eq!(
        live,
        ["Round 1 was fine. Round 2 of 4 started. Dock and Echo are on."]
    );
    let t = drawn(&out);
    for s in [
        "Finding the module that breaks Hideout \u{b7} Round 2 of 4",
        "2 of 6 modules are on: Dock, Echo.",
        "This round ends in 4 min if Hideout stays open.",
    ] {
        assert!(t.iter().any(|x| x == s), "{s} not drawn: {t:?}");
    }
    assert_ne!(focused_label(&out).as_deref(), Some(w::ITS_FINE));
    // The first Tab lands on It's fine, before the sidebar's pages.
    let out = ctx.run(
        input(1100.0, 640.0, vec![key(Key::Tab, Modifiers::NONE)]),
        |c| hub.frame(c),
    );
    assert_eq!(focused_label(&out).as_deref(), Some(w::ITS_FINE));
    let out = ctx.run(
        input(1100.0, 640.0, vec![key(Key::Tab, Modifiers::NONE)]),
        |c| hub.frame(c),
    );
    assert_eq!(focused_label(&out).as_deref(), Some(w::IT_CLOSED));
    // Shift+Tab back, Enter: It's fine.
    let _ = ctx.run(
        input(1100.0, 640.0, vec![key(Key::Tab, Modifiers::SHIFT)]),
        |c| hub.frame(c),
    );
    let _ = ctx.run(
        input(1100.0, 640.0, vec![key(Key::Enter, Modifiers::NONE)]),
        |c| hub.frame(c),
    );
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert_eq!(*f.acts.lock().unwrap(), [Act::Fine]);
    // The same view again is not said again.
    let before = hub.announce.as_ref().map(|a| a.n);
    let _ = frames(&ctx, &mut hub, vec![]);
    assert_eq!(hub.announce.as_ref().map(|a| a.n), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The between-rounds card, the questions and the result take the focus on their primary
/// button when they appear; Esc does nothing; Enter is the primary (#1033).
#[test]
fn a_search_card_focuses_its_primary_and_esc_does_nothing() {
    use hideout_supervisor::bisect::Act;
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("bisect-card");
    for (steps, primary, act) in [
        ("x", "Start round 2", Act::StartNext),
        ("q", "Go on with round 1", Act::GoOn),
        ("u", "It closed", Act::Closed),
        ("ccx", "Turn off Art", Act::TurnOff),
        ("ffff", "Start normally", Act::StartNormally),
        ("xx", "Copy diagnostics", Act::CopyDiagnostics),
    ] {
        let mut hub = hub_in(&dir, None);
        let s = search_after(steps);
        let f = fake_finder(&s, 3, None);
        hub.search = Some(f.clone());
        let out = frames(&ctx, &mut hub, vec![]);
        assert_eq!(
            focused_label(&out).as_deref(),
            Some(primary),
            "{steps}: {s:?}"
        );
        let out = ctx.run(
            input(1100.0, 640.0, vec![key(Key::Escape, Modifiers::NONE)]),
            |c| hub.frame(c),
        );
        assert_eq!(
            focused_label(&out).as_deref(),
            Some(primary),
            "{steps}: Esc"
        );
        assert!(f.acts.lock().unwrap().is_empty(), "{steps}: Esc acted");
        let _ = ctx.run(
            input(1100.0, 640.0, vec![key(Key::Enter, Modifiers::NONE)]),
            |c| hub.frame(c),
        );
        let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
        assert_eq!(*f.acts.lock().unwrap(), [act], "{steps}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The search through the real hub frame, for review (#1033, docs/hideout-safe-mode-ux.md):
/// the banner with the link, a round (bar, Modules rows, the lock line), between rounds, the
/// paused and asked questions and the three results.
#[test]
fn renders_the_search_for_review() {
    use crate::modules_list::{
        order, page_ui_with, row, search_words, Banner, Find, Installed, Row, Top,
    };
    use hideout_supervisor::bisect::{words as w, Search};
    struct ModulesPage(Vec<Row>, Top<'static>);
    impl Page for ModulesPage {
        fn id(&self) -> &str {
            "hub.modules"
        }
        fn title(&self) -> &str {
            "Modules"
        }
        fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens) {
            page_ui_with(ui, t, &self.0, self.1);
        }
    }
    let safe = |k: &str| (k == "hub.safe_mode").then_some(serde_json::Value::Bool(true));
    let rows_for = |s: Option<&Search>| {
        let mut rows: Vec<Row> = ["Art", "Beat", "Chat", "Dock", "Echo", "Fern", "Notes"]
            .iter()
            .map(|n| {
                let m = Installed {
                    id: format!("m.{}", n.to_lowercase()),
                    name: n.to_string(),
                    version: "1.0.0".into(),
                    enabled: *n != "Notes",
                };
                let mut r = row(&m, &safe, None);
                if let Some(s) = s.filter(|s| s.active()) {
                    let on = s
                        .all_ids()
                        .contains(&m.id)
                        .then(|| s.on_ids().contains(&m.id));
                    search_words(&mut r, on);
                }
                r
            })
            .collect();
        order(&mut rows);
        rows
    };
    let dir = temp_dir("png-bisect");
    let shots: Vec<(&str, Option<Search>, Option<&str>)> = vec![
        ("built-bisect-banner.png", None, None),
        ("built-bisect-round.png", Some(search_after("f")), None),
        ("built-bisect-between.png", Some(search_after("x")), None),
        ("built-bisect-paused.png", Some(search_after("q")), None),
        ("built-bisect-asked.png", Some(search_after("u")), None),
        (
            "built-bisect-found.png",
            Some(search_after("ccx")),
            Some("0.4.2"),
        ),
        (
            "built-bisect-not-found.png",
            Some(search_after("ffff")),
            None,
        ),
        (
            "built-bisect-hideout-itself.png",
            Some(search_after("xx")),
            None,
        ),
    ];
    for (name, s, previous) in shots {
        let top = match &s {
            None => Top {
                banner: Some(Banner {
                    line: hideout_supervisor::words::SAFE_MODE_LINE,
                    note: Some(w::STOPPED),
                    find: Find::Link(w::FIND),
                }),
                locked: false,
            },
            Some(s) => Top {
                banner: None,
                locked: s.active(),
            },
        };
        let mut hub = hub_in(&dir, None);
        hub.pages = vec![Box::new(ModulesPage(rows_for(s.as_ref()), top))];
        hub.tabs = Tabs::default();
        hub.tabs.open(TabId::page("hub.modules"));
        if let Some(s) = &s {
            hub.search = Some(fake_finder(s, 1, previous));
        }
        let Some(rgba) = render(&mut hub, 1000, 760, |_| {}) else {
            eprintln!("no system font; render test skipped");
            return;
        };
        save_png(name, 1000, 760, rgba);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
