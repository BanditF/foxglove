//! The tray panel (#391, design B of epic #393): a left click on the tray icon opens a small
//! borderless panel next to the icon for daily things (both halves, profile, lights, right art,
//! the left panel's pause) with "Open Foxglove →" for the rest. A right click keeps the native
//! menu.
//!
//! The panel is an egui area on the overlay window, which covers the work area, placed against
//! the taskbar's edge by [`place`]. While it's open the overlay takes the mouse only over the
//! panel ([`hit_region`], #405), so a click elsewhere reaches the window under it at once. It
//! closes on Esc, on a click outside it, and when the window loses focus (the window's focus
//! event); it opens from the tray icon's click event. Nothing here repeats or waits: each
//! frame comes from an event.

use std::sync::{Arc, Mutex};

use digitalis::protocol::RgbHalf;
use egui::{pos2, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::shell::{StripState, Tone};
use super::theme;

pub const WIDTH: f32 = 400.0;

/// The taskbar edge the tray icon sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// The gap between the panel and the taskbar or the screen's edge, in points.
const MARGIN: f32 = 12.0;

/// Which edge the icon is on: the side of the work area it lies beyond, or (when the taskbar
/// hides and the icon is inside the work area) the nearest side.
pub fn edge(icon: Rect, work: Rect) -> Edge {
    let c = icon.center();
    if c.y >= work.bottom() {
        return Edge::Bottom;
    }
    if c.y <= work.top() {
        return Edge::Top;
    }
    if c.x <= work.left() {
        return Edge::Left;
    }
    if c.x >= work.right() {
        return Edge::Right;
    }
    let gaps = [
        (work.bottom() - c.y, Edge::Bottom),
        (c.y - work.top(), Edge::Top),
        (c.x - work.left(), Edge::Left),
        (work.right() - c.x, Edge::Right),
    ];
    gaps.into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Edge::Bottom, |(_, e)| e)
}

/// Where the panel's top-left corner goes, all in screen points: against the taskbar's edge of
/// the work area, centred on the icon along it, and kept inside the work area.
pub fn place(icon: Rect, work: Rect, panel: Vec2) -> Pos2 {
    let clamp = |v: f32, lo: f32, hi: f32| if hi < lo { lo } else { v.clamp(lo, hi) };
    let x_along = clamp(
        icon.center().x - panel.x / 2.0,
        work.left() + MARGIN,
        work.right() - panel.x - MARGIN,
    );
    let y_along = clamp(
        icon.center().y - panel.y / 2.0,
        work.top() + MARGIN,
        work.bottom() - panel.y - MARGIN,
    );
    match edge(icon, work) {
        Edge::Bottom => pos2(x_along, (work.bottom() - panel.y - MARGIN).max(work.top())),
        Edge::Top => pos2(x_along, work.top() + MARGIN),
        Edge::Left => pos2(work.left() + MARGIN, y_along),
        Edge::Right => pos2((work.right() - panel.x - MARGIN).max(work.left()), y_along),
    }
}

/// A tray click waiting for the next frame: the icon's rectangle in physical pixels.
static CLICKED: Mutex<Option<Rect>> = Mutex::new(None);
static WAKE: std::sync::OnceLock<crate::ui_wake::UiWake> = std::sync::OnceLock::new();

/// The UI's wake, so a tray click (on the tray's thread) draws a frame.
pub fn set_wake(ctx: &egui::Context) {
    let _ = WAKE.set(crate::ui_wake::UiWake::from_ctx(ctx));
}

/// Draws a frame from another thread (Foxglove paused or resumed from the tray menu, #404).
pub fn wake() {
    if let Some(w) = WAKE.get() {
        w.request_repaint();
    }
}

/// The tray icon was left-clicked: `position` and `size` are its rectangle in physical pixels.
pub fn clicked(position: [f64; 2], size: [f64; 2]) {
    let min = pos2(position[0] as f32, position[1] as f32);
    let rect = Rect::from_min_size(min, egui::vec2(size[0] as f32, size[1] as f32));
    *CLICKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(rect);
    if let Some(w) = WAKE.get() {
        w.request_repaint();
    }
}

