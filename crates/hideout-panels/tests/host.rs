//! The panel model and the egui renderer, window-free: patches, the panel guard on patches,
//! bindings, optimistic input, the feed, and view state (scroll, tab) through a re-registration
//! drawn in egui's headless context.

use hideout_contract::messages::UiPatch;
use hideout_contract::panel::Register;
use hideout_panels::egui;
use hideout_panels::feed::{channel, Inbound, Outbound};
use hideout_panels::model::Interaction;
use hideout_panels::{panel_ui, DrawReport, PanelHost, PanelKey};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const M: &str = "example.module";

fn register(host: &mut PanelHost, instance: &str, tree: Value) -> Vec<String> {
    let params: Register = serde_json::from_value(json!({"instance": instance, "panels": [
        {"id": "main", "slot": "page", "title": "Main", "icon": "image", "tree": tree}]}))
    .unwrap();
    host.register(M, "Example", None, &params)
        .into_iter()
        .map(|r| r.error)
        .collect()
}

fn patch(ops: Value) -> UiPatch {
    serde_json::from_value(json!({"panel": "main", "ops": ops})).unwrap()
}

fn key() -> PanelKey {
    PanelKey::new(M, "main")
}

/// A long page (so it scrolls) with tabs at the top.
fn sample(labels: usize, extra_tab: bool) -> Value {
    let mut tabs = vec![
        json!({"type": "column", "id": "tab-a", "props": {"title": "A"}, "children": [
            {"type": "label", "id": "in-a", "props": {"text": "first"}}]}),
        json!({"type": "column", "id": "tab-b", "props": {"title": "B"}, "children": [
            {"type": "button", "id": "go", "props": {"label": "Go"}}]}),
    ];
    if extra_tab {
        tabs.insert(
            0,
            json!({"type": "column", "id": "tab-new", "props": {"title": "New"}}),
        );
    }
    let mut children = vec![json!({"type": "tabs", "id": "tabs", "children": tabs})];
    children.extend((0..labels).map(
        |i| json!({"type": "label", "id": format!("l{i}"), "props": {"text": format!("Line {i}")}}),
    ));
    json!({"type": "column", "id": "root", "children": children})
}

#[test]
fn patches_apply_and_a_bad_patch_leaves_the_tree() {
    let mut host = PanelHost::default();
    assert!(register(&mut host, "i1", sample(2, false)).is_empty());
    let now = Instant::now();
    host.patch(
        M,
        &patch(json!([
            {"op": "set", "node": "l0", "props": {"text": "changed", "extra": 1}},
            {"op": "insert", "node": "root", "index": 1,
             "tree": {"type": "label", "id": "added", "props": {"text": "new"}}},
            {"op": "remove", "node": "l1"}])),
        now,
    )
    .unwrap();
    let tree = host.panel(&key()).unwrap().tree.clone();
    let ids: Vec<&str> = tree.children.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["tabs", "added", "l0"]);
    assert_eq!(tree.children[2].props["text"], "changed");

    // Null removes a prop.
    let later = now + Duration::from_secs(1);
    host.patch(
        M,
        &patch(json!([{"op": "set", "node": "l0", "props": {"extra": null}}])),
        later,
    )
    .unwrap();
    let good = host.panel(&key()).unwrap().tree.clone();
    assert!(!good.children[2].props.contains_key("extra"));

    // A duplicate id, a missing node, removing the root and a cap breach are refused whole.
    let bad = [
        json!([{"op": "set", "node": "l0", "props": {"text": "x"}},
               {"op": "insert", "node": "root", "tree": {"type": "label", "id": "l0"}}]),
        json!([{"op": "set", "node": "nope", "props": {}}]),
        json!([{"op": "remove", "node": "root"}]),
        json!([{"op": "set", "node": "l0", "props": {"text": "x".repeat(16 * 1024 + 1)}}]),
    ];
    for (i, ops) in bad.into_iter().enumerate() {
        let t = later + Duration::from_secs(1 + i as u64);
        let e = host.patch(M, &patch(ops), t).unwrap_err();
        assert_eq!(e.id(), "ui.limits");
        assert_eq!(host.panel(&key()).unwrap().tree, good);
    }
    // Patches only reach the module's own panels.
    assert_eq!(
        host.patch("other.module", &patch(json!([])), later)
            .unwrap_err()
            .id(),
        "ui.undeclared"
    );
}

