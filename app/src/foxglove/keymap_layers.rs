//! The keymap editor's Layers prompt for Studio boards (#583): rename, move, remove and add
//! layers, put back a layer removed this session, and switch the physical layout. Emulates ZMK
//! Studio's `LayerPicker.tsx` and `PhysicalLayoutPicker.tsx` (zmk-studio @ 1bb6bb84a3,
//! Apache-2.0). The limits come from the board: `available_layers` (reserved slots left),
//! `max_layer_name_length` and its physical layouts. Every change is checked first
//! ([`check_layer_op`]); the board is the source of truth afterwards, so each change is
//! followed by a read of its keymap, and the overlay and the layer names follow it.

use egui::RichText;
use egui_phosphor::regular as icon;

use crate::foxglove::behavior_catalog::LayerRoom;
use crate::foxglove::keymap_health::{self, Change};
use crate::foxglove::keymap_session::{plain_reason, Prompt};
use crate::foxglove::studio_session::{LayerOp, Status, StudioRequest, LOCKED_MESSAGE};
use crate::key_action::LayerInfo;
use crate::keyboard::Keyboard;
use crate::keymap_editor::EditorState;

/// Said when the board has no reserved layer slot left (or never had one).
pub const NO_SPACE: &str = "No space for another layer. Layers can only be added in spare \
    layer slots that the keyboard's firmware sets aside (layers marked reserved in its keymap).";

/// A refused add or restore, short: the headroom line under the layers says why.
const NO_SPACE_SHORT: &str = "there's no space for another layer";

/// Checks a layer change before it is sent, against what the board last reported. `Err` is
/// the reason in the user's words; nothing is sent.
pub fn check_layer_op(
    layers: &[LayerInfo],
    room: Option<&LayerRoom>,
    op: &LayerOp,
) -> Result<(), String> {
    let Some(room) = room else {
        return Err("the keyboard hasn't said how much room it has for layers yet".into());
    };
    let count = layers.len() as u32;
    match op {
        LayerOp::Add if room.free == 0 => Err(NO_SPACE_SHORT.into()),
        LayerOp::Add => Ok(()),
        LayerOp::Remove { .. } if count <= 1 => Err("the keymap needs at least one layer".into()),
        LayerOp::Remove { index } if *index >= count => Err("that layer isn't there".into()),
        LayerOp::Remove { .. } => Ok(()),
        LayerOp::Restore { id, .. } if !room.removed.iter().any(|(r, _)| r == id) => {
            Err("only layers removed since the last save can be put back".into())
        }
        LayerOp::Restore { .. } if room.free == 0 => Err(NO_SPACE_SHORT.into()),
        LayerOp::Restore { at, .. } if *at > count => Err("that position isn't there".into()),
        LayerOp::Restore { .. } => Ok(()),
        LayerOp::Move { from, to } if *from >= count || *to >= count => {
            Err("that position isn't there".into())
        }
        LayerOp::Move { from, to } if from == to => Err("the layer is already there".into()),
        LayerOp::Move { .. } => Ok(()),
        LayerOp::Rename { id, .. } if !layers.iter().any(|l| l.id == *id) => {
            Err("that layer isn't there".into())
        }
        LayerOp::Rename { name, .. } if name.trim().is_empty() => {
            Err("a layer needs a name".into())
        }
        LayerOp::Rename { name, .. }
            if room.name_max > 0 && name.trim().chars().count() > room.name_max as usize =>
        {
            Err(format!(
                "the keyboard keeps names of at most {} characters",
                room.name_max
            ))
        }
        LayerOp::Rename { .. } => Ok(()),
        LayerOp::Layout { index } if *index as usize >= room.layouts.len() => {
            Err("that layout isn't on the keyboard".into())
        }
        LayerOp::Layout { index } if *index as usize == room.active_layout => {
            Err("that layout is already in use".into())
        }
        LayerOp::Layout { .. } => Ok(()),
    }
}

/// What the headroom line says.
pub fn headroom(room: &LayerRoom) -> String {
    match room.free {
        0 => NO_SPACE.into(),
        1 => "1 more layer can be added.".into(),
        n => format!("{n} more layers can be added."),
    }
}

