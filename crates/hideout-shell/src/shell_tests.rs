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
    hub_with_pages(
        dir,
        feed,
        vec![
            Box::new(FakePage("modules", "Modules")),
            Box::new(FakePage("settings", "Settings")),
        ],
    )
}

fn hub_with_pages(
    dir: &std::path::Path,
    feed: Option<Box<dyn PanelFeed>>,
    pages: Vec<Box<dyn Page>>,
) -> Hub {
    let root = dir.as_os_str().to_owned();
    let paths = HubPaths::resolve(|k| (k == "HIDEOUT_CONFIG_DIR").then(|| root.clone())).unwrap();
    let shared = Arc::new(Shared {
        paths,
        prefs: Mutex::new(Prefs::default()),
        paused: AtomicBool::new(false),
        hidden: AtomicBool::new(false),
        flyout_click: Mutex::new(None),
        flyout_dismiss: AtomicBool::new(false),
        start_normally: None,
        quit: QuitAsk::default(),
    });
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
    events: Vec<Vec<Event>>,
    draw: impl FnMut(&egui::Context),
) -> Option<Vec<u8>> {
    rasterise_in(ThemeChoice::Dark, w, h, events, draw)
}

/// [`rasterise_events`] on the given theme.
fn rasterise_in(
    theme: ThemeChoice,
    w: u32,
    h: u32,
    mut events: Vec<Vec<Event>>,
    mut draw: impl FnMut(&egui::Context),
) -> Option<Vec<u8>> {
    hideout_panels::render::test_fonts()?;
    let ctx = egui::Context::default();
    // The shell's own fonts (Segoe UI plus Segoe UI Symbol for the marks), as the hub draws.
    install_fonts(&ctx);
    set_theme(&ctx, theme);
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
            // A module's tiles are the ones its panels declare (ids are its own).
            let tiles: Vec<PanelKey> = host
                .panels()
                .iter()
                .filter(|p| p.key.module == id && p.slot == hideout_panels::Slot::Tile)
                .map(|p| p.key.clone())
                .collect();
            for key in tiles {
                row.push(key);
                if row.len() == 4 {
                    rows.push(std::mem::take(&mut row));
                }
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
            extra.push(PanelKey::new(&id, first.panel.clone()));
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

/// #997 / #1076 (design review of #1080): a double click on the tray icon takes the flyout away
/// in its own handler, before Hideout shows. An open not drawn yet never appears; one already
/// shown is gone by the next frame and doesn't read as closed by focus (so the next single
/// click opens it again at once). Flyout open, then a double click: the first click closes it,
/// the double click keeps it closed, and it doesn't come back.
#[test]
fn a_double_click_takes_the_flyout_away_before_hideout_shows() {
    let ctx = ctx();
    let mut hub = hub_in(&temp_dir("flyout-dbl"), None);
    hub.shared.hidden.store(true, Ordering::Release);
    quiet_frame(&ctx, &mut hub, vec![]);
    // The first up's open, not drawn yet, then the double click.
    tray_click(&hub);
    on_tray(tray::Command::DoubleClick, &hub.shared, &ctx);
    assert!(
        hub.shared.flyout_click.lock().unwrap().is_none(),
        "the pending open survived the double click"
    );
    for _ in 0..3 {
        quiet_frame(&ctx, &mut hub, vec![]);
        assert!(
            hub.flyout.is_none(),
            "a flyout was drawn after the double click"
        );
    }
    // Shown already (a slow double click): gone at the next frame, not a focus close.
    hub.shared.hidden.store(true, Ordering::Release);
    tray_click(&hub);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(hub.flyout.is_some());
    on_tray(tray::Command::DoubleClick, &hub.shared, &ctx);
    for _ in 0..3 {
        quiet_frame(&ctx, &mut hub, vec![]);
        assert!(
            hub.flyout.is_none(),
            "the shown flyout stayed after the double click"
        );
    }
    assert!(hub.flyout_closed_by_focus.is_none());
    // A single click afterwards opens it at once.
    tray_click(&hub);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(
        hub.flyout.is_some(),
        "the next single click didn't open the flyout"
    );
    // Flyout open, then a double click: the first click closes it, the double click opens
    // Hideout, and the flyout stays closed.
    tray_click(&hub);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert!(hub.flyout.is_none());
    on_tray(tray::Command::DoubleClick, &hub.shared, &ctx);
    for _ in 0..3 {
        quiet_frame(&ctx, &mut hub, vec![]);
        assert!(
            hub.flyout.is_none(),
            "the flyout came back after the double click"
        );
    }
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
        flyout_dismiss: AtomicBool::new(false),
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
    fn update_hub(&mut self) {
        self.0.lock().unwrap().push("update-hub".into());
    }
    fn update_hub_for(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("update-hub {m}"));
    }
    fn force_update(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("force {m}"));
    }
    fn not_now(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("not-now {m}"));
    }
    fn quarantine(&mut self, m: &str) {
        self.0.lock().unwrap().push(format!("quarantine {m}"));
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
                serde_json::json!("Art stopped unexpectedly. Restarting in 4 s, try 1 of 5."),
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
            .any(|s| s == "Art stopped unexpectedly. Restarting in 6 s, try 3 of 5."),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == words::PLACE_KEPT));
    assert!(t.iter().any(|s| s == words::OPEN_LOG));
    assert!(!t.iter().any(|s| s.contains("Restart now")));
    // Neither the frozen reason nor the greyed banner is drawn.
    assert!(!t.iter().any(|s| s.contains("in 4 s, try 1 of 5")), "{t:?}");
    assert!(!t.iter().any(|s| s.starts_with("Art: ")), "{t:?}");
    // Repainted once a second while shown.
    assert!(repaint_delay(&out) <= Duration::from_secs(1));

    // At 0: "Restarting…", and no more repaints.
    let (mut hub, _) = restarting(&dir, 2, 3, Duration::from_secs(5));
    let out = frames(&ctx, &mut hub, vec![]);
    assert!(
        drawn(&out)
            .iter()
            .any(|s| s == "Art stopped unexpectedly. Restarting, try 4 of 5."),
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
    assert_eq!(hub.look(i).tooltip(), "Art\nRestarting, try 3 of 5");
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

#[path = "drag_tests.rs"]
mod drags;

// Waiting, needs-hub, quarantined and the update states on every surface (#983, #1049).
#[path = "states_tests.rs"]
mod states_surfaces;

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
        adopted: false,
        dev: false,
        dev_folder: None,
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
        flyout_dismiss: AtomicBool::new(false),
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

/// Finding the app that breaks Hideout (#1033): a search whose view the test sets, and
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
        "Finding the app that breaks Hideout \u{b7} Round 2 of 4",
        "2 of 6 apps are on: Dock, Echo.",
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
                    adopted: false,
                    dev: false,
                    dev_folder: None,
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
                remove: None,
                can_remove: true,
            },
            Some(s) => Top {
                banner: None,
                locked: s.active(),
                remove: None,
                can_remove: true,
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

/// Set key (#991, #1087): while a Keys row waits for a key, the window's key presses are its:
/// Ctrl+W is refused (one modifier; it stays the apps') rather than closing the tab, and
/// Ctrl+Shift+W binds and is saved in `hideout.toml`'s `[keys]`. Esc ends the next capture.
#[test]
fn set_key_takes_the_press_from_the_window_and_saves_it() {
    let ctx = ctx();
    let dir = temp_dir("set-key");
    let mut hub = hub_in(&dir, None);
    hub.panels.set_manifest(keys_manifest());
    quiet_frame(&ctx, &mut hub, vec![]);
    let tabs = hub.tabs.len();
    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    hub.panels.start_capture(SUP, "toggle");
    quiet_frame(&ctx, &mut hub, vec![key(Key::W, ctrl)]);
    assert_eq!(
        hub.tabs.len(),
        tabs,
        "Ctrl+W went to the row; no tab closed"
    );
    assert_eq!(hub.panels.key(SUP, "toggle"), None);
    assert_eq!(
        hub.panels.key_note(SUP, "toggle"),
        Some(hideout_panels::keys::NEEDS_MODIFIER)
    );
    quiet_frame(&ctx, &mut hub, vec![key(Key::W, ctrl | Modifiers::SHIFT)]);
    assert_eq!(hub.panels.key(SUP, "toggle"), Some("Ctrl+Shift+W"));
    assert_eq!(hub.panels.capturing(), None);
    let saved = Prefs::load(&hub.shared.paths.prefs_file()).0;
    assert_eq!(
        saved.keys.get(&format!("{SUP}/toggle")).map(String::as_str),
        Some("Ctrl+Shift+W")
    );
    hub.panels.start_capture(SUP, "toggle");
    quiet_frame(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert_eq!(hub.panels.capturing(), None);
    assert_eq!(hub.panels.key(SUP, "toggle"), Some("Ctrl+Shift+W"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A module with one action, for the Set key tests.
fn keys_manifest() -> hideout_contract::module::Module {
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

[[actions]]
id = "toggle"
name = "Toggle it"
"#
    );
    hideout_contract::module::Module::parse(&toml, &Default::default()).unwrap()
}

/// The capture ends when its row isn't on screen (#1087): a frame with another page up (the
/// module's page isn't drawn) ends it, so Ctrl+K and text fields work again.
#[test]
fn a_capture_ends_when_its_row_is_not_drawn() {
    let ctx = ctx();
    let dir = temp_dir("set-key-off");
    let mut hub = hub_in(&dir, None);
    hub.panels.set_manifest(keys_manifest());
    quiet_frame(&ctx, &mut hub, vec![]);
    hub.panels.start_capture(SUP, "toggle");
    // The active tab is a hub page, not the module's: the row isn't drawn. (The frame that
    // starts a capture counts as drawn: Set key was clicked in it.)
    quiet_frame(&ctx, &mut hub, vec![]);
    quiet_frame(&ctx, &mut hub, vec![]);
    assert_eq!(hub.panels.capturing(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review image of the Keys rows (#991): a bound key with Change and Remove, Set key, and a row
/// waiting for a key that was just refused, on the first real module with three actions.
#[test]
fn renders_the_keys_rows_for_review() {
    let (mut host, ids) = review_modules();
    let Some(id) = ids
        .iter()
        .find(|id| host.manifest(id).is_some_and(|m| m.actions.len() >= 3))
        .cloned()
    else {
        return;
    };
    let actions: Vec<String> = host
        .manifest(&id)
        .unwrap()
        .actions
        .iter()
        .map(|a| a.decl.id.clone())
        .collect();
    host.set_key(&id, &actions[0], Some("Ctrl+Shift+M".into()));
    host.start_capture(&id, &actions[2]);
    host.capture_press(hideout_panels::keys::Chord::press(Key::M, Modifiers::CTRL));
    host.set_fact(&format!("module.{id}.state"), serde_json::json!("running"));
    let Some(page) = host.navigate(&id, "page") else {
        return;
    };
    let (w, h) = (640u32, 260u32);
    let rgba = rasterise(w, h, |c| {
        egui::CentralPanel::default().show(c, |ui| {
            hideout_panels::page_sections_ui(&mut host, ui, &page)
        });
    })
    .unwrap();
    save_png("built-keys-rows.png", w, h, rgba);
}

/// Review image of the Settings page (#1087): Privacy with Forget seen apps (pressed, so its
/// line shows), then About.
#[test]
fn renders_the_settings_page_for_review() {
    let t = theme::tokens(true);
    let (w, h) = (720u32, 300u32);
    let rgba = rasterise(w, h, |c| {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(t.bg.0, t.bg.1, t.bg.2))
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(c, |ui| {
                crate::settings_view::settings_ui(ui, &t, "0.3.0", true);
            });
    })
    .unwrap();
    save_png("built-settings-page.png", w, h, rgba);
}

/// The Start with Windows section with its dialog open, for the UX review (H5.2).
#[test]
fn renders_the_start_with_windows_dialog_for_review() {
    let t = theme::tokens(true);
    let (w, h) = (720u32, 600u32);
    for (status, replaces, apps, file) in [
        (
            crate::startup_switch::STATUS_OFF,
            false,
            false,
            "built-settings-start-with-windows.png",
        ),
        (
            crate::startup_switch::STATUS_FOREIGN,
            true,
            false,
            "built-settings-start-with-windows-replaces.png",
        ),
        (
            crate::startup_switch::STATUS_OFF,
            false,
            true,
            "built-settings-start-with-windows-apps.png",
        ),
        (
            crate::startup_switch::STATUS_FOREIGN,
            true,
            true,
            "built-settings-start-with-windows-apps-replaces.png",
        ),
    ] {
        let row = crate::startup_switch::Row {
            status: status.to_string(),
            apps,
            dialog: Some(replaces),
            ..Default::default()
        };
        let rgba = rasterise(w, h, |c| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(t.bg.0, t.bg.1, t.bg.2))
                        .inner_margin(egui::Margin::same(24)),
                )
                .show(c, |ui| {
                    crate::settings_view::settings_ui_with(ui, &t, "0.3.0", false, Some(&row));
                });
        })
        .unwrap();
        save_png(file, w, h, rgba);
    }
}

/// The keyboard grant's dialog (#1155-B), window-free: Esc answers Not now, no key answers
/// nothing, Enter on the opening focus answers Not now (it never allows by itself).
#[test]
fn the_grant_dialog_answers_not_now_to_esc_and_enter() {
    use crate::keyboard_grant::{Answer, Dialog};
    let d = Dialog::open(r"C:\Program Files\Foxglove\foxglove.exe", false);
    let t = crate::theme::tokens(true);
    let run = |frames: Vec<Vec<Event>>| {
        let ctx = egui::Context::default();
        let mut answers = Vec::new();
        for (i, ev) in frames.into_iter().enumerate() {
            let _ = ctx.run(input(800.0, 600.0, ev), |c| {
                answers.push(crate::keyboard_grant_ui::dialog_ui(c, &t, &d, i == 0));
            });
        }
        answers
    };
    let none = run(vec![vec![], vec![], vec![]]);
    assert!(none.iter().all(|a| a.is_none()), "{none:?}");
    let esc = run(vec![vec![], vec![key(Key::Escape, Modifiers::NONE)]]);
    assert_eq!(esc.last().unwrap(), &Some(Answer::NotNow));
    let enter = run(vec![vec![], vec![], vec![key(Key::Enter, Modifiers::NONE)]]);
    assert!(
        enter.iter().all(|a| *a != Some(Answer::Allow)),
        "Enter must never allow: {enter:?}"
    );
}

/// Tab stays inside the dialog: from Not now to Allow and back, never out to the page behind;
/// the dialog is a Dialog node named by its title.
#[test]
fn the_grant_dialog_keeps_tab_inside_and_is_named() {
    use crate::keyboard_grant::{Dialog, DIALOG_TITLE};
    let d = Dialog::open(r"C:\Program Files\Foxglove\foxglove.exe", false);
    let t = crate::theme::tokens(true);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let tab = key(Key::Tab, Modifiers::NONE);
    let mut ids = [egui::Id::NULL; 2];
    let mut first_focus = None;
    let mut seen = std::collections::HashSet::new();
    for i in 0..8 {
        let ev = if i < 2 { vec![] } else { vec![tab.clone()] };
        let out = ctx.run(input(800.0, 600.0, ev), |c| {
            // A button behind the dialog, which Tab must not reach.
            egui::CentralPanel::default().show(c, |ui| {
                let _ = ui.button("behind");
            });
            ids = crate::keyboard_grant_ui::dialog_ids(c, &t, &d, i == 0).1;
        });
        let f = ctx.memory(|m| m.focused());
        if i == 1 {
            first_focus = f;
        }
        if i >= 1 {
            assert!(
                f.is_some_and(|f| ids.contains(&f)),
                "frame {i}: focus left the dialog: {f:?} not in {ids:?}"
            );
            seen.insert(f);
        }
        let dialog = out.platform_output.accesskit_update.as_ref().and_then(|u| {
            u.nodes
                .iter()
                .find(|(_, n)| n.role() == egui::accesskit::Role::Dialog)
                .map(|(_, n)| n.label().map(str::to_owned))
        });
        if i >= 1 {
            assert_eq!(dialog, Some(Some(DIALOG_TITLE.to_string())));
        }
    }
    assert_eq!(first_focus, Some(ids[1]), "Not now is focused on open");
    assert_eq!(seen.len(), 2, "Tab reaches both buttons: {seen:?}");
}

/// Review images of the Done card (first-run FR4): running with a keyboard, running with none,
/// stopped, with and without the offers, both themes, wide and narrow (`HIDEOUT_REVIEW_DIR`,
/// files `built-done-card-*.png`).
#[test]
fn renders_the_done_card_for_review() {
    use crate::done_card::{card_ui, Done, Inputs};
    use serde_json::{json, Value};
    let with_kb = |k: &str| -> Option<Value> {
        match k {
            "module.foxglove.state" => Some(json!("running")),
            "keyboard.connected" => Some(json!(true)),
            "keyboard.name" => Some(json!("Eyelash Corne")),
            "keyboard.layer_name" => Some(json!("Base")),
            "keyboard.left.battery" => Some(json!(82)),
            "keyboard.right.battery" => Some(json!(91)),
            _ => None,
        }
    };
    let no_kb = |k: &str| -> Option<Value> {
        match k {
            "module.foxglove.state" => Some(json!("running")),
            "keyboard.connected" => Some(json!(false)),
            _ => None,
        }
    };
    let stopped = |k: &str| -> Option<Value> {
        (k == "module.foxglove.state").then(|| json!("crash-stopped"))
    };
    let mut done = Done::default();
    done.show_for("foxglove");
    for (dark, name) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        for (w, tag) in [(760u32, "wide"), (400u32, "narrow")] {
            let h = 760u32;
            let Some(rgba) = rasterise(w, h, |ctx| {
                set_theme(ctx, choice);
                egui::CentralPanel::default().show(ctx, |ui| {
                    for (f, identify, tail) in [
                        (
                            &with_kb as &dyn Fn(&str) -> Option<Value>,
                            Some("Show the lights"),
                            true,
                        ),
                        (&no_kb, None, true),
                        (&stopped, None, false),
                    ] {
                        let mut i = Inputs {
                            name: "Foxglove",
                            facts: f,
                            identify: identify.map(String::from),
                            hint: Some("Press a layer key to see the layer overlay.".into()),
                            has_page: true,
                            sharing_open: tail,
                            more: if tail {
                                vec![("vulpes".into(), "Vulpes".into())]
                            } else {
                                Vec::new()
                            },
                        };
                        i.sharing_open = tail;
                        if let Some(c) = done.card(&i) {
                            card_ui(ui, &t, &c);
                        }
                        ui.add_space(14.0);
                    }
                });
            }) else {
                eprintln!("no system font; render test skipped");
                return;
            };
            save_png(&format!("built-done-card-{tag}-{name}.png"), w, h, rgba);
        }
    }
}

/// Review images of the keyboard grant (#1155-B): the dialog (plain and after a change) and the
/// header lines and Permissions rows of every state, both themes (`HIDEOUT_REVIEW_DIR`).
#[test]
fn renders_the_keyboard_grant_for_review() {
    use crate::keyboard_grant::{header, row, Dialog, Situation};
    use crate::keyboard_grant_ui::{dialog_ui, header_ui, row_ui};
    let at = "2026-11-12T10:00:00Z".to_string();
    let states = vec![
        Situation::NotAsked {
            started_without_act: true,
        },
        Situation::Granted { at: at.clone() },
        Situation::Declined,
        Situation::Revoked,
        Situation::Changed { at, moved: false },
    ];
    let path = r"C:\Program Files\Foxglove\foxglove.exe";
    for (dark, name) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        let (w, h) = (720u32, 900u32);
        let Some(rgba) = rasterise(w, h, |ctx| {
            set_theme(ctx, choice);
            egui::CentralPanel::default().show(ctx, |ui| {
                for s in &states {
                    if let Some(hd) = header(s) {
                        header_ui(ui, &t, &hd);
                        ui.add_space(6.0);
                    }
                    if let Some(r) = row(s) {
                        row_ui(ui, &t, &r);
                    }
                    ui.add_space(18.0);
                }
            });
        }) else {
            eprintln!("no system font; render test skipped");
            return;
        };
        save_png(&format!("built-keyboard-grant-rows-{name}.png"), w, h, rgba);
        let long = r"C:\Users\someone-with-a-long-name\AppData\Local\Programs\Bandit Fox Labs\Foxglove\current-release\bin\x86_64-pc-windows-msvc\foxglove.exe";
        for (after_change, long_path, tag) in [
            (false, false, "dialog"),
            (true, false, "dialog-changed"),
            (false, true, "dialog-long-path"),
        ] {
            let d = Dialog::open(if long_path { long } else { path }, after_change);
            let (w, h) = (720u32, 560u32);
            let rgba = rasterise(w, h, |ctx| {
                set_theme(ctx, choice);
                // No fade-in: the picture is the settled dialog, over the page behind it.
                ctx.style_mut(|s| s.animation_time = 0.0);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label(egui::RichText::new("Modules").size(24.0).strong());
                    ui.add_space(12.0);
                    if let Some(r) = row(&Situation::Granted {
                        at: "2026-11-12T10:00:00Z".into(),
                    }) {
                        row_ui(ui, &t, &r);
                    }
                });
                dialog_ui(ctx, &t, &d, true);
            })
            .unwrap();
            save_png(
                &format!("built-keyboard-grant-{tag}-{name}.png"),
                w,
                h,
                rgba,
            );
        }
    }
}