#[test]
fn duplicate_ids_reject_the_panel() {
    let mut host = PanelHost::default();
    let tree = json!({"type": "column", "id": "root", "children": [
        {"type": "label", "id": "x"}, {"type": "label", "id": "x"}]});
    assert_eq!(register(&mut host, "i1", tree), ["ui.limits"]);
}

#[test]
fn optimistic_input_reverts_on_refusal() {
    let mut host = PanelHost::default();
    let tree = json!({"type": "toggle", "id": "t", "props": {"value": false}});
    register(&mut host, "i1", tree);
    let toggle = |host: &mut PanelHost| {
        host.emit(Interaction {
            key: key(),
            node: "t".into(),
            kind: "change".into(),
            value: json!(true),
            optimistic: true,
        });
        host.take_events()[0].seq
    };
    let seq = toggle(&mut host);
    assert_eq!(host.view(&key()).unwrap().optimistic["t"], json!(true));
    host.event_result(seq, Err("Not now".into()));
    assert!(host.view(&key()).unwrap().optimistic.is_empty());
    assert_eq!(
        host.panel(&key()).unwrap().notice.as_deref(),
        Some("Not now")
    );

    let seq = toggle(&mut host);
    host.event_result(seq, Ok(()));
    assert_eq!(host.panel(&key()).unwrap().tree.props["value"], json!(true));
    assert!(host.panel(&key()).unwrap().notice.is_none());
}

#[test]
fn bindings_follow_facts() {
    let mut host = PanelHost::default();
    register(
        &mut host,
        "i1",
        json!({"type": "value", "id": "v", "props": {"bind": "fact:keyboard.layer"}}),
    );
    assert!(host.bound_facts().contains("keyboard.layer"));
    assert!(host.set_fact("keyboard.layer", json!(2)));
    assert!(
        !host.set_fact("keyboard.layer", json!(2)),
        "no redraw without a change"
    );
    assert!(!host.set_fact("unbound.fact", json!(1)));
}

#[test]
fn the_feed_carries_registration_patches_and_events() {
    let (router, mut feed) = channel();
    let woken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woken.clone();
    hideout_panels::PanelFeed::set_waker(
        &mut feed,
        Box::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }),
    );
    let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "main", "slot": "page", "title": "Main", "icon": "x",
         "tree": {"type": "button", "id": "b", "props": {"label": "Go"}}},
        {"id": "undeclared", "slot": "tile", "title": "T", "icon": "x",
         "tree": {"type": "label", "id": "l"}}]}))
    .unwrap();
    router.push(Inbound::Register {
        call: 7,
        module: M.into(),
        name: "Example".into(),
        declared: Some(vec!["main".into()]),
        params,
    });
    assert_eq!(woken.load(std::sync::atomic::Ordering::SeqCst), 1);
    let mut host = PanelHost::default();
    let (changed, next) = host.pump(&mut feed, Instant::now());
    assert!(changed && next.is_none());
    match &router.take()[..] {
        [Outbound::Registered {
            call: 7, rejected, ..
        }] => {
            assert_eq!(rejected.len(), 1);
            assert_eq!(rejected[0].error, "ui.undeclared");
        }
        other => panic!("{other:?}"),
    }
    host.emit(Interaction {
        key: key(),
        node: "b".into(),
        kind: "click".into(),
        value: Value::Null,
        optimistic: false,
    });
    host.pump(&mut feed, Instant::now());
    let out = router.take();
    assert!(matches!(&out[..], [Outbound::Event(e)] if e.module == M && e.event.node == "b"));
}

// ---- egui, headless ----

fn frame(ctx: &egui::Context, host: &mut PanelHost, events: Vec<egui::Event>) -> DrawReport {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        events,
        ..Default::default()
    };
    let mut report = None;
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            report = Some(panel_ui(host, ui, &key()));
        });
    });
    report.unwrap()
}

