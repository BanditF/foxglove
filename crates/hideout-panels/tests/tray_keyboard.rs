//! The keyboard inside the tray panel (#991, docs/hideout-plugin-modules-ux.md "Keyboard and
//! screen reader"), window-free over egui's focus and AccessKit tree: the first tile has the
//! focus when the flyout opens, arrows move it in grid order, Enter opens the page, Space presses
//! the tile's button, Shift+F10 opens the ··· menu and Esc gives the focus back to the tile.
//! Also: a tile's fourth row (a button under a meter) is inside its frame, not clipped.
use std::time::Instant;

use egui::accesskit::{NodeId, Role, TreeUpdate};
use egui::{Event, Key, Modifiers};
use hideout_contract::module::{Context, Module};
use hideout_contract::panel::Register;
use hideout_panels::tray::{arrow_target, focus_first_tile, menu_key, tile_focus_id};
use hideout_panels::{tray_panel_ui, HubCall, PanelHost, PanelKey, TrayAction, TrayLayout};
use serde_json::json;

fn manifest(id: &str) -> Module {
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

[[ui.panels]]
id = "page"
slot = "page"
title = "Title {id}"
icon = "mic"

[[actions]]
id = "toggle"
name = "Toggle it"
"#
    );
    Module::parse(&text, &Context::default()).expect("manifest parses")
}

/// A tile with a value, a sub line, a meter and a button: four rows, like the Focus timer's.
fn register(host: &mut PanelHost, id: &str) {
    let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "tile", "slot": "tile", "title": "T", "icon": "mic", "tree":
            {"type": "column", "id": "tile", "children": [
                {"type": "value", "id": "glance", "props": {"value": "18:42"}},
                {"type": "label", "id": "sub", "props": {"text": "Focus · round 2 of 4"}},
                {"type": "meter", "id": "meter", "props": {"value": 378.0, "max": 1500.0}},
                {"type": "button", "id": "go", "props": {"label": "Pause", "action": "toggle"}}]}},
        {"id": "page", "slot": "page", "title": "T", "icon": "mic", "tree":
            {"type": "label", "id": "l", "props": {"text": "page"}}}]}))
    .unwrap();
    let declared = vec!["tile".to_string(), "page".to_string()];
    let actions = vec!["toggle".to_string()];
    host.set_manifest(manifest(id));
    let rejected = host.register_with(id, id, Some(&declared), Some(&actions), &params);
    assert!(rejected.is_empty(), "{rejected:?}");
}

struct Panel {
    ctx: egui::Context,
    host: PanelHost,
    layout: TrayLayout,
    keys: Vec<PanelKey>,
}

/// What one pass saw.
struct Pass {
    actions: Vec<TrayAction>,
    tree: TreeUpdate,
}

impl Pass {
    fn label(&self, id: NodeId) -> Option<String> {
        self.tree
            .nodes
            .iter()
            .find(|(n, _)| *n == id)
            .and_then(|(_, node)| node.label().map(str::to_string))
    }

    /// The focused node's accessible name.
    fn focus(&self) -> Option<String> {
        self.label(self.tree.focus)
    }

    fn named(&self, role: Role, name: &str) -> bool {
        self.tree
            .nodes
            .iter()
            .any(|(_, n)| n.role() == role && n.label() == Some(name))
    }
}

impl Panel {
    /// Three tiles in a two-column grid: two in the first row, one in the second. `None`
    /// without a system font (the frame can't lay text out).
    fn new() -> Option<Panel> {
        let fonts = hideout_panels::render::test_fonts()?;
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        ctx.enable_accesskit();
        let mut host = PanelHost::default();
        let ids = ["a.one", "b.two", "c.three"];
        for id in ids {
            register(&mut host, id);
        }
        host.set_key("a.one", "toggle", Some("F13".into()));
        let keys = ids.iter().map(|m| PanelKey::new(*m, "tile")).collect();
        Some(Panel {
            ctx,
            host,
            layout: TrayLayout::default(),
            keys,
        })
    }

    fn pass_with(
        &mut self,
        events: Vec<Event>,
        before: impl FnOnce(&egui::Context, &PanelHost, &TrayLayout),
    ) -> Pass {
        let mut before = Some(before);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(560.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let (host, layout) = (&mut self.host, &mut self.layout);
        let mut actions = Vec::new();
        let out = self.ctx.run(input, |ctx| {
            if let Some(b) = before.take() {
                b(ctx, host, layout);
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut editing = false;
                actions = tray_panel_ui(host, ui, layout, &mut editing, Instant::now()).actions;
            });
        });
        Pass {
            actions,
            tree: out
                .platform_output
                .accesskit_update
                .expect("accesskit is on"),
        }
    }

    fn pass(&mut self, events: Vec<Event>) -> Pass {
        self.pass_with(events, |_, _, _| {})
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) -> Pass {
        let ev = |pressed| Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        };
        let p = self.pass(vec![ev(true), ev(false)]);
        // The focus a pass requested shows in the next pass's tree.
        let next = self.pass(Vec::new());
        Pass {
            actions: p.actions,
            tree: next.tree,
        }
    }

    fn focused(&self) -> Option<egui::Id> {
        self.ctx.memory(|m| m.focused())
    }

    fn opens(&mut self) {
        self.pass_with(Vec::new(), |ctx, host, layout| {
            assert!(focus_first_tile(ctx, host, layout));
        });
    }
}

const A: &str = "Title a.one, 18:42, Focus · round 2 of 4";