/// The panel while it's open.
struct Open {
    /// The tray icon, in screen points.
    icon: Rect,
    /// The window had focus since the panel opened (so losing it closes the panel).
    focused: bool,
    draft: Draft,
    /// Where the panel was last drawn, in window points.
    rect: Option<Rect>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Draft {
    /// Brightness while its slider is dragged, sent when it's let go.
    pub brightness: Option<u8>,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);

/// When the panel last closed because the window lost focus. A click on the tray icon takes
/// the focus first, so a click that lands just after it closed the panel is the same click
/// asking to close it, not to open it again.
static LOST_FOCUS_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
const SAME_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// Whether the panel is open (the overlay then takes the mouse).
pub fn is_open() -> bool {
    OPEN.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// What the panel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// Left and right: how each half is, and in words.
    pub halves: [(Tone, String); 2],
    pub profiles: Vec<String>,
    pub active_profile: Option<String>,
    /// The left half's lights as last read.
    pub lights: Option<RgbHalf>,
    /// The keymap has an RGB key, and nothing is being sent.
    pub lights_ready: bool,
    pub art_name: String,
    pub art_image: Option<Arc<egui::ColorImage>>,
    /// A keyboard with the art relay answers, and no transfer is running.
    pub art_ready: bool,
    pub panel_paused: bool,
    /// Foxglove is paused (#404).
    pub paused: bool,
    /// "Update available": v…", when there is one.
    pub update: Option<String>,
}

/// What a click asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    Profile(Option<String>),
    Lights(bool),
    Effect(u8),
    Color {
        hue: u16,
        sat: u8,
        brightness: u8,
    },
    NextArt,
    PushFile,
    PauseLeftPanel(bool),
    /// Pause (true) or resume Foxglove (#404).
    Pause(bool),
    OpenFoxglove,
}

/// Next to the title while Foxglove is paused.
pub const PAUSED_BADGE: &str = "PAUSED";

/// The header's pause button: its label and what a click asks for.
pub fn pause_button(paused: bool) -> (&'static str, Intent) {
    if paused {
        ("Resume Foxglove", Intent::Pause(false))
    } else {
        ("Pause Foxglove", Intent::Pause(true))
    }
}

/// Paused, the panel's controls that would change the keyboard rest.
pub fn while_paused(mut v: View) -> View {
    if v.paused {
        v.lights_ready = false;
        v.art_ready = false;
    }
    v
}

/// Both halves in a few words, from the status strip's state.
pub fn halves(s: &StripState) -> [(Tone, String); 2] {
    if !s.keyboard {
        return [
            (Tone::Off, "not connected".into()),
            (Tone::Off, "not connected".into()),
        ];
    }
    let left = super::shell::left_link(s.usb, s.host_link);
    let battery = s.right_battery.map(|b| format!("battery {b}%"));
    let right = match (&s.right_build, battery) {
        (Some(Err(_)), _) => (Tone::Warn, "no reply yet".to_string()),
        (None, None) => (Tone::Off, "asking…".to_string()),
        (_, Some(b)) => (Tone::Ok, b),
        (Some(Ok(_)), None) => (Tone::Ok, "linked".to_string()),
    };
    [(Tone::Ok, left), right]
}

/// The right art part from the art service's snapshot, read afresh each frame: its name, its
/// picture (a library file) and whether Next and Send take a click. Each snapshot change draws a
/// frame (the art service's wake), so the panel follows the keyboard.
fn art(
    s: &super::art_service::Snapshot,
    library: &[super::art_library::Entry],
) -> (String, Option<Arc<egui::ColorImage>>, bool) {
    let ready = s.connected && !s.busy;
    if !s.connected {
        return ("…".to_string(), None, ready);
    }
    let image = s.showing_file().and_then(|path| {
        super::art_ui::tiles(library)
            .0
            .into_iter()
            .find(|t| t.path == *path)
            .map(|t| t.image)
    });
    (super::art_ui::playing(s).0, image, ready)
}

