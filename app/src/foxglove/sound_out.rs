//! Sound's light output (the plan: review/ux-plan-sound-effect.md, slice 4): what the lights
//! show for the level, as a pure state machine over scene milliseconds. No device, no thread, no
//! clock of its own: [`super::scene_output::Outputs`] hands it the newest peak and asks it, per
//! frame, for the effect to draw, so tests drive it with made-up peaks and a fake clock.
//!
//! What it does:
//! - Every light takes one colour: the level's place between the look's floor and full
//!   ([`super::sound::Shaper`]: level, Smoothing envelope, colour, flash limit). The room map,
//!   the case, the Dynamic Lighting devices and the keyboard's frames all draw that colour, each
//!   at its own rate limit, and send only what changed.
//! - At most [`UPDATE_MS`] apart (20 a second), like Screen glow.
//! - No level (nothing playing, muted, no device, a failed read) is silence: the lights ease to
//!   the floor and stay. Once still, [`Driver::still`] is true and the frame thread sleeps (the
//!   still-screen rule, #1085) until the level changes.
//! - In game mode the keyboard holds the floor colour ([`Driver::hold`]): one steady effect,
//!   sent once, and no frames after it. The PC's lights still follow.
//! - Whatever the peaks do, a light swings at most 3 times a second (the flash limit runs last,
//!   on the colour sent): a hostile or broken source cannot strobe.
use foxglove_scene::{Effect, Rgb};

use super::sound::{colour_at, Colours, Shaper, Smooth};
use super::temperature_out::steady;

/// The lights take a new colour at most this often.
pub const UPDATE_MS: i64 = foxglove_scene::screen::MIN_UPDATE_MS;

/// The shortest time between two changes of the keyboard's hold, ms: a game mode flipped on and
/// off quickly can't flash the keyboard between the floor and a loud colour faster than the
/// flash limit lets the level itself.
pub const HOLD_MIN_MS: i64 = 1000 / foxglove_scene::screen::MAX_SWINGS as i64 + 1;

/// What a Sound look needs to draw, from its saved parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundLook {
    pub colours: Colours,
    pub smooth: Smooth,
    /// Windows' animation effects are off: the slow envelope, whatever Smoothing says.
    pub calm: bool,
}

impl SoundLook {
    /// The colour with no sound.
    pub fn floor(&self) -> Rgb {
        colour_at(0.0, self.colours)
    }
}

pub struct Driver {
    look: SoundLook,
    shaper: Shaper,
    /// The newest peak; `None`: silence.
    peak: Option<f32>,
    /// The colour shown, and when it was last worked out.
    colour: Rgb,
    at: Option<i64>,
    /// Game mode: the keyboard holds the floor.
    hold: bool,
    /// The hold asked for, taken at the next colour once [`HOLD_MIN_MS`] has passed, and when
    /// the hold last changed.
    want_hold: bool,
    hold_at: Option<i64>,
    /// The keyboard's own effect must be worked out again.
    changed: bool,
}

impl Driver {
    pub fn new(look: SoundLook) -> Driver {
        Driver {
            look,
            shaper: Shaper::default(),
            peak: None,
            colour: look.floor(),
            at: None,
            hold: false,
            want_hold: false,
            hold_at: None,
            changed: true,
        }
    }

    /// The look was edited (palette, Smoothing): the level carries on, in the new colours.
    pub fn set_look(&mut self, look: SoundLook) {
        if look != self.look {
            self.look = look;
            self.at = None;
            self.changed = true;
        }
    }

    /// The newest peak (`None`: no level to follow) and whether the keyboard holds steady.
    pub fn set_level(&mut self, peak: Option<f32>, hold: bool) {
        self.peak = peak;
        self.want_hold = hold;
    }

