//! Collapsible settings sections, kept apart from KeyPeek's shared widgets.

pub fn section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    // Only the current page's sections show (#385).
    if !super::shell::shows(title) {
        return;
    }
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
    match style(title) {
        // The page draws its own cards (#386 on).
        Style::Bare => {
            ui.add_space(4.0);
            add_contents(ui);
            return;
        }
        Style::Card => {
            card(ui, title, add_contents);
            return;
        }
        Style::Collapsing => {}
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

/// How a section draws: redesigned pages (#386 on) drop the collapsing headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    /// No frame of its own: the section lays out its own cards.
    Bare,
    /// One card with its title, always open.
    Card,
    /// A card with a collapsing header (the pages not redesigned yet).
    Collapsing,
}

fn style(title: &str) -> Style {
    match title {
        "Keyboard" | "Lights" => Style::Bare,
        "Connection" | "Left screen · PC panel" | "Right screen art" => Style::Card,
        _ => Style::Collapsing,
    }
}

/// A card as the canvas draws it: a surface, a line around it, its title in the HUD face.
pub fn card<R>(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    super::theme::card(ui)
        .inner_margin(egui::Margin::symmetric(18, 14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(super::theme::heading(ui, title));
            ui.add_space(6.0);
            add_contents(ui)
        })
        .inner
}

/// A card row: the name on the left, the value on the right.
pub fn row(ui: &mut egui::Ui, name: &str, value: impl Into<egui::WidgetText>) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(value)).inner
    })
    .inner
}

fn open_by_default(title: &str) -> bool {
    // A page with one or two sections opens them (#385).
    matches!(
        title,
        "Connection" | "Overlay Appearance" | "Keyboard" | "Lights" | "Screens" | "Right screen art" | "Plugins"
    )
}

/// The window's footer: project link and version on one row, then the KeyPeek credit and the
/// font credits on their own rows, so nothing overlaps at the window's fixed width (#384).
pub fn footer(ui: &mut egui::Ui) {
    if !super::shell::shows("About") {
        return;
    }
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