fn view(ctx: &egui::Context) -> View {
    let (profiles, active_profile) = super::store::cached_profiles();
    let lights = super::keyboard_settings::rgb_state(ctx).map(|s| s.left);
    let lights_ready = super::keyboard_actions::underglow_behavior().is_some()
        && !super::keyboard_settings::invoking();
    let (art_name, art_image, art_ready) = match super::art_service::handle() {
        Some(h) => art(&h.snapshot(), &super::art_library::snapshot().entries),
        None => ("…".to_string(), None, false),
    };
    while_paused(View {
        halves: halves(&super::shell::strip_state(ctx)),
        profiles,
        active_profile,
        lights,
        lights_ready,
        art_name,
        art_image,
        art_ready,
        panel_paused: super::screens::paused(),
        paused: super::pause::paused(),
        update: super::update::available_version().map(|v| format!("Update available: v{v}")),
    })
}

/// Draws the panel when it's open, opens it on a tray click, and closes it on Esc, a click
/// outside it or the window losing focus. Returns true when "Open Foxglove" was clicked.
pub fn show(ctx: &egui::Context) -> bool {
    set_wake(ctx);
    // The tray reports physical pixels; the window works in points.
    let ppp = ctx
        .input(|i| i.viewport().native_pixels_per_point)
        .unwrap_or_else(|| ctx.pixels_per_point());
    if let Some(icon) = CLICKED.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
        let just_closed = LOST_FOCUS_AT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .is_some_and(|at| at.elapsed() < SAME_CLICK);
        if open.is_some() || just_closed {
            // A second click on the icon closes it, as Windows' own flyouts do.
            *open = None;
            return false;
        }
        let icon = Rect::from_min_max(
            (icon.min.to_vec2() / ppp).to_pos2(),
            (icon.max.to_vec2() / ppp).to_pos2(),
        );
        *open = Some(Open {
            icon,
            focused: false,
            draft: Draft::default(),
            rect: None,
        });
        // Focus, so a click elsewhere (or Alt+Tab) closes the panel through the focus event.
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }
    let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(open) = guard.as_mut() else {
        return false;
    };
    let focused = ctx.input(|i| i.viewport().focused);
    if focused == Some(true) {
        open.focused = true;
    }
    let lost_focus = open.focused && focused == Some(false);
    if lost_focus {
        *LOST_FOCUS_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
    }
    if lost_focus || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *guard = None;
        return false;
    }
    // The work area the overlay covers, in screen points.
    let work = ctx
        .input(|i| i.viewport().inner_rect)
        .unwrap_or_else(|| ctx.content_rect());
    let id = egui::Id::new("foxglove-tray-panel");
    let size = egui::AreaState::load(ctx, id)
        .and_then(|s| s.size)
        .unwrap_or(egui::vec2(WIDTH, 560.0));
    let at = place(open.icon, work, size) - work.min.to_vec2();
    let v = view(ctx);
    let mut intents = Vec::new();
    let response = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| panel(ui, &v, &mut open.draft, &mut intents))
        .response;
    open.rect = Some(response.rect);
    let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !response.rect.contains(p));
    drop(guard);
    let mut open_settings = false;
    for intent in intents {
        open_settings |= act(ctx, intent);
    }
    if open_settings || clicked_outside {
        *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
        ctx.request_repaint();
    }
    open_settings
}

/// The panel's corner radius, in points.
const CORNER: u8 = 10;

/// While the panel is open and drawn: the part of the overlay window that takes the mouse, in
/// window pixels (see [`region`]).
pub fn hit_region(ctx: &egui::Context) -> Option<[i32; 5]> {
    let rect = OPEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .rect?;
    Some(region(rect, ctx.pixels_per_point()))
}

/// A panel rectangle in window points as a window region in pixels: left, top, right, bottom
/// (right and bottom exclusive, rounded outward so no pixel of the panel falls outside) and
/// the corner's diameter.
pub fn region(rect: Rect, ppp: f32) -> [i32; 5] {
    let px = |v: f32| v * ppp;
    [
        px(rect.left()).floor() as i32,
        px(rect.top()).floor() as i32,
        px(rect.right()).ceil() as i32,
        px(rect.bottom()).ceil() as i32,
        (px(f32::from(CORNER)) * 2.0).round() as i32,
    ]
}

