//! "Check LED map" (guess and confirm): instead of one click per LED, a dot runs along the
//! keyboard's LED chain while the drawing shows where Foxglove thinks each LED is. The user says
//! whether the two match, and if not, how they differ: backwards, mirrored, or rows flipped. One
//! to three answers fix a chain that is a snake over the keys, which most per-key boards are.
//!
//! The proposal is the board profile's map when it ships one (the Eyelash Corne's, from crkbd's
//! chain), else a column snake guessed from the drawing ([`serpentine`]). A map the user found
//! with "Find my LEDs" is shown as it is (no corrections: it isn't a chain over keys). A board
//! whose LEDs aren't one per key can't be guessed: "Find my LEDs" (one click per LED) is the
//! fallback, here and for a chain the answers can't fix.
//!
//! "Matches" saves the map as points at the keys' centres in the store's `[led_map]`, which
//! marks it checked, so the profile's shipped map is asked about once.

use foxglove_devices::Rgb;

use super::boards::{Board, Drawing};
use super::led_map::{BoardLeds, Seen, LIT};

/// The steps the dot takes a second (one LED each).
pub const STEP_MS: u64 = 350;
/// How many times the dot runs along the chain before it stops (Run again starts it anew).
pub const ROUNDS: u32 = 4;

/// What the user said about the dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// It follows the drawing.
    Matches,
    /// Same path, the other way round.
    Backwards,
    /// Left and right swapped within each half (it starts on the other side).
    Mirrored,
    /// Top and bottom swapped (it goes up where the drawing goes down).
    RowsFlipped,
}

/// Where the proposal came from, for the card's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The board profile's map (shipped, or already checked).
    Profile { checked: bool },
    /// "Find my LEDs" points the user saved.
    Found,
    /// Guessed from the drawing: a column snake.
    Guess,
}

/// The dot test's state: the chain proposed now and what the user said so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub board: String,
    pub name: String,
    pub drawing: Drawing,
    pub halves: u8,
    pub per_half: usize,
    pub source: Source,
    /// The proposal as keys: lamp `i` of a half sits under key `keys[i]` of its half's drawing
    /// (the right half mirrors the left). `None` for a found map ([`Self::found`]).
    pub keys: Option<Vec<u8>>,
    /// A found map: each lamp's point on its half, or `None` for one not seen.
    pub found: Vec<Option<[f32; 2]>>,
    /// The corrections so far, in order.
    pub fixes: Vec<Answer>,
    /// A problem to show (a failed save).
    pub note: Option<String>,
}

/// Each LED of a half a key, as a column snake: the outer column top to bottom, the next bottom
/// to top, and so on; then the thumbs, outer first. `None` when the half's LEDs aren't one per
/// key (no guess: "Find my LEDs").
pub fn serpentine(drawing: &Drawing, per_half: usize) -> Option<Vec<u8>> {
    let (columns, rows, thumbs) = drawing.grid();
    if columns * rows + thumbs != per_half {
        return None;
    }
    let mut keys = Vec::with_capacity(per_half);
    for c in 0..columns {
        for i in 0..rows {
            let r = if c % 2 == 0 { i } else { rows - 1 - i };
            keys.push(u8::try_from(c * rows + r).ok()?);
        }
    }
    for t in 0..thumbs {
        keys.push(u8::try_from(columns * rows + t).ok()?);
    }
    Some(keys)
}

/// A key chain corrected by one answer. Mirrored and rows flipped move each LED to the key in
/// the mirror place (the thumbs swap ends when mirrored and stay when the rows flip); backwards
/// reverses the chain.
pub fn fix(keys: &[u8], drawing: &Drawing, answer: Answer) -> Vec<u8> {
    let (columns, rows, thumbs) = drawing.grid();
    let grid = columns * rows;
    let moved = |k: u8, f: &dyn Fn(usize, usize) -> (usize, usize), t: &dyn Fn(usize) -> usize| {
        let k = usize::from(k);
        let to = if k < grid {
            let (c, r) = f(k / rows, k % rows);
            c * rows + r
        } else if k < grid + thumbs {
            grid + t(k - grid)
        } else {
            k
        };
        u8::try_from(to).unwrap_or(u8::MAX)
    };
    match answer {
        Answer::Matches => keys.to_vec(),
        Answer::Backwards => keys.iter().rev().copied().collect(),
        Answer::Mirrored => keys
            .iter()
            .map(|&k| moved(k, &|c, r| (columns - 1 - c, r), &|t| thumbs - 1 - t))
            .collect(),
        Answer::RowsFlipped => keys
            .iter()
            .map(|&k| moved(k, &|c, r| (c, rows - 1 - r), &|t| t))
            .collect(),
    }
}

/// Whether a key chain fits a half: one LED per key, every key once.
fn fits(keys: &[u8], drawing: &Drawing, per_half: usize) -> bool {
    let n = drawing.keys(false).len();
    let mut seen = vec![false; n];
    keys.len() == per_half
        && keys.iter().all(|&k| {
            let k = usize::from(k);
            k < n && !std::mem::replace(&mut seen[k], true)
        })
}

