//! The calibrator's A/B choices (#474): two candidates either side of where a match stands, a
//! fixed Oklab distance apart, and the user picks the one closer to the reference or says they
//! match. Each choice halves the bracket around the picked side: the match moves half a step
//! toward the picked candidate and the next pair sits half as far either side of it, so a colour
//! settles in a few choices, and every step looks about the same size in dark blues as in bright
//! yellows.
//!
//! Each question changes one perceptual thing (an axis): warm or cool and green or pink for a
//! white or grey, hue and strength (Oklch chroma) for a colour, brightness for a colour that
//! lights one channel only. Candidates never light a channel the test colour leaves off: a
//! match says nothing about a channel its colour doesn't use.
//!
//! **Stop rule.** It stops when the user says they match, when every axis's step has fallen
//! below a just-noticeable difference ([`oklab::JND`]), or after [`MAX_ROUNDS`] choices. With
//! [`START_STEP`] 0.08 that is three choices an axis (0.08, 0.04, 0.02; then 0.01 is below
//! one): six for a white, a grey or a mixed colour, three for red, green or blue. A match can
//! move up to 0.07 on an axis (0.04 + 0.02 + 0.01) in one round of choices ("Ask again" goes on
//! from there) and ends within 0.01 of where the user's choices put it.

use crate::oklab::{self, JND};

/// The first step on each axis (Oklab ΔE from where the match stands to either candidate):
/// about four just-noticeable differences, so the first choice is an easy one.
pub const START_STEP: f32 = 0.08;

/// Choices an axis gets: the step halves after each one and the axis ends below [`JND`].
pub const PER_AXIS: u8 = per_axis();

/// The most choices for one colour, whatever happens.
pub const MAX_ROUNDS: u8 = 8;

const fn per_axis() -> u8 {
    let mut n = 0u8;
    let mut step = START_STEP;
    while step >= JND - 1e-6 {
        step /= 2.0;
        n += 1;
    }
    n
}

/// How far from grey a test colour can be and still be asked as a white (warm or cool, green
/// or pink) rather than by hue and strength.
const GREY_CHROMA: f32 = 0.03;

/// What one choice changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Oklab b: blue to yellow.
    WarmCool,
    /// Oklab a: green to magenta.
    Tint,
    /// Around the Oklch hue circle.
    Hue,
    /// Oklch chroma: paler or stronger.
    Chroma,
    /// Oklab lightness.
    Lightness,
}

impl Axis {
    /// In words, for the question's caption.
    pub fn name(self) -> &'static str {
        match self {
            Axis::WarmCool => "warm or cool",
            Axis::Tint => "green or pink",
            Axis::Hue => "hue",
            Axis::Chroma => "strength",
            Axis::Lightness => "brightness",
        }
    }
}

/// One of the two candidates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    #[default]
    A,
    B,
}

/// The user's answer to one choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// This one looks closer to the reference.
    Closer(Pick),
    /// They match: neither looks closer.
    Same,
}

/// Why it stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The user said they match.
    Same,
    /// Every axis's step fell below a just-noticeable difference.
    Settled,
    /// [`MAX_ROUNDS`] choices.
    Rounds,
}

#[derive(Clone, Debug, PartialEq)]
struct Line {
    axis: Axis,
    /// A unit direction in Oklab.
    dir: [f32; 3],
    asked: u8,
    /// Its two candidates come out the same in 8 bits: nothing to ask.
    flat: bool,
}

impl Line {
    fn step(&self) -> f32 {
        START_STEP / f32::from(1u16 << self.asked)
    }

    fn open(&self) -> bool {
        !self.flat && self.asked < PER_AXIS
    }
}

/// The A/B search for one side of one test colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Bracket {
    /// The test colour: which channels candidates may light.
    color: [u8; 3],
    /// Where it started (8-bit sRGB).
    start: [u8; 3],
    /// Where the match stands now, in Oklab (kept unrounded between choices).
    center: [f32; 3],
    lines: Vec<Line>,
    /// The axis being asked.
    turn: usize,
    rounds: u8,
    stop: Option<Stop>,
}

