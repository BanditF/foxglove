//! The Screens page (#13, #388; the art flow's tiers in #516): **Showing now** with both
//! screens side by side and Rotation, then the right screen's art on the keyboard and on this
//! PC (`art_ui`), then the left screen's settings (its switches and the per-game hints). The
//! left screen's parts show only while a keyboard with the host link has answered HELLO.
//!
//! Changes are saved to `screens.ini`, which the panel service watches, so the service never
//! needs a channel from the UI. The preview redraws at the panel's own rate only while this
//! section is on screen.

use std::cell::RefCell;

use super::screens::{self, Frame};
use super::screens_settings::ScreensSettings;

#[derive(Default)]
struct UiState {
    /// The settings as last loaded or saved; reloaded when the section opens.
    settings: Option<ScreensSettings>,
    new_exe: String,
    new_hint: String,
}

thread_local! { static UI: RefCell<UiState> = RefCell::new(UiState::default()); }

fn right_link(link: u8) -> &'static str {
    match link {
        0 => "not connected",
        1 => "connected",
        _ => "connected, art relay ready",
    }
}

#[cfg(test)]
thread_local! {
    /// The previews' keyboard, so they never ask a real one.
    pub static PREVIEW: RefCell<Option<screens::Snapshot>> = const { RefCell::new(None) };
}

/// The Screens page: Showing now, On the keyboard, On this PC, then the left screen's card and
/// Notifications (#507).
pub fn page(ui: &mut egui::Ui) {
    showing_now(ui);
    super::art_ui::keyboard_card(ui);
    super::art_ui::pc_card(ui);
    group(ui);
    super::notifications_ui::card(ui);
}

/// Showing now: the left screen, the right screen and Rotation in three columns, stacked when
/// the window is narrow. Drawn once either screen's keyboard answers.
fn showing_now(ui: &mut egui::Ui) {
    let left = snapshot().hello.is_some();
    let right = super::art_ui::current(ui.ctx()).is_some_and(|a| a.s.connected);
    if !left && !right {
        return;
    }
    super::settings_ui::section(ui, "Showing now", |ui| {
        let gap = 18.0;
        let width = ui.available_width();
        type Part = fn(&mut egui::Ui);
        let parts: [(f32, Part); 3] = [
            (0.27, now_left),
            (0.43, super::art_ui::now_right),
            (0.30, super::art_ui::rotation),
        ];
        if width < 720.0 {
            for (_, part) in parts {
                part(ui);
                ui.add_space(10.0);
            }
            return;
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let room = width - gap * 2.0;
            for (share, part) in parts {
                let w = (room * share).floor();
                ui.allocate_ui_with_layout(
                    egui::vec2(w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(w);
                        part(ui);
                    },
                );
            }
        });
    });
}

/// What the left screen shows, in a name and a line.
pub fn left_words(settings: &ScreensSettings, paused: bool) -> (&'static str, &'static str) {
    use super::screens_settings::Layout;
    if !settings.drive_left {
        return ("Clock and status", "The keyboard's own screen.");
    }
    let name = match settings.layout {
        Layout::Info => "Clock, media and hints",
        Layout::Wallpaper => "Wallpaper animation",
        Layout::Stats => "PC load",
    };
    if paused {
        (name, "Paused: the keyboard shows its own screen meanwhile.")
    } else {
        (name, "Drawn by Foxglove while it runs on this PC.")
    }
}

