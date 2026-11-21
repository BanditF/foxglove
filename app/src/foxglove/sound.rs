//! Sound, a lighting effect whose brightness follows how loud the PC is playing (the plan:
//! review/ux-plan-sound-effect.md, slice 1): the pure core. The mapping from the output
//! device's peak to a level, the Smoothing envelope, the colour for a level and the flash limit
//! over it, and the saved key's default. Nothing here reads a meter, draws a control or sends a
//! colour.
//!
//! The rules the code keeps:
//! - One number comes in: the default output device's peak (0-1), read by someone else. No
//!   samples, no spectrum, never the microphone.
//! - The lights never go dark: quiet is the [`FLOOR`], a steady colour, and a missing reading
//!   (`None`: nothing playing, no device) is the floor too, never a made-up level.
//! - At most [`MAX_SWINGS`] brightness swings a second (WCAG 2.3.1), whatever the sound does:
//!   the screen's own [`FlashLimit`] runs after the envelope and can't be switched off.
//! - The clock is passed in (milliseconds), so tests need no time.

use foxglove_scene::screen::FlashLimit;
#[cfg(test)]
use foxglove_scene::screen::{luminance, MAX_SWINGS, SWING};
use foxglove_scene::Rgb;
use serde::{Deserialize, Serialize};

use super::palette_file;
use super::scene_looks;

/// The effect's name on the Scenes page.
pub const NAME: &str = "Sound";
/// What the pill's hover says.
pub const DESCRIPTION: &str =
    "Your lights get brighter and dimmer with the sound your PC is playing.";
/// What it follows, on the card.
pub const FOLLOWS: &str = "What this PC is playing (speakers or headset)";
/// The card's privacy line, always shown.
pub const PRIVACY: &str =
    "Foxglove only reads how loud your PC's sound is. It records nothing and never uses the microphone.";

/// The brightness at silence, of full: the lights never go dark, and no swing is larger than
/// the rest.
pub const FLOOR: f32 = 0.35;
/// The peak, in decibels below full scale, that is "quiet" (level 0) and "loud" (level 1).
pub const QUIET_DB: f32 = -40.0;
pub const LOUD_DB: f32 = -6.0;

/// How quickly the lights follow the sound: the card's Smoothing (Screen glow's word; its own
/// type, since the two effects' steps differ).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Smooth {
    /// Follows a song's loudness, not every drum hit.
    #[default]
    Calm,
    /// Follows the beat (within the flash limit).
    Quick,
}

/// The hint under Smoothing.
pub const SMOOTHING_HINT: &str = "Calm follows a song's loudness. Quick follows the beat.";

/// How long the level takes to cross its whole range, up and down (ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    pub rise_ms: u32,
    pub fall_ms: u32,
}

/// With Windows' animation effects off: the loudness of a song, never its beat.
pub const REDUCED: Times = Times {
    rise_ms: 1_000,
    fall_ms: 2_000,
};

impl Smooth {
    pub const ALL: [Smooth; 2] = [Smooth::Calm, Smooth::Quick];

    pub fn label(self) -> &'static str {
        match self {
            Smooth::Calm => "Calm",
            Smooth::Quick => "Quick",
        }
    }

    /// Its times; with `reduced_motion` both act as [`REDUCED`].
    pub fn times(self, reduced_motion: bool) -> Times {
        if reduced_motion {
            return REDUCED;
        }
        match self {
            Smooth::Calm => Times {
                rise_ms: 150,
                fall_ms: 900,
            },
            Smooth::Quick => Times {
                rise_ms: 40,
                fall_ms: 350,
            },
        }
    }
}

/// The hint under Smoothing while Windows' animation effects are off (Quick is disabled).
pub const REDUCED_HINT: &str = "Animation effects are off in Windows, so Sound stays calm.";