/// Review images of the module permission dialog (#1178): the pending list, the list that
/// changed while it was open, a voided grant, and the granted state with its header line and
/// Permissions section, both themes (`HIDEOUT_REVIEW_DIR`, files `built-permission-*.png`).
#[test]
fn renders_the_permission_dialog_for_review() {
    use crate::permission_dialog::{
        header, Group, HeaderKind, ItemWord, KeyboardChoice, ListItem, PermDialog, Reason, Section,
        SectionItem,
    };
    use crate::permission_dialog_ui::{perm_dialog_ui, perm_header_ui, section_ui};

    // The 16 `foxglove` lines (#1238: with the three wallpaper-engine ones), a sensitive one and
    // one that can't be allowed.
    let rows: [(&str, &str, Group); 16] = [
        (
            "facts.write:keyboard.*",
            "Share all of your keyboard's status (layer, battery, lights and more)",
            Group::ShareStatus,
        ),
        (
            "facts.read:module.*",
            "See every app Hideout runs and whether it is running",
            Group::ShareStatus,
        ),
        (
            "provide:health.read",
            "The keyboard's health report",
            Group::AnswerQuestions,
        ),
        (
            "provide:lights.page",
            "What the Lights page shows",
            Group::AnswerQuestions,
        ),
        (
            "provide:scenes.page",
            "Which scenes and wallpaper looks are listed",
            Group::AnswerQuestions,
        ),
        (
            "provide:overlay.window",
            "Where the layer overlay window is",
            Group::AnswerQuestions,
        ),
        (
            "provide:overlay.shown",
            "Whether the layer overlay is on",
            Group::AnswerQuestions,
        ),
        (
            "provide:tray.panel",
            "Whether the tray panel is open",
            Group::AnswerQuestions,
        ),
        (
            "provide:pc_lighting.list",
            "Your PC's lighting zones",
            Group::AnswerQuestions,
        ),
        (
            "provide:overlay.restore",
            "Bring the layer overlay back on screen",
            Group::MakeChanges,
        ),
        (
            "provide:overlay.set_shown",
            "Turn the layer overlay on or off",
            Group::MakeChanges,
        ),
        (
            "provide:tray.set_panel",
            "Open or close the tray panel",
            Group::MakeChanges,
        ),
        (
            "provide:pc_lighting.set_zone",
            "Change a PC lighting zone's colour",
            Group::MakeChanges,
        ),
        (
            "provide:wallpaper-engine.pause",
            "Pause your live wallpaper",
            Group::MakeChanges,
        ),
        (
            "provide:wallpaper-engine.release",
            "Let your live wallpaper play again",
            Group::MakeChanges,
        ),
        (
            "provide:wallpaper-engine.status",
            "Whether your live wallpaper is paused",
            Group::AnswerQuestions,
        ),
    ];
    let dialog = |reason: Option<Reason>, extra: bool| {
        let mut items: Vec<ListItem> = rows
            .iter()
            .map(|(raw, line, g)| ListItem {
                raw: (*raw).into(),
                line: (*line).into(),
                group: *g,
                allowable: true,
                sensitive: false,
                reason: None,
                checked: true,
                confirmed: false,
                new: false,
                before: false,
            })
            .collect();
        if extra {
            items.push(ListItem {
                raw: "key.events".into(),
                line: "See every key you press".into(),
                group: Group::Sensitive,
                allowable: true,
                sensitive: true,
                reason: None,
                checked: false,
                confirmed: false,
                new: false,
                before: false,
            });
            items.push(ListItem {
                raw: "modules.manage".into(),
                line: "Turn other modules on and off".into(),
                group: Group::CantBeAllowed,
                allowable: false,
                sensitive: false,
                reason: Some("Hideout never allows this for this module.".into()),
                checked: false,
                confirmed: false,
                new: false,
                before: false,
            });
        }
        items.sort_by_key(|i| (i.group, i.raw.clone()));
        PermDialog {
            id: "foxglove".into(),
            name: "Foxglove".into(),
            code: "7f3a91c20b4e55d1".into(),
            path: r"C:\Program Files\Foxglove\foxglove.exe".into(),
            items,
            reason,
            keyboard: Some(KeyboardChoice {
                on: false,
                expanded: false,
            }),
            keep: None,
            removed: Vec::new(),
            same_place: false,
            view: Default::default(),
            error: None,
        }
    };
    // Every item the list has: the status item and the sensitive one Off, one that can't be
    // allowed, the rest Allowed (15 allowed, as the command-line header says).
    let section_items = || {
        let mut v: Vec<SectionItem> = rows
            .iter()
            .enumerate()
            .map(|(i, (raw, line, g))| SectionItem {
                raw: (*raw).into(),
                line: (*line).into(),
                group: *g,
                word: if i == 0 {
                    ItemWord::Off
                } else {
                    ItemWord::Allowed
                },
            })
            .chain([
                SectionItem {
                    raw: "key.events".into(),
                    line: "See every key you press".into(),
                    group: Group::Sensitive,
                    word: ItemWord::Off,
                },
                SectionItem {
                    raw: "modules.manage".into(),
                    line: "Turn other modules on and off".into(),
                    group: Group::CantBeAllowed,
                    word: ItemWord::CantBeAllowed,
                },
            ])
            .collect();
        v.sort_by_key(|i| (i.group, i.raw.clone()));
        v
    };
    let section = || Section {
        id: "foxglove".into(),
        name: "Foxglove".into(),
        items: section_items(),
        allowed_on: Some("Allowed from the command line on 13 Nov 2026".into()),
        history: vec!["Updated to 0.3.1 on 14 Nov 2026. Your OK stayed.".into()],
        can_change: true,
        kept_void: None,
        keep: None,
        note: crate::permission_dialog::status_note("Foxglove", &section_items()),
    };
    for (dark, name) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        // The text that carries meaning draws at 3:1 or better on the window and the cards.
        assert!(crate::theme::contrast(t.text_muted, t.bg) >= 3.0);
        assert!(crate::theme::contrast(t.text_muted, t.surface) >= 3.0);
        for (tag, reason) in [
            ("pending", None),
            ("list-changed", Some(Reason::ListChanged)),
            ("voided", Some(Reason::Void)),
        ] {
            let mut d = dialog(reason, true);
            match tag {
                "pending" => {
                    // 18 things, 1 can't be allowed, All 17, Allow 16 (the sensitive one is off).
                    assert!(d.intro().contains("these 18 things. 1 can't be allowed."));
                    assert_eq!(
                        (d.all_label().as_str(), d.allow_label().as_str()),
                        ("All 16 other items", "Allow 16")
                    );
                }
                "list-changed" => {
                    // The list the user saw had one more line and lacked one: new code, marks.
                    let old = d.clone();
                    d.items.retain(|i| i.raw != "provide:pc_lighting.list");
                    d.items.push(ListItem {
                        raw: "provide:lights.set_effect".into(),
                        line: "Change which lighting effect plays".into(),
                        group: Group::MakeChanges,
                        allowable: true,
                        sensitive: false,
                        reason: None,
                        checked: true,
                        confirmed: false,
                        new: false,
                        before: false,
                    });
                    d.items.sort_by_key(|i| (i.group, i.raw.clone()));
                    d.code = "c41e07ab92d3f6a8".into();
                    d.mark_changes(&old);
                    assert_ne!(d.code, old.code);
                    assert_eq!(d.removed, ["Your PC's lighting zones"]);
                }
                _ => {
                    // The grant was: everything but the sensitive item and one change item.
                    d.set_checked("provide:pc_lighting.set_zone", false);
                    assert_eq!(d.allow_label(), "Allow 15");
                    assert_eq!(d.all_label(), "All 16 other items");
                }
            }
            let (w, h) = (760u32, 1000u32);
            let Some(rgba) = rasterise(w, h, |ctx| {
                set_theme(ctx, choice);
                ctx.style_mut(|s| s.animation_time = 0.0);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label(egui::RichText::new("Modules").size(24.0).strong());
                    let h = header(
                        if reason == Some(Reason::Void) {
                            HeaderKind::Changed
                        } else {
                            HeaderKind::Ask
                        },
                        "foxglove",
                        "Foxglove",
                        None,
                    );
                    perm_header_ui(ui, &t, &h);
                });
                perm_dialog_ui(ctx, &t, &mut d, true);
            }) else {
                eprintln!("no system font; render test skipped");
                return;
            };
            save_png(&format!("built-permission-{tag}-{name}.png"), w, h, rgba);
        }
        // Granted: the CLI header line, then the section with one item off.
        let (w, h) = (760u32, 900u32);
        let s = section();
        let hd = header(
            HeaderKind::Cli,
            "foxglove",
            "Foxglove",
            Some((15, "13 Nov 2026")),
        );
        let rgba = rasterise(w, h, |ctx| {
            set_theme(ctx, choice);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label(egui::RichText::new("Modules").size(24.0).strong());
                ui.add_space(8.0);
                perm_header_ui(ui, &t, &hd);
                ui.add_space(12.0);
                section_ui(ui, &t, &s, true);
            });
        })
        .unwrap();
        save_png(&format!("built-permission-granted-{name}.png"), w, h, rgba);
    }
}