    /// Takes the hold asked for, no sooner than [`HOLD_MIN_MS`] after its last change.
    fn settle_hold(&mut self, t: i64) {
        if self.want_hold != self.hold
            && self
                .hold_at
                .is_none_or(|at| t - at >= HOLD_MIN_MS || t < at)
        {
            self.hold = self.want_hold;
            self.hold_at = Some(t);
            self.changed = true;
        }
    }

    /// The effect to draw at `t`: one steady colour, worked out at most every [`UPDATE_MS`].
    pub fn effect_at(&mut self, t: i64) -> Effect {
        self.settle_hold(t);
        if self.at.is_none_or(|at| t - at >= UPDATE_MS || t < at) {
            self.at = Some(t);
            let times = self.look.smooth.times(self.look.calm);
            self.colour = self.shaper.colour(self.peak, t, times, self.look.colours);
        }
        steady(self.colour)
    }

    /// The colour shown now.
    #[cfg(test)]
    pub fn colour(&self) -> Rgb {
        self.colour
    }

    /// Nothing more to draw while the peak stays as it is: the envelope arrived and the flash
    /// limit holds nothing back.
    pub fn still(&self) -> bool {
        self.at.is_some()
            && self.want_hold == self.hold
            && self.shaper.still(self.peak)
            && self.colour == colour_at(self.shaper.level(), self.look.colours)
    }

    /// When the next colour is due.
    pub fn next_at(&self) -> Option<i64> {
        self.at.map(|at| at + UPDATE_MS)
    }

    /// The colour the keyboard holds in game mode; `None`: it follows like every light.
    pub fn hold(&self) -> Option<Rgb> {
        self.hold.then(|| self.look.floor())
    }

    /// The keyboard's own effect while it holds: the floor, steady. `None`: frames.
    pub fn keyboard_effect(&self) -> Option<Effect> {
        self.hold().map(steady)
    }

    /// True once after the keyboard's own effect changed (a hold came or went, the look).
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review: game mode flipped on and off quickly can't flash the keyboard between
    /// the floor and a loud colour; the hold changes at most [`MAX_SWINGS`] times a second.
    #[test]
    fn a_flapping_hold_changes_no_faster_than_the_flash_limit() {
        let mut d = Driver::new(look(Smooth::Quick));
        d.take_changed();
        let mut changes = 0;
        for i in 0..200 {
            let t = i * 10;
            d.set_level(Some(1.0), i % 2 == 0);
            d.effect_at(t);
            if d.take_changed() {
                changes += 1;
            }
            // A hold still owed keeps the frames coming.
            assert!(!d.still() || d.hold().is_some() == (i % 2 == 0));
        }
        assert!((1..=2 * MAX_SWINGS).contains(&changes), "{changes} in 2 s");
        // Left alone, the hold asked for last is taken.
        d.set_level(Some(1.0), true);
        d.effect_at(2_000 + HOLD_MIN_MS);
        assert!(d.hold().is_some());
    }
    use foxglove_scene::screen::{luminance, MAX_SWINGS, SWING};

    fn look(smooth: Smooth) -> SoundLook {
        SoundLook {
            colours: super::super::sound::colours("Bandit"),
            smooth,
            calm: false,
        }
    }

    fn shown(e: &Effect) -> Rgb {
        match e {
            Effect::Breathe { from, to, .. } => {
                assert_eq!(from, to, "one steady colour");
                *from
            }
            other => panic!("not a steady colour: {other:?}"),
        }
    }

    #[test]
    fn silence_is_the_floor_and_still_at_once() {
        let mut d = Driver::new(look(Smooth::Calm));
        assert!(!d.still(), "nothing drawn yet");
        assert_eq!(shown(&d.effect_at(0)), look(Smooth::Calm).floor());
        assert!(d.still());
        assert_eq!(shown(&d.effect_at(10_000)), look(Smooth::Calm).floor());
    }

