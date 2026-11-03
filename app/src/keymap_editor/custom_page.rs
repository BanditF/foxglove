//! Foxglove (#582): the "Behaviours" page for the behaviours a Studio board defines in its own
//! keymap. Built from what the board reports (`foxglove::behavior_catalog`), the way ZMK Studio
//! builds its picker (zmk-studio @ 1bb6bb84a3, Apache-2.0: `BehaviorBindingPicker.tsx`,
//! `BehaviorParametersPicker.tsx`, `ParameterValuePicker.tsx`): pick a behaviour, then each
//! parameter by its type. A binding is written once it is complete and the board's metadata
//! accepts it; the built-in roles keep their own pages.

use zmk_studio_api::Behavior;

use super::picker::{multi_candidate_groups, SelectedKey};
use super::{zmk_catalog, EditTarget, EditorState};
use crate::foxglove::behavior_catalog::{CustomBehavior, Widget};
use crate::key_action::KeyAction;
use crate::keyboard::Keyboard;
use crate::ui_widgets::titled_group;

/// The custom page's draft: which behaviour, and the values picked so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomDraft {
    pub id: Option<u32>,
    pub param1: Option<u32>,
    pub param2: Option<u32>,
}

impl CustomDraft {
    /// The draft for a key bound to a custom behaviour.
    pub fn from_behavior(behavior: &Behavior) -> Option<Self> {
        match behavior {
            Behavior::Custom {
                behavior_id,
                param1,
                param2,
                ..
            } => Some(Self {
                id: Some(*behavior_id),
                param1: Some(param1.to_raw()),
                param2: Some(param2.to_raw()),
            }),
            _ => None,
        }
    }

    /// `behavior` picked: values start empty, except a range at its minimum (as Studio does).
    pub fn pick(behavior: &CustomBehavior) -> Self {
        let param1 = first_range(&behavior.first());
        let param2 = param1.and_then(|p1| first_range(&behavior.second(p1)));
        Self {
            id: Some(behavior.id),
            param1,
            param2,
        }
    }

    /// The binding this draft makes, when complete and accepted by the board's metadata.
    pub fn staged(&self, catalog: &[CustomBehavior]) -> Option<Behavior> {
        let behavior = catalog.iter().find(|b| Some(b.id) == self.id)?;
        behavior.stage(self.param1, self.param2)
    }
}

/// A lone range's minimum, so a count starts at its lowest value; anything else starts empty.
fn first_range(widgets: &[Widget]) -> Option<u32> {
    match widgets {
        [Widget::Range { min, .. }] => u32::try_from(*min).ok(),
        _ => None,
    }
}

/// Which parameter a widget sets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    First,
    Second,
}

impl EditorState {
    fn custom_draft(&mut self) -> &mut CustomDraft {
        self.zmk_draft
            .custom
            .get_or_insert_with(CustomDraft::default)
    }

    /// Writes the custom draft once it is complete.
    fn commit_custom(&mut self, keyboard: &Keyboard, target: EditTarget) {
        let catalog = keyboard.custom_behaviors();
        let staged = self
            .zmk_draft
            .custom
            .as_ref()
            .and_then(|d| d.staged(&catalog))
            .map(KeyAction::Zmk);
        self.commit_staged(keyboard, target, staged);
    }

    /// The Behaviours page: the board's own behaviours, then the chosen one's parameters.
    pub(super) fn draw_custom_page(
        &mut self,
        ui: &mut egui::Ui,
        keyboard: &Keyboard,
        target: EditTarget,
        search_query: &str,
        style: &crate::key_paint::KeyPaintStyle,
    ) {
        let catalog = keyboard.custom_behaviors();
        let chosen_id = self.zmk_draft.custom.as_ref().and_then(|d| d.id);
        let query = search_query.trim().to_lowercase();
        titled_group(ui, "Behaviours", |ui| {
            if catalog.is_empty() {
                ui.weak("This keyboard's keymap defines no behaviours of its own.");
            }
            for behavior in &catalog {
                let matches = query.is_empty() || behavior.name.to_lowercase().contains(&query);
                if !matches && Some(behavior.id) != chosen_id {
                    continue;
                }
                let picked = Some(behavior.id) == chosen_id;
                if ui.selectable_label(picked, &behavior.name).clicked() && !picked {
                    self.zmk_draft.custom = Some(CustomDraft::pick(behavior));
                    self.commit_custom(keyboard, target);
                }
            }
        });
        let Some(behavior) = catalog.iter().find(|b| Some(b.id) == chosen_id) else {
            return;
        };
        let first = behavior.first();
        let param1 = if first.is_empty() {
            Some(0)
        } else {
            self.custom_draft().param1
        };
        let second = param1.map(|p1| behavior.second(p1)).unwrap_or_default();
        // A key grid is long: once the key is picked, the second value goes above it.
        let key_first = first == [Widget::Key] && param1.is_some() && !second.is_empty();
        if key_first {
            self.draw_param(
                ui,
                keyboard,
                target,
                Slot::Second,
                &second,
                search_query,
                style,
            );
        }
        self.draw_param(
            ui,
            keyboard,
            target,
            Slot::First,
            &first,
            search_query,
            style,
        );
        if param1.is_none() {
            ui.weak("Choose a value above.");
            return;
        }
        if !key_first {
            self.draw_param(
                ui,
                keyboard,
                target,
                Slot::Second,
                &second,
                search_query,
                style,
            );
        }
        let draft = self.custom_draft().clone();
        if draft.staged(&catalog).is_none() {
            ui.weak("Choose every value to set this key.");
        }
    }