/// Review images for the #1238 "asks for more" header, from the real renderer (no window).
#[test]
fn renders_the_grants_more_header_for_review() {
    use crate::permission_dialog::{
        header, Group, HeaderKind, ItemWord, ListItem, PermDialog, Reason, Section, SectionItem,
    };
    use crate::permission_dialog_ui::{perm_dialog_ui, perm_header_ui, section_ui};
    let old: [(&str, &str, Group); 13] = [
        (
            "facts.write:keyboard.*",
            "Share all of your keyboard's status (layer, battery, lights and more)",
            Group::ShareStatus,
        ),
        (
            "facts.read:module.*",
            "See every app Hideout runs and whether it is running",
            Group::ShareStatus,
        ),
        (
            "provide:health.read",
            "The keyboard's health report",
            Group::AnswerQuestions,
        ),
        (
            "provide:lights.page",
            "What the Lights page shows",
            Group::AnswerQuestions,
        ),
        (
            "provide:scenes.page",
            "Which scenes and wallpaper looks are listed",
            Group::AnswerQuestions,
        ),
        (
            "provide:overlay.window",
            "Where the layer overlay window is",
            Group::AnswerQuestions,
        ),
        (
            "provide:overlay.shown",
            "Whether the layer overlay is on",
            Group::AnswerQuestions,
        ),
        (
            "provide:tray.panel",
            "Whether the tray panel is open",
            Group::AnswerQuestions,
        ),
        (
            "provide:pc_lighting.list",
            "Your PC's lighting zones",
            Group::AnswerQuestions,
        ),
        (
            "provide:overlay.restore",
            "Bring the layer overlay back on screen",
            Group::MakeChanges,
        ),
        (
            "provide:overlay.set_shown",
            "Turn the layer overlay on or off",
            Group::MakeChanges,
        ),
        (
            "provide:tray.set_panel",
            "Open or close the tray panel",
            Group::MakeChanges,
        ),
        (
            "provide:pc_lighting.set_zone",
            "Change a PC lighting zone's colour",
            Group::MakeChanges,
        ),
    ];
    let new: [(&str, &str, Group); 3] = [
        (
            "provide:wallpaper-engine.pause",
            "Pause your live wallpaper",
            Group::MakeChanges,
        ),
        (
            "provide:wallpaper-engine.release",
            "Let your live wallpaper play again",
            Group::MakeChanges,
        ),
        (
            "provide:wallpaper-engine.status",
            "Whether your live wallpaper is paused",
            Group::AnswerQuestions,
        ),
    ];
    let section = |n_new: usize| {
        let mut items: Vec<SectionItem> = old
            .iter()
            .map(|(raw, line, g)| SectionItem {
                raw: (*raw).into(),
                line: (*line).into(),
                group: *g,
                word: ItemWord::Allowed,
            })
            .chain(new.iter().take(n_new).map(|(raw, line, g)| SectionItem {
                raw: (*raw).into(),
                line: (*line).into(),
                group: *g,
                word: ItemWord::Waits,
            }))
            .collect();
        items.sort_by_key(|i| (i.group, i.raw.clone()));
        Section {
            id: "foxglove".into(),
            name: "Foxglove".into(),
            items,
            allowed_on: Some("Allowed in Hideout on 13 Nov 2026".into()),
            history: vec![if n_new == 0 {
                "Updated to 0.3.2 on 16 Nov 2026. Your OK stayed.".to_string()
            } else {
                format!("Updated to 0.3.2 on 16 Nov 2026. Your OK stayed; {n_new} new thing{} wait{} for it.",
                    if n_new == 1 { "" } else { "s" }, if n_new == 1 { "s" } else { "" })
            }],
            can_change: true,
            kept_void: None,
            keep: None,
            note: None,
        }
    };
    let page =
        |name: &str, dark: bool, hdr: Option<(usize, &str)>, n_new: usize, open: bool, h: u32| {
            let t = crate::theme::tokens(dark);
            let choice = if dark {
                ThemeChoice::Dark
            } else {
                ThemeChoice::Light
            };
            let s = section(n_new);
            let w = 760u32;
            let Some(rgba) = rasterise(w, h, |ctx| {
                set_theme(ctx, choice);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label(egui::RichText::new("Modules").size(24.0).strong());
                    ui.add_space(8.0);
                    if let Some(c) = hdr {
                        perm_header_ui(
                            ui,
                            &t,
                            &header(HeaderKind::More, "foxglove", "Foxglove", Some(c)),
                        );
                        ui.add_space(12.0);
                    }
                    section_ui(ui, &t, &s, open);
                });
            }) else {
                eprintln!("no system font; render test skipped");
                return;
            };
            save_png(name, w, h, rgba);
        };
    for (dark, tag) in [(true, "dark"), (false, "light")] {
        page(
            &format!("grants-1-header-many-{tag}.png"),
            dark,
            Some((3, "Pause your live wallpaper")),
            3,
            true,
            900,
        );
    }
    page(
        "grants-2-header-one-dark.png",
        true,
        Some((1, "Pause your live wallpaper")),
        1,
        false,
        330,
    );
    page("grants-4-not-now-dark.png", true, None, 3, true, 820);
    page("grants-5-answered-dark.png", true, None, 0, false, 260);
    // The Review… dialog: only what the update added, each marked New.
    for (dark, tag) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        let mut items: Vec<ListItem> = new
            .iter()
            .map(|(raw, line, g)| ListItem {
                raw: (*raw).into(),
                line: (*line).into(),
                group: *g,
                allowable: true,
                sensitive: false,
                reason: None,
                checked: true,
                confirmed: false,
                new: true,
                before: false,
            })
            .collect();
        items.sort_by_key(|i| (i.group, i.raw.clone()));
        let mut d = PermDialog {
            id: "foxglove".into(),
            name: "Foxglove".into(),
            code: "5be0c7d21a9f4e36".into(),
            path: r"C:\Program Files\Foxglove\foxglove.exe".into(),
            items,
            reason: Some(Reason::More),
            keyboard: None,
            keep: None,
            removed: Vec::new(),
            same_place: false,
            view: Default::default(),
            error: None,
        };
        let (w, h) = (760u32, 720u32);
        let Some(rgba) = rasterise(w, h, |ctx| {
            set_theme(ctx, choice);
            ctx.style_mut(|s| s.animation_time = 0.0);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label(egui::RichText::new("Modules").size(24.0).strong());
                perm_header_ui(
                    ui,
                    &t,
                    &header(
                        HeaderKind::More,
                        "foxglove",
                        "Foxglove",
                        Some((3, "Pause your live wallpaper")),
                    ),
                );
            });
            perm_dialog_ui(ctx, &t, &mut d, true);
        }) else {
            return;
        };
        save_png(&format!("grants-3-review-{tag}.png"), w, h, rgba);
    }
}

/// Review images for the re-ask that starts at the user's last answer (window-free): the
/// non-sensitive items allowed before are checked and marked, the sensitive one is not.
#[test]
fn renders_the_reask_with_allowed_before_for_review() {
    use crate::permission_dialog::{Group, ListItem, PermDialog, Reason};
    use crate::permission_dialog_ui::perm_dialog_ui;
    let it = |raw: &str, line: &str, g, sensitive: bool, before: bool| ListItem {
        raw: raw.into(),
        line: line.into(),
        group: g,
        allowable: true,
        sensitive,
        reason: None,
        checked: before,
        confirmed: false,
        new: false,
        before,
    };
    for (dark, tag) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        let mut items = vec![
            it("net", "Use the network", Group::Sensitive, true, false),
            it(
                "provide:overlay.shown",
                "Whether the layer overlay is on",
                Group::AnswerQuestions,
                false,
                true,
            ),
            it(
                "provide:tray.set_panel",
                "Open or close the tray panel",
                Group::MakeChanges,
                false,
                true,
            ),
            it(
                "provide:wallpaper-engine.pause",
                "Pause your live wallpaper",
                Group::MakeChanges,
                false,
                false,
            ),
        ];
        items[3].new = true;
        items.sort_by_key(|i| (i.group, i.raw.clone()));
        let mut d = PermDialog {
            id: "foxglove".into(),
            name: "Foxglove".into(),
            code: "5be0c7d21a9f4e36".into(),
            path: r"C:\Program Files\Foxglove\foxglove.exe".into(),
            items,
            reason: Some(Reason::Void),
            keyboard: None,
            keep: None,
            removed: Vec::new(),
            same_place: false,
            view: Default::default(),
            error: None,
        };
        assert_eq!(d.allow_label(), "Allow 2");
        let (w, h) = (760u32, 720u32);
        let Some(rgba) = rasterise(w, h, |ctx| {
            set_theme(ctx, choice);
            ctx.style_mut(|s| s.animation_time = 0.0);
            perm_dialog_ui(ctx, &t, &mut d, true);
        }) else {
            eprintln!("no system font; render test skipped");
            return;
        };
        save_png(&format!("reask-allowed-before-{tag}.png"), w, h, rgba);
    }
}

