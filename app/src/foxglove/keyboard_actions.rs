//! Safe non-typing keyboard actions, taken from the keymap (0xD7 INVOKE).

use std::collections::HashSet;
use std::sync::Mutex;

use zmk_studio_api::proto::zmk::keymap::Keymap;
use zmk_studio_api::{
    Behavior, ExternalPowerCommand, ResolvedLayer, UnderglowCommand,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Lights,
    Layers,
    Power,
    Other,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Lights => "Lights",
            Group::Layers => "Layers",
            Group::Power => "Power and firmware",
            Group::Other => "Other",
        }
    }
}

/// One thing a key on the keyboard does, ready to invoke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub label: String,
    pub group: Group,
    /// Behavior local id and parameters, as the keymap binds them.
    pub binding: (u16, u32, u32),
    /// How long to hold it.
    pub hold_ms: u16,
    /// Whether this action needs confirmation.
    pub risky: bool,
}

struct Known {
    actions: Vec<Action>,
    /// The `&rgb_ug` behavior's id, for setting colour and effect directly.
    underglow: Option<u16>,
}

static KNOWN: Mutex<Known> = Mutex::new(Known {
    actions: Vec::new(),
    underglow: None,
});

pub fn actions() -> Vec<Action> {
    KNOWN.lock().unwrap().actions.clone()
}

pub fn underglow_behavior() -> Option<u16> {
    KNOWN.lock().unwrap().underglow
}

/// Learns the actions from a keymap (the resolved bindings for meaning, the raw ones for ids).
pub fn learn(resolved: &[ResolvedLayer], raw: &Keymap) {
    let name = |layer_id: u32| {
        resolved
            .iter()
            .find(|l| l.id == layer_id)
            .map(|l| l.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("layer {layer_id}"))
    };
    let mut seen = HashSet::new();
    let mut actions = Vec::new();
    let mut underglow = None;
    for (layer, raw_layer) in resolved.iter().zip(&raw.layers) {
        for (behavior, b) in layer.bindings.iter().zip(&raw_layer.bindings) {
            let Ok(id) = u16::try_from(b.behavior_id) else {
                continue;
            };
            if matches!(behavior, Behavior::Underglow(_)) {
                underglow = Some(id);
            }
            let Some((group, label, hold_ms, risky)) = describe(behavior, &name) else {
                continue;
            };
            if seen.insert((id, b.param1, b.param2)) {
                actions.push(Action {
                    label,
                    group,
                    binding: (id, b.param1, b.param2),
                    hold_ms,
                    risky,
                });
            }
        }
    }
    actions.sort_by_key(|a| a.group);
    let allowed = actions
        .iter()
        .map(|a| palm::BehaviorInfo {
            id: a.binding.0.into(),
            param1: a.binding.1,
            param2: a.binding.2,
            name: a.label.clone(),
            hold_ms: a.hold_ms,
            risk: if a.risky {
                palm::Risk::Confirm
            } else {
                palm::Risk::Safe
            },
        })
        .collect();
    super::palm_host::set_actions(allowed, underglow);
    *KNOWN.lock().unwrap() = Known { actions, underglow };
}

