//! A keyboard's LEDs by part (#561): how many each part (a half, or the one piece) has, in
//! SET_FRAME's order. Lamp `i` of part `p` is the frame's lamp `offset(p) + i`.
//!
//! The counts come from the board: the firmware, else the board profile. Firmware that says
//! nothing is the Eyelash Corne's ([`LampParts::old_firmware`]), the only board such firmware
//! ran on. A frame addresses lamps with one byte, so a board has at most [`MAX_LAMPS`].

use std::fmt;

/// Lamps a frame can address: its start index is one byte (0-255).
pub const MAX_LAMPS: usize = 256;

/// The Eyelash Corne's LEDs per half: what firmware that doesn't report its parts has.
const CORNE_HALF_LAMPS: u8 = 21;

/// Lamps per part, in frame order: the left half's first, or the one piece's only.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LampParts(Vec<u8>);

/// Why a set of counts isn't a board's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LampsError {
    /// No parts, or a part with no LEDs.
    Empty,
    /// More lamps than one byte of lamp index addresses.
    TooMany(usize),
}

impl fmt::Display for LampsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LampsError::Empty => write!(f, "a board needs at least one part with LEDs"),
            LampsError::TooMany(n) => write!(
                f,
                "{n} LEDs are more than a frame addresses (at most {MAX_LAMPS})"
            ),
        }
    }
}

impl std::error::Error for LampsError {}

impl LampParts {
    /// A board's parts. Refused when there are none, one has no LEDs, or they add up to more
    /// than [`MAX_LAMPS`] (never cut short).
    pub fn new(parts: &[usize]) -> Result<LampParts, LampsError> {
        if parts.is_empty() || parts.contains(&0) {
            return Err(LampsError::Empty);
        }
        let total: usize = parts.iter().sum();
        if total > MAX_LAMPS {
            return Err(LampsError::TooMany(total));
        }
        // Each part fits a byte: the total does, and a lone part of 256 doesn't occur in
        // practice but is refused rather than wrapped.
        parts
            .iter()
            .map(|&n| u8::try_from(n).map_err(|_| LampsError::TooMany(total)))
            .collect::<Result<Vec<u8>, _>>()
            .map(LampParts)
    }

    /// `lamps` shared out evenly over `parts` (any remainder to the last part).
    pub fn even(lamps: usize, parts: usize) -> Result<LampParts, LampsError> {
        let parts = parts.max(1);
        let each = lamps / parts;
        let mut counts = vec![each; parts];
        if let Some(last) = counts.last_mut() {
            *last += lamps - each * parts;
        }
        LampParts::new(&counts)
    }

    /// Firmware that doesn't report its parts: the Eyelash Corne's two halves of 21.
    pub fn old_firmware() -> LampParts {
        LampParts(vec![CORNE_HALF_LAMPS; 2])
    }

    /// Each part's count, in frame order.
    pub fn counts(&self) -> impl ExactSizeIterator<Item = usize> + '_ {
        self.0.iter().map(|&n| usize::from(n))
    }

    /// How many parts.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Never: a board has at least one part.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Part `part`'s count, 0 past the last.
    pub fn count(&self, part: usize) -> usize {
        self.0.get(part).map_or(0, |&n| usize::from(n))
    }

    /// Every lamp of the board.
    pub fn total(&self) -> usize {
        self.counts().sum()
    }

    /// The frame's index of part `part`'s first lamp.
    pub fn offset(&self, part: usize) -> usize {
        self.counts().take(part).sum()
    }

    /// The frame's lamps of part `part`.
    pub fn range(&self, part: usize) -> std::ops::Range<usize> {
        let at = self.offset(part);
        at..at + self.count(part)
    }

    /// The part a frame lamp is on, and its place on that part's strip.
    pub fn part_of(&self, lamp: usize) -> Option<(usize, usize)> {
        let mut at = 0;
        for (p, n) in self.counts().enumerate() {
            if lamp < at + n {
                return Some((p, lamp - at));
            }
            at += n;
        }
        None
    }

    /// Whether every part has the same count (halves that mirror each other).
    pub fn even_split(&self) -> bool {
        self.0.windows(2).all(|w| w[0] == w[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_place_each_lamp_in_frame_order() {
        let corne = LampParts::old_firmware();
        assert_eq!((corne.len(), corne.total()), (2, 42));
        assert_eq!(corne.part_of(21), Some((1, 0)));
        assert!(corne.even_split());

        // A split board with uneven halves.
        let uneven = LampParts::new(&[30, 24]).unwrap();
        assert_eq!(uneven.range(1), 30..54);
        assert_eq!(uneven.part_of(29), Some((0, 29)));
        assert_eq!(uneven.part_of(53), Some((1, 23)));
        assert_eq!(uneven.part_of(54), None);
        assert!(!uneven.even_split());

        // A one-piece board.
        let one = LampParts::even(61, 1).unwrap();
        assert_eq!((one.len(), one.total(), one.range(0)), (1, 61, 0..61));
        assert_eq!(
            LampParts::even(43, 2).unwrap().counts().collect::<Vec<_>>(),
            [21, 22]
        );
    }

    #[test]
    fn more_than_a_byte_of_lamps_is_refused_not_cut_short() {
        assert!(LampParts::new(&[128, 128]).is_ok());
        assert_eq!(LampParts::new(&[200, 57]), Err(LampsError::TooMany(257)));
        assert_eq!(LampParts::new(&[256]), Err(LampsError::TooMany(256)));
        assert_eq!(LampParts::new(&[]), Err(LampsError::Empty));
        assert_eq!(LampParts::new(&[21, 0]), Err(LampsError::Empty));
    }
}
