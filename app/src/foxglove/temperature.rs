//! Temperature, a lighting effect that shows how hot a part of the PC is (the plan:
//! review/ux-plan-gpu-temp-effect.md, slice 1): the pure core. The mapping from a reading to a
//! colour, the words for it, the saved keys' defaults and the fixed Cool to hot ramp. Nothing
//! here reads a sensor, draws a control or sends a colour; the reading feed is
//! [`super::temperature_feed`], the Scenes page is slice 3 and the light output is
//! [`super::temperature_out`].
//!
//! The rules the code keeps:
//! - A missing reading is `None`, never 0, and never the cool colour (cool would say "cold"):
//!   [`colour_for`] and [`word`] answer `None` for it, and the caller shows the
//!   [`fallback`] look instead.
//! - Which temperature it follows is data (`temp_source`, a sensor group such as `gpu`); no
//!   vendor or card name is in the code.
//! - The Palette colours are the chosen palette's own (Bandit orange by default); Cool to hot
//!   is the effect's own fixed ramp, not a palette (docs/palette.md is untouched).
//!

use std::ops::RangeInclusive;

use foxglove_scene::Rgb;
use serde::{Deserialize, Serialize};

use super::palette_file;
use super::scene_looks;

/// The effect's name on the Scenes page.
pub const NAME: &str = "Temperature";
/// What the pill's hover says.
pub const DESCRIPTION: &str = "Your lights show how hot your graphics card is: cool colour when it's cool, hot colour when it's hot.";

/// The sensor group a new look follows: the facts `sensor.<group>.temp_c` and
/// `sensor.<group>.state`.
pub const DEFAULT_SOURCE: &str = "gpu";
/// Where the colour starts and ends, in whole degrees Celsius: an idle desktop card sits around
/// 30-45, a gaming load around 65-83.
pub const DEFAULT_COOL_C: u16 = 40;
pub const DEFAULT_HOT_C: u16 = 80;
/// What the two ends may be set to, in steps of [`STEP_C`], with [`MIN_GAP_C`] between them.
pub const RANGE_C: RangeInclusive<u16> = 20..=100;
#[allow(dead_code)] // the settings page (slice 3)
pub const STEP_C: u16 = 5;
pub const MIN_GAP_C: u16 = 10;
/// The colour eases to a new reading over this long, so a one-second reading step never
/// flickers (the frame scheduler sleeps once the ease ends).
pub const EASE_MS: u32 = 2_000;
/// The fallback look's loop: "running, no reading".
pub const FALLBACK_MS: u32 = 8_000;

/// Which colours the effect runs through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Colours {
    /// The chosen palette, from its calm end to its accent (Bandit: ember to bright orange).
    #[default]
    Palette,
    /// The fixed ramp [`COOL_TO_HOT`].
    CoolToHot,
}

impl Colours {
    pub const ALL: [Colours; 2] = [Colours::Palette, Colours::CoolToHot];

    pub fn label(self) -> &'static str {
        match self {
            Colours::Palette => "Palette",
            Colours::CoolToHot => "Cool to hot",
        }
    }
}

/// The Cool to hot ramp: blue, teal, amber, red.
pub const COOL_TO_HOT: [[u8; 3]; 4] = [
    [0x2e, 0x6b, 0xff],
    [0x1f, 0xb8, 0xa6],
    [0xff, 0xb3, 0x1f],
    [0xe8, 0x2d, 0x1f],
];

/// The fallback's colour with Cool to hot: a neutral warm white, so it is neither "cold" nor
/// "hot".
const FALLBACK_NEUTRAL: [u8; 3] = [0xff, 0xd6, 0xaa];

/// The ends within their ranges and [`MIN_GAP_C`] apart (a hand-edited file may hold anything).
/// Cool moves first, then hot clears it.
pub fn clamp_ends(cool: u16, hot: u16) -> (u16, u16) {
    let top = *RANGE_C.end();
    let cool = cool.clamp(*RANGE_C.start(), top - MIN_GAP_C);
    let hot = hot.clamp(cool + MIN_GAP_C, top);
    (cool, hot)
}