/// "Waits for your OK" is drawn in the attention tone (#1238); its words carry the meaning, the
/// colour only has to stay readable on the card and the page in both themes.
#[test]
fn the_waits_word_keeps_its_contrast_in_both_themes() {
    for dark in [true, false] {
        let t = crate::theme::tokens(dark);
        for bg in [t.surface, t.bg] {
            let r = crate::theme::contrast(t.attention, bg);
            assert!(r >= 4.5, "attention on {bg:?} in dark={dark}: {r:.2}");
        }
    }
}

/// Review images for #1134: the dialog for a module that asks to be the GPU manager, from the
/// real renderer (no window). The "I understand" line names the module.
#[test]
fn renders_the_gpu_manager_dialog_for_review() {
    use crate::permission_dialog::{Group, ListItem, PermDialog};
    use crate::permission_dialog_ui::perm_dialog_ui;
    for (dark, tag) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        let it = |raw: &str, line: &str, g, sensitive, confirmed| ListItem {
            raw: raw.into(),
            line: line.into(),
            group: g,
            allowable: true,
            sensitive,
            reason: None,
            checked: true,
            confirmed,
            new: false,
            before: false,
        };
        let mut d = PermDialog {
            id: "example.gpu-helper".into(),
            name: "GPU Helper".into(),
            code: "9c41e07b3a5d8f26".into(),
            path: r"C:\Program Files\GPU Helper\gpu-helper.exe".into(),
            items: vec![
                it(
                    "provide:gpu.*",
                    "Act as the GPU manager for all apps",
                    Group::Sensitive,
                    true,
                    false,
                ),
                it(
                    "provide:wallpaper-engine.status",
                    "Whether your live wallpaper is paused",
                    Group::AnswerQuestions,
                    false,
                    false,
                ),
            ],
            reason: None,
            keyboard: None,
            keep: None,
            removed: Vec::new(),
            same_place: false,
            view: Default::default(),
            error: None,
        };
        let (w, h) = (760u32, 620u32);
        let Some(rgba) = rasterise(w, h, |ctx| {
            set_theme(ctx, choice);
            ctx.style_mut(|s| s.animation_time = 0.0);
            perm_dialog_ui(ctx, &t, &mut d, true);
        }) else {
            return;
        };
        save_png(&format!("gpu-manager-{tag}.png"), w, h, rgba);
    }
}

/// A dialog with `n` allowable items for the keep-box tests.
fn keep_test_dialog(
    n: usize,
    keep: Option<crate::permission_dialog::Keep>,
) -> crate::permission_dialog::PermDialog {
    use crate::permission_dialog::{Group, ListItem, PermDialog};
    let items = (0..n)
        .map(|i| ListItem {
            raw: format!("answer:thing.{i:02}"),
            line: format!("Answer question number {i}"),
            group: Group::AnswerQuestions,
            allowable: true,
            sensitive: false,
            reason: None,
            checked: true,
            confirmed: false,
            new: false,
            before: false,
        })
        .collect();
    PermDialog {
        id: "foxglove".into(),
        name: "Foxglove".into(),
        code: "7f3a91c20b4e55d1".into(),
        path: r"C:\Program Files\Foxglove\foxglove.exe".into(),
        items,
        reason: None,
        keyboard: None,
        keep,
        removed: Vec::new(),
        same_place: false,
        view: crate::permission_dialog::View {
            starts: false,
            handover: false,
            everyday_open: true,
            group_ack: None,
        },
        error: None,
    }
}

/// The keep box sits after the list and before Allow in Tab order; the "I understand" appears
/// only once it is checked; Tab stays inside; the dialog says the box's state and the box has
/// the helper as its description.
#[test]
fn the_keep_box_has_its_place_in_tab_order_and_is_described() {
    use crate::permission_dialog::{Keep, KEEP_LABEL};
    use crate::permission_dialog_ui::perm_dialog_ids;
    let t = crate::theme::tokens(true);
    for checked in [false, true] {
        let mut d = keep_test_dialog(3, Some(Keep::offered(false)));
        if checked {
            d.keep.as_mut().unwrap().set_on(true);
        }
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut ids = Vec::new();
        let mut last = None;
        for i in 0..3 {
            last = Some(ctx.run(input(800.0, 900.0, vec![]), |c| {
                ids = perm_dialog_ids(c, &t, &mut d, i == 0).1;
            }));
        }
        // All, Hide, 3 items, the keep box, [I understand], Allow, Not now.
        assert_eq!(ids.len(), 2 + 3 + 1 + usize::from(checked) + 2, "{ids:?}");
        let upd = last.unwrap().platform_output.accesskit_update.unwrap();
        let find = |l: &str| {
            upd.nodes
                .iter()
                .find(|(_, n)| n.label() == Some(l))
                .map(|(id, n)| (*id, n.description().map(str::to_owned)))
        };
        let state = if checked { "checked" } else { "not checked" };
        let (_, desc) = find(&format!("{KEEP_LABEL}, {state}")).expect("the box is named");
        assert_eq!(
            desc.as_deref(),
            Some(crate::permission_dialog::keep_helper("Foxglove", "foxglove.exe").as_str())
        );
        let dialog = upd
            .nodes
            .iter()
            .find(|(_, n)| n.role() == egui::accesskit::Role::Dialog)
            .map(|(_, n)| n.description().unwrap().to_owned())
            .unwrap();
        assert!(
            dialog.ends_with(&format!(
                "{KEEP_LABEL}: {}.",
                if checked { "on" } else { "off" }
            )),
            "{dialog}"
        );
        let understand = crate::permission_dialog::keep_understand_said("Foxglove", "foxglove.exe");
        assert_eq!(find(&understand).is_some(), checked);
        // Tab stays inside.
        let tab = key(Key::Tab, Modifiers::NONE);
        for _ in 0..(ids.len() + 3) {
            let _ = ctx.run(input(800.0, 900.0, vec![tab.clone()]), |c| {
                ids = perm_dialog_ids(c, &t, &mut d, false).1;
            });
            let f = ctx.memory(|m| m.focused());
            assert!(f.is_some_and(|f| ids.contains(&f)), "focus left: {f:?}");
        }
    }
}

/// A refused box is disabled, outside Tab order, and says why.
#[test]
fn a_refused_keep_box_is_disabled_and_says_why() {
    use crate::permission_dialog::{Keep, KEEP_REFUSED};
    use crate::permission_dialog_ui::perm_dialog_ids;
    let t = crate::theme::tokens(true);
    let mut d = keep_test_dialog(2, Some(Keep::refused()));
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut ids = Vec::new();
    let mut out = None;
    for i in 0..3 {
        out = Some(ctx.run(input(800.0, 900.0, vec![]), |c| {
            ids = perm_dialog_ids(c, &t, &mut d, i == 0).1;
        }));
    }
    assert_eq!(ids.len(), 2 + 2 + 2, "no keep box in Tab order: {ids:?}");
    let upd = out.unwrap().platform_output.accesskit_update.unwrap();
    assert!(upd
        .nodes
        .iter()
        .any(|(_, n)| n.description() == Some(KEEP_REFUSED)));
    assert!(!d.allowed().keep);
}

/// On a 720 px high window a long list scrolls inside and Allow / Not now stay on screen, with
/// the keep box checked (the tallest the fixed part gets).
#[test]
fn the_keep_box_leaves_the_buttons_on_a_720_px_window() {
    use crate::permission_dialog::Keep;
    use crate::permission_dialog_ui::perm_dialog_ids;
    let t = crate::theme::tokens(true);
    for n in [3usize, 30] {
        let mut d = keep_test_dialog(n, Some(Keep::offered(false)));
        d.keep.as_mut().unwrap().set_on(true);
        let ctx = egui::Context::default();
        let mut ids = Vec::new();
        for i in 0..6 {
            let _ = ctx.run(input(900.0, 720.0, vec![]), |c| {
                ids = perm_dialog_ids(c, &t, &mut d, i == 0).1;
            });
        }
        let n_ids = ids.len();
        for id in &ids[n_ids - 2..] {
            let r = ctx.read_response(*id).expect("drawn").rect;
            assert!(r.bottom() <= 720.0, "n={n}: button bottom {}", r.bottom());
            assert!(r.top() >= 0.0);
        }
        // The keep box and its understanding are on screen too.
        for id in &ids[n_ids - 4..n_ids - 2] {
            let r = ctx.read_response(*id).expect("drawn").rect;
            assert!(r.bottom() <= 720.0 && r.top() >= 0.0, "n={n}: {r:?}");
        }
    }
}

/// The window's close button answers an open dialog like Esc: Not now for the Allow dialog and
/// the first-run dialog, Cancel for the keep confirm. A frame without it answers nothing.
#[test]
fn the_windows_close_button_answers_not_now() {
    use crate::permission_dialog::{KeepAnswer, KeepConfirm, PermAnswer};
    use crate::permission_dialog_ui::{
        first_run_dialog_ids, keep_confirm_ids, perm_dialog_ids, FirstRunAnswer,
    };
    let t = crate::theme::tokens(true);
    let closing = || {
        let mut raw = input(900.0, 720.0, vec![]);
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        raw
    };
    // Two quiet frames, then the close request.
    let frames = || {
        [
            input(900.0, 720.0, vec![]),
            input(900.0, 720.0, vec![]),
            closing(),
        ]
    };

    let mut d = keep_test_dialog(3, None);
    let ctx = egui::Context::default();
    let mut got = Vec::new();
    for (i, raw) in frames().into_iter().enumerate() {
        let _ = ctx.run(raw, |c| got.push(perm_dialog_ids(c, &t, &mut d, i == 0).0));
    }
    assert_eq!(got, [None, None, Some(PermAnswer::NotNow)]);

    let mut d = first_run_test_dialog();
    let ctx = egui::Context::default();
    let mut got = Vec::new();
    for (i, raw) in frames().into_iter().enumerate() {
        let _ = ctx.run(raw, |c| {
            got.push(first_run_dialog_ids(c, &t, &mut d, i == 0).0)
        });
    }
    assert_eq!(got, [None, None, Some(FirstRunAnswer::NotNow)]);

    let k = KeepConfirm {
        id: "foxglove".into(),
        name: "Foxglove".into(),
        file: "foxglove.exe".into(),
        folder: r"C:\Apps\Fox".into(),
        code: "7f3a91c20b4e55d1".into(),
        error: None,
    };
    let ctx = egui::Context::default();
    let mut got = Vec::new();
    for (i, raw) in frames().into_iter().enumerate() {
        let _ = ctx.run(raw, |c| got.push(keep_confirm_ids(c, &t, &k, i == 0).0));
    }
    assert_eq!(got, [None, None, Some(KeepAnswer::Cancel)]);
}

/// The same for the keyboard-sharing dialog (Not now) and the dev-folder Remove confirm
/// (Cancel: nothing is sent and the confirm is gone).
#[test]
fn the_windows_close_button_answers_the_other_dialogs_too() {
    use crate::keyboard_grant::{Answer, Dialog};
    let closing = |w: f32, h: f32| {
        let mut raw = input(w, h, vec![]);
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        raw
    };
    let t = crate::theme::tokens(true);
    let d = Dialog::open(r"C:\Program Files\Foxglove\foxglove.exe", false);
    let ctx = egui::Context::default();
    let mut got = Vec::new();
    let frames = [
        input(800.0, 600.0, vec![]),
        input(800.0, 600.0, vec![]),
        closing(800.0, 600.0),
    ];
    for (i, raw) in frames.into_iter().enumerate() {
        let _ = ctx.run(raw, |c| {
            got.push(crate::keyboard_grant_ui::dialog_ui(c, &t, &d, i == 0));
        });
    }
    assert_eq!(got, [None, None, Some(Answer::NotNow)]);

    let mut d = DevDriver::new("ok");
    d.press(&crate::dev_settings::remove_name(FOX));
    assert!(d.dialog_open());
    let (view, dark) = (d.view.clone(), d.dark);
    let mut sent = 0;
    let _ = d.ctx.run(closing(d.w, 1400.0), |c| {
        sent += usize::from(draw_settings_with_dev(c, dark, &view).dev.is_some());
    });
    d.frames(2, vec![]);
    assert_eq!(sent, 0);
    assert!(d.sent.is_empty());
    assert!(!d.dialog_open());
}