impl Confirm {
    /// The dot test for a board profile: its found map, its own map, or a guess. `None` when
    /// there's nothing to propose (the LEDs aren't one per key, or the halves have different
    /// counts, so the dot can't run along both at once): "Find my LEDs" instead.
    pub fn new(board: &Board) -> Option<Confirm> {
        let halves = if board.halves == 1 { 1 } else { 2 };
        let lighting = board.lighting.as_ref()?;
        let parts = lighting.lamp_parts(halves)?;
        if !parts.even_split() {
            return None;
        }
        let per_half = parts.count(0);
        let drawing = board.drawing.clone().unwrap_or_default();
        let mut c = Confirm {
            board: board.id.clone(),
            name: board.name.clone(),
            drawing,
            halves,
            per_half,
            source: Source::Guess,
            keys: None,
            found: Vec::new(),
            fixes: Vec::new(),
            note: None,
        };
        if per_half == 0 {
            return None;
        }
        if !lighting.led_points.is_empty() {
            c.source = Source::Found;
            c.found = lighting.led_points.clone();
            c.found.resize(per_half * usize::from(halves), None);
            return Some(c);
        }
        // A one-piece board's keys are both halves' drawings side by side: guesses are per half.
        if halves == 2 && fits(&lighting.led_keys, &c.drawing, per_half) {
            c.source = Source::Profile {
                checked: lighting.led_map == super::boards::LED_MAP_CHECKED,
            };
            c.keys = Some(lighting.led_keys.clone());
            return Some(c);
        }
        if halves == 2 {
            c.keys = Some(serpentine(&c.drawing, per_half)?);
            return Some(c);
        }
        None
    }

    pub fn total(&self) -> usize {
        self.per_half * usize::from(self.halves)
    }

    /// Whether the answers can correct the proposal (it's a chain over keys).
    pub fn correctable(&self) -> bool {
        self.keys.is_some()
    }

    /// The user's answer: a correction turns the chain; Matches changes nothing (the caller
    /// saves).
    pub fn answer(&mut self, a: Answer) {
        let Some(keys) = &self.keys else { return };
        if a != Answer::Matches {
            self.keys = Some(fix(keys, &self.drawing, a));
            self.fixes.push(a);
        }
    }

    /// Takes the last correction back.
    pub fn undo(&mut self) {
        let (Some(keys), Some(a)) = (&self.keys, self.fixes.pop()) else {
            return;
        };
        // Each correction is its own inverse.
        self.keys = Some(fix(keys, &self.drawing, a));
    }

    /// Where lamp `index` of half `half` (0 left, 1 right) is proposed to be: a point on its
    /// half's drawing, in key units.
    pub fn point(&self, half: usize, index: usize) -> Option<[f32; 2]> {
        match &self.keys {
            Some(keys) => {
                let k = *keys.get(index)?;
                let c = self.drawing.keys(half == 1).get(usize::from(k))?.center();
                Some([c.x, c.y])
            }
            None => *self.found.get(half * self.per_half + index)?,
        }
    }

    /// The dot's frames: step `i` lights LED `i` of every half (white), the others off.
    pub fn frames(&self) -> Vec<Vec<Rgb>> {
        let [r, g, b] = LIT;
        (0..self.per_half)
            .map(|i| {
                let mut f = vec![Rgb { r: 0, g: 0, b: 0 }; self.total()];
                for h in 0..usize::from(self.halves) {
                    f[h * self.per_half + i] = Rgb { r, g, b };
                }
                f
            })
            .collect()
    }

    /// The map to save once it matches: each LED at its proposed point.
    pub fn result(&self) -> BoardLeds {
        let leds = (0..usize::from(self.halves))
            .flat_map(|h| (0..self.per_half).map(move |i| (h, i)))
            .filter_map(|(h, i)| {
                let [x, y] = self.point(h, i)?;
                Some(Seen {
                    lamp: u16::try_from(h * self.per_half + i).ok()?,
                    x: (x * 1000.0).round() / 1000.0,
                    y: (y * 1000.0).round() / 1000.0,
                })
            })
            .collect();
        BoardLeds {
            lamps: u16::try_from(self.total()).unwrap_or(u16::MAX),
            halves: self.halves,
            leds,
        }
    }
}

