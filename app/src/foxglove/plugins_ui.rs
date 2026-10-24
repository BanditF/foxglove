//! The Plugins tab's settings section: installed plugins, the curated catalog, and installing a plugin from
//! a file after the user approves its permissions.

use std::cell::RefCell;

use super::plugin_icons as icons;
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
    /// Why no plugin is trusted, if so: checked when the catalog may have changed, not every
    /// frame (it reads and verifies the signed files).
    trust: Option<(super::catalog_refresh::Status, Result<(), String>)>,
    /// What the user is typing into a secret's field, by `plugin/name`; never shown back.
    secret_input: std::collections::HashMap<String, String>,
    /// Whether each declared secret is set, read once per plugin list, not every frame.
    secret_set: std::collections::HashMap<String, bool>,
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
            } else {
                let status = super::catalog_refresh::status();
                if state.trust.as_ref().is_none_or(|(s, _)| *s != status) {
                    state.trust = Some((status, super::plugin_trust::catalog().map(|_| ())));
                }
                if let Some((_, Err(e))) = &state.trust {
                    ui.weak(format!("Only signed plugins install and run; {e}."));
                }
            }
            catalog_status(ui);

            ui.add_space(4.0);
            ui.label("Installed");
            if installed.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add(icons::empty(ui, 120.0));
                    ui.weak("No plugins yet. Pick one from the catalog below, or install one from a file.");
                });
            }
            let plugins_dir = plugin_install::plugins_dir();
            for plugin in &installed {
                let dir = plugins_dir.as_ref().map(|d| d.join(&plugin.manifest.id));
                ui.horizontal_wrapped(|ui| {
                    ui.add(icons::glyph(ui, &plugin.manifest.id, dir.as_deref(), 20.0));
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
                action_keys(ui, plugin, state);
                secret_fields(ui, plugin, state);
            }

            ui.add_space(6.0);
            ui.label("Catalog");
            for entry in plugin_install::catalog() {
                if installed.iter().any(|k| k.manifest.id == entry.id) {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.add(icons::tile(ui, &entry.id, None, 36.0));
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&entry.name);
                            // Downloads come with the first released plugin; until then entries are listed only.
                            ui.add_enabled(false, egui::Button::new("Coming soon"));
                        });
                        ui.weak(&entry.description);
                    });
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

/// A field for each secret the plugin declares (#213): the value goes straight into Windows'
/// credential store and is never shown again.
fn secret_fields(ui: &mut egui::Ui, plugin: &Installed, state: &mut UiState) {
    use foxglove_plugin_host::secrets;
    let id = &plugin.manifest.id;
    for decl in &plugin.manifest.secrets {
        let key = format!("{id}/{}", decl.name);
        let set = *state
            .secret_set
            .entry(key.clone())
            .or_insert_with(|| secrets::get(id, &decl.name).ok().flatten().is_some());
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(&decl.label);
            let input = state.secret_input.entry(key.clone()).or_default();
            ui.add(egui::TextEdit::singleline(input).password(true).desired_width(160.0).hint_text(if set { "saved" } else { "not set" }));
            if ui.add_enabled(!input.is_empty(), egui::Button::new("Save")).clicked() {
                state.message = secrets::set(id, &decl.name, Some(input.as_str())).err();
                input.clear();
                state.secret_set.remove(&key);
            }
            if set && ui.small_button("Clear").clicked() {
                state.message = secrets::set(id, &decl.name, None).err();
                state.secret_set.remove(&key);
            }
        });
    }
}

/// A key (F13–F24) for each of the plugin's actions (#212).
fn action_keys(ui: &mut egui::Ui, plugin: &Installed, state: &mut UiState) {
    use super::plugin_actions::{self, KEYS};
    if plugin.manifest.actions.is_empty() {
        return;
    }
    let bindings = plugin_actions::bindings();
    let taken = plugin_actions::taken();
    for action in &plugin.manifest.actions {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(&action.name);
            let current = plugin_actions::key_for(&bindings, &plugin.manifest.id, &action.id);
            let mut chosen = current.clone();
            egui::ComboBox::from_id_salt(("plugin-action", &plugin.manifest.id, &action.id))
                .selected_text(current.as_deref().unwrap_or("no key"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "no key");
                    for key in KEYS {
                        let used_by = bindings.get(key).filter(|b| b.plugin != plugin.manifest.id || b.action != action.id);
                        let label = if used_by.is_some() { format!("{key} (in use)") } else { key.to_string() };
                        ui.selectable_value(&mut chosen, Some(key.to_string()), label);
                    }
                });
            if chosen != current {
                state.message = plugin_actions::bind(&plugin.manifest.id, &action.id, chosen.as_deref()).err();
            }
            if current.as_ref().is_some_and(|k| taken.contains(k)) {
                ui.colored_label(ui.visuals().warn_fg_color, "another program holds this key");
            }
        });
    }
    ui.weak("    Put F13–F24 on a key in ZMK Studio or VIA; it types nothing, and Foxglove passes it to the plugin.");
}

/// The catalog's freshness, and "Check now". Opening the tab asks for a check when one is due
/// (at most every 10 minutes, and never while backing off).
fn catalog_status(ui: &mut egui::Ui) {
    use super::catalog_refresh::{self, Status, Trigger};
    catalog_refresh::request(Trigger::TabOpened);
    ui.horizontal_wrapped(|ui| {
        let status = catalog_refresh::status();
        match &status {
            Status::Idle => ui.weak("Catalog: not checked yet this session."),
            Status::Checking => {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                ui.weak("Catalog: checking…")
            }
            Status::UpToDate { sequence: Some(n) } => ui.weak(format!("Catalog: updated (sequence {n}).")),
            Status::UpToDate { sequence: None } => ui.weak("Catalog: up to date."),
            Status::Failed(e) => ui.weak(format!("Catalog: couldn't check ({e}).")),
        };
        if ui.add_enabled(status != Status::Checking, egui::Button::new("Check now").small()).clicked() {
            catalog_refresh::request(Trigger::Manual);
        }
    });
}

/// Shows what a staged plugin may do; returns it again while the user hasn't decided.
fn approval(ui: &mut egui::Ui, staged: Staged, state: &mut UiState) -> Option<Staged> {
    let m = &staged.manifest;
    ui.horizontal(|ui| {
        ui.add(icons::tile(ui, &m.id, Some(&staged.dir), 48.0));
        ui.strong(format!("Install {} {}?", m.name, m.version));
    });
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
