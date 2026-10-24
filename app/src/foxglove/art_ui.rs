//! Right screen controls and the file picker shared with the tray.
use super::{art_library, art_service};
use std::sync::atomic::Ordering;

#[derive(Default)]
struct UiState {
    dialog: egui_file_dialog::FileDialog,
    textures: Vec<(std::path::PathBuf, egui::TextureHandle)>,
}
thread_local! { static UI: std::cell::RefCell<UiState> = std::cell::RefCell::new(UiState::default()); }

pub fn update(ctx: &egui::Context) {
    art_service::set_wake(crate::ui_wake::UiWake::from_ctx(ctx));
    UI.with_borrow_mut(|state| {
        if art_library::PICK_FILE.swap(false, Ordering::Relaxed) {
            state.dialog.pick_file();
        }
        state.dialog.update(ctx);
        if let Some(path) = state.dialog.take_picked() {
            if let Some(h) = art_service::handle() {
                h.push(path);
            }
        }
    });
}
pub fn group(ui: &mut egui::Ui) {
    let Some(h) = art_service::handle() else {
        return;
    };
    let s = h.snapshot();
    if !s.connected {
        return;
    }
    super::settings_ui::section(ui, "Right screen art", |ui| {
        ui.label(
            s.status
                .map_or_else(|| "Waiting for art status".into(), |s| s.to_string()),
        );
        if let Some(pin) = &s.pinned {
            let name = match pin {
                art_service::Choice::BuiltIn(-1) => "Random".into(),
                art_service::Choice::BuiltIn(n) => format!("Set {n}"),
                art_service::Choice::File(p) => p.display().to_string(),
            };
            ui.weak(format!("Pinned: {name} (re-applied after power cycles)"));
        }
        ui.add_enabled_ui(!s.busy, |ui| {
            egui::ComboBox::from_id_salt("right-art-built-in")
                .selected_text("Choose built-in")
                .show_ui(ui, |ui| {
                    if ui.selectable_label(false, "Random").clicked() {
                        h.select(-1);
                    }
                    for (n, name) in s.built_ins() {
                        if ui.selectable_label(false, name).clicked() {
                            h.select(n);
                        }
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("Push file...").clicked() {
                    art_library::PICK_FILE.store(true, Ordering::Relaxed);
                }
                if ui.button("Refresh").clicked() {
                    h.refresh();
                    art_library::refresh();
                    UI.with_borrow_mut(|s| s.textures.clear());
                }
            });
            if let Some(folder) = art_library::folder() {
                ui.weak(format!("Library: {}", folder.display()));
            }
            let library = art_library::snapshot();
            if let Some(error) = library.error {
                ui.colored_label(egui::Color32::RED, error);
            }
            for entry in library.entries {
                ui.horizontal(|ui| {
                    let valid = entry.preview.is_ok();
                    match entry.preview {
                        Ok(image) => UI.with_borrow_mut(|state| {
                            let index = state
                                .textures
                                .iter()
                                .position(|(p, _)| p == &entry.path)
                                .unwrap_or_else(|| {
                                    let texture = ui.ctx().load_texture(
                                        entry.path.display().to_string(),
                                        (*image).clone(),
                                        egui::TextureOptions::NEAREST,
                                    );
                                    state.textures.push((entry.path.clone(), texture));
                                    state.textures.len() - 1
                                });
                            ui.image((state.textures[index].1.id(), egui::vec2(140.0, 68.0)));
                        }),
                        Err(error) => {
                            ui.weak(error);
                        }
                    }
                    ui.label(entry.path.file_stem().unwrap_or_default().to_string_lossy());
                    if ui.add_enabled(valid, egui::Button::new("Push")).clicked() {
                        h.push(entry.path.clone());
                    }
                    if ui.button("Delete").clicked() {
                        art_library::delete(entry.path);
                    }
                });
            }
        });
        if s.busy {
            if let Some((done, total)) = s.progress() {
                ui.add(
                    egui::ProgressBar::new(if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    })
                    .text(format!("{done} / {total} bytes")),
                );
            } else {
                ui.spinner();
            }
            if ui.button("Cancel").clicked() {
                h.cancel();
            }
        }
        if s.cancelled {
            ui.weak("Cancelled");
        }
        if let Some(error) = s.error {
            ui.colored_label(egui::Color32::RED, error);
        }
    });
}
