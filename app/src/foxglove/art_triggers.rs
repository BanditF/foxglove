//! Right art that follows what's happening, above the hotlist (#60, docs/art-cycling.md): a
//! layer's art while that layer is active, and a lighting preset's art while it's applied.
//!
//! Priority, highest first: layer art, preset art, then the pinned choice (the user's own pick,
//! the hotlist position or the wallpaper match, which all pin). Everything here reacts to
//! events; the only clock is the one-shot `LAYER_DELAY` after a layer change, so tapping a
//! momentary layer doesn't flash the screen.

use std::time::{Duration, Instant};

use super::art_cycle::{Cycle, Item};

/// How long a layer must stay active before its art shows.
pub const LAYER_DELAY: Duration = Duration::from_millis(400);

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

    /// When the pending layer's art is due, if one is waiting.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.map(|(_, at)| at)
    }

    /// The pending layer's delay ran out.
    pub fn due(&mut self, cycle: &Cycle, now: Instant) -> Change {
        match self.pending {
            Some((layer, at)) if at <= now => {
                self.pending = None;
                match cycle.layer_art(layer) {
                    Some(art) => {
                        self.showing_layer = Some(layer);
                        Change::Show(art.clone())
                    }
                    None => Change::Nothing,
                }
            }
            _ => Change::Nothing,
        }
    }

    /// A lighting preset was applied (`Some`, its art) or cleared (`None`).
    pub fn preset(&mut self, art: Option<Item>) -> Change {
        if self.preset == art {
            return Change::Nothing;
        }
        let ended = self.preset.is_some() && art.is_none();
        self.preset = art.clone();
        if self.showing_layer.is_some() {
            return Change::Nothing; // the layer's art stays on top; the preset shows after it
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
}