/// What a binding does, if it's something worth a button: (group, label, hold ms, risky).
fn describe(b: &Behavior, layer: &dyn Fn(u32) -> String) -> Option<(Group, String, u16, bool)> {
    use Group::*;
    let (group, label, hold, risky) = match b {
        Behavior::Underglow(c) => {
            let what = match c {
                UnderglowCommand::Toggle => "Toggle",
                UnderglowCommand::On => "On",
                UnderglowCommand::Off => "Off",
                UnderglowCommand::HueInc => "Hue +",
                UnderglowCommand::HueDec => "Hue −",
                UnderglowCommand::SatInc => "Saturation +",
                UnderglowCommand::SatDec => "Saturation −",
                UnderglowCommand::BrightInc => "Brightness +",
                UnderglowCommand::BrightDec => "Brightness −",
                UnderglowCommand::SpeedInc => "Speed +",
                UnderglowCommand::SpeedDec => "Speed −",
                UnderglowCommand::EffectInc => "Next effect",
                UnderglowCommand::EffectDec => "Previous effect",
                UnderglowCommand::EffectSet | UnderglowCommand::Color => return None,
                UnderglowCommand::Other { .. } => "Other",
            };
            (Lights, format!("Lights: {what}"), 0, false)
        }
        Behavior::Backlight(c) => (Lights, format!("Backlight: {c:?}"), 0, false),
        Behavior::ToggleLayer { layer_id } => {
            (Layers, format!("Toggle {}", layer(*layer_id)), 0, false)
        }
        Behavior::ToLayer { layer_id } => (Layers, format!("Go to {}", layer(*layer_id)), 0, false),
        Behavior::StickyLayer { layer_id } => {
            (Layers, format!("Sticky {}", layer(*layer_id)), 0, false)
        }
        Behavior::ExternalPower(c) => {
            let what = match c {
                ExternalPowerCommand::On => "on",
                ExternalPowerCommand::Off => "off",
                ExternalPowerCommand::Toggle => "toggle",
                ExternalPowerCommand::Other(_) => "other",
            };
            (Power, format!("External power {what}"), 0, false)
        }
        Behavior::CapsWord => (Other, "Caps word".into(), 0, false),
        Behavior::StudioUnlock => (Other, "Unlock ZMK Studio".into(), 0, false),
        _ => return None,
    };
    Some((group, label, hold, risky))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zmk_studio_api::proto::zmk::keymap::{BehaviorBinding, Layer};

    fn raw(bindings: &[(i32, u32, u32)]) -> Layer {
        Layer {
            id: 0,
            name: String::new(),
            bindings: bindings
                .iter()
                .map(|&(behavior_id, param1, param2)| BehaviorBinding {
                    behavior_id,
                    param1,
                    param2,
                })
                .collect(),
        }
    }

    #[test]
    fn keeps_the_non_key_actions_once_each_with_their_raw_binding() {
        let resolved = vec![ResolvedLayer {
            id: 0,
            name: "QWERTY".into(),
            bindings: vec![
                Behavior::Underglow(UnderglowCommand::On),
                Behavior::Transparent,
                Behavior::ToggleLayer { layer_id: 0 },
                Behavior::Underglow(UnderglowCommand::On),
                Behavior::SoftOff,
            ],
        }];
        let keymap = Keymap {
            layers: vec![raw(&[
                (9, 1, 0),
                (1, 0, 0),
                (4, 0, 0),
                (9, 1, 0),
                (7, 0, 0),
            ])],
            ..Default::default()
        };
        learn(&resolved, &keymap);
        let a = actions();
        assert_eq!(
            a.len(),
            2,
            "duplicates, typing, and disruptive actions are left out"
        );
        assert_eq!(a[0].label, "Lights: On");
        assert_eq!(a[0].binding, (9, 1, 0));
        assert_eq!(a[1].label, "Toggle QWERTY");

        assert_eq!(underglow_behavior(), Some(9));
        assert!(permitted((9, 14, 0x123456)));
        assert!(permitted((4, 0, 0)));
        assert!(!permitted((7, 0, 0)), "soft off cannot bypass the list");
        assert!(!permitted((u16::MAX, 0, 0)));
    }
}

/// Only bindings learned as safe actions (or direct RGB commands) may be sent.
pub fn permitted(binding: (u16, u32, u32)) -> bool {
    let known = KNOWN.lock().unwrap();
    known.actions.iter().any(|a| a.binding == binding)
        || known.underglow == Some(binding.0)
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    use zmk_studio_api::{BluetoothCommand, OutputSelection};

    #[test]
    fn disruptive_behaviors_are_never_actions() {
        for b in [Behavior::Bluetooth(BluetoothCommand::Next),
            Behavior::OutputSelection(OutputSelection::Usb), Behavior::Bootloader,
            Behavior::Reset, Behavior::SoftOff] {
            assert!(describe(&b, &|_| String::new()).is_none());
        }
    }
}
