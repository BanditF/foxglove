//! Temperature's light output (the plan: review/ux-plan-gpu-temp-effect.md, slice 4): what the
//! lights show for the reading, as a pure state machine over scene milliseconds. No device, no
//! thread, no clock of its own: [`super::scene_output::Outputs`] feeds it the reading and asks
//! it, per frame, for the effect to draw, so tests drive it with mock readings and a fake clock.
//!
//! What it does:
//! - Every light takes one colour, the reading's place between cool and hot ([`colour_for`]),
//!   eased over [`EASE_MS`] ([`Driver::effect_at`]). The room map, the case, the Dynamic
//!   Lighting devices and the keyboard's frames all draw that one eased colour, each at its own
//!   rate limit, and send only what changed.
//! - The keyboard's own on-device effect is the steady colour: `Breathe` with both colours
//!   equal, the effect message the scene already sends (no firmware change). It carries the
//!   target colour, not the ease, and is sent only when the target changes
//!   ([`Driver::take_changed`]).
//! - A missing reading is the fallback look (a slow breathe, or a steady half-brightness colour
//!   with Windows' animation effects off): never the cool colour, never the last colour held.
//! - Between readings nothing moves: once the ease ends [`Driver::settled`] is true and the
//!   frame thread sleeps (the still-screen rule, #1085) until the next reading or the next
//!   allowed change ([`Driver::next_change_at`]).
//! - A spoofed or noisy reading cannot flicker the lights (security note A2 of #1358): the target
//!   changes at most once per [`MIN_CHANGE_MS`] ([`GAME_CHANGE_MS`] between two colours in game
//!   mode, when a new reading comes only every 10 s), the newest wish wins when the wait ends,
//!   and the ease runs on top. A reading that flips back inside the wait changes nothing.
use foxglove_scene::{Effect, Rgb};

use super::temperature::{colour_for, EASE_MS, FALLBACK_MS};

/// The target colour changes at most this often: one reading (1 s).
pub const MIN_CHANGE_MS: i64 = 1_000;
/// Between two colours in game mode: the sensors app reads every 10 s then.
pub const GAME_CHANGE_MS: i64 = 10_000;

/// What a Temperature look needs to draw, from its saved parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct TempLook {
    /// Where the colours start and end (whole degrees Celsius, clamped by the look).
    pub cool_c: u16,
    pub hot_c: u16,
    /// The colours the effect runs through, cool end first ([`super::temperature::ramp`]).
    pub ramp: Vec<Rgb>,
    /// The fallback look's colour and the colour it breathes from.
    pub fallback: Rgb,
    pub background: Rgb,
    /// Windows' animation effects are off: the ease is one step and the fallback is steady.
    pub calm: bool,
}

/// What the lights show.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shows {
    Colour(Rgb),
    Fallback,
}

/// One steady colour, as an effect every output already knows: `Breathe` with both colours the
/// same (the keyboard draws it itself, the room's frames draw it flat).
pub fn steady(colour: Rgb) -> Effect {
    Effect::Breathe {
        from: colour,
        to: colour,
        period_ms: FALLBACK_MS,
    }
}

/// The state machine: the reading in, the effect to draw out.
#[derive(Clone, Debug)]
pub struct Driver {
    look: TempLook,
    /// The reading, `None` for "no reading", and whether the sensors app is in game mode.
    reading: Option<f32>,
    game: bool,
    /// What the lights are showing the way to (the accepted target).
    target: Shows,
    /// The colour shown when the ease towards `target` began.
    from: Rgb,
    eased_from: i64,
    /// When the target last changed (scene ms), for the rate limit.
    last_change: Option<i64>,
    /// The target changed since [`Driver::take_changed`].
    changed: bool,
}

impl Driver {
    /// Starts with this reading (`None` before the first one: the fallback look, at once).
    pub fn new(look: TempLook, reading: Option<f32>, game: bool, t: i64) -> Driver {
        let mut d = Driver {
            from: look.fallback,
            look,
            reading,
            game,
            target: Shows::Fallback,
            eased_from: t,
            last_change: None,
            changed: true,
        };
        // The first look is shown at once: nothing earlier to rate-limit against.
        d.consider(t);
        d
    }

