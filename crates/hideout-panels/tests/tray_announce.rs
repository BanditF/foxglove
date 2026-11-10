//! The tray panel's polite announcements (#991, docs/hideout-plugin-modules-ux.md "Keyboard and
//! screen reader"), window-free: a tile's phase change (its glance word or tone, or the frame's
//! state) is said once, at most every 5 s per tile; a value that ticks ("18:42") never is; what a
//! tile shows when the flyout opens is only remembered. The live node itself is the shell's
//! (`crates/hideout-shell`, the flyout's AccessKit tree).
use std::time::{Duration, Instant};

use hideout_contract::messages::UiPatch;
use hideout_contract::panel::Register;
use hideout_panels::tray::ANNOUNCE_EVERY;
use hideout_panels::{tray_panel_ui, PanelHost, TileAnnouncer, TrayLayout};
use serde_json::{json, Value};

struct Panel {
    ctx: egui::Context,
    host: PanelHost,
    layout: TrayLayout,
    announcer: TileAnnouncer,
    t0: Instant,
}

impl Panel {
    /// A tile `m/tile` titled "Timer" with `rows`. `None` without a system font.
    fn new(rows: Value) -> Option<Panel> {
        let fonts = hideout_panels::render::test_fonts()?;
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let mut host = PanelHost::default();
        let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
            {"id": "tile", "slot": "tile", "title": "Timer", "icon": "timer", "tree":
                {"type": "column", "id": "tile", "children": rows}}]}))
        .unwrap();
        assert!(host.register("m", "M", None, &params).is_empty());
        Some(Panel {
            ctx,
            host,
            layout: TrayLayout::default(),
            announcer: TileAnnouncer::default(),
            t0: Instant::now(),
        })
    }

    /// Sets `node`'s props at `at` seconds.
    fn set(&mut self, at: f32, node: &str, props: Value) {
        let patch: UiPatch = serde_json::from_value(json!({"panel": "tile", "ops": [
            {"op": "set", "node": node, "props": props}]}))
        .unwrap();
        self.host
            .patch("m", &patch, self.t0 + Duration::from_secs_f32(at))
            .unwrap();
    }

    /// One draw at `at` seconds: what is said.
    fn pass(&mut self, at: f32) -> Option<String> {
        let now = self.t0 + Duration::from_secs_f32(at);
        let (host, layout) = (&mut self.host, &mut self.layout);
        let mut phases = Vec::new();
        let _ = self.ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut editing = false;
                phases = tray_panel_ui(host, ui, layout, &mut editing, now).phases;
            });
        });
        self.announcer.see(&phases, now)
    }
}

fn timer() -> Value {
    json!([
        {"type": "value", "id": "glance", "props": {"value": "18:42", "tone": "active"}},
        {"type": "label", "id": "sub", "props": {"text": "Focus \u{00B7} round 2 of 4"}},
        {"type": "meter", "id": "meter", "props": {"value": 378.0, "max": 1500.0}}])
}

fn mic() -> Value {
    json!([
        {"type": "value", "id": "glance", "props": {"value": "Mic on", "tone": "neutral"}},
        {"type": "label", "id": "sub", "props": {"text": "Default communications mic"}}])
}

#[test]
fn a_glance_word_change_is_said_once() {
    let Some(mut p) = Panel::new(mic()) else {
        eprintln!("no system font; announcement test skipped");
        return;
    };
    assert_eq!(p.pass(0.0), None, "what shows on opening is not said");
    p.set(10.0, "glance", json!({"value": "Muted", "tone": "live"}));
    assert_eq!(p.pass(10.0).as_deref(), Some("Timer, Muted"));
    assert_eq!(p.pass(10.1), None, "said once");
    assert_eq!(p.pass(30.0), None, "and not again later");
}

#[test]
fn a_value_tick_is_never_said_and_a_tone_change_says_the_phase() {
    let Some(mut p) = Panel::new(timer()) else {
        eprintln!("no system font; announcement test skipped");
        return;
    };
    assert_eq!(p.pass(0.0), None);
    for s in 1..8 {
        p.set(
            s as f32,
            "glance",
            json!({"value": format!("18:{:02}", 42 - s)}),
        );
        p.set(s as f32 + 0.5, "meter", json!({"value": 378.0 + s as f64}));
        assert_eq!(p.pass(s as f32 + 0.5), None, "tick {s} was said");
    }
    // Focus to Rest: the tone changes (and the sub line, which only gives the words).
    p.set(20.0, "glance", json!({"value": "5:00", "tone": "ok"}));
    p.set(20.5, "sub", json!({"text": "Rest \u{00B7} then round 3"}));
    assert_eq!(p.pass(20.5).as_deref(), Some("Timer, Rest"));
    // The sub line alone is no phase change.
    p.set(30.0, "sub", json!({"text": "Rest \u{00B7} then round 4"}));
    assert_eq!(p.pass(30.0), None);
}

#[test]
fn two_changes_within_5_s_are_said_once_the_later_after_the_window() {
    let Some(mut p) = Panel::new(mic()) else {
        eprintln!("no system font; announcement test skipped");
        return;
    };
    assert_eq!(p.pass(0.0), None);
    p.set(10.0, "glance", json!({"value": "Muted", "tone": "live"}));
    assert_eq!(p.pass(10.0).as_deref(), Some("Timer, Muted"));
    p.set(
        12.0,
        "glance",
        json!({"value": "Mic on", "tone": "neutral"}),
    );
    assert_eq!(p.pass(12.0), None, "inside the 5 s window");
    assert_eq!(p.pass(14.9), None, "still inside");
    let after = 10.0 + ANNOUNCE_EVERY.as_secs_f32() + 0.1;
    assert_eq!(
        p.pass(after).as_deref(),
        Some("Timer, Mic on"),
        "the later one"
    );
    assert_eq!(p.pass(after + 0.1), None);
    // A change undone inside the window is never said.
    p.set(16.0, "glance", json!({"value": "Muted", "tone": "live"}));
    p.set(
        17.0,
        "glance",
        json!({"value": "Mic on", "tone": "neutral"}),
    );
    assert_eq!(p.pass(17.0), None);
    assert_eq!(p.pass(after + 6.0), None);
}

#[test]
fn a_tile_not_drawn_starts_again_from_its_next_sight() {
    let Some(mut p) = Panel::new(mic()) else {
        eprintln!("no system font; announcement test skipped");
        return;
    };
    assert_eq!(p.pass(0.0), None);
    p.layout.hidden.insert("m/tile".into());
    assert_eq!(p.pass(1.0), None);
    p.set(2.0, "glance", json!({"value": "Muted", "tone": "live"}));
    p.layout.hidden.clear();
    assert_eq!(
        p.pass(3.0),
        None,
        "a hidden tile's change is not said when it shows"
    );
}
