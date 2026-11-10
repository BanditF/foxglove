// SPDX-License-Identifier: MIT
//! Draws one panel's tree in egui (§5.2). Each panel draws in its own guarded scope: a panic
//! greys that panel ("<Module>'s page couldn't be shown"), not the hub (§5.5). Input becomes
//! [`Interaction`]s that the host turns into `ui.event`. Live streams are stubs until H2.9: a
//! `canvas` draws its declared shapes, `image` and `text` show what they are bound to.

use crate::assets::Assets;
use crate::model::{binding, HostedPanel, Interaction, PanelHost, PanelKey, PanelState};
use crate::view::ViewState;
use egui::{Color32, Rect, RichText, Sense, Stroke, Ui, Vec2};
use hideout_contract::panel::Node;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// What one draw saw: the screen rect of each node (tab buttons as `<tabs>#<child>`, canvas
/// shapes as `<canvas>/<shape>`), for hit tests and window-free tests.
#[derive(Debug, Default)]
pub struct DrawReport {
    pub rects: HashMap<String, Rect>,
    pub panicked: bool,
}

/// Draws the panel `key` into `ui`. Unknown keys draw nothing.
pub fn panel_ui(host: &mut PanelHost, ui: &mut Ui, key: &PanelKey) -> DrawReport {
    let mut report = DrawReport::default();
    let mut out = Vec::new();
    host.breadcrumb = Some(key.clone());
    if let Some((panel, view, facts, assets)) = host.parts(key) {
        let mut d = Draw {
            key,
            view,
            facts,
            assets,
            out: &mut out,
            rects: &mut report.rects,
        };
        let result = catch_unwind(AssertUnwindSafe(|| draw_panel(ui, panel, &mut d)));
        if result.is_err() {
            log::error!("{}/{}: drawing the panel panicked", key.module, key.panel);
            panel.state = PanelState::Broken;
            report.panicked = true;
        }
    }
    host.breadcrumb = None;
    for i in out {
        host.emit(i);
    }
    report
}

struct Draw<'a> {
    key: &'a PanelKey,
    view: &'a mut ViewState,
    facts: &'a HashMap<String, Value>,
    assets: &'a Assets,
    out: &'a mut Vec<Interaction>,
    rects: &'a mut HashMap<String, Rect>,
}

impl Draw<'_> {
    fn emit(&mut self, node: &str, kind: &str, value: Value, optimistic: bool) {
        self.out.push(Interaction {
            key: self.key.clone(),
            node: node.to_string(),
            kind: kind.to_string(),
            value,
            optimistic,
        });
    }

    /// The node's props with its binding and any optimistic value filled in.
    fn props(&self, n: &Node) -> Map<String, Value> {
        let mut p = n.props.clone();
        if let Some(v) = binding(n).and_then(|f| self.facts.get(f)) {
            p.insert("value".into(), v.clone());
        }
        if let Some(v) = self.view.optimistic.get(&n.id) {
            p.insert("value".into(), v.clone());
        }
        p
    }

    fn salt(&self, n: &Node) -> egui::Id {
        egui::Id::new(("hideout.panel", &self.key.module, &self.key.panel, &n.id))
    }

    /// A scroll area keyed by node id whose offset lives in the view state (§5.6).
    fn scrolled(
        &mut self,
        ui: &mut Ui,
        n: &Node,
        max_h: f32,
        body: impl FnOnce(&mut Ui, &mut Self),
    ) {
        let mut area = egui::ScrollArea::vertical()
            .id_salt(self.salt(n))
            .max_height(max_h)
            .auto_shrink([false, true]);
        if self.view.restore {
            if let Some([x, y]) = self.view.scroll.get(&n.id) {
                area = area.scroll_offset(Vec2::new(*x, *y));
            }
        }
        let output = area.show(ui, |ui| body(ui, self));
        let o = output.state.offset;
        self.view.scroll.insert(n.id.clone(), [o.x, o.y]);
    }
}