/// The left screen's column of Showing now: its live picture and what it shows.
fn now_left(ui: &mut egui::Ui) {
    let snap = snapshot();
    let p = super::theme::palette(ui);
    ui.label(super::theme::caps_label(ui, "Left screen"));
    if snap.hello.is_none() {
        ui.weak("Waiting for the keyboard.");
        return;
    }
    UI.with_borrow_mut(|state| {
        let settings = state
            .settings
            .get_or_insert_with(ScreensSettings::load)
            .clone();
        match snap.frame.as_ref().filter(|_| settings.drive_left) {
            Some(frame) => {
                let t = frame_texture_here(ui.ctx(), frame);
                ui.add(egui::Image::new((t.id(), t.size_vec2() * 2.0)).corner_radius(3));
                // Repainted when the panel gets a new frame, only while this part is drawn.
                super::screens::want_preview(crate::ui_wake::UiWake::from_ctx(ui.ctx()));
            }
            None => {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(64.0, 52.0) * 2.0, egui::Sense::hover());
                ui.painter().rect(
                    rect,
                    4.0,
                    egui::Color32::BLACK,
                    egui::Stroke::new(1.0, p.control_line),
                    egui::StrokeKind::Inside,
                );
                let words = if settings.drive_left {
                    "No picture yet"
                } else {
                    "Keyboard's own"
                };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    words,
                    super::theme::body(12.0),
                    p.muted,
                );
            }
        }
        let (name, line) = left_words(&settings, screens::paused());
        ui.label(
            egui::RichText::new(name)
                .font(super::theme::hud(15.0))
                .color(p.text_strong),
        );
        ui.weak(line);
        if let Some(pick) = change_picker(ui, &settings) {
            let mut new = settings.clone();
            pick.apply(&mut new);
            match new.save() {
                Ok(()) => {
                    remember(&new);
                    state.settings = Some(new);
                }
                Err(e) => eprintln!("Foxglove: can't save the left screen: {e}"),
            }
        }
    });
}

/// What the left screen can show (#521, the Change… picker).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftChoice {
    /// The keyboard's own screen: Foxglove doesn't draw.
    KeyboardOwn,
    Info,
    Wallpaper,
    /// CPU, GPU and memory use (#506).
    Stats,
}

impl LeftChoice {
    pub const ALL: [LeftChoice; 4] = [
        LeftChoice::KeyboardOwn,
        LeftChoice::Info,
        LeftChoice::Wallpaper,
        LeftChoice::Stats,
    ];

    pub fn of(settings: &ScreensSettings) -> LeftChoice {
        use super::screens_settings::Layout;
        match (settings.drive_left, settings.layout) {
            (false, _) => LeftChoice::KeyboardOwn,
            (true, Layout::Info) => LeftChoice::Info,
            (true, Layout::Wallpaper) => LeftChoice::Wallpaper,
            (true, Layout::Stats) => LeftChoice::Stats,
        }
    }

    pub fn apply(self, settings: &mut ScreensSettings) {
        use super::screens_settings::Layout;
        match self {
            LeftChoice::KeyboardOwn => settings.drive_left = false,
            LeftChoice::Info => {
                settings.drive_left = true;
                settings.layout = Layout::Info;
            }
            LeftChoice::Wallpaper => {
                settings.drive_left = true;
                settings.layout = Layout::Wallpaper;
            }
            LeftChoice::Stats => {
                settings.drive_left = true;
                settings.layout = Layout::Stats;
            }
        }
    }

    /// Its name and what it needs.
    pub fn words(self) -> (&'static str, &'static str) {
        match self {
            LeftChoice::KeyboardOwn => (
                "Clock and status",
                "The keyboard's own screen; needs nothing.",
            ),
            LeftChoice::Info => (
                "Clock, media and hints",
                "Drawn by Foxglove while it runs on this PC.",
            ),
            LeftChoice::Wallpaper => (
                "Wallpaper animation",
                "The wallpaper's own left frames, while Foxglove runs; the clock when it has none.",
            ),
            LeftChoice::Stats => (
                "PC load",
                "CPU, graphics and memory use, once a second while it shows. Stops while the PC is locked or the screen is off.",
            ),
        }
    }
}

/// The PC load the previews show.
pub fn sample_stats() -> foxglove_panel::stats::Stats {
    foxglove_panel::stats::Stats {
        cpu: Some(37),
        gpu: Some(64),
        ram: Some(52),
    }
}

/// A preview of a left-screen choice, as the panel draws it (64×52, ink on black).
fn left_preview(choice: LeftChoice) -> egui::ColorImage {
    use foxglove_panel::render::{self, Panel, View};
    let mut panel = Panel::new(64, 52);
    match choice {
        LeftChoice::Info => {
            let view = View {
                time: "14:32".into(),
                date: "Mon 27 Oct".into(),
                date_short: "27 Oct".into(),
                title: Some("Bandit radio".into()),
                artist: Some("Foxglove".into()),
                playing: true,
                ..View::default()
            };
            render::draw(&mut panel, &view, 0);
        }
        LeftChoice::Wallpaper => {
            let monitor = super::art_cycle::Cycle::load().wallpaper_monitor;
            let first = super::wallpaper::current(monitor.as_deref())
                .and_then(|w| w.left)
                .and_then(|left| super::wallpaper::left_frames(&left, panel.bits.len()))
                .and_then(|frames| frames.into_iter().next());
            match first {
                Some(bits) => panel.bits = bits,
                // No wallpaper frames now: what it falls back to.
                None => render::draw(
                    &mut panel,
                    &View {
                        time: "14:32".into(),
                        ..View::default()
                    },
                    0,
                ),
            }
        }
        LeftChoice::Stats => render::draw_stats(&mut panel, &sample_stats()),
        LeftChoice::KeyboardOwn => {}
    }
    frame_image(&Frame {
        width: panel.width,
        height: panel.height,
        stride: panel.stride,
        bits: panel.bits,
    })
}