/// Carries out a click; true when the settings window should open.
fn act(ctx: &egui::Context, intent: Intent) -> bool {
    use super::keyboard_settings as ks;
    match intent {
        Intent::Profile(name) => {
            if let Err(e) = super::store::set_active(name.as_deref()) {
                eprintln!("Foxglove: can't switch profile: {e}");
            }
        }
        Intent::Lights(on) => ks::set_lights(on, ctx.clone()),
        Intent::Effect(e) => ks::set_effect(e, ctx.clone()),
        Intent::Color {
            hue,
            sat,
            brightness,
        } => ks::set_color(hue, sat, brightness, ctx.clone()),
        Intent::NextArt => {
            if let Some(h) = super::art_service::handle() {
                let _ = h.next();
            }
        }
        Intent::PushFile => {
            // The file picker lives with the Screens page, as the tray menu's Push file... does.
            super::art_library::PICK_FILE.store(true, std::sync::atomic::Ordering::Relaxed);
            super::shell::set_page(super::shell::Page::Screens);
            return true;
        }
        Intent::PauseLeftPanel(on) => super::screens::set_paused(on),
        Intent::Pause(on) => super::pause::set(on),
        Intent::OpenFoxglove => return true,
    }
    false
}

/// A part of the panel: a line above it and a small label (not a settings section).
fn part(
    ui: &mut egui::Ui,
    label: &str,
    right: impl FnOnce(&mut egui::Ui),
    add: impl FnOnce(&mut egui::Ui),
) {
    let p = theme::palette(ui);
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, 0.0, p.line);
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(theme::caps_label(ui, label));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
            });
            ui.add_space(4.0);
            add(ui);
        });
}

/// Paints the panel; returns what was clicked.
pub fn panel(ui: &mut egui::Ui, v: &View, draft: &mut Draft, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    egui::Frame::new()
        .fill(p.ground)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(CornerRadius::same(CORNER))
        .shadow(ui.visuals().popup_shadow)
        .show(ui, |ui| {
            ui.set_width(WIDTH);
            egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // #404: paused, the title dims and says so.
                    let title = if v.paused { p.muted } else { p.text_strong };
                    ui.label(RichText::new("FOXGLOVE").font(theme::hud(17.0)).color(title));
                    if v.paused {
                        ui.label(RichText::new(PAUSED_BADGE).font(theme::hud(13.0)).color(p.warn));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // #404: paused, the button is the selected pill that resumes.
                        let (label, intent) = pause_button(v.paused);
                        if theme::pill(ui, v.paused, label).clicked() {
                            out.push(intent);
                        }
                    });
                });
                if v.paused {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Paused. Foxglove leaves the keyboard alone: its own lights, the left panel back to the firmware, no art changes and no layer overlay.")
                            .color(p.warn),
                    );
                }
                ui.add_space(6.0);
                ui.columns(2, |cols| {
                    for (col, (name, (tone, text))) in cols.iter_mut().zip(["Left", "Right"].into_iter().zip(&v.halves)) {
                        half_chip(col, name, *tone, text);
                    }
                });
            });
            part(ui, "Profile", |_| {}, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if theme::pill(ui, v.active_profile.is_none(), "Base settings").clicked() && v.active_profile.is_some() {
                        out.push(Intent::Profile(None));
                    }
                    for name in &v.profiles {
                        let on = v.active_profile.as_deref() == Some(name.as_str());
                        if theme::pill(ui, on, name).clicked() && !on {
                            out.push(Intent::Profile(Some(name.clone())));
                        }
                    }
                });
            });
            lights(ui, v, draft, out);
            part(ui, "Right art", |_| {}, |ui| {
                ui.horizontal(|ui| {
                    art_thumb(ui, v.art_image.as_ref());
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&v.art_name).font(theme::hud(15.0)).color(p.text_strong));
                        ui.add_enabled_ui(v.art_ready, |ui| {
                            ui.horizontal(|ui| {
                                if ui.button("Next").clicked() {
                                    out.push(Intent::NextArt);
                                }
                                if ui.button("Push file...").clicked() {
                                    out.push(Intent::PushFile);
                                }
                            });
                        });
                    });
                });
            });
            part(
                ui,
                "Left panel",
                |ui| {
                    let text = if v.panel_paused { "Resume left panel" } else { "Pause left panel" };
                    if ui.add_enabled(!v.paused, egui::Button::new(text)).clicked() {
                        out.push(Intent::PauseLeftPanel(!v.panel_paused));
                    }
                },
                |_| {},
            );
            let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, p.line);
            egui::Frame::new().fill(p.well).inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let open = ui.add(egui::Button::new(RichText::new("Open Foxglove →").color(p.accent)).frame(false));
                    if open.clicked() {
                        out.push(Intent::OpenFoxglove);
                    }
                    if let Some(u) = &v.update {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(u).small().color(p.muted));
                        });
                    }
                });
            });
        });
}

