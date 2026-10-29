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
//!
//! An LED that drives a group of keys (the Eyelash Corne's columns, #608) shows at its keys'
//! centre and the corrections move each of its keys; one that lights nothing is left out of
//! the dot's run and the drawing.
//!
//! A split board's halves each have their own chain: an answer is for both halves, or for one
//! when their dots run differently. The dot's pace is the user's (about 0.7 s an LED by
//! default), and the drawing's dot moves when the keyboard has the frame, so both show the same
//! LED at the same time.

use foxglove_devices::Rgb;

use super::boards::{Board, Drawing, Lamp};
use super::led_map::{BoardLeds, Seen, LIT};

/// The dot's paces: a name and how long each LED stays lit, in ms. Each is well above what a
/// split board's other half needs to follow (SET_FRAME relays it at most 10 times a second, one
/// frame acknowledged before the next), so both halves show the same LED, and slow enough to
/// follow on the board and the drawing at once.
pub const PACES: [(&str, u64); 3] = [("Slower", 1100), ("Normal", 700), ("Faster", 450)];
/// The pace the test starts at.
pub const DEFAULT_PACE: usize = 1;
/// About how long the dot runs before it stops (Run again starts it anew): at least two rounds.
const RUN_MS: u64 = 60_000;
const MIN_ROUNDS: u32 = 2;
const MAX_ROUNDS: u32 = 4;

/// How long each LED stays lit at pace `pace` (an index into [`PACES`]; out of range is the
/// default).
pub fn step_ms(pace: usize) -> u64 {
    PACES.get(pace).unwrap_or(&PACES[DEFAULT_PACE]).1
}

/// How many rounds a chain of `leds` runs at `step_ms` an LED: about [`RUN_MS`], two to four.
pub fn rounds(leds: usize, step_ms: u64) -> u32 {
    let round = (leds as u64).max(1) * step_ms.max(1);
    let n = RUN_MS.div_ceil(round);
    u32::try_from(n)
        .unwrap_or(MAX_ROUNDS)
        .clamp(MIN_ROUNDS, MAX_ROUNDS)
}

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

/// One correction: what was said, and for which half (`None`: every half).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fix {
    pub half: Option<usize>,
    pub answer: Answer,
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
    /// The proposal as keys, one chain per half: lamp `i` of half `h` lights keys
    /// `keys[h][i]` of its half's drawing (the right half's drawing mirrors the left's): one
    /// key, a group, or none (dead). Both start the same; an answer for one half turns only its
    /// chain. `None` for a found map ([`Self::found`]).
    pub keys: Option<Vec<Vec<Lamp>>>,
    /// A found map: each lamp's point on its half, or `None` for one not seen.
    pub found: Vec<Option<[f32; 2]>>,
    /// The corrections so far, in order.
    pub fixes: Vec<Fix>,
    /// Which half the next answer is for (`None`: every half). The card's choice.
    pub half: Option<usize>,
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

/// A lamp chain corrected by one answer ([`fix`] for each lamp's keys). Backwards reverses
/// the lamps that light something and leaves the dead ones where they are in the chain.
pub fn fix_lamps(lamps: &[Lamp], drawing: &Drawing, answer: Answer) -> Vec<Lamp> {
    match answer {
        Answer::Backwards => {
            let mut live = lamps.iter().filter(|l| !l.dead()).rev();
            lamps
                .iter()
                .map(|l| {
                    if l.dead() {
                        l.clone()
                    } else {
                        live.next().cloned().unwrap_or_default()
                    }
                })
                .collect()
        }
        _ => lamps
            .iter()
            .map(|l| Lamp(fix(l.keys(), drawing, answer)))
            .collect(),
    }
}

