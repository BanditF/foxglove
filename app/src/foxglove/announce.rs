//! A small shared way to say something aloud to a screen reader (#1366): a polite live region.
//!
//! A page calls [`say`] with the words when something changed that a sighted user sees (a
//! stepper's new value), and draws [`region`] once, always, where the controls are. The region
//! has no size and no looks; its accessible name is the last thing said, and it is marked polite,
//! so the reader speaks each new text without taking the keyboard focus.
//!
//! Words are the caller's: say only what is known (never a missing reading as 0).

use egui::accesskit::Live;

/// The memory slot of what was last said, per region id.
fn slot(id: egui::Id) -> egui::Id {
    id.with("announce-said")
}

/// Says `text` through the region named `id` (the next [`region`] call with the same id).
pub fn say(ctx: &egui::Context, id: egui::Id, text: impl Into<String>) {
    let text: String = text.into();
    ctx.data_mut(|d| d.insert_temp(slot(id), text));
}

/// What the region holds now (the last thing said), if anything.
pub fn said(ctx: &egui::Context, id: egui::Id) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(slot(id)))
}

/// Draws the live region: nothing to see, a polite node with the last thing said as its name.
pub fn region(ui: &mut egui::Ui, id: egui::Id) {
    let text = said(ui.ctx(), id).unwrap_or_default();
    let (_, r) = ui.allocate_exact_size(egui::Vec2::ZERO, egui::Sense::hover());
    let node = r.id;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text.clone()));
    // The name is the words (a plain label node keeps its text as a value, which a live region
    // may not speak), as the hub's crash bar does.
    ui.ctx().accesskit_node_builder(node, |n| {
        n.set_live(Live::Polite);
        n.set_label(text.clone());
    });
}