    /// The ends or colours were edited while running: the colour re-maps now (an edit is the
    /// user's, not the sensors'), easing from what the lights show.
    pub fn set_look(&mut self, look: TempLook, t: i64) {
        if look == self.look {
            return;
        }
        let was_fallback = self.is_fallback();
        let shown = self.shown(t);
        self.look = look;
        self.from = if was_fallback {
            self.fallback_colour()
        } else {
            shown
        };
        self.eased_from = t;
        self.target = self.wanted();
        self.last_change = Some(t);
        self.changed = true;
    }

    /// A new reading (or none) from the sensors.
    pub fn set_reading(&mut self, reading: Option<f32>, game: bool, t: i64) {
        self.reading = reading;
        self.game = game;
        self.consider(t);
    }

    /// What the reading asks the lights to show now.
    fn wanted(&self) -> Shows {
        match colour_for(
            self.reading,
            self.look.cool_c,
            self.look.hot_c,
            &self.look.ramp,
        ) {
            Some(c) => Shows::Colour(c),
            None => Shows::Fallback,
        }
    }

    /// How long a change of target must wait after the last one.
    fn min_wait(&self, to: Shows) -> i64 {
        match (self.target, to) {
            (Shows::Colour(_), Shows::Colour(_)) if self.game => GAME_CHANGE_MS,
            _ => MIN_CHANGE_MS,
        }
    }

    /// Takes the wanted target when the rate limit allows; else leaves it for
    /// [`Driver::next_change_at`].
    fn consider(&mut self, t: i64) {
        let want = self.wanted();
        if want == self.target {
            return;
        }
        let due = self
            .last_change
            .is_none_or(|l| t - l >= self.min_wait(want) || t < l);
        if due {
            self.from = self.shown(t);
            self.eased_from = t;
            self.target = want;
            self.last_change = Some(t);
            self.changed = true;
        }
    }

    /// How far the ease has come at `t`, 0 to 1.
    fn progress(&self, t: i64) -> f32 {
        if self.look.calm {
            return 1.0;
        }
        ((t - self.eased_from) as f32 / EASE_MS as f32).clamp(0.0, 1.0)
    }

    /// The colour the room shows at `t` (the fallback's own colour for the fallback look).
    fn shown(&self, t: i64) -> Rgb {
        match self.target {
            Shows::Colour(c) => self.from.mix(c, self.progress(t)),
            Shows::Fallback => self.fallback_colour(),
        }
    }

    /// The fallback's colour: steady at half brightness with animations off.
    fn fallback_colour(&self) -> Rgb {
        if self.look.calm {
            self.look.fallback.mix(Rgb([0, 0, 0]), 0.5)
        } else {
            self.look.fallback
        }
    }

    fn fallback_effect(&self) -> Effect {
        if self.look.calm {
            steady(self.fallback_colour())
        } else {
            Effect::Breathe {
                from: self.look.background,
                to: self.look.fallback,
                period_ms: FALLBACK_MS,
            }
        }
    }

    /// The effect to draw at scene time `t`: the eased colour, or the fallback look. Lets a
    /// change that was waiting on the rate limit through once its time has come.
    pub fn effect_at(&mut self, t: i64) -> Effect {
        self.consider(t);
        match self.target {
            Shows::Colour(_) => steady(self.shown(t)),
            Shows::Fallback => self.fallback_effect(),
        }
    }

    /// The effect the keyboard draws itself: the target as one steady colour (no ease), or the
    /// fallback look.
    pub fn target_effect(&self) -> Effect {
        match self.target {
            Shows::Colour(c) => steady(c),
            Shows::Fallback => self.fallback_effect(),
        }
    }

    /// Whether the target changed since the last call (the keyboard gets one message).
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// The lights show their target and nothing moves: a colour with its ease over. (The
    /// fallback breathes, so it is never still, except steady with animations off.)
    pub fn eased(&self, t: i64) -> bool {
        match self.target {
            Shows::Colour(_) => self.progress(t) >= 1.0,
            Shows::Fallback => self.look.calm,
        }
    }

    /// Nothing moves and nothing waits: the ease is over and no change is held back.
    #[cfg(test)]
    pub fn settled(&self, t: i64) -> bool {
        self.eased(t) && self.next_change_at().is_none()
    }

    /// When a change that the rate limit is holding back may go (scene ms); `None` when the
    /// reading asks for nothing new.
    pub fn next_change_at(&self) -> Option<i64> {
        let want = self.wanted();
        if want == self.target {
            return None;
        }
        Some(self.last_change.map_or(0, |l| l + self.min_wait(want)))
    }

