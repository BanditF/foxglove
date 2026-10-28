//! Art across firmware updates (#521): what the keyboard held when Foxglove last saw it (its
//! built-ins by name and its kept pieces), in the store's `[art.seen]`. When a connect reports
//! something else, the Screens page says what happened: the kept pieces still there (they live
//! above the firmware image, so an update leaves them), and the pieces that came or went with
//! the new firmware. Read from the keyboard on the connect after the update; no polling.

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
        Some(Seen {
            built_ins: s.built_ins().into_iter().map(|(_, name)| name).collect(),
            named: super::art_manifest::sets(st.right_set_count).is_some(),
            kept: s
                .slots
                .iter()
                .filter(|i| i.used)
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
        assert_eq!(seen.built_ins.len(), 2);
    }
}
