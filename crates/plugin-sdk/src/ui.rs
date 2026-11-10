//! Hideout panels for a plugin (contract §4.10, §5): a tray tile and a page, sent with
//! `ui.register` and kept current with `ui.patch` through the hub's stdio bridge.
//!
//! [`Panels::show`] takes the whole set of panels every time; it registers them the first time
//! (or when a tree's shape changes) and otherwise sends only the props that changed. Foxglove
//! has no panels and refuses `ui.register`; after that refusal nothing more is sent, so the same
//! plugin runs under both hosts.
//!
//! The builders draw the tile body the contract expects (§5.5, tile caps): one `value` with a
//! `tone`, at most one `label`, one `meter` and one `button`.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::Plugin;

/// A message from Hideout for the plugin's panels: a `ui.event` call (with `id`, answer it with
/// [`Plugin::reply`]) or a `ui.visibility` notification.
#[derive(Clone, Debug, PartialEq)]
pub struct UiIn {
    pub method: String,
    pub params: Value,
    pub id: Option<Value>,
}

impl UiIn {
    /// The panel it is about.
    pub fn panel(&self) -> &str {
        self.params
            .get("panel")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// For `ui.event`: the node that was used.
    pub fn node(&self) -> &str {
        self.params
            .get("node")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// For `ui.visibility`: whether the panel is on screen now.
    pub fn visible(&self) -> Option<bool> {
        (self.method == "ui.visibility")
            .then(|| self.params.get("state") == Some(&json!("visible")))
    }
}

/// What a tile's button does (contract §5.2, v1.4).
#[derive(Clone, Debug, PartialEq)]
pub enum Press {
    /// A declared `[[actions]]` id: the hub sends it like a key and shows the key chip.
    Action(&'static str),
    /// `page`, `page#<node>` or `settings`: the hub opens it without a round trip.
    Opens(&'static str),
    /// A `ui.event` click on this node id.
    Event(&'static str),
}

/// A button: its words and what it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub label: String,
    pub press: Press,
}

impl Button {
    pub fn new(label: impl Into<String>, press: Press) -> Button {
        Button {
            label: label.into(),
            press,
        }
    }
}

/// A tile body: the glance, its tone, one sub line, an optional meter and one button.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    pub value: String,
    pub tone: &'static str,
    pub sub: Option<String>,
    /// `(value, max)`.
    pub meter: Option<(f64, f64)>,
    pub button: Option<Button>,
}

/// A `value` node.
pub fn value(id: &str, text: &str, tone: &str) -> Value {
    json!({"type": "value", "id": id, "props": {"value": text, "tone": tone}})
}

/// A `label` node.
pub fn label(id: &str, text: &str) -> Value {
    json!({"type": "label", "id": id, "props": {"text": text}})
}

/// A `markdown` node.
pub fn markdown(id: &str, text: &str) -> Value {
    json!({"type": "markdown", "id": id, "props": {"text": text}})
}

/// A `button` node.
pub fn button(id: &str, b: &Button) -> Value {
    let mut props = Map::new();
    props.insert("label".into(), json!(b.label));
    let id = match &b.press {
        Press::Action(a) => {
            props.insert("action".into(), json!(a));
            id
        }
        Press::Opens(o) => {
            props.insert("opens".into(), json!(o));
            id
        }
        Press::Event(node) => node,
    };
    json!({"type": "button", "id": id, "props": props})
}

/// A `section` node with a title.
pub fn section(id: &str, title: &str, children: Vec<Value>) -> Value {
    json!({"type": "section", "id": id, "props": {"title": title}, "children": children})
}

/// A `column` node.
pub fn column(id: &str, children: Vec<Value>) -> Value {
    json!({"type": "column", "id": id, "children": children})
}

impl Tile {
    /// The tile's tree: a `column` with the glance first.
    pub fn tree(&self) -> Value {
        let mut kids = vec![value("glance", &self.value, self.tone)];
        if let Some(sub) = &self.sub {
            kids.push(label("sub", sub));
        }
        if let Some((v, max)) = self.meter {
            kids.push(json!({"type": "meter", "id": "meter", "props": {"value": v, "max": max}}));
        }
        if let Some(b) = &self.button {
            kids.push(button("go", b));
        }
        column("tile", kids)
    }
}

/// One panel for `ui.register`.
pub fn panel(id: &str, slot: &str, title: &str, icon: &str, tree: Value) -> Value {
    json!({"id": id, "slot": slot, "title": title, "icon": icon, "tree": tree})
}

/// The page's "Now" section: the tile's glance at page size, its sub line and its button, plus
/// `more` (at most one more button, contract UX plan).
pub fn now_section(tile: &Tile, more: Vec<Value>) -> Value {
    let mut kids = vec![value("now-value", &tile.value, tile.tone)];
    if let Some(sub) = &tile.sub {
        kids.push(label("now-sub", sub));
    }
    if let Some(b) = &tile.button {
        kids.push(button("now-go", b));
    }
    kids.extend(more);
    section("now", "Now", kids)
}

/// Every node in `tree`, depth first: (id, type, depth, props).
fn flat(tree: &Value) -> Vec<(String, String, usize, Value)> {
    let mut out = Vec::new();
    let mut stack = vec![(tree, 0usize)];
    while let Some((n, d)) = stack.pop() {
        out.push((
            n["id"].as_str().unwrap_or_default().to_string(),
            n["type"].as_str().unwrap_or_default().to_string(),
            d,
            n.get("props").cloned().unwrap_or(json!({})),
        ));
        if let Some(kids) = n.get("children").and_then(Value::as_array) {
            stack.extend(kids.iter().rev().map(|c| (c, d + 1)));
        }
    }
    out
}

/// The `ui.patch` ops that turn `old` into `new`, or `None` when their shape differs (then the
/// panels register again).
pub fn diff(old: &Value, new: &Value) -> Option<Vec<Value>> {
    let (a, b) = (flat(old), flat(new));
    if a.len() != b.len()
        || a.iter()
            .zip(&b)
            .any(|(x, y)| (&x.0, &x.1, x.2) != (&y.0, &y.1, y.2))
    {
        return None;
    }
    let mut ops = Vec::new();
    for (x, y) in a.iter().zip(&b) {
        if x.3 == y.3 {
            continue;
        }
        let mut props = y.3.as_object().cloned().unwrap_or_default();
        for k in x.3.as_object().into_iter().flat_map(|o| o.keys()) {
            props.entry(k.clone()).or_insert(Value::Null);
        }
        ops.push(json!({"op": "set", "node": y.0, "props": props}));
    }
    Some(ops)
}

/// The plugin's panels in Hideout.
pub struct Panels {
    plugin: Arc<Plugin>,
    instance: String,
    shown: Option<Vec<Value>>,
    /// Foxglove refused `ui.register`: no panels here.
    off: bool,
}

impl Panels {
    pub fn new(plugin: Arc<Plugin>) -> Panels {
        Panels {
            plugin,
            instance: format!("pid-{}", std::process::id()),
            shown: None,
            off: false,
        }
    }

    /// Whether the host takes panels (false once Foxglove refused them).
    pub fn on(&self) -> bool {
        !self.off
    }

    /// Shows `panels` (each from [`panel`]): registers when needed, patches otherwise.
    pub fn show(&mut self, panels: Vec<Value>) {
        if self.off || self.shown.as_ref() == Some(&panels) {
            return;
        }
        let patches = self.shown.as_ref().and_then(|old| {
            if old.len() != panels.len() {
                return None;
            }
            let mut all = Vec::new();
            for (o, n) in old.iter().zip(&panels) {
                if (&o["id"], &o["title"], &o["icon"]) != (&n["id"], &n["title"], &n["icon"]) {
                    return None;
                }
                let ops = diff(&o["tree"], &n["tree"])?;
                if !ops.is_empty() {
                    all.push(json!({"panel": n["id"], "ops": ops}));
                }
            }
            Some(all)
        });
        match patches {
            Some(all) => {
                for p in all {
                    if let Err(e) = self.plugin.notify("ui.patch", p) {
                        eprintln!("ui.patch: {e}");
                    }
                }
            }
            None => {
                let params = json!({"instance": self.instance, "panels": panels});
                match self.plugin.call("ui.register", params) {
                    Ok(r) => {
                        if let Some(rej) = r.get("rejected").and_then(Value::as_array) {
                            for x in rej {
                                eprintln!("ui.register: panel rejected: {x}");
                            }
                        }
                    }
                    Err(e) => {
                        // Foxglove has no panels; nothing more to send.
                        eprintln!("ui.register: {e}; panels off");
                        self.off = true;
                        return;
                    }
                }
            }
        }
        self.shown = Some(panels);
    }
}

/// Test helper: the panels of each named state, as one vectors document for the hub's
/// window-free tests (`tests/hideout-vectors/modules/<file>.json`). With `UPDATE_VECTORS=1` the
/// file is written; otherwise it must match, so a plugin's tiles and the hub's tests never drift.
pub fn check_vectors(
    file: &str,
    manifest: (&str, &str, &[String], &[String]),
    states: Vec<(&str, Vec<Value>)>,
) {
    let (id, name, declared, actions) = manifest;
    let doc = json!({
        "module": id,
        "name": name,
        "declared": declared,
        "actions": actions,
        "states": states
            .into_iter()
            .map(|(s, panels)| json!({"state": s, "panels": panels}))
            .collect::<Vec<_>>(),
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/hideout-vectors/modules")
        .join(file);
    let text = serde_json::to_string_pretty(&doc).expect("json") + "\n";
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().expect("dir")).expect("vectors dir");
        std::fs::write(&path, &text).expect("write vectors");
        return;
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        on_disk == text,
        "{} is out of date: run the test with UPDATE_VECTORS=1",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_with_the_same_shape_patches_only_what_changed() {
        let a = Tile {
            value: "18:42".into(),
            tone: "active",
            sub: Some("Focus".into()),
            meter: Some((1.0, 10.0)),
            button: Some(Button::new("Pause", Press::Action("pause"))),
        };
        let mut b = a.clone();
        b.value = "18:41".into();
        let ops = diff(&a.tree(), &b.tree()).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0]["node"], "glance");
        assert_eq!(ops[0]["props"]["value"], "18:41");
        b.button = None;
        assert!(
            diff(&a.tree(), &b.tree()).is_none(),
            "a shape change registers again"
        );
    }

    #[test]
    fn a_dropped_prop_is_cleared_with_null() {
        let a = json!({"type": "button", "id": "go", "props": {"label": "x", "action": "t"}});
        let b = json!({"type": "button", "id": "go", "props": {"label": "x"}});
        let ops = diff(&a, &b).unwrap();
        assert_eq!(ops[0]["props"]["action"], Value::Null);
    }
}
