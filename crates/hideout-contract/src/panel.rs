//! The panel schema across the process boundary (§5): `ui.register` trees and their caps.

use serde::{Deserialize, Serialize};

use crate::error::{fail, Error, ErrorId};
use crate::module::Slot;

/// Caps on one panel's tree (§5.5, `ui.limits`). Over a cap, that panel is rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub nodes: usize,
    pub depth: usize,
    /// Bytes of UTF-8 in any one string in a node's props.
    pub text_bytes: usize,
    pub canvas_shapes: usize,
    pub table_rows: usize,
    pub streams: usize,
}

/// The published caps (spec v1.1, #918). Canvas shapes and table rows are this crate's numbers.
pub const LIMITS: Limits = Limits {
    nodes: 2000,
    depth: 32,
    text_bytes: 16 * 1024,
    canvas_shapes: 1000,
    table_rows: 10_000,
    streams: 8,
};

/// Largest stream frame payload (§5.4, spec v1.1).
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
/// Most frames a second a panel stream runs at (spec v1.1).
pub const MAX_STREAM_FPS: u32 = 60;
/// `ui.asset.put` limit (§5.3).
pub const MAX_ASSET_BYTES: usize = 256 * 1024;

/// Node types of the v1 vocabulary (§5.2). Anything else draws as "needs a newer Hideout".
pub const NODE_TYPES: &[&str] = &[
    "column",
    "row",
    "grid",
    "section",
    "tabs",
    "label",
    "value",
    "button",
    "toggle",
    "slider",
    "choice",
    "color",
    "text_input",
    "list",
    "table",
    "meter",
    "image",
    "gallery",
    "log",
    "markdown",
    "text",
    "canvas",
];
/// Shapes inside a `canvas` (§5.2).
pub const SHAPE_TYPES: &[&str] = &["rect", "key", "circle", "text"];
/// Nodes that take a `stream` prop (§5.4).
pub const STREAM_NODES: &[&str] = &["canvas", "image", "text"];

/// One node `{type, id, props, bind?, stream?, children}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Node {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub props: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

impl Node {
    /// Whether this hub's vocabulary knows the type.
    pub fn known(&self) -> bool {
        NODE_TYPES.contains(&self.kind.as_str())
    }

    /// The stream it is bound to, as a node field or a prop.
    pub fn stream_name(&self) -> Option<&str> {
        self.stream
            .as_deref()
            .or_else(|| self.props.get("stream").and_then(|v| v.as_str()))
    }
}

/// One panel in `ui.register`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Panel {
    pub id: String,
    pub slot: Slot,
    pub title: String,
    pub icon: String,
    pub tree: Node,
}

fn longest_string(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::String(s) => s.len(),
        serde_json::Value::Array(a) => a.iter().map(longest_string).max().unwrap_or(0),
        serde_json::Value::Object(o) => o.values().map(longest_string).max().unwrap_or(0),
        _ => 0,
    }
}

/// Checks one panel's tree against `limits` (§5.5).
pub fn validate(panel: &Panel, limits: &Limits) -> Result<(), Error> {
    let mut nodes = 0;
    let mut streams = 0;
    let mut stack = vec![(&panel.tree, 1usize)];
    while let Some((node, depth)) = stack.pop() {
        nodes += 1;
        let over = |what: &str| {
            fail(
                ErrorId::UiLimits,
                format!("{}: {what} over the cap", panel.id),
            )
        };
        if nodes > limits.nodes {
            return over("nodes");
        }
        if depth > limits.depth {
            return over("depth");
        }
        let text = node.props.values().map(longest_string).max().unwrap_or(0);
        if text > limits.text_bytes {
            return over("text");
        }
        if node.kind == "canvas" && node.children.len() > limits.canvas_shapes {
            return over("canvas shapes");
        }
        if node.kind == "table" {
            let rows = node
                .props
                .get("rows")
                .and_then(|r| r.as_array())
                .map_or(0, Vec::len);
            if rows > limits.table_rows {
                return over("table rows");
            }
        }
        if node.stream_name().is_some() {
            streams += 1;
            if streams > limits.streams {
                return over("streams");
            }
        }
        stack.extend(node.children.iter().map(|c| (c, depth + 1)));
    }
    Ok(())
}

/// `ui.register` params (§5.1, §8.8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Register {
    pub instance: String,
    pub panels: Vec<serde_json::Value>,
}

/// One rejected panel in the `ui.register` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Rejected {
    pub panel: String,
    pub error: String,
}

/// What `ui.register` does with each panel: the registered ones, and the rejected ones with
/// their id. `declared` is the module's `[[ui.panels]]` ids, when known.
pub fn register(
    params: &Register,
    declared: Option<&[String]>,
    limits: &Limits,
) -> (Vec<Panel>, Vec<Rejected>) {
    let mut ok = Vec::new();
    let mut rejected = Vec::new();
    for value in &params.panels {
        let name = value
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let reject = |id: ErrorId| Rejected {
            panel: name.clone(),
            error: id.id().to_string(),
        };
        if declared.is_some_and(|d| !d.contains(&name)) {
            rejected.push(reject(ErrorId::UiUndeclared));
            continue;
        }
        // A tree nested past the cap is rejected before it is read into nodes (each node is an
        // object inside a `children` array: two levels). An unreadable tree counts as over a cap.
        if json_depth(value) > 2 * limits.depth + 2 {
            rejected.push(reject(ErrorId::UiLimits));
            continue;
        }
        match serde_json::from_value::<Panel>(value.clone()) {
            Ok(panel) => match validate(&panel, limits) {
                Ok(()) => ok.push(panel),
                Err(e) => rejected.push(reject(e.id)),
            },
            Err(_) => rejected.push(reject(ErrorId::UiLimits)),
        }
    }
    (ok, rejected)
}

/// How deeply `value` nests, counted without recursion.
pub fn json_depth(value: &serde_json::Value) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(value, 1usize)];
    while let Some((v, d)) = stack.pop() {
        deepest = deepest.max(d);
        match v {
            serde_json::Value::Array(a) => stack.extend(a.iter().map(|c| (c, d + 1))),
            serde_json::Value::Object(o) => stack.extend(o.values().map(|c| (c, d + 1))),
            _ => {}
        }
    }
    deepest
}

/// Bracket nesting a hub accepts in one message before parsing it.
pub const MAX_MESSAGE_NESTING: usize = 128;

/// Parses one wire message. The text's nesting is counted first, without recursion, and a message
/// over [`MAX_MESSAGE_NESTING`] is refused as `ui.limits` before any recursive parser sees it, so
/// a hostile frame cannot overflow the stack. serde_json's own limit is off because the count
/// above already bounds it (a panel tree at the depth cap nests about 2 * 32 + 2 plus the envelope).
pub fn parse_message(text: &str) -> Result<serde_json::Value, String> {
    let (mut depth, mut deepest, mut in_string, mut escaped) = (0usize, 0usize, false, false);
    for b in text.bytes() {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if deepest > MAX_MESSAGE_NESTING {
        return Err(format!(
            "ui.limits: message nests {deepest} deep, over {MAX_MESSAGE_NESTING}"
        ));
    }
    let mut de = serde_json::Deserializer::from_str(text);
    de.disable_recursion_limit();
    let value = serde::Deserialize::deserialize(&mut de).map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(value)
}
