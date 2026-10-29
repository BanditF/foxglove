//! Names for the right half's built-in art sets, from the manifest `flipper_art.py` writes next
//! to the firmware's `custom_art.c` (format `foxglove-builtin-art` v1, #95).
//!
//! The firmware only reports how many sets it has, so the names apply only when that count
//! matches; otherwise the sets stay "Set n". A manifest at `<art folder>/builtins.json` (same
//! format) wins over the bundled one, for custom firmware. A set's `name` is its stable id.

use std::sync::OnceLock;

use serde::Deserialize;

const BUNDLED: &str =
    include_str!("../../../boards/shields/nice_view_custom/widgets/custom_art.json");

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Set {
    pub index: u8,
    /// The C name: the set's stable id (#60's hotlist stores it).
    pub name: String,
    pub label: String,
    pub frames: u32,
    pub fps: u32,
}

#[derive(Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    set_count: u8,
    sets: Vec<Set>,
}

fn parse(text: &str) -> Option<Vec<Set>> {
    let m: Manifest = serde_json::from_str(text).ok()?;
    let ordered = m
        .sets
        .iter()
        .enumerate()
        .all(|(i, s)| usize::from(s.index) == i);
    (m.format == "foxglove-builtin-art"
        && m.version == 1
        && usize::from(m.set_count) == m.sets.len()
        && ordered)
        .then_some(m.sets)
}

/// The manifests Foxglove knows: the art folder's `builtins.json` first, then the bundled one.
fn manifests() -> impl Iterator<Item = &'static Vec<Set>> {
    static OVERRIDE: OnceLock<Option<Vec<Set>>> = OnceLock::new();
    static BUNDLED_SETS: OnceLock<Option<Vec<Set>>> = OnceLock::new();
    let custom = OVERRIDE.get_or_init(|| {
        let path = super::art_library::folder()?.join("builtins.json");
        parse(&std::fs::read_to_string(path).ok()?)
    });
    let bundled = BUNDLED_SETS.get_or_init(|| parse(BUNDLED));
    [custom, bundled].into_iter().flatten()
}

/// The sets for firmware with `set_count` built-ins, if a manifest matches it.
pub fn sets(set_count: u8) -> Option<&'static [Set]> {
    manifests()
        .find(|sets| sets.len() == usize::from(set_count))
        .map(Vec::as_slice)
}

/// Whether a known manifest has a built-in named `name`. Since #671 the firmware keeps one
/// built-in (`Bandit_Emblem`), so data that names another one (a wallpaper's `builtin`) goes on
/// to its file instead of a set the keyboard doesn't have.
pub fn known(name: &str) -> bool {
    manifests().any(|sets| sets.iter().any(|s| s.name == name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_built_in_is_known() {
        assert!(known("Bandit_Emblem"), "the fallback built-in since #671");
        assert!(!known("ZMK_1"), "ZMK's stock art left the firmware (#671)");
        assert!(!known("Bandit_Idle"));
    }

    #[test]
    fn bundled_manifest_parses_in_order() {
        let bundled = parse(BUNDLED).expect("bundled custom_art.json is valid");
        assert!(!bundled.is_empty());
        assert_eq!(
            sets(bundled.len() as u8).map(<[Set]>::len),
            Some(bundled.len())
        );
        assert_eq!(
            sets(bundled.len() as u8 + 1),
            None,
            "a different firmware keeps Set n"
        );
    }

    #[test]
    fn rejects_wrong_format_count_or_order() {
        let set =
            |i: u8| format!(r#"{{"index":{i},"name":"S{i}","label":"L{i}","frames":1,"fps":0}}"#);
        let doc = |format: &str, count: u8, sets: &[u8]| {
            let sets: Vec<_> = sets.iter().map(|&i| set(i)).collect();
            format!(
                r#"{{"format":"{format}","version":1,"set_count":{count},"sets":[{}]}}"#,
                sets.join(",")
            )
        };
        assert!(parse(&doc("foxglove-builtin-art", 2, &[0, 1])).is_some());
        assert!(parse(&doc("other", 2, &[0, 1])).is_none());
        assert!(parse(&doc("foxglove-builtin-art", 3, &[0, 1])).is_none());
        assert!(parse(&doc("foxglove-builtin-art", 2, &[1, 0])).is_none());
    }
}