/// Where a reading sits between the ends, 0 (cool) to 1 (hot); below cool is 0, above hot is 1.
/// `None` for a reading that is not a number.
pub fn position(temp_c: f32, cool: u16, hot: u16) -> Option<f32> {
    if !temp_c.is_finite() {
        return None;
    }
    let (cool, hot) = clamp_ends(cool, hot);
    let (cool, hot) = (f32::from(cool), f32::from(hot));
    Some(((temp_c - cool) / (hot - cool)).clamp(0.0, 1.0))
}

/// The colours the effect runs through, cool end first. For [`Colours::Palette`]: a harmony
/// palette's second accent up to its accent; any other file palette (Bandit) a deep tone of its
/// accent up to the accent; a palette without roles (a wallpaper's, Foxglove's own) its stops
/// as they are.
pub fn ramp(colours: Colours, palette: &str) -> Vec<Rgb> {
    match colours {
        Colours::CoolToHot => COOL_TO_HOT.iter().map(|c| Rgb(*c)).collect(),
        Colours::Palette => match palette_file::by_name(palette) {
            Some(p) => {
                let r = p.roles;
                let calm = r.accent2.unwrap_or_else(|| r.accent.mix(r.base, 0.6));
                vec![calm, r.accent]
            }
            None => {
                let stops = scene_looks::palette(palette);
                if stops.len() < 2 {
                    // Never a one-stop ramp: it would show one colour for every reading.
                    let one = stops
                        .first()
                        .copied()
                        .unwrap_or(Rgb(palette_file::bandit().roles.accent.0));
                    vec![one.mix(Rgb([0, 0, 0]), 0.6), one]
                } else {
                    stops
                }
            }
        },
    }
}

/// The colour for a reading: its place between cool and hot along `ramp`. `None` when there is
/// no reading (never a colour for "nothing"), so the caller shows [`fallback`].
pub fn colour_for(temp_c: Option<f32>, cool: u16, hot: u16, ramp: &[Rgb]) -> Option<Rgb> {
    let at = position(temp_c?, cool, hot)?;
    Some(along(ramp, at))
}

/// The colour `at` (0..=1) along evenly spaced stops.
pub fn along(stops: &[Rgb], at: f32) -> Rgb {
    match stops {
        [] => Rgb([0, 0, 0]),
        [one] => *one,
        _ => {
            let at = at.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
            let i = (at.floor() as usize).min(stops.len() - 2);
            stops[i].mix(stops[i + 1], at - i as f32)
        }
    }
}

/// The look for "running, no reading": a slow breathe of this colour, the palette's main one
/// (Bandit orange by default), or a neutral warm white with Cool to hot.
pub fn fallback(colours: Colours, palette: &str) -> Rgb {
    match colours {
        Colours::CoolToHot => Rgb(FALLBACK_NEUTRAL),
        Colours::Palette => ramp(Colours::Palette, palette)
            .last()
            .copied()
            .unwrap_or(Rgb(FALLBACK_NEUTRAL)),
    }
}

/// How hot, in a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    Cool,
    Warm,
    Hot,
}

impl Word {
    pub fn text(self) -> &'static str {
        match self {
            Word::Cool => "cool",
            Word::Warm => "warm",
            Word::Hot => "hot",
        }
    }
}

/// The word for a reading: cool in the bottom third between the ends, warm in the middle, hot
/// in the top third. `None` without a reading.
pub fn word(temp_c: Option<f32>, cool: u16, hot: u16) -> Option<Word> {
    let at = position(temp_c?, cool, hot)?;
    Some(if at < 1.0 / 3.0 {
        Word::Cool
    } else if at < 2.0 / 3.0 {
        Word::Warm
    } else {
        Word::Hot
    })
}

/// A reading as written: whole degrees, a space before the unit ("64 °C").
pub fn degrees(temp_c: f32) -> String {
    format!("{}\u{a0}°C", temp_c.round() as i32)
}

