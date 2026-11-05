//! A QMK keyboard.json / info.json as a definition (#74): for boards that answer VIA but have no
//! VIA definition anyone publishes. QMK's data-driven files give what the keymap needs: the USB
//! ids (`usb.vid`, `usb.pid`), the matrix (`matrix_size`, else `matrix_pins`) and the physical
//! layouts (`layouts.LAYOUT_*.layout`: each key's `x`, `y`, optional `w`/`h` and
//! `matrix: [row, col]`). The layers come from the board's own keymap read, as with a VIA file.
//!
//! QMK splits a keyboard's data over the folders of its tree: a variant's info.json may hold
//! only its product id and features, with the vendor id, matrix and layouts in the info.json
//! of a folder above it (`keyboards/<maker>/<board>/info.json`). QMK's build merges them from
//! the top down, the deeper file winning; a file without its layouts or ids is completed the
//! same way from the info.json / keyboard.json files of the folders above it, a few levels up
//! at most and never past `keyboards`.
//!
//! The layout drawn: the one `layout_aliases.LAYOUT` names, else one named `LAYOUT`, else the
//! first whose name has a word matching a folder below the merged top (the `ansi` in
//! `k5_pro/ansi/white` picks `LAYOUT_108_ansi`), else the first in the file.
//!
//! Written from QMK's documented info.json schema; no QMK or maker files are bundled.

use std::path::{Path, PathBuf};

use foxglove_devices::Key;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

use crate::client::Channel;
use crate::definition::{Definition, Placed};

/// The file names QMK keeps a keyboard's data in.
const FILES: [&str; 2] = ["keyboard.json", "info.json"];
/// How many folders up a file is completed from.
const UP: usize = 4;

/// Whether a JSON value looks like a QMK keyboard.json / info.json (or part of one).
pub fn looks_like(v: &Value) -> bool {
    let Some(o) = v.as_object() else {
        return false;
    };
    let qmk_layouts = o
        .get("layouts")
        .and_then(Value::as_object)
        .is_some_and(|l| l.values().any(|l| l.get("layout").is_some()));
    qmk_layouts
        || ["keyboard_name", "usb", "matrix_size", "matrix_pins"]
            .iter()
            .any(|k| o.contains_key(*k))
}

/// The definition from a QMK file at `path` (its text is `text`), completed from the files of
/// the folders above it when it lacks its ids or layouts.
pub fn load(path: &Path, text: &str) -> Result<Definition, String> {
    let own = Part::parse(text)?;
    let mut parts = vec![own];
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut top = dir.clone();
    if !complete(&parts[0].value) {
        for up in dir.ancestors().skip(1).take(UP) {
            if up
                .file_name()
                .is_none_or(|n| n.eq_ignore_ascii_case("keyboards"))
            {
                break;
            }
            for f in FILES {
                let p = up.join(f);
                if let Ok(t) = std::fs::read_to_string(&p) {
                    parts.push(Part::parse(&t).map_err(|e| format!("{}: {e}", p.display()))?);
                    top = up.to_path_buf();
                }
            }
        }
    }
    parts.reverse(); // the top first, as QMK merges
    from_parts(&parts, &hints(&dir, &top))
}

/// The folder names below the merged top, deepest first: `white`, `ansi`.
fn hints(dir: &Path, top: &Path) -> Vec<String> {
    let below = dir.strip_prefix(top).map(PathBuf::from).unwrap_or_default();
    let mut v: Vec<String> = below
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    v.reverse();
    v
}

/// Whether a file has what a definition needs without the folders above it.
fn complete(v: &Value) -> bool {
    let has = |p: &str| v.pointer(p).is_some();
    has("/usb/vid") && has("/usb/pid") && v["layouts"].as_object().is_some_and(|l| !l.is_empty())
}

/// One file: its JSON, and its layouts' names in file order (a JSON object's order is lost).
pub(crate) struct Part {
    value: Value,
    order: Vec<String>,
}