impl EditorState {
    /// Opens the Layers prompt with the layers' names ready to edit.
    pub(crate) fn open_layers(&mut self, keyboard: &Keyboard) {
        self.studio.layer_names = names(&keyboard.layer_infos());
        self.studio.prompt = Some(Prompt::Layers);
    }

    /// Checks `op` and sends it; a refused change is shown and noted, and never sent.
    pub(crate) fn layer_op(&mut self, keyboard: &Keyboard, op: LayerOp) {
        if self.session_busy() {
            return;
        }
        let room = keyboard.layer_room();
        match check_layer_op(&keyboard.layer_infos(), room.as_ref(), &op) {
            Ok(()) => self.start_studio(keyboard, StudioRequest::Layers(op), false),
            Err(why) => {
                self.error = Some(format!("Not changed: {why}"));
                keymap_health::note(|h| h.changed(Change::Refused(why)));
            }
        }
    }

    /// The board's answer to a layer change. Returns true when the editor must close (the
    /// physical layout changed: the app connects again to take the board's new shape).
    pub(crate) fn layer_op_done(
        &mut self,
        keyboard: &Keyboard,
        op: LayerOp,
        result: Result<Status, String>,
    ) -> bool {
        match result {
            Ok(status) => {
                self.zmk_dirty = status.unsaved;
                // The layer list changed under the history's positions.
                self.studio.history.clear();
                self.studio.layer_names = names(&keyboard.layer_infos());
                keymap_health::note(|h| h.changed(Change::Written));
                if let Some(mut target) = self.target {
                    if target.layer_index >= keyboard.layer_infos().len() {
                        target.layer_index = 0;
                    }
                    self.retarget(keyboard, target);
                }
                if matches!(op, LayerOp::Layout { .. }) {
                    keyboard.reconnect();
                    return true;
                }
            }
            Err(e) if e == LOCKED_MESSAGE => {
                self.studio.lock = crate::foxglove::keymap_session::Lock::Locked;
            }
            Err(e) => {
                // ZMK makes each layer change whole or not at all; the read-back shows which.
                let why = plain_reason(&e);
                self.error = Some(format!("The keyboard didn't make that change: {why}."));
                keymap_health::note(|h| h.changed(Change::Rejected(why)));
                self.start_studio_keep_error(keyboard, StudioRequest::Reread);
            }
        }
        false
    }

    /// The Layers prompt's body.
    pub(crate) fn layers_prompt(&mut self, ui: &mut egui::Ui, keyboard: &Keyboard) {
        ui.label(
            RichText::new(format!("{}  Layers", icon::STACK))
                .size(16.0)
                .strong(),
        );
        ui.add_space(6.0);
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            ui.add_space(4.0);
        }
        let layers = keyboard.layer_infos();
        let room = keyboard.layer_room().unwrap_or_default();
        if self.studio.layer_names.len() != layers.len() {
            self.studio.layer_names = names(&layers);
        }
        let last = layers.len().saturating_sub(1);
        let mut op = None;
        for (i, layer) in layers.iter().enumerate() {
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut self.studio.layer_names[i])
                    .hint_text(format!("Layer {}", i + 1))
                    .desired_width(150.0)
                    .char_limit(if room.name_max > 0 {
                        room.name_max as usize
                    } else {
                        usize::MAX
                    });
                let response = ui.add(edit);
                let typed = self.studio.layer_names[i].trim().to_string();
                let current = layer.name.clone().unwrap_or_default();
                if response.lost_focus() && typed != current {
                    op = Some(LayerOp::Rename {
                        id: layer.id,
                        name: typed,
                    });
                }
                let at = i as u32;
                if ui
                    .add_enabled(i > 0, egui::Button::new(icon::ARROW_UP))
                    .on_hover_text("Move up")
                    .clicked()
                {
                    op = Some(LayerOp::Move {
                        from: at,
                        to: at - 1,
                    });
                }
                if ui
                    .add_enabled(i < last, egui::Button::new(icon::ARROW_DOWN))
                    .on_hover_text("Move down")
                    .clicked()
                {
                    op = Some(LayerOp::Move {
                        from: at,
                        to: at + 1,
                    });
                }
                if ui
                    .add_enabled(layers.len() > 1, egui::Button::new(icon::TRASH))
                    .on_hover_text("Remove this layer")
                    .clicked()
                {
                    op = Some(LayerOp::Remove { index: at });
                }
            });
        }
        ui.add_space(6.0);
        ui.weak(headroom(&room));
        ui.add_space(4.0);
        if ui
            .add_enabled(room.free > 0, egui::Button::new("Add layer"))
            .clicked()
        {
            op = Some(LayerOp::Add);
        }
        if !room.removed.is_empty() {
            ui.add_space(6.0);
            ui.weak("Removed since the last save:");
            for (id, name) in &room.removed {
                ui.horizontal(|ui| {
                    ui.label(if name.is_empty() { "(no name)" } else { name });
                    if ui.button("Restore").clicked() {
                        op = Some(LayerOp::Restore {
                            id: *id,
                            at: layers.len() as u32,
                        });
                    }
                });
            }
        }
        if room.layouts.len() > 1 {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Physical layout");
                let active = room.layouts.get(room.active_layout).cloned();
                egui::ComboBox::from_id_salt("foxglove-physical-layout")
                    .selected_text(active.unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for (i, name) in room.layouts.iter().enumerate() {
                            if ui.selectable_label(i == room.active_layout, name).clicked() {
                                op = Some(LayerOp::Layout { index: i as u32 });
                            }
                        }
                    });
            });
            ui.weak("Switching reconnects the keyboard so the overlay takes its new shape.");
        }
        ui.add_space(10.0);
        if ui.button("Done").clicked() {
            self.studio.prompt = None;
            self.error = None;
        }
        if let Some(op) = op {
            self.layer_op(keyboard, op);
        }
    }
}

