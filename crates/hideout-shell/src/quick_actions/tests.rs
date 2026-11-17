// SPDX-License-Identifier: MIT
//! The tray menu's quick actions as plain data (#1234): no tray, no window.

use super::*;
use hideout_contract::module::Context;
use serde_json::json;

const VULPES: &str = include_str!("../../../hideout-panels/tests/vectors/vulpes.module.toml");

/// The two `action` panels the UX plan gives Arete (#1234 slice A1), after its manifest.
const ACTION_PANELS: &str = r#"
[[ui.panels]]
id = "free-gpu"
slot = "action"
title = "Free GPU"
icon = "sparkle"
when = "!gpu.reserved"

[[ui.panels]]
id = "release-gpu-reservation"
slot = "action"
title = "Release GPU reservation"
icon = "sparkle"
when = "gpu.reserved"
"#;

fn vulpes() -> Module {
    let text = format!("{VULPES}{ACTION_PANELS}");
    Module::parse(&text, &Context::default()).expect("Arete's manifest with action panels")
}

fn module(id: &str, extra: &str) -> Module {
    let text = format!(
        "id = \"{id}\"\nname = \"Other\"\nversion = \"0.1.0\"\nkind = \"service\"\nfamily = \"foxglove\"\nhub = \"1.0\"\nrun = [\"x.exe\"]\n{extra}"
    );
    Module::parse(&text, &Context::default()).expect("manifest")
}

fn words(q: &mut QuickActions) -> Vec<(String, bool)> {
    q.items().into_iter().map(|i| (i.text, i.enabled)).collect()
}

fn one(text: &str) -> Vec<(String, bool)> {
    vec![(text.to_string(), true)]
}

#[test]
fn one_item_per_gpu_reserved_and_unknown_shows_free_gpu() {
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    // Cold, `gpu.reserved` never set: Free GPU.
    assert_eq!(words(&mut q), one("Free GPU"));
    q.fact("gpu.reserved", &json!(true));
    assert_eq!(words(&mut q), one("Release GPU reservation"));
    q.fact("gpu.reserved", &json!(false));
    assert_eq!(words(&mut q), one("Free GPU"));
    // The module stopped by itself (idle, no state fact): the cached fact still decides.
    q.fact("gpu.reserved", &json!(true));
    q.fact("module.vulpes.state", &json!("running"));
    q.fact("module.vulpes.state", &Value::Null);
    assert_eq!(words(&mut q), one("Release GPU reservation"));
}

/// Game mode alone never adds, hides, greys or changes an item (§8.9, the UX plan's rule).
#[test]
fn game_mode_never_changes_the_menu() {
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    for reserved in [None, Some(false), Some(true)] {
        if let Some(r) = reserved {
            q.fact("gpu.reserved", &json!(r));
        }
        let before = q.take_change();
        assert!(before.is_some() || reserved.is_some());
        for i in 0..100 {
            q.fact("keyboard.game_mode", &json!(i % 2 == 0));
            q.fact(
                "keyboard.layer_name",
                &json!(if i % 2 == 0 { "GAME" } else { "BASE" }),
            );
            assert!(!q.changed(), "game mode changed the tray menu");
            assert_eq!(q.take_change(), None);
        }
    }
}

#[test]
fn no_item_while_missing_off_down_or_in_safe_mode() {
    let mut q = QuickActions::default();
    assert!(q.items().is_empty(), "no module, no item");
    q.manifest(&vulpes());
    for s in HIDDEN_STATES {
        q.fact("module.vulpes.state", &json!(s));
        assert!(q.items().is_empty(), "{s}");
    }
    for s in ["starting", "running", "restarting", "updating"] {
        q.fact("module.vulpes.state", &json!(s));
        assert_eq!(q.items().len(), 1, "{s}");
    }
    q.fact("hub.safe_mode", &json!(true));
    assert!(q.items().is_empty(), "safe mode holds the module");
    q.fact("hub.safe_mode", &json!(false));
    assert_eq!(q.items().len(), 1);
    q.removed("vulpes");
    assert!(q.items().is_empty(), "removed");
}

#[test]
fn an_action_panel_must_name_a_declared_action() {
    let mut q = QuickActions::default();
    // Declares an `action` panel, but no `[[actions]]` entry with its id.
    q.manifest(&module(
        "example.loose",
        "[[ui.panels]]\nid = \"do-it\"\nslot = \"action\"\ntitle = \"Do it\"\nicon = \"play\"\n",
    ));
    assert!(q.items().is_empty());
    // An action whose own `when` is false shows no item either.
    q.manifest(&module(
        "example.gated",
        "[[ui.panels]]\nid = \"go\"\nslot = \"action\"\ntitle = \"Go\"\nicon = \"play\"\n\n[[actions]]\nid = \"go\"\nname = \"Go\"\nwhen = \"lab.go\"\n",
    ));
    assert!(q.items().is_empty());
    q.fact("lab.go", &json!(true));
    assert_eq!(words(&mut q), one("Go"));
}

