//! A keyboard's parts (#559): what the app calls each piece of the board, from its profile.
//! A split board has a left and a right half, a one-piece board is "Keyboard", and a profile
//! can name its own (`[[parts]]`). The first part is the one the PC talks to (ZMK's central);
//! the host link calls the first two "left" and "right" whatever the board looks like.
//!
//! Every string that names a part comes from here, so a one-piece board never reads "Left
//! half" and the Corne still does.

use super::boards::Board;

/// One part: its id (the link's "left" / "right" for the halves) and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub id: String,
    pub name: String,
}

/// A board's parts, the one the PC talks to first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parts {
    list: Vec<Part>,
    /// Named by a profile: a reading from a part it doesn't list doesn't rename the others.
    named: bool,
}

/// The host link's names for the first two parts.
const LINK_IDS: [&str; 2] = ["left", "right"];

fn part(id: &str, name: &str) -> Part {
    Part {
        id: id.into(),
        name: name.into(),
    }
}

impl Parts {
    /// `halves` as the profile shorthand: 1 is one piece, 2 a left and a right half.
    pub fn halves(n: u8) -> Parts {
        let list = match n {
            0 | 1 => vec![part("left", "Keyboard")],
            2 => vec![part("left", "Left half"), part("right", "Right half")],
            n => (0..usize::from(n))
                .map(|i| Part {
                    id: LINK_IDS
                        .get(i)
                        .map_or(format!("part{}", i + 1), |s| (*s).to_string()),
                    name: format!("Part {}", i + 1),
                })
                .collect(),
        };
        Parts { list, named: false }
    }

    /// A board's parts from its profile. Without one, one part ("Keyboard") until the keyboard
    /// shows a second ([`Parts::seen`]).
    pub fn of(board: Option<&Board>) -> Parts {
        match board {
            Some(b) if !b.parts.is_empty() => Parts {
                list: b
                    .parts
                    .iter()
                    .map(|p| Part {
                        id: p.id.clone(),
                        name: p.name.clone(),
                    })
                    .collect(),
                named: true,
            },
            Some(b) => Parts {
                named: true,
                ..Parts::halves(b.halves)
            },
            None => Parts::halves(1),
        }
    }

    /// The connected keyboard's parts.
    pub fn current() -> Parts {
        super::boards::current().map_or_else(Parts::default, |b| b.parts())
    }

    /// With `n` parts seen on the keyboard: a board without a profile that reports a second
    /// part is a split. A profile's parts stay as it names them.
    pub fn seen(self, n: usize) -> Parts {
        if self.named || n <= self.list.len() {
            return self;
        }
        Parts::halves(u8::try_from(n).unwrap_or(u8::MAX))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Whether settings about the parts' link ("follow the left") apply.
    pub fn is_split(&self) -> bool {
        self.list.len() > 1
    }

    /// Part `i`'s name: "Left half", "Keyboard". One past the list is "Part 3".
    pub fn name(&self, i: usize) -> String {
        self.list
            .get(i)
            .map_or_else(|| format!("Part {}", i + 1), |p| p.name.clone())
    }

    /// Part `i` in a sentence: "the left half", "the keyboard".
    pub fn the(&self, i: usize) -> String {
        format!("the {}", self.lower(i))
    }

    /// Part `i`'s name within a sentence: "left half", "keyboard".
    pub fn lower(&self, i: usize) -> String {
        lower_first(&self.name(i))
    }

    /// Part `i`'s name where space is short (a row label, a tooltip): "Left", "Keyboard".
    pub fn short(&self, i: usize) -> String {
        let name = self.name(i);
        match name.strip_suffix(" half") {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => name,
        }
    }

    /// The host link's id for part `i`: "left" for the one the PC talks to, "right" for the
    /// other half. `None` past the two the link knows.
    pub fn link_id(i: usize) -> Option<&'static str> {
        LINK_IDS.get(i).copied()
    }