impl Part {
    pub(crate) fn parse(text: &str) -> Result<Part, String> {
        let text = text.trim_start_matches('\u{feff}');
        #[derive(Deserialize)]
        struct Top {
            #[serde(default)]
            layouts: Names,
        }
        let value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let order = serde_json::from_str::<Top>(text)
            .map(|t| t.layouts.0)
            .unwrap_or_default();
        Ok(Part { value, order })
    }
}

/// An object's keys in file order.
#[derive(Default)]
struct Names(Vec<String>);

impl<'de> Deserialize<'de> for Names {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Names, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Names;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Names, A::Error> {
                let mut names = Vec::new();
                while let Some((k, IgnoredAny)) = map.next_entry::<String, IgnoredAny>()? {
                    names.push(k);
                }
                Ok(Names(names))
            }
        }
        d.deserialize_map(V)
    }
}

/// QMK's merge: objects key by key, anything else the later file's.
fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

fn hex(v: &Value, what: &str) -> Result<u16, String> {
    let s = v
        .as_str()
        .ok_or(format!("no {what} (QMK keeps it in an info.json of a folder above, so load the file from a copy of the keyboard's folders)"))?;
    u16::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
        .map_err(|_| format!("bad {what} {s:?}"))
}

/// The definition from a keyboard's files, the top first; `hints` are the folder names that
/// pick among its layouts.
pub(crate) fn from_parts(parts: &[Part], hints: &[String]) -> Result<Definition, String> {
    let mut v = Value::Object(Default::default());
    let mut order: Vec<String> = Vec::new();
    for p in parts {
        merge(&mut v, &p.value);
        for n in &p.order {
            if !order.contains(n) {
                order.push(n.clone());
            }
        }
    }
    let ids = (
        hex(&v["usb"]["vid"], "usb.vid")?,
        hex(&v["usb"]["pid"], "usb.pid")?,
    );
    let layouts = v["layouts"]
        .as_object()
        .filter(|l| !l.is_empty())
        .ok_or("no layouts (QMK keeps them in an info.json of a folder above, so load the file from a copy of the keyboard's folders)")?;
    for n in layouts.keys() {
        if !order.contains(n) {
            order.push(n.clone());
        }
    }
    let alias = v["layout_aliases"]["LAYOUT"].as_str().map(str::to_string);
    let chosen = alias
        .filter(|a| layouts.contains_key(a))
        .or_else(|| layouts.contains_key("LAYOUT").then(|| "LAYOUT".to_string()))
        .or_else(|| {
            hints.iter().find_map(|h| {
                order
                    .iter()
                    .find(|n| n.to_lowercase().split('_').any(|w| w == h))
                    .cloned()
            })
        })
        .or_else(|| order.first().cloned())
        .ok_or("no layouts")?;
    let all: Vec<(&String, Vec<Key>)> = order
        .iter()
        .filter_map(|n| layouts.get(n).map(|l| keys(n, l).map(|k| (n, k))))
        .collect::<Result<_, _>>()?;
    let keys = all
        .iter()
        .find(|(n, _)| **n == chosen)
        .map(|(_, k)| k.clone())
        .unwrap_or_default();
    if keys.is_empty() {
        return Err(format!("{chosen} has no keys"));
    }
    let matrix = matrix(&v).unwrap_or_else(|| {
        // Neither size nor pins: just big enough for every layout's keys.
        all.iter()
            .flat_map(|(_, k)| k)
            .fold((0, 0), |(r, c), k| (r.max(k.row + 1), c.max(k.col + 1)))
    });
    let name = name(&v, ids);
    let lighting = [
        ("rgb_matrix", Channel::RgbMatrix),
        ("rgblight", Channel::RgbLight),
        ("led_matrix", Channel::LedMatrix),
        ("backlight", Channel::Backlight),
    ]
    .into_iter()
    .filter(|(f, _)| v["features"][f].as_bool() == Some(true))
    .map(|(_, c)| c)
    .collect();
    let placed = keys
        .into_iter()
        .map(|key| Placed { key, option: None })
        .collect();
    Definition::assemble(name, ids, matrix, placed, vec![], lighting, Some(chosen))
}

