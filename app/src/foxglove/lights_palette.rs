//! The Lights page's palette (#871 P2, docs/palette.md "Harmony palettes"): which bundled
//! palette the Keyboard card offers. **Bandit** is the default and changes nothing: the card's
//! swatches and the keyboard's lights stay as they were. A harmony palette adds its second and
//! third accents to the swatches and offers its slow effects, Two-tone and Sweep, which run as
//! a scene (`scenes_ui::start_look`), so the Scenes page, ripples on key press and the
//! keyboard's own effect path (SET_EFFECT, else frames) all apply as for any scene.
//!
//! Saved in the store's `[lights_palette]` section, read when the page is drawn (no timer).

use serde::{Deserialize, Serialize};

use super::palette_file::{self, PaletteFile, Tier};
use super::scene_looks::{Kind, Look};

/// The store section.
pub const SECTION: &str = "lights_palette";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    name: String,
}

/// The chosen palette: a bundled one, else Bandit.
pub fn chosen() -> &'static PaletteFile {
    let name = super::store::try_load::<Saved>(SECTION)
        .unwrap_or_default()
        .name;
    of(&name)
}

/// The bundled palette called `name`, else Bandit.
pub fn of(name: &str) -> &'static PaletteFile {
    palette_file::by_name(name).unwrap_or_else(palette_file::bandit)
}

/// Saves the choice.
pub fn set(name: &str) {
    let saved = Saved {
        name: of(name).name.clone(),
    };
    if let Err(e) = super::store::save(SECTION, &saved) {
        eprintln!("Foxglove: the lights' palette wasn't saved: {e}");
    }
}

/// The name a pill shows: Bandit's own, and the harmony palettes without "Bandit ".
pub fn short(p: &PaletteFile) -> &str {
    p.name.strip_prefix("Bandit ").unwrap_or(&p.name)
}

/// A palette's colours as the pill's dots: accent, then the second and third accents, else its
/// stops.
pub fn dots(p: &PaletteFile) -> Vec<[u8; 3]> {
    let r = &p.roles;
    match r.accent2 {
        Some(a2) => [Some(r.accent), Some(a2), r.accent3]
            .into_iter()
            .flatten()
            .map(|c| c.0)
            .collect(),
        None => p.stops.iter().take(3).map(|c| c.0).collect(),
    }
}

/// The swatches the card offers for `p`: Bandit's own six, with a harmony palette's second
/// and third accents after Bandit orange.
pub fn swatches(p: &PaletteFile) -> Vec<(String, [u8; 3])> {
    let mut out: Vec<(String, [u8; 3])> = super::lights_page::SWATCHES
        .iter()
        .map(|(n, c)| ((*n).to_string(), *c))
        .collect();
    let extra = [
        p.roles
            .accent2
            .map(|c| (format!("{}: second accent", p.name), c.0)),
        p.roles
            .accent3
            .map(|c| (format!("{}: third accent", p.name), c.0)),
    ];
    let mut at = 1;
    for (name, c) in extra.into_iter().flatten() {
        // A swatch of (nearly) the same colour already there stays the one.
        let hs = super::accent::hue_sat(c[0], c[1], c[2]);
        if !out
            .iter()
            .any(|(_, o)| super::lights_page::is_swatch(hs, *o))
        {
            out.insert(at, (name, c));
            at += 1;
        }
    }
    out
}

/// The palette's effects on the card, in order: a bold palette's Tide first, then Two-tone
/// and the slow sweep (docs/palette.md); none for a palette without a second accent.
pub fn effects(p: &PaletteFile) -> Vec<Kind> {
    match (p.roles.is_harmony(), p.tier) {
        (false, _) => vec![],
        (true, Tier::Bold) => vec![Kind::Tide, Kind::TwoTone, Kind::Sweep],
        (true, Tier::Harmony) => vec![Kind::TwoTone, Kind::Sweep],
    }
}

/// What a palette effect's pill says.
pub fn effect_label(kind: Kind) -> &'static str {
    match kind {
        Kind::TwoTone => "Two-tone",
        Kind::Tide => "Tide",
        _ => "Slow sweep",
    }
}