/// The Modules confirm: Cancel is focused on open, Enter does not keep, Esc cancels, Tab stays
/// inside, and the dialog is named and described.
#[test]
fn the_keep_confirm_answers_cancel_by_default_and_traps_tab() {
    use crate::permission_dialog::{KeepAnswer, KeepConfirm, KEEP_CONFIRM_TITLE};
    use crate::permission_dialog_ui::keep_confirm_ids;
    let k = KeepConfirm {
        id: "foxglove".into(),
        name: "Foxglove".into(),
        file: "foxglove.exe".into(),
        folder: r"C:\Apps\Fox".into(),
        code: "7f3a91c20b4e55d1".into(),
        error: None,
    };
    let t = crate::theme::tokens(true);
    let run = |frames: Vec<Vec<Event>>| {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut got = Vec::new();
        let mut ids = Vec::new();
        let mut last = None;
        for (i, ev) in frames.into_iter().enumerate() {
            last = Some(ctx.run(input(800.0, 600.0, ev), |c| {
                let (a, v) = keep_confirm_ids(c, &t, &k, i == 0);
                got.push(a);
                ids = v;
            }));
        }
        (got, ids, ctx, last.unwrap())
    };
    let (got, ids, ctx, out) = run(vec![vec![], vec![], vec![key(Key::Enter, Modifiers::NONE)]]);
    assert_eq!(
        ctx.memory(|m| m.focused()),
        Some(ids[1]),
        "Cancel is focused"
    );
    assert!(got.iter().all(|a| *a != Some(KeepAnswer::Keep)), "{got:?}");
    let dialog = out
        .platform_output
        .accesskit_update
        .unwrap()
        .nodes
        .into_iter()
        .find(|(_, n)| n.role() == egui::accesskit::Role::Dialog)
        .map(|(_, n)| {
            (
                n.label().map(str::to_owned),
                n.description().map(str::to_owned),
            )
        })
        .unwrap();
    assert_eq!(dialog.0.as_deref(), Some(KEEP_CONFIRM_TITLE));
    assert_eq!(dialog.1.as_deref(), Some(k.said().as_str()));
    let (got, _, _, _) = run(vec![vec![], vec![key(Key::Escape, Modifiers::NONE)]]);
    assert_eq!(got.last().unwrap(), &Some(KeepAnswer::Cancel));
    let tab = key(Key::Tab, Modifiers::NONE);
    let (_, ids, ctx, _) = run(vec![
        vec![],
        vec![],
        vec![tab.clone()],
        vec![tab.clone()],
        vec![tab],
    ]);
    let f = ctx.memory(|m| m.focused());
    assert!(f.is_some_and(|f| ids.contains(&f)));
}

/// Two apps for the first-run review (UX cut 3): each with its own list and its own sensitive item.
fn first_run_test_dialog() -> crate::permission_dialog::FirstRun {
    use crate::permission_dialog::{FirstRun, Group, ListItem, PermDialog};
    let it = |raw: &str, line: &str, g, sensitive| ListItem {
        raw: raw.into(),
        line: line.into(),
        group: g,
        allowable: true,
        sensitive,
        reason: None,
        checked: !sensitive,
        confirmed: false,
        new: false,
        before: false,
    };
    let sec = |id: &str, name: &str, code: &str, items| PermDialog {
        id: id.into(),
        name: name.into(),
        code: code.into(),
        path: format!(r"C:\Program Files\{name}\{id}.exe"),
        items,
        reason: None,
        keyboard: None,
        keep: None,
        removed: Vec::new(),
        same_place: false,
        view: Default::default(),
        error: None,
    };
    FirstRun {
        sections: vec![
            sec(
                "foxglove",
                "Foxglove",
                "7f3a91c20b4e55d1",
                vec![
                    it(
                        "hub.notify",
                        "Show notifications",
                        Group::MakeChanges,
                        false,
                    ),
                    it("net", "Use the network", Group::Sensitive, true),
                ],
            ),
            sec(
                "vulpes",
                "Vulpes",
                "11c0d4e9a8b73f02",
                vec![
                    it(
                        "ui.window",
                        "Open its own window",
                        Group::MakeChanges,
                        false,
                    ),
                    it(
                        "device.hid:lighting",
                        "Talk to your lighting devices",
                        Group::Sensitive,
                        true,
                    ),
                ],
            ),
        ],
        error: None,
    }
}

/// Foxglove's real kind of list (three HID classes, the network, notifications, the keyboard
/// choice, the Keep box) next to a second, smaller app.
fn first_run_realistic_dialog() -> crate::permission_dialog::FirstRun {
    use crate::permission_dialog::{Group, Keep, KeyboardChoice, ListItem};
    let mut d = first_run_test_dialog();
    let it = |raw: &str, line: &str, g, sensitive| ListItem {
        raw: raw.into(),
        line: line.into(),
        group: g,
        allowable: true,
        sensitive,
        reason: None,
        checked: !sensitive,
        confirmed: false,
        new: false,
        before: false,
    };
    let fox = &mut d.sections[0];
    fox.items = vec![
        it(
            "device.hid:keyboard",
            "Talk to your keyboards",
            Group::Sensitive,
            true,
        ),
        it(
            "device.hid:lighting",
            "Talk to your lighting devices",
            Group::Sensitive,
            true,
        ),
        it(
            "device.hid:vendor",
            "Talk to devices in their own way",
            Group::Sensitive,
            true,
        ),
        it("net", "Use the network", Group::Sensitive, true),
        it(
            "hub.notify",
            "Show notifications",
            Group::MakeChanges,
            false,
        ),
        it(
            "ui.window",
            "Open its own window",
            Group::MakeChanges,
            false,
        ),
    ];
    fox.keyboard = Some(KeyboardChoice {
        on: false,
        expanded: false,
    });
    fox.keep = Some(Keep::offered(false));
    d
}

/// Review images for UX cut 3: the one first-run dialog with a section per app, real renderer.
#[test]
fn renders_the_first_run_dialog_for_review() {
    use crate::permission_dialog_ui::first_run_dialog_ui;
    for (dark, tag) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        let mut d = first_run_realistic_dialog();
        let (w, h) = (760u32, 720u32);
        let Some(rgba) = rasterise_events(w, h, vec![Vec::new(); 10], |ctx| {
            set_theme(ctx, choice);
            ctx.style_mut(|s| s.animation_time = 0.0);
            first_run_dialog_ui(ctx, &t, &mut d, true);
        }) else {
            return;
        };
        save_png(&format!("firstrun-1-dialog-{tag}.png"), w, h, rgba);
    }
}

/// Tab stays inside, every section has its own boxes, Allow and Not now stay on a 720 px
/// window, and the dialog is named and described.
#[test]
fn the_first_run_dialog_traps_tab_names_itself_and_keeps_its_buttons_on_screen() {
    use crate::permission_dialog_ui::first_run_dialog_ids;
    let t = crate::theme::tokens(true);
    let mut d = first_run_test_dialog();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut ids = Vec::new();
    let mut out = None;
    for i in 0..6 {
        out = Some(ctx.run(input(900.0, 720.0, vec![]), |c| {
            ids = first_run_dialog_ids(c, &t, &mut d, i == 0).1;
        }));
    }
    // Per app: its All box and two items; then Allow and Not now.
    assert_eq!(ids.len(), 2 * 3 + 2, "{ids:?}");
    for id in &ids[ids.len() - 2..] {
        let r = ctx.read_response(*id).expect("drawn").rect;
        assert!(r.bottom() <= 720.0 && r.top() >= 0.0, "{r:?}");
    }
    let upd = out.unwrap().platform_output.accesskit_update.unwrap();
    assert!(upd.nodes.iter().any(|(_, n)| {
        n.role() == egui::accesskit::Role::Dialog && n.label() == Some("Let 2 apps run in Hideout?")
    }));
}

/// Esc and Not now grant nothing; Allow carries one entry per app with its own list code, and
/// a sensitive item that is ticked but not understood is not in it.
#[test]
fn the_first_run_dialog_answers_per_app_and_counts_only_understood_sensitive_items() {
    let mut d = first_run_test_dialog();
    d.sections[0].set_checked("net", true);
    let a = d.allowed();
    assert_eq!(a.len(), 2);
    assert_eq!(a[0].1.code, "7f3a91c20b4e55d1");
    assert_eq!(a[1].1.code, "11c0d4e9a8b73f02");
    assert!(!a[0].1.chosen.iter().any(|r| r == "net"));
    d.sections[0]
        .items
        .iter_mut()
        .find(|i| i.raw == "net")
        .unwrap()
        .confirmed = true;
    assert!(d.allowed()[0].1.chosen.iter().any(|r| r == "net"));
    // The other app's sensitive item is untouched.
    assert!(!d.allowed()[1]
        .1
        .chosen
        .iter()
        .any(|r| r == "device.hid:lighting"));
    // Esc answers Not now.
    use crate::permission_dialog_ui::{first_run_dialog_ids, FirstRunAnswer};
    let t = crate::theme::tokens(true);
    let ctx = egui::Context::default();
    let mut ans = None;
    for i in 0..3 {
        let ev = if i == 2 {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        } else {
            vec![]
        };
        let _ = ctx.run(input(900.0, 800.0, ev), |c| {
            ans = first_run_dialog_ids(c, &t, &mut d, i == 0).0;
        });
    }
    assert_eq!(ans, Some(FirstRunAnswer::NotNow));
}

/// The Start with Windows dialog opens with Not now focused: Enter never turns it on.
#[test]
fn the_start_with_windows_dialog_opens_on_not_now() {
    use crate::startup_switch::{Click, Row};
    let t = theme::tokens(true);
    let row = Row {
        status: crate::startup_switch::STATUS_OFF.to_string(),
        dialog: Some(false),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let mut clicks = Vec::new();
    for i in 0..4 {
        let ev = if i == 3 {
            vec![key(Key::Enter, Modifiers::NONE)]
        } else {
            vec![]
        };
        let _ = ctx.run(input(800.0, 600.0, ev), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let a = crate::settings_view::settings_ui_with(ui, &t, "0.3.0", false, Some(&row));
                clicks.push(a.startup);
            });
        });
    }
    assert!(
        clicks.iter().all(|c| *c != Some(Click::Confirm)),
        "{clicks:?}"
    );
    assert_eq!(clicks.last().unwrap(), &Some(Click::Cancel), "{clicks:?}");
}

/// The first-run dialog of the plan (FR2): 4 sensitive asks and 17 everyday ones, as the plan
/// counts them.
fn fr2_test_dialog() -> crate::permission_dialog::PermDialog {
    use crate::permission_dialog::{Group, ListItem, PermDialog, View};
    let mk = |raw: String, line: &str, group, sensitive: bool| ListItem {
        raw,
        line: line.into(),
        group,
        allowable: true,
        sensitive,
        reason: None,
        checked: !sensitive,
        confirmed: false,
        new: false,
        before: false,
    };
    let mut items = vec![
        mk(
            "device.hid:keyboard".into(),
            "Talk to your keyboard (USB)",
            Group::Sensitive,
            true,
        ),
        mk(
            "device.hid:lighting".into(),
            "Talk to your PC's lighting devices",
            Group::Sensitive,
            true,
        ),
        mk(
            "device.hid:vendor".into(),
            "Talk to other devices from the same maker",
            Group::Sensitive,
            true,
        ),
        mk("net".into(), "Go online", Group::Sensitive, true),
    ];
    for i in 0..17 {
        items.push(mk(
            format!("provide:gizmo.m{i:02}"),
            &format!("Answer question number {i}"),
            Group::AnswerQuestions,
            false,
        ));
    }
    PermDialog {
        id: "gizmo-keys".into(),
        name: "Gizmo Keys".into(),
        code: "7f3a91c20b4e55d1".into(),
        path: r"C:\Users\someone\Programs\Gizmo\gizmo.exe".into(),
        items,
        reason: None,
        keyboard: None,
        keep: None,
        removed: Vec::new(),
        same_place: false,
        view: View {
            starts: true,
            handover: false,
            everyday_open: false,
            group_ack: Some(false),
        },
        error: None,
    }
}

