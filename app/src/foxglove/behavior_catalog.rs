//! The behaviours a Studio board defines in its own keymap (#582): tap dances, macros, hold-taps
//! and the like, which ZMK reports by name and parameter metadata but which have no built-in
//! role. The editor builds a picker from this metadata the way ZMK Studio does
//! (zmk-studio @ 1bb6bb84a3, Apache-2.0: `BehaviorParametersPicker.tsx`,
//! `ParameterValuePicker.tsx`): each parameter gets a widget by its type, and when a behaviour
//! publishes several parameter sets, the second parameter's choices follow the set the first
//! value picks. Nothing here names a behaviour: it all comes from the board.

use std::sync::{Arc, Mutex};

use zmk_studio_api::proto::zmk::behaviors::behavior_parameter_value_description::ValueType;
use zmk_studio_api::proto::zmk::behaviors::{
    BehaviorBindingParametersSet, BehaviorParameterValueDescription,
};
use zmk_studio_api::{params_match_metadata, Behavior, BehaviorParam};

/// One behaviour from the board's keymap, as it describes itself.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomBehavior {
    pub id: u32,
    pub name: String,
    pub sets: Vec<BehaviorBindingParametersSet>,
}

/// The connected board's own behaviours, shared between the keyboard reader (which reads them
/// when the edit session opens) and the editor.
pub type Catalog = Arc<Mutex<Vec<CustomBehavior>>>;

/// How one parameter is chosen.
#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    /// A key, from the key grid (`hid_usage`).
    Key,
    /// A layer, from the board's layers (`layer_id`).
    Layer,
    /// A number between these (`range`), both included; `name` is the board's label for it.
    Range { name: String, min: i32, max: i32 },
    /// Named values (`constant`s), in the board's order.
    Choices(Vec<(String, u32)>),
}

/// The widgets for one parameter position, from its value descriptions (deduplicated across
/// sets; `nil` needs none).
fn widgets<'a>(
    descriptions: impl Iterator<Item = &'a BehaviorParameterValueDescription>,
) -> Vec<Widget> {
    let mut out: Vec<Widget> = Vec::new();
    let mut choices: Vec<(String, u32)> = Vec::new();
    for d in descriptions {
        let widget = match &d.value_type {
            Some(ValueType::HidUsage(_)) => Widget::Key,
            Some(ValueType::LayerId(_)) => Widget::Layer,
            Some(ValueType::Range(r)) => Widget::Range {
                name: d.name.clone(),
                min: r.min,
                max: r.max,
            },
            Some(ValueType::Constant(value)) => {
                if !choices.iter().any(|(_, v)| v == value) {
                    let name = if d.name.is_empty() {
                        value.to_string()
                    } else {
                        d.name.clone()
                    };
                    choices.push((name, *value));
                }
                continue;
            }
            Some(ValueType::Nil(_)) | None => continue,
        };
        if !out.contains(&widget) {
            out.push(widget);
        }
    }
    if !choices.is_empty() {
        out.insert(0, Widget::Choices(choices));
    }
    out
}

impl CustomBehavior {
    /// The first parameter's widgets; empty when it takes none.
    pub fn first(&self) -> Vec<Widget> {
        widgets(self.sets.iter().flat_map(|s| s.param1.iter()))
    }

    /// The second parameter's widgets once the first is `param1`: only the sets that accept it.
    pub fn second(&self, param1: u32) -> Vec<Widget> {
        widgets(
            self.sets
                .iter()
                .filter(|s| accepts(&s.param1, param1))
                .flat_map(|s| s.param2.iter()),
        )
    }

    /// The binding for these values, if they are complete and the board's metadata accepts
    /// them (the firmware's own rule, `params_match_metadata`). A parameter without widgets is
    /// zero.
    pub fn stage(&self, param1: Option<u32>, param2: Option<u32>) -> Option<Behavior> {
        let p1 = match (self.first().is_empty(), param1) {
            (true, _) => 0,
            (false, Some(v)) => v,
            (false, None) => return None,
        };
        let p2 = match (self.second(p1).is_empty(), param2) {
            (true, _) => 0,
            (false, Some(v)) => v,
            (false, None) => return None,
        };
        if !params_match_metadata(&self.sets, p1, p2) {
            return None;
        }
        let (param1, param2) = typed_params(&self.sets, p1, p2);
        Some(Behavior::Custom {
            behavior_id: self.id,
            display_name: self.name.clone(),
            param1,
            param2,
        })
    }

    /// Whether `behavior` is this behaviour with values its metadata accepts.
    pub fn accepts(&self, behavior: &Behavior) -> bool {
        matches!(behavior, Behavior::Custom { behavior_id, .. } if *behavior_id == self.id)
            && behavior.matches_metadata(&self.sets)
    }
}

