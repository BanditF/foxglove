//! The art playlist (#522): Rotation steps through a list of pieces longer than the keyboard
//! keeps. Pieces from this PC go into the kept places lent to the playlist (`Cycle::places`)
//! ahead of each step, with STORE (D6 09: kept without being shown), so the step itself is a
//! SELECT of that place: instant, and the keyboard keeps playing the last pieces without
//! Foxglove. Built-ins and the user's own kept places in the list show as themselves.
//!
//! Emulates Wallpaper Engine's playlists (a list, an order, a timer or an event to move on) and
//! a watch's face rotation over what is installed on it; the staging is ours, from the keyboard's
//! limits:
//! - **Relay timing:** a 4 KiB piece takes about 1 s over the split link under its latency
//!   (relay-sim `playlist-store`), plus about 0.65 s of flash write, so the next piece is staged
//!   right after a step, well before the next one is due (a minute at the least).
//! - **Flash wear:** a place lasts about 10 000 writes. The playlist writes a place at most once
//!   per [`PLACE_GAP`] and stops at [`WEAR_STOP`] writes; a step whose piece isn't staged then
//!   sends it from this PC instead (about 1 s, not kept).
//! - **Firmware without STORE:** every step sends its piece; nothing is staged.
//!
//! Pure: the art service asks these what to show and what to stage; the tests check them
//! without a keyboard. No timers of its own: staging starts on the step that was just shown.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use foxglove_link::protocol::{ArtStatus, SlotInfo};

use super::art_cycle::{Cycle, Item};
use super::art_service::Choice;

/// A place's flash endurance (the nRF52840's, in erase/write cycles).
pub const WEAR_LIMIT: u32 = 10_000;
/// The playlist leaves a place alone from here on (90% of [`WEAR_LIMIT`]).
pub const WEAR_STOP: u32 = WEAR_LIMIT / 10 * 9;
/// The least time between two playlist writes of one place: 24 a day at most, so a place lent
/// to it lasts more than a year.
pub const PLACE_GAP: Duration = Duration::from_secs(60 * 60);
/// At most this many places can be lent: two is enough for one showing and one staged.
pub const MAX_PLACES: usize = 2;

/// Whether Rotation plays the playlist: on, with something in it.
pub fn active(c: &Cycle) -> bool {
    c.playlist && !c.items.is_empty()
}

/// A piece in a place as the keyboard names it: the place's name and its length.
pub type Key = (String, usize);

/// The lent place that holds the piece `key` now (its name and length, as the Screens page
/// matches places to pieces).
pub fn place_of(lent: &[u8], slots: &[SlotInfo], key: &Key) -> Option<u8> {
    lent.iter().copied().find(|n| {
        slots
            .iter()
            .any(|i| i.slot == *n && i.used && i.name == key.0 && usize::from(i.len) == key.1)
    })
}

/// What the entry `item` shows now: a built-in or kept place as itself (`resolve`), a file from
/// the lent place that holds it, else sent from this PC. `None` when it can't show (a missing
/// file, a built-in this firmware doesn't have).
pub fn choice_for(
    item: &Item,
    c: &Cycle,
    slots: &[SlotInfo],
    key_of: &dyn Fn(&Path) -> Option<Key>,
    resolve: &dyn Fn(&Item) -> Option<Choice>,
) -> Option<Choice> {
    match item {
        Item::File(p) => {
            let key = key_of(p)?;
            Some(
                place_of(&c.places, slots, &key)
                    .map_or_else(|| Choice::File(p.clone()), Choice::Slot),
            )
        }
        other => resolve(other),
    }
}

/// The step after (or before) the entry at `c.position`: its new position and what it shows,
/// skipping entries that can't show. `None` when nothing in the list can.
pub fn step(
    c: &Cycle,
    slots: &[SlotInfo],
    forward: bool,
    key_of: &dyn Fn(&Path) -> Option<Key>,
    resolve: &dyn Fn(&Item) -> Option<Choice>,
) -> Option<(usize, Choice)> {
    let n = c.items.len();
    if n == 0 {
        return None;
    }
    let at = c.position % n;
    (1..=n).find_map(|k| {
        let i = if forward {
            (at + k) % n
        } else {
            (at + n - k % n) % n
        };
        choice_for(&c.items[i], c, slots, key_of, resolve).map(|ch| (i, ch))
    })
}

/// A piece to put into a lent place ahead of the next step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    pub file: PathBuf,
    pub slot: u8,
    /// The name it is kept under (cut to the place's 16 bytes).
    pub name: String,
}

