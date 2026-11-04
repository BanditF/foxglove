//! Art across firmware updates (#521): what the keyboard held when Foxglove last saw it (its
//! built-ins by name and its kept pieces), in the store's `[art.seen]`. When a connect reports
//! something else, the Screens page says what happened: the kept pieces still there (they live
//! above the firmware image, so an update leaves them), and the pieces that came or went with
//! the new firmware. Read from the keyboard on the connect after the update; no polling.
//!
//! Places a connect finds empty that held a piece before (a settings reset, an update that
//! moved the art partition) are put back from this PC's copy (#521, [`plan`]): the art service
//! sends each one into its old place with STORE, without showing it, and says "Foxglove put back
//! N of M" ([`restored_line`]; health row "art restored N/M").

use serde::{Deserialize, Serialize};

use super::art_service::Snapshot;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    /// The built-ins' labels (only meaningful when `named`).
    pub built_ins: Vec<String>,
    /// The art manifest matched the firmware, so the labels are real names.
    pub named: bool,
    /// The kept pieces' names.
    pub kept: Vec<String>,
    /// The kept places with what each held (#521), so a lost one can be put back.
    pub places: Vec<Place>,
}

/// A kept place and the piece in it, as the keyboard reports it (its name and length).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    pub slot: u8,
    pub name: String,
    pub len: u16,
}

impl Seen {
    /// What `s` reports now, once its kept places have been read; `None` before.
    pub fn of(s: &Snapshot) -> Option<Seen> {
        let st = s.status?;
        // Until the right half is linked the left reports it zeroed: no built-ins, no places.
        if st.link != 2 || st.right_set_count == 0 {
            return None;
        }
        if st.flash_slots > 0 && s.slots.len() < usize::from(st.flash_slots.min(8)) {
            return None; // the places aren't read yet
        }
        let places = s
            .slots
            .iter()
            .filter(|i| i.used && !s.cycle.places.contains(&i.slot))
            .map(|i| Place {
                slot: i.slot,
                name: i.name.clone(),
                len: i.len,
            })
            .collect();
        Some(Seen {
            places,
            built_ins: s.built_ins().into_iter().map(|(_, name)| name).collect(),
            named: super::art_manifest::sets(st.right_set_count).is_some(),
            kept: s
                .slots
                .iter()
                // A place lent to the playlist (#522) changes with it: not one of the user's.
                .filter(|i| i.used && !s.cycle.places.contains(&i.slot))
                .map(|i| i.name.clone())
                .collect(),
        })
    }
}

pub fn load() -> Option<Seen> {
    super::store::try_load::<Seen>("art.seen").filter(|s| *s != Seen::default())
}

pub fn save(seen: &Seen) {
    if let Err(e) = super::store::save("art.seen", seen) {
        eprintln!("Foxglove: can't save what the keyboard holds: {e}");
    }
}

/// The places that held a piece at the last look and don't hold it now.
pub fn lost(before: &Seen, now: &Seen) -> Vec<Place> {
    before
        .places
        .iter()
        .filter(|p| !now.places.contains(p))
        .cloned()
        .collect()
}

/// How to put the lost places back: each into its old place from the PC file with the same
/// name and length (`files`: a file and the name and length it is kept under). Not put back,
/// with the reason: a place that holds another piece now, a piece this PC has no copy of.
#[allow(clippy::type_complexity)]
pub fn plan(
    lost: &[Place],
    flash_used: u8,
    files: &[(std::path::PathBuf, (String, usize))],
) -> (Vec<(Place, std::path::PathBuf)>, Vec<(String, String)>) {
    let mut go = Vec::new();
    let mut not = Vec::new();
    for p in lost {
        if p.slot < 8 && flash_used & (1 << p.slot) != 0 {
            not.push((p.name.clone(), "its place holds another piece now".into()));
            continue;
        }
        match files
            .iter()
            .find(|(_, (name, len))| *name == p.name && *len == usize::from(p.len))
        {
            Some((file, _)) => go.push((p.clone(), file.clone())),
            None => not.push((p.name.clone(), "this PC has no copy of it".into())),
        }
    }
    (go, not)
}

