//! The right art hotlist and how it cycles (#60), saved in `art-cycle.json` next to the other
//! settings. The art service does the cycling; this module only holds the list and picks the
//! next entry.
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cycle {
    pub mode: Mode,
    pub items: Vec<Item>,
    /// The entry shown last, so a restart carries on where it left off.
    #[serde(default)]
    pub position: usize,
}

impl Cycle {
    pub fn path() -> Option<PathBuf> {
        crate::settings::Settings::config_file_path().map(|p| p.with_file_name("art-cycle.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path().ok_or_else(|| std::io::Error::other("no settings directory"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)
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
}
