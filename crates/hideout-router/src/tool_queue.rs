// SPDX-License-Identifier: MIT
//! Outside tools waiting for the user (S1, contract section 6.4). A tool that asks for more than
//! the free set is put here, by program file (never by the name it gives itself), and the hub's
//! Apps page shows one line about it. Nothing is allowed from here: the user reviews it in the
//! hub's dialog, and only the answer reaches [`crate::router::Router::set_tool_grants`]. The
//! queue lives in memory only and holds at most [`QUEUE_MAX`] programs: the newest ask wins,
//! and one program is only ever there once, so no program can fill it.

use std::collections::VecDeque;

use crate::ToolId;

/// The most programs waiting at once.
pub const QUEUE_MAX: usize = 5;
/// The most permissions kept for one program; the rest of its list is dropped.
pub const ITEMS_MAX: usize = 32;

/// One permission a tool asked for that only the user can allow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskItem {
    /// The permission string (`lighting.write`).
    pub raw: String,
    /// The plain words, from the contract (`change your keyboard's lighting`-style text for the
    /// end of "Hideout didn't let it ...").
    pub does: String,
    /// Whether the dialog draws it in the sensitive (red) style.
    pub red: bool,
}

/// A program waiting for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolAsk {
    pub id: ToolId,
    /// What the program calls itself: display text only, untrusted, never cleaned here.
    pub name: String,
    pub items: Vec<AskItem>,
    /// The program runs scripts: what is allowed is for this connection only.
    pub runs_scripts: bool,
}

#[derive(Default)]
pub struct ToolQueue {
    /// Oldest first.
    entries: VecDeque<ToolAsk>,
}

impl ToolQueue {
    /// Adds the ask. The same program replaces its earlier ask and becomes the newest; when
    /// there are more than [`QUEUE_MAX`] the oldest goes.
    pub fn push(&mut self, mut ask: ToolAsk) {
        ask.items.truncate(ITEMS_MAX);
        self.entries.retain(|e| e.id != ask.id);
        self.entries.push_back(ask);
        while self.entries.len() > QUEUE_MAX {
            self.entries.pop_front();
        }
    }

    /// Takes this program's ask out.
    pub fn remove(&mut self, id: &ToolId) -> Option<ToolAsk> {
        let at = self.entries.iter().position(|e| &e.id == id)?;
        self.entries.remove(at)
    }

    /// Drops every ask for this program file at any hash: the file changed.
    pub fn remove_path(&mut self, image_path: &str) {
        self.entries
            .retain(|e| !e.id.image_path.eq_ignore_ascii_case(image_path));
    }

    /// The waiting programs, newest first.
    pub fn list(&self) -> Vec<ToolAsk> {
        self.entries.iter().rev().cloned().collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(n: usize) -> ToolAsk {
        ToolAsk {
            id: ToolId {
                image_path: format!(r"C:\T\{n}.exe"),
                image_sha256: "h".into(),
            },
            name: format!("tool {n}"),
            items: vec![AskItem {
                raw: "lighting.write".into(),
                does: "change lighting".into(),
                red: false,
            }],
            runs_scripts: false,
        }
    }

    #[test]
    fn queue_caps_at_5_newest_wins() {
        let mut q = ToolQueue::default();
        for n in 0..8 {
            q.push(ask(n));
        }
        assert_eq!(q.len(), QUEUE_MAX);
        let names: Vec<String> = q.list().into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["tool 7", "tool 6", "tool 5", "tool 4", "tool 3"]);
        // The same program again is one entry, and the newest.
        q.push(ask(3));
        assert_eq!(q.len(), QUEUE_MAX);
        assert_eq!(q.list()[0].name, "tool 3");
        // The same name from another file is another program.
        let mut other = ask(9);
        other.name = "tool 3".into();
        q.push(other);
        assert_eq!(q.len(), QUEUE_MAX);
        assert_eq!(
            q.list().iter().filter(|a| a.name == "tool 3").count(),
            2,
            "the name decides nothing"
        );
    }

    #[test]
    fn one_program_cannot_hold_a_long_list() {
        let mut q = ToolQueue::default();
        let mut a = ask(1);
        a.items = (0..100)
            .map(|i| AskItem {
                raw: format!("facts.read:k{i}"),
                does: String::new(),
                red: false,
            })
            .collect();
        q.push(a);
        assert_eq!(q.list()[0].items.len(), ITEMS_MAX);
    }

    #[test]
    fn a_changed_file_drops_its_ask() {
        let mut q = ToolQueue::default();
        q.push(ask(1));
        q.push(ask(2));
        q.remove_path(r"c:\t\1.EXE");
        assert_eq!(q.len(), 1);
        assert!(q.remove(&ask(2).id).is_some());
        assert!(q.is_empty());
    }
}
