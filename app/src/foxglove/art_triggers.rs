//! Right art that follows what's happening, above the hotlist (#60, docs/art-cycling.md): a
//! layer's art while that layer is active, the typing art while the user types (#886), and a
//! lighting preset's art while it's applied.
//!
//! Priority, highest first: layer art, typing art, preset art, then the pinned choice (the
//! user's own pick, the hotlist position or the wallpaper match, which all pin). Everything here
//! reacts to events; the only clocks are one-shot deadlines: `LAYER_DELAY` after a layer change,
//! so tapping a momentary layer doesn't flash the screen, and `TYPING_IDLE` after the last key
//! while the typing art shows. Typing starts on key events alone (no timer while building up).

use std::time::{Duration, Instant};

use super::art_cycle::{Cycle, Item};
use super::typing_art::Activity;

/// How long a layer must stay active before its art shows.
pub const LAYER_DELAY: Duration = Duration::from_millis(400);
/// How long the user must keep typing before the typing art shows (#886 hysteresis).
pub const TYPING_ENTER: Duration = Duration::from_secs(2);
/// The longest pause inside sustained typing; a longer one starts `TYPING_ENTER` again.
pub const TYPING_GAP: Duration = Duration::from_millis(1500);
/// How long without a key before the typing art leaves.
pub const TYPING_IDLE: Duration = Duration::from_secs(5);

/// The typing trigger's state (#886). Holds times only, never what was typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Typing {
    #[default]
    Idle,
    /// Typing that hasn't lasted `TYPING_ENTER` yet: when it began, and the last key.
    Building { since: Instant, last: Instant },
    /// The typing art is on (under any layer art): the piece, and the last key.
    Active { art: Item, last: Instant },
}

/// What the art service should do after a trigger event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Nothing,
    /// Show this (without pinning it).
    Show(Item),
    /// A trigger ended: show the next one down, or the pinned choice.
    Restore,
}

#[derive(Debug, Default)]
pub struct Triggers {
    /// A configured layer that became the top one, and when its art is due.
    pending: Option<(u8, Instant)>,
    /// The layer whose art is showing.
    showing_layer: Option<u8>,
    /// The applied lighting preset's art.
    preset: Option<Item>,
    /// Typing art (#886).
    typing: Typing,
}

impl Triggers {
    /// The highest active layer that has art.
    fn top_layer(active: &[u8], cycle: &Cycle) -> Option<u8> {
        active
            .iter()
            .copied()
            .filter(|l| cycle.layer_art(*l).is_some())
            .max()
    }

    /// The keyboard's active layers changed.
    pub fn layers(&mut self, active: &[u8], cycle: &Cycle, now: Instant) -> Change {
        let top = Self::top_layer(active, cycle);
        if top == self.showing_layer {
            self.pending = None;
            return Change::Nothing;
        }
        match top {
            Some(layer) => {
                if self.pending.map(|(l, _)| l) != Some(layer) {
                    self.pending = Some((layer, now + LAYER_DELAY));
                }
                Change::Nothing
            }
            None => {
                self.pending = None;
                if self.showing_layer.take().is_some() {
                    Change::Restore
                } else {
                    Change::Nothing
                }
            }
        }
    }