/// The item keeps one place in every state: the arrow count to it never changes.
#[test]
fn the_item_keeps_its_place_and_number() {
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    let other = |id: &str| {
        module(
            id,
            "[[ui.panels]]\nid = \"go\"\nslot = \"action\"\ntitle = \"Go\"\nicon = \"play\"\n\n[[actions]]\nid = \"go\"\nname = \"Go\"\n",
        )
    };
    q.manifest(&other("a.first"));
    q.manifest(&other("z.last"));
    let at = |q: &mut QuickActions| {
        q.items()
            .iter()
            .position(|i| i.module == "vulpes")
            .expect("the AI item")
    };
    let free = q.items()[at(&mut q)].n;
    let place = at(&mut q);
    q.fact("gpu.reserved", &json!(true));
    assert_eq!(at(&mut q), place);
    let release = q.items()[place].n;
    assert_ne!(free, release, "each action has its own number");
    q.fact("gpu.reserved", &json!(false));
    assert_eq!(at(&mut q), place);
    assert_eq!(q.items()[place].n, free, "numbers are kept for the run");
}

#[test]
fn a_bound_key_shows_after_a_tab() {
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    q.set_keys(&[("vulpes".into(), "free-gpu".into(), "Ctrl+Alt+G".into())]);
    assert_eq!(words(&mut q), one("Free GPU\tCtrl+Alt+G"));
    q.fact("gpu.reserved", &json!(true));
    assert_eq!(
        words(&mut q),
        one("Release GPU reservation"),
        "no key, no text"
    );
    q.set_keys(&[]);
    q.fact("gpu.reserved", &json!(false));
    assert_eq!(words(&mut q), one("Free GPU"));
}

/// While Arete runs, its `action` tree's button gives the words and the grey: "Freeing GPU…"
/// greyed while it waits; when it stops, the manifest's words are back.
#[test]
fn the_running_module_patches_words_and_grey() {
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    q.fact("module.vulpes.state", &json!("running"));
    let button = |label: &str| {
        json!([{"id": "free-gpu", "slot": "action", "title": "Free GPU", "icon": "sparkle",
            "tree": {"type": "button", "id": "press", "props": {"label": label}}}])
    };
    q.panels("vulpes", &button("Free GPU"));
    assert_eq!(words(&mut q), one("Free GPU"));
    q.patch(
        "vulpes",
        &json!({"panel": "free-gpu", "ops": [{"op": "set", "node": "press",
            "props": {"label": "Freeing GPU\u{2026}", "disabled": true}}]}),
    );
    assert_eq!(
        words(&mut q),
        vec![("Freeing GPU\u{2026}".to_string(), false)]
    );
    let n = q.take_change().unwrap()[0].n;
    assert_eq!(q.target(n), None, "a grey item presses nothing");
    // A patch on another node or panel changes nothing.
    q.patch(
        "vulpes",
        &json!({"panel": "free-gpu", "ops": [{"op": "set", "node": "other",
            "props": {"label": "Hacked", "disabled": false}}]}),
    );
    q.patch(
        "vulpes",
        &json!({"panel": "tile", "ops": [{"op": "set", "node": "press",
            "props": {"label": "Hacked"}}]}),
    );
    assert_eq!(words(&mut q)[0].0, "Freeing GPU\u{2026}");
    // It stopped: the manifest's words from the cached facts, never "Not running".
    q.fact("module.vulpes.state", &Value::Null);
    assert_eq!(words(&mut q), one("Free GPU"));
}

#[test]
fn pause_greys_the_items() {
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    q.set_paused(true);
    assert_eq!(words(&mut q), vec![("Free GPU".to_string(), false)]);
    let n = q.take_change().unwrap()[0].n;
    assert_eq!(q.target(n), None);
    q.set_paused(false);
    let n = q.take_change().unwrap()[0].n;
    assert_eq!(q.target(n), Some(("vulpes".into(), "free-gpu".into())));
}

/// A pick of an item that has since gone presses nothing (a stale menu).
#[test]
fn a_stale_pick_presses_nothing() {
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    let free = q.take_change().unwrap()[0].n;
    q.fact("gpu.reserved", &json!(true));
    let release = q.take_change().unwrap()[0].n;
    assert_eq!(q.target(free), None, "Free GPU left the menu");
    assert_eq!(
        q.target(release),
        Some(("vulpes".into(), "release-gpu-reservation".into()))
    );
    assert_eq!(q.target(999), None);
}

