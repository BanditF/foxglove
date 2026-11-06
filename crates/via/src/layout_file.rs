//! A layout file the user loads for a keyboard whose own layout isn't known (#798): a VIA
//! definition, a QMK keyboard.json / info.json or a raw KLE layout, told apart by what's in it.
//! Only the drawing is taken from it (where each key is); its USB ids, matrix and lighting are
//! left alone, so a file for a sibling board (a QMK Corne's for a ZMK Corne) draws as well.
//!
//! The file comes from the user, so reading it is bounded: at most [`MAX_BYTES`] (the cap on a
//! Vial definition, 0.2's limit for definitions), UTF-8 JSON, at most [`MAX_KEYS`] keys, each a
//! sane size in a sane place. Every refusal is one short line that says why.

use std::io::Read;
use std::path::Path;

use foxglove_devices::Layout;
use serde_json::Value;

use crate::definition::Definition;

/// The largest file read: the cap on a Vial definition's JSON.
pub const MAX_BYTES: usize = crate::vial::MAX_JSON;
/// The most keys a loaded layout may have (the pages draw at most this many).
pub const MAX_KEYS: usize = 256;
/// The largest key, and the farthest a key may sit from the origin, in key units.
const MAX_KEY_SIZE: f32 = 16.0;
const MAX_REACH: f32 = 200.0;

/// Which kind of file it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Via,
    Qmk,
    Kle,
}

/// A loaded layout: the keys (in key units, the default layout's) and its name.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutFile {
    pub format: Format,
    pub layout: Layout,
}

/// Reads and parses the layout file at `path`. Only `.json` files are read; the size is checked
/// before reading and while reading.
pub fn read(path: &Path) -> Result<LayoutFile, String> {
    let json = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if !json {
        return Err("only .json files (VIA, QMK or KLE) can be loaded".into());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("it can't be opened ({e})"))?;
    let too_big = || format!("it's over {} KiB, too big for a layout", MAX_BYTES / 1024);
    if file.metadata().is_ok_and(|m| m.len() > MAX_BYTES as u64) {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("it can't be read ({e})"))?;
    if bytes.len() > MAX_BYTES {
        return Err(too_big());
    }
    let text = String::from_utf8(bytes).map_err(|_| "it isn't text".to_string())?;
    parse(path, &text)
}

/// Parses a layout file's text; `path` names it (a KLE layout without a name takes the file's)
/// and, for a QMK file without its layouts, says where the folders above it are.
pub fn parse(path: &Path, text: &str) -> Result<LayoutFile, String> {
    if text.len() > MAX_BYTES {
        return Err(format!(
            "it's over {} KiB, too big for a layout",
            MAX_BYTES / 1024
        ));
    }
    let text = text.trim_start_matches('\u{feff}');
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("it isn't valid JSON ({e})"))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (format, layout) = if value.is_array() {
        (Format::Kle, crate::kle::parse(&value, &stem)?)
    } else if value.pointer("/layouts/keymap").is_some() {
        // VIA's own reader for its KLE rows; the ids are the keyboard's, not the file's.
        let d = Definition::from_board(text, 0, 0, &stem)?;
        (Format::Via, d.layout)
    } else if crate::qmk::looks_like(&value) {
        let d = qmk(path, &value, text)?;
        let name = d.qmk_layout.clone().unwrap_or(d.name);
        (
            Format::Qmk,
            Layout {
                name,
                keys: d.layout.keys,
            },
        )
    } else {
        return Err(
            "it isn't a VIA definition, a QMK keyboard.json / info.json or a KLE layout".into(),
        );
    };
    check(&layout)?;
    Ok(LayoutFile { format, layout })
}

/// A QMK file through #806's reader. A file with its layouts but without USB ids gets stand-in
/// ids, since only the drawing is used; one without its layouts is completed from the folders
/// above it, as QMK's build does.
fn qmk(path: &Path, value: &Value, text: &str) -> Result<Definition, String> {
    let has_layouts = value["layouts"].as_object().is_some_and(|l| !l.is_empty());
    let has_ids = value.pointer("/usb/vid").is_some() && value.pointer("/usb/pid").is_some();
    if has_layouts && !has_ids {
        let mut v = value.clone();
        let usb = v.as_object_mut().map(|o| {
            o.entry("usb")
                .or_insert_with(|| Value::Object(Default::default()))
        });
        if let Some(Value::Object(usb)) = usb {
            usb.entry("vid").or_insert_with(|| "0x0000".into());
            usb.entry("pid").or_insert_with(|| "0x0000".into());
        }
        return crate::qmk::load(path, &v.to_string());
    }
    crate::qmk::load(path, text)
}