    /// Whether the lights show the fallback look now.
    pub fn is_fallback(&self) -> bool {
        self.target == Shows::Fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::temperature::{ramp, Colours};

    const BLUE: Rgb = Rgb([0x2e, 0x6b, 0xff]);
    const RED: Rgb = Rgb([0xe8, 0x2d, 0x1f]);

    fn look() -> TempLook {
        TempLook {
            cool_c: 40,
            hot_c: 80,
            ramp: ramp(Colours::CoolToHot, "Bandit"),
            fallback: Rgb([0xff, 0xd6, 0xaa]),
            background: Rgb([0, 0, 0]),
            calm: false,
        }
    }

    fn colour_of(e: &Effect) -> Rgb {
        match e {
            Effect::Breathe { from, to, .. } if from == to => *from,
            other => panic!("not a steady colour: {other:?}"),
        }
    }

    #[test]
    fn a_reading_is_one_steady_colour_eased_over_two_seconds() {
        let mut d = Driver::new(look(), Some(40.0), false, 0);
        // The first colour eases in from the fallback; once the ease is over it is blue.
        assert_eq!(colour_of(&d.effect_at(EASE_MS as i64)), BLUE);
        d.set_reading(Some(80.0), false, 3_000);
        // The ease starts at the reading: still blue, then half way, then red.
        assert_eq!(colour_of(&d.effect_at(3_000)), BLUE);
        let mid = colour_of(&d.effect_at(4_000));
        assert_ne!(mid, BLUE);
        assert_ne!(mid, RED);
        assert_eq!(colour_of(&d.effect_at(5_000)), RED);
        assert!(d.settled(5_000));
        assert!(!d.settled(4_000));
        // The keyboard is told the target, not the ease.
        assert_eq!(colour_of(&d.target_effect()), RED);
    }

    #[test]
    fn the_keyboard_hears_a_changed_target_once_and_only_then() {
        let mut d = Driver::new(look(), Some(50.0), false, 0);
        assert!(d.take_changed(), "the first colour");
        assert!(!d.take_changed());
        // A reading that is the same colour, and frames meanwhile: nothing to send.
        d.set_reading(Some(50.0), false, 1_000);
        for t in (1_000..5_000).step_by(33) {
            d.effect_at(t);
        }
        assert!(!d.take_changed());
        d.set_reading(Some(51.0), false, 5_000);
        assert!(d.take_changed());
        assert!(!d.take_changed());
    }

    #[test]
    fn once_the_ease_ends_nothing_is_due_until_the_next_reading() {
        let mut d = Driver::new(look(), Some(60.0), false, 0);
        d.set_reading(Some(70.0), false, 1_000);
        assert!(!d.settled(1_500));
        assert!(d.settled(1_000 + EASE_MS as i64));
        assert_eq!(d.next_change_at(), None, "no timer of its own");
    }

    #[test]
    fn a_missing_reading_is_the_fallback_never_cool_never_zero() {
        let mut d = Driver::new(look(), None, false, 0);
        assert!(d.is_fallback());
        assert!(matches!(d.effect_at(0), Effect::Breathe { from, to, .. } if from != to));
        assert!(!d.settled(10_000), "the breathe moves");
        // A reading comes: the first change is taken at once, easing from the fallback's
        // colour, not from blue.
        d.set_reading(Some(60.0), false, 100);
        assert!(!d.is_fallback());
        assert_eq!(colour_of(&d.effect_at(100)), look().fallback);
        // It goes again, and no reading never shows a colour.
        d.set_reading(None, false, 5_000);
        assert!(d.is_fallback());
        assert!(matches!(d.effect_at(5_000), Effect::Breathe { from, to, .. } if from != to));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            d.set_reading(Some(bad), false, 7_000);
            assert!(d.is_fallback(), "{bad}");
        }
    }

    #[test]
    fn with_animations_off_the_ease_is_one_step_and_the_fallback_is_steady() {
        let calm = TempLook {
            calm: true,
            ..look()
        };
        let mut d = Driver::new(calm.clone(), None, false, 0);
        let f = colour_of(&d.effect_at(0));
        assert_eq!(f, calm.fallback.mix(Rgb([0, 0, 0]), 0.5));
        assert!(d.settled(0), "a steady fallback is still");
        d.set_reading(Some(80.0), false, 1_000);
        assert_eq!(colour_of(&d.effect_at(1_000)), RED, "one step");
        assert!(d.settled(1_000));
    }