/// One layout's keys.
fn keys(name: &str, layout: &Value) -> Result<Vec<Key>, String> {
    let entries = layout["layout"]
        .as_array()
        .ok_or(format!("{name} has no layout list"))?;
    let mut out = Vec::new();
    for e in entries {
        let Some(m) = e["matrix"].as_array() else {
            continue; // not a matrix key
        };
        let pos = |i: usize| {
            m.get(i)
                .and_then(Value::as_u64)
                .and_then(|n| u16::try_from(n).ok())
        };
        let (Some(row), Some(col)) = (pos(0), pos(1)) else {
            return Err(format!("{name}: bad matrix {e}"));
        };
        let num = |k: &str, d: f32| e[k].as_f64().map(|n| n as f32).unwrap_or(d);
        out.push(Key {
            row,
            col,
            x: num("x", 0.0),
            y: num("y", 0.0),
            w: num("w", 1.0),
            h: num("h", 1.0),
            r: 0.0,
        });
    }
    Ok(out)
}

/// `matrix_size`, else the pins (a split board's rows twice over, one set per half).
fn matrix(v: &Value) -> Option<(u16, u16)> {
    let n = |p: &str| {
        v.pointer(p)
            .and_then(Value::as_u64)
            .and_then(|n| u16::try_from(n).ok())
    };
    if let (Some(r), Some(c)) = (n("/matrix_size/rows"), n("/matrix_size/cols")) {
        return Some((r, c));
    }
    let len = |p: &str| {
        v.pointer(p)
            .and_then(Value::as_array)
            .map(|a| a.len() as u16)
    };
    let (r, c) = match (len("/matrix_pins/rows"), len("/matrix_pins/cols")) {
        (Some(r), Some(c)) => (r, c),
        _ => (len("/matrix_pins/direct")?, len("/matrix_pins/direct/0")?),
    };
    let halves = if v["split"]["enabled"].as_bool() == Some(true) {
        2
    } else {
        1
    };
    Some((r * halves, c))
}

/// `keyboard_name`, after the manufacturer unless it starts with it.
fn name(v: &Value, (vid, pid): (u16, u16)) -> String {
    let maker = v["manufacturer"].as_str().unwrap_or("").trim();
    match v["keyboard_name"].as_str().map(str::trim) {
        Some(n) if maker.is_empty() || n.starts_with(maker) => n.to_string(),
        Some(n) => format!("{maker} {n}"),
        None => format!("QMK keyboard {vid:04x}:{pid:04x}"),
    }
}

/// A made-up two-row board split the way QMK trees are: the board's info.json (vendor id,
/// matrix, two layouts) above a variant's info.json (product id, features).
#[cfg(test)]
pub(crate) const BOARD: &str = r#"{
  "keyboard_name": "Fox 7",
  "manufacturer": "Synthetic",
  "usb": {"vid": "0xFEED"},
  "matrix_size": {"rows": 2, "cols": 4},
  "layouts": {
    "LAYOUT_7_ansi": {"layout": [
      {"matrix": [0, 0], "x": 0, "y": 0},
      {"matrix": [0, 1], "x": 1, "y": 0, "w": 1.5},
      {"matrix": [0, 3], "x": 2.5, "y": 0, "w": 1.5},
      {"matrix": [1, 0], "x": 0, "y": 1, "w": 2.25},
      {"matrix": [1, 2], "x": 2.25, "y": 1, "w": 1.75},
      {"matrix": [1, 3], "x": 4.25, "y": 0, "h": 2}
    ]},
    "LAYOUT_8_iso": {"layout": [
      {"matrix": [0, 0], "x": 0, "y": 0},
      {"matrix": [0, 1], "x": 1, "y": 0},
      {"matrix": [0, 2], "x": 2, "y": 0},
      {"matrix": [0, 3], "x": 3, "y": 0},
      {"matrix": [1, 0], "x": 0, "y": 1},
      {"matrix": [1, 1], "x": 1, "y": 1},
      {"matrix": [1, 2], "x": 2, "y": 1},
      {"matrix": [1, 3], "x": 3, "y": 1}
    ]}
  }
}"#;

