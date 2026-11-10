// SPDX-License-Identifier: MIT
//! `ui.patch` operations on a panel tree (§5.3): `set` merges props, `insert` adds a subtree
//! under a parent at an index, `remove` takes a node out. A patch applies whole or not at all:
//! the caller applies it to a copy and validates the result against the caps (§5.5).

use hideout_contract::error::{Error, ErrorId};
use hideout_contract::messages::{PatchOp, PatchOpKind};
use hideout_contract::panel::Node;
use std::collections::HashSet;

fn bad(message: String) -> Error {
    Error::new(ErrorId::UiLimits, message)
}

/// The path of child indices from the root to the node `id`.
pub fn path_to(tree: &Node, id: &str) -> Option<Vec<usize>> {
    let mut stack: Vec<(&Node, Vec<usize>)> = vec![(tree, Vec::new())];
    while let Some((n, path)) = stack.pop() {
        if n.id == id {
            return Some(path);
        }
        for (i, c) in n.children.iter().enumerate() {
            let mut p = path.clone();
            p.push(i);
            stack.push((c, p));
        }
    }
    None
}

pub fn find<'a>(tree: &'a Node, id: &str) -> Option<&'a Node> {
    let path = path_to(tree, id)?;
    Some(path.iter().fold(tree, |n, &i| &n.children[i]))
}

pub fn find_mut<'a>(tree: &'a mut Node, id: &str) -> Option<&'a mut Node> {
    let path = path_to(tree, id)?;
    Some(path.iter().fold(tree, |n, &i| &mut n.children[i]))
}

/// The first id that appears twice in `tree`, if any. Ids MUST be unique in a panel (§5.2).
pub fn duplicate_id(tree: &Node) -> Option<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![tree];
    while let Some(n) = stack.pop() {
        if !seen.insert(n.id.as_str()) {
            return Some(n.id.clone());
        }
        stack.extend(n.children.iter());
    }
    None
}

/// Applies `ops` to `tree` in order. On an error `tree` may be half changed: apply to a copy.
pub fn apply(tree: &mut Node, ops: &[PatchOp]) -> Result<(), Error> {
    for op in ops {
        match op.op {
            PatchOpKind::Set => {
                let node = find_mut(tree, &op.node)
                    .ok_or_else(|| bad(format!("set: no node {}", op.node)))?;
                for (k, v) in op.props.iter().flatten() {
                    if v.is_null() {
                        node.props.remove(k);
                    } else {
                        node.props.insert(k.clone(), v.clone());
                    }
                }
            }
            PatchOpKind::Insert => {
                let sub = op
                    .tree
                    .as_ref()
                    .ok_or_else(|| bad(format!("insert under {}: no tree", op.node)))?;
                let sub: Node = serde_json::from_value(sub.clone())
                    .map_err(|e| bad(format!("insert under {}: {e}", op.node)))?;
                let parent = find_mut(tree, &op.node)
                    .ok_or_else(|| bad(format!("insert: no node {}", op.node)))?;
                let at = op
                    .index
                    .map_or(parent.children.len(), |i| i as usize)
                    .min(parent.children.len());
                parent.children.insert(at, sub);
            }
            PatchOpKind::Remove => {
                let path = path_to(tree, &op.node)
                    .ok_or_else(|| bad(format!("remove: no node {}", op.node)))?;
                let Some((&last, parent)) = path.split_last() else {
                    return Err(bad("remove: the root stays".into()));
                };
                let parent = parent.iter().fold(&mut *tree, |n, &i| &mut n.children[i]);
                parent.children.remove(last);
            }
        }
    }
    match duplicate_id(tree) {
        Some(id) => Err(bad(format!("node id {id} used twice"))),
        None => Ok(()),
    }
}

/// The nodes whose props a patch sets: their optimistic values give way to the module's.
pub fn touched(ops: &[PatchOp]) -> impl Iterator<Item = &str> {
    ops.iter()
        .filter(|o| o.op == PatchOpKind::Set)
        .map(|o| o.node.as_str())
}
