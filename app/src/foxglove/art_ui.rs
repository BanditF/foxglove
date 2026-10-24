//! Right screen controls and the file picker shared with the tray.
use super::{art_library, art_service};
use std::sync::atomic::Ordering;

#[derive(Default)]
struct UiState {
    dialog: egui_file_dialog::FileDialog,
    textures: Vec<(std::path::PathBuf, egui::TextureHandle)>,
    save_slot: u8,
    save_name: String,
    /// The slot whose Erase was clicked once and waits for the second click.
    confirm_erase: Option<u8>,
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
                art_service::Choice::Slot(n) => s
                    .slots
                    .iter()
                    .find(|i| i.slot == *n)
                    .map_or_else(|| format!("saved slot {n}"), |i| format!("saved \"{}\"", i.name)),
            };
            ui.weak(format!("Pinned: {name} (re-applied after power cycles)"));
        }
        ui.add_enabled_ui(!s.busy, |ui| {
            let built_ins = s.built_ins();
            let showing = s.showing_built_in();
            let current = built_ins
                .iter()
                .find(|(n, _)| Some(*n) == showing)
                .map_or_else(|| "Choose built-in".to_string(), |(_, name)| name.clone());
            egui::ComboBox::from_id_salt("right-art-built-in")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(false, "Random").clicked() {
                        h.select(-1);
                    }
                    for (n, name) in built_ins {
                        // Selected = what the right half shows now.
                        if ui.selectable_label(Some(n) == showing, name).clicked() {
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
                    if s.showing_file() == Some(&entry.path) {
                        ui.strong("on the keyboard now");
                    }
                    if ui.add_enabled(valid, egui::Button::new("Push")).clicked() {
                        h.push(entry.path.clone());
                    }
                    if ui.button("Delete").clicked() {
                        art_library::delete(entry.path);
                    }
                });
            }
            slots_section(ui, &h, &s);
            cycle_section(ui, &h, &s);
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

/// The right half's flash slots (firmware #123): sets saved there survive the keyboard's reboots.
fn slots_section(ui: &mut egui::Ui, h: &art_service::Handle, s: &art_service::Snapshot) {
    use digitalis::protocol::right_state;
    let Some(st) = s.status.filter(|st| st.flash_slots > 0) else {
        return;
    };
    ui.add_space(6.0);
    ui.label("Saved on the keyboard");
    UI.with_borrow_mut(|state| {
        for info in &s.slots {
            ui.horizontal(|ui| {
                ui.label(format!("{}.", info.slot + 1));
                if !info.used {
                    ui.weak("empty");
                    return;
                }
                ui.label(&info.name);
                ui.weak(format!("{} bytes · written {}×", info.len, info.writes));
                let playing = i16::from(st.right_set) == i16::from(st.right_set_count) + i16::from(info.slot);
                if playing {
                    ui.strong("on the keyboard now");
                } else if ui.button("Play").clicked() {
                    h.play_slot(info.slot);
                }
                let confirming = state.confirm_erase == Some(info.slot);
                if ui.button(if confirming { "Erase? Click again" } else { "Erase" }).clicked() {
                    if confirming {
                        state.confirm_erase = None;
                        h.erase_slot(info.slot);
                    } else {
                        state.confirm_erase = Some(info.slot);
                    }
                }
            });
        }
        // Saving copies the pushed set now playing on the right into flash.
        let can_save = st.right_state == right_state::PLAYING;
        ui.add_enabled_ui(can_save, |ui| {
            ui.horizontal(|ui| {
                ui.label("Save what's playing to");
                egui::ComboBox::from_id_salt("art-save-slot")
                    .selected_text(format!("slot {}", state.save_slot + 1))
                    .show_ui(ui, |ui| {
                        for n in 0..st.flash_slots {
                            let used = st.flash_used & (1 << n) != 0;
                            let text = format!("slot {}{}", n + 1, if used { " (replace)" } else { "" });
                            ui.selectable_value(&mut state.save_slot, n, text);
                        }
                    });
                if state.save_name.is_empty() {
                    if let Some(art_service::Choice::File(p)) = &s.pinned {
                        state.save_name = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    }
                }
                ui.add(
                    egui::TextEdit::singleline(&mut state.save_name)
                        .char_limit(digitalis::protocol::art::NAME_LEN)
                        .hint_text("name")
                        .desired_width(120.0),
                );
                if ui.button("Save").clicked() {
                    h.save_to_slot(state.save_slot, state.save_name.trim().to_string());
                }
            });
        });
        if !can_save {
            ui.weak("Push a file first; saving copies the set now playing on the right.");
        }
    });
}

/// The hotlist and how it cycles (#60).
fn cycle_section(ui: &mut egui::Ui, h: &art_service::Handle, s: &art_service::Snapshot) {
    use super::art_cycle::{Item, Mode};
    let Some(st) = s.status else {
        return;
    };
    ui.add_space(6.0);
    ui.label("Hotlist");
    let mut cycle = s.cycle.clone();
    let mut changed = false;

    ui.horizontal(|ui| {
        changed |= ui.radio_value(&mut cycle.mode, Mode::Off, "Don't cycle").changed();
        let minutes = match cycle.mode {
            Mode::Minutes(m) => m,
            _ => 5,
        };
        changed |= ui
            .radio_value(&mut cycle.mode, Mode::Minutes(minutes), "Every")
            .changed();
        let mut m = minutes;
        if ui
            .add(egui::DragValue::new(&mut m).range(1..=1440).suffix(" min"))
            .changed()
        {
            cycle.mode = Mode::Minutes(m);
            changed = true;
        }
        ui.add_enabled(false, egui::RadioButton::new(cycle.mode == Mode::LightsWake, "When the lights wake"))
            .on_disabled_hover_text("Needs keyboard firmware that reports when the lights come back on.");
    });

    let names = super::art_manifest::sets(st.right_set_count);
    let label = |item: &Item| match item {
        Item::BuiltIn(name) => names
            .and_then(|sets| sets.iter().find(|set| &set.name == name))
            .map_or_else(|| name.clone(), |set| set.label.clone()),
        Item::Slot(n) => s
            .slots
            .iter()
            .find(|i| i.slot == *n && i.used)
            .map_or_else(|| format!("slot {} (empty)", n + 1), |i| format!("saved \"{}\"", i.name)),
        Item::File(p) => p.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
    };
    let mut remove = None;
    for (i, item) in cycle.items.iter().enumerate() {
        ui.horizontal(|ui| {
            if i == cycle.position {
                ui.strong("▶");
            }
            ui.label(label(item));
            if ui.small_button("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        cycle.items.remove(i);
        cycle.position = cycle.position.min(cycle.items.len().saturating_sub(1));
        changed = true;
    }

    ui.horizontal(|ui| {
        let mut add = None;
        egui::ComboBox::from_id_salt("art-hotlist-add")
            .selected_text("Add to hotlist")
            .show_ui(ui, |ui| {
                for set in names.unwrap_or_default() {
                    if ui.selectable_label(false, &set.label).clicked() {
                        add = Some(Item::BuiltIn(set.name.clone()));
                    }
                }
                for info in s.slots.iter().filter(|i| i.used) {
                    if ui.selectable_label(false, format!("saved \"{}\"", info.name)).clicked() {
                        add = Some(Item::Slot(info.slot));
                    }
                }
                for entry in super::art_library::snapshot().entries.into_iter().filter(|e| e.preview.is_ok()) {
                    let name = entry.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    if ui.selectable_label(false, name).clicked() {
                        add = Some(Item::File(entry.path));
                    }
                }
            });
        if let Some(item) = add.filter(|i| !cycle.items.contains(i)) {
            cycle.items.push(item);
            changed = true;
        }
        if ui
            .add_enabled(!cycle.items.is_empty() && !s.busy, egui::Button::new("Next"))
            .clicked()
        {
            h.next();
        }
    });
    if names.is_none() {
        ui.weak("Built-ins can be added once the art manifest matches this keyboard's firmware.");
    }
    if changed {
        if let Err(e) = h.set_cycle(cycle) {
            ui.colored_label(egui::Color32::RED, e);
        }
    }
}