/// The Change… button and its list, each choice with a preview; the one clicked, if any.
fn change_picker(ui: &mut egui::Ui, settings: &ScreensSettings) -> Option<LeftChoice> {
    let mut picked = None;
    let now = LeftChoice::of(settings);
    // The previews are made when the list opens (the wallpaper's may have changed since).
    let open_id = egui::Id::new("left-picker-open");
    let was_open = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(open_id))
        .unwrap_or(false);
    let mut open = false;
    ui.menu_button("Change…", |ui| {
        open = true;
        if !was_open {
            ui.ctx().data_mut(|d| {
                for choice in LeftChoice::ALL {
                    d.remove::<egui::TextureHandle>(egui::Id::new(("left-preview", choice as u8)));
                }
            });
        }
        picked = left_choices(ui, now);
        if picked.is_some() {
            ui.close();
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(open_id, open));
    picked
}

/// The picker's list: each choice with its preview, name and what it needs.
pub(crate) fn left_choices(ui: &mut egui::Ui, now: LeftChoice) -> Option<LeftChoice> {
    let mut picked = None;
    {
        let p = super::theme::palette(ui);
        ui.set_min_width(330.0);
        for choice in LeftChoice::ALL {
            let (name, needs) = choice.words();
            let r = ui
                .horizontal(|ui| {
                    let id = egui::Id::new(("left-preview", choice as u8));
                    let t = ui
                        .ctx()
                        .data(|d| d.get_temp::<egui::TextureHandle>(id))
                        .unwrap_or_else(|| {
                            let t = ui.ctx().load_texture(
                                format!("left-preview-{name}"),
                                left_preview(choice),
                                egui::TextureOptions::NEAREST,
                            );
                            ui.ctx().data_mut(|d| d.insert_temp(id, t.clone()));
                            t
                        });
                    let size = egui::vec2(64.0, 52.0) * 1.5;
                    if choice == LeftChoice::KeyboardOwn {
                        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                        ui.painter().rect(
                            rect,
                            3.0,
                            egui::Color32::BLACK,
                            egui::Stroke::new(1.0, p.control_line),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "its own\nscreen",
                            super::theme::body(11.0),
                            p.muted,
                        );
                    } else {
                        ui.add(egui::Image::new((t.id(), size)).corner_radius(3));
                    }
                    ui.vertical(|ui| {
                        let color = if choice == now {
                            p.accent
                        } else {
                            p.text_strong
                        };
                        ui.label(egui::RichText::new(name).color(color));
                        ui.label(egui::RichText::new(needs).small().color(p.muted));
                    });
                })
                .response;
            let r = ui.interact(
                r.rect,
                egui::Id::new(("left-choice", choice as u8)),
                egui::Sense::click(),
            );
            if r.hovered() {
                ui.painter().rect_stroke(
                    r.rect.expand(2.0),
                    4.0,
                    egui::Stroke::new(1.0, p.accent),
                    egui::StrokeKind::Outside,
                );
            }
            if r.clicked() && choice != now {
                picked = Some(choice);
            }
            ui.add_space(4.0);
        }
    }
    picked
}

/// The left screen's settings as last loaded or saved here, for the tray panel's line. Only
/// the line's words come from it; a change made outside Foxglove's pages shows after a restart.
static LEFT: std::sync::Mutex<Option<ScreensSettings>> = std::sync::Mutex::new(None);

fn remember(settings: &ScreensSettings) {
    *LEFT.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings.clone());
}

/// What the left screen shows, in words, without reading the store every frame.
pub fn left_now() -> (&'static str, &'static str, bool) {
    let settings = LEFT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(ScreensSettings::load)
        .clone();
    let (name, line) = left_words(&settings, screens::paused());
    (name, line, settings.drive_left)
}