/// A peak (0-1, linear amplitude) as a level: 0 at and below [`QUIET_DB`], 1 at and above
/// [`LOUD_DB`], even in decibels between. Anything that isn't a number, or is zero, is 0.
pub fn level(peak: f32) -> f32 {
    if !peak.is_finite() || peak <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * peak.min(1.0).log10();
    ((db - QUIET_DB) / (LOUD_DB - QUIET_DB)).clamp(0.0, 1.0)
}

/// The level as the lights follow it: rising and falling no faster than its [`Times`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Envelope {
    value: f32,
    at: Option<i64>,
}

impl Envelope {
    /// The eased level at `t_ms` on the way to `target` (0-1).
    pub fn step(&mut self, target: f32, t_ms: i64, times: Times) -> f32 {
        let target = if target.is_finite() {
            target.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dt = self.at.map_or(0, |at| (t_ms - at).max(0)) as f32;
        self.at = Some(t_ms);
        let delta = target - self.value;
        let over = if delta > 0.0 {
            times.rise_ms
        } else {
            times.fall_ms
        };
        let most = if over == 0 { 1.0 } else { dt / over as f32 };
        self.value += delta.clamp(-most, most);
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    /// Whether it has reached `target` (nothing more to draw until the sound changes).
    pub fn settled(&self, target: f32) -> bool {
        (self.value - target.clamp(0.0, 1.0)).abs() < 1.0 / 512.0
    }
}

/// The two colours a look runs between: at the floor, and at full level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colours {
    pub quiet: Rgb,
    pub loud: Rgb,
}

/// The palette's colours for Sound: its main colour, leaning to a harmony palette's second
/// accent as it gets louder; a palette without roles (a wallpaper's, Foxglove's own) runs from
/// its first stop to its last. Bandit: orange, both ends.
pub fn colours(palette: &str) -> Colours {
    match palette_file::by_name(palette) {
        Some(p) => Colours {
            quiet: p.roles.accent,
            loud: p.roles.second(),
        },
        None => {
            let stops = scene_looks::palette(palette);
            let bandit = Rgb(palette_file::bandit().roles.accent.0);
            Colours {
                quiet: stops.first().copied().unwrap_or(bandit),
                loud: stops.last().copied().unwrap_or(bandit),
            }
        }
    }
}

/// The colour at an eased level (0-1): between the two colours, at a brightness from
/// [`FLOOR`] to full.
pub fn colour_at(level: f32, colours: Colours) -> Rgb {
    let level = if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let hue = colours.quiet.mix(colours.loud, level);
    let bright = FLOOR + (1.0 - FLOOR) * level;
    let [r, g, b] = hue.linear();
    Rgb::from_linear([r * bright, g * bright, b * bright])
}

/// One light's colour from the peaks as they come: level, envelope, colour, flash limit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaper {
    envelope: Envelope,
    limit: FlashLimit,
}

impl Shaper {
    /// The colour to show at `t_ms`. `peak` is the device's peak now; `None` (nothing playing,
    /// no device, the read failed) is silence: the lights settle at the floor.
    pub fn colour(&mut self, peak: Option<f32>, t_ms: i64, times: Times, colours: Colours) -> Rgb {
        let eased = self.envelope.step(peak.map_or(0.0, level), t_ms, times);
        self.limit.apply(colour_at(eased, colours), t_ms)
    }

    /// The eased level now (0-1).
    pub fn level(&self) -> f32 {
        self.envelope.value()
    }

