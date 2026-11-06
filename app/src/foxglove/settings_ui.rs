//! Collapsible settings sections, kept apart from KeyPeek's shared widgets.

pub fn section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    // Only the current page's sections show (#385); the keymap source only with a choice (#534).
    if !super::shell::shows(title) || (title == "Connection" && !keymap_source_shown()) {
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
        // The layer overlay's card in parts (#390, #534): KeyPeek's grid on top, its colours
        // and layers below a line, then the keymap source when it shows, drawn as one card.
        part @ (Style::CardTop | Style::CardMiddle | Style::CardBottom) => {
            let top = part == Style::CardTop;
            if !top {
                // Back over the gap between sections, so the two outlines meet in one line.
                ui.add_space(-(6.0 + ui.spacing().item_spacing.y + 1.0));
            }
            let (r, z) = (8, 0);
            let (upper, lower) = match part {
                Style::CardTop => (r, z),
                Style::CardMiddle => (z, z),
                _ => (z, r),
            };
            let corners = egui::CornerRadius {
                nw: upper,
                ne: upper,
                sw: lower,
                se: lower,
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
    /// The top, middle and bottom parts of one card.
    CardTop,
    CardMiddle,
    CardBottom,
}

thread_local! {
    /// Keyboards found for the overlay's keymap, whether one is connected, and its layouts (#534).
    static KEYMAP_CHOICE: std::cell::Cell<(usize, bool, usize)> = const { std::cell::Cell::new((0, false, 0)) };
}

/// What the keymap source has to offer this frame: the keyboards found, whether one is
/// connected, and the connected one's layouts. KeyPeek's settings code tells it before drawing
/// the sections.
pub fn keymap_choice(found: usize, connected: bool, layouts: usize) {
    KEYMAP_CHOICE.set((found, connected, layouts));
}

/// The keymap source shows unless there's nothing to choose: one keyboard, connected, with at
/// most one layout.
pub fn keymap_source_shown() -> bool {
    let (found, connected, layouts) = KEYMAP_CHOICE.get();
    !(found == 1 && connected && layouts <= 1)
}

/// A heading row inside one of KeyPeek's two-column grids ("When it shows", "Appearance").
pub fn grid_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(super::theme::caps_label(ui, text));
    ui.end_row();
}

/// The name a section's card shows, where the canvas names it differently from the section.
fn shown_title(title: &str) -> &str {
    match title {
        "Connection" => "Keymap source",
        "Overlay Appearance" => "Layer overlay",
        "Theme" => "Layer colours",
        "Other keyboards (VIA)" => "Other keyboards",
        "Profile" => "Profiles",
        other => other,
    }
}

fn style(title: &str) -> Style {
    match title {
        "Keyboard" | "Lights" | "Scenes" | "Plugins" => Style::Bare,
        "Showing now"
        | "On the keyboard"
        | "On this PC"
        | "Info screen"
        | "Profile"
        | "Appearance"
        | "Startup and updates"
        | "Live wallpaper"
        | "Troubleshooting"
        | "Keyboard health"
        | "Other keyboards (VIA)" => Style::Card,
        "Overlay Appearance" => Style::CardTop,
        "Theme" if keymap_source_shown() => Style::CardMiddle,
        "Theme" => Style::CardBottom,
        "Connection" => Style::CardBottom,
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

fn expander_id(key: &str) -> egui::Id {
    egui::Id::new(("foxglove-expander", key))
}

/// Whether the expander `key` is open: closed until the user opens it, then remembered for the
/// session only (rule 6, as the zone drawings' "Show zones" is). With egui's "everything is
/// visible" (the tests that look for every sentence) it is open.
pub fn expander_open(ctx: &egui::Context, key: &str) -> bool {
    ctx.memory(|m| m.everything_is_visible())
        || ctx.data(|d| d.get_temp::<bool>(expander_id(key))) == Some(true)
}

/// Opens or closes the expander `key` (the previews and tests open it).
pub fn set_expander_open(ctx: &egui::Context, key: &str, open: bool) {
    ctx.data_mut(|d| d.insert_temp(expander_id(key), open));
}

/// What an [`expander`] drew this frame.
pub struct Expander<R> {
    /// The header row: clicked (or Enter/Space) when it opened or closed.
    pub header: egui::Response,
    /// What the contents returned, when open.
    #[cfg_attr(not(test), allow(dead_code))] // Other devices takes it up in #795.
    pub body: Option<R>,
}

/// A section collapsed by default (WinUI's `SettingsExpander`, rule 6): a header row with a
/// chevron, the `header` and a muted one-line `summary`, and `add_contents` under it once
/// opened. `key` names it for the session's memory ([`expander_open`]), so give each device row
/// its own.
pub fn expander<R>(
    ui: &mut egui::Ui,
    key: &str,
    header: &str,
    summary: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Expander<R> {
    expander_with(ui, key, header, summary, |_| {}, add_contents)
}

/// [`expander`] with `header_extra` drawn after the summary in the header row, such as a
/// status chip that keeps amber on screen while the section is closed. It sits on the row: a
/// click on it opens or closes the section too.
pub fn expander_with<R>(
    ui: &mut egui::Ui,
    key: &str,
    header: &str,
    summary: &str,
    header_extra: impl FnOnce(&mut egui::Ui),
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Expander<R> {
    use super::theme;
    let p = theme::palette(ui);
    let mut open = expander_open(ui.ctx(), key);
    let height = ui.spacing().interact_size.y + 6.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if response.clicked() {
        open = !open;
        set_expander_open(ui.ctx(), key, open);
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, header)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(6);
        if response.hovered() {
            painter.rect_filled(rect, radius, p.surface_raised);
        }
        if response.has_focus() {
            // The focus ring every control has (#384).
            painter.rect_stroke(
                rect,
                radius,
                egui::Stroke::new(2.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        // The chevron: right when closed, down when open.
        let c = egui::pos2(rect.left() + 10.0, rect.center().y);
        let points = if open {
            vec![
                c + egui::vec2(-4.0, -2.0),
                c + egui::vec2(4.0, -2.0),
                c + egui::vec2(0.0, 3.0),
            ]
        } else {
            vec![
                c + egui::vec2(-2.0, -4.0),
                c + egui::vec2(3.0, 0.0),
                c + egui::vec2(-2.0, 4.0),
            ]
        };
        let color = if response.hovered() || response.has_focus() {
            p.text_strong
        } else {
            p.muted
        };
        painter.add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    }
    let inner = rect
        .with_min_x(rect.left() + 22.0)
        .shrink2(egui::vec2(4.0, 0.0));
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.set_clip_rect(inner.intersect(ui.clip_rect()));
    row.add(
        egui::Label::new(egui::RichText::new(header).color(p.text_strong))
            .truncate()
            .selectable(false),
    );
    if !summary.is_empty() {
        row.add(
            egui::Label::new(egui::RichText::new(summary).color(p.muted))
                .truncate()
                .selectable(false),
        );
    }
    header_extra(&mut row);
    let body = open.then(|| {
        ui.add_space(2.0);
        ui.indent(expander_id(key), add_contents).inner
    });
    Expander {
        header: response,
        body,
    }
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

#[cfg(test)]
mod expander_tests {
    use super::*;
    use crate::foxglove::theme::headless::{self, click, frame, tap, texts};
    use crate::foxglove::theme::Choice;

    fn draw(ui: &mut egui::Ui, rect: &mut egui::Rect) {
        let e = expander(ui, "more-sources", "More sources", "OpenRGB off", |ui| {
            ui.label("Use OpenRGB's devices");
        });
        *rect = e.header.rect;
        assert_eq!(e.body.is_some(), expander_open(ui.ctx(), "more-sources"));
        expander(ui, "adjust-levels", "Adjust levels…", "", |ui| {
            ui.label("On battery");
        });
    }

    fn shows(shapes: &[egui::epaint::Shape], text: &str) -> bool {
        texts(shapes).iter().any(|t| t == text)
    }

    #[test]
    fn an_expander_is_closed_by_default_and_remembers_for_the_session() {
        let ctx = headless::ctx(Choice::Dark);
        let mut rect = egui::Rect::NOTHING;
        let shapes = frame(&ctx, 0.0, vec![], |ui| draw(ui, &mut rect));
        assert!(shows(&shapes, "More sources") && shows(&shapes, "OpenRGB off"));
        assert!(
            !shows(&shapes, "Use OpenRGB's devices"),
            "closed by default"
        );
        assert!(!expander_open(&ctx, "more-sources"));

        // A click on the header opens it; frames later it is still open, the other one not.
        frame(&ctx, 1.0, click(rect.center()), |ui| draw(ui, &mut rect));
        let shapes = frame(&ctx, 2.0, vec![], |ui| draw(ui, &mut rect));
        assert!(shows(&shapes, "Use OpenRGB's devices"));
        assert!(!shows(&shapes, "On battery"));
        assert!(expander_open(&ctx, "more-sources"));

        // From the keyboard: Tab onto the header, Enter closes it.
        frame(&ctx, 3.0, tap(egui::Key::Tab), |ui| draw(ui, &mut rect));
        frame(&ctx, 4.0, tap(egui::Key::Enter), |ui| draw(ui, &mut rect));
        let shapes = frame(&ctx, 5.0, vec![], |ui| draw(ui, &mut rect));
        assert!(!shows(&shapes, "Use OpenRGB's devices"));

        // The tests that look for every sentence see inside all of them.
        ctx.memory_mut(|m| m.set_everything_is_visible(true));
        let shapes = frame(&ctx, 6.0, vec![], |ui| draw(ui, &mut rect));
        assert!(shows(&shapes, "Use OpenRGB's devices") && shows(&shapes, "On battery"));
    }

    #[test]
    fn the_header_extra_sits_in_the_header_row() {
        let ctx = headless::ctx(Choice::Light);
        let shapes = frame(&ctx, 0.0, vec![], |ui| {
            expander_with(
                ui,
                "more-sources",
                "More sources",
                "OpenRGB: on",
                |ui| {
                    crate::foxglove::theme::hint_chip(
                        ui,
                        "OpenRGB not connected",
                        None,
                        Some(crate::foxglove::theme::Dot::Warn),
                    );
                },
                |_| {},
            );
        });
        assert!(shows(&shapes, "OpenRGB not connected"));
    }
}

#[cfg(test)]
mod keymap_source_tests {
    use super::*;

    /// The keymap source hides only when there's nothing to choose (#534): one keyboard found,
    /// connected, with one layout. Then the layer colours close the card.
    #[test]
    fn the_keymap_source_shows_only_with_a_choice() {
        for (found, connected, layouts, shown) in [
            (0, false, 0, true),
            (1, false, 0, true),
            (1, true, 1, false),
            (1, true, 2, true),
            (2, true, 1, true),
            (2, false, 0, true),
        ] {
            keymap_choice(found, connected, layouts);
            assert_eq!(
                keymap_source_shown(),
                shown,
                "{found} found, connected {connected}"
            );
            let theme = if shown {
                Style::CardMiddle
            } else {
                Style::CardBottom
            };
            assert_eq!(style("Theme"), theme);
        }
        assert_eq!(style("Connection"), Style::CardBottom);
        keymap_choice(0, false, 0);
    }
}