/// What to stage after the step to `c.position`: the next entry, when it is a file no lent
/// place holds yet, into the lent place that isn't on screen and was written least, while that
/// place is under [`WEAR_STOP`] and its last playlist write was [`PLACE_GAP`] ago. `Ok(None)`
/// when nothing needs staging; `Err` says why the next piece will be sent at its step instead.
pub fn stage(
    c: &Cycle,
    st: &ArtStatus,
    slots: &[SlotInfo],
    key_of: &dyn Fn(&Path) -> Option<Key>,
    last_write: &dyn Fn(u8) -> Option<Instant>,
    now: Instant,
    can_store: bool,
) -> Result<Option<Stage>, String> {
    let n = c.items.len();
    if n < 2 {
        return Ok(None);
    }
    // The next entry that is a file; built-ins and kept places need nothing.
    let next = (1..n)
        .map(|k| &c.items[(c.position + k) % n])
        .find(|i| match i {
            Item::File(p) => key_of(p).is_some(),
            _ => false,
        });
    let Some(Item::File(file)) = next else {
        return Ok(None);
    };
    let Some(key) = key_of(file) else {
        return Ok(None);
    };
    if place_of(&c.places, slots, &key).is_some() {
        return Ok(None); // staged already
    }
    if !can_store {
        return Err("the keyboard's firmware can't keep a piece without showing it".into());
    }
    if c.places.is_empty() {
        return Err("no places lent to the playlist".into());
    }
    let shown = match super::art_cycle::on_screen(st) {
        Some(Choice::Slot(n)) => Some(n),
        _ => None,
    };
    let places = usize::from(st.flash_slots.min(8));
    let writes = |n: u8| slots.iter().find(|i| i.slot == n).map_or(0, |i| i.writes);
    let free: Vec<u8> = c
        .places
        .iter()
        .copied()
        .filter(|n| usize::from(*n) < places && Some(*n) != shown)
        .collect();
    if free.is_empty() {
        return Err("the only lent place is on screen".into());
    }
    let unworn: Vec<u8> = free
        .iter()
        .copied()
        .filter(|n| writes(*n) < WEAR_STOP)
        .collect();
    if unworn.is_empty() {
        return Err(format!(
            "the lent places are worn ({WEAR_STOP} writes or more)"
        ));
    }
    let rested: Vec<u8> = unworn
        .iter()
        .copied()
        .filter(|n| last_write(*n).is_none_or(|at| now.saturating_duration_since(at) >= PLACE_GAP))
        .collect();
    let Some(slot) = rested.iter().copied().min_by_key(|n| writes(*n)) else {
        return Err(format!(
            "each lent place was written in the last {} min (flash wear)",
            PLACE_GAP.as_secs() / 60
        ));
    };
    Ok(Some(Stage {
        file: file.clone(),
        slot,
        name: key.0,
    }))
}