/// Types both values against the metadata, as the client types a binding it reads (the set
/// that accepts both, else the first; each value by the description that covers it). Mirrors
/// `zmk_studio_api::typed_params`, which 0.6.0 keeps private.
pub fn typed_params(
    sets: &[BehaviorBindingParametersSet],
    param1: u32,
    param2: u32,
) -> (BehaviorParam, BehaviorParam) {
    let Some(set) = sets
        .iter()
        .find(|s| accepts(&s.param1, param1) && accepts(&s.param2, param2))
        .or(sets.first())
    else {
        return (BehaviorParam::Unused, BehaviorParam::Unused);
    };
    (typed(&set.param1, param1), typed(&set.param2, param2))
}

fn typed(values: &[BehaviorParameterValueDescription], value: u32) -> BehaviorParam {
    let covering = values
        .iter()
        .find(|d| accepts(std::slice::from_ref(*d), value));
    match covering.map(|d| &d.value_type) {
        Some(Some(ValueType::HidUsage(_))) => {
            BehaviorParam::Keycode(zmk_studio_api::HidUsage::from_encoded(value))
        }
        Some(Some(ValueType::LayerId(_))) => BehaviorParam::LayerId(value),
        Some(Some(ValueType::Nil(_))) => BehaviorParam::Unused,
        Some(_) => BehaviorParam::Number(value),
        None if value == 0 => BehaviorParam::Unused,
        None => BehaviorParam::Number(value),
    }
}

/// True when one of `values` covers `value` (an empty list takes only 0, as the firmware does).
fn accepts(values: &[BehaviorParameterValueDescription], value: u32) -> bool {
    let set = BehaviorBindingParametersSet {
        param1: values.to_vec(),
        param2: vec![],
    };
    params_match_metadata(&[set], value, 0)
}

/// True for the names ZMK's built-in behaviours report (their `display-name`, or the
/// devicetree node name for the three without one). Mirrors `role_from_display_name` in
/// `zmk-studio-api` 0.6.0 (Apache-2.0), which re-exports it only from 0.6.1; use that when the
/// app moves up.
fn is_built_in(name: &str) -> bool {
    const BUILT_IN: [&str; 26] = [
        "key press",
        "key toggle",
        "layer-tap",
        "mod-tap",
        "sticky key",
        "sticky layer",
        "momentary layer",
        "toggle layer",
        "to layer",
        "bluetooth",
        "external power",
        "output selection",
        "backlight",
        "underglow",
        "mouse key press",
        "caps word",
        "key repeat",
        "reset",
        "bootloader",
        "studio unlock",
        "grave/escape",
        "transparent",
        "none",
        "mouse_move",
        "mouse_scroll",
        "z_so_off",
    ];
    BUILT_IN.contains(&name.trim().to_ascii_lowercase().as_str())
}