/// What a restore did, in words: "Foxglove put back 2 of 3 kept pieces: fury, radar. Not put
/// back: gem (this PC has no copy of it)."
pub fn restored_line(restored: &[String], failed: &[(String, String)], total: usize) -> String {
    let mut out = match restored.len() {
        0 => format!("Foxglove couldn't put back the {total} lost pieces."),
        n if n == total && n == 1 => format!("Foxglove put back the lost piece: {}.", restored[0]),
        n => format!(
            "Foxglove put back {n} of {total} kept pieces: {}.",
            restored.join(", ")
        ),
    };
    if !failed.is_empty() {
        let why: Vec<String> = failed.iter().map(|(n, e)| format!("{n} ({e})")).collect();
        out.push_str(&format!(" Not put back: {}.", why.join(", ")));
    }
    out
}

/// The connect's notice with a restore's line in place of its "No longer kept on the keyboard:
/// …." sentence, which the restore answers.
pub fn with_restored(notice: Option<String>, line: &str) -> String {
    const LOST: &str = "No longer kept on the keyboard: ";
    let Some(notice) = notice else {
        return line.to_string();
    };
    match notice.find(LOST) {
        Some(at) => {
            let end = notice[at..].find(". ").map_or(notice.len(), |e| at + e + 2);
            let rest = notice[end..].trim();
            let head = notice[..at].trim();
            [head, line, rest]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
        None => format!("{notice} {line}"),
    }
}

/// What changed between two looks at the keyboard, in words, when its built-ins changed (a
/// firmware update) or kept pieces went missing; `None` when nothing worth saying did.
pub fn change(before: &Seen, now: &Seen) -> Option<String> {
    // Without names on both sides only the count can tell an update.
    let named = before.named && now.named;
    let renamed = !named && before.built_ins.len() != now.built_ins.len();
    let came: Vec<&String> = now
        .built_ins
        .iter()
        .filter(|b| named && !before.built_ins.contains(b))
        .collect();
    let went: Vec<&String> = before
        .built_ins
        .iter()
        .filter(|b| named && !now.built_ins.contains(b))
        .collect();
    let lost: Vec<&String> = before
        .kept
        .iter()
        .filter(|k| !now.kept.contains(k))
        .collect();
    let updated = !came.is_empty() || !went.is_empty() || renamed;
    if !updated && lost.is_empty() {
        return None;
    }
    let list = |v: &[&String]| v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    let mut out = Vec::new();
    if updated {
        out.push("The keyboard's firmware changed.".to_string());
    }
    let kept = now.kept.len();
    if lost.is_empty() {
        out.push(match kept {
            0 => "It keeps none of your pieces.".into(),
            1 => "Your kept piece is still on the keyboard.".into(),
            n => format!("Your {n} kept pieces are still on the keyboard."),
        });
    } else {
        out.push(format!("No longer kept on the keyboard: {}.", list(&lost)));
    }
    if !came.is_empty() {
        out.push(format!("New with the keyboard: {}.", list(&came)));
    }
    if !went.is_empty() {
        out.push(format!("No longer with the keyboard: {}.", list(&went)));
    }
    Some(out.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(built_ins: &[&str], kept: &[&str]) -> Seen {
        Seen {
            built_ins: built_ins.iter().map(|s| s.to_string()).collect(),
            named: true,
            kept: kept.iter().map(|s| s.to_string()).collect(),
            places: Vec::new(),
        }
    }

    /// Two firmware manifests: one piece gone, one new, the five kept pieces all still there.
    #[test]
    fn a_firmware_update_says_what_happened_to_the_art() {
        let kept = ["fury", "radar", "gem", "skyline", "vitals"];
        let before = seen(&["ZMK balloon", "Bandit gem", "Bandit boot"], &kept);
        let after = seen(&["ZMK balloon", "Bandit gem", "Bandit skyline"], &kept);
        assert_eq!(
            change(&before, &after).as_deref(),
            Some(
                "The keyboard's firmware changed. Your 5 kept pieces are still on the keyboard. \
                 New with the keyboard: Bandit skyline. No longer with the keyboard: Bandit boot."
            )
        );
        assert_eq!(change(&before, &before), None, "nothing to say");
        let wiped = seen(&["ZMK balloon", "Bandit gem", "Bandit boot"], &[]);
        assert_eq!(
            change(&before, &wiped).as_deref(),
            Some("No longer kept on the keyboard: fury, radar, gem, skyline, vitals.")
        );
        // Unknown firmware: no names to compare, only the count.
        let unknown = Seen {
            named: false,
            built_ins: vec!["Set 0".into(), "Set 1".into()],
            ..after.clone()
        };
        assert_eq!(
            change(&before, &unknown).as_deref(),
            Some("The keyboard's firmware changed. Your 5 kept pieces are still on the keyboard.")
        );
        let one = seen(&["a"], &["fury"]);
        let one_after = seen(&["b"], &["fury"]);
        assert!(change(&one, &one_after)
            .unwrap()
            .contains("Your kept piece is still on the keyboard."));
    }

    use std::path::PathBuf;

    fn place(slot: u8, name: &str, len: u16) -> Place {
        Place {
            slot,
            name: name.into(),
            len,
        }
    }

    /// Places lost since the last look are put back from the PC copy with the same name and
    /// length; a place that holds something else now, or a piece the PC lacks, is said so.
    #[test]
    fn lost_places_are_planned_back_from_this_pc() {
        let before = Seen {
            places: vec![
                place(0, "fury", 900),
                place(1, "radar", 3600),
                place(2, "gem", 2048),
                place(3, "orbit", 1000),
            ],
            ..Seen::default()
        };
        let now = Seen {
            places: vec![place(0, "fury", 900)],
            ..Seen::default()
        };
        let lost = lost(&before, &now);
        assert_eq!(
            lost.iter().map(|p| p.slot).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let files = vec![
            (PathBuf::from("art/radar.bin"), ("radar".to_string(), 3600)),
            (PathBuf::from("art/gem.bin"), ("gem".to_string(), 2047)),
            (PathBuf::from("art/orbit.bin"), ("orbit".to_string(), 1000)),
        ];
        // Place 4 (index 3) holds another piece now.
        let (go, not) = plan(&lost, 0b1001, &files);
        assert_eq!(
            go,
            vec![(place(1, "radar", 3600), PathBuf::from("art/radar.bin"))]
        );
        assert_eq!(
            not,
            vec![
                ("gem".to_string(), "this PC has no copy of it".to_string()),
                (
                    "orbit".to_string(),
                    "its place holds another piece now".to_string()
                )
            ]
        );
        assert_eq!(
            restored_line(&["radar".into()], &not, 3),
            "Foxglove put back 1 of 3 kept pieces: radar. Not put back: gem (this PC has no copy \
             of it), orbit (its place holds another piece now)."
        );
        assert_eq!(
            restored_line(&["fury".into()], &[], 1),
            "Foxglove put back the lost piece: fury."
        );
        assert!(restored_line(&[], &not, 2).starts_with("Foxglove couldn't put back the 2"));
        // The restore's line answers the connect's "No longer kept" sentence.
        let notice = "The keyboard's firmware changed. No longer kept on the keyboard: radar, \
                      gem. New with the keyboard: Bandit skyline.";
        assert_eq!(
            with_restored(
                Some(notice.into()),
                "Foxglove put back 2 of 2 kept pieces: radar, gem."
            ),
            "The keyboard's firmware changed. Foxglove put back 2 of 2 kept pieces: radar, gem. \
             New with the keyboard: Bandit skyline."
        );
        assert_eq!(with_restored(None, "x."), "x.");
        assert_eq!(with_restored(Some("a.".into()), "x."), "a. x.");
        assert!(super::lost(&now, &now).is_empty());
    }

    #[test]
    fn seen_waits_for_the_places() {
        let unlinked = Snapshot {
            status: Some(foxglove_link::protocol::ArtStatus {
                link: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(Seen::of(&unlinked), None, "the right half isn't linked yet");
        let mut s = Snapshot {
            status: Some(foxglove_link::protocol::ArtStatus {
                link: 2,
                right_set_count: 2,
                flash_slots: 8,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(Seen::of(&s), None, "places not read yet");
        s.slots = (0..8)
            .map(|n| foxglove_link::protocol::SlotInfo {
                slot: n,
                used: n == 1,
                name: "fury".into(),
                ..Default::default()
            })
            .collect();
        let seen = Seen::of(&s).unwrap();
        assert_eq!(seen.kept, vec!["fury".to_string()]);
        assert_eq!(
            seen.places.iter().map(|p| p.slot).collect::<Vec<_>>(),
            vec![1]
        );
        // A place lent to the playlist (#522) isn't one of the user's.
        s.cycle.places = vec![1];
        assert!(Seen::of(&s).unwrap().places.is_empty());
        assert_eq!(seen.built_ins.len(), 2);
    }
}