#[test]
fn arrows_follow_the_grid_order_and_stop_at_the_edges() {
    let (l, r, u, d) = (
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::ArrowUp,
        Key::ArrowDown,
    );
    // Five tiles: rows [0 1] [2 3] [4].
    assert_eq!(arrow_target(0, 5, l), 0);
    assert_eq!(arrow_target(1, 5, r), 2, "Right steps into the next row");
    assert_eq!(arrow_target(2, 5, l), 1, "Left steps into the row before");
    assert_eq!(arrow_target(4, 5, r), 4);
    assert_eq!(arrow_target(1, 5, d), 3);
    assert_eq!(
        arrow_target(3, 5, d),
        4,
        "Down to a short last row: its last tile"
    );
    assert_eq!(arrow_target(4, 5, d), 4);
    assert_eq!(arrow_target(1, 5, u), 1);
    assert_eq!(arrow_target(3, 5, u), 1);
    assert_eq!(arrow_target(0, 0, r), 0);
}

#[test]
fn the_first_tile_has_the_focus_and_its_name_says_title_value_and_sub() {
    let Some(mut p) = Panel::new() else {
        eprintln!("no system font; keyboard test skipped");
        return;
    };
    p.pass(Vec::new());
    p.opens();
    let s = p.pass(Vec::new());
    assert_eq!(p.focused(), Some(tile_focus_id(&p.keys[0])));
    assert_eq!(s.focus().as_deref(), Some(A));
    // The button's name has its key ("Unmute, F13"); a tile with no key bound says the label.
    assert!(
        s.named(Role::Button, "Pause, F13"),
        "{:?}",
        s.tree.nodes.len()
    );
    assert!(s.named(Role::Button, "Pause"));
}

#[test]
fn arrows_enter_space_and_the_menu() {
    let Some(mut p) = Panel::new() else {
        eprintln!("no system font; keyboard test skipped");
        return;
    };
    p.pass(Vec::new());
    p.opens();
    p.pass(Vec::new());
    // Right: the second tile (not the first tile's button, egui's nearest widget).
    let s = p.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(p.focused(), Some(tile_focus_id(&p.keys[1])));
    assert!(s.focus().unwrap().starts_with("Title b.two"));
    // Down from the second: the third, alone in the last row.
    p.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(p.focused(), Some(tile_focus_id(&p.keys[2])));
    p.key(Key::ArrowUp, Modifiers::NONE);
    p.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(p.focused(), Some(tile_focus_id(&p.keys[0])));

    // Space presses the focused tile's button: its action, and no page opens.
    p.host.take_hub_calls();
    let s = p.key(Key::Space, Modifiers::NONE);
    assert!(s.actions.is_empty(), "{:?}", s.actions);
    assert_eq!(
        p.host.take_hub_calls(),
        [HubCall::Invoke {
            module: "a.one".into(),
            action: "toggle".into()
        }]
    );
    assert_eq!(
        p.focused(),
        Some(tile_focus_id(&p.keys[0])),
        "the focus stays"
    );

    // Enter opens the tile's page.
    let s = p.key(Key::Enter, Modifiers::NONE);
    assert_eq!(
        s.actions,
        [TrayAction::Open {
            module: "a.one".into(),
            target: "page".into()
        }]
    );

    // Shift+F10 opens the ··· menu with its first item focused.
    p.key(Key::F10, Modifiers::SHIFT);
    let s = p.pass(Vec::new());
    assert!(egui::Popup::is_any_open(&p.ctx), "the menu is open");
    assert_eq!(s.focus().as_deref(), Some("Open Name a.one"));
    // Esc closes the menu and the focus goes back to its tile.
    p.key(Key::Escape, Modifiers::NONE);
    let s = p.pass(Vec::new());
    assert!(!egui::Popup::is_any_open(&p.ctx), "the menu closed");
    assert_eq!(p.focused(), Some(tile_focus_id(&p.keys[0])));
    assert_eq!(s.focus().as_deref(), Some(A));

    // The Menu key (the shell's WM_CONTEXTMENU) does what Shift+F10 does.
    p.pass_with(Vec::new(), |ctx, _, _| menu_key(ctx));
    let s = p.pass(Vec::new());
    assert!(egui::Popup::is_any_open(&p.ctx));
    assert_eq!(s.focus().as_deref(), Some("Open Name a.one"));
}

#[test]
fn a_fourth_row_is_inside_the_tile_not_clipped() {
    let Some(mut p) = Panel::new() else {
        eprintln!("no system font; keyboard test skipped");
        return;
    };
    for _ in 0..3 {
        p.pass(Vec::new());
    }
    let tile = p
        .ctx
        .read_response(tile_focus_id(&p.keys[0]))
        .expect("the tile was drawn");
    let s = p.pass(Vec::new());
    // The button node's bounds lie inside the tile's.
    let button = s
        .tree
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::Button && n.label() == Some("Pause, F13"))
        .and_then(|(_, n)| n.bounds())
        .expect("the Pause button is in the tree");
    let ppp = p.ctx.pixels_per_point() as f64;
    let r = tile.rect;
    assert!(
        button.y1 <= r.max.y as f64 * ppp + 0.5 && button.y0 >= r.min.y as f64 * ppp - 0.5,
        "Pause {button:?} outside the tile {r:?}"
    );
    // And the tile's clip lets it show: its bottom is above the tile's bottom edge.
    assert!(button.y1 < r.max.y as f64 * ppp);
}