/// On a 720 px window the first-run dialog keeps Allow and Not now on screen, collapsed and
/// with the everyday items shown; Show lists the 17 items without granting anything, and the
/// 4 sensitive items stay unticked until the user ticks them. Review images for both.
#[test]
fn the_first_run_dialog_collapses_the_everyday_items_on_a_720_px_window() {
    use crate::permission_dialog_ui::perm_dialog_ids;
    let t = crate::theme::tokens(true);
    for open in [false, true] {
        let mut d = fr2_test_dialog();
        d.view.everyday_open = open;
        let ctx = egui::Context::default();
        let mut ids = Vec::new();
        for i in 0..6 {
            let _ = ctx.run(input(900.0, 720.0, vec![]), |c| {
                ids = perm_dialog_ids(c, &t, &mut d, i == 0).1;
            });
        }
        // The group I understand (the 4 sensitive boxes are greyed and out of Tab order until
        // it is ticked), the everyday box, [Hide|Show], [17 items], Allow, Not now.
        assert_eq!(
            ids.len(),
            1 + 1 + 1 + if open { 17 } else { 0 } + 2,
            "open={open}"
        );
        for id in &ids[ids.len() - 2..] {
            let r = ctx.read_response(*id).expect("drawn").rect;
            assert!(r.bottom() <= 720.0 && r.top() >= 0.0, "open={open}: {r:?}");
        }
        // Not "and start" while the sensitive items are unticked: the app is not limited.
        assert_eq!(d.allow_label(), "Allow 17");
        d.set_group_ack(true);
        assert_eq!(d.allow_label(), "Allow and start");
        d.set_group_ack(false);
        assert_eq!(
            d.count(),
            17,
            "the sensitive items are not granted until understood"
        );
        let tag = if open { "shown" } else { "collapsed" };
        // Dark 760 px as before, plus light, and a narrow 480 px window where lines wrap.
        for (suffix, dark, w, understood) in [
            ("", true, 760, false),
            ("-light", false, 760, false),
            ("-narrow", true, 480, false),
            ("-understood", true, 760, true),
            ("-understood-light", false, 760, true),
            ("-understood-narrow", true, 480, true),
        ] {
            let tk = crate::theme::tokens(dark);
            let Some(rgba) = rasterise(w, 720, |ctx| {
                set_theme(
                    ctx,
                    if dark {
                        ThemeChoice::Dark
                    } else {
                        ThemeChoice::Light
                    },
                );
                ctx.style_mut(|s| s.animation_time = 0.0);
                let mut d = fr2_test_dialog();
                d.view.everyday_open = open;
                d.set_group_ack(understood);
                crate::permission_dialog_ui::perm_dialog_ui(ctx, &tk, &mut d, true);
            }) else {
                continue;
            };
            save_png(&format!("first-run-fr3-{tag}{suffix}.png"), w, 720, rgba);
        }
    }
}

/// Show lists the everyday items and Hide closes them again; the box over them still ticks and
/// clears all 17 at once.
#[test]
fn show_and_hide_open_the_everyday_items() {
    use crate::permission_dialog_ui::perm_dialog_ids;
    let t = crate::theme::tokens(true);
    let mut d = fr2_test_dialog();
    let ctx = egui::Context::default();
    let mut ids = Vec::new();
    for i in 0..3 {
        let _ = ctx.run(input(900.0, 900.0, vec![]), |c| {
            ids = perm_dialog_ids(c, &t, &mut d, i == 0).1;
        });
    }
    let show = ids[2];
    let r = ctx.read_response(show).expect("drawn").rect;
    let click = |p: egui::Pos2| {
        vec![
            Event::PointerMoved(p),
            Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]
    };
    let _ = ctx.run(input(900.0, 900.0, click(r.center())), |c| {
        ids = perm_dialog_ids(c, &t, &mut d, false).1;
    });
    assert!(d.view.everyday_open, "Show opens the list");
    assert_eq!(d.count(), 17, "Show grants and changes nothing");
    d.toggle_all();
    assert_eq!(d.count(), 0);
    d.toggle_all();
    assert_eq!(d.count(), 17);
}

/// The group "I understand": one click ticks and confirms the four sensitive items (nothing
/// counts before it), each item can still be unticked by itself, an item ticked again while the
/// box is on is covered by it, and clearing the box takes all four back.
#[test]
fn the_group_i_understand_confirms_every_sensitive_item_and_each_stays_untickable() {
    use crate::permission_dialog_ui::perm_dialog_ids;
    let t = crate::theme::tokens(true);
    let mut d = fr2_test_dialog();
    let ctx = egui::Context::default();
    let mut ids = Vec::new();
    for i in 0..3 {
        let _ = ctx.run(input(900.0, 900.0, vec![]), |c| {
            ids = perm_dialog_ids(c, &t, &mut d, i == 0).1;
        });
    }
    assert_eq!(d.count(), 17, "nothing sensitive counts before the click");
    // Greyed until the click: only the group box, the everyday box, Show, Allow, Not now.
    assert_eq!(
        ids.len(),
        5,
        "the sensitive boxes are not tabbable before the click"
    );
    let click = |p: egui::Pos2| {
        let b = |pressed| Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        vec![Event::PointerMoved(p), b(true), b(false)]
    };
    let ack = ctx.read_response(ids[0]).expect("drawn").rect;
    let _ = ctx.run(input(900.0, 900.0, click(ack.center())), |c| {
        ids = perm_dialog_ids(c, &t, &mut d, false).1;
    });
    assert!(d.group_understood());
    assert_eq!(d.count(), 21, "one click confirms the four");
    assert!(d
        .items
        .iter()
        .filter(|i| i.sensitive)
        .all(|i| i.checked && i.confirmed));
    // The next frame draws the four boxes live: 4 items, the group box, everyday, Show, 2.
    let _ = ctx.run(input(900.0, 900.0, vec![]), |c| {
        ids = perm_dialog_ids(c, &t, &mut d, false).1;
    });
    assert_eq!(ids.len(), 4 + 5);
    // One item off by itself: it stops counting, the rest stay.
    d.set_checked("net", false);
    assert_eq!(d.count(), 20);
    assert_eq!(d.allow_label(), "Allow 20");
    // The same item ticked again by hand (the UI marks it covered while the box is on).
    let _ = ctx.run(input(900.0, 900.0, vec![]), |c| {
        ids = perm_dialog_ids(c, &t, &mut d, false).1;
    });
    let net = ctx.read_response(ids[3]).expect("drawn").rect;
    let _ = ctx.run(input(900.0, 900.0, click(net.center())), |c| {
        ids = perm_dialog_ids(c, &t, &mut d, false).1;
    });
    assert_eq!(d.count(), 21);
    // Unticking all four one by one clears the group box by itself.
    for raw in SENSITIVE_RAWS {
        d.set_checked(raw, false);
    }
    let _ = ctx.run(input(900.0, 900.0, vec![]), |c| {
        ids = perm_dialog_ids(c, &t, &mut d, false).1;
    });
    assert!(!d.group_understood(), "no item ticked, no I understand");
    assert_eq!(d.count(), 17);
    // Ticking the box again, then clearing it, takes all four back.
    d.set_group_ack(true);
    d.set_group_ack(false);
    assert!(d
        .items
        .iter()
        .filter(|i| i.sensitive)
        .all(|i| !i.checked && !i.confirmed));
}

const SENSITIVE_RAWS: [&str; 4] = [
    "device.hid:keyboard",
    "device.hid:lighting",
    "device.hid:vendor",
    "net",
];

/// In the group form the risk sentence of every item the group note does not say stays on
/// screen: every key press, the GPU manager, AI requests, stored secrets. Devices and the
/// network are in the note, so they add no line.
#[test]
fn the_group_form_keeps_the_risk_sentence_of_items_the_note_does_not_say() {
    use crate::permission_dialog::{Group, ListItem};
    let mk = |raw: &str| ListItem {
        raw: raw.into(),
        line: "line".into(),
        group: Group::Sensitive,
        allowable: true,
        sensitive: true,
        reason: None,
        checked: false,
        confirmed: false,
        new: false,
        before: false,
    };
    for raw in SENSITIVE_RAWS {
        assert_eq!(mk(raw).group_risk_line("Gizmo Keys"), None, "{raw}");
    }
    for (raw, says) in [
        ("key.events", "see every key you press"),
        ("provide:gpu.broker", "see every AI request from every app"),
        ("ai.ask:chat", "send questions to an AI service"),
        ("secrets.ref:token", "use a secret you stored"),
        (
            "something.new",
            "do something Hideout can't describe in a line",
        ),
    ] {
        let line = mk(raw).group_risk_line("Gizmo Keys").expect(raw);
        assert!(
            line.starts_with("This lets Gizmo Keys ") && line.contains(says),
            "{raw}: {line}"
        );
    }
}

/// A re-ask or any dialog without the group box keeps one "I understand" per sensitive item.
#[test]
fn without_the_group_box_each_sensitive_item_keeps_its_own_i_understand() {
    let mut d = fr2_test_dialog();
    d.view.group_ack = None;
    d.set_group_ack(true);
    assert_eq!(d.count(), 17, "no group box, no group confirm");
    assert!(!d.group_understood());
}

/// A page that hands the shell words to announce (a rebuild restart finishing, say).
struct SaysPage(Arc<Mutex<Vec<String>>>);

