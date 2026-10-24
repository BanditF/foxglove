//! The Screens settings (#13): the left panel's status, its controls, a live preview, and the
//! per-game hints. Shown only while a keyboard with the host link has answered HELLO.
//!
//! Changes are saved to `screens.ini`, which the panel service watches, so the service never
//! needs a channel from the UI. The preview redraws at the panel's own rate only while this
//! section is on screen.

use std::cell::RefCell;

use super::screens::{self, Frame};
use super::screens_settings::ScreensSettings;

/// The preview's scale: the 64x52 panel at 3x.
const SCALE: f32 = 3.0;

#[derive(Default)]
struct UiState {
    /// The settings as last loaded or saved; reloaded when the section opens.
    settings: Option<ScreensSettings>,
    texture: Option<(Frame, egui::TextureHandle)>,
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

pub fn group(ui: &mut egui::Ui) {
    let snap = screens::snapshot();
    let Some(hello) = snap.hello else {
        UI.with_borrow_mut(|s| s.settings = None);
        return;
    };
    super::settings_ui::section(ui, "Screens", |ui| {
        UI.with_borrow_mut(|state| {
            let settings = state.settings.get_or_insert_with(ScreensSettings::load);
            let mut changed = false;

            ui.label(screens::status());
            egui::Grid::new("screens-status")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.weak("Keyboard");
                    ui.label(format!("{} · host link v{}", snap.product, hello.version));
                    ui.end_row();
                    ui.weak("Right half");
                    ui.label(right_link(hello.right_link));
                    ui.end_row();
                    ui.weak("Right battery");
                    ui.label(
                        hello
                            .right_battery
                            .map_or("unknown".into(), |b| format!("{b}%")),
                    );
                    ui.end_row();
                });
            egui::CollapsingHeader::new("Details").show(ui, |ui| {
                ui.weak(format!(
                    "Relay discovery: {} ({} tries)",
                    hello.discovery_text(),
                    hello.discovery_tries
                ));
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
                    ui.weak(format!("Screen {}: {half} {kind}, {}×{}{on}", s.id, s.width, s.height));
                }
            });

            ui.add_space(4.0);
            changed |= ui
                .checkbox(&mut settings.drive_left, "Drive left screen")
                .changed();
            ui.add_enabled_ui(settings.drive_left, |ui| {
                let mut paused = screens::paused();
                if ui.checkbox(&mut paused, "Pause").changed() {
                    screens::set_paused(paused);
                }
                changed |= ui
                    .add(egui::Slider::new(&mut settings.fps, 1..=10).text("frames per second"))
                    .changed();
                ui.horizontal(|ui| {
                    use super::screens_settings::Layout;
                    ui.label("Show");
                    changed |= ui
                        .radio_value(&mut settings.layout, Layout::Info, "Clock, media and hints")
                        .changed();
                    changed |= ui
                        .radio_value(&mut settings.layout, Layout::Wallpaper, "The wallpaper's animation")
                        .on_hover_text(
                            "The left frames of the Wallpaper Engine wallpaper on screen (wallpapers.json in the art folder); the clock when it has none.",
                        )
                        .changed();
                });
            });

            if let Some(frame) = snap.frame.as_ref().filter(|_| settings.drive_left) {
                preview(ui, state_texture(ui.ctx(), &mut state.texture, frame));
                // Repainted when the panel gets a new frame, only while this section is drawn.
                super::screens::want_preview(crate::ui_wake::UiWake::from_ctx(ui.ctx()));
            }

            ui.add_space(4.0);
            ui.label("Game hints");
            ui.weak("Shown on the panel while that program is in front.");
            let mut remove = None;
            egui::Grid::new("screens-games")
                .num_columns(3)
                .show(ui, |ui| {
                    for (exe, hint) in settings.games.iter_mut() {
                        ui.label(exe.as_str());
                        changed |= ui.text_edit_singleline(hint).lost_focus();
                        if ui.small_button("Remove").clicked() {
                            remove = Some(exe.clone());
                        }
                        ui.end_row();
                    }
                });
            if let Some(exe) = remove {
                settings.games.remove(&exe);
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.new_exe)
                        .hint_text("game.exe")
                        .desired_width(120.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut state.new_hint)
                        .hint_text("hint")
                        .desired_width(160.0),
                );
                if let Some(exe) = snap.foreground.as_ref() {
                    if ui
                        .small_button("Add foreground app")
                        .on_hover_text(exe)
                        .clicked()
                    {
                        state.new_exe.clone_from(exe);
                    }
                }
                let exe = state.new_exe.trim().to_lowercase();
                if ui
                    .add_enabled(!exe.is_empty(), egui::Button::new("Add"))
                    .clicked()
                {
                    settings
                        .games
                        .insert(exe, state.new_hint.trim().to_string());
                    state.new_exe.clear();
                    state.new_hint.clear();
                    changed = true;
                }
            });

            if changed {
                if let Err(e) = settings.save() {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Can't save: {e}"));
                }
            }
        });
    });
}

/// The preview texture, rebuilt only when the frame changed.
fn state_texture<'a>(
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

fn preview(ui: &mut egui::Ui, texture: &egui::TextureHandle) {
    let size = texture.size_vec2() * SCALE;
    ui.add(egui::Image::new((texture.id(), size)));
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
}