fn half_chip(ui: &mut egui::Ui, name: &str, tone: Tone, text: &str) {
    let p = theme::palette(ui);
    theme::card(ui)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                let color = match tone {
                    Tone::Ok => p.ok,
                    Tone::Warn => p.warn,
                    Tone::Off => p.muted,
                };
                ui.painter().circle_filled(dot.center(), 4.0, color);
                ui.label(RichText::new(name).strong().color(p.text_strong));
            });
            ui.label(text);
        });
}

fn lights(ui: &mut egui::Ui, v: &View, draft: &mut Draft, out: &mut Vec<Intent>) {
    let left = v.lights;
    let mut switched = None;
    part(
        ui,
        "Lights",
        |ui| {
            let mut on = left.is_some_and(|l| l.on);
            if ui
                .add_enabled(
                    v.lights_ready && left.is_some(),
                    egui::Checkbox::new(&mut on, "On"),
                )
                .changed()
            {
                switched = Some(on);
            }
        },
        |ui| {
            let Some(l) = left else {
                ui.weak("The lights show here once the keyboard reports them.");
                return;
            };
            ui.add_enabled_ui(v.lights_ready, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (i, name) in (0u8..).zip(super::lights_page::EFFECTS) {
                        if theme::pill(ui, l.effect == i, name).clicked() && l.effect != i {
                            out.push(Intent::Effect(i));
                        }
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for (name, rgb) in super::lights_page::SWATCHES {
                        let selected = super::lights_page::is_swatch((l.hue, l.sat), rgb);
                        let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                        if super::lights_page::swatch(ui, color, selected)
                            .on_hover_text(name)
                            .clicked()
                            && !selected
                        {
                            let (hue, sat) = super::accent::hue_sat(rgb[0], rgb[1], rgb[2]);
                            out.push(Intent::Color {
                                hue,
                                sat,
                                brightness: l.brightness,
                            });
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label(theme::caps_label(ui, "Brightness"));
                let mut b = draft.brightness.unwrap_or(l.brightness);
                ui.spacing_mut().slider_width = ui.available_width();
                let r = ui.add(egui::Slider::new(&mut b, 0..=100).show_value(false));
                if r.changed() {
                    draft.brightness = Some(b);
                }
                // Sent when the drag ends (or on a key press), not on every step.
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    draft.brightness = None;
                    if b != l.brightness {
                        out.push(Intent::Color {
                            hue: l.hue,
                            sat: l.sat,
                            brightness: b,
                        });
                    }
                }
            });
        },
    );
    if let Some(on) = switched {
        out.push(Intent::Lights(on));
    }
}

thread_local! {
    /// The art thumbnail's texture, by the picture it was made from.
    /// Kept with the picture itself, so a freed picture's address can't pass for a new one.
    static ART: std::cell::RefCell<Option<(Arc<egui::ColorImage>, egui::TextureHandle)>> = const { std::cell::RefCell::new(None) };
}

/// The art playing on the right, drawn in the screen's two tones; a dark box when unknown.
fn art_thumb(ui: &mut egui::Ui, image: Option<&Arc<egui::ColorImage>>) {
    let p = theme::palette(ui);
    let size = egui::vec2(54.0, 112.0);
    match image {
        Some(stored) => {
            // Made once per picture, not every frame.
            let t = ART.with_borrow_mut(|cache| match cache {
                Some((k, t)) if Arc::ptr_eq(k, stored) => t.clone(),
                _ => {
                    let half = super::art_ui::thumbnail(stored, true);
                    let t = ui.ctx().load_texture(
                        "foxglove-tray-art",
                        half,
                        egui::TextureOptions::NEAREST,
                    );
                    *cache = Some((stored.clone(), t.clone()));
                    t
                }
            });
            ui.add(egui::Image::new((t.id(), t.size_vec2())).corner_radius(3));
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect(
                rect,
                CornerRadius::same(3),
                Color32::from_rgb(0x0b, 0x0d, 0x0c),
                Stroke::new(1.0, p.control_line),
                egui::StrokeKind::Inside,
            );
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), egui::vec2(w, h))
    }

    /// A 1920x1080 screen whose work area leaves a 48-point taskbar on one edge.
    fn screen(edge: Edge) -> Rect {
        match edge {
            Edge::Bottom => r(0.0, 0.0, 1920.0, 1032.0),
            Edge::Top => r(0.0, 48.0, 1920.0, 1032.0),
            Edge::Left => r(48.0, 0.0, 1872.0, 1080.0),
            Edge::Right => r(0.0, 0.0, 1872.0, 1080.0),
        }
    }

    const PANEL: Vec2 = egui::vec2(400.0, 600.0);

    #[test]
    fn the_panel_opens_against_the_taskbar_next_to_the_icon() {
        // Bottom: the icon near the right end of the taskbar; the panel above it, kept on screen.
        let work = screen(Edge::Bottom);
        let icon = r(1700.0, 1040.0, 24.0, 32.0);
        assert_eq!(edge(icon, work), Edge::Bottom);
        let at = place(icon, work, PANEL);
        assert_eq!(at, pos2(1920.0 - 400.0 - MARGIN, 1032.0 - 600.0 - MARGIN));
        // In the middle of the taskbar it centres on the icon.
        let at = place(r(900.0, 1040.0, 24.0, 32.0), work, PANEL);
        assert_eq!(at.x, 912.0 - 200.0);

        let work = screen(Edge::Top);
        let icon = r(1700.0, 8.0, 24.0, 32.0);
        assert_eq!(edge(icon, work), Edge::Top);
        assert_eq!(
            place(icon, work, PANEL),
            pos2(1920.0 - 400.0 - MARGIN, 48.0 + MARGIN)
        );

        let work = screen(Edge::Left);
        let icon = r(12.0, 1000.0, 24.0, 24.0);
        assert_eq!(edge(icon, work), Edge::Left);
        assert_eq!(
            place(icon, work, PANEL),
            pos2(48.0 + MARGIN, 1080.0 - 600.0 - MARGIN)
        );

        let work = screen(Edge::Right);
        let icon = r(1884.0, 300.0, 24.0, 24.0);
        assert_eq!(edge(icon, work), Edge::Right);
        assert_eq!(
            place(icon, work, PANEL),
            pos2(1872.0 - 400.0 - MARGIN, 312.0 - 300.0)
        );
    }

    #[test]
    fn a_hidden_taskbar_or_a_tiny_screen_still_fits() {
        // An auto-hiding taskbar: the icon lies inside the work area, near its bottom.
        let work = r(0.0, 0.0, 1920.0, 1080.0);
        let icon = r(1700.0, 1050.0, 24.0, 24.0);
        assert_eq!(edge(icon, work), Edge::Bottom);
        let at = place(icon, work, PANEL);
        assert!(Rect::from_min_size(at, PANEL).max.y <= work.bottom());
        // A work area smaller than the panel: pinned to its top-left, never off the top.
        let small = r(0.0, 0.0, 300.0, 400.0);
        let at = place(r(280.0, 410.0, 16.0, 16.0), small, PANEL);
        assert!(at.y >= small.top() && at.x >= small.left(), "{at:?}");
    }

    #[test]
    fn the_pause_button_pauses_and_resumes() {
        assert_eq!(pause_button(false), ("Pause Foxglove", Intent::Pause(true)));
        assert_eq!(
            pause_button(true),
            ("Resume Foxglove", Intent::Pause(false))
        );
    }

    /// The right art part names what the snapshot says plays now: after Next (a new status)
    /// it names the new set, not the one the panel opened with.
    #[test]
    fn the_art_part_follows_the_art_snapshot() {
        use digitalis::protocol::ArtStatus;
        let status = |set| ArtStatus {
            link: 2,
            right_state: 0,
            right_set: set,
            right_set_count: 12,
            ..ArtStatus::default()
        };
        let mut s = crate::foxglove::art_service::Snapshot {
            connected: true,
            status: Some(status(1)),
            ..Default::default()
        };
        let before = art(&s, &[]).0;
        s.status = Some(status(4));
        let after = art(&s, &[]).0;
        assert_ne!(before, after);
        assert_eq!(after, crate::foxglove::art_ui::playing(&s).0);
        s.busy = true;
        assert!(!art(&s, &[]).2, "no clicks while the art is being sent");
        s.connected = false;
        assert_eq!(art(&s, &[]).0, "…");
    }

    #[test]
    fn paused_the_lights_and_art_rest() {
        let v = while_paused(sample());
        assert!(v.lights_ready && v.art_ready, "not paused: as they were");
        let v = while_paused(View {
            paused: true,
            ..sample()
        });
        assert!(!v.lights_ready && !v.art_ready);
        assert_eq!(v.profiles, sample().profiles, "profiles stay");
    }

    #[test]
    fn the_hit_region_covers_the_panel_in_pixels() {
        // 100 %: the panel's own rectangle; the corner is the radius doubled.
        assert_eq!(
            region(r(1508.0, 420.0, 400.0, 600.0), 1.0),
            [1508, 420, 1908, 1020, 20]
        );
        // 150 %: points become pixels, rounded outward.
        assert_eq!(
            region(r(10.3, 20.6, 400.0, 600.0), 1.5),
            [15, 30, 616, 931, 30]
        );
        // 125 % with a fractional size: the right and bottom edges round up.
        let [l, t, rgt, b, c] = region(r(0.0, 0.0, 400.4, 559.9), 1.25);
        assert_eq!((l, t, rgt, b, c), (0, 0, 501, 700, 25));
    }

    #[test]
    fn no_hit_region_while_the_panel_is_closed() {
        let ctx = egui::Context::default();
        assert_eq!(hit_region(&ctx), None);
    }

    #[test]
    fn halves_in_a_few_words() {
        assert_eq!(
            halves(&StripState::default()).map(|h| h.1),
            ["not connected", "not connected"]
        );
        let s = StripState {
            keyboard: true,
            usb: Some(true),
            right_battery: Some(61),
            right_build: Some(Ok("x".into())),
            ..Default::default()
        };
        assert_eq!(
            halves(&s),
            [
                (Tone::Ok, "on USB".to_string()),
                (Tone::Ok, "battery 61%".to_string())
            ]
        );
        let s = StripState {
            keyboard: true,
            usb: Some(false),
            right_build: Some(Err("no reply".into())),
            ..Default::default()
        };
        assert_eq!(
            halves(&s),
            [
                (Tone::Ok, "on Bluetooth".to_string()),
                (Tone::Warn, "no reply yet".to_string())
            ]
        );
    }

    /// The previews' panel: both halves seen, two profiles, the lights on, art playing.
    pub fn sample() -> View {
        let (snapshot, library) = super::super::art_ui::tests::sample();
        let image = snapshot.showing_file().and_then(|p| {
            super::super::art_ui::tiles(&library.entries)
                .0
                .into_iter()
                .find(|t| t.path == *p)
                .map(|t| t.image)
        });
        View {
            halves: [
                (Tone::Ok, "on USB".into()),
                (Tone::Ok, "battery 61%".into()),
            ],
            profiles: vec!["Work".into(), "Game".into()],
            active_profile: None,
            lights: Some(RgbHalf {
                on: true,
                hue: 26,
                sat: 76,
                brightness: 80,
                effect: 0,
                speed: 3,
            }),
            lights_ready: true,
            art_name: super::super::art_ui::playing(&snapshot).0,
            art_image: image,
            art_ready: true,
            panel_paused: false,
            paused: false,
            update: Some("Update available: v0.2.1".into()),
        }
    }
}
