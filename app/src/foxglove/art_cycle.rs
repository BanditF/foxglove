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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Off,
    /// The next entry every so many minutes, while a keyboard is connected.
    Minutes(u32),
    /// The next entry each time the lights wake after going off (needs firmware support).
    LightsWake,
    /// The art that goes with the Wallpaper Engine wallpaper on screen (art\\wallpapers.json).
    Wallpaper,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cycle {
    pub mode: Mode,
    pub items: Vec<Item>,
    /// The entry shown last, so a restart carries on where it left off.
    #[serde(default)]
    pub position: usize,
    /// Which monitor's wallpaper to follow (a Wallpaper Engine monitor key); `None` = the
    /// first that matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallpaper_monitor: Option<String>,
}

impl Cycle {
    /// The hotlist (the active profile's, else the base), from the store's `[art.cycle]`.
    /// Imported once from the `art-cycle.json` of earlier builds.
    pub fn load() -> Self {
        super::store::load_or_migrate("art.cycle", || {
            let json = crate::settings::Settings::config_file_path()?.with_file_name("art-cycle.json");
            let cycle = serde_json::from_slice(&std::fs::read(&json).ok()?).ok()?;
            Some((cycle, vec![json]))
        })
    }

    pub fn save(&self) -> std::io::Result<()> {
        super::store::save("art.cycle", self)
    }

    /// How long until the next timed change; `None` when not cycling on a timer.
    pub fn period(&self) -> Option<Duration> {
        match self.mode {
            Mode::Minutes(m) if !self.items.is_empty() => {
                Some(Duration::from_secs(u64::from(m.max(1)) * 60))
            }
            _ => None,
        }
    }

    /// Moves to the next entry the keyboard can show and returns it as a choice. Entries that
    /// can't be shown now (a built-in this firmware lacks, an empty slot, a missing file) are
    /// skipped; `None` if none can.
    pub fn advance(&mut self, resolve: impl Fn(&Item) -> Option<Choice>) -> Option<Choice> {
        let n = self.items.len();
        (1..=n).find_map(|step| {
            let i = (self.position + step) % n;
            let choice = resolve(&self.items[i])?;
            self.position = i;
            Some(choice)
        })
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
            mode: Mode::Minutes(5),
            items: vec![Item::Slot(0), Item::Slot(1), Item::Slot(2)],
            position: 0,
            wallpaper_monitor: None,
        };
        let used = 0b101; // slot 1 is empty
        let r = |i: &Item| match i {
            Item::Slot(n) => (used & (1 << n) != 0).then_some(Choice::Slot(*n)),
            _ => None,
        };
        assert_eq!(c.advance(r), Some(Choice::Slot(2)));
        assert_eq!(c.advance(r), Some(Choice::Slot(0)));
        assert_eq!(c.position, 0);
        assert_eq!(c.advance(|_| None), None, "nothing showable");
        assert_eq!(c.period(), Some(Duration::from_secs(300)));
        c.mode = Mode::LightsWake;
        assert_eq!(c.period(), None);
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
            mode: Mode::Minutes(5),
            items: vec![Item::BuiltIn("ZMK_1".into()), Item::Slot(2), Item::File(PathBuf::from(r"C:rtox.bin"))],
            position: 1,
            wallpaper_monitor: Some("Monitor0".into()),
        };
        assert_eq!(toml::from_str::<Cycle>(&toml::to_string(&c).unwrap()).unwrap(), c);
        let off = Cycle { mode: Mode::LightsWake, ..Cycle::default() };
        assert_eq!(toml::from_str::<Cycle>(&toml::to_string(&off).unwrap()).unwrap(), off);
    }
}