    #[test]
    fn a_level_brightens_the_lights_and_silence_eases_back_to_the_floor() {
        let l = look(Smooth::Calm);
        let mut d = Driver::new(l);
        d.effect_at(0);
        d.set_level(Some(0.5), false);
        assert!(!d.still());
        let mut t = 0;
        while !d.still() {
            t += UPDATE_MS;
            d.effect_at(t);
            assert!(t < 5_000, "it settles");
        }
        let loud = d.colour();
        assert!(luminance(loud) > luminance(l.floor()));
        d.set_level(None, false);
        while !d.still() {
            t += UPDATE_MS;
            d.effect_at(t);
            assert!(t < 10_000, "it settles");
        }
        assert_eq!(d.colour(), l.floor());
    }

    #[test]
    fn the_colour_changes_at_most_twenty_times_a_second() {
        let mut d = Driver::new(look(Smooth::Quick));
        d.effect_at(0);
        let mut changes = 0;
        let mut last = d.colour();
        for t in 1..=1_000 {
            // A new peak every millisecond.
            d.set_level(Some(if t % 2 == 0 { 1.0 } else { 0.01 }), false);
            d.effect_at(t);
            if d.colour() != last {
                last = d.colour();
                changes += 1;
            }
        }
        assert!(changes <= 20, "{changes} changes in a second");
    }

    #[test]
    fn the_worst_peaks_never_swing_the_lights_more_than_three_times_a_second() {
        for smooth in Smooth::ALL {
            let mut d = Driver::new(look(smooth));
            let mut swings: Vec<i64> = Vec::new();
            let (mut dir, mut extreme) = (0i8, luminance(shown(&d.effect_at(0))));
            for step in 1..=20 * 30 {
                let t = step * UPDATE_MS;
                // Full and silent in turn, a few updates each.
                d.set_level((step / 3 % 2 == 0).then_some(1.0), false);
                let y = luminance(shown(&d.effect_at(t)));
                let delta = y - extreme;
                let way = if delta > 0.0 { 1 } else { -1 };
                if dir == 0 || way == dir {
                    if delta.abs() >= SWING {
                        dir = way;
                    }
                    if dir == way {
                        extreme = y;
                    }
                } else if delta.abs() >= SWING {
                    swings.push(t);
                    dir = way;
                    extreme = y;
                }
                let recent = swings.iter().filter(|&&s| t - s < 1_000).count();
                assert!(
                    recent <= MAX_SWINGS,
                    "{smooth:?}: {recent} swings by {t} ms"
                );
            }
        }
    }

    #[test]
    fn reduced_motion_runs_the_slow_envelope_whatever_smoothing_says() {
        let mut quick = Driver::new(look(Smooth::Quick));
        let mut calm = Driver::new(SoundLook {
            calm: true,
            ..look(Smooth::Quick)
        });
        for d in [&mut quick, &mut calm] {
            d.effect_at(0);
            d.set_level(Some(1.0), false);
            d.effect_at(UPDATE_MS);
            d.effect_at(2 * UPDATE_MS);
        }
        assert!(luminance(calm.colour()) < luminance(quick.colour()));
    }

    #[test]
    fn game_mode_holds_the_keyboard_at_the_floor_and_says_so_once() {
        let l = look(Smooth::Calm);
        let mut d = Driver::new(l);
        assert!(d.take_changed());
        assert_eq!(d.keyboard_effect(), None, "frames, like every light");
        // The hold is taken with the next colour.
        d.set_level(Some(1.0), true);
        d.effect_at(0);
        assert!(d.take_changed());
        assert!(!d.take_changed());
        assert_eq!(d.hold(), Some(l.floor()));
        assert_eq!(d.keyboard_effect(), Some(steady(l.floor())));
        // The same again changes nothing.
        d.set_level(Some(0.2), true);
        d.effect_at(UPDATE_MS);
        assert!(!d.take_changed());
        d.set_level(Some(0.2), false);
        d.effect_at(HOLD_MIN_MS);
        assert!(d.take_changed());
        assert_eq!(d.hold(), None);
    }
}