/// What the corrections so far say, in words ("Turned round, then mirrored").
pub fn fixes_text(fixes: &[Answer]) -> Option<String> {
    let words: Vec<&str> = fixes
        .iter()
        .map(|a| match a {
            Answer::Matches => "as it was",
            Answer::Backwards => "turned round",
            Answer::Mirrored => "mirrored",
            Answer::RowsFlipped => "rows flipped",
        })
        .collect();
    let text = words.join(", then ");
    let mut chars = text.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corne() -> Board {
        Board::parse(include_str!("../../resources/boards/eyelash-corne.toml")).unwrap()
    }

    #[test]
    fn the_guess_is_a_column_snake_with_the_thumbs_last() {
        let d = Drawing::default();
        let keys = serpentine(&d, 21).unwrap();
        assert_eq!(
            keys,
            [0, 1, 2, 5, 4, 3, 6, 7, 8, 11, 10, 9, 12, 13, 14, 17, 16, 15, 18, 19, 20]
        );
        assert!(fits(&keys, &d, 21));
        // Six underglow LEDs on a 21-key half: no guess.
        assert!(serpentine(&d, 6).is_none());
    }

    #[test]
    fn each_correction_is_a_permutation_and_its_own_inverse() {
        let d = Drawing::default();
        let shipped = corne().lighting.unwrap().led_keys;
        for a in [Answer::Backwards, Answer::Mirrored, Answer::RowsFlipped] {
            let once = fix(&shipped, &d, a);
            assert!(fits(&once, &d, 21), "{a:?}");
            assert_ne!(once, shipped, "{a:?}");
            assert_eq!(fix(&once, &d, a), shipped, "{a:?}");
        }
        // Mirrored: the inner thumb (20) becomes the outer (18), the outer column the inner.
        let m = fix(&shipped, &d, Answer::Mirrored);
        assert_eq!(m[0], 18);
        assert_eq!(
            m[18], 15,
            "the outer column's top key mirrors to the inner's"
        );
        // Rows flipped: the inner column's bottom key (17) becomes its top (15).
        assert_eq!(fix(&shipped, &d, Answer::RowsFlipped)[1], 15);
    }

    #[test]
    fn a_shipped_map_is_proposed_and_one_to_three_answers_reach_any_snake() {
        let mut c = Confirm::new(&corne()).unwrap();
        assert_eq!(c.source, Source::Profile { checked: false });
        assert!(c.correctable());
        let shipped = c.keys.clone().unwrap();
        // The keyboard's real chain is the shipped one mirrored and backwards: two answers.
        let real = fix(
            &fix(&shipped, &c.drawing, Answer::Mirrored),
            &c.drawing,
            Answer::Backwards,
        );
        c.answer(Answer::Mirrored);
        c.answer(Answer::Backwards);
        assert_eq!(c.keys.as_ref(), Some(&real));
        assert_eq!(
            fixes_text(&c.fixes).as_deref(),
            Some("Mirrored, then turned round")
        );
        c.answer(Answer::Matches);
        assert_eq!(c.fixes.len(), 2, "Matches isn't a correction");
        c.undo();
        assert_eq!(
            c.keys.as_ref(),
            Some(&fix(&shipped, &c.drawing, Answer::Mirrored))
        );
        c.undo();
        assert_eq!(c.keys.as_ref(), Some(&shipped));
        c.undo();
        assert_eq!(c.keys.as_ref(), Some(&shipped), "nothing more to undo");
    }

    #[test]
    fn the_dot_lights_the_same_led_on_both_halves_and_the_result_is_key_centres() {
        let c = Confirm::new(&corne()).unwrap();
        let frames = c.frames();
        assert_eq!(frames.len(), 21);
        for (i, f) in frames.iter().enumerate() {
            let lit: Vec<usize> = (0..42).filter(|&l| f[l].r == 255).collect();
            assert_eq!(lit, vec![i, 21 + i]);
        }
        let r = c.result();
        assert_eq!((r.lamps, r.halves, r.leds.len()), (42, 2, 42));
        // LED 0 under the inner thumb on the left; the right half's LED 0 is its mirror image.
        let (w, _) = c.drawing.size();
        let thumb = c.drawing.keys(false)[20].center();
        assert_eq!([r.leds[0].x, r.leds[0].y], [thumb.x, thumb.y]);
        assert!((r.leds[21].x - (w - thumb.x)).abs() < 1e-3);
    }

    #[test]
    fn unknown_boards_get_a_guess_found_maps_are_shown_as_they_are() {
        let mut board = corne();
        let l = board.lighting.as_mut().unwrap();
        l.led_keys.clear();
        l.led_map = "provisional".into();
        let c = Confirm::new(&board).unwrap();
        assert_eq!(c.source, Source::Guess);
        assert_eq!(c.keys, serpentine(&Drawing::default(), 21));
        // A broken profile map (a key twice) falls back to the guess too.
        let l = board.lighting.as_mut().unwrap();
        l.led_keys = vec![0; 21];
        assert_eq!(Confirm::new(&board).unwrap().source, Source::Guess);
        // Underglow that isn't one LED per key: no proposal (Find my LEDs).
        board.lighting.as_mut().unwrap().lamps = 12;
        assert!(Confirm::new(&board).is_none());
        // Halves of different counts (21 + 18): no dot along both at once (Find my LEDs).
        let l = board.lighting.as_mut().unwrap();
        (l.lamps, l.parts) = (39, vec![21, 18]);
        assert!(Confirm::new(&board).is_none());
        // A found map: shown, not correctable.
        let mut board = corne();
        let l = board.lighting.as_mut().unwrap();
        l.led_points = vec![Some([1.0, 2.0]); 42];
        let mut c = Confirm::new(&board).unwrap();
        assert_eq!(c.source, Source::Found);
        assert!(!c.correctable());
        c.answer(Answer::Mirrored);
        assert!(c.fixes.is_empty());
        assert_eq!(c.point(1, 3), Some([1.0, 2.0]));
        assert_eq!(c.result().leds.len(), 42);
    }
}
