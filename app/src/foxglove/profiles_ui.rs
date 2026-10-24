//! The profile picker at the top of Foxglove's settings (docs/profiles.md, phase 2). While a
//! profile is active, changes to screens, lighting and art are saved into that profile.

use super::store;

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
}