fn s<'a>(p: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}

fn f(p: &Map<String, Value>, k: &str) -> Option<f64> {
    p.get(k).and_then(Value::as_f64)
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "\u{2014}".into(),
        other => other.to_string(),
    }
}

fn draw_panel(ui: &mut Ui, panel: &mut HostedPanel, d: &mut Draw) {
    let banner = match &panel.state {
        PanelState::Live => None,
        PanelState::Greyed(state) => Some(format!("{} is {state}.", panel.module_name)),
        PanelState::Rejected(id) => Some(format!(
            "{}'s page couldn't be shown ({id}).",
            panel.module_name
        )),
        PanelState::Broken | PanelState::Quarantined => {
            Some(format!("{}'s page couldn't be shown.", panel.module_name))
        }
    };
    if let Some(b) = &banner {
        ui.label(RichText::new(b).color(ui.visuals().warn_fg_color));
    }
    if let Some(n) = &panel.notice {
        ui.label(RichText::new(n).color(ui.visuals().error_fg_color));
    }
    // Broken and quarantined panels draw nothing more: drawing is what failed.
    if matches!(panel.state, PanelState::Broken | PanelState::Quarantined) {
        return;
    }
    let tree = &panel.tree;
    let live = panel.state == PanelState::Live;
    ui.add_enabled_ui(live, |ui| {
        d.scrolled(ui, tree, f32::INFINITY, |ui, d| node(ui, tree, d));
    });
    d.view.restore = false;
}

fn node(ui: &mut Ui, n: &Node, d: &mut Draw) {
    let r = ui.scope(|ui| widget(ui, n, d)).response;
    d.rects.entry(n.id.clone()).or_insert(r.rect);
}

fn children(ui: &mut Ui, n: &Node, d: &mut Draw) {
    for c in &n.children {
        node(ui, c, d);
    }
}