impl Bracket {
    /// Starts at `start` (one side's value) for the test colour `color`.
    pub fn new(color: [u8; 3], start: [u8; 3]) -> Self {
        let lab = oklab::from_srgb(color);
        let c = oklab::chroma(lab);
        let lit = color.iter().filter(|v| **v > 0).count();
        let (hue, chroma) = if c > 1e-6 {
            (
                [0.0, -lab[2] / c, lab[1] / c],
                [0.0, lab[1] / c, lab[2] / c],
            )
        } else {
            ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
        };
        let axes: Vec<(Axis, [f32; 3])> = match lit {
            0 => Vec::new(),
            1 => vec![(Axis::Lightness, [1.0, 0.0, 0.0])],
            2 => vec![(Axis::Hue, hue), (Axis::Lightness, [1.0, 0.0, 0.0])],
            _ if c < GREY_CHROMA => vec![
                (Axis::WarmCool, [0.0, 0.0, 1.0]),
                (Axis::Tint, [0.0, 1.0, 0.0]),
            ],
            _ => vec![(Axis::Hue, hue), (Axis::Chroma, chroma)],
        };
        let mut b = Self {
            color,
            start,
            center: oklab::from_srgb(start),
            lines: axes
                .into_iter()
                .map(|(axis, dir)| Line {
                    axis,
                    dir,
                    asked: 0,
                    flat: false,
                })
                .collect(),
            turn: 0,
            rounds: 0,
            stop: None,
        };
        b.advance(0);
        b
    }

    /// An Oklab point as 8-bit sRGB, with the channels the test colour leaves off kept off.
    fn quantize(&self, lab: [f32; 3]) -> [u8; 3] {
        let mut lin = oklab::to_linear(lab);
        for (v, c) in lin.iter_mut().zip(self.color) {
            if c == 0 {
                *v = 0.0;
            }
        }
        oklab::linear_to_srgb8(lin)
    }

    fn at(&self, line: &Line, sign: f32) -> [u8; 3] {
        let s = sign * line.step();
        self.quantize([
            self.center[0] + s * line.dir[0],
            self.center[1] + s * line.dir[1],
            self.center[2] + s * line.dir[2],
        ])
    }

    /// Asks the next open axis from `from` on (round-robin), skipping any whose candidates
    /// come out the same; stops when none is left.
    fn advance(&mut self, from: usize) {
        let n = self.lines.len();
        for k in 0..n {
            let i = (from + k) % n;
            if !self.lines[i].open() {
                continue;
            }
            let line = &self.lines[i];
            if self.at(line, -1.0) == self.at(line, 1.0) {
                self.lines[i].flat = true;
                continue;
            }
            self.turn = i;
            return;
        }
        self.stop = Some(Stop::Settled);
    }

    /// The two candidates on offer, `None` once it stopped.
    pub fn pair(&self) -> Option<([u8; 3], [u8; 3])> {
        if self.stop.is_some() {
            return None;
        }
        let line = &self.lines[self.turn];
        Some((self.at(line, -1.0), self.at(line, 1.0)))
    }

    /// One candidate, `None` once it stopped.
    pub fn candidate(&self, pick: Pick) -> Option<[u8; 3]> {
        self.pair().map(|(a, b)| match pick {
            Pick::A => a,
            Pick::B => b,
        })
    }

    /// The axis being asked, `None` once it stopped.
    pub fn axis(&self) -> Option<Axis> {
        self.stop.is_none().then(|| self.lines[self.turn].axis)
    }

    /// The step being asked: the Oklab ΔE from where the match stands to either candidate.
    pub fn step(&self) -> Option<f32> {
        self.stop.is_none().then(|| self.lines[self.turn].step())
    }