/// Whether a lamp chain fits a half: one lamp per LED, each key under at most one lamp, at
/// least one lamp lighting something.
fn fits(lamps: &[Lamp], drawing: &Drawing, per_half: usize) -> bool {
    let n = drawing.keys(false).len();
    let mut seen = vec![false; n];
    lamps.len() == per_half
        && lamps.iter().any(|l| !l.dead())
        && lamps.iter().flat_map(Lamp::keys).all(|&k| {
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
        let drawing = super::boards::drawing(Some(board));
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
            half: None,
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
        // A split's drawing is one half (the other mirrors it); a one-piece board's is the whole
        // board (#559). Either way the chain is per drawing.
        if fits(&lighting.led_keys, &c.drawing, per_half) {
            c.source = Source::Profile {
                checked: lighting.led_map == super::boards::LED_MAP_CHECKED,
            };
            c.keys = Some(vec![lighting.led_keys.clone(); usize::from(halves)]);
            return Some(c);
        }
        let guess = Lamp::chain(&serpentine(&c.drawing, per_half)?);
        c.keys = Some(vec![guess; usize::from(halves)]);
        Some(c)
    }

    pub fn total(&self) -> usize {
        self.per_half * usize::from(self.halves)
    }

    /// Whether the answers can correct the proposal (it's a chain over keys).
    pub fn correctable(&self) -> bool {
        self.keys.is_some()
    }

    /// The user's answer, for the half picked ([`Self::half`]): a correction turns that half's
    /// chain (every half's when none is picked); Matches changes nothing (the caller saves).
    pub fn answer(&mut self, a: Answer) {
        let half = self.half.filter(|&h| h < usize::from(self.halves));
        if a != Answer::Matches && self.turn(half, a) {
            self.fixes.push(Fix { half, answer: a });
        }
    }

    /// Takes the last correction back.
    pub fn undo(&mut self) {
        if let Some(f) = self.fixes.pop() {
            // Each correction is its own inverse.
            self.turn(f.half, f.answer);
        }
    }

    /// Turns the chain of `half` (every half's for `None`) by `a`; false for a found map.
    fn turn(&mut self, half: Option<usize>, a: Answer) -> bool {
        let Some(keys) = &mut self.keys else {
            return false;
        };
        for (h, chain) in keys.iter_mut().enumerate() {
            if half.is_none_or(|only| only == h) {
                *chain = fix_lamps(chain, &self.drawing, a);
            }
        }
        true
    }

    /// Where lamp `index` of half `half` (0 left, 1 right) is proposed to be: a point on its
    /// half's drawing, in key units (a group's centre). `None` for a lamp that lights nothing.
    pub fn point(&self, half: usize, index: usize) -> Option<[f32; 2]> {
        match &self.keys {
            Some(keys) => {
                let lamp = keys.get(half)?.get(index)?;
                let c = lamp.center(&self.drawing.keys(half == 1))?;
                Some([c.x, c.y])
            }
            None => *self.found.get(half * self.per_half + index)?,
        }
    }

    /// The lamps the dot lights, in order: every one that lights something on some half (a
    /// found map: all of them).
    pub fn steps(&self) -> Vec<usize> {
        (0..self.per_half)
            .filter(|&i| {
                self.keys.as_ref().is_none_or(|keys| {
                    keys.iter()
                        .any(|chain| chain.get(i).is_some_and(|l| !l.dead()))
                })
            })
            .collect()
    }

    /// The lamp the dot's step `step` lights.
    pub fn lamp_at(&self, step: usize) -> Option<usize> {
        self.steps().get(step).copied()
    }

    /// The dot's frames: step `n` lights the `n`th of [`Self::steps`] on every half (white),
    /// the others off.
    pub fn frames(&self) -> Vec<Vec<Rgb>> {
        let [r, g, b] = LIT;
        self.steps()
            .into_iter()
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
            parts: vec![u16::try_from(self.per_half).unwrap_or(u16::MAX); usize::from(self.halves)],
            leds,
        }
    }
}

