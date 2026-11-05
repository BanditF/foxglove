//! Settings for other keyboards over VIA: turn it on, load VIA JSON definitions or find any VIA
//! or Vial board, see what was found. Read-only: Foxglove never writes to these keyboards.

use std::cell::RefCell;

use super::via_host::{self, Found, ViaSettings};

#[derive(Default)]
struct UiState {
    settings: Option<ViaSettings>,
    dialog: egui_file_dialog::FileDialog,
    message: Option<String>,
}

thread_local! { static UI: RefCell<UiState> = RefCell::new(UiState::default()); }

pub fn group(ui: &mut egui::Ui) {
    draw(ui, &via_host::found());
}

/// The group with these settings and boards, for the theme preview: nothing is read from or
/// saved to the store unless a control is used.
#[cfg(test)]
pub(crate) fn preview(ui: &mut egui::Ui, s: ViaSettings, found: &[Found]) {
    UI.with_borrow_mut(|state| state.settings = Some(s));
    draw(ui, found);
}

fn draw(ui: &mut egui::Ui, found: &[Found]) {
    super::settings_ui::section(ui, "Other keyboards (VIA)", |ui| {
        UI.with_borrow_mut(|state| {
            state.dialog.update(ui.ctx());
            let mut s = state.settings.get_or_insert_with(via_host::settings).clone();
            let mut changed = false;
            if let Some(path) = state.dialog.take_picked() {
                if !s.definitions.contains(&path) {
                    s.definitions.push(path);
                    changed = true;
                }
            }
            ui.weak("QMK keyboards with VIA or Vial, over USB. Foxglove only reads them: the keymap, layers and lights.");
            changed |= ui.checkbox(&mut s.enabled, "Use VIA keyboards").changed();
            ui.add_enabled_ui(s.enabled, |ui| {
                changed |= ui
                    .checkbox(&mut s.generic, "Find any VIA or Vial keyboard")
                    .on_hover_text("Also finds keyboards without a loaded definition. A Vial keyboard gives its own; others get lighting but no keymap. Each is sent a VIA version read when found.")
                    .changed();
                let mut remove = None;
                for (i, d) in s.definitions.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(d.file_name().unwrap_or_default().to_string_lossy());
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    s.definitions.remove(i);
                    changed = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("Add VIA definition (JSON)…").clicked() {
                        state.dialog.pick_file();
                    }
                    if ui.button("Refresh").clicked() {
                        via_host::rescan();
                    }
                });
                for f in found {
                    ui.horizontal_wrapped(|ui| {
                        match (&f.opened, f.paused) {
                            (_, true) => ui.weak(format!("{}: paused, so VIA or its maker's app can use it", f.name)),
                            (Ok(b), _) => match b.layers {
                                Some(layers) => ui.label(format!("{}: connected, {layers} layers (read-only)", f.name)),
                                None => ui.label(format!("{}: connected, no keymap (read-only)", f.name)),
                            },
                            (Err(e), _) => ui.weak(format!("{}: {e}", f.name)),
                        };
                        if let Some(key) = &f.key {
                            let (text, pause) = if f.paused { ("Resume", false) } else { ("Let other apps use it", true) };
                            if ui.small_button(text).on_hover_text("Closes Foxglove's connection to it, so only one app talks to it at a time.").clicked() {
                                state.message = via_host::set_paused(key, pause).err();
                                state.settings = None;
                            }
                        }
                    });
                    if let (Ok(b), false) = (&f.opened, f.paused) {
                        ui.weak(format!("    {}", via_host::summary_line(b)));
                        if b.layers.is_some() {
                            ui.weak("    Active layer: unknown (stock firmware doesn't report it).");
                        }
                    }
                    // A keyboard of a known model on a new port, while the known one is away.
                    if let Some((id, known)) = &f.maybe {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("    Is this the {} you used before, on another port?", known.name));
                            let model = known.model.clone();
                            if ui.small_button("This is the same keyboard").clicked() {
                                state.message = via_host::same_keyboard(id, &f.name, &model, Some(known)).err();
                                state.settings = None;
                            }
                            if ui.small_button("A different keyboard").clicked() {
                                state.message = via_host::same_keyboard(id, &f.name, &model, None).err();
                                state.settings = None;
                            }
                        });
                    }
                }
                if s.enabled && !s.generic && s.definitions.is_empty() {
                    ui.weak("Add the keyboard's VIA definition (from its maker), or find any VIA or Vial keyboard.");
                }
            });
            if changed {
                state.message = via_host::save(&s).err();
                state.settings = Some(s);
            }
            if let Some(m) = &state.message {
                ui.colored_label(ui.visuals().error_fg_color, m);
            }
        });
    });
}