pub(super) fn snapshot() -> screens::Snapshot {
    #[cfg(test)]
    if let Some(s) = PREVIEW.with_borrow(|s| s.clone()) {
        return s;
    }
    screens::snapshot()
}

/// The left screen's card: the live preview, its switches, and the app hints.
pub fn group(ui: &mut egui::Ui) {
    let snap = snapshot();
    let Some(hello) = snap.hello else {
        UI.with_borrow_mut(|s| s.settings = None);
        return;
    };
    super::settings_ui::section(ui, "Left screen", |ui| {
        UI.with_borrow_mut(|state| {
            let settings = state.settings.get_or_insert_with(ScreensSettings::load);
            let mut changed = false;

            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut settings.drive_left, "On").changed();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let paused = screens::paused();
                    let button = ui.add_enabled(settings.drive_left, egui::Button::new(if paused { "Resume" } else { "Pause" }));
                    if button.on_hover_text("Pause the left screen; the keyboard shows its own screen meanwhile").clicked() {
                        screens::set_paused(!paused);
                    }
                });
            });
            ui.add_enabled_ui(settings.drive_left, |ui| {
                use super::screens_settings::Layout;
                ui.horizontal(|ui| {
                    ui.label("Show");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let shown = match settings.layout {
                            Layout::Info => "Clock, media and hints",
                            Layout::Wallpaper => "Wallpaper animation",
                            Layout::Stats => "PC load",
                        };
                        egui::ComboBox::from_id_salt("screens-show").selected_text(shown).show_ui(ui, |ui| {
                            changed |= ui.selectable_value(&mut settings.layout, Layout::Info, "Clock, media and hints").changed();
                            changed |= ui
                                .selectable_value(&mut settings.layout, Layout::Wallpaper, "Wallpaper animation")
                                .on_hover_text(
                                    "The left frames of the wallpaper on screen (wallpapers.json in the art folder); the clock when it has none. Works best with Wallpaper Engine; without it, the Windows desktop wallpaper stands in.",
                                )
                                .changed();
                            changed |= ui
                                .selectable_value(&mut settings.layout, Layout::Stats, "PC load")
                                .on_hover_text("CPU, graphics and memory use, once a second while it shows")
                                .changed();
                        });
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("Frames per second");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        changed |= ui.add(egui::Slider::new(&mut settings.fps, 1..=10)).changed();
                    });
                });
            });

            ui.add_space(6.0);
            ui.label(super::theme::caps_label(ui, "App hints"));
            ui.weak("Shown on the left screen while that program is in front.");
            let mut remove = None;
            for (exe, hint) in settings.games.iter_mut() {
                ui.horizontal(|ui| {
                    ui.label(exe.as_str());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Remove").clicked() {
                            remove = Some(exe.clone());
                        }
                        changed |= ui.add(egui::TextEdit::singleline(hint).desired_width(160.0)).lost_focus();
                    });
                });
            }
            if let Some(exe) = remove {
                settings.games.remove(&exe);
                changed = true;
            }
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut state.new_exe).hint_text("game.exe").desired_width(110.0));
                ui.add(egui::TextEdit::singleline(&mut state.new_hint).hint_text("hint").desired_width(140.0));
                let exe = state.new_exe.trim().to_lowercase();
                if ui.add_enabled(!exe.is_empty(), egui::Button::new("Add")).clicked() {
                    settings.games.insert(exe, state.new_hint.trim().to_string());
                    state.new_exe.clear();
                    state.new_hint.clear();
                    changed = true;
                }
            });
            if let Some(exe) = snap.foreground.as_ref() {
                if ui.button("Add foreground app").on_hover_text(exe).clicked() {
                    state.new_exe.clone_from(exe);
                }
            }

            // What the keyboard said, for when something looks wrong.
            egui::CollapsingHeader::new("Details").id_salt("screens-details").show(ui, |ui| {
                ui.weak(screens::status());
                egui::Grid::new("screens-status").num_columns(2).show(ui, |ui| {
                    ui.weak("Keyboard");
                    ui.label(format!("{} · host link v{}", snap.product, hello.version));
                    ui.end_row();
                    // The board profile (#75): what Foxglove knows beyond the firmware.
                    ui.weak("Board type");
                    ui.label(match super::boards::current() {
                        Some(b) if b.status == super::boards::Status::Tested => b.name,
                        Some(b) => format!("{} (untested)", b.name),
                        None => "none: only what the firmware says".into(),
                    });
                    ui.end_row();
                    ui.weak("Right half");
                    ui.label(right_link(hello.right_link));
                    ui.end_row();
                    ui.weak("Right battery");
                    ui.label(hello.right_battery.map_or("unknown".into(), |b| format!("{b}%")));
                    ui.end_row();
                });
                ui.weak(format!("Relay discovery: {} ({} tries)", hello.discovery_text(), hello.discovery_tries));
                ui.weak(format!("Relay counters: {:?}", hello.debug));
                ui.weak(format!("Capabilities: {:#x}", hello.capabilities));
                for s in &snap.screens {
                    use digitalis::protocol::ScreenKind;
                    let kind = match s.kind {
                        ScreenKind::Display => "display",
                        ScreenKind::Panel => "PC panel",
                        ScreenKind::Art => "art area",
                        ScreenKind::Other(_) => "other",
                    };
                    let half = if s.half == 0 { "left" } else { "right" };
                    let on = s.parent.map_or(String::new(), |p| format!(" at ({}, {}) on screen {p}", s.x, s.y));
                    ui.weak(format!("Screen {}: {half} {kind}, {}×{}, {}{on}", s.id, s.width, s.height, s.format_name()));
                }
            });

            if changed {
                match settings.save() {
                    Ok(()) => remember(settings),
                    Err(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, format!("Can't save: {e}"));
                    }
                }
            }
        });
    });
}

