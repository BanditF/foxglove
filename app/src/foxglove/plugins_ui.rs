//! The Plugins tab's settings section: installed plugins, the curated catalog, and installing a plugin from
//! a file after the user approves its permissions.

use std::cell::RefCell;

use super::plugin_install::{self, Installed, Staged};

#[derive(Default)]
struct UiState {
    /// Read from disk when the section opens and after every change, not every frame.
    installed: Option<Vec<Installed>>,
    dialog: egui_file_dialog::FileDialog,
    /// A Plugin waiting for the user's approval.
    staged: Option<Staged>,
    confirm_remove: Option<String>,
    message: Option<String>,
}

thread_local! { static UI: RefCell<UiState> = RefCell::new(UiState::default()); }

pub fn group(ui: &mut egui::Ui) {
    super::settings_ui::section(ui, "Plugins", |ui| {
        UI.with_borrow_mut(|state| {
            state.dialog.update(ui.ctx());
            if let Some(path) = state.dialog.take_picked() {
                match plugin_install::stage(&path) {
                    Ok(staged) => state.staged = Some(staged),
                    Err(e) => state.message = Some(format!("Can't install {}: {e}", path.display())),
                }
            }
            if let Some(staged) = state.staged.take() {
                state.staged = approval(ui, staged, state);
                return;
            }
            let installed = state.installed.get_or_insert_with(plugin_install::installed).clone();
            ui.weak("Plugins add features to Foxglove. Each runs as its own program with only the permissions you approve.");
            if super::plugin_trust::dev_unsigned() {
                ui.colored_label(ui.visuals().warn_fg_color, "Developer mode: unsigned plugins are allowed (FOXGLOVE_DEV_UNSIGNED_PLUGINS).");
            } else if let Err(e) = super::plugin_trust::catalog() {
                ui.weak(format!("Only signed plugins install and run; {e}."));
            }

            ui.add_space(4.0);
            ui.label("Installed");
            if installed.is_empty() {
                ui.weak("None yet.");
            }
            for plugin in &installed {
                ui.horizontal_wrapped(|ui| {
                    let mut enabled = plugin.record.enabled;
                    if ui.checkbox(&mut enabled, format!("{} {}", plugin.manifest.name, plugin.manifest.version)).changed() {
                        state.message = plugin_install::set_enabled(&plugin.manifest.id, enabled).err();
                        state.installed = None;
                        super::plugin_runner::sync();
                    }
                    let confirming = state.confirm_remove.as_deref() == Some(plugin.manifest.id.as_str());
                    if ui.small_button(if confirming { "Remove? Click again" } else { "Remove" }).clicked() {
                        if confirming {
                            super::plugin_runner::stop(&plugin.manifest.id);
                            state.message = plugin_install::remove(&plugin.manifest.id).err();
                            super::plugin_runner::sync();
                            state.confirm_remove = None;
                            state.installed = None;
                        } else {
                            state.confirm_remove = Some(plugin.manifest.id.clone());
                        }
                    }
                });
                if plugin.record.enabled {
                    use foxglove_plugin_host::runner::PluginState;
                    let text = match super::plugin_runner::state(&plugin.manifest.id) {
                        Some(PluginState::Running) => "running".to_string(),
                        Some(PluginState::Restarting { reason, wait }) => format!("restarting in {} s: {reason}", wait.as_secs()),
                        Some(PluginState::Disabled(reason)) => format!("stopped after repeated failures ({reason}); turn it off and on to retry"),
                        Some(PluginState::Stopped) | None => "starting".to_string(),
                    };
                    ui.weak(format!("    {text}"));
                }
                let perms: Vec<String> = plugin.record.approved.iter().map(|p| p.describe()).collect();
                ui.weak(format!("    may {}", if perms.is_empty() { "do nothing beyond running".into() } else { perms.join(", ") }));
            }

            ui.add_space(6.0);
            ui.label("Catalog");
            for entry in plugin_install::catalog() {
                if installed.iter().any(|k| k.manifest.id == entry.id) {
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&entry.name);
                    ui.weak(&entry.description);
                    // Downloads come with the first released plugin; until then entries are listed only.
                    ui.add_enabled(false, egui::Button::new("Coming soon"));
                });
            }
            ui.add_space(4.0);
            if ui.button("Install from file…").clicked() {
                state.dialog.pick_file();
            }
            if let Some(m) = &state.message {
                ui.colored_label(ui.visuals().error_fg_color, m);
            }
        });
    });
}

/// Shows what a staged plugin may do; returns it again while the user hasn't decided.
fn approval(ui: &mut egui::Ui, staged: Staged, state: &mut UiState) -> Option<Staged> {
    let m = &staged.manifest;
    ui.strong(format!("Install {} {}?", m.name, m.version));
    if !m.description.is_empty() {
        ui.label(&m.description);
    }
    ui.weak(format!("{} · built for API {}", m.id, m.api));
    if staged.new_permissions.is_empty() {
        ui.label("It asks for no new permissions.");
    } else {
        ui.label("It will be able to:");
        for p in &staged.new_permissions {
            if p.sensitive() {
                ui.colored_label(ui.visuals().warn_fg_color, format!("• {} (sensitive)", p.describe()));
            } else {
                ui.label(format!("• {}", p.describe()));
            }
        }
    }
    let mut decided = None;
    ui.horizontal(|ui| {
        if ui.button("Install").clicked() {
            decided = Some(true);
        }
        if ui.button("Cancel").clicked() {
            decided = Some(false);
        }
    });
    match decided {
        None => Some(staged),
        Some(install) => {
            state.installed = None;
            if install {
                let name = staged.manifest.name.clone();
                super::plugin_runner::stop(&staged.manifest.id);
                state.message = plugin_install::commit(staged, true).err().map(|e| format!("Can't install {name}: {e}"));
                super::plugin_runner::sync();
            } else {
                plugin_install::discard(staged);
            }
            None
        }
    }
}
