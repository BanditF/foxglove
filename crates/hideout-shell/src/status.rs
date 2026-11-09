// SPDX-License-Identifier: MIT
//! The status strip's budget (design UX15): at most four items, the one root problem first, then
//! the user's pinned items in their order, then the rest by priority hint. The user's choice
//! beats a module's hint.

/// One status item. Modules declare them; the hub's own (Paused) come from the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub text: String,
    /// The one root problem (design UX4), if this item is it.
    pub problem: bool,
    /// A module's hint; higher first. The user's pins override it.
    pub priority: i32,
}

impl Item {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            problem: false,
            priority: 0,
        }
    }
}

/// The most the strip shows.
pub const MAX_ITEMS: usize = 4;

/// The items the strip shows, in order.
pub fn visible<'a>(items: &'a [Item], pinned: &[String]) -> Vec<&'a Item> {
    let problem = items.iter().find(|i| i.problem);
    let mut out: Vec<&Item> = problem.into_iter().collect();
    for id in pinned {
        if let Some(i) = items.iter().find(|i| &i.id == id && !i.problem) {
            if !out.iter().any(|o| o.id == i.id) {
                out.push(i);
            }
        }
    }
    let mut rest: Vec<&Item> = items
        .iter()
        .filter(|i| !i.problem && !pinned.contains(&i.id))
        .collect();
    rest.sort_by_key(|i| -i.priority);
    out.extend(rest);
    out.truncate(MAX_ITEMS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Item> {
        let mut p = Item::new("kb", "Keyboard battery low");
        p.problem = true;
        let mut hi = Item::new("hi", "High");
        hi.priority = 5;
        vec![
            Item::new("a", "A"),
            hi,
            Item::new("b", "B"),
            p,
            Item::new("c", "C"),
            Item::new("d", "D"),
        ]
    }

    #[test]
    fn problem_first_then_pins_then_priority_and_at_most_four() {
        let all = items();
        let shown: Vec<_> = visible(&all, &["c".into(), "b".into()])
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(shown, ["kb", "c", "b", "hi"]);
    }

    #[test]
    fn pins_of_missing_items_are_skipped() {
        let all = items();
        let shown: Vec<_> = visible(&all[..2], &["gone".into()])
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(shown, ["hi", "a"]);
        assert!(visible(&[], &[]).is_empty());
    }
}