/// The reading with its word ("64 °C, warm"); `None` without a reading, so "0 °C" is never
/// written for one.
pub fn summary(temp_c: Option<f32>, cool: u16, hot: u16) -> Option<String> {
    let t = temp_c.filter(|t| t.is_finite())?;
    Some(format!(
        "{}, {}",
        degrees(t),
        word(Some(t), cool, hot)?.text()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUE: Rgb = Rgb([0x2e, 0x6b, 0xff]);
    const RED: Rgb = Rgb([0xe8, 0x2d, 0x1f]);

    #[test]
    fn defaults_are_the_plans() {
        assert_eq!((DEFAULT_COOL_C, DEFAULT_HOT_C), (40, 80));
        assert_eq!(DEFAULT_SOURCE, "gpu");
        assert_eq!(Colours::default(), Colours::Palette);
        assert_eq!(*RANGE_C.start(), 20);
        assert_eq!(*RANGE_C.end(), 100);
    }

    #[test]
    fn ends_stay_in_range_and_ten_apart() {
        assert_eq!(clamp_ends(40, 80), (40, 80));
        assert_eq!(clamp_ends(40, 45), (40, 50));
        assert_eq!(clamp_ends(95, 100), (90, 100));
        assert_eq!(clamp_ends(0, 0), (20, 30));
        assert_eq!(clamp_ends(500, 500), (90, 100));
        for cool in 0..=130 {
            for hot in 0..=130 {
                let (c, h) = clamp_ends(cool, hot);
                assert!(RANGE_C.contains(&c) && RANGE_C.contains(&h));
                assert!(h >= c + MIN_GAP_C);
            }
        }
    }

    #[test]
    fn below_cool_is_the_cool_end_above_hot_the_hot_end() {
        let r = ramp(Colours::CoolToHot, "Bandit");
        assert_eq!(colour_for(Some(-20.0), 40, 80, &r), Some(BLUE));
        assert_eq!(colour_for(Some(40.0), 40, 80, &r), Some(BLUE));
        assert_eq!(colour_for(Some(80.0), 40, 80, &r), Some(RED));
        assert_eq!(colour_for(Some(250.0), 40, 80, &r), Some(RED));
        let mid = colour_for(Some(60.0), 40, 80, &r).unwrap();
        assert_ne!(mid, BLUE);
        assert_ne!(mid, RED);
    }

    #[test]
    fn the_midpoint_is_the_mix_of_the_middle_stops() {
        let r = ramp(Colours::CoolToHot, "Bandit");
        // Four stops: the middle of the range is half way between teal and amber.
        let want = Rgb(COOL_TO_HOT[1]).mix(Rgb(COOL_TO_HOT[2]), 0.5);
        assert_eq!(colour_for(Some(60.0), 40, 80, &r), Some(want));
        // A two-stop ramp: the midpoint is the mix.
        let two = [Rgb([0, 0, 0]), Rgb([200, 100, 50])];
        assert_eq!(
            colour_for(Some(60.0), 40, 80, &two),
            Some(two[0].mix(two[1], 0.5))
        );
    }

    #[test]
    fn cool_to_hot_runs_blue_teal_amber_red() {
        let r = ramp(Colours::CoolToHot, "anything");
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], BLUE);
        assert_eq!(r[3], RED);
        // At each stop's own place along the range.
        let at = |t: f32| colour_for(Some(t), 40, 80, &r).unwrap();
        assert_eq!(at(40.0 + 40.0 / 3.0), r[1]);
        assert_eq!(at(40.0 + 80.0 / 3.0), r[2]);
    }

    #[test]
    fn a_missing_reading_has_no_colour_and_no_word() {
        for colours in Colours::ALL {
            let r = ramp(colours, "Bandit");
            assert_eq!(colour_for(None, 40, 80, &r), None);
            assert_eq!(colour_for(Some(f32::NAN), 40, 80, &r), None);
            assert_eq!(colour_for(Some(f32::INFINITY), 40, 80, &r), None);
            assert_eq!(colour_for(Some(f32::NEG_INFINITY), 40, 80, &r), None);
        }
        assert_eq!(word(None, 40, 80), None);
        assert_eq!(word(Some(f32::NAN), 40, 80), None);
        assert_eq!(summary(None, 40, 80), None);
        // 0 degrees is a reading (very cold), not a missing one: it is the cool colour.
        let r = ramp(Colours::CoolToHot, "Bandit");
        assert_eq!(colour_for(Some(0.0), 40, 80, &r), Some(BLUE));
    }

    #[test]
    fn every_palette_has_a_ramp_that_moves() {
        // Bandit orange first (the default), every bundled palette, Foxglove's own, Wallpaper.
        let mut names: Vec<String> = scene_looks::PALETTES
            .iter()
            .map(|(n, _)| n.to_string())
            .collect();
        names.push(scene_looks::WALLPAPER.into());
        names.push("A wallpaper long gone".into());
        assert!(names.iter().any(|n| n == "Bandit"));
        for name in names {
            let r = ramp(Colours::Palette, &name);
            assert!(r.len() >= 2, "{name}");
            let cool = colour_for(Some(30.0), 40, 80, &r).unwrap();
            let hot = colour_for(Some(90.0), 40, 80, &r).unwrap();
            assert_ne!(cool, hot, "{name}: the ends must differ");
            assert_eq!(hot, *r.last().unwrap(), "{name}");
            assert_eq!(cool, r[0], "{name}");
        }
    }

    #[test]
    fn bandit_orange_is_the_default_palette_ramp() {
        let bandit = palette_file::bandit().roles;
        let r = ramp(Colours::Palette, "Bandit");
        assert_eq!(r.len(), 2);
        assert_eq!(
            *r.last().unwrap(),
            bandit.accent,
            "bright orange at the hot end"
        );
        // The cool end is a deeper, darker ember: never darker than the room's night.
        assert!(r[0] != bandit.accent);
        assert!(
            r[0].0.iter().map(|c| u32::from(*c)).sum::<u32>()
                < bandit.accent.0.iter().map(|c| u32::from(*c)).sum::<u32>()
        );
    }

    #[test]
    fn a_harmony_palette_cools_to_its_second_accent() {
        let dusk = palette_file::by_name("Bandit Dusk").expect("bundled");
        assert!(dusk.roles.is_harmony());
        let r = ramp(Colours::Palette, "Bandit Dusk");
        assert_eq!(r, vec![dusk.roles.accent2.unwrap(), dusk.roles.accent]);
    }

    #[test]
    fn the_cool_end_is_a_colour_not_dark() {
        for colours in Colours::ALL {
            let r = ramp(colours, "Bandit");
            let c = r[0].0;
            assert!(c.iter().any(|v| *v >= 0x30), "{colours:?}: {c:?}");
        }
    }

    #[test]
    fn words_split_the_range_in_thirds() {
        let w = |t: f32| word(Some(t), 40, 80).unwrap();
        assert_eq!(w(10.0), Word::Cool);
        assert_eq!(w(40.0), Word::Cool);
        assert_eq!(w(53.0), Word::Cool);
        assert_eq!(w(54.0), Word::Warm);
        assert_eq!(w(60.0), Word::Warm);
        assert_eq!(w(66.0), Word::Warm);
        assert_eq!(w(67.0), Word::Hot);
        assert_eq!(w(80.0), Word::Hot);
        assert_eq!(w(120.0), Word::Hot);
    }

    #[test]
    fn degrees_are_whole_with_a_space_before_the_unit() {
        assert_eq!(degrees(64.0), "64\u{a0}°C");
        assert_eq!(degrees(63.6), "64\u{a0}°C");
        assert_eq!(degrees(63.4), "63\u{a0}°C");
        assert_eq!(
            summary(Some(64.0), 40, 80).as_deref(),
            Some("64\u{a0}°C, warm")
        );
        assert_eq!(
            summary(Some(81.0), 40, 80).as_deref(),
            Some("81\u{a0}°C, hot")
        );
        assert_eq!(
            summary(Some(35.0), 40, 80).as_deref(),
            Some("35\u{a0}°C, cool")
        );
    }

    #[test]
    fn the_fallback_is_never_the_cool_end() {
        // Palette: the palette's main colour (Bandit orange), which is its hot end by design
        // (a breathe, not a reading); Cool to hot: neither blue nor red.
        assert_eq!(
            fallback(Colours::Palette, "Bandit"),
            palette_file::bandit().roles.accent
        );
        let f = fallback(Colours::CoolToHot, "Bandit");
        assert_ne!(f, BLUE);
        assert_ne!(f, RED);
        assert_eq!(f, Rgb(FALLBACK_NEUTRAL));
    }

    #[test]
    fn the_copy_avoids_the_words_the_plan_never_shows() {
        let all = [
            NAME,
            DESCRIPTION,
            Colours::Palette.label(),
            Colours::CoolToHot.label(),
        ];
        for text in all {
            for bad in [
                "sensor.",
                "fact",
                "subscribe",
                "demand",
                "module",
                "plugin",
                "NVML",
                "GPU",
                "CPU",
                "unavailable",
                "unknown",
                "0\u{a0}°C",
                "0 °C",
            ] {
                assert!(!text.contains(bad), "{text:?} has {bad:?}");
            }
        }
    }
}