    /// Takes the user's answer: a pick moves the match half a step toward that candidate and
    /// halves the step (the bracket halves around the picked side); they match stops there.
    pub fn answer(&mut self, answer: Answer) {
        if self.stop.is_some() {
            return;
        }
        self.rounds += 1;
        let sign = match answer {
            Answer::Same => {
                self.stop = Some(Stop::Same);
                return;
            }
            Answer::Closer(Pick::A) => -1.0,
            Answer::Closer(Pick::B) => 1.0,
        };
        let line = &mut self.lines[self.turn];
        let s = sign * line.step() / 2.0;
        for (c, d) in self.center.iter_mut().zip(line.dir) {
            *c += s * d;
        }
        line.asked += 1;
        if self.rounds >= MAX_ROUNDS {
            self.stop = Some(Stop::Rounds);
            return;
        }
        self.advance(self.turn + 1);
    }

    /// Why it stopped, `None` while it's asking.
    pub fn stop(&self) -> Option<Stop> {
        self.stop
    }

    /// Choices answered so far.
    pub fn rounds(&self) -> u8 {
        self.rounds
    }

    /// How many choices this colour takes if none is "they match": the ones answered and the
    /// ones still open, at most [`MAX_ROUNDS`].
    pub fn planned(&self) -> u8 {
        if self.stop.is_some() {
            return self.rounds;
        }
        let open: u8 = self
            .lines
            .iter()
            .filter(|l| !l.flat)
            .map(|l| PER_AXIS - l.asked.min(PER_AXIS))
            .sum();
        (self.rounds + open).min(MAX_ROUNDS)
    }

    /// Where the match stands (8-bit sRGB): the value it ends with.
    pub fn result(&self) -> [u8; 3] {
        self.quantize(self.center)
    }

    /// Where it started.
    pub fn start(&self) -> [u8; 3] {
        self.start
    }

    /// How far the match has moved from where it started (Oklab ΔE).
    pub fn moved(&self) -> f32 {
        oklab::delta_e(self.start, self.result())
    }