fn names(layers: &[LayerInfo]) -> Vec<String> {
    layers
        .iter()
        .map(|l| l.name.clone().unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layers(n: u32) -> Vec<LayerInfo> {
        (0..n)
            .map(|id| LayerInfo {
                id,
                name: Some(format!("L{id}")),
            })
            .collect()
    }

    fn room(free: u32) -> LayerRoom {
        LayerRoom {
            free,
            name_max: 8,
            layouts: vec!["Standard".into(), "Split space".into()],
            active_layout: 0,
            removed: vec![(7, "Gone".into())],
        }
    }

    #[test]
    fn layer_changes_are_checked_against_what_the_board_reports() {
        let l = layers(3);
        let ok = |op: LayerOp, free| check_layer_op(&l, Some(&room(free)), &op);
        assert!(
            check_layer_op(&l, None, &LayerOp::Add).is_err(),
            "not read yet"
        );
        assert!(ok(LayerOp::Add, 1).is_ok());
        assert_eq!(ok(LayerOp::Add, 0), Err(NO_SPACE_SHORT.into()));
        assert!(ok(LayerOp::Remove { index: 2 }, 0).is_ok());
        assert!(ok(LayerOp::Remove { index: 3 }, 0).is_err());
        assert!(
            check_layer_op(&layers(1), Some(&room(0)), &LayerOp::Remove { index: 0 })
                .unwrap_err()
                .contains("at least one layer")
        );
        assert!(ok(LayerOp::Restore { id: 7, at: 3 }, 1).is_ok());
        assert!(
            ok(LayerOp::Restore { id: 8, at: 3 }, 1).is_err(),
            "not removed"
        );
        assert!(ok(LayerOp::Move { from: 0, to: 2 }, 0).is_ok());
        assert!(ok(LayerOp::Move { from: 0, to: 3 }, 0).is_err());
        assert!(ok(LayerOp::Move { from: 1, to: 1 }, 0).is_err());
        let rename = |name: &str| LayerOp::Rename {
            id: 1,
            name: name.into(),
        };
        assert!(ok(rename("Nav"), 0).is_ok());
        assert!(ok(rename("  "), 0).is_err());
        assert!(ok(rename("Navigation"), 0)
            .unwrap_err()
            .contains("at most 8 characters"));
        assert!(ok(LayerOp::Layout { index: 1 }, 0).is_ok());
        assert!(
            ok(LayerOp::Layout { index: 0 }, 0).is_err(),
            "already in use"
        );
        assert!(ok(LayerOp::Layout { index: 2 }, 0).is_err());
    }

    #[test]
    fn the_headroom_says_how_many_layers_fit() {
        assert_eq!(headroom(&room(2)), "2 more layers can be added.");
        assert_eq!(headroom(&room(1)), "1 more layer can be added.");
        assert_eq!(headroom(&room(0)), NO_SPACE);
    }
}