impl Page for SaysPage {
    fn id(&self) -> &str {
        "modules"
    }
    fn title(&self) -> &str {
        "Modules"
    }
    fn ui(&mut self, ui: &mut egui::Ui, _t: &Tokens) {
        ui.label("A hub page.");
    }
    fn take_said(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// The words a page hands over are said once through the polite announcement, and later
/// frames do not say them again.
#[test]
fn a_pages_words_are_announced_once_politely() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("page-says");
    let queue = Arc::new(Mutex::new(Vec::new()));
    let mut hub = hub_with_pages(&dir, None, vec![Box::new(SaysPage(queue.clone()))]);
    let _ = frames(&ctx, &mut hub, vec![]);
    assert!(hub.announce.is_none());
    queue
        .lock()
        .unwrap()
        .push("Foxglove rebuilt and restarted".into());
    let said = |out: &egui::FullOutput| -> Vec<egui::accesskit::NodeId> {
        out.platform_output
            .accesskit_update
            .iter()
            .flat_map(|u| u.nodes.iter())
            .filter(|(_, n)| {
                n.live() == Some(egui::accesskit::Live::Polite)
                    && n.label() == Some("Foxglove rebuilt and restarted")
            })
            .map(|(id, _)| *id)
            .collect()
    };
    let first = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    let second = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    // One node, one id: the screen reader says it when it appears and not again.
    let (a, b) = (said(&first), said(&second));
    assert_eq!(a.len(), 1, "one polite node on the first frame");
    assert_eq!(a, b, "the same node (same id) on the next frame");
    let n = hub.announce.as_ref().map(|a| a.n);
    assert!(n.is_some());
    assert!(queue.lock().unwrap().is_empty());
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert_eq!(hub.announce.as_ref().map(|a| a.n), n);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review images of the plain-words fact lines (L3 of sec-1332): the sensors app's dialog (a
/// wide write, one exact read), a wider read beside an exact one, and two keys no table knows.
/// Light, dark and a narrow window (`HIDEOUT_REVIEW_DIR`, files `built-consent-*.png`).
#[test]
fn renders_the_consent_copy_for_review() {
    use crate::permission_dialog::{Group, ListItem, PermDialog};
    use crate::permission_dialog_ui::perm_dialog_ui;
    use hideout_contract::permission::fact_words;

    let up = |s: String| {
        let mut c = s.chars();
        c.next().map_or_else(String::new, |f| {
            f.to_uppercase().chain(c).collect::<String>()
        })
    };
    let item = |raw: &str, line: String| ListItem {
        raw: raw.into(),
        line,
        group: Group::ShareStatus,
        allowable: true,
        sensitive: false,
        reason: None,
        checked: false,
        confirmed: false,
        new: false,
        before: false,
    };
    let dialog = || {
        let mut items: Vec<ListItem> = [
            ("facts.write:sensor.*", false, "sensor.*"),
            ("facts.read:keyboard.game_mode", true, "keyboard.game_mode"),
            ("facts.read:keyboard.*", true, "keyboard.*"),
            ("facts.write:weather.temp", false, "weather.temp"),
            ("facts.read:weather.*", true, "weather.*"),
        ]
        .into_iter()
        .map(|(raw, read, key)| item(raw, up(fact_words(read, key))))
        .collect();
        items.sort_by_key(|i| i.raw.clone());
        PermDialog {
            id: "sensors".into(),
            name: "Sensors".into(),
            code: "7f3a91c20b4e55d1".into(),
            path: r"C:\Program Files\Foxglove\sensors.exe".into(),
            items,
            reason: None,
            keyboard: None,
            keep: None,
            removed: Vec::new(),
            same_place: false,
            view: Default::default(),
            error: None,
        }
    };
    for (dark, name) in [(true, "dark"), (false, "light")] {
        let t = crate::theme::tokens(dark);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        for (size, w, h) in [("wide", 760u32, 760u32), ("narrow", 420, 900)] {
            let mut d = dialog();
            d.view.everyday_open = true;
            let Some(rgba) = rasterise(w, h, |ctx| {
                set_theme(ctx, choice);
                ctx.style_mut(|s| s.animation_time = 0.0);
                perm_dialog_ui(ctx, &t, &mut d, true);
            }) else {
                eprintln!("no system font; render test skipped");
                return;
            };
            save_png(&format!("built-consent-{size}-{name}.png"), w, h, rgba);
        }
    }
}

// ---- R5: Settings > Developer ---------------------------------------------------------------

fn dev_view(case: &str) -> crate::dev_settings::DevView {
    use crate::dev_settings::{AppState, DevApp, DevRow, DevView, Note, Problem};
    let app = |name: &str, state| DevApp {
        id: name.to_lowercase().replace(' ', "-"),
        name: name.into(),
        state,
    };
    let ok = DevRow {
        path: r"D:\dev\foxglove".into(),
        apps: vec![
            app("Foxglove", AppState::Running),
            app("Vulpes", AppState::NeedsOk),
            app("Gizmo Keys", AppState::NotKept),
            app("Tally", AppState::NotAllowed),
        ],
        problem: None,
        also_inside: None,
    };
    match case {
        "off" => DevView::default(),
        "off-listed" => DevView {
            on: false,
            folders: vec![DevRow {
                path: r"D:\dev\foxglove".into(),
                apps: vec![],
                problem: None,
                also_inside: None,
            }],
            ..Default::default()
        },
        "empty" => DevView {
            on: true,
            restart: true,
            ..Default::default()
        },
        "problem" => DevView {
            on: true,
            restart: false,
            folders: vec![
                ok,
                DevRow {
                    path: r"D:\shared\tools".into(),
                    apps: vec![
                        app("Relay", AppState::Ignored),
                        app("Probe", AppState::Ignored),
                    ],
                    problem: Some(Problem::FolderOpen),
                    also_inside: None,
                },
                DevRow {
                    path: r"D:\dev\new-app".into(),
                    apps: vec![
                        app("Sensor Hub", AppState::Off),
                        app("Beacon", AppState::Restarting),
                        app("Lamp", AppState::Stopped),
                    ],
                    problem: None,
                    also_inside: None,
                },
            ],
            note: Some(Note {
                text: Problem::Profile.line().into(),
                error: true,
            }),
        },
        // DF3/DF4: each way a listed folder stops qualifying, and a folder held by another one.
        "df34" => DevView {
            on: true,
            restart: true,
            folders: vec![
                DevRow {
                    path: r"D:\dev\app".into(),
                    apps: vec![app("Foxglove", AppState::Running)],
                    problem: None,
                    also_inside: None,
                },
                DevRow {
                    path: r"D:\dev\app\tools".into(),
                    apps: vec![],
                    problem: None,
                    also_inside: Some(r"D:\dev\app".into()),
                },
                DevRow {
                    path: r"E:\usb\app".into(),
                    apps: vec![app("Relay", AppState::Ignored)],
                    problem: Some(Problem::PlugIn),
                    also_inside: None,
                },
                DevRow {
                    path: r"\\nas\share\app".into(),
                    apps: vec![],
                    problem: Some(Problem::NetworkDrive),
                    also_inside: None,
                },
                DevRow {
                    path: r"C:\Users\me\Sync\app".into(),
                    apps: vec![app("Probe", AppState::Ignored)],
                    problem: Some(Problem::Cloud),
                    also_inside: None,
                },
                DevRow {
                    path: r"F:\mounted\app".into(),
                    apps: vec![],
                    problem: Some(Problem::VirtualDrive),
                    also_inside: None,
                },
                DevRow {
                    path: r"C:\Users\me\Unsure\app".into(),
                    apps: vec![app("Beacon", AppState::Ignored)],
                    problem: Some(Problem::CloudUnknown),
                    also_inside: None,
                },
            ],
            note: Some(Note {
                text: Problem::Cloud.line().into(),
                error: true,
            }),
        },
        "inside" => DevView {
            on: true,
            restart: true,
            folders: vec![ok],
            note: Some(Note {
                text: crate::dev_settings::inside_line(r"D:\dev\foxglove"),
                error: false,
            }),
        },
        "added" => DevView {
            on: true,
            restart: true,
            folders: vec![ok],
            note: Some(Note {
                text: crate::dev_settings::added_line(&["Vulpes".into(), "Gizmo Keys".into()]),
                error: false,
            }),
        },
        _ => DevView {
            on: true,
            restart: true,
            folders: vec![ok],
            note: Some(Note {
                text: crate::dev_settings::ADDED.into(),
                error: false,
            }),
        },
    }
}

fn draw_settings_with_dev(
    c: &egui::Context,
    dark: bool,
    view: &crate::dev_settings::DevView,
) -> crate::settings_view::Actions {
    let t = theme::tokens(dark);
    let mut acts = None;
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(t.bg.0, t.bg.1, t.bg.2))
                .inner_margin(egui::Margin::same(24)),
        )
        .show(c, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                acts = Some(crate::settings_view::settings_ui_full(
                    ui,
                    &t,
                    "0.3.0",
                    false,
                    None,
                    Some(view),
                ));
            });
        });
    acts.unwrap()
}

/// The Developer section, light, dark and narrow, dev on with a problem row and dev off, for the
/// UX review. Images go to `HIDEOUT_REVIEW_DIR`.
#[test]
fn renders_the_developer_settings_for_review() {
    for (case, dark, w, h, file) in [
        ("ok", true, 720u32, 760u32, "r5-developer-dark.png"),
        ("ok", false, 720, 760, "r5-developer-light.png"),
        ("problem", true, 720, 980, "r5-developer-problem-dark.png"),
        ("problem", false, 720, 980, "r5-developer-problem-light.png"),
        ("empty", false, 720, 640, "r5-developer-empty-light.png"),
        ("ok", true, 360, 1100, "r5-developer-narrow-dark.png"),
        ("ok", false, 360, 1100, "r5-developer-narrow-light.png"),
        (
            "problem",
            false,
            360,
            1400,
            "r5-developer-problem-narrow-light.png",
        ),
        ("off-listed", true, 720, 520, "r5-developer-off-dark.png"),
        ("off-listed", false, 720, 520, "r5-developer-off-light.png"),
        ("off-listed", true, 720, 520, "df5-off-dark.png"),
        ("off-listed", false, 720, 520, "df5-off-light.png"),
        ("off-listed", false, 360, 760, "df5-off-narrow-light.png"),
        ("ok", false, 720, 760, "df2-apps-light.png"),
        ("added", false, 720, 760, "df2-added-light.png"),
        ("added", true, 720, 760, "df2-added-dark.png"),
        ("added", false, 360, 1200, "df2-added-narrow-light.png"),
        ("ok", true, 720, 760, "df2-apps-dark.png"),
        ("ok", false, 360, 1200, "df2-apps-narrow-light.png"),
        ("problem", false, 720, 1100, "df2-states-light.png"),
        ("problem", true, 720, 1100, "df2-states-dark.png"),
        ("problem", false, 360, 1500, "df2-states-narrow-light.png"),
        ("df34", false, 720, 1300, "df34-refusals-light.png"),
        ("df34", true, 720, 1300, "df34-refusals-dark.png"),
        ("df34", false, 360, 2100, "df34-refusals-narrow-light.png"),
        ("inside", false, 720, 760, "df34-inside-light.png"),
    ] {
        let view = dev_view(case);
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        let Some(rgba) = rasterise_in(choice, w, h, Vec::new(), |c| {
            draw_settings_with_dev(c, dark, &view);
        }) else {
            eprintln!("no system font; render test skipped");
            return;
        };
        save_png(file, w, h, rgba);
    }
}

type Seen = Vec<(
    egui::accesskit::Role,
    Option<String>,
    Option<egui::accesskit::Live>,
)>;

fn accesskit_nodes(view: &crate::dev_settings::DevView) -> Seen {
    let ctx = ctx();
    ctx.enable_accesskit();
    let mut last = None;
    for _ in 0..3 {
        last = Some(ctx.run(input(720.0, 900.0, vec![]), |c| {
            draw_settings_with_dev(c, true, view);
        }));
    }
    last.unwrap()
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|u| u.nodes.iter())
        .map(|(_, n)| {
            (
                n.role(),
                n.label().or(n.value()).map(String::from),
                n.live(),
            )
        })
        .collect()
}

/// What a screen reader gets: the Remove buttons are named with their folder, the switch is a
/// checkbox with its words, the note is a polite live region, and dev off has nothing to act on.
#[test]
fn the_developer_section_reads_well_to_a_screen_reader() {
    use egui::accesskit::{Live, Role};
    let on = accesskit_nodes(&dev_view("problem"));
    let button = |name: &str| {
        on.iter()
            .any(|(r, l, _)| *r == Role::Button && l.as_deref() == Some(name))
    };
    assert!(button(r"Remove dev folder D:\dev\foxglove"), "{on:?}");
    assert!(button(r"Remove dev folder D:\shared\tools"), "{on:?}");
    assert!(button(crate::dev_settings::ADD), "{on:?}");
    assert!(
        on.iter().any(|(r, l, _)| *r == Role::CheckBox
            && l.as_deref() == Some(crate::dev_settings::RESTART_SWITCH)),
        "{on:?}"
    );
    assert!(
        on.iter().any(|(_, l, live)| {
            l.as_deref() == Some(crate::dev_settings::Problem::Profile.line())
                && live.is_some_and(|x| x != Live::Off)
        }),
        "the note is a live region: {on:?}"
    );
    // DF3/DF4: every row line, and "Also inside", is text a screen reader reaches.
    let df = accesskit_nodes(&dev_view("df34"));
    for want in [
        crate::dev_settings::Problem::PlugIn.row_line().to_string(),
        crate::dev_settings::Problem::NetworkDrive
            .row_line()
            .to_string(),
        crate::dev_settings::Problem::Cloud.row_line().to_string(),
        crate::dev_settings::Problem::VirtualDrive
            .row_line()
            .to_string(),
        crate::dev_settings::Problem::CloudUnknown
            .row_line()
            .to_string(),
        crate::dev_settings::also_inside_line(r"D:\dev\app"),
    ] {
        assert!(
            df.iter()
                .any(|(_, l, _)| l.as_deref() == Some(want.as_str())),
            "{want}: {df:?}"
        );
    }
    assert!(
        df.iter().any(|(_, l, live)| {
            l.as_deref() == Some(crate::dev_settings::Problem::Cloud.line())
                && live.is_some_and(|x| x != Live::Off)
        }),
        "the refusal is a live region: {df:?}"
    );
    // Dev off with folders listed: the off line and the paths, none of this section's controls.
    let off = accesskit_nodes(&dev_view("off-listed"));
    assert!(off
        .iter()
        .any(|(_, l, _)| l.as_deref() == Some(crate::dev_settings::MODE_OFF)));
    // DF5: the second line says what happens to their apps and how to turn it on.
    assert!(off
        .iter()
        .any(|(_, l, _)| l.as_deref() == Some(crate::dev_settings::mode_off_more())));
    assert!(off
        .iter()
        .any(|(_, l, _)| l.as_deref() == Some(r"D:\dev\foxglove")));
    let no_controls = |nodes: &Seen| {
        !nodes.iter().any(|(_, l, _)| {
            l.as_deref().is_some_and(|l| {
                (l.contains("dev folder")
                    && l != crate::dev_settings::MODE_OFF
                    && l != crate::dev_settings::mode_off_more())
                    || l == crate::dev_settings::ADD
                    || l == crate::dev_settings::RESTART_SWITCH
            })
        })
    };
    assert!(no_controls(&off), "{off:?}");
    // Dev off and nothing listed: the section is not there at all.
    let hidden = accesskit_nodes(&dev_view("off"));
    assert!(hidden.iter().all(|(_, l, _)| {
        l.as_deref() != Some(crate::dev_settings::HEADING)
            && l.as_deref() != Some(crate::dev_settings::MODE_OFF)
            && l.as_deref() != Some(crate::dev_settings::mode_off_more())
    }));
    // Dev on: the dev-off lines are not there.
    assert!(on
        .iter()
        .all(|(_, l, _)| l.as_deref() != Some(crate::dev_settings::mode_off_more())));
    assert!(no_controls(&hidden), "{hidden:?}");
}

/// One page the tests drive frame by frame: a persistent context, a view they may change, and
/// the accessibility names of what was drawn.
struct DevDriver {
    ctx: egui::Context,
    view: crate::dev_settings::DevView,
    dark: bool,
    w: f32,
    names: HashMap<egui::accesskit::NodeId, (egui::accesskit::Role, String, String)>,
    sent: Vec<crate::dev_settings::DevClick>,
}

impl DevDriver {
    fn new(case: &str) -> Self {
        let ctx = ctx();
        ctx.enable_accesskit();
        let mut d = Self {
            ctx,
            view: dev_view(case),
            dark: true,
            w: 720.0,
            names: HashMap::new(),
            sent: Vec::new(),
        };
        d.frames(3, vec![]);
        d
    }