/// What the corrections so far say, in words ("Turned round, then the right half mirrored").
/// `halves` names the halves; a one-piece board's fixes are never for one half.
pub fn fixes_text(fixes: &[Fix], halves: u8) -> Option<String> {
    let words: Vec<String> = fixes
        .iter()
        .map(|f| {
            let what = match f.answer {
                Answer::Matches => "as it was",
                Answer::Backwards => "turned round",
                Answer::Mirrored => "mirrored",
                Answer::RowsFlipped => "rows flipped",
            };
            match f.half.filter(|_| halves > 1) {
                Some(h) => format!("{} {what}", super::parts::Parts::halves(halves).the(h)),
                None => what.to_string(),
            }
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

    /// crkbd's per-key chain, the Corne's map before #608: an old one-key-per-LED profile.
    const PER_KEY: [u8; 21] = [
        20, 17, 16, 15, 12, 13, 14, 19, 18, 11, 10, 9, 6, 7, 8, 5, 4, 3, 0, 1, 2,
    ];

    /// A Corne-shaped board whose profile maps one key per LED.
    fn per_key() -> Board {
        let mut b = corne();
        b.lighting.as_mut().unwrap().led_keys = Lamp::chain(&PER_KEY);
        b
    }

    /// #608: the Corne's grouped map is proposed as it is; the dot runs over the 6 pixels that
    /// light something, each shown at its keys' centre; corrections move every key of a group
    /// and keep the dead pixels dead.
    #[test]
    fn a_grouped_map_runs_the_dot_over_live_lamps_only() {
        let mut c = Confirm::new(&corne()).unwrap();
        assert_eq!(c.source, Source::Profile { checked: false });
        assert_eq!(c.steps(), [0, 1, 2, 3, 4, 5]);
        let frames = c.frames();
        assert_eq!(frames.len(), 6, "no step for a pixel that lights nothing");
        for (i, f) in frames.iter().enumerate() {
            let lit: Vec<usize> = (0..42).filter(|&l| f[l].r == 255).collect();
            assert_eq!(lit, vec![i, 21 + i]);
        }
        assert_eq!(c.lamp_at(5), Some(5));
        assert_eq!(c.point(0, 6), None, "dead: not drawn");
        let keys = c.drawing.keys(false);
        let centre = [15, 16, 17, 20]
            .iter()
            .fold(egui::Vec2::ZERO, |s, &k| s + keys[k].center().to_vec2())
            / 4.0;
        let p = c.point(0, 0).unwrap();
        assert!((p[0] - centre.x).abs() < 1e-4 && (p[1] - centre.y).abs() < 1e-4);
        // Backwards: the outer column first, the dead pixels still dead.
        c.answer(Answer::Backwards);
        let chain = &c.keys.as_ref().unwrap()[0];
        assert_eq!(chain[0].keys(), [0, 1, 2]);
        assert_eq!(chain[5].keys(), [15, 16, 17, 20]);
        assert!(chain[6..].iter().all(Lamp::dead));
        c.undo();
        // Mirrored: pixel 0 moves to the outer column with the outer thumb.
        c.answer(Answer::Mirrored);
        assert_eq!(c.keys.as_ref().unwrap()[0][0].keys(), [0, 1, 2, 18]);
        // The saved map: the 6 live pixels a half, the dead ones unseen.
        assert_eq!(c.result().leds.len(), 12);
    }

    #[test]
    fn the_guess_is_a_column_snake_with_the_thumbs_last() {
        let d = corne().drawing.unwrap();
        let keys = serpentine(&d, 21).unwrap();
        assert_eq!(
            keys,
            [0, 1, 2, 5, 4, 3, 6, 7, 8, 11, 10, 9, 12, 13, 14, 17, 16, 15, 18, 19, 20]
        );
        assert!(fits(&Lamp::chain(&keys), &d, 21));
        // Six underglow LEDs on a 21-key half: no guess.
        assert!(serpentine(&d, 6).is_none());
    }

    #[test]
    fn each_correction_is_a_permutation_and_its_own_inverse() {
        let d = corne().drawing.unwrap();
        let shipped = PER_KEY.to_vec();
        for a in [Answer::Backwards, Answer::Mirrored, Answer::RowsFlipped] {
            let once = fix(&shipped, &d, a);
            assert!(fits(&Lamp::chain(&once), &d, 21), "{a:?}");
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
        let mut c = Confirm::new(&per_key()).unwrap();
        assert_eq!(c.source, Source::Profile { checked: false });
        assert!(c.correctable());
        let shipped = c.keys.as_ref().unwrap()[0].clone();
        let both = |k: Vec<Lamp>| Some(vec![k.clone(), k]);
        let fix = fix_lamps;
        // The keyboard's real chain is the shipped one mirrored and backwards: two answers.
        let real = fix(
            &fix(&shipped, &c.drawing, Answer::Mirrored),
            &c.drawing,
            Answer::Backwards,
        );
        c.answer(Answer::Mirrored);
        c.answer(Answer::Backwards);
        assert_eq!(c.keys, both(real));
        assert_eq!(
            fixes_text(&c.fixes, c.halves).as_deref(),
            Some("Mirrored, then turned round")
        );
        c.answer(Answer::Matches);
        assert_eq!(c.fixes.len(), 2, "Matches isn't a correction");
        c.undo();
        assert_eq!(c.keys, both(fix(&shipped, &c.drawing, Answer::Mirrored)));
        c.undo();
        assert_eq!(c.keys, both(shipped.clone()));
        c.undo();
        assert_eq!(c.keys, both(shipped), "nothing more to undo");
    }

    #[test]
    fn an_answer_for_one_half_turns_only_its_chain() {
        let mut c = Confirm::new(&per_key()).unwrap();
        let shipped = c.keys.as_ref().unwrap()[0].clone();
        let fix = fix_lamps;
        // The halves' dots ran differently: the right one ran backwards.
        c.half = Some(1);
        c.answer(Answer::Backwards);
        c.half = None;
        c.answer(Answer::Mirrored);
        let keys = c.keys.clone().unwrap();
        assert_eq!(keys[0], fix(&shipped, &c.drawing, Answer::Mirrored));
        assert_eq!(
            keys[1],
            fix(
                &fix(&shipped, &c.drawing, Answer::Backwards),
                &c.drawing,
                Answer::Mirrored
            )
        );
        assert_eq!(
            fixes_text(&c.fixes, c.halves).as_deref(),
            Some("The right half turned round, then mirrored")
        );
        // The right half's dot now starts where the left's ends; the left's is unchanged.
        let (w, _) = c.drawing.size();
        let left_last = c.point(0, 20).unwrap();
        let right_first = c.point(1, 0).unwrap();
        assert!((right_first[0] - (w - left_last[0])).abs() < 1e-3);
        assert_eq!(right_first[1], left_last[1]);
        // The saved map has each half where its own chain put it.
        let r = c.result();
        assert_eq!([r.leds[21].x, r.leds[21].y], right_first);
        // Undo takes back each answer for the halves it was for.
        c.undo();
        c.undo();
        assert_eq!(c.keys, Some(vec![shipped.clone(), shipped]));
        // A half past the board's (a stale pick) answers for every half.
        c.half = Some(5);
        c.answer(Answer::RowsFlipped);
        assert_eq!(c.fixes[0].half, None);
    }

    #[test]
    fn the_pace_is_slow_enough_for_the_relay_and_the_dot_runs_about_a_minute() {
        // Every pace leaves a split's other half (10 frames a second at most) four frames' time.
        for (_, ms) in PACES {
            assert!(ms >= 400, "{ms}");
        }
        assert!((600..=800).contains(&step_ms(DEFAULT_PACE)));
        assert!(step_ms(0) > step_ms(1) && step_ms(1) > step_ms(2));
        assert_eq!(step_ms(99), step_ms(DEFAULT_PACE));
        // The Corne's 21 a half at 0.7 s: 14.7 s a round, four rounds (the most).
        assert_eq!(rounds(21, 700), 4);
        // The second desk's 61 at 0.7 s: 43 s a round, still two.
        assert_eq!(rounds(61, 700), 2);
        assert_eq!(rounds(0, 0), 4);
    }

    #[test]
    fn the_second_desks_one_piece_board_gets_its_own_dot_test() {
        use crate::foxglove::fixtures::second_desk;
        let mut c = Confirm::new(&second_desk::board()).unwrap();
        assert_eq!((c.halves, c.per_half), (1, second_desk::LAMPS));
        assert!(c.correctable());
        assert_eq!(c.keys.as_ref().map(Vec::len), Some(1), "one chain");
        let frames = c.frames();
        assert_eq!(frames.len(), second_desk::LAMPS);
        assert!(frames.iter().all(|f| f.len() == second_desk::LAMPS));
        // A one-piece board's answers are never for a half, whatever was picked.
        c.half = Some(1);
        c.answer(Answer::Mirrored);
        assert_eq!(c.fixes[0].half, None);
        assert_eq!(fixes_text(&c.fixes, c.halves).as_deref(), Some("Mirrored"));
        assert_eq!(c.result().leds.len(), second_desk::LAMPS);
    }

    #[test]
    fn the_dot_lights_the_same_led_on_both_halves_and_the_result_is_key_centres() {
        let c = Confirm::new(&per_key()).unwrap();
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
        assert_eq!(
            c.keys,
            serpentine(&corne().drawing.unwrap(), 21).map(|k| vec![Lamp::chain(&k); 2])
        );
        // A broken profile map (a key twice, or every lamp dead) falls back to the guess too.
        let l = board.lighting.as_mut().unwrap();
        l.led_keys = Lamp::chain(&[0; 21]);
        assert_eq!(Confirm::new(&board).unwrap().source, Source::Guess);
        let l = board.lighting.as_mut().unwrap();
        l.led_keys = vec![Lamp::default(); 21];
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