/// Its hover.
pub fn effect_hint(kind: Kind) -> &'static str {
    match kind {
        Kind::TwoTone => {
            "Orange and the second accent breathe in turn, once every 8 s, never below 35 %. \
             Runs as a scene."
        }
        Kind::Tide => {
            "A slow wave of the second accent rolls in from the outer keys and back, 12 s a \
             loop; each key changes colour only at a soft dip. Runs as a scene."
        }
        _ => {
            "The palette's colours drift slowly across the split, one turn every 12 s (9 s for \
             a bold palette). Runs as a scene."
        }
    }
}

/// The scene look a palette effect runs: the docs' slow looks (8 s two-tone; the sweep at
/// speed 1, left to right across the halves).
pub fn look(p: &PaletteFile, kind: Kind) -> Look {
    let base = Look {
        palette: p.name.clone(),
        ..Look::default()
    };
    match kind {
        Kind::TwoTone | Kind::Tide => Look { kind, ..base },
        _ => super::scene_looks::slow_sweep(&p.name),
    }
}

/// Which palette effect runs now: the running scene's look, when it is one of `p`'s.
pub fn running(p: &PaletteFile, scene_runs: bool, chosen: &Look) -> Option<Kind> {
    if !scene_runs {
        return None;
    }
    effects(p).into_iter().find(|&k| look(p, k) == *chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::scene_looks::Direction;

    #[test]
    fn bandit_is_the_default_and_changes_nothing() {
        let _env = super::super::store::test_sandbox();
        assert_eq!(chosen().name, "Bandit");
        let b = palette_file::bandit();
        let plain: Vec<_> = super::super::lights_page::SWATCHES
            .iter()
            .map(|(n, c)| ((*n).to_string(), *c))
            .collect();
        assert_eq!(
            swatches(b),
            plain,
            "Bandit's swatches are the card's own six"
        );
        assert_eq!(of("nonsense").name, "Bandit");
        // Saved (one test: the tests share the store's folder).
        set("Bandit Ember");
        assert_eq!(chosen().name, "Bandit Ember");
        set("nonsense");
        assert_eq!(chosen().name, "Bandit");
    }

    #[test]
    fn a_harmony_palette_adds_its_accents_and_slow_looks() {
        let dusk = of("Bandit Dusk");
        let s = swatches(dusk);
        assert_eq!(s[0].0, "Bandit orange");
        assert_eq!(s[1].1, [0x62, 0xbc, 0xb6]);
        assert_eq!(s[2].1, [0x99, 0x9e, 0xe6]);
        assert_eq!(s.len(), 8);
        // Steel's second accent is (nearly) the Steel blue swatch: no duplicate.
        assert_eq!(
            swatches(of("Bandit Steel")).len(),
            6 + usize::from(!super::super::lights_page::SWATCHES.iter().any(|(_, c)| {
                super::super::lights_page::is_swatch(
                    super::super::accent::hue_sat(0x6c, 0xa5, 0xd0),
                    *c,
                )
            }))
        );
        assert_eq!(short(dusk), "Dusk");
        assert_eq!(dots(dusk).len(), 3);
        assert_eq!(dots(palette_file::bandit()).len(), 3);
        let two = look(dusk, Kind::TwoTone);
        assert_eq!(
            (two.kind, two.palette.as_str()),
            (Kind::TwoTone, "Bandit Dusk")
        );
        let sweep = look(dusk, Kind::Sweep);
        // The slow sweep: 12 s a turn (9 s for a bold palette), left to right.
        assert_eq!(super::super::scene_looks::period_ms(sweep.speed), 12_000);
        assert_eq!(sweep.direction, Direction::Right);
        let bold_sweep = look(of("Bandit Ember Bold"), Kind::Sweep);
        assert_eq!(
            super::super::scene_looks::period_ms(bold_sweep.speed),
            9_000
        );
        assert_eq!(running(dusk, true, &two), Some(Kind::TwoTone));
        assert_eq!(running(dusk, false, &two), None);
        assert_eq!(running(dusk, true, &Look::default()), None);
        assert_eq!(effects(dusk), [Kind::TwoTone, Kind::Sweep]);
        assert_eq!(
            effects(of("Bandit Dusk Bold")),
            [Kind::Tide, Kind::TwoTone, Kind::Sweep]
        );
        assert!(
            effects(palette_file::bandit()).is_empty(),
            "Bandit offers none"
        );
    }
}