fn widget(ui: &mut Ui, n: &Node, d: &mut Draw) {
    let p = d.props(n);
    let label = s(&p, "label").unwrap_or_default().to_string();
    match n.kind.as_str() {
        "column" => {
            ui.vertical(|ui| children(ui, n, d));
        }
        "row" => {
            ui.horizontal_wrapped(|ui| children(ui, n, d));
        }
        "grid" => {
            let cols = f(&p, "columns").unwrap_or(2.0).clamp(1.0, 64.0) as usize;
            egui::Grid::new(d.salt(n)).show(ui, |ui| {
                for (i, c) in n.children.iter().enumerate() {
                    node(ui, c, d);
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        }
        "section" => {
            let title = s(&p, "title").unwrap_or(&label).to_string();
            let default = p.get("open").and_then(Value::as_bool).unwrap_or(true);
            let open = *d.view.open.get(&n.id).unwrap_or(&default);
            let r = egui::CollapsingHeader::new(title)
                .id_salt(d.salt(n))
                .open(Some(open))
                .show(ui, |ui| children(ui, n, d));
            if r.header_response.clicked() {
                d.view.open.insert(n.id.clone(), !open);
            }
        }
        "tabs" => tabs(ui, n, d),
        "label" => {
            ui.label(text_of(
                p.get("value").or(p.get("text")).unwrap_or(&Value::Null),
            ));
        }
        "value" => {
            ui.horizontal(|ui| {
                if !label.is_empty() {
                    ui.label(RichText::new(&label).weak());
                }
                ui.label(RichText::new(text_of(p.get("value").unwrap_or(&Value::Null))).strong());
            });
        }
        "button" => {
            if ui.button(&label).clicked() {
                d.emit(&n.id, "click", Value::Null, false);
            }
        }
        "toggle" => {
            let mut on = p.get("value").and_then(Value::as_bool).unwrap_or(false);
            if ui.checkbox(&mut on, &label).changed() {
                d.emit(&n.id, "change", Value::Bool(on), true);
            }
        }
        "slider" => {
            let (lo, hi) = (f(&p, "min").unwrap_or(0.0), f(&p, "max").unwrap_or(1.0));
            let mut v = f(&p, "value").unwrap_or(lo);
            let r = ui.add(egui::Slider::new(&mut v, lo..=hi.max(lo)).text(&label));
            if r.changed() {
                let value = serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number);
                d.emit(&n.id, "change", value, true);
            }
        }
        "choice" => {
            let options: Vec<String> = p
                .get("options")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(text_of).collect())
                .unwrap_or_default();
            let current = s(&p, "value").unwrap_or_default().to_string();
            egui::ComboBox::from_id_salt(d.salt(n))
                .selected_text(&current)
                .show_ui(ui, |ui| {
                    for o in &options {
                        if ui.selectable_label(o == &current, o).clicked() && o != &current {
                            d.emit(&n.id, "change", Value::String(o.clone()), true);
                        }
                    }
                });
        }
        "color" => {
            let hex = s(&p, "value").unwrap_or("#000000");
            let mut c = Color32::from_hex(hex).unwrap_or(Color32::BLACK);
            if ui.color_edit_button_srgba(&mut c).changed() {
                let v = format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b());
                d.emit(&n.id, "change", Value::String(v), true);
            }
        }
        "text_input" => text_input(ui, n, &p, d),
        "list" => {
            let items: Vec<String> = p
                .get("items")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(text_of).collect())
                .unwrap_or_default();
            let selected = f(&p, "value").map(|v| v as usize);
            d.scrolled(ui, n, 240.0, |ui, d| {
                for (i, it) in items.iter().enumerate() {
                    if ui.selectable_label(selected == Some(i), it).clicked() {
                        d.emit(&n.id, "select", Value::from(i), true);
                    }
                }
            });
        }
        "table" => {
            let cols: Vec<String> = p
                .get("columns")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(text_of).collect())
                .unwrap_or_default();
            let rows = p
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let salt = d.salt(n);
            d.scrolled(ui, n, 320.0, |ui, _| {
                egui::Grid::new(salt).striped(true).show(ui, |ui| {
                    for c in &cols {
                        ui.label(RichText::new(c).strong());
                    }
                    ui.end_row();
                    for row in &rows {
                        for cell in row.as_array().into_iter().flatten() {
                            ui.label(text_of(cell));
                        }
                        ui.end_row();
                    }
                });
            });
        }
        "meter" => {
            let max = f(&p, "max").unwrap_or(1.0).max(f64::EPSILON);
            let v = (f(&p, "value").unwrap_or(0.0) / max).clamp(0.0, 1.0) as f32;
            let mut bar = egui::ProgressBar::new(v);
            if !label.is_empty() {
                bar = bar.text(&label);
            }
            ui.add(bar);
        }
        "image" | "gallery" => {
            let hashes: Vec<String> = match n.kind.as_str() {
                "image" => s(&p, "hash").map(str::to_string).into_iter().collect(),
                _ => p
                    .get("hashes")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|h| h.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            // Stub until H2.9: the image's place, and whether its asset arrived.
            ui.horizontal_wrapped(|ui| {
                if n.stream_name().is_some() {
                    ui.label(RichText::new("Live image").weak());
                }
                for h in &hashes {
                    let have = d.assets.get(h).is_some();
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
                    ui.painter().rect_stroke(
                        r,
                        4.0,
                        Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
                        egui::StrokeKind::Inside,
                    );
                    if !have {
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            "\u{2026}",
                            egui::FontId::proportional(14.0),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
            });
        }
        "log" => {
            let lines: Vec<String> = p
                .get("lines")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(text_of).collect())
                .unwrap_or_default();
            d.scrolled(ui, n, 240.0, |ui, _| {
                for l in &lines {
                    ui.label(RichText::new(l).monospace());
                }
            });
        }
        "markdown" => markdown(ui, s(&p, "text").unwrap_or_default()),
        "text" => {
            // Stub until H2.9: appended text arrives on the stream.
            ui.label(s(&p, "text").unwrap_or_default());
            if let Some(stream) = n.stream_name() {
                ui.label(RichText::new(format!("Live text ({stream})")).weak());
            }
        }
        "canvas" => canvas(ui, n, &p, d),
        #[cfg(test)]
        "test.panic" => panic!("test panic in a panel"),
        _ => {
            ui.label(RichText::new("Needs a newer Hideout").color(ui.visuals().warn_fg_color));
        }
    }
}