/// Reads the board's own behaviours: every behaviour it lists whose name is not one of ZMK's
/// built-in roles. One `get_behavior_details` per behaviour, as ZMK Studio does.
pub fn read<T: std::io::Read + std::io::Write>(
    client: &mut zmk_studio_api::StudioClient<T>,
) -> Result<Vec<CustomBehavior>, zmk_studio_api::ClientError> {
    let mut out = Vec::new();
    for id in client.list_all_behaviors()? {
        let details = client.get_behavior_details(id)?;
        if is_built_in(&details.display_name) {
            continue;
        }
        out.push(CustomBehavior {
            id,
            name: details.display_name,
            sets: details.metadata,
        });
    }
    out.sort_by_key(|b| b.name.to_lowercase());
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use zmk_studio_api::proto::zmk::behaviors::{
        BehaviorParameterHidUsage, BehaviorParameterLayerId, BehaviorParameterNil,
        BehaviorParameterValueDescriptionRange,
    };
    use zmk_studio_api::HidUsage;

    pub(crate) fn key() -> BehaviorParameterValueDescription {
        BehaviorParameterValueDescription {
            name: "Key".into(),
            value_type: Some(ValueType::HidUsage(BehaviorParameterHidUsage {
                keyboard_max: 0xff,
                consumer_max: 0xfff,
            })),
        }
    }

    pub(crate) fn range(name: &str, min: i32, max: i32) -> BehaviorParameterValueDescription {
        BehaviorParameterValueDescription {
            name: name.into(),
            value_type: Some(ValueType::Range(BehaviorParameterValueDescriptionRange {
                min,
                max,
            })),
        }
    }

    pub(crate) fn constant(name: &str, value: u32) -> BehaviorParameterValueDescription {
        BehaviorParameterValueDescription {
            name: name.into(),
            value_type: Some(ValueType::Constant(value)),
        }
    }

    pub(crate) fn layer() -> BehaviorParameterValueDescription {
        BehaviorParameterValueDescription {
            name: "Layer".into(),
            value_type: Some(ValueType::LayerId(BehaviorParameterLayerId {})),
        }
    }

    pub(crate) fn nil() -> BehaviorParameterValueDescription {
        BehaviorParameterValueDescription {
            name: String::new(),
            value_type: Some(ValueType::Nil(BehaviorParameterNil {})),
        }
    }

    /// A user-defined behaviour with a key and a count (#582's acceptance case).
    pub(crate) fn key_and_count(id: u32) -> CustomBehavior {
        CustomBehavior {
            id,
            name: "Repeat key".into(),
            sets: vec![BehaviorBindingParametersSet {
                param1: vec![key()],
                param2: vec![range("Times", 1, 5)],
            }],
        }
    }

    /// Two sets, as `&bt` publishes: "Next" takes nothing more, "Select" takes a slot 0-4.
    pub(crate) fn two_sets(id: u32) -> CustomBehavior {
        CustomBehavior {
            id,
            name: "Profile".into(),
            sets: vec![
                BehaviorBindingParametersSet {
                    param1: vec![constant("Next", 0)],
                    param2: vec![nil()],
                },
                BehaviorBindingParametersSet {
                    param1: vec![constant("Select", 1)],
                    param2: vec![range("Slot", 0, 4)],
                },
            ],
        }
    }

    #[test]
    fn built_in_names_are_not_the_boards_own() {
        assert!(is_built_in("Key Press"));
        assert!(is_built_in(" mouse_move "));
        assert!(!is_built_in("Repeat key"));
        for role in [
            zmk_studio_api::BehaviorRole::KeyPress,
            zmk_studio_api::BehaviorRole::Transparent,
            zmk_studio_api::BehaviorRole::MomentaryLayer,
        ] {
            assert!(is_built_in(role.label()), "{}", role.label());
        }
    }

    #[test]
    fn widgets_follow_the_parameter_types() {
        let b = key_and_count(5);
        assert_eq!(b.first(), vec![Widget::Key]);
        assert_eq!(
            b.second(0x07_0004),
            vec![Widget::Range {
                name: "Times".into(),
                min: 1,
                max: 5
            }]
        );
        let none = CustomBehavior {
            id: 6,
            name: "Caps sentence".into(),
            sets: vec![],
        };
        assert!(none.first().is_empty());
        assert!(none.second(0).is_empty());
        let layered = CustomBehavior {
            id: 7,
            name: "Hold layer".into(),
            sets: vec![BehaviorBindingParametersSet {
                param1: vec![layer()],
                param2: vec![key()],
            }],
        };
        assert_eq!(layered.first(), vec![Widget::Layer]);
        assert_eq!(layered.second(1), vec![Widget::Key]);
    }

    #[test]
    fn the_second_parameter_follows_the_set_the_first_picks() {
        let b = two_sets(8);
        assert_eq!(
            b.first(),
            vec![Widget::Choices(vec![
                ("Next".into(), 0),
                ("Select".into(), 1)
            ])]
        );
        assert!(b.second(0).is_empty(), "Next takes nothing more");
        assert_eq!(
            b.second(1),
            vec![Widget::Range {
                name: "Slot".into(),
                min: 0,
                max: 4
            }]
        );
        assert!(b.stage(Some(0), None).is_some());
        assert_eq!(b.stage(Some(1), None), None, "Select needs a slot");
        assert!(b.stage(Some(1), Some(3)).is_some());
        assert_eq!(b.stage(Some(1), Some(9)), None, "out of range");
        assert_eq!(b.stage(Some(2), None), None, "not a value the board lists");
    }

    #[test]
    fn a_complete_binding_stages_and_round_trips() {
        let b = key_and_count(5);
        let a = HidUsage::from_encoded(0x07_0004);
        assert_eq!(b.stage(None, Some(2)), None);
        assert_eq!(b.stage(Some(a.to_hid_usage()), None), None);
        let staged = b.stage(Some(a.to_hid_usage()), Some(3)).unwrap();
        assert_eq!(
            staged,
            Behavior::Custom {
                behavior_id: 5,
                display_name: "Repeat key".into(),
                param1: BehaviorParam::Keycode(a),
                param2: BehaviorParam::Number(3),
            }
        );
        assert!(b.accepts(&staged));
        assert_eq!(staged.raw_params(), (0x07_0004, 3));
        assert_eq!(b.stage(Some(a.to_hid_usage()), Some(6)), None, "over max");
        assert!(!key_and_count(9).accepts(&staged), "another behaviour");
    }
}