    /// One parameter's widgets, by type.
    #[allow(clippy::too_many_arguments)]
    fn draw_param(
        &mut self,
        ui: &mut egui::Ui,
        keyboard: &Keyboard,
        target: EditTarget,
        slot: Slot,
        widgets: &[Widget],
        search_query: &str,
        style: &crate::key_paint::KeyPaintStyle,
    ) {
        let title = match slot {
            Slot::First => "Value",
            Slot::Second => "Second value",
        };
        let current = {
            let d = self.custom_draft();
            match slot {
                Slot::First => d.param1,
                Slot::Second => d.param2,
            }
        };
        let mut picked: Option<u32> = None;
        let mut commit = false;
        for widget in widgets {
            match widget {
                Widget::Choices(choices) => {
                    titled_group(ui, title, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (name, value) in choices {
                                if ui.selectable_label(current == Some(*value), name).clicked() {
                                    picked = Some(*value);
                                    commit = true;
                                }
                            }
                        });
                    });
                }
                Widget::Layer => {
                    titled_group(ui, "Layer", |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (i, info) in keyboard.layer_infos().iter().enumerate() {
                                let name = info
                                    .name
                                    .clone()
                                    .filter(|n| !n.is_empty())
                                    .unwrap_or_else(|| format!("Layer {}", i + 1));
                                if ui
                                    .selectable_label(current == Some(info.id), name)
                                    .clicked()
                                {
                                    picked = Some(info.id);
                                    commit = true;
                                }
                            }
                        });
                    });
                }
                Widget::Range { name, min, max } => {
                    let title = if name.is_empty() {
                        title
                    } else {
                        name.as_str()
                    };
                    titled_group(ui, title, |ui| {
                        ui.horizontal(|ui| {
                            let (lo, hi) = (i64::from(*min), i64::from(*max));
                            let mut value = current.map_or(lo, i64::from).clamp(lo, hi);
                            let drag = ui.add(egui::DragValue::new(&mut value).range(lo..=hi));
                            ui.weak(format!("{min} to {max}"));
                            if drag.changed() {
                                picked = u32::try_from(value).ok();
                            }
                            commit |= drag.drag_stopped() || drag.lost_focus();
                        });
                    });
                }
                Widget::Key => {
                    titled_group(ui, "Key", |ui| {
                        let action = current.map(|v| {
                            KeyAction::Zmk(Behavior::KeyPress(
                                zmk_studio_api::HidUsage::from_encoded(v),
                            ))
                        });
                        let selected = action.as_ref().map(SelectedKey::valid);
                        multi_candidate_groups(
                            ui,
                            zmk_catalog::categories(),
                            search_query,
                            |c| keyboard.is_action_supported(&c.binding),
                            selected,
                            style,
                            |_, candidate| {
                                if let KeyAction::Zmk(Behavior::KeyPress(usage)) =
                                    &candidate.binding
                                {
                                    picked = Some(usage.to_hid_usage());
                                    commit = true;
                                }
                            },
                        );
                    });
                }
            }
        }
        if let Some(value) = picked {
            let catalog = keyboard.custom_behaviors();
            let d = self.custom_draft();
            match slot {
                Slot::First => {
                    d.param1 = Some(value);
                    // The second value's choices follow the first: start it again.
                    let behavior = catalog.iter().find(|b| Some(b.id) == d.id);
                    d.param2 = behavior.and_then(|b| first_range(&b.second(value)));
                }
                Slot::Second => d.param2 = Some(value),
            }
        }
        if commit {
            self.commit_custom(keyboard, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::behavior_catalog::tests::{key_and_count, two_sets};

    #[test]
    fn picking_a_behaviour_starts_a_count_at_its_minimum() {
        let draft = CustomDraft::pick(&key_and_count(5));
        assert_eq!(draft.param1, None, "a key has no default");
        assert_eq!(draft.param2, None, "the count waits for the key's set");
        let draft = CustomDraft::pick(&two_sets(6));
        assert_eq!(draft.param1, None, "named choices start empty");
    }

    #[test]
    fn a_bound_key_reopens_its_draft_and_stages_the_same_binding() {
        let catalog = vec![key_and_count(5), two_sets(6)];
        let bound = catalog[0].stage(Some(0x07_0004), Some(2)).unwrap();
        let draft = CustomDraft::from_behavior(&bound).unwrap();
        assert_eq!(draft.staged(&catalog), Some(bound));
        assert_eq!(CustomDraft::from_behavior(&Behavior::Transparent), None);
        let missing = CustomDraft {
            id: Some(99),
            param1: None,
            param2: None,
        };
        assert_eq!(missing.staged(&catalog), None, "not on this board");
    }
}
