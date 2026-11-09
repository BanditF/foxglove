// SPDX-License-Identifier: MIT
//! The command palette's model (contract §2.6, design UX18): entries come from manifests and the
//! registry only, so this module never starts anything; it filters, ranks and keeps the
//! selection. Hidden or stopped items stay listed, greyed, with their reason. The registry fills
//! the entries in H2.2; until then the palette opens empty.

/// One thing the palette can run or open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What the hub does when it is picked (a page or an action id; opaque here).
    pub target: String,
    /// The plain name and verb, `AI: Free GPU` (never an id).
    pub label: String,
    /// When set, the entry is shown greyed with this reason and can't be run.
    pub unavailable: Option<String>,
}

impl Entry {
    pub fn new(target: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            label: label.into(),
            unavailable: None,
        }
    }
}

/// A subsequence match of `query` in `label`, case-insensitive. Higher is better: earlier and
/// contiguous matches and matches at word starts score more. `None` when it doesn't match.
pub fn score(query: &str, label: &str) -> Option<i32> {
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let l: Vec<char> = label.to_lowercase().chars().collect();
    let (mut qi, mut total, mut last) = (0, 0i32, None::<usize>);
    for (i, c) in l.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            let word_start = i == 0 || !l[i - 1].is_alphanumeric();
            total += 10 - (i as i32).min(9);
            if word_start {
                total += 8;
            }
            if last == Some(i.wrapping_sub(1)) {
                total += 5;
            }
            last = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(total)
}

/// The palette's state while it is open.
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    /// Index into `matches`.
    pub selected: usize,
}

impl Palette {
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.query.clear();
            self.selected = 0;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    /// The entries matching the query, best first; available ones before unavailable ones, then
    /// by score, then in their given order.
    pub fn matches<'a>(&self, entries: &'a [Entry]) -> Vec<&'a Entry> {
        let mut found: Vec<(bool, i32, usize, &Entry)> = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                score(&self.query, &e.label).map(|s| (e.unavailable.is_some(), -s, i, e))
            })
            .collect();
        found.sort_by_key(|(unavail, neg, i, _)| (*unavail, *neg, *i));
        found.into_iter().map(|(.., e)| e).collect()
    }

    /// Moves the selection by `delta`, wrapping, over `count` matches.
    pub fn step(&mut self, delta: i32, count: usize) {
        if count == 0 {
            self.selected = 0;
            return;
        }
        let n = count as i32;
        self.selected = (((self.selected as i32 + delta) % n + n) % n) as usize;
    }

    /// The entry Enter would run: the selected match if it is available.
    pub fn chosen<'a>(&self, entries: &'a [Entry]) -> Option<&'a Entry> {
        self.matches(entries)
            .get(self.selected)
            .copied()
            .filter(|e| e.unavailable.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        let mut stopped = Entry::new("art", "Art: Open");
        stopped.unavailable = Some("stopped after 5 crashes".into());
        vec![
            Entry::new("modules", "Modules"),
            Entry::new("gpu", "AI: Free GPU"),
            stopped,
            Entry::new("mic", "Mic: Mute"),
        ]
    }

    #[test]
    fn empty_query_lists_everything_available_first() {
        let p = Palette::default();
        let es = entries();
        let labels: Vec<_> = p.matches(&es).iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Modules", "AI: Free GPU", "Mic: Mute", "Art: Open"]
        );
    }

    #[test]
    fn subsequence_matching_ranks_word_starts() {
        let p = Palette {
            query: "fg".into(),
            ..Default::default()
        };
        assert_eq!(p.matches(&entries())[0].target, "gpu");
        let p = Palette {
            query: "mute".into(),
            ..Default::default()
        };
        assert_eq!(p.matches(&entries()).len(), 1);
        assert_eq!(p.matches(&entries())[0].target, "mic");
        assert_eq!(score("zz", "Modules"), None);
        assert!(score("mod", "Modules") > score("mod", "A Mixed Odd Dog"));
    }

    #[test]
    fn unavailable_entries_show_but_never_run() {
        let p = Palette {
            query: "art".into(),
            ..Default::default()
        };
        let es = entries();
        assert_eq!(p.matches(&es)[0].target, "art");
        assert_eq!(p.chosen(&es), None);
    }

    #[test]
    fn selection_wraps_and_resets() {
        let mut p = Palette::default();
        p.toggle();
        assert!(p.open);
        p.step(-1, 4);
        assert_eq!(p.selected, 3);
        p.step(2, 4);
        assert_eq!(p.selected, 1);
        p.step(1, 0);
        assert_eq!(p.selected, 0);
        p.query = "x".into();
        p.toggle();
        assert!(!p.open && p.query.is_empty());
    }

    #[test]
    fn an_empty_registry_opens_an_empty_palette() {
        let mut p = Palette::default();
        p.toggle();
        assert!(p.matches(&[]).is_empty());
        assert_eq!(p.chosen(&[]), None);
    }
}
