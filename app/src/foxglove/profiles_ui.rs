//! The profile picker at the top of Foxglove's settings, and the rules that switch profiles by
//! the program in front (docs/profiles.md, phases 2-3). While a profile is active, changes to
//! screens, lighting and art are saved into that profile.

use std::cell::RefCell;

use super::store::{self, Rule};

#[derive(Default)]
struct UiState {
    /// Read when the settings open and after each change, not every frame.
    rules: Option<(bool, Vec<Rule>)>,
    new_program: String,
    new_profile: String,
}

thread_local! { static UI: RefCell<UiState> = RefCell::new(UiState::default()); }

pub fn picker(ui: &mut egui::Ui) {
    let (names, active) = store::cached_profiles();
    if names.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label("Profile");
        let shown = active.clone().unwrap_or_else(|| "Base settings".into());
        let mut chosen = None;
        egui::ComboBox::from_id_salt("foxglove-profile")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui.selectable_label(active.is_none(), "Base settings").clicked() {
                    chosen = Some(None);
                }
                for name in &names {
                    if ui.selectable_label(active.as_deref() == Some(name.as_str()), name).clicked() {
                        chosen = Some(Some(name.clone()));
                    }
                }
            });
        if let Some(name) = chosen.filter(|n| *n != active) {
            if let Err(e) = store::set_active(name.as_deref()) {
                ui.colored_label(ui.visuals().error_fg_color, format!("Can't switch: {e}"));
            }
        }
        if active.is_some() {
            ui.weak("Changes to screens, lights and art are saved in this profile.");
        }
    });
    egui::CollapsingHeader::new("Switch profile by program").show(ui, |ui| rules(ui, &names));
}

fn rules(ui: &mut egui::Ui, names: &[String]) {
    UI.with_borrow_mut(|state| {
        let (mut auto, mut rules) = state.rules.get_or_insert_with(store::rules).clone();
        let mut changed = ui.checkbox(&mut auto, "Switch automatically when a listed program comes to the front").changed();
        let mut remove = None;
        egui::Grid::new("profile-rules").num_columns(3).show(ui, |ui| {
            for (i, rule) in rules.iter_mut().enumerate() {
                ui.label(&rule.program);
                egui::ComboBox::from_id_salt(("profile-rule", i))
                    .selected_text(rule.profile.clone())
                    .show_ui(ui, |ui| {
                        for name in names {
                            changed |= ui.selectable_value(&mut rule.profile, name.clone(), name).changed();
                        }
                    });
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            rules.remove(i);
            changed = true;
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut state.new_program).hint_text("game.exe").desired_width(140.0));
            if let Some(palm::facts::Value::Text(front)) = super::palm_host::bus().facts().get("app.foreground") {
                if ui.small_button("Program in front").on_hover_text(&front).clicked() {
                    state.new_program = front;
                }
            }
            if state.new_profile.is_empty() {
                state.new_profile = names.last().cloned().unwrap_or_default();
            }
            egui::ComboBox::from_id_salt("profile-rule-new")
                .selected_text(state.new_profile.clone())
                .show_ui(ui, |ui| {
                    for name in names {
                        ui.selectable_value(&mut state.new_profile, name.clone(), name);
                    }
                });
            let program = state.new_program.trim().to_lowercase();
            if ui.add_enabled(!program.is_empty(), egui::Button::new("Add")).clicked() {
                rules.retain(|r| r.program != program);
                rules.push(Rule { program, profile: state.new_profile.clone() });
                state.new_program.clear();
                changed = true;
            }
        });
        if changed {
            match store::set_rules(auto, &rules) {
                Ok(()) => state.rules = Some((auto, rules)),
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Can't save: {e}"));
                }
            }
        }
    });
}