#[test]
fn menu_words_are_plain_and_short() {
    assert_eq!(menu_words("Free\tGPU"), "Free GPU", "no tab fakes a key");
    assert_eq!(menu_words("Save & quit"), "Save && quit");
    assert_eq!(menu_words("\u{202e}evil\u{200b}"), "evil");
    assert_eq!(menu_words("  a \n b  "), "a b");
    let long = menu_words(&"x".repeat(100));
    assert_eq!(long.chars().count(), LABEL_MAX + 1);
    assert!(long.ends_with('\u{2026}'));
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    q.fact("module.vulpes.state", &json!("running"));
    q.panels(
        "vulpes",
        &json!([{"id": "free-gpu", "slot": "action", "title": "Free GPU", "icon": "sparkle",
            "tree": {"type": "button", "id": "b", "props": {"label": "\t\u{200b}"}}}]),
    );
    assert_eq!(words(&mut q), one("Free GPU"), "empty words fall back");
}

#[test]
fn failure_notices() {
    let n = failure_notice("AI", "Free GPU", "vulpes", true, "");
    assert_eq!(n.title, "AI didn't start");
    assert_eq!(
        n.body,
        "So \u{201c}Free GPU\u{201d} didn't happen. Try again from the tray menu."
    );
    assert_eq!(n.button, "Open AI");
    let n = failure_notice(
        "AI",
        "Free GPU",
        "vulpes",
        false,
        "The chat job didn't stop.",
    );
    assert_eq!(n.title, "Couldn't free GPU");
    assert_eq!(n.body, "The chat job didn't stop.");
    let n = failure_notice("AI", "Release GPU reservation", "vulpes", false, "");
    assert_eq!(n.title, "Couldn't release GPU reservation");
    assert_eq!(n.body, "AI said no.");
    let long = "\u{e9}".repeat(100);
    let n = failure_notice("AI", "Free GPU", "vulpes", false, &long);
    assert!(n.body.len() <= REASON_MAX, "{}", n.body.len());
    assert!(n.body.ends_with('\u{2026}'));
    let n = failure_notice("AI", "Free GPU", "vulpes", false, "a\u{202e}b\nc");
    assert_eq!(n.body, "ab c");
}

/// The review image's data (#1234): the menu's items in each state of the UX plan, from this
/// model, written to `HIDEOUT_REVIEW_DIR/tray-menu-items.json` for `tray_menu_mock.py` (the native
/// menu itself can't be drawn window-free). Without the variable it only checks the states.
#[test]
fn review_states_of_the_menu() {
    let menu = |q: &mut QuickActions| -> Vec<Value> {
        q.items()
            .into_iter()
            .map(|i| {
                let (label, key) = match i.text.split_once('\t') {
                    Some((l, k)) => (l.to_string(), Some(k.to_string())),
                    None => (i.text.clone(), None),
                };
                json!({"label": label.replace("&&", "&"), "key": key, "enabled": i.enabled})
            })
            .collect()
    };
    let mut states = Vec::new();
    let mut q = QuickActions::default();
    q.manifest(&vulpes());
    q.set_keys(&[("vulpes".into(), "free-gpu".into(), "Ctrl+Alt+G".into())]);
    states.push(("A", menu(&mut q)));
    q.fact("module.vulpes.state", &json!("running"));
    q.panels(
        "vulpes",
        &json!([{"id": "free-gpu", "slot": "action", "title": "Free GPU", "icon": "sparkle",
            "tree": {"type": "button", "id": "press",
                "props": {"label": "Freeing GPU\u{2026}", "disabled": true}}}]),
    );
    states.push(("B", menu(&mut q)));
    q.fact("gpu.reserved", &json!(true));
    q.fact("keyboard.game_mode", &json!(false));
    states.push(("C", menu(&mut q)));
    q.fact("hub.safe_mode", &json!(true));
    states.push(("D", menu(&mut q)));
    q.fact("hub.safe_mode", &json!(false));
    q.set_paused(true);
    states.push(("E", menu(&mut q)));
    let n1 = failure_notice(
        "AI",
        "Free GPU",
        "vulpes",
        false,
        "The chat job didn't stop in time.",
    );
    let n2 = failure_notice("AI", "Free GPU", "vulpes", true, "");
    assert_eq!(states[0].1.len(), 1);
    assert_eq!(states[1].1[0]["enabled"], false);
    assert_eq!(states[2].1[0]["label"], "Release GPU reservation");
    assert!(states[3].1.is_empty());
    assert_eq!(states[4].1[0]["enabled"], false);
    let Some(dir) = std::env::var_os("HIDEOUT_REVIEW_DIR") else {
        return;
    };
    let states: Vec<Value> = states
        .iter()
        .map(|(s, m)| json!({"state": s, "items": m}))
        .collect();
    let notices: Vec<Value> = [n1, n2]
        .iter()
        .map(|n| json!({"title": n.title, "body": n.body, "button": n.button}))
        .collect();
    let doc = json!({"states": states, "notices": notices});
    let path = std::path::Path::new(&dir).join("tray-menu-items.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    println!("{}", path.display());
}