    /// Security note A2: a writer flipping ok/hot against ok/cool every few milliseconds cannot
    /// flicker the lights. The target changes at most once a second, and the eased colour never
    /// moves faster than the ease allows.
    #[test]
    fn a_spoofing_writer_flipping_hot_and_cool_cannot_flicker_the_lights() {
        let mut d = Driver::new(look(), Some(40.0), false, 0);
        d.take_changed();
        let (mut changes, mut last) = (0, colour_of(&d.effect_at(0)));
        let mut biggest_step = 0u32;
        // Ten seconds, a flip every 20 ms, frames at about 30 a second.
        for ms in 0..10_000i64 {
            if ms % 20 == 0 {
                let hot = (ms / 20) % 2 == 0;
                d.set_reading(Some(if hot { 90.0 } else { 20.0 }), false, ms);
            }
            if ms % 33 == 0 {
                let c = colour_of(&d.effect_at(ms));
                biggest_step = biggest_step.max(
                    c.0.iter()
                        .zip(last.0)
                        .map(|(a, b)| u32::from(*a).abs_diff(u32::from(b)))
                        .max()
                        .unwrap_or(0),
                );
                last = c;
            }
            if d.take_changed() {
                changes += 1;
            }
        }
        // One a second at the most.
        assert!(changes <= 10, "{changes} target changes in 10 s");
        // 33 ms of a 2 s ease moves a channel by 255 * 33 / 2000 at the most.
        assert!(biggest_step <= 255 * 33 / 2_000 + 2, "{biggest_step}");
    }

    #[test]
    fn a_flip_that_flips_back_inside_the_wait_changes_nothing() {
        let mut d = Driver::new(look(), Some(40.0), false, 0);
        d.set_reading(Some(90.0), false, 1_000);
        d.take_changed();
        // 100 ms later it flips to cool and back to hot: the target is still red, and
        // nothing is due when it comes back.
        d.set_reading(Some(20.0), false, 1_100);
        assert_eq!(d.next_change_at(), Some(2_000));
        d.set_reading(Some(90.0), false, 1_200);
        assert_eq!(d.next_change_at(), None);
        assert!(!d.take_changed());
    }

    #[test]
    fn the_newest_wish_wins_when_the_wait_ends() {
        let mut d = Driver::new(look(), Some(40.0), false, 0);
        d.set_reading(Some(90.0), false, 1_000);
        d.take_changed();
        d.set_reading(Some(60.0), false, 1_300);
        d.set_reading(Some(70.0), false, 1_600);
        assert_eq!(colour_of(&d.target_effect()), RED);
        d.effect_at(1_900);
        assert!(!d.take_changed());
        d.effect_at(2_000);
        assert!(d.take_changed());
        let want = colour_for(Some(70.0), 40, 80, &look().ramp).unwrap();
        assert_eq!(colour_of(&d.target_effect()), want);
    }

    #[test]
    fn game_mode_changes_colour_every_ten_seconds_at_the_most() {
        let mut d = Driver::new(look(), Some(40.0), true, 0);
        d.take_changed();
        d.set_reading(Some(60.0), true, 2_000);
        assert_eq!(d.next_change_at(), Some(10_000));
        d.effect_at(9_999);
        assert!(!d.take_changed());
        d.effect_at(10_000);
        assert!(d.take_changed());
        // A lost reading is not held for ten seconds.
        d.set_reading(None, true, 10_500);
        assert_eq!(d.next_change_at(), Some(11_000));
    }

    #[test]
    fn editing_the_ends_re_maps_without_a_restart() {
        let mut d = Driver::new(look(), Some(60.0), false, 0);
        let before = colour_of(&d.effect_at(5_000));
        d.take_changed();
        d.set_look(
            TempLook {
                cool_c: 50,
                hot_c: 60,
                ..look()
            },
            5_000,
        );
        assert!(d.take_changed());
        assert_eq!(colour_of(&d.target_effect()), RED, "60 is the hot end now");
        assert_ne!(before, RED);
        // Easing from what was shown, not jumping.
        assert_eq!(colour_of(&d.effect_at(5_000)), before);
    }
}
