//! Collapsible settings sections, kept apart from KeyPeek's shared widgets.

pub fn section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    #[cfg(test)]
    if super::theme_preview::legacy() {
        // The look before #384, for the before/after preview.
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::CollapsingHeader::new(egui::RichText::new(title).weak())
                    .id_salt(("settings-section", title))
                    .default_open(open_by_default(title))
                    .show(ui, add_contents);
            });
        return;
    }
    // A card on the window's ground, its title in the HUD face (#384).
    super::theme::card(ui)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let title_text = super::theme::heading(ui, title);
            egui::CollapsingHeader::new(title_text)
                .id_salt(("settings-section", title))
                .default_open(open_by_default(title))
                .show(ui, add_contents);
        });
}

fn open_by_default(title: &str) -> bool {
    matches!(title, "Connection" | "Overlay Appearance" | "Keyboard")
}

/// The window's footer: project link and version on one row, then the KeyPeek credit and the
/// font credits on their own rows, so nothing overlaps at the window's fixed width (#384).
pub fn footer(ui: &mut egui::Ui) {
    let link = |ui: &mut egui::Ui, text: String, url: &str| {
        ui.add(egui::Hyperlink::from_label_and_url(egui::RichText::new(text).weak(), url));
    };
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        link(ui, "github.com/BanditF/foxglove".into(), "https://github.com/BanditF/foxglove");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            link(ui, format!("Version {}", env!("CARGO_PKG_VERSION")), "https://github.com/BanditF/foxglove/releases");
        });
    });
    link(ui, "based on KeyPeek by Stephan Rumswinkel".into(), "https://github.com/srwi/keypeek");
    super::theme::credits(ui);
}