    /// Whether the colour will stay as it is while the peak stays `peak`: the frame thread may
    /// sleep until the next read.
    pub fn still(&self, peak: Option<f32>) -> bool {
        self.envelope.settled(peak.map_or(0.0, level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALM: Times = Times {
        rise_ms: 150,
        fall_ms: 900,
    };

    fn bandit() -> Colours {
        colours("Bandit")
    }

    #[test]
    fn defaults_and_words_are_the_plans() {
        assert_eq!(Smooth::default(), Smooth::Calm);
        assert_eq!(NAME, "Sound");
        assert_eq!(Smooth::Calm.times(false), CALM);
        assert_eq!(
            Smooth::Quick.times(false),
            Times {
                rise_ms: 40,
                fall_ms: 350
            }
        );
        for r in Smooth::ALL {
            assert_eq!(r.times(true), REDUCED, "{r:?} with animations off");
        }
        for text in [
            NAME,
            DESCRIPTION,
            FOLLOWS,
            PRIVACY,
            SMOOTHING_HINT,
            REDUCED_HINT,
        ] {
            let low = text.to_lowercase();
            for word in [
                "endpoint", "render", "session", "peak", "meter", "db", "loopback", "fact", "poll",
                "%",
            ] {
                assert!(
                    !low.split(|c: char| !c.is_alphanumeric() && c != '%')
                        .any(|w| w == word),
                    "{word:?} in {text:?}"
                );
            }
        }
        assert!(PRIVACY.contains("never uses the microphone"));
    }

    #[test]
    fn the_level_runs_from_quiet_to_loud_in_decibels() {
        let amp = |db: f32| 10f32.powf(db / 20.0);
        assert_eq!(level(0.0), 0.0);
        assert_eq!(level(-1.0), 0.0);
        assert_eq!(level(f32::NAN), 0.0);
        assert_eq!(level(f32::INFINITY), 0.0);
        assert_eq!(level(amp(-60.0)), 0.0);
        assert!(level(amp(QUIET_DB)).abs() < 1e-4);
        assert!((level(amp(LOUD_DB)) - 1.0).abs() < 1e-4);
        assert_eq!(level(1.0), 1.0);
        assert_eq!(level(7.0), 1.0);
        assert!((level(amp(-23.0)) - 0.5).abs() < 1e-3);
        let mut last = 0.0;
        for i in 0..=100 {
            let l = level(i as f32 / 100.0);
            assert!(l >= last, "not rising at {i}");
            last = l;
        }
    }

    #[test]
    fn the_envelope_rises_and_falls_in_its_times() {
        let mut e = Envelope::default();
        assert_eq!(
            e.step(1.0, 0, CALM),
            0.0,
            "the first step has no time behind it"
        );
        assert!((e.step(1.0, 75, CALM) - 0.5).abs() < 1e-3);
        assert!((e.step(1.0, 150, CALM) - 1.0).abs() < 1e-3);
        assert_eq!(e.step(1.0, 5_000, CALM), 1.0);
        assert!(e.settled(1.0));
        assert!((e.step(0.0, 5_450, CALM) - 0.5).abs() < 1e-3);
        assert!(!e.settled(0.0));
        assert_eq!(e.step(0.0, 5_900, CALM), 0.0);
        assert!(e.settled(0.0));
        // A clock that steps back moves nothing; a target that isn't a number is silence.
        let mut e = Envelope::default();
        e.step(1.0, 1_000, CALM);
        assert_eq!(e.step(1.0, 900, CALM), 0.0);
        e.step(1.0, 2_000, CALM);
        assert!(e.step(f32::NAN, 2_090, CALM) < 1.0);
        // With animations off it is slower both ways.
        let mut e = Envelope::default();
        e.step(1.0, 0, REDUCED);
        assert!((e.step(1.0, 500, REDUCED) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn the_colour_never_goes_below_the_floor_and_bandit_stays_orange() {
        let c = bandit();
        let accent = palette_file::bandit().roles.accent;
        assert_eq!(c.quiet, accent);
        assert_eq!(c.loud, accent, "Bandit has one accent: no lean");
        assert_eq!(colour_at(1.0, c), accent);
        assert_eq!(colour_at(9.0, c), accent);
        let floor = colour_at(0.0, c);
        assert_eq!(colour_at(-3.0, c), floor);
        assert_eq!(colour_at(f32::NAN, c), floor);
        let want = FLOOR * luminance(accent);
        assert!((luminance(floor) - want).abs() < 0.01, "{floor:?}");
        assert!(luminance(floor) > 0.0);
        let mut last = 0.0;
        for i in 0..=20 {
            let y = luminance(colour_at(i as f32 / 20.0, c));
            assert!(y >= last - 1e-4, "not brighter at {i}");
            last = y;
        }
    }

    #[test]
    fn a_palette_without_roles_runs_from_its_first_stop_to_its_last() {
        // A name nobody knows falls back inside `scene_looks::palette`; whatever it gives, both
        // ends are colours and the floor is lit.
        let c = colours("no such palette");
        assert!(luminance(colour_at(0.0, c)) > 0.0);
        assert!(luminance(colour_at(1.0, c)) > 0.0);
    }

    #[test]
    fn silence_and_no_reading_settle_at_the_floor() {
        let c = bandit();
        let floor = colour_at(0.0, c);
        let mut s = Shaper::default();
        let mut t = 0;
        for _ in 0..20 {
            s.colour(Some(1.0), t, CALM, c);
            t += 50;
        }
        assert!(!s.still(None));
        let mut shown = floor;
        for _ in 0..40 {
            shown = s.colour(None, t, CALM, c);
            t += 50;
        }
        assert_eq!(shown, floor);
        assert!(s.still(None) && s.still(Some(0.0)));
        assert!(!s.still(Some(1.0)));
    }

    /// The swings of a colour sequence, counted apart from `FlashLimit`: turns in luminance of
    /// at least [`SWING`], and the most of them in any second.
    fn most_swings_in_a_second(shown: &[(i64, Rgb)]) -> usize {
        let mut swings = Vec::new();
        let mut dir = 0i8;
        let mut extreme = luminance(shown[0].1);
        for &(t, c) in &shown[1..] {
            let y = luminance(c);
            let delta = y - extreme;
            let way = if delta > 0.0 { 1 } else { -1 };
            if dir == 0 || way == dir {
                if dir == 0 && delta.abs() >= SWING {
                    dir = way;
                }
                if dir == way && (way > 0 && y > extreme || way < 0 && y < extreme) {
                    extreme = y;
                }
            } else if delta.abs() >= SWING {
                swings.push(t);
                dir = way;
                extreme = y;
            }
        }
        swings
            .iter()
            .map(|&s| swings.iter().filter(|&&o| o >= s && o - s < 1000).count())
            .max()
            .unwrap_or(0)
    }

    /// Whatever the sound does, no light swings more than three times in any second.
    #[test]
    fn no_sound_makes_more_than_three_swings_a_second() {
        // A small fixed generator: the test needs no crate and repeats exactly.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        let white = Colours {
            quiet: Rgb([255, 255, 255]),
            loud: Rgb([255, 255, 255]),
        };
        // A palette that leans from a dark colour to a bright one: the limit counts the colour
        // sent, lean included.
        let lean = Colours {
            quiet: Rgb([0x10, 0x20, 0x90]),
            loud: Rgb([0xff, 0xf0, 0x40]),
        };
        for (name, smooth, c) in [
            ("calm bandit", Smooth::Calm, bandit()),
            ("quick bandit", Smooth::Quick, bandit()),
            ("quick white", Smooth::Quick, white),
            ("quick lean", Smooth::Quick, lean),
        ] {
            let times = smooth.times(false);
            // A square wave at 10 Hz (the worst a beat can do), then noise, then gaps.
            let mut shown = Vec::new();
            let mut s = Shaper::default();
            for i in 0..600i64 {
                let t = i * 50;
                let peak = match i {
                    0..=199 => Some(if i % 2 == 0 { 1.0 } else { 0.0 }),
                    200..=399 => Some(next()),
                    _ => (next() > 0.5).then_some(1.0),
                };
                shown.push((t, s.colour(peak, t, times, c)));
            }
            let most = most_swings_in_a_second(&shown);
            assert!(most <= MAX_SWINGS, "{name}: {most} swings in a second");
            // And it does move: the limit didn't freeze it.
            let ys: Vec<f32> = shown.iter().map(|(_, c)| luminance(*c)).collect();
            let (lo, hi) = ys
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
            assert!(hi - lo > 0.05, "{name}: the lights never moved");
        }
    }
}