/// Places to lend when the playlist is turned on: the highest free ones, up to `want`.
pub fn lend(st: &ArtStatus, want: usize) -> Vec<u8> {
    let places = st.flash_slots.min(8);
    let mut out: Vec<u8> = (0..places)
        .rev()
        .filter(|n| st.flash_used & (1 << n) == 0)
        .take(want.min(MAX_PLACES))
        .collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(n: u8, name: &str, len: u16, writes: u32) -> SlotInfo {
        SlotInfo {
            slot: n,
            used: true,
            writes,
            len,
            name: name.into(),
        }
    }

    fn status(set: i8, used: u8) -> ArtStatus {
        ArtStatus {
            link: 2,
            right_state: 0,
            right_set: set,
            right_set_count: 1,
            flash_slots: 8,
            flash_used: used,
            ..Default::default()
        }
    }

    fn cycle(items: Vec<Item>, places: Vec<u8>) -> Cycle {
        Cycle {
            items,
            places,
            playlist: true,
            ..Cycle::default()
        }
    }

    fn file(name: &str) -> Item {
        Item::File(PathBuf::from(format!("art/{name}.bin")))
    }

    // Every file "exists" and is 1000 bytes; "gone" doesn't.
    fn key_of(p: &Path) -> Option<Key> {
        let name = p.file_stem()?.to_string_lossy().into_owned();
        (name != "gone").then_some((name, 1000))
    }

    fn resolve(i: &Item) -> Option<Choice> {
        match i {
            Item::BuiltIn(n) if n == "Bandit_Emblem" => Some(Choice::BuiltIn(0)),
            Item::Slot(n) => Some(Choice::Slot(*n)),
            _ => None,
        }
    }

    /// A step shows a staged file from its place (instant), an unstaged one sent from this PC,
    /// built-ins and kept places as themselves, and skips what can't show; both ways round.
    #[test]
    fn a_step_shows_the_staged_place_or_sends() {
        let mut c = cycle(
            vec![
                file("radar"),
                file("orbit"),
                file("gone"),
                Item::BuiltIn("Bandit_Emblem".into()),
                Item::Slot(2),
                Item::BuiltIn("no such set".into()),
            ],
            vec![6, 7],
        );
        let slots = vec![slot(6, "radar", 1000, 3), slot(7, "orbit", 999, 1)];
        c.position = 5;
        let step = |c: &Cycle, f| step(c, &slots, f, &key_of, &resolve);
        assert_eq!(
            step(&c, true),
            Some((0, Choice::Slot(6))),
            "staged: instant"
        );
        c.position = 0;
        assert_eq!(
            step(&c, true),
            Some((1, Choice::File(PathBuf::from("art/orbit.bin")))),
            "place 7 holds another length: sent"
        );
        c.position = 1;
        assert_eq!(
            step(&c, true),
            Some((3, Choice::BuiltIn(0))),
            "a missing file is skipped"
        );
        c.position = 3;
        assert_eq!(step(&c, true), Some((4, Choice::Slot(2))));
        assert_eq!(
            step(&c, false),
            Some((1, Choice::File("art/orbit.bin".into())))
        );
        c.position = 0;
        assert_eq!(
            step(&c, false),
            Some((4, Choice::Slot(2))),
            "back past the unknown built-in"
        );
        assert_eq!(step(&cycle(vec![], vec![]), true), None);
        assert_eq!(step(&cycle(vec![file("gone")], vec![]), true), None);
        assert!(!active(&cycle(vec![], vec![6])));
        assert!(active(&cycle(vec![file("radar")], vec![])));
    }

    /// The next file goes into the lent place not on screen, the one written least; nothing
    /// when it is staged already or the next entry is a built-in.
    #[test]
    fn the_next_piece_is_staged_into_the_other_place() {
        let now = Instant::now();
        let never = |_: u8| None;
        let mut c = cycle(
            vec![
                file("radar"),
                file("orbit"),
                Item::BuiltIn("Bandit_Emblem".into()),
            ],
            vec![6, 7],
        );
        let slots = vec![slot(6, "radar", 1000, 40), slot(7, "old", 500, 12)];
        // Showing place 6 (radar, set 1 + 6): orbit goes into 7.
        let st = status(7, 0b1100_0000);
        let plan = stage(&c, &st, &slots, &key_of, &never, now, true).unwrap();
        assert_eq!(
            plan,
            Some(Stage {
                file: "art/orbit.bin".into(),
                slot: 7,
                name: "orbit".into()
            })
        );
        // Showing a built-in: the place written less (7) even though 6 is free of the screen.
        let st0 = status(0, 0b1100_0000);
        assert_eq!(
            stage(&c, &st0, &slots, &key_of, &never, now, true)
                .unwrap()
                .unwrap()
                .slot,
            7
        );
        // Orbit already in 7: nothing to do.
        let staged = vec![slot(6, "radar", 1000, 40), slot(7, "orbit", 1000, 13)];
        assert_eq!(
            stage(&c, &st, &staged, &key_of, &never, now, true),
            Ok(None)
        );
        // The next file after orbit (position 1) is radar, two entries on, past the built-in.
        c.position = 1;
        assert_eq!(
            stage(
                &c,
                &status(8, 0b1100_0000),
                &staged,
                &key_of,
                &never,
                now,
                true
            ),
            Ok(None),
            "radar is in 6 already"
        );
        c.position = 2;
        assert_eq!(
            stage(&c, &st0, &staged, &key_of, &never, now, true),
            Ok(None)
        );
    }

    /// Why nothing is staged: firmware without STORE, no places, only the shown place, worn
    /// places, places written too recently. Each reason is what the health row says.
    #[test]
    fn staging_respects_the_places_limits() {
        let t0 = Instant::now();
        let never = |_: u8| None;
        let c = cycle(vec![file("radar"), file("orbit")], vec![7]);
        let st = status(8, 0b1000_0000); // place 7 on screen
        let slots = vec![slot(7, "radar", 1000, 3)];
        let why = |c: &Cycle, slots: &[SlotInfo], store: bool| {
            stage(c, &st, slots, &key_of, &never, t0, store).unwrap_err()
        };
        assert!(why(&c, &slots, false).contains("can't keep a piece"));
        assert!(why(&cycle(c.items.clone(), vec![]), &slots, true).contains("no places"));
        assert!(why(&c, &slots, true).contains("on screen"));
        let c2 = cycle(c.items.clone(), vec![6, 7]);
        let worn = vec![slot(6, "x", 1, WEAR_STOP), slot(7, "radar", 1000, 3)];
        assert!(why(&c2, &worn, true).contains("worn"));
        // Place 6 written at t0: a minute later it rests; a PLACE_GAP later it takes the next.
        let at_t0 = |_: u8| Some(t0);
        let soon = stage(
            &c2,
            &st,
            &slots,
            &key_of,
            &at_t0,
            t0 + Duration::from_secs(60),
            true,
        );
        assert!(soon.unwrap_err().contains("flash wear"));
        let later = stage(&c2, &st, &slots, &key_of, &at_t0, t0 + PLACE_GAP, true);
        assert_eq!(later.unwrap().map(|s| s.slot), Some(6));
    }

    /// Turning the playlist on lends the highest free places, two at most.
    #[test]
    fn the_highest_free_places_are_lent() {
        assert_eq!(lend(&status(0, 0b0001_1111), 2), vec![6, 7]);
        assert_eq!(lend(&status(0, 0b1011_1111), 2), vec![6]);
        assert_eq!(lend(&status(0, 0xff), 2), Vec::<u8>::new());
        assert_eq!(lend(&status(0, 0), 5).len(), MAX_PLACES);
    }
}