fn tabs(ui: &mut Ui, n: &Node, d: &mut Draw) {
    if n.children.is_empty() {
        return;
    }
    let chosen = d
        .view
        .tab
        .get(&n.id)
        .and_then(|id| n.children.iter().position(|c| &c.id == id))
        .unwrap_or(0);
    let mut pick = chosen;
    ui.horizontal(|ui| {
        for (i, c) in n.children.iter().enumerate() {
            let name = s(&c.props, "title")
                .or(s(&c.props, "label"))
                .unwrap_or(&c.id)
                .to_string();
            let r = ui.selectable_label(i == chosen, name);
            d.rects.insert(format!("{}#{}", n.id, c.id), r.rect);
            if r.clicked() {
                pick = i;
            }
        }
    });
    if pick != chosen || !d.view.tab.contains_key(&n.id) {
        d.view.tab.insert(n.id.clone(), n.children[pick].id.clone());
    }
    ui.separator();
    node(ui, &n.children[pick], d);
}

fn text_input(ui: &mut Ui, n: &Node, p: &Map<String, Value>, d: &mut Draw) {
    let multiline = p.get("multiline").and_then(Value::as_bool).unwrap_or(false);
    let salt = d.salt(n);
    let text = d
        .view
        .text
        .entry(n.id.clone())
        .or_insert_with(|| s(p, "value").unwrap_or_default().to_string());
    let mut edit = if multiline {
        egui::TextEdit::multiline(text)
    } else {
        egui::TextEdit::singleline(text)
    };
    if let Some(h) = s(p, "hint") {
        edit = edit.hint_text(h);
    }
    let r = ui.add(edit.id_salt(salt).desired_width(f32::INFINITY));
    // Enter sends a single line; Ctrl+Enter sends a multi-line one.
    let send = if multiline {
        r.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
    } else {
        r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
    };
    if send {
        let value = std::mem::take(d.view.text.get_mut(&n.id).expect("inserted above"));
        d.emit(&n.id, "submit", Value::String(value), false);
    }
}

/// The safe markdown subset (§5.2): headings, bullets, code blocks and bold lines; no HTML,
/// no links, no images. Everything is drawn as text.
fn markdown(ui: &mut Ui, text: &str) {
    let mut code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        if code {
            ui.label(RichText::new(line).monospace());
        } else if let Some(h) = line.strip_prefix("# ") {
            ui.label(RichText::new(h).heading());
        } else if let Some(h) = line.strip_prefix("## ").or(line.strip_prefix("### ")) {
            ui.label(RichText::new(h).strong().size(16.0));
        } else if let Some(b) = line.strip_prefix("- ").or(line.strip_prefix("* ")) {
            ui.label(format!("\u{2022} {}", b.replace("**", "")));
        } else if line.len() > 4 && line.starts_with("**") && line.ends_with("**") {
            ui.label(RichText::new(&line[2..line.len() - 2]).strong());
        } else {
            ui.label(line.replace("**", ""));
        }
    }
}