#[cfg(test)]
pub(crate) const VARIANT: &str = r#"{"usb": {"pid": "0x0074"}, "features": {"led_matrix": true}}"#;

#[cfg(test)]
pub(crate) fn board(hints: &[&str]) -> Definition {
    let parts = [Part::parse(BOARD).unwrap(), Part::parse(VARIANT).unwrap()];
    let hints: Vec<String> = hints.iter().map(|h| h.to_string()).collect();
    from_parts(&parts, &hints).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(d: &Definition, r: u16, c: u16) -> (f32, f32, f32, f32) {
        let k = d
            .layout
            .keys
            .iter()
            .find(|k| (k.row, k.col) == (r, c))
            .unwrap();
        (k.x, k.y, k.w, k.h)
    }

    #[test]
    fn a_variant_merged_with_its_board_gives_ids_matrix_and_the_layout() {
        let d = board(&["white", "ansi"]);
        assert_eq!(
            (d.vendor_id, d.product_id, d.rows, d.cols),
            (0xFEED, 0x0074, 2, 4)
        );
        assert_eq!(d.name, "Synthetic Fox 7");
        assert_eq!(d.qmk_layout.as_deref(), Some("LAYOUT_7_ansi"));
        assert_eq!(d.layout.keys.len(), 6);
        assert_eq!(at(&d, 0, 1), (1.0, 0.0, 1.5, 1.0));
        assert_eq!(at(&d, 1, 0), (0.0, 1.0, 2.25, 1.0));
        assert_eq!(at(&d, 1, 3), (4.25, 0.0, 1.0, 2.0), "a tall key");
        assert_eq!(d.lighting, vec![Channel::LedMatrix]);
        assert!(
            d.options.is_empty(),
            "QMK layouts have no VIA layout options"
        );
        assert_eq!(d.layout_for(0b1), d.layout);
    }

    /// The folder picks the layout: `iso` the ISO one; no matching folder, the first in the
    /// file (not the alphabetical first); an alias named `LAYOUT` wins over both.
    #[test]
    fn the_layout_is_the_alias_the_variant_folder_or_the_first() {
        assert_eq!(
            board(&["white", "iso"]).qmk_layout.as_deref(),
            Some("LAYOUT_8_iso")
        );
        assert_eq!(board(&["iso"]).layout.keys.len(), 8);
        let reordered = BOARD.replace("LAYOUT_7_ansi", "LAYOUT_9_zzz");
        let parts = [
            Part::parse(&reordered).unwrap(),
            Part::parse(VARIANT).unwrap(),
        ];
        let d = from_parts(&parts, &[]).unwrap();
        assert_eq!(d.qmk_layout.as_deref(), Some("LAYOUT_9_zzz"), "file order");
        let aliased = BOARD.replacen(
            "\"layouts\"",
            "\"layout_aliases\": {\"LAYOUT\": \"LAYOUT_8_iso\"}, \"layouts\"",
            1,
        );
        let parts = [
            Part::parse(&aliased).unwrap(),
            Part::parse(VARIANT).unwrap(),
        ];
        let d = from_parts(&parts, &["ansi".into()]).unwrap();
        assert_eq!(d.qmk_layout.as_deref(), Some("LAYOUT_8_iso"));
    }

    #[test]
    fn a_part_alone_says_what_it_lacks() {
        let e = from_parts(&[Part::parse(VARIANT).unwrap()], &[]).unwrap_err();
        assert!(e.contains("usb.vid") && e.contains("folders"), "{e}");
        let bad = BOARD.replace("[1, 3]", "[1, 9]");
        let parts = [Part::parse(&bad).unwrap(), Part::parse(VARIANT).unwrap()];
        assert!(from_parts(&parts, &[]).unwrap_err().contains("outside"));
    }

    #[test]
    fn the_matrix_comes_from_the_size_the_pins_or_the_keys() {
        let pins = BOARD.replace(
            "\"matrix_size\": {\"rows\": 2, \"cols\": 4}",
            "\"matrix_pins\": {\"rows\": [\"A0\"], \"cols\": [\"B0\", \"B1\", \"B2\", \"B3\", \"B4\"]}, \"split\": {\"enabled\": true}",
        );
        let parts = [Part::parse(&pins).unwrap(), Part::parse(VARIANT).unwrap()];
        let d = from_parts(&parts, &[]).unwrap();
        assert_eq!((d.rows, d.cols), (2, 5), "one row per half, two halves");
        let none = BOARD.replace("\"matrix_size\": {\"rows\": 2, \"cols\": 4},", "");
        let parts = [Part::parse(&none).unwrap(), Part::parse(VARIANT).unwrap()];
        let d = from_parts(&parts, &[]).unwrap();
        assert_eq!((d.rows, d.cols), (2, 4));
    }

    #[test]
    fn sniffs_qmk_files_but_not_via_ones() {
        assert!(looks_like(&serde_json::from_str(BOARD).unwrap()));
        assert!(looks_like(&serde_json::from_str(VARIANT).unwrap()));
        let via: Value = serde_json::from_str(crate::definition::SAMPLE).unwrap();
        assert!(!looks_like(&via));
    }

    /// From disk: the variant's file is completed from the board's folder above, as QMK's
    /// build does; a VIA file still loads as VIA; anything else is refused.
    #[test]
    fn loads_a_variant_from_its_folders() {
        let root = std::env::temp_dir().join(format!("foxglove-qmk-{}", std::process::id()));
        let variant = root.join("keyboards/synthetic/fox7/ansi/white");
        std::fs::create_dir_all(&variant).unwrap();
        std::fs::write(root.join("keyboards/synthetic/fox7/info.json"), BOARD).unwrap();
        // Saved with a byte order mark, as Windows editors may.
        std::fs::write(variant.join("info.json"), format!("\u{feff}{VARIANT}")).unwrap();
        std::fs::write(root.join("keyboards/info.json"), "not json").unwrap(); // never read
        let d = Definition::load(&variant.join("info.json")).unwrap();
        assert_eq!((d.vendor_id, d.product_id), (0xFEED, 0x0074));
        assert_eq!(d.qmk_layout.as_deref(), Some("LAYOUT_7_ansi"));
        let via = root.join("via.json");
        std::fs::write(&via, crate::definition::SAMPLE).unwrap();
        let v = Definition::load(&via).unwrap();
        assert_eq!((v.product_id, v.qmk_layout), (0x0250, None));
        let other = root.join("other.json");
        std::fs::write(&other, r#"{"hello": 1}"#).unwrap();
        assert!(Definition::load(&other).unwrap_err().contains("neither"));
        // A complete keyboard.json loads alone.
        let lone = root.join("lone");
        std::fs::create_dir_all(&lone).unwrap();
        let full = BOARD.replace(
            "\"vid\": \"0xFEED\"",
            "\"vid\": \"0xFEED\", \"pid\": \"0x0001\"",
        );
        std::fs::write(lone.join("keyboard.json"), full).unwrap();
        let d = Definition::load(&lone.join("keyboard.json")).unwrap();
        assert_eq!(
            (d.product_id, d.qmk_layout.as_deref()),
            (0x0001, Some("LAYOUT_7_ansi"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