fn context() -> Option<egui::Context> {
    let fonts = hideout_panels::render::test_fonts()?;
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    Some(ctx)
}

fn click(ctx: &egui::Context, host: &mut PanelHost, at: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(ctx, host, vec![egui::Event::PointerMoved(at), button(true)]);
    frame(ctx, host, vec![button(false)]);
}

#[test]
fn a_re_registration_keeps_scroll_and_tab_in_egui() {
    let Some(ctx) = context() else {
        eprintln!("no system font; render test skipped");
        return;
    };
    let mut host = PanelHost::default();
    register(&mut host, "inst-1", sample(100, false));
    let r = frame(&ctx, &mut host, vec![]);
    for _ in 0..2 {
        frame(&ctx, &mut host, vec![]);
    }
    // The user picks tab B, then scrolls down.
    click(&ctx, &mut host, r.rects["tabs#tab-b"].center());
    assert_eq!(host.view(&key()).unwrap().tab["tabs"], "tab-b");
    host.view_mut(&key())
        .scroll
        .insert("root".into(), [0.0, 300.0]);
    // A restart: the module greys, then a new instance registers a changed tree (a new tab in
    // front, so the index of B moves).
    host.module_gone(M, "restarting");
    register(&mut host, "inst-2", sample(100, true));
    // A fresh egui context too, so nothing comes from egui's own memory.
    let ctx = context().unwrap();
    let r = frame(&ctx, &mut host, vec![]);
    frame(&ctx, &mut host, vec![]);
    let view = host.view(&key()).unwrap();
    assert_eq!(view.tab["tabs"], "tab-b");
    assert_eq!(view.scroll["root"], [0.0, 300.0]);
    assert!(r.rects.contains_key("go"), "tab B's content is drawn");
    assert!(!r.rects.contains_key("in-a"));
}

#[test]
fn a_click_becomes_a_ui_event_for_the_owner() {
    let Some(ctx) = context() else {
        return;
    };
    let mut host = PanelHost::default();
    let tree = json!({"type": "column", "id": "root", "children": [
        {"type": "button", "id": "go", "props": {"label": "Go"}},
        {"type": "future.widget", "id": "f"},
        {"type": "canvas", "id": "cv", "props": {"width": 100, "height": 50}, "children": [
            {"type": "key", "id": "k1", "props": {"x": 10, "y": 10, "w": 20, "h": 20}}]}]});
    register(&mut host, "i1", tree);
    let r = frame(&ctx, &mut host, vec![]);
    frame(&ctx, &mut host, vec![]);
    click(&ctx, &mut host, r.rects["go"].center());
    click(&ctx, &mut host, r.rects["cv/k1"].center());
    let out = host.take_events();
    let got: Vec<(&str, &str, Value)> = out
        .iter()
        .map(|o| {
            (
                o.event.node.as_str(),
                o.event.kind.as_str(),
                o.event.value.clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [("go", "click", Value::Null), ("cv", "click", json!("k1"))]
    );
    assert!(out.iter().all(|o| o.module == M));
    // A greyed panel takes no input.
    host.module_gone(M, "stopped");
    click(&ctx, &mut host, r.rects["go"].center());
    assert!(host.take_events().is_empty());
}

#[test]
fn every_node_type_draws() {
    let Some(ctx) = context() else {
        return;
    };
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/hideout-vectors/panels/valid/all-node-types.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut panel = v["params"]["panels"][0].clone();
    panel["id"] = json!("main");
    let params: Register =
        serde_json::from_value(json!({"instance": "i1", "panels": [panel]})).unwrap();
    let mut host = PanelHost::default();
    assert!(host.register(M, "Example", None, &params).is_empty());
    let r = frame(&ctx, &mut host, vec![]);
    assert!(!r.panicked);
    for id in [
        "l1", "v1", "b1", "t1", "sl1", "ta1", "md1", "tx1", "cv1", "cv1/sh1",
    ] {
        assert!(r.rects.contains_key(id), "{id} drawn");
    }
}
