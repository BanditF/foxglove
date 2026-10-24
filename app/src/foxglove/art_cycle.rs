//! The right art hotlist and how it cycles (#60), saved in the settings store's `[art.cycle]`
//! (profiled: each profile can have its own). The art service does the cycling; this module
//! only holds the list and picks the next entry.
//!
//! Built-ins are stored by their manifest name (the stable id), not their index, so a firmware
//! that reorders its sets still shows the same art.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::art_service::Choice;

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
            let json = crate::settings::Settings::config_file_path()?.with_file_name("art-cycle.json");
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
        self.layers.iter().find(|l| l.layer == layer).map(|l| &l.art)
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
        (self.steps_on(Step::Minutes) && !self.items.is_empty())
            .then(|| Duration::from_secs(u64::from(self.minutes.max(1)) * 60))
    }

    /// Moves to the next entry the keyboard can show and returns it as a choice. Entries that
    /// can't be shown now (a built-in this firmware lacks, an empty slot, a missing file) are
    /// skipped; `None` if none can.
    /// Forwards (`dir` > 0) or backwards (the art key's previous).
    pub fn step_by(&mut self, dir: i8, resolve: impl Fn(&Item) -> Option<Choice>) -> Option<Choice> {
        let n = self.items.len();
        (1..=n).find_map(|step| {
            let i = if dir < 0 { (self.position + n - step) % n } else { (self.position + step) % n };
            let choice = resolve(&self.items[i])?;
            self.position = i;
            Some(choice)
        })
    }

    /// Whether the app takes the art keys: only while they'd step something.
    pub fn wants_art_keys(&self) -> bool {
        self.steps_on(Step::Key) && !self.items.is_empty()
    }
}

/// What an entry means on the keyboard as it is now: built-in names through the art manifest
/// (only when it matches this firmware), slots only while they hold a set.
pub fn resolve(item: &Item, set_count: u8, flash_used: u8) -> Option<Choice> {
    match item {
        Item::BuiltIn(name) => super::art_manifest::sets(set_count)?
            .iter()
            .find(|s| &s.name == name)
            .and_then(|s| i8::try_from(s.index).ok())
            .map(Choice::BuiltIn),
        Item::Slot(n) => (*n < 8 && flash_used & (1 << n) != 0).then_some(Choice::Slot(*n)),
        Item::File(p) => p.exists().then(|| Choice::File(p.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_round_robin_and_skips_what_cannot_show() {
        let mut c = Cycle {
            items: vec![Item::Slot(0), Item::Slot(1), Item::Slot(2)],
            step: vec![Step::Minutes],
            ..Cycle::default()
        };
        let used = 0b101; // slot 1 is empty
        let r = |i: &Item| match i {
            Item::Slot(n) => (used & (1 << n) != 0).then_some(Choice::Slot(*n)),
            _ => None,
        };
        assert_eq!(c.step_by(1, r), Some(Choice::Slot(2)));
        assert_eq!(c.step_by(1, r), Some(Choice::Slot(0)));
        assert_eq!(c.position, 0);
        assert_eq!(c.step_by(1, |_| None), None, "nothing showable");
        assert_eq!(c.step_by(-1, r), Some(Choice::Slot(2)), "back from 0 wraps to the end");
        assert_eq!(c.step_by(-1, r), Some(Choice::Slot(0)), "and skips the empty slot");
        assert_eq!(c.period(), Some(Duration::from_secs(300)));
        c.set_step(Step::Minutes, false);
        c.set_step(Step::LightsWake, true);
        assert_eq!(c.period(), None);
    }

    #[test]
    fn old_modes_migrate_to_triggers() {
        let old = |mode: &str| toml::from_str::<Cycle>(&format!("mode = {mode}\nitems = []")).unwrap().migrated();
        let m = old("{ Minutes = 7 }");
        assert_eq!((m.step.clone(), m.minutes, m.follow), (vec![Step::Minutes], 7, Follow::Nothing));
        assert_eq!(old("\"LightsWake\"").step, vec![Step::LightsWake]);
        let w = old("\"Wallpaper\"");
        assert_eq!((w.step.is_empty(), w.follow), (true, Follow::Wallpaper));
        assert_eq!(old("\"Off\""), Cycle::default());
        assert!(!toml::to_string(&m).unwrap().contains("mode"), "mode is never written back");
    }

    #[test]
    fn layer_art_is_one_per_layer_and_sorted() {
        let mut c = Cycle::default();
        c.set_layer_art(3, Some(Item::BuiltIn("Bandit_Radar".into())));
        c.set_layer_art(1, Some(Item::Slot(0)));
        c.set_layer_art(3, Some(Item::Slot(2)));
        assert_eq!(c.layers.iter().map(|l| l.layer).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(c.layer_art(3), Some(&Item::Slot(2)));
        c.set_layer_art(1, None);
        assert_eq!(c.layer_art(1), None);
    }

    #[test]
    fn empty_slots_and_unknown_names_do_not_resolve() {
        assert_eq!(resolve(&Item::Slot(3), 17, 0b1000), Some(Choice::Slot(3)));
        assert_eq!(resolve(&Item::Slot(3), 17, 0), None);
        assert_eq!(resolve(&Item::BuiltIn("no such set".into()), 17, 0), None);
    }

    #[test]
    fn hotlist_round_trips_through_toml() {
        let c = Cycle {
            items: vec![Item::BuiltIn("ZMK_1".into()), Item::Slot(2), Item::File(PathBuf::from(r"C:\art\fox.bin"))],
            position: 1,
            step: vec![Step::Minutes, Step::Key],
            minutes: 9,
            follow: Follow::Wallpaper,
            layers: vec![LayerArt { layer: 3, art: Item::BuiltIn("Bandit_Radar".into()) }],
            wallpaper_monitor: Some("Monitor0".into()),
            ..Cycle::default()
        };
        assert_eq!(toml::from_str::<Cycle>(&toml::to_string(&c).unwrap()).unwrap(), c);
        let off = Cycle { step: vec![Step::LightsWake], ..Cycle::default() };
        assert_eq!(toml::from_str::<Cycle>(&toml::to_string(&off).unwrap()).unwrap(), off);
    }
}
