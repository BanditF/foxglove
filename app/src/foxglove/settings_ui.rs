//! Collapsible settings sections, kept apart from KeyPeek's shared widgets.

pub fn section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::CollapsingHeader::new(egui::RichText::new(title).weak())
                .id_salt(("settings-section", title))
                .default_open(matches!(
                    title,
                    "Connection" | "Overlay Appearance" | "Keyboard"
                ))
                .show(ui, add_contents);
        });
}
