// SPDX-License-Identifier: MIT
//! View state, kept by the hub per panel and keyed by node id (§5.6). It lives apart from the
//! tree, so a new registration of the same panel finds it again.

use hideout_contract::panel::Node;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// What the user did to one panel that the module never sees.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    /// Scroll offset `[x, y]` of a scrolling node (the panel root, `list`, `table`, `log`).
    pub scroll: HashMap<String, [f32; 2]>,
    /// The selected child of a `tabs` node, by the child's id (so reordering keeps it).
    pub tab: HashMap<String, String>,
    /// Whether a `section` is open.
    pub open: HashMap<String, bool>,
    /// Text being typed in a `text_input`.
    pub text: HashMap<String, String>,
    /// Values the user set that wait for the module's answer to their `ui.event` (§5.3). Drawn
    /// in place of the tree's value; a refusal drops them.
    pub optimistic: HashMap<String, Value>,
    /// A node to bring into view on the next draw (a button's `opens = "page#<node>"`, v1.4);
    /// a `section` there opens. Cleared once drawn.
    pub focus: Option<String>,
    /// Set when the panel was (re)built: the next draw pushes the scroll offsets back into egui.
    pub(crate) restore: bool,
}

impl ViewState {
    /// Drops the state of nodes that are no longer in `tree`, so it cannot grow without bound
    /// over many registrations. Optimistic values are dropped too: a new tree is the module's word.
    pub(crate) fn rebuilt(&mut self, tree: &Node) {
        let ids = ids(tree);
        self.scroll.retain(|k, _| ids.contains(k.as_str()));
        self.tab.retain(|k, _| ids.contains(k.as_str()));
        self.open.retain(|k, _| ids.contains(k.as_str()));
        self.text.retain(|k, _| ids.contains(k.as_str()));
        self.optimistic.clear();
        self.restore = true;
    }
}

/// Every node id in `tree`, without recursion.
pub fn ids(tree: &Node) -> HashSet<&str> {
    let mut out = HashSet::new();
    let mut stack = vec![tree];
    while let Some(n) = stack.pop() {
        out.insert(n.id.as_str());
        stack.extend(n.children.iter());
    }
    out
}