/// Keys a page can draw: some, not too many, each a sane size in a sane place.
fn check(layout: &Layout) -> Result<(), String> {
    if layout.keys.is_empty() {
        return Err("it has no keys".into());
    }
    if layout.keys.len() > MAX_KEYS {
        return Err(format!(
            "it has {} keys, over the {MAX_KEYS} a layout may have",
            layout.keys.len()
        ));
    }
    let sane = |k: &foxglove_devices::Key| {
        [k.x, k.y, k.w, k.h, k.r].iter().all(|v| v.is_finite())
            && k.w > 0.0
            && k.h > 0.0
            && k.w <= MAX_KEY_SIZE
            && k.h <= MAX_KEY_SIZE
            && k.x.abs() <= MAX_REACH
            && k.y.abs() <= MAX_REACH
    };
    if !layout.keys.iter().all(sane) {
        return Err("a key in it has an impossible size or place".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("foxglove-layout-file-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(name: &str, text: &str) -> std::path::PathBuf {
        let p = dir().join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn via_qmk_and_kle_files_load_and_say_which_they_are() {
        let via = read(&write("sample.json", crate::definition::SAMPLE)).unwrap();
        assert_eq!(via.format, Format::Via);
        assert_eq!(via.layout.name, "Sample K5-like");
        assert!(!via.layout.keys.is_empty());

        // A loose QMK info.json with its layouts and no USB ids: drawn all the same.
        let qmk = r#"{"keyboard_name": "Split 3x2", "layouts": {"LAYOUT_split_3x2": {"layout": [
            {"matrix": [0, 0], "x": 0, "y": 0}, {"matrix": [0, 1], "x": 1, "y": 0},
            {"matrix": [1, 0], "x": 4, "y": 0}, {"matrix": [1, 1], "x": 5, "y": 0, "w": 1.5}]}}}"#;
        let qmk = read(&write("info.json", qmk)).unwrap();
        assert_eq!(qmk.format, Format::Qmk);
        assert_eq!(qmk.layout.name, "LAYOUT_split_3x2");
        assert_eq!(qmk.layout.keys.len(), 4);

        let kle = read(&write("my-board.JSON", r#"[["Esc", "Q"], ["A", "S"]]"#)).unwrap();
        assert_eq!(kle.format, Format::Kle);
        assert_eq!(kle.layout.name, "my-board");
        assert_eq!(kle.layout.keys.len(), 4);
    }

    #[test]
    fn a_bad_file_says_why_in_one_line() {
        let cases = [
            ("notes.txt", "[[\"A\"]]", "only .json files"),
            ("broken.json", "{ nope", "isn't valid JSON"),
            ("other.json", r#"{"hello": 1}"#, "isn't a VIA definition"),
            ("empty.json", "[]", "no keys"),
            ("decals.json", r#"[[{"d": true}, "logo"]]"#, "no keys"),
            ("huge-key.json", r#"[[{"w": 1e9}, "A"]]"#, "impossible size"),
            (
                "far.json",
                r#"[[{"x": 5000}, "A"]]"#,
                "impossible size or place",
            ),
        ];
        for (name, text, why) in cases {
            let e = read(&write(name, text)).unwrap_err();
            assert!(e.contains(why), "{name}: {e}");
            assert!(!e.contains('\n'), "{name}: one line: {e}");
        }
        let many = format!("[[{}]]", vec!["\"k\""; MAX_KEYS + 1].join(","));
        let e = read(&write("many.json", &many)).unwrap_err();
        assert!(e.contains("257 keys, over the 256"), "{e}");
    }

    #[test]
    fn the_size_is_bounded_before_parsing() {
        let big = format!("[[\"A\"]]{}", " ".repeat(MAX_BYTES));
        let e = read(&write("big.json", &big)).unwrap_err();
        assert_eq!(e, "it's over 1024 KiB, too big for a layout");
        assert!(parse(Path::new("big.json"), &big).is_err());
        // Just under the cap still loads.
        let fits = format!("[[\"A\"]]{}", " ".repeat(MAX_BYTES - 8));
        assert!(read(&write("fits.json", &fits)).is_ok());
    }
}
