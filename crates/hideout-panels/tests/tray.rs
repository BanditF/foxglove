//! The tray panel and the hub-drawn page sections, window-free (#991): which tiles show
//! (`when`, `apps.seen`, Edit tiles, off modules), the frame's state words from the supervisor's
//! facts, button navigation and `action.invoke`, and the settings form's calls.
use std::time::{Duration, Instant};

use hideout_contract::module::{Context, Module};
use hideout_contract::panel::Register;
use hideout_panels::tray::{last_seen_words, restart_words, tray_tiles};
use hideout_panels::{
    page_sections_ui, tile_ui_at, tray_panel_ui, Go, HubCall, PanelHost, PanelKey, TrayAction,
    TrayLayout,
};
use serde_json::json;

fn manifest(id: &str, when: Option<&str>) -> Module {
    let when = when
        .map(|w| format!("when = \"{w}\"\n"))
        .unwrap_or_default();
    let text = format!(
        r#"
id = "{id}"
name = "Name {id}"
version = "1.0.0"
kind = "service"
family = "foxglove"
hub = "1.1"
transport = "stdio"
api = {{ foxglove = "1.2" }}
run = ["x.exe"]
description = "An example."

[[ui.panels]]
id = "tile"
slot = "tile"
title = "Title {id}"
icon = "mic"
{when}
[[ui.panels]]
id = "page"
slot = "page"
title = "Title {id}"
icon = "mic"

[[actions]]
id = "toggle"
name = "Toggle it"

[[settings]]
name = "device"
label = "Device"
kind = "text"
default = ""
"#
    );
    Module::parse(&text, &Context::default()).expect("manifest parses")
}

fn register(host: &mut PanelHost, id: &str, button: serde_json::Value) {
    let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "tile", "slot": "tile", "title": "T", "icon": "mic", "tree":
            {"type": "column", "id": "tile", "children": [
                {"type": "value", "id": "glance", "props": {"value": "On"}},
                {"type": "button", "id": "go", "props": button}]}},
        {"id": "page", "slot": "page", "title": "T", "icon": "mic", "tree":
            {"type": "column", "id": "page", "children": [
                {"type": "section", "id": "connect", "props": {"title": "How to connect"},
                 "children": [{"type": "label", "id": "step", "props": {"text": "1."}}]}]}}]}))
    .unwrap();
    let declared = vec!["tile".to_string(), "page".to_string()];
    let actions = vec!["toggle".to_string()];
    let rejected = host.register_with(id, id, Some(&declared), Some(&actions), &params);
    assert!(rejected.is_empty(), "{rejected:?}");
}

/// Runs `draw` in a headless egui frame, `clicks` at the given points (in order, one frame
/// each after a first layout frame).
fn frame(mut draw: impl FnMut(&mut egui::Ui)) {
    let Some(fonts) = hideout_panels::render::test_fonts() else {
        return;
    };
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| draw(ui));
    });
}

#[test]
fn default_tiles_follow_when_and_apps_seen() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("a.always", None));
    host.set_manifest(manifest("b.enriches", Some("apps.seen.example-app")));
    let mut layout = TrayLayout::default();
    let shown = |host: &PanelHost, layout: &TrayLayout| -> Vec<String> {
        tray_tiles(host, layout)
            .into_iter()
            .filter(|t| t.shown)
            .map(|t| t.key.module)
            .collect()
    };
    assert_eq!(shown(&host, &layout), ["a.always"]);
    // The app has been seen on this PC: its tile shows (decision 2, §4.6).
    assert!(host.set_fact("apps.seen.example-app", json!(true)));
    assert_eq!(shown(&host, &layout), ["a.always", "b.enriches"]);
    // Hidden by the user, then shown again from Edit tiles though its app is gone.
    layout.hidden.insert("b.enriches/tile".into());
    assert_eq!(shown(&host, &layout), ["a.always"]);
    layout.hidden.clear();
    host.set_fact("apps.seen.example-app", serde_json::Value::Null);
    layout.shown.insert("b.enriches/tile".into());
    assert_eq!(shown(&host, &layout), ["a.always", "b.enriches"]);
    // An off module has no tile at all.
    host.set_fact("module.a.always.state", json!("off"));
    assert_eq!(shown(&host, &layout), ["b.enriches"]);
    // The user's order wins.
    host.set_fact("module.a.always.state", json!("running"));
    layout.order = vec!["b.enriches/tile".into(), "a.always/tile".into()];
    assert_eq!(shown(&host, &layout), ["b.enriches", "a.always"]);
}

