// SPDX-License-Identifier: MIT
//! The sections the hub adds after a module's page tree (§4.7): **Keys**, one row per
//! `[[actions]]` entry whose `when` is true, with its bound key or Set key (v1.4); and
//! **Settings**, the form generated from `[[settings]]`. A module never draws either.

use crate::model::{PanelHost, PanelKey};
use crate::render::key_chip;
use egui::{RichText, Ui};
use hideout_contract::module::ModuleSettingKind;
use serde_json::Value;

/// The node id the Settings section answers to (`opens = "settings"` scrolls here).
pub const SETTINGS_ID: &str = "hub.settings";

/// Whether the page `key` gets a generated section at all.
pub fn has_sections(host: &PanelHost, module: &str) -> bool {
    host.manifest(module).is_some_and(|m| {
        m.actions.iter().any(|a| host.when(a.when.as_deref()))
            || m.settings.iter().any(|s| host.when(s.when.as_deref()))
            || !m.secrets.is_empty()
    })
}

/// Draws Keys and Settings for the page `key`'s module. Changes go out as `settings.set`
/// ([`PanelHost::set_setting`]); the first draw asks for the values (`settings.get`).
pub fn page_sections_ui(host: &mut PanelHost, ui: &mut Ui, key: &PanelKey) {
    let Some(m) = host.manifest(&key.module).cloned() else {
        return;
    };
    let module = key.module.as_str();
    let live = host.panel(key).is_none_or(|p| p.live());
    let actions: Vec<_> = m
        .actions
        .iter()
        .filter(|a| host.when(a.when.as_deref()))
        .collect();
    if !actions.is_empty() {
        egui::CollapsingHeader::new("Keys")
            .id_salt(("hideout.keys", module))
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new(("hideout.keys.grid", module))
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for a in &actions {
                            ui.label(&a.decl.name);
                            match host.key(module, &a.decl.id) {
                                Some(k) => key_chip(ui, k),
                                // Choosing a key (UX3) needs the key capture, not in this hub yet.
                                None => {
                                    ui.label(RichText::new("No key").weak())
                                        .on_hover_text("Choosing a key comes in a later Hideout.");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
    }
    let settings: Vec<_> = m
        .settings
        .iter()
        .filter(|s| host.when(s.when.as_deref()))
        .cloned()
        .collect();
    if settings.is_empty() && m.secrets.is_empty() {
        return;
    }
    let values = if settings.is_empty() {
        Default::default()
    } else {
        host.settings_values(module)
    };
    let module_name = host.module_name(module);
    let focus = host
        .view(key)
        .and_then(|v| v.focus.clone())
        .is_some_and(|f| f == SETTINGS_ID);
    let r = egui::CollapsingHeader::new("Settings")
        .id_salt(("hideout.settings", module))
        .default_open(true)
        .open(focus.then_some(true))
        .show(ui, |ui| {
            if !live {
                ui.label(
                    RichText::new(format!(
                        "{module_name} isn't running; settings come back when it is."
                    ))
                    .weak(),
                );
            }
            ui.add_enabled_ui(live, |ui| {
                for s in &settings {
                    let current = values.get(&s.name).cloned().unwrap_or(Value::Null);
                    if let Some(v) = setting_row(ui, host, module, s, &current) {
                        host.set_setting(module, &s.name, v);
                    }
                }
            });
            // Secrets are kept by the hub, so they can be set while the module is down.
            for s in &m.secrets {
                if let Some(v) = secret_row(ui, host, module, &s.name, &s.label) {
                    host.set_secret(module, &s.name, v);
                }
            }
            if let Some(n) = host.settings_notice.get(module) {
                ui.label(RichText::new(n).color(ui.visuals().error_fg_color));
            }
        });
    if focus {
        r.header_response.scroll_to_me(Some(egui::Align::TOP));
        host.view_mut(key).focus = None;
    }
}

/// One row of the generated form; the new value when the user changed it.
fn setting_row(
    ui: &mut Ui,
    host: &mut PanelHost,
    module: &str,
    s: &hideout_contract::module::ModuleSetting,
    current: &Value,
) -> Option<Value> {
    let id = egui::Id::new(("hideout.setting", module, &s.name));
    match s.kind {
        ModuleSettingKind::Bool => {
            let mut on = current.as_bool().unwrap_or(false);
            ui.checkbox(&mut on, &s.label)
                .changed()
                .then_some(Value::Bool(on))
        }
        ModuleSettingKind::Int => {
            let mut n = current.as_i64().unwrap_or(0);
            let changed = ui
                .horizontal(|ui| {
                    ui.label(&s.label);
                    let mut drag = egui::DragValue::new(&mut n);
                    if let (Some(lo), Some(hi)) = (s.min, s.max) {
                        drag = drag.range(lo..=hi);
                    }
                    ui.add(drag).changed()
                })
                .inner;
            changed.then(|| Value::from(n))
        }
        ModuleSettingKind::Text | ModuleSettingKind::File => {
            // Typed text is kept in egui's memory until Enter or leaving the field sends it.
            let shown = current.as_str().unwrap_or_default().to_string();
            ui.label(&s.label);
            let mut text = ui
                .data_mut(|d| d.get_temp::<String>(id))
                .unwrap_or_else(|| shown.clone());
            let hint = if s.kind == ModuleSettingKind::File {
                "Full path of the file"
            } else {
                ""
            };
            let (r, picked) = ui
                .horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .id(id)
                            .hint_text(hint)
                            .desired_width(320.0),
                    );
                    // Choose file (v1.4): the system picker with the manifest's filter. The
                    // field stays for pasting a path.
                    let mut picked = None;
                    if s.kind == ModuleSettingKind::File
                        && host.picker.is_some()
                        && ui.button("Choose file").clicked()
                    {
                        picked = host.pick_file(s.filter.as_deref());
                    }
                    (r, picked)
                })
                .inner;
            if let Some(path) = picked {
                ui.data_mut(|d| d.remove::<String>(id));
                return (path != shown).then_some(Value::String(path));
            }
            if r.changed() {
                ui.data_mut(|d| d.insert_temp(id, text.clone()));
            }
            if r.lost_focus() {
                ui.data_mut(|d| d.remove::<String>(id));
                if text != shown {
                    return Some(Value::String(text));
                }
            }
            None
        }
    }
}

/// One `[[secrets]]` row: a masked field, never filled with the stored value. Once saved it
/// reads "Saved" with **Change**. The typed text is sent on Enter or **Save**.
fn secret_row(
    ui: &mut Ui,
    host: &mut PanelHost,
    module: &str,
    name: &str,
    label: &str,
) -> Option<String> {
    let id = egui::Id::new(("hideout.secret", module, name));
    let changing_id = id.with("changing");
    let saved = host.secret_saved(module, name);
    let changing = ui
        .data(|d| d.get_temp::<bool>(changing_id))
        .unwrap_or(false);
    ui.label(label);
    if saved && !changing {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Saved").weak());
            if ui.button("Change").clicked() {
                ui.data_mut(|d| d.insert_temp(changing_id, true));
            }
        });
        return None;
    }
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_default();
    let (r, save) = ui
        .horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .id(id)
                    .password(true)
                    .desired_width(240.0),
            );
            let save = ui
                .add_enabled(!text.is_empty(), egui::Button::new("Save"))
                .clicked();
            (r, save)
        })
        .inner;
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    }
    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if (save || enter) && !text.is_empty() {
        ui.data_mut(|d| {
            d.remove::<String>(id);
            d.remove::<bool>(changing_id);
        });
        return Some(text);
    }
    None
}
