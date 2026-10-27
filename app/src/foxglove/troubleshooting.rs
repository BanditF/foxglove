//! App › Troubleshooting (#535): Copy diagnostics (also in the status strip), Open log folder,
//! and what the keyboard said when it connected (host link version, board type, the relay, the
//! screens). The connection details were a "Details" fold on the Screens page.

use egui::RichText;

use super::screens::{self, Snapshot};
use super::theme;

/// What the right half's link reports, in words.
fn right_link(link: u8) -> &'static str {
    match link {
        0 => "not connected",
        1 => "connected",
        _ => "connected, art relay ready",
    }
}

/// The App page's Troubleshooting card.
pub fn group(ui: &mut egui::Ui) {
    super::settings_ui::section(ui, "Troubleshooting", |ui| {
        ui.horizontal_wrapped(|ui| {
            copy_diagnostics(ui);
            if ui
                .button("Open log folder")
                .on_hover_text("Foxglove's logs, in the settings folder")
                .clicked()
            {
                super::app_log::open_folder();
            }
        });
        ui.add_space(6.0);
        details(ui, &super::screens_ui::snapshot());
    });
}

/// "Copy diagnostics" (#371): copies the report; it reads "Copied" until the pointer leaves it.
pub fn copy_diagnostics(ui: &mut egui::Ui) {
    let id = ui.id().with("diagnostics-copied");
    let copied = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let response = ui
        .button(if copied { "Copied" } else { "Copy diagnostics" })
        .on_hover_text(
            "Copies a report for bug reports: versions, the connection, the GPU and the last log \
             lines. No names, paths or passwords.",
        );
    if response.clicked() {
        super::diagnostics::copy(ui.ctx());
        ui.data_mut(|d| d.insert_temp(id, true));
    } else if copied && !response.hovered() {
        ui.data_mut(|d| d.remove::<bool>(id));
    }
}

/// The connection details: what the keyboard said when it connected, for when something looks
/// wrong.
fn details(ui: &mut egui::Ui, snap: &Snapshot) {
    ui.label(theme::caps_label(ui, "Connection details"));
    let Some(hello) = snap.hello else {
        ui.label(
            RichText::new("No keyboard with Foxglove's firmware is connected.")
                .color(theme::palette(ui).muted),
        );
        return;
    };
    let status = screens::status();
    if !status.is_empty() {
        ui.add(egui::Label::new(RichText::new(status).weak()).wrap());
    }
    egui::Grid::new("troubleshooting-details")
        .num_columns(2)
        .show(ui, |ui| {
            ui.weak("Keyboard");
            ui.add(
                egui::Label::new(format!("{} · host link v{}", snap.product, hello.version)).wrap(),
            );
            ui.end_row();
            // The board profile (#75): what Foxglove knows beyond the firmware.
            ui.weak("Board type");
            ui.add(
                egui::Label::new(match super::boards::current() {
                    Some(b) if b.status == super::boards::Status::Tested => b.name,
                    Some(b) => format!("{} (untested)", b.name),
                    None => "none: only what the firmware says".into(),
                })
                .wrap(),
            );
            ui.end_row();
            // The other half of a split (#559): a one-piece board has none.
            let parts = super::parts::Parts::current().seen(if hello.right_battery.is_some() {
                2
            } else {
                1
            });
            if parts.is_split() {
                ui.weak(parts.name(1));
                ui.label(right_link(hello.right_link));
                ui.end_row();
                ui.weak(format!("{} battery", parts.short(1)));
                ui.label(
                    hello
                        .right_battery
                        .map_or("unknown".into(), |b| format!("{b}%")),
                );
                ui.end_row();
            }
        });
    let line = |ui: &mut egui::Ui, text: String| {
        ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
    };
    line(
        ui,
        format!(
            "Relay discovery: {} ({} tries)",
            hello.discovery_text(),
            hello.discovery_tries
        ),
    );
    line(ui, format!("Relay counters: {:?}", hello.debug));
    line(ui, format!("Capabilities: {:#x}", hello.capabilities));
    for s in &snap.screens {
        use digitalis::protocol::ScreenKind;
        let kind = match s.kind {
            ScreenKind::Display => "display",
            ScreenKind::Panel => "PC panel",
            ScreenKind::Art => "art area",
            ScreenKind::Other(_) => "other",
        };
        let half = if s.half == 0 { "left" } else { "right" };
        let on = s.parent.map_or(String::new(), |p| {
            format!(" at ({}, {}) on screen {p}", s.x, s.y)
        });
        line(
            ui,
            format!(
                "Screen {}: {half} {kind}, {}×{}, {}{on}",
                s.id,
                s.width,
                s.height,
                s.format_name()
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_right_link_in_words() {
        assert_eq!(right_link(0), "not connected");
        assert_eq!(right_link(1), "connected");
        assert_eq!(right_link(2), "connected, art relay ready");
    }
}