#[test]
fn the_supervisor_facts_drive_the_tile_frame() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("m.one", None));
    register(
        &mut host,
        "m.one",
        json!({"label": "Toggle", "action": "toggle"}),
    );
    let key = PanelKey::new("m.one", "tile");
    assert!(host.panel(&key).unwrap().live());
    let t0 = Instant::now();
    host.set_fact("module.m.one.state", json!("restarting"));
    host.set_fact(
        "module.m.one.restart",
        json!({"delay_s": 4, "crash": 2, "of": 5}),
    );
    host.set_restart_since("m.one", t0);
    assert!(!host.panel(&key).unwrap().live());
    let r = host
        .restart("m.one", t0 + Duration::from_millis(1500))
        .unwrap();
    assert_eq!(
        restart_words(r.in_s, r.crash, r.of),
        "Restarting in 3 s (2 of 5)"
    );
    assert!(host.last_seen("m.one", Instant::now()).is_some());
    assert_eq!(
        last_seen_words(Duration::from_secs(12)),
        "Last seen 12 s ago"
    );
    assert_eq!(
        last_seen_words(Duration::from_secs(185)),
        "Last seen 3 min ago"
    );
    // A new registration makes the tile live again.
    host.set_fact("module.m.one.restart", serde_json::Value::Null);
    host.set_fact("module.m.one.state", json!("starting"));
    register(
        &mut host,
        "m.one",
        json!({"label": "Toggle", "action": "toggle"}),
    );
    assert!(host.panel(&key).unwrap().live());
    // Every state draws without panicking, with no module name in the frame's state words.
    for state in [
        "running",
        "restarting",
        "crash-stopped",
        "needs-hub",
        "quarantined",
    ] {
        host.set_fact("module.m.one.state", json!(state));
        frame(|ui| {
            let r = tile_ui_at(&mut host, ui, &key, Instant::now());
            assert!(!r.draw.panicked, "{state}");
            if matches!(state, "crash-stopped" | "needs-hub") {
                assert!(!r.draw.rects.contains_key("go"), "{state}: body hidden");
            }
        });
    }
}

#[test]
fn buttons_navigate_or_invoke_and_never_send_ui_event() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("m.two", None));
    register(
        &mut host,
        "m.two",
        json!({"label": "How to", "opens": "page#connect"}),
    );
    let page = host.navigate("m.two", "page#connect").unwrap();
    assert_eq!(page, PanelKey::new("m.two", "page"));
    assert_eq!(host.view(&page).unwrap().focus.as_deref(), Some("connect"));
    assert_eq!(
        host.navigate("m.two", "settings")
            .map(|k| host.view(&k).unwrap().focus.clone()),
        Some(Some(hideout_panels::sections::SETTINGS_ID.to_string()))
    );
    host.invoke("m.two", "toggle");
    assert_eq!(
        host.take_hub_calls(),
        [HubCall::Invoke {
            module: "m.two".into(),
            action: "toggle".into()
        }]
    );
    assert!(
        host.take_events().is_empty(),
        "no ui.event for opens or action"
    );
    let _ = Go::Open {
        module: "m.two".into(),
        target: "page".into(),
    };
}

#[test]
fn the_settings_section_asks_once_and_sets_one_value() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("m.three", None));
    register(&mut host, "m.three", json!({"label": "Go"}));
    host.set_key("m.three", "toggle", Some("F13".into()));
    let page = PanelKey::new("m.three", "page");
    frame(|ui| page_sections_ui(&mut host, ui, &page));
    frame(|ui| page_sections_ui(&mut host, ui, &page));
    if hideout_panels::render::test_fonts().is_some() {
        assert_eq!(
            host.take_hub_calls(),
            [HubCall::SettingsGet {
                module: "m.three".into()
            }]
        );
    }
    let mut values = serde_json::Map::new();
    values.insert("device".into(), json!("K70"));
    host.settings_result("m.three", values);
    assert_eq!(host.settings_values("m.three")["device"], json!("K70"));
    host.take_hub_calls();
    host.set_setting("m.three", "device", json!("K95"));
    assert_eq!(
        host.take_hub_calls(),
        [HubCall::SettingsSet {
            module: "m.three".into(),
            name: "device".into(),
            value: json!("K95")
        }]
    );
    host.setting_refused("m.three", "No device by that name".into());
    assert_eq!(
        host.settings_notice.get("m.three").map(String::as_str),
        Some("No device by that name")
    );
}

#[test]
fn the_tray_panel_draws_and_edits() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("m.four", None));
    host.set_manifest(manifest("m.five", Some("apps.seen.other")));
    register(
        &mut host,
        "m.four",
        json!({"label": "Toggle", "action": "toggle"}),
    );
    let mut layout = TrayLayout::default();
    let mut editing = false;
    host.set_tray_open(true);
    frame(|ui| {
        let r = tray_panel_ui(&mut host, ui, &mut layout, &mut editing, Instant::now());
        assert!(!r.actions.contains(&TrayAction::LayoutChanged));
    });
    // The when-false tile is hidden from the module (`ui.visibility` only once visible).
    assert!(host.tile_hidden(&PanelKey::new("m.five", "tile")));
    assert!(!host.tile_hidden(&PanelKey::new("m.four", "tile")));
    let vis = host.take_visibility();
    assert!(
        vis.iter().any(|(m, v)| m == "m.four" && v.panel == "tile"),
        "{vis:?}"
    );
    editing = true;
    frame(|ui| {
        tray_panel_ui(&mut host, ui, &mut layout, &mut editing, Instant::now());
    });
}
