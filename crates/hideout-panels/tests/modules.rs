// SPDX-License-Identifier: MIT
//! Window-free tests over the panel model for every module in `tests/hideout-vectors/modules`
//! (H2.8): each state's tile and page register with the module's declared panels and actions,
//! stay inside the tile caps and the published tones and icons, show none of the words a person
//! never sees, and draw without panicking. The hub knows no module: it reads whatever is there.

use hideout_contract::messages::Visibility;
use hideout_contract::module::Slot;
use hideout_contract::panel::{Node, Register, ICONS, TONES};
use hideout_panels::{panel_ui, tile_ui, PanelHost, PanelKey};
use serde_json::{json, Value};
use std::path::PathBuf;

fn vectors() -> Vec<(String, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors/modules");
    let mut out: Vec<(String, Value)> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            (
                p.display().to_string(),
                serde_json::from_str(&text).unwrap(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no module vectors in {}", dir.display());
    out
}

fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().flat_map(strings).collect(),
        Value::Object(o) => o.values().flat_map(strings).collect(),
        _ => vec![],
    }
}

fn list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn each(n: &Node, f: &mut dyn FnMut(&Node)) {
    f(n);
    for c in &n.children {
        each(c, f);
    }
}

fn count(n: &Node, kind: &str) -> usize {
    let mut c = 0;
    each(n, &mut |x| c += usize::from(x.kind == kind));
    c
}

/// Every string a person can read in the tree: the nodes' props (ids and types are not shown).
fn shown_text(n: &Node) -> String {
    let mut out = Vec::new();
    each(n, &mut |x| {
        out.extend(strings(&Value::Object(x.props.clone())))
    });
    out.join("\n")
}

#[test]
fn every_module_state_registers_within_the_tile_rules() {
    for (file, doc) in vectors() {
        let module = doc["module"].as_str().unwrap();
        let name = doc["name"].as_str().unwrap();
        let (declared, actions) = (list(&doc["declared"]), list(&doc["actions"]));
        for st in doc["states"].as_array().unwrap() {
            let what = format!("{file}: {}", st["state"]);
            let params: Register =
                serde_json::from_value(json!({"instance": "i1", "panels": st["panels"]})).unwrap();
            let mut host = PanelHost::default();
            let rejected =
                host.register_with(module, name, Some(&declared), Some(&actions), &params);
            assert!(rejected.is_empty(), "{what}: {rejected:?}");
            let slots: Vec<Slot> = host.panels().iter().map(|p| p.slot).collect();
            assert_eq!(
                slots,
                [Slot::Tile, Slot::Page],
                "{what}: one tile and one page"
            );
            for p in host.panels() {
                assert!(ICONS.contains(&p.icon.as_str()), "{what}: icon {}", p.icon);
                each(&p.tree, &mut |n| {
                    if let Some(t) = n.props.get("tone") {
                        assert!(TONES.contains(&t.as_str().unwrap()), "{what}: tone {t}");
                    }
                });
                // Words a person never reads (UX plan, Wording): ids, the ui.* names, "plugin".
                let text = shown_text(&p.tree);
                for word in [module, "org.foxglove", "ui.", "plugin", "Plugin"] {
                    assert!(!text.contains(word), "{what}: {word:?} shows");
                }
            }
            let tile = &host.panels()[0].tree;
            assert_eq!(count(tile, "value"), 1, "{what}: one glance");
            for kind in ["label", "meter", "button"] {
                assert!(count(tile, kind) <= 1, "{what}: at most one {kind}");
            }
        }
    }
}

#[test]
fn every_module_state_draws() {
    let Some(fonts) = hideout_panels::render::test_fonts() else {
        eprintln!("no system font; render test skipped");
        return;
    };
    for (file, doc) in vectors() {
        let module = doc["module"].as_str().unwrap();
        for st in doc["states"].as_array().unwrap() {
            let params: Register =
                serde_json::from_value(json!({"instance": "i1", "panels": st["panels"]})).unwrap();
            let mut host = PanelHost::default();
            host.register(module, "M", None, &params);
            let ctx = egui::Context::default();
            ctx.set_fonts(fonts.clone());
            let (tile, page) = (PanelKey::new(module, "tile"), PanelKey::new(module, "page"));
            let mut panicked = false;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    panicked |= tile_ui(&mut host, ui, &tile).draw.panicked;
                    panicked |= panel_ui(&mut host, ui, &page).panicked;
                });
            });
            assert!(!panicked, "{file}: {} panicked", st["state"]);
        }
    }
}

#[test]
fn a_stopped_tile_offers_open_only() {
    let Some(fonts) = hideout_panels::render::test_fonts() else {
        return;
    };
    let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "t", "slot": "tile", "title": "T", "icon": "mic",
         "tree": {"type": "button", "id": "go", "props": {"label": "Go"}}}]}))
    .unwrap();
    let mut host = PanelHost::default();
    host.register("m", "M", None, &params);
    host.module_gone("m", "crash-stopped");
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    let mut report = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            report = Some(tile_ui(&mut host, ui, &PanelKey::new("m", "t")));
        });
    });
    let report = report.unwrap();
    assert!(
        !report.draw.rects.contains_key("go"),
        "the body's button is not drawn"
    );
    assert!(!report.open);
}

#[test]
fn tiles_are_visible_only_while_the_tray_panel_is_open() {
    let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "t", "slot": "tile", "title": "T", "icon": "mic",
         "tree": {"type": "value", "id": "v", "props": {"value": "On"}}},
        {"id": "p", "slot": "page", "title": "P", "icon": "mic",
         "tree": {"type": "label", "id": "l"}}]}))
    .unwrap();
    let mut host = PanelHost::default();
    host.register("m", "M", None, &params);
    let tile = PanelKey::new("m", "t");
    let states = |host: &mut PanelHost| -> Vec<(String, Visibility)> {
        host.take_visibility()
            .into_iter()
            .map(|(_, v)| (v.panel, v.state))
            .collect()
    };
    host.set_visible(&[]);
    assert_eq!(states(&mut host), [("p".into(), Visibility::Hidden)]);
    host.set_tray_open(true);
    assert_eq!(states(&mut host), [("t".into(), Visibility::Visible)]);
    // A page change leaves the open tray's tile as it is (sent on change only).
    host.set_visible(&[PanelKey::new("m", "p")]);
    assert_eq!(states(&mut host), [("p".into(), Visibility::Visible)]);
    host.set_tile_hidden(&tile, true);
    assert_eq!(states(&mut host), [("t".into(), Visibility::Hidden)]);
    host.set_tile_hidden(&tile, false);
    host.set_tray_open(false);
    assert_eq!(
        states(&mut host),
        [
            ("t".into(), Visibility::Visible),
            ("t".into(), Visibility::Hidden)
        ]
    );
    // A tile is never closed.
    host.close_panel(&tile);
    host.set_tray_open(true);
    assert_eq!(states(&mut host), [("t".into(), Visibility::Visible)]);
}

#[test]
fn a_button_naming_no_declared_action_is_refused() {
    let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "t", "slot": "tile", "title": "T", "icon": "mic",
         "tree": {"type": "button", "id": "go", "props": {"label": "Go", "action": "nope"}}}]}))
    .unwrap();
    let mut host = PanelHost::default();
    let rejected = host.register_with("m", "M", None, Some(&["toggle".into()]), &params);
    assert_eq!(rejected[0].error, "ui.action");
}
