//! Rotation: what changes the right screen's art and when (#60), saved in the settings store's
//! `[art.cycle]` (profiled: each profile can have its own). The art service does the stepping.
//!
//! Rotation steps through the keyboard's own art, the pieces that come with it and then the
//! kept places, the order its art keys use without Foxglove (#515: "the hotlist and stored on
//! the device should be one and the same"). `items` is the old hotlist, kept for the playlist
//! idea (#522) and not stepped.
//!
//! Built-ins are stored by their manifest name (the stable id), not their index, so a firmware
//! that reorders its sets still shows the same art.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::art_service::Choice;
use digitalis::protocol::ArtStatus;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Item {
    /// A built-in set by its manifest name.
    BuiltIn(String),
    Slot(u8),
    File(PathBuf),
}

/// The single cycling mode of earlier builds, read once and turned into `step` / `follow`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Off,
    Minutes(u32),
    LightsWake,
    Wallpaper,
}

/// What moves the hotlist to its next entry (docs/art-cycling.md). Any number can be on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// The keyboard's `&art_next` / `&art_prev` keys (needs firmware support).
    Key,
    /// Each time the lights wake after going off (needs firmware support).
    LightsWake,
    /// Every `minutes`, while a keyboard is connected: the one timed trigger, a single deadline.
    Minutes,
}

/// What picks the art outright, instead of the hotlist position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Follow {
    #[default]
    Nothing,
    /// The art that goes with the Wallpaper Engine wallpaper on screen (art\\wallpapers.json).
    Wallpaper,
}

/// Art shown while a layer is active (see `art_triggers`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerArt {
    pub layer: u8,
    pub art: Item,
}