    fn frames(&mut self, n: usize, events: Vec<Event>) {
        let mut events = Some(events);
        for _ in 0..n {
            let raw = input(self.w, 1400.0, events.take().unwrap_or_default());
            let view = self.view.clone();
            let dark = self.dark;
            let sent = &mut self.sent;
            let out = self.ctx.run(raw, |c| {
                if let Some(d) = draw_settings_with_dev(c, dark, &view).dev {
                    sent.push(d);
                }
            });
            if let Some(u) = &out.platform_output.accesskit_update {
                for (id, n) in &u.nodes {
                    self.names.insert(
                        *id,
                        (
                            n.role(),
                            n.label().or(n.value()).unwrap_or_default().to_string(),
                            n.description().unwrap_or_default().to_string(),
                        ),
                    );
                }
            }
        }
    }

    fn id_of(&self, role: egui::accesskit::Role, label: &str) -> Option<egui::accesskit::NodeId> {
        self.names
            .iter()
            .find(|(_, (r, l, _))| *r == role && l == label)
            .map(|(id, _)| *id)
    }

    /// Presses the named button (by its accessible name), then lets the page settle.
    fn press(&mut self, label: &str) {
        self.press_only(label);
        self.frames(2, vec![]);
    }

    /// The click frame alone: the hub changes the view before the next frame draws.
    fn press_only(&mut self, label: &str) {
        let id = self
            .id_of(egui::accesskit::Role::Button, label)
            .or_else(|| self.id_of(egui::accesskit::Role::CheckBox, label))
            .or_else(|| self.id_of(egui::accesskit::Role::Link, label))
            .unwrap_or_else(|| panic!("no button {label}"));
        let req = egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target: id,
            data: None,
        };
        self.frames(1, vec![Event::AccessKitActionRequest(req)]);
    }

    fn has_button(&self, label: &str) -> bool {
        self.id_of(egui::accesskit::Role::Button, label).is_some()
    }

    fn dialog_open(&mut self) -> bool {
        // Names of nodes that are gone from the page stay in the map: read a fresh frame.
        self.names.clear();
        self.frames(1, vec![]);
        self.names.values().any(|(r, l, _)| {
            *r == egui::accesskit::Role::Dialog && l == crate::dev_settings::CONFIRM_TITLE
        })
    }

    /// The accessible name of the focused widget.
    fn focused(&mut self) -> Option<String> {
        self.names.clear();
        self.frames(1, vec![]);
        let f = self.ctx.memory(|m| m.focused())?;
        self.names
            .get(&egui::accesskit::NodeId(f.value()))
            .map(|(_, l, _)| l.clone())
    }
}

const FOX: &str = r"D:\dev\foxglove";

/// DF2: each app is one line with its state in words, a screen reader hears it as one label,
/// the link names its app, and a row Hideout ignores still lists what was stored.
#[test]
fn app_lines_say_their_state_in_words() {
    let d = DevDriver::new("problem");
    let has = |label: &str| d.names.values().any(|(_, l, _)| l == label);
    for l in [
        "Foxglove, running",
        "Vulpes, needs your OK",
        "Gizmo Keys, not kept over rebuilds yet",
        "Tally, not allowed yet",
        "Relay, not kept while Hideout ignores this folder",
        "Sensor Hub, off",
        "Beacon, restarting",
        "Lamp, stopped",
    ] {
        assert!(has(l), "{l}: {:?}", d.names.values().collect::<Vec<_>>());
    }
    for (role, l) in [
        (egui::accesskit::Role::Link, "Review Vulpes"),
        (egui::accesskit::Role::Link, "Review Gizmo Keys"),
        (egui::accesskit::Role::Link, "Set up Tally"),
    ] {
        assert!(d.id_of(role, l).is_some(), "{l}");
    }
    // No link for a running, off, restarting, stopped or ignored app.
    for l in [
        "Review Foxglove",
        "Review Relay",
        "Review Beacon",
        "Set up Lamp",
    ] {
        assert!(d.id_of(egui::accesskit::Role::Link, l).is_none(), "{l}");
    }
    // A row with apps does not also say there are none.
    assert!(!has(crate::dev_settings::NO_APPS) || d.view.folders.iter().any(|r| r.apps.is_empty()));
}

/// DF2: Review... and Set up... send the app's id, once, and nothing else.
#[test]
fn review_and_set_up_send_their_app() {
    use crate::dev_settings::DevClick;
    let mut d = DevDriver::new("ok");
    d.press("Review Vulpes");
    assert_eq!(d.sent, vec![DevClick::Review("vulpes".into())]);
    d.sent.clear();
    d.press("Review Gizmo Keys");
    assert_eq!(d.sent, vec![DevClick::Review("gizmo-keys".into())]);
    d.sent.clear();
    d.press("Set up Tally");
    assert_eq!(d.sent, vec![DevClick::SetUp("tally".into())]);
    d.frames(3, vec![]);
    assert_eq!(d.sent.len(), 1);
}

/// Each click the section makes. Add carries no path (the hub opens the picker itself). Remove
/// sends nothing until the confirm is answered: a count of two clicks.
#[test]
fn the_developer_section_sends_its_clicks() {
    use crate::dev_settings::{self as w, DevClick};
    let mut d = DevDriver::new("ok");
    d.press(w::ADD);
    assert_eq!(d.sent, vec![DevClick::Add]);
    d.sent.clear();
    d.press(w::RESTART_SWITCH);
    assert_eq!(d.sent, vec![DevClick::Restart(false)]);
    d.sent.clear();
    // Remove: click 1 asks and sends nothing; click 2 (Remove folder) sends exactly one.
    d.press(&w::remove_name(FOX));
    assert!(d.sent.is_empty(), "Remove... only asks: {:?}", d.sent);
    assert!(d.dialog_open());
    d.press(w::CONFIRM_OK);
    assert_eq!(d.sent, vec![DevClick::Remove(FOX.into())]);
    // Nothing else is sent by the settling frames, and the dialog is closed.
    d.frames(3, vec![]);
    assert_eq!(d.sent.len(), 1);
    assert!(!d.dialog_open());
}

/// Cancel and Esc send nothing and put the focus back on the same Remove...
#[test]
fn the_remove_confirm_cancels_with_the_button_and_with_escape() {
    use crate::dev_settings::{self as w};
    let mut d = DevDriver::new("ok");
    // Cancel is what has the focus when the confirm opens.
    d.press(&w::remove_name(FOX));
    assert!(d.dialog_open());
    assert_eq!(d.focused().as_deref(), Some(w::CONFIRM_CANCEL));
    d.press(w::CONFIRM_CANCEL);
    assert!(d.sent.is_empty());
    assert!(!d.dialog_open());
    assert_eq!(d.focused(), Some(w::remove_name(FOX)));
    // Escape does the same.
    d.press(&w::remove_name(FOX));
    assert!(d.dialog_open());
    d.frames(1, vec![key(Key::Escape, Modifiers::NONE)]);
    d.frames(2, vec![]);
    assert!(d.sent.is_empty());
    assert!(!d.dialog_open());
    assert_eq!(d.focused(), Some(w::remove_name(FOX)));
    // Enter on the opening focus is Cancel: it never removes.
    d.press(&w::remove_name(FOX));
    d.frames(1, vec![key(Key::Enter, Modifiers::NONE)]);
    d.frames(2, vec![]);
    assert!(d.sent.is_empty(), "{:?}", d.sent);
}

/// A click outside the dialog is Cancel too: nothing is sent, the dialog closes, and the focus
/// goes back to the same Remove...
#[test]
fn a_click_outside_the_remove_confirm_cancels_it() {
    use crate::dev_settings::{self as w};
    let mut d = DevDriver::new("ok");
    d.press(&w::remove_name(FOX));
    assert!(d.dialog_open());
    let at = egui::pos2(4.0, 1390.0);
    let b = |pressed| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    d.frames(1, vec![Event::PointerMoved(at)]);
    d.frames(1, vec![b(true)]);
    d.frames(1, vec![b(false)]);
    d.frames(3, vec![]);
    assert!(d.sent.is_empty(), "{:?}", d.sent);
    assert!(!d.dialog_open());
    assert_eq!(d.focused(), Some(w::remove_name(FOX)));
}

/// After Remove folder the focus goes to the next row's Remove..., else the previous one's, else
/// Add a folder....
#[test]
fn focus_after_a_removal_goes_to_the_next_row_then_the_previous_then_add() {
    use crate::dev_settings::{self as w, DevRow};
    let row = |p: &str| DevRow {
        path: p.into(),
        apps: vec![],
        problem: None,
        also_inside: None,
    };
    let rows = [r"D:\a", r"D:\b", r"D:\c"];
    let view = |list: &[&str]| crate::dev_settings::DevView {
        on: true,
        restart: true,
        folders: list.iter().map(|p| row(p)).collect(),
        ..Default::default()
    };
    // Middle row: the next one (c).
    let mut d = DevDriver::new("empty");
    d.view = view(&rows);
    d.frames(3, vec![]);
    d.press(&w::remove_name(r"D:\b"));
    d.press_only(w::CONFIRM_OK);
    d.view = view(&[r"D:\a", r"D:\c"]);
    d.frames(3, vec![]);
    assert_eq!(d.focused(), Some(w::remove_name(r"D:\c")));
    // Last row: the previous one (a).
    d.press(&w::remove_name(r"D:\c"));
    d.press_only(w::CONFIRM_OK);
    d.view = view(&[r"D:\a"]);
    d.frames(3, vec![]);
    assert_eq!(d.focused(), Some(w::remove_name(r"D:\a")));
    // The only row: Add a folder...
    d.press(&w::remove_name(r"D:\a"));
    d.press_only(w::CONFIRM_OK);
    d.view = view(&[]);
    d.frames(3, vec![]);
    assert_eq!(d.focused().as_deref(), Some(w::ADD));
    // A save that failed leaves the row: the focus stays on its own Remove....
    let mut d = DevDriver::new("empty");
    d.view = view(&[r"D:\a", r"D:\b"]);
    d.frames(3, vec![]);
    d.press(&w::remove_name(r"D:\a"));
    d.press(w::CONFIRM_OK);
    d.frames(3, vec![]);
    assert_eq!(d.focused(), Some(w::remove_name(r"D:\a")));
}

/// A row that goes while its confirm is open (removed elsewhere) takes the confirm with it.
#[test]
fn a_confirm_for_a_folder_that_went_closes() {
    use crate::dev_settings::{self as w};
    let mut d = DevDriver::new("ok");
    d.press(&w::remove_name(FOX));
    assert!(d.dialog_open());
    d.view = dev_view("empty");
    d.frames(3, vec![]);
    assert!(!d.dialog_open());
    assert!(d.sent.is_empty());
}

/// What a screen reader gets from the dialog: its title names it, its body describes it, and it
/// says the running copy stays and the next build asks.
#[test]
fn the_remove_confirm_reads_well_to_a_screen_reader() {
    use crate::dev_settings::{self as w};
    let mut d = DevDriver::new("ok");
    d.press(&w::remove_name(FOX));
    d.names.clear();
    d.frames(2, vec![]);
    let (_, label, desc) = d
        .names
        .values()
        .find(|(r, ..)| *r == egui::accesskit::Role::Dialog)
        .cloned()
        .expect("a dialog");
    assert_eq!(label, w::CONFIRM_TITLE);
    assert!(desc.contains(FOX), "{desc}");
    assert!(desc.contains("keeps running"), "{desc}");
    assert!(desc.contains("next build asks for your OK again"), "{desc}");
    assert!(d.has_button(w::CONFIRM_CANCEL));
    assert!(d.has_button(w::CONFIRM_OK));
}

/// The confirm, light, dark and narrow, with apps and without, for the UX review.
#[test]
fn renders_the_remove_confirm_for_review() {
    for (case, dark, width, h, file) in [
        ("ok", true, 720u32, 640u32, "df1-remove-confirm-dark.png"),
        ("ok", false, 720, 640, "df1-remove-confirm-light.png"),
        ("ok", true, 360, 900, "df1-remove-confirm-narrow-dark.png"),
        ("ok", false, 360, 900, "df1-remove-confirm-narrow-light.png"),
        (
            "empty-app",
            false,
            720,
            640,
            "df1-remove-confirm-noapps-light.png",
        ),
    ] {
        let path = if case == "empty-app" {
            r"D:\dev\new-app"
        } else {
            FOX
        };
        let view = dev_view(if case == "empty-app" { "problem" } else { case });
        let choice = if dark {
            ThemeChoice::Dark
        } else {
            ThemeChoice::Light
        };
        let mut opened = false;
        let Some(rgba) = rasterise_in(choice, width, h, vec![Vec::new(); 12], |c| {
            if !opened {
                opened = true;
                crate::settings_view::open_remove_confirm(c, path);
            }
            draw_settings_with_dev(c, dark, &view);
        }) else {
            eprintln!("no system font; render test skipped");
            return;
        };
        save_png(file, width, h, rgba);
    }
}

#[test]
fn the_empty_lines_say_app_never_module() {
    for line in [EMPTY_PALETTE, hideout_panels::tray::EMPTY_TILES_LINE] {
        assert!(!line.to_lowercase().contains("module"), "{line:?}");
    }
    assert_eq!(
        EMPTY_PALETTE,
        "No commands yet. Apps add their actions and pages here."
    );
}
