//! Keyboard paths to layers, learned alongside the Studio keymap without UI-side I/O.

use std::sync::Mutex;
use zmk_studio_api::{Behavior, ResolvedLayer};

static HINTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Conditional rules are absent from Studio's keymap API. Only use the bundled rules when
/// its complete layer-name list matches, rather than assigning this board's rules to others.
fn configured_rules(layers: &[ResolvedLayer], source: &str) -> Vec<(Vec<u32>, u32)> {
    let keymap = source
        .split("compatible = \"zmk,keymap\";")
        .nth(1)
        .unwrap_or(source);
    let names: Vec<_> = keymap
        .split("display-name = \"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .collect();
    if names.len() != layers.len() || names.iter().zip(layers).any(|(a, b)| *a != b.name) {
        return Vec::new();
    }
    source
        .split("if-layers")
        .skip(1)
        .filter_map(|s| {
            let block = s.split('}').next()?;
            let cells = |s: &str| -> Option<Vec<u32>> {
                s.split('<')
                    .nth(1)?
                    .split('>')
                    .next()?
                    .split_whitespace()
                    .map(str::parse)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
            };
            let inputs: Vec<u32> = cells(block)?
                .into_iter()
                .map(|index| layers.get(index as usize).map(|l| l.id))
                .collect::<Option<_>>()?;
            let outputs = cells(block.split("then-layer").nth(1)?)?;
            (!inputs.is_empty() && outputs.len() == 1 && (outputs[0] as usize) < layers.len())
                .then(|| (inputs, layers[outputs[0] as usize].id))
        })
        .collect()
}

fn key_name(base: &ResolvedLayer, position: usize) -> String {
    let legend = base
        .bindings
        .get(position)
        .and_then(|b| match b {
            Behavior::KeyPress(_) | Behavior::LayerTap { .. } | Behavior::ModTap { .. } => {
                crate::zmk_keycode_labels::behavior_to_layout_key(b, &[]).map(|k| k.tap.full)
            }
            _ => None,
        })
        .filter(|s| !s.is_empty());
    legend.unwrap_or_else(|| format!("the key at position {}", position + 1))
}

/// Pure derivation; rules and binding targets use Studio layer ids, output uses display order.
fn derive(layers: &[ResolvedLayer], rules: &[(Vec<u32>, u32)]) -> Vec<String> {
    let Some(base) = layers.first() else {
        return Vec::new();
    };
    layers
        .iter()
        .map(|target| {
            if target.id == base.id {
                return "Base layer".into();
            }
            let mut paths = Vec::new();
            for source in layers {
                if source.id == target.id {
                    continue;
                }
                for (position, binding) in source.bindings.iter().enumerate() {
                    let (id, verb, suffix) = match binding {
                        Behavior::MomentaryLayer { layer_id }
                        | Behavior::LayerTap { layer_id, .. } => (*layer_id, "Hold", ""),
                        Behavior::ToggleLayer { layer_id } => (*layer_id, "Tap", " to toggle"),
                        Behavior::ToLayer { layer_id } => (*layer_id, "Tap", " to switch"),
                        Behavior::StickyLayer { layer_id } => {
                            (*layer_id, "Tap", " for the next key")
                        }
                        _ => continue,
                    };
                    if id != target.id {
                        continue;
                    }
                    let context = if source.id == base.id {
                        String::new()
                    } else {
                        format!("On {}: ", source.name)
                    };
                    paths.push((
                        if source.id == base.id { 0 } else { 2 },
                        format!("{context}{verb} {}{suffix}", key_name(base, position)),
                    ));
                }
            }
            for (inputs, output) in rules {
                if *output != target.id || inputs.contains(output) {
                    continue;
                }
                let names: Option<Vec<_>> = inputs
                    .iter()
                    .map(|id| layers.iter().find(|l| l.id == *id).map(|l| l.name.as_str()))
                    .collect();
                if let Some(names) = names {
                    paths.push((
                        1,
                        format!(
                            "{} ({})",
                            names.join(" + "),
                            if inputs.len() == 2 {
                                "tri-layer"
                            } else {
                                "conditional layer"
                            }
                        ),
                    ));
                }
            }
            paths.sort_by_key(|(rank, _)| *rank);
            let mut unique = Vec::new();
            for (_, path) in paths {
                if !unique.contains(&path) {
                    unique.push(path);
                }
                if unique.len() == 2 {
                    break;
                }
            }
            unique.join("\n")
        })
        .collect()
}

pub fn learn(layers: &[ResolvedLayer]) {
    let rules = configured_rules(layers, include_str!("../../../config/eyelash_corne.keymap"));
    if let Ok(mut hints) = HINTS.lock() {
        *hints = derive(layers, &rules);
    }
}

/// The label in a Show/Hold row; aggregate rows have no keyboard path.
pub fn label(ui: &mut egui::Ui, name: &str, mask: u32) {
    let response = ui.label(name);
    if mask.count_ones() == 1 {
        if let Ok(hints) = HINTS.try_lock() {
            if let Some(hint) = hints
                .get(mask.trailing_zeros() as usize)
                .filter(|s| !s.is_empty())
            {
                response.on_hover_text(hint);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layer(id: u32, name: &str, bindings: Vec<Behavior>) -> ResolvedLayer {
        ResolvedLayer {
            id,
            name: name.into(),
            bindings,
        }
    }
    #[test]
    fn prefers_base_deduplicates_and_limits_paths() {
        let layers = vec![
            layer(
                10,
                "BASE",
                vec![Behavior::MomentaryLayer { layer_id: 20 }; 3],
            ),
            layer(20, "NUMBER", vec![]),
            layer(30, "OTHER", vec![Behavior::ToggleLayer { layer_id: 20 }]),
        ];
        assert_eq!(
            derive(&layers, &[]),
            vec![
                "Base layer",
                "Hold the key at position 1\nHold the key at position 2",
                ""
            ]
        );
    }
    #[test]
    fn tap_legends_and_all_activation_kinds() {
        use zmk_studio_api::HidUsage;
        let space = HidUsage::from_parts(7, 44, 0);
        let layers = vec![
            layer(
                0,
                "BASE",
                vec![
                    Behavior::LayerTap {
                        layer_id: 1,
                        tap: space,
                    },
                    Behavior::KeyPress(space),
                ],
            ),
            layer(
                1,
                "ONE",
                vec![
                    Behavior::ToLayer { layer_id: 2 },
                    Behavior::StickyLayer { layer_id: 2 },
                ],
            ),
            layer(2, "TWO", vec![]),
        ];
        let hints = derive(&layers, &[]);
        assert_eq!(hints[1], "Hold Space");
        assert_eq!(
            hints[2],
            "On ONE: Tap Space to switch\nOn ONE: Tap Space for the next key"
        );
    }
    #[test]
    fn conditional_rules_require_matching_names_and_valid_ids() {
        let layers = vec![
            layer(0, "BASE", vec![]),
            layer(1, "NUMBER", vec![]),
            layer(2, "SYMBOL", vec![]),
            layer(5, "FUNCTION", vec![]),
        ];
        let source = "display-name = \"BASE\"; display-name = \"NUMBER\"; display-name = \"SYMBOL\"; display-name = \"FUNCTION\"; if-layers = <1 2>; then-layer = <3>; }";
        let rules = configured_rules(&layers, source);
        assert_eq!(derive(&layers, &rules)[3], "NUMBER + SYMBOL (tri-layer)");
        assert!(configured_rules(&layers[..3], source).is_empty());
        assert_eq!(derive(&layers, &[(vec![99], 5)])[3], "");
        assert!(derive(&[], &[]).is_empty());
    }
    #[test]
    fn bundled_rules_map_config_indices_to_studio_ids() {
        let mut layers: Vec<_> = [
            "QWERTY", "NUMBER", "SYMBOL", "GAME", "GAME+", "FUNCTION", "SYSTEM",
        ]
        .iter()
        .enumerate()
        .map(|(i, name)| layer(10 + i as u32, name, vec![]))
        .collect();
        layers[0].bindings = vec![Behavior::KeyPress(zmk_studio_api::HidUsage::from_parts(
            7, 43, 0,
        ))];
        layers[5].bindings = vec![Behavior::MomentaryLayer { layer_id: 16 }];
        let rules = configured_rules(
            &layers,
            include_str!("../../../config/eyelash_corne.keymap"),
        );
        assert_eq!(rules, vec![(vec![11, 12], 15), (vec![14, 12], 15)]);
        assert_eq!(derive(&layers, &rules)[6], "On FUNCTION: Hold Tab");
        assert!(configured_rules(
            &layers[..6],
            include_str!("../../../config/eyelash_corne.keymap")
        )
        .is_empty());
        assert_eq!(
            derive(&layers, &rules)[5],
            "NUMBER + SYMBOL (tri-layer)\nGAME+ + SYMBOL (tri-layer)"
        );
    }

    #[test]
    fn bundled_keymap_keeps_disruptive_keys_behind_system() {
        let source = include_str!("../../../config/eyelash_corne.keymap");
        let keymap = source.split("compatible = \"zmk,keymap\";").nth(1).unwrap();
        let bindings: Vec<Vec<String>> = keymap
            .split("display-name = \"")
            .skip(1)
            .map(|layer| {
                layer
                    .split("bindings = <")
                    .nth(1)
                    .unwrap()
                    .split('>')
                    .next()
                    .unwrap()
                    .split('&')
                    .skip(1)
                    .map(|binding| binding.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect()
            })
            .collect();
        assert_eq!(bindings.len(), 7);
        assert!(bindings.iter().all(|layer| layer.len() == 48));
        for layer in &bindings[..6] {
            assert!(!layer.iter().any(|binding| {
                [
                    "bt ",
                    "out ",
                    "bootloader",
                    "sys_reset",
                    "soft_off",
                    "tog ",
                    "to ",
                ]
                .iter()
                .any(|prefix| binding.starts_with(prefix))
            }));
        }
        assert_eq!(bindings[5][0], "mo 6");
        assert_eq!(bindings[6][0], "trans");
        assert_eq!(bindings[6][28], "bt BT_CLR");
        assert_eq!(bindings[6][41], "bt BT_CLR_ALL");
        for profile in 0..5 {
            assert_eq!(bindings[6][profile + 1], format!("bt BT_SEL {profile}"));
        }
        // Neither reset, bootloader nor soft-off belongs on the home row.
        assert!(!bindings[6][13..28].iter().any(|binding| {
            ["bootloader", "sys_reset", "soft_off"].contains(&binding.as_str())
        }));
    }

    #[test]
    fn duplicate_legends_self_bindings_and_unknown_targets() {
        use zmk_studio_api::HidUsage;
        let space = HidUsage::from_parts(7, 44, 0);
        let layers = vec![
            layer(
                0,
                "BASE",
                vec![
                    Behavior::LayerTap {
                        layer_id: 1,
                        tap: space,
                    },
                    Behavior::LayerTap {
                        layer_id: 1,
                        tap: space,
                    },
                    Behavior::ToggleLayer { layer_id: 1 },
                    Behavior::MomentaryLayer { layer_id: 99 },
                ],
            ),
            layer(1, "ONE", vec![Behavior::MomentaryLayer { layer_id: 1 }]),
        ];
        assert_eq!(
            derive(&layers, &[])[1],
            "Hold Space\nTap the key at position 3 to toggle"
        );
    }
}