    /// The next one-shot deadline: a pending layer's art, or the typing art's idle timeout.
    pub fn deadline(&self) -> Option<Instant> {
        let layer = self.pending.map(|(_, at)| at);
        let typing = match &self.typing {
            Typing::Active { last, .. } => Some(*last + TYPING_IDLE),
            _ => None,
        };
        match (layer, typing) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// A deadline ran out: the pending layer's delay, else the typing idle timeout. One at a
    /// time; another that is due too comes back as the next deadline (already past).
    pub fn due(&mut self, cycle: &Cycle, now: Instant) -> Change {
        match self.pending {
            Some((layer, at)) if at <= now => {
                self.pending = None;
                return match cycle.layer_art(layer) {
                    Some(art) => {
                        self.showing_layer = Some(layer);
                        Change::Show(art.clone())
                    }
                    None => Change::Nothing,
                };
            }
            _ => {}
        }
        match &self.typing {
            Typing::Active { last, .. } if *last + TYPING_IDLE <= now => {
                self.typing = Typing::Idle;
                super::typing_art::note_switch(false);
                self.under_layer(Change::Restore)
            }
            _ => Change::Nothing,
        }
    }

    /// `change` unless layer art is on top, which stays.
    fn under_layer(&self, change: Change) -> Change {
        if self.showing_layer.is_some() {
            Change::Nothing
        } else {
            change
        }
    }

    /// Typing activity (#886): a key went down or up somewhere. Shows the typing art once the
    /// typing has lasted `TYPING_ENTER` with no pause over `TYPING_GAP`; each key also moves
    /// the `TYPING_IDLE` deadline. Inert while no typing art is set.
    pub fn typed(&mut self, activity: Activity, cycle: &Cycle) -> Change {
        let now = activity.at();
        let Some(art) = cycle.typing.clone() else {
            return self.typing_art_changed(cycle);
        };
        match &mut self.typing {
            Typing::Idle => {
                self.typing = Typing::Building {
                    since: now,
                    last: now,
                };
                Change::Nothing
            }
            Typing::Building { since, last } => {
                if now.saturating_duration_since(*last) > TYPING_GAP {
                    *since = now;
                }
                *last = now;
                if now.saturating_duration_since(*since) < TYPING_ENTER {
                    return Change::Nothing;
                }
                self.typing = Typing::Active {
                    art: art.clone(),
                    last: now,
                };
                super::typing_art::note_switch(true);
                self.under_layer(Change::Show(art))
            }
            Typing::Active { last, .. } => {
                *last = now;
                Change::Nothing
            }
        }
    }

    /// The typing art setting changed (Screens, or a profile switch): a showing typing art
    /// follows it, or ends when it's off.
    pub fn typing_art_changed(&mut self, cycle: &Cycle) -> Change {
        match (&mut self.typing, cycle.typing.clone()) {
            (Typing::Active { art, .. }, Some(new)) if *art != new => {
                *art = new.clone();
                self.under_layer(Change::Show(new))
            }
            (Typing::Active { .. }, None) => {
                self.typing = Typing::Idle;
                super::typing_art::note_switch(false);
                self.under_layer(Change::Restore)
            }
            (Typing::Building { .. }, None) => {
                self.typing = Typing::Idle;
                Change::Nothing
            }
            _ => Change::Nothing,
        }
    }

    /// The typing art while it shows (under any layer art).
    fn typing_art(&self) -> Option<&Item> {
        match &self.typing {
            Typing::Active { art, .. } => Some(art),
            _ => None,
        }
    }

    /// A preset's art: a running scene's wallpaper art (#488, `scene_run::set_art`), `Some`,
    /// or none any more (`None`).
    pub fn preset(&mut self, art: Option<Item>) -> Change {
        if self.preset == art {
            return Change::Nothing;
        }
        let ended = self.preset.is_some() && art.is_none();
        self.preset = art.clone();
        if self.showing_layer.is_some() || self.typing_art().is_some() {
            return Change::Nothing; // layer or typing art stays on top; the preset shows after it
        }
        match art {
            Some(art) => Change::Show(art),
            None if ended => Change::Restore,
            None => Change::Nothing,
        }
    }

    /// The trigger art that should show now, if any (for restores and reconnects).
    pub fn top(&self, cycle: &Cycle) -> Option<Item> {
        self.showing_layer
            .and_then(|l| cycle.layer_art(l).cloned())
            .or_else(|| self.typing_art().cloned())
            .or_else(|| self.preset.clone())
    }

    /// The keyboard went away: layers are unknown until it's back. Preset art stays.
    pub fn disconnected(&mut self) {
        self.pending = None;
        self.showing_layer = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle() -> Cycle {
        let mut c = Cycle::default();
        c.set_layer_art(3, Some(Item::BuiltIn("Bandit_Radar".into())));
        c.set_layer_art(5, Some(Item::Slot(1)));
        c
    }

    #[test]
    fn layer_art_waits_out_the_delay_and_a_tap_shows_nothing() {
        let (c, t0) = (cycle(), Instant::now());
        let mut t = Triggers::default();
        assert_eq!(t.layers(&[0, 3], &c, t0), Change::Nothing);
        assert_eq!(t.deadline(), Some(t0 + LAYER_DELAY));
        assert_eq!(
            t.due(&c, t0 + Duration::from_millis(100)),
            Change::Nothing,
            "not yet"
        );
        // released before the delay: the pending art is dropped, nothing to restore
        assert_eq!(
            t.layers(&[0], &c, t0 + Duration::from_millis(200)),
            Change::Nothing
        );
        assert_eq!(t.deadline(), None);
        // held: shows, then restores on release
        assert_eq!(t.layers(&[0, 3], &c, t0), Change::Nothing);
        assert_eq!(
            t.due(&c, t0 + LAYER_DELAY),
            Change::Show(Item::BuiltIn("Bandit_Radar".into()))
        );
        assert_eq!(
            t.layers(&[0, 3, 1], &c, t0),
            Change::Nothing,
            "layer 1 has no art"
        );
        assert_eq!(t.layers(&[0], &c, t0), Change::Restore);
    }

    #[test]
    fn the_highest_layer_with_art_wins() {
        let (c, t0) = (cycle(), Instant::now());
        let mut t = Triggers::default();
        t.layers(&[0, 3, 5], &c, t0);
        assert_eq!(t.due(&c, t0 + LAYER_DELAY), Change::Show(Item::Slot(1)));
        t.layers(&[0, 3], &c, t0 + LAYER_DELAY);
        assert_eq!(
            t.due(&c, t0 + 2 * LAYER_DELAY),
            Change::Show(Item::BuiltIn("Bandit_Radar".into()))
        );
    }

    #[test]
    fn preset_art_sits_under_layer_art() {
        let (c, t0) = (cycle(), Instant::now());
        let mut t = Triggers::default();
        let fury = Item::File("fury.bin".into());
        assert_eq!(t.preset(Some(fury.clone())), Change::Show(fury.clone()));
        assert_eq!(
            t.preset(Some(fury.clone())),
            Change::Nothing,
            "same preset again"
        );
        t.layers(&[3], &c, t0);
        t.due(&c, t0 + LAYER_DELAY);
        assert_eq!(t.top(&c), Some(Item::BuiltIn("Bandit_Radar".into())));
        assert_eq!(t.layers(&[0], &c, t0), Change::Restore);
        assert_eq!(t.top(&c), Some(fury), "back to the preset's art");
        assert_eq!(t.preset(None), Change::Restore);
        assert_eq!(t.top(&c), None, "back to the pinned choice");
    }

    // Typing art (#886): a fake clock (`t0 + ms`), no window, no Raw Input.

    fn typing_cycle() -> Cycle {
        let mut c = cycle();
        c.typing = Some(Item::BuiltIn("Bandit_Scanline".into()));
        c
    }

    fn ms(t0: Instant, n: u64) -> Activity {
        Activity::at_time(t0 + Duration::from_millis(n))
    }

    /// Keys every `step` ms from `from` to `to` (inclusive); the last change.
    fn type_run(t: &mut Triggers, c: &Cycle, t0: Instant, from: u64, to: u64, step: u64) -> Change {
        let mut last = Change::Nothing;
        let mut at = from;
        while at <= to {
            let change = t.typed(ms(t0, at), c);
            if change != Change::Nothing {
                last = change;
            }
            at += step;
        }
        last
    }

    #[test]
    fn typing_art_shows_after_sustained_typing_only() {
        let (c, t0) = (typing_cycle(), Instant::now());
        let mut t = Triggers::default();
        // A short burst: nothing, and no timer runs while building up.
        assert_eq!(type_run(&mut t, &c, t0, 0, 1500, 250), Change::Nothing);
        assert_eq!(t.deadline(), None, "no timer while building up");
        // A pause over TYPING_GAP starts the count again.
        assert_eq!(type_run(&mut t, &c, t0, 3500, 5000, 250), Change::Nothing);
        // Typing on past TYPING_ENTER since 3500 shows the art.
        assert_eq!(
            type_run(&mut t, &c, t0, 5250, 5500, 250),
            Change::Show(Item::BuiltIn("Bandit_Scanline".into()))
        );
        assert_eq!(t.top(&c), Some(Item::BuiltIn("Bandit_Scanline".into())));
    }

    #[test]
    fn typing_art_leaves_after_the_idle_timeout_and_keys_push_it_back() {
        let (c, t0) = (typing_cycle(), Instant::now());
        let mut t = Triggers::default();
        type_run(&mut t, &c, t0, 0, 2000, 250);
        assert!(t.top(&c).is_some());
        assert_eq!(
            t.deadline(),
            Some(t0 + Duration::from_millis(2000) + TYPING_IDLE)
        );
        // A key before the deadline moves it; the old one does nothing.
        assert_eq!(t.typed(ms(t0, 6000), &c), Change::Nothing);
        assert_eq!(t.due(&c, t0 + Duration::from_millis(7000)), Change::Nothing);
        assert_eq!(
            t.deadline(),
            Some(t0 + Duration::from_millis(6000) + TYPING_IDLE)
        );
        assert_eq!(
            t.due(&c, t0 + Duration::from_millis(6000) + TYPING_IDLE),
            Change::Restore
        );
        assert_eq!(t.top(&c), None);
        assert_eq!(t.deadline(), None, "idle: no timer");
        // A single key after leaving doesn't bring it straight back.
        assert_eq!(t.typed(ms(t0, 12_000), &c), Change::Nothing);
        assert_eq!(t.top(&c), None);
    }

    #[test]
    fn typing_art_sits_between_layer_and_preset_art() {
        let (c, t0) = (typing_cycle(), Instant::now());
        let scan = Item::BuiltIn("Bandit_Scanline".into());
        let fury = Item::File("fury.bin".into());
        let mut t = Triggers::default();
        t.preset(Some(fury.clone()));
        // Typing goes over the preset.
        assert_eq!(
            type_run(&mut t, &c, t0, 0, 2000, 250),
            Change::Show(scan.clone())
        );
        assert_eq!(t.preset(None), Change::Nothing, "typing art stays");
        t.preset(Some(fury.clone()));
        // A layer goes over typing; leaving it shows typing again.
        t.layers(&[3], &c, t0 + Duration::from_millis(2100));
        t.due(&c, t0 + Duration::from_millis(2100) + LAYER_DELAY);
        assert_eq!(t.top(&c), Some(Item::BuiltIn("Bandit_Radar".into())));
        assert_eq!(t.layers(&[0], &c, t0), Change::Restore);
        assert_eq!(t.top(&c), Some(scan));
        // Idle: back to the preset.
        let idle = t.deadline().unwrap();
        assert_eq!(t.due(&c, idle), Change::Restore);
        assert_eq!(t.top(&c), Some(fury));
    }

    #[test]
    fn typing_art_leaving_under_a_layer_changes_nothing_on_screen() {
        let (c, t0) = (typing_cycle(), Instant::now());
        let mut t = Triggers::default();
        t.layers(&[3], &c, t0);
        t.due(&c, t0 + LAYER_DELAY);
        assert_eq!(
            type_run(&mut t, &c, t0, 0, 2000, 250),
            Change::Nothing,
            "layer on top"
        );
        let idle = t.deadline().unwrap();
        assert_eq!(t.due(&c, idle), Change::Nothing);
        assert_eq!(t.top(&c), Some(Item::BuiltIn("Bandit_Radar".into())));
    }

    #[test]
    fn typing_art_off_is_inert_and_turning_it_off_restores() {
        let (c, t0) = (cycle(), Instant::now());
        let mut t = Triggers::default();
        assert_eq!(type_run(&mut t, &c, t0, 0, 10_000, 250), Change::Nothing);
        assert_eq!(t.deadline(), None);
        let on = typing_cycle();
        type_run(&mut t, &on, t0, 20_000, 22_000, 250);
        assert!(t.top(&on).is_some());
        let mut other = on.clone();
        other.typing = Some(Item::Slot(1));
        assert_eq!(t.typing_art_changed(&other), Change::Show(Item::Slot(1)));
        assert_eq!(t.typing_art_changed(&c), Change::Restore);
        assert_eq!(t.top(&c), None);
        assert_eq!(t.deadline(), None);
    }
}