/// A retained set of shapes (§5.2): drawn from their props, a click returns the shape id.
/// Colours from a stream are H2.9.
fn canvas(ui: &mut Ui, n: &Node, p: &Map<String, Value>, d: &mut Draw) {
    let w = f(p, "width").unwrap_or(400.0).clamp(1.0, 4096.0) as f32;
    let h = f(p, "height").unwrap_or(200.0).clamp(1.0, 4096.0) as f32;
    let (area, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let painter = ui.painter_at(area);
    let fg = ui.visuals().text_color();
    let mut hit = None;
    for sh in &n.children {
        let sp = &sh.props;
        let (x, y) = (
            f(sp, "x").unwrap_or(0.0) as f32,
            f(sp, "y").unwrap_or(0.0) as f32,
        );
        let (sw, shh) = (
            f(sp, "w").unwrap_or(10.0) as f32,
            f(sp, "h").unwrap_or(10.0) as f32,
        );
        let rect = Rect::from_min_size(area.min + Vec2::new(x, y), Vec2::new(sw, shh));
        let color = s(sp, "color")
            .and_then(|c| Color32::from_hex(c).ok())
            .unwrap_or(ui.visuals().widgets.inactive.bg_fill);
        match sh.kind.as_str() {
            "rect" => {
                painter.rect_filled(rect, 0.0, color);
            }
            "key" => {
                painter.rect_filled(rect, 4.0, color);
            }
            "circle" => {
                let r = f(sp, "r").map_or(sw.min(shh) / 2.0, |r| r as f32);
                painter.circle_filled(area.min + Vec2::new(x, y), r, color);
            }
            "text" => {
                painter.text(
                    rect.min,
                    egui::Align2::LEFT_TOP,
                    s(sp, "text").unwrap_or_default(),
                    egui::FontId::proportional(f(sp, "size").unwrap_or(13.0) as f32),
                    fg,
                );
            }
            _ => {}
        }
        d.rects.insert(format!("{}/{}", n.id, sh.id), rect);
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.clicked() && rect.contains(pos) {
                hit = Some(sh.id.clone());
            }
        }
    }
    if let Some(id) = hit {
        d.emit(&n.id, "click", Value::String(id), false);
    }
}

/// A system font for window-free render tests (the crate ships no fonts). `None` when the
/// machine has none of the usual ones.
#[doc(hidden)]
pub fn test_fonts() -> Option<egui::FontDefinitions> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let candidates = [
        format!("{windir}\\Fonts\\segoeui.ttf"),
        format!("{windir}\\Fonts\\arial.ttf"),
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into(),
        "/usr/share/fonts/TTF/DejaVuSans.ttf".into(),
        "/System/Library/Fonts/Supplemental/Arial.ttf".into(),
    ];
    let bytes = candidates.iter().find_map(|p| std::fs::read(p).ok())?;
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        "test".into(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.insert(fam, vec!["test".into()]);
    }
    Some(fonts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_contract::panel::Register;
    use serde_json::json;

    #[test]
    fn a_panicking_panel_greys_itself_not_the_hub() {
        let Some(fonts) = crate::render::test_fonts() else {
            eprintln!("no system font; render test skipped");
            return;
        };
        let mut host = PanelHost::default();
        let tree = json!({"type": "column", "id": "root", "children": [
            {"type": "test.panic", "id": "boom"}]});
        let params: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
            {"id": "bad", "slot": "page", "title": "Bad", "icon": "x", "tree": tree},
            {"id": "good", "slot": "page", "title": "Good", "icon": "x",
             "tree": {"type": "label", "id": "l", "props": {"text": "ok"}}}]}))
        .unwrap();
        assert!(host.register("m", "Example", None, &params).is_empty());
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let (bad, good) = (PanelKey::new("m", "bad"), PanelKey::new("m", "good"));
        let mut reports = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                reports.push(panel_ui(&mut host, ui, &bad));
                reports.push(panel_ui(&mut host, ui, &good));
            });
        });
        assert!(reports[0].panicked && !reports[1].panicked);
        assert_eq!(host.panel(&bad).unwrap().state, PanelState::Broken);
        assert!(host.panel(&good).unwrap().live());
        assert!(host.breadcrumb().is_none());
    }
}