fn default_minutes() -> u32 {
    5
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cycle {
    /// Read from earlier builds only; `load` moves it into `step` and `follow`.
    #[serde(skip_serializing)]
    pub mode: Mode,
    /// The old hotlist: no longer stepped (see the module), kept for a playlist (#522).
    pub items: Vec<Item>,
    /// The entry shown last, so a restart carries on where it left off.
    #[serde(default)]
    pub position: usize,
    #[serde(default)]
    pub step: Vec<Step>,
    #[serde(default = "default_minutes")]
    pub minutes: u32,
    #[serde(default)]
    pub follow: Follow,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerArt>,
    /// Which monitor's wallpaper to follow (a Wallpaper Engine monitor key); `None` = the
    /// first that matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallpaper_monitor: Option<String>,
}

impl Default for Cycle {
    fn default() -> Self {
        Cycle {
            mode: Mode::Off,
            items: Vec::new(),
            position: 0,
            step: Vec::new(),
            minutes: default_minutes(),
            follow: Follow::Nothing,
            layers: Vec::new(),
            wallpaper_monitor: None,
        }
    }
}

impl Cycle {
    /// The hotlist (the active profile's, else the base), from the store's `[art.cycle]`.
    /// Imported once from the `art-cycle.json` of earlier builds.
    pub fn load() -> Self {
        let cycle: Cycle = super::store::load_or_migrate("art.cycle", || {
            let json =
                crate::settings::Settings::config_file_path()?.with_file_name("art-cycle.json");
            let cycle = serde_json::from_slice(&std::fs::read(&json).ok()?).ok()?;
            Some((cycle, vec![json]))
        });
        cycle.migrated()
    }

    /// Turns the old single `mode` into the trigger set; the next save drops `mode`.
    pub fn migrated(mut self) -> Self {
        match std::mem::take(&mut self.mode) {
            Mode::Off => {}
            Mode::Minutes(m) => {
                self.minutes = m.max(1);
                self.set_step(Step::Minutes, true);
            }
            Mode::LightsWake => self.set_step(Step::LightsWake, true),
            Mode::Wallpaper => self.follow = Follow::Wallpaper,
        }
        self
    }

    pub fn save(&self) -> std::io::Result<()> {
        super::store::save("art.cycle", self)
    }

    pub fn steps_on(&self, step: Step) -> bool {
        self.step.contains(&step)
    }

    pub fn set_step(&mut self, step: Step, on: bool) {
        self.step.retain(|s| *s != step);
        if on {
            self.step.push(step);
        }
    }

    /// The art for a layer, if one is set.
    pub fn layer_art(&self, layer: u8) -> Option<&Item> {
        self.layers
            .iter()
            .find(|l| l.layer == layer)
            .map(|l| &l.art)
    }

    pub fn set_layer_art(&mut self, layer: u8, art: Option<Item>) {
        self.layers.retain(|l| l.layer != layer);
        if let Some(art) = art {
            self.layers.push(LayerArt { layer, art });
            self.layers.sort_by_key(|l| l.layer);
        }
    }

    /// How long until the next timed change; `None` when not cycling on a timer.
    pub fn period(&self) -> Option<Duration> {
        self.steps_on(Step::Minutes)
            .then(|| Duration::from_secs(u64::from(self.minutes.max(1)) * 60))
    }
}

/// Rotation's pieces on the keyboard as it reports itself: its own built-ins, then the kept
/// places that hold a piece.
pub fn rotation(st: &ArtStatus) -> Vec<Choice> {
    let built_ins = (0..st.right_set_count.min(127)).map(|n| Choice::BuiltIn(n as i8));
    let kept = (0..st.flash_slots.min(8))
        .filter(|n| st.flash_used & (1 << n) != 0)
        .map(Choice::Slot);
    built_ins.chain(kept).collect()
}

/// The piece on screen as a rotation entry, when it is one of the keyboard's own.
pub fn on_screen(st: &ArtStatus) -> Option<Choice> {
    match super::art_words::showing(st) {
        super::art_words::Showing::BuiltIn(n) => Some(Choice::BuiltIn(n)),
        super::art_words::Showing::Kept(n) => Some(Choice::Slot(n)),
        _ => None,
    }
}

/// The piece after (or before) `current` in `list`, wrapping round; from something not in
/// the list, the first (or the last). `None` when there is nothing else to show.
pub fn step(list: &[Choice], current: Option<Choice>, forward: bool) -> Option<Choice> {
    let n = list.len();
    let at = current.and_then(|c| list.iter().position(|x| *x == c));
    let next = match (at, forward) {
        (Some(i), true) => (i + 1) % n.max(1),
        (Some(i), false) => (i + n - 1) % n.max(1),
        (None, true) => 0,
        (None, false) => n.checked_sub(1)?,
    };
    let choice = list.get(next)?.clone();
    (Some(&choice) != at.map(|i| &list[i])).then_some(choice)
}

/// What an entry means on the keyboard as it is now: built-in names through the art manifest
/// (only when it matches this firmware), slots only while they hold a set.
pub fn resolve(item: &Item, set_count: u8, flash_used: u8) -> Option<Choice> {
    resolve_with(item, set_count, flash_used, super::art_sources::stand_in)
}

/// As `resolve`, with the stand-ins (art_sources) passed in: a built-in with a saved slot
/// standing in for it shows that slot, while the slot holds a set.
pub fn resolve_with(
    item: &Item,
    set_count: u8,
    flash_used: u8,
    stand_in: impl Fn(&str) -> Option<u8>,
) -> Option<Choice> {
    let used = |n: u8| n < 8 && flash_used & (1 << n) != 0;
    match item {
        Item::BuiltIn(name) if stand_in(name).is_some_and(used) => stand_in(name).map(Choice::Slot),
        Item::BuiltIn(name) => super::art_manifest::sets(set_count)?
            .iter()
            .find(|s| &s.name == name)
            .and_then(|s| i8::try_from(s.index).ok())
            .map(Choice::BuiltIn),
        Item::Slot(n) => used(*n).then_some(Choice::Slot(*n)),
        Item::File(p) => p.exists().then(|| Choice::File(p.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(set: i8, state: u8, used: u8) -> ArtStatus {
        ArtStatus {
            link: 2,
            right_state: state,
            right_set: set,
            right_set_count: 3,
            flash_slots: 8,
            flash_used: used,
            ..ArtStatus::default()
        }
    }

    /// Rotation is the keyboard's art: built-ins, then the kept places that hold a piece, the
    /// order the keyboard's own art keys use.
    #[test]
    fn rotation_is_what_the_keyboard_keeps() {
        let st = status(1, 0, 0b101); // place 2 (index 1) is free
        assert_eq!(
            rotation(&st),
            vec![
                Choice::BuiltIn(0),
                Choice::BuiltIn(1),
                Choice::BuiltIn(2),
                Choice::Slot(0),
                Choice::Slot(2)
            ]
        );
        let old = ArtStatus {
            flash_slots: 0,
            ..st
        };
        assert_eq!(rotation(&old).len(), 3, "firmware without places");
    }

    #[test]
    fn steps_wrap_both_ways() {
        let list = rotation(&status(0, 0, 0b1));
        let next = |c, f| step(&list, c, f);
        assert_eq!(
            next(Some(Choice::BuiltIn(0)), true),
            Some(Choice::BuiltIn(1))
        );
        assert_eq!(next(Some(Choice::Slot(0)), true), Some(Choice::BuiltIn(0)));
        assert_eq!(next(Some(Choice::BuiltIn(0)), false), Some(Choice::Slot(0)));
        assert_eq!(
            next(None, true),
            Some(Choice::BuiltIn(0)),
            "from a sent piece"
        );
        assert_eq!(next(None, false), Some(Choice::Slot(0)));
        assert_eq!(step(&[], None, true), None);
        assert_eq!(
            step(&[Choice::BuiltIn(0)], Some(Choice::BuiltIn(0)), true),
            None
        );
        // On screen: a kept place shows as set count + place, in state 0 after a select.
        assert_eq!(on_screen(&status(3, 0, 0b1)), Some(Choice::Slot(0)));
        assert_eq!(on_screen(&status(2, 0, 0b1)), Some(Choice::BuiltIn(2)));
        assert_eq!(on_screen(&status(-1, 2, 0b1)), None, "a sent piece");
    }

    #[test]
    fn the_timer_runs_while_minutes_is_on() {
        let mut c = Cycle::default();
        c.set_step(Step::Minutes, true);
        assert_eq!(
            c.period(),
            Some(Duration::from_secs(300)),
            "no list needed any more"
        );
        c.set_step(Step::Minutes, false);
        c.set_step(Step::LightsWake, true);
        assert_eq!(c.period(), None);
    }

    #[test]
    fn old_modes_migrate_to_triggers() {
        let old = |mode: &str| {
            toml::from_str::<Cycle>(&format!("mode = {mode}\nitems = []"))
                .unwrap()
                .migrated()
        };
        let m = old("{ Minutes = 7 }");
        assert_eq!(
            (m.step.clone(), m.minutes, m.follow),
            (vec![Step::Minutes], 7, Follow::Nothing)
        );
        assert_eq!(old("\"LightsWake\"").step, vec![Step::LightsWake]);
        let w = old("\"Wallpaper\"");
        assert_eq!((w.step.is_empty(), w.follow), (true, Follow::Wallpaper));
        assert_eq!(old("\"Off\""), Cycle::default());
        assert!(
            !toml::to_string(&m).unwrap().contains("mode"),
            "mode is never written back"
        );
    }

    #[test]
    fn layer_art_is_one_per_layer_and_sorted() {
        let mut c = Cycle::default();
        c.set_layer_art(3, Some(Item::BuiltIn("Bandit_Radar".into())));
        c.set_layer_art(1, Some(Item::Slot(0)));
        c.set_layer_art(3, Some(Item::Slot(2)));
        assert_eq!(
            c.layers.iter().map(|l| l.layer).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(c.layer_art(3), Some(&Item::Slot(2)));
        c.set_layer_art(1, None);
        assert_eq!(c.layer_art(1), None);
    }

    #[test]
    fn empty_slots_and_unknown_names_do_not_resolve() {
        let none = |_: &str| None;
        assert_eq!(
            resolve_with(&Item::Slot(3), 17, 0b1000, none),
            Some(Choice::Slot(3))
        );
        assert_eq!(resolve_with(&Item::Slot(3), 17, 0, none), None);
        assert_eq!(
            resolve_with(&Item::BuiltIn("no such set".into()), 17, 0, none),
            None
        );
        // a stand-in slot shows instead of the built-in, but only while it holds a set
        let gem = |n: &str| (n == "Bandit_Gem").then_some(2);
        assert_eq!(
            resolve_with(&Item::BuiltIn("Bandit_Gem".into()), 17, 0b100, gem),
            Some(Choice::Slot(2))
        );
        assert_ne!(
            resolve_with(&Item::BuiltIn("Bandit_Gem".into()), 17, 0, gem),
            Some(Choice::Slot(2))
        );
    }

    #[test]
    fn hotlist_round_trips_through_toml() {
        let c = Cycle {
            items: vec![
                Item::BuiltIn("ZMK_1".into()),
                Item::Slot(2),
                Item::File(PathBuf::from(r"C:\art\fox.bin")),
            ],
            position: 1,
            step: vec![Step::Minutes, Step::Key],
            minutes: 9,
            follow: Follow::Wallpaper,
            layers: vec![LayerArt {
                layer: 3,
                art: Item::BuiltIn("Bandit_Radar".into()),
            }],
            wallpaper_monitor: Some("Monitor0".into()),
            ..Cycle::default()
        };
        assert_eq!(
            toml::from_str::<Cycle>(&toml::to_string(&c).unwrap()).unwrap(),
            c
        );
        let off = Cycle {
            step: vec![Step::LightsWake],
            ..Cycle::default()
        };
        assert_eq!(
            toml::from_str::<Cycle>(&toml::to_string(&off).unwrap()).unwrap(),
            off
        );
    }
}