    /// How far off the match can still be (Oklab ΔE): the largest step left on any axis. Below
    /// [`JND`] once it settled.
    pub fn left(&self) -> f32 {
        self.lines
            .iter()
            .filter(|l| !l.flat)
            .map(Line::step)
            .fold(0.0, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user who sees exactly: picks the candidate nearer `target` in Oklab, or says they
    /// match when the two are equally near (within a tenth of a just-noticeable difference).
    fn observe(b: &mut Bracket, target: [u8; 3]) {
        let t = oklab::from_srgb(target);
        while let Some((a, bb)) = b.pair() {
            let da = oklab::distance(oklab::from_srgb(a), t);
            let db = oklab::distance(oklab::from_srgb(bb), t);
            b.answer(if (da - db).abs() < JND / 10.0 {
                Answer::Same
            } else if da < db {
                Answer::Closer(Pick::A)
            } else {
                Answer::Closer(Pick::B)
            });
            assert!(b.rounds() <= MAX_ROUNDS);
        }
    }

    #[test]
    fn three_choices_an_axis_and_never_more_than_eight() {
        assert_eq!(PER_AXIS, 3, "0.08, 0.04, 0.02, then 0.01 is below one JND");
        let white = Bracket::new([153; 3], [153; 3]);
        assert_eq!(white.axis(), Some(Axis::WarmCool));
        assert_eq!(white.planned(), 6);
        let red = Bracket::new([153, 0, 0], [153, 0, 0]);
        assert_eq!((red.axis(), red.planned()), (Some(Axis::Lightness), 3));
        let skin = Bracket::new([153, 112, 92], [153, 112, 92]);
        assert_eq!((skin.axis(), skin.planned()), (Some(Axis::Hue), 6));
        // A user who always picks A: every axis runs out, at six.
        let mut b = white.clone();
        while b.pair().is_some() {
            b.answer(Answer::Closer(Pick::A));
        }
        assert_eq!((b.stop(), b.rounds()), (Some(Stop::Settled), 6));
        assert!(b.left() < JND, "{}", b.left());
    }

    #[test]
    fn candidates_sit_one_step_either_side_in_oklab() {
        let b = Bracket::new([153; 3], [153; 3]);
        let (a, bb) = b.pair().unwrap();
        let c = [153; 3];
        for x in [a, bb] {
            let d = oklab::delta_e(c, x);
            assert!((d - START_STEP).abs() < 0.006, "{x:?}: {d}");
        }
        // Warm or cool: A is the bluer, B the yellower.
        assert!(a[2] > bb[2] && a[0] < bb[0], "{a:?} {bb:?}");
        // The step looks the same size on a dark blue.
        let blue = Bracket::new([0, 0, 153], [0, 0, 153]);
        let (a, bb) = blue.pair().unwrap();
        assert_eq!((a[0], a[1], bb[0], bb[1]), (0, 0, 0, 0), "unlit stays off");
        let d = oklab::delta_e(a, bb);
        assert!(d > START_STEP, "{a:?} {bb:?}: {d}");
    }

    #[test]
    fn bracketing_finds_an_off_white_within_one_jnd() {
        // Whites at the test colour's lightness: warmer, cooler and greener, pinker, as is.
        let white = oklab::from_srgb([153; 3]);
        for (da, db) in [
            (0.0, 0.04),
            (-0.03, -0.05),
            (0.06, 0.02),
            (-0.06, 0.05),
            (0.0, 0.0),
        ] {
            let target = oklab::to_srgb([white[0], white[1] + da, white[2] + db]);
            let mut b = Bracket::new([153; 3], [153; 3]);
            observe(&mut b, target);
            let off = oklab::delta_e(b.result(), target);
            assert!(off < JND, "{target:?}: {:?} is {off} off", b.result());
            assert!(b.rounds() <= 6);
        }
    }

    #[test]
    fn every_test_colour_settles_and_keeps_its_unlit_channels_off() {
        let colours = [
            [153, 153, 153],
            [115, 115, 115],
            [153, 0, 0],
            [0, 153, 0],
            [0, 0, 153],
            [153, 112, 92],
            [153, 97, 38],
            [153, 77, 0],
        ];
        for color in colours {
            // Somewhere a little off, as a real device is.
            let target = color.map(|v| if v == 0 { 0 } else { v - v / 12 });
            let mut b = Bracket::new(color, color);
            let t = oklab::from_srgb(target);
            while let Some((a, bb)) = b.pair() {
                for x in [a, bb] {
                    for i in 0..3 {
                        assert!(color[i] > 0 || x[i] == 0, "{color:?}: {x:?}");
                    }
                }
                let closer = oklab::distance(oklab::from_srgb(a), t)
                    <= oklab::distance(oklab::from_srgb(bb), t);
                b.answer(Answer::Closer(if closer { Pick::A } else { Pick::B }));
            }
            assert!(b.rounds() <= MAX_ROUNDS && b.rounds() >= 3, "{color:?}");
            // Two-way report: what's left between the match and the target, in ΔE (for a
            // colour that lights every channel, in hue and strength: its brightness isn't asked).
            let (r, t) = (oklab::from_srgb(b.result()), oklab::from_srgb(target));
            let off = if color.iter().all(|v| *v > 0) {
                oklab::distance([0.0, r[1], r[2]], [0.0, t[1], t[2]])
            } else {
                oklab::distance(r, t)
            };
            assert!(
                off < 2.0 * JND,
                "{color:?}: {:?} vs {target:?}: {off}",
                b.result()
            );
        }
    }

    #[test]
    fn they_match_stops_at_once_and_black_has_nothing_to_ask() {
        let mut b = Bracket::new([153; 3], [153, 150, 140]);
        b.answer(Answer::Closer(Pick::B));
        b.answer(Answer::Same);
        assert_eq!(
            (b.stop(), b.rounds(), b.pair()),
            (Some(Stop::Same), 2, None)
        );
        assert!(b.moved() > JND, "it moved one step");
        b.answer(Answer::Closer(Pick::A));
        assert_eq!(b.rounds(), 2, "answers after it stopped change nothing");
        let black = Bracket::new([0; 3], [0; 3]);
        assert_eq!(
            (black.stop(), black.pair(), black.planned()),
            (Some(Stop::Settled), None, 0)
        );
    }
}
