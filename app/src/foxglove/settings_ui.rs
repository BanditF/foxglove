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
        // The layer overlay's card in two parts (#390): KeyPeek's appearance grid on top, its
        // colours and layers below the line, drawn as one card.
        Style::CardTop | Style::CardBottom => {
            let top = style(title) == Style::CardTop;
            if !top {
                // Back over the gap between sections, so the two outlines meet in one line.
                ui.add_space(-(6.0 + ui.spacing().item_spacing.y + 1.0));
            }
            let (r, z) = (8, 0);
            let corners = if top {
                egui::CornerRadius {
                    nw: r,
                    ne: r,
                    sw: z,
                    se: z,
                }
            } else {
                egui::CornerRadius {
                    nw: z,
                    ne: z,
                    sw: r,
                    se: r,
                }
            };
            super::theme::card(ui)
                .corner_radius(corners)
                .inner_margin(egui::Margin::symmetric(18, 14))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    if top {
                        ui.label(super::theme::heading(ui, shown_title(title)));
                    } else {
                        ui.label(super::theme::caps_label(ui, shown_title(title)));
                    }
                    ui.add_space(6.0);
                    add_contents(ui);
                });
            return;
        }
        // The page draws its own cards (#386 on).
        Style::Bare => {
            ui.add_space(4.0);
            add_contents(ui);
            return;
        }
        Style::Card => {
            card(ui, shown_title(title), add_contents);
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
    /// The top and bottom parts of one card.
    CardTop,
    CardBottom,
}

/// The name a section's card shows, where the canvas names it differently from the section.
fn shown_title(title: &str) -> &str {
    match title {
        "Overlay Appearance" => "Layer overlay",
        "Theme" => "Colours and layers",
        "Profile" => "Profiles",
        other => other,
    }
}

fn style(title: &str) -> Style {
    match title {
        "Keyboard" | "Lights" | "Plugins" => Style::Bare,
        "Connection"
        | "Showing now"
        | "On the keyboard"
        | "On this PC"
        | "Left screen"
        | "Profile"
        | "Theme and updates"
        | "Other keyboards (VIA)" => Style::Card,
        "Overlay Appearance" => Style::CardTop,
        "Theme" => Style::CardBottom,
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

/// A card row: the name on the left, the value on the right. A value longer than the room
/// beside the name wraps: in a horizontal row a plain label would stretch the card, and with it
/// the settings window (it grew on each reconnect, when the rows fill in).
pub fn row(ui: &mut egui::Ui, name: &str, value: impl Into<egui::WidgetText>) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(value).wrap())
        })
        .inner
    })
    .inner
}

/// A row with `right` against the right edge and `left` in the room left of it, where text
/// wraps. `right` is laid out first, so a narrow card squeezes `left`, never the other way round
/// (a right-to-left part after a label runs back over the label when the card is too narrow).
pub fn split_row(
    ui: &mut egui::Ui,
    right: impl FnOnce(&mut egui::Ui),
    left: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right(ui);
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                left,
            );
        });
    });
}

/// A label and its value in two columns: the label in a column `name_width` wide (cut with an
/// ellipsis if longer), the value in the rest, wrapping there.
pub fn pair(ui: &mut egui::Ui, name_width: f32, name: &str, value: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(name_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(name_width);
                ui.add(egui::Label::new(name).truncate());
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 0.0),
            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
            value,
        );
    });
}

fn open_by_default(title: &str) -> bool {
    // A page with one or two sections opens them (#385).
    matches!(
        title,
        "Connection" | "Overlay Appearance" | "Keyboard" | "Lights" | "Screens" | "Plugins"
    )
}

/// The window's footer: project link and version on one row, then the KeyPeek credit and the
/// font credits on their own rows, so nothing overlaps at the window's fixed width (#384).
pub fn footer(ui: &mut egui::Ui) {
    if !super::shell::shows("About") {
        return;
    }
    let link = |ui: &mut egui::Ui, text: String, url: &str| {
        ui.add(egui::Hyperlink::from_label_and_url(
            egui::RichText::new(text).weak(),
            url,
        ));
    };
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        link(
            ui,
            "github.com/BanditF/foxglove".into(),
            "https://github.com/BanditF/foxglove",
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            link(
                ui,
                format!("Version {}", env!("CARGO_PKG_VERSION")),
                "https://github.com/BanditF/foxglove/releases",
            );
        });
    });
    link(
        ui,
        "based on KeyPeek by Stephan Rumswinkel".into(),
        "https://github.com/srwi/keypeek",
    );
    super::theme::credits(ui);
}