/// The preview texture, rebuilt only when the frame changed.
pub(crate) fn frame_texture<'a>(
    ctx: &egui::Context,
    cached: &'a mut Option<(Frame, egui::TextureHandle)>,
    frame: &Frame,
) -> &'a egui::TextureHandle {
    if cached.as_ref().is_none_or(|(f, _)| f != frame) {
        let image = frame_image(frame);
        let texture = ctx.load_texture("screens-preview", image, egui::TextureOptions::NEAREST);
        *cached = Some((frame.clone(), texture));
    }
    &cached.as_ref().unwrap().1
}

/// [`frame_texture`] with the cache kept in the context it was made for, so a fresh context
/// (each preview's) never draws another's texture.
fn frame_texture_here(ctx: &egui::Context, frame: &Frame) -> egui::TextureHandle {
    let id = egui::Id::new("foxglove-left-screen-texture");
    let mut cached = ctx
        .data(|d| d.get_temp::<Option<(Frame, egui::TextureHandle)>>(id))
        .flatten();
    let t = frame_texture(ctx, &mut cached, frame).clone();
    ctx.data_mut(|d| d.insert_temp(id, cached));
    t
}

/// The panel's 1-bit rows (MSB first) as an image: ink white on black, like the nice!view.
fn frame_image(frame: &Frame) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(frame.width * frame.height);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let on = frame
                .bits
                .get(y * frame.stride + x / 8)
                .is_some_and(|b| b & (0x80 >> (x % 8)) != 0);
            pixels.push(if on {
                egui::Color32::WHITE
            } else {
                egui::Color32::BLACK
            });
        }
    }
    egui::ColorImage::new([frame.width, frame.height], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_bits_map_msb_first() {
        let frame = Frame {
            width: 10,
            height: 2,
            stride: 2,
            bits: vec![0x80, 0x40, 0x00, 0x01],
        };
        let image = frame_image(&frame);
        let on = |x: usize, y: usize| image.pixels[y * 10 + x] == egui::Color32::WHITE;
        assert!(on(0, 0) && on(9, 0) && !on(1, 0));
        assert!(!on(0, 1) && !on(8, 1));
    }

    /// The Change… picker's choices set the settings the panel service reads, one click each.
    #[test]
    fn left_choices_round_trip() {
        let mut s = ScreensSettings::default();
        for choice in LeftChoice::ALL {
            choice.apply(&mut s);
            assert_eq!(LeftChoice::of(&s), choice);
            assert_eq!(
                left_words(&s, false).0,
                choice.words().0,
                "one name everywhere"
            );
        }
        let info = left_preview(LeftChoice::Info);
        assert_eq!(info.size, [64, 52]);
        assert!(
            info.pixels.contains(&egui::Color32::WHITE),
            "the clock preview draws something"
        );
    }
}