    /// The part the host link calls `id` ("left", "right") or the profile does.
    pub fn index(&self, id: &str) -> Option<usize> {
        LINK_IDS
            .iter()
            .position(|l| *l == id)
            .filter(|i| *i < self.list.len() || !self.named)
            .or_else(|| self.list.iter().position(|p| p.id == id))
    }

    /// The name of the part the link calls `id`; an id it doesn't know, capitalised.
    pub fn name_of(&self, id: &str) -> String {
        match self.index(id) {
            Some(i) => self.clone().seen(i + 1).name(i),
            None => upper_first(id),
        }
    }

    /// Every part at once: "both halves", "the keyboard", "all 3 parts".
    pub fn all(&self) -> String {
        match self.list.len() {
            0 | 1 => "the keyboard".into(),
            2 if self.list.iter().all(|p| p.name.ends_with(" half")) => "both halves".into(),
            2 => "both parts".into(),
            n => format!("all {n} parts"),
        }
    }

    /// What one part is called in general: "half" on a split of halves, else "part".
    pub fn noun(&self) -> &'static str {
        if self.list.len() == 2 && self.list.iter().all(|p| p.name.ends_with(" half")) {
            "half"
        } else {
            "part"
        }
    }
}

impl Default for Parts {
    /// One piece, until a profile or the keyboard says more.
    fn default() -> Self {
        Parts::halves(1)
    }
}

pub fn lower_first(s: &str) -> String {
    // "Keyboard" -> "keyboard", but keep a name that is all capitals ("USB hub" stays).
    let mut c = s.chars();
    match (c.next(), s.chars().nth(1)) {
        (Some(f), Some(n)) if n.is_lowercase() || n == ' ' => f.to_lowercase().chain(c).collect(),
        _ => s.to_string(),
    }
}

pub fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::fixtures::{second_desk, users_desk};

    #[test]
    fn the_corne_has_a_left_and_a_right_half() {
        let p = users_desk::board().parts();
        assert_eq!((p.len(), p.is_split()), (2, true));
        assert_eq!(
            (p.name(0), p.name(1)),
            ("Left half".into(), "Right half".into())
        );
        assert_eq!(p.the(1), "the right half");
        assert_eq!(p.name_of("right"), "Right half");
        assert_eq!((p.all(), p.noun()), ("both halves".into(), "half"));
    }

    #[test]
    fn a_one_piece_board_is_the_keyboard() {
        let p = second_desk::board().parts();
        assert_eq!((p.len(), p.is_split()), (1, false));
        assert_eq!(p.name(0), "Keyboard");
        assert_eq!(p.the(0), "the keyboard");
        assert_eq!(p.name_of("left"), "Keyboard");
        assert_eq!(p.all(), "the keyboard");
        assert!(!format!("{p:?}").contains("half"));
    }

    #[test]
    fn no_profile_is_one_piece_until_a_second_part_reports() {
        let p = Parts::of(None);
        assert_eq!(p.name(0), "Keyboard");
        // A battery reading from the link's "right": it is a split after all.
        assert_eq!(p.name_of("right"), "Right half");
        let p = p.seen(2);
        assert_eq!(
            (p.name(0), p.all()),
            ("Left half".into(), "both halves".into())
        );
        // A profile that says one piece stays one piece.
        let one = second_desk::board().parts();
        assert_eq!(one.clone().seen(2), one);
        assert_eq!(one.index("right"), None);
    }

    #[test]
    fn named_parts_read_in_sentences() {
        let mut b = users_desk::board();
        b.parts = vec![
            super::super::boards::PartSpec {
                id: "main".into(),
                name: "Main board".into(),
            },
            super::super::boards::PartSpec {
                id: "pad".into(),
                name: "Numpad".into(),
            },
        ];
        let p = b.parts();
        assert_eq!(
            (p.the(0), p.all(), p.noun()),
            ("the main board".into(), "both parts".into(), "part")
        );
        assert_eq!(p.index("right"), Some(1), "the link's second part");
        assert_eq!(p.index("pad"), Some(1));
        assert_eq!(lower_first("USB hub"), "USB hub");
    }
}
