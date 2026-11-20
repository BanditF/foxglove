//! Every control a screen reader can focus has a name (#1376, slice 2): each page of the
//! settings window drawn headless with AccessKit on, and every focusable node of its tree
//! checked for a non-empty name made of words (no private-use icon glyph).

use egui::accesskit::{Action, Node, NodeId, Role};

use super::pill_groups::page_tree;
use super::Samples;
use crate::foxglove::shell::Page;

/// True for a character from the private-use area, where the icon font keeps its glyphs.
fn is_icon_glyph(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

/// Why `n` has no usable name, if it hasn't one. A name is the node's label, or for a field the
/// value it holds counts too only when it also has a label.
fn nameless(n: &Node, all: &std::collections::HashMap<NodeId, &Node>) -> Option<String> {
    // A name can come from the node's own label or from the label it is `labelled_by`.
    let by = n
        .labelled_by()
        .iter()
        .filter_map(|l| all.get(l))
        .find_map(|l| l.value().or_else(|| l.label()));
    let name = n.label().or(by).unwrap_or_default();
    if name.trim().is_empty() {
        return Some("no name".into());
    }
    if name.chars().any(is_icon_glyph) {
        return Some(format!("a glyph in its name {name:?}"));
    }
    // UTF-8 read as Windows-1252 (the "A-circumflex" and "a-circumflex euro" pairs) means a
    // wrongly encoded string at the source.
    if name.contains('\u{c2}') || name.contains("\u{e2}\u{20ac}") {
        return Some(format!("mojibake in its name {name:?}"));
    }
    None
}

/// Whether the reader can put focus on `n`: the roles that are controls, or anything that
/// takes the focus action.
fn focusable(n: &Node) -> bool {
    n.supports_action(Action::Focus)
        && !matches!(
            n.role(),
            Role::Window | Role::GenericContainer | Role::Label
        )
}

/// Every nameless focusable node of one page's tree, as `role (bounds)`.
fn gaps(page: Page, samples: &Samples, height: f32) -> Vec<String> {
    let t = page_tree(page, samples, height);
    let all: std::collections::HashMap<NodeId, &Node> =
        t.nodes.iter().map(|(i, n)| (*i, n)).collect();
    t.nodes
        .iter()
        .filter(|(_, n)| focusable(n))
        .filter_map(|(_, n)| {
            nameless(n, &all).map(|why| {
                // The text drawn nearest before it, to find the widget in the source.
                let near = near_text(n, &all);
                let at = n
                    .bounds()
                    .map(|b| format!("{:.0},{:.0} {:.0}x{:.0}", b.x0, b.y0, b.width(), b.height()))
                    .unwrap_or_default();
                format!("{page:?}: {:?} ({at}) near {near:?}: {why}", n.role())
            })
        })
        .collect()
}

#[test]
fn every_focusable_control_has_a_name() {
    let mut all = Vec::new();
    for page in Page::ALL {
        for samples in [Samples::full(), Samples::none()] {
            all.extend(gaps(page, &samples, 1400.0));
        }
    }
    all.sort();
    all.dedup();
    assert!(
        all.is_empty(),
        "controls without a name:\n{}",
        all.join("\n")
    );
}

/// A swatch row's "Colour" radio group sits in its card, not at the end of the window, and its
/// swatches are its children.
#[test]
fn a_swatch_group_is_in_its_row_with_its_swatches() {
    let t = super::pill_groups::page_tree(Page::Lights, &Samples::full(), 1400.0);
    let root = egui::accesskit_root_id().accesskit_id();
    let group = t
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::RadioGroup && n.label() == Some("Colour"))
        .expect("a colour group on the Lights page");
    let parents = |id: NodeId| {
        t.nodes
            .iter()
            .filter(|(_, n)| n.children().contains(&id))
            .count()
    };
    assert_eq!(parents(group.0), 1, "one parent");
    let under_root = t
        .nodes
        .iter()
        .any(|(id, n)| *id == root && n.children().contains(&group.0));
    assert!(!under_root, "the group is not a direct child of the window");
    assert!(group.1.children().len() >= 2, "the swatches are in it");
}

/// The text of the label drawn closest to the left of or above `n`, for the failure message.
fn near_text(n: &Node, all: &std::collections::HashMap<NodeId, &Node>) -> String {
    let Some(b) = n.bounds() else {
        return String::new();
    };
    all.values()
        .filter(|l| l.role() == Role::Label)
        .filter_map(|l| Some((l, l.bounds()?, l.value()?)))
        .filter(|(_, lb, _)| lb.y0 <= b.y1 && lb.y1 >= b.y0 - 30.0 && lb.x0 <= b.x0 + 1.0)
        .min_by(|a, c| {
            let d = |r: &egui::accesskit::Rect| (b.x0 - r.x1).abs() + (b.y0 - r.y0).abs() * 3.0;
            d(&a.1).total_cmp(&d(&c.1))
        })
        .map(|(_, _, v)| v.to_owned())
        .unwrap_or_default()
}

#[test]
fn a_control_after_its_label_takes_it_as_its_name() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut v = 3.0_f64;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| {
        egui::Grid::new("g").show(ui, |ui| {
            ui.label("Opacity");
            ui.add(egui::DragValue::new(&mut v));
            ui.end_row();
        });
        crate::foxglove::theme::name_controls(ui.ctx());
    });
    out.textures_delta.clear();
    let t = out.platform_output.accesskit_update.expect("tree");
    let all: std::collections::HashMap<NodeId, &Node> =
        t.nodes.iter().map(|(i, n)| (*i, n)).collect();
    let spin = t
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::SpinButton)
        .expect("a drag value")
        .1
        .clone();
    assert!(nameless(&spin, &all).is_none());
}
