//! A raw KLE (keyboard-layout-editor.com) layout: the JSON its "Download JSON" saves (#798).
//!
//! Unlike a VIA definition's KLE rows ([`crate::definition`]), a raw KLE file's labels are the
//! keycaps' legends, not `row,col`, so every key that isn't a decal counts, in file order. The
//! rows follow kle-serial's rules (KLE's own reader): an object before a key sets the next key's
//! offset (`x`, `y`), size (`w`, `h`), decal (`d`) and turn (`r` degrees around `rx`, `ry`); a
//! new `rx` or `ry` starts a cluster there; a row ends one unit down, back at `rx`. The first
//! entry may be an object of metadata (`name`, `author`, colours). Stepped and ISO keys' second
//! rectangles (`x2`, `y2`, `w2`, `h2`) aren't drawn.
//!
//! Written from kle-serial's documented format; no KLE code or files are bundled.

use foxglove_devices::{Key, Layout};
use serde_json::Value;

/// The most keys read from one file. A wall of keys is a bad file, not a board.
pub const MAX_KEYS: usize = 512;

/// The layout in a KLE file's JSON (`value`, its top-level array); `fallback` names it when its
/// metadata doesn't. Each key's `x`, `y` is where its top-left lands when it's turned about its
/// own centre, as the rest of the device layer places turned keys.
pub fn parse(value: &Value, fallback: &str) -> Result<Layout, String> {
    let rows = value.as_array().ok_or("a KLE layout is a list of rows")?;
    let mut name = fallback.to_string();
    let mut keys = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut w, mut h) = (1.0f32, 1.0f32);
    let (mut r, mut rx, mut ry) = (0.0f32, 0.0f32, 0.0f32);
    let mut decal = false;
    for (i, row) in rows.iter().enumerate() {
        let items = match row {
            Value::Array(items) => items,
            Value::Object(meta) if i == 0 => {
                if let Some(n) = meta.get("name").and_then(Value::as_str) {
                    if !n.trim().is_empty() {
                        name = n.trim().to_string();
                    }
                }
                continue;
            }
            _ => return Err(format!("row {} isn't a list of keys", i + 1)),
        };
        for item in items {
            match item {
                Value::Object(o) => {
                    let num = |k: &str| o.get(k).and_then(Value::as_f64).map(|v| v as f32);
                    if let Some(v) = num("r") {
                        r = v;
                    }
                    if let Some(v) = num("rx") {
                        rx = v;
                        (x, y) = (rx, ry);
                    }
                    if let Some(v) = num("ry") {
                        ry = v;
                        (x, y) = (rx, ry);
                    }
                    x += num("x").unwrap_or(0.0);
                    y += num("y").unwrap_or(0.0);
                    w = num("w").unwrap_or(w);
                    h = num("h").unwrap_or(h);
                    if let Some(d) = o.get("d").and_then(Value::as_bool) {
                        decal = d;
                    }
                }
                Value::String(_) => {
                    if !decal {
                        if keys.len() == MAX_KEYS {
                            return Err(format!("it has over {MAX_KEYS} keys"));
                        }
                        let (kx, ky) = turned_top_left(x, y, w, h, r, rx, ry);
                        keys.push(Key {
                            row: 0,
                            col: keys.len() as u16,
                            x: kx,
                            y: ky,
                            w,
                            h,
                            r,
                        });
                    }
                    x += w;
                    (w, h, decal) = (1.0, 1.0, false);
                }
                _ => return Err(format!("row {} has something that isn't a key", i + 1)),
            }
        }
        y += 1.0;
        x = rx;
    }
    Ok(Layout { name, keys })
}

/// A key at `x`, `y` (unturned), turned `r` degrees about `rx`, `ry`: where its unturned
/// rectangle's top-left goes so that its centre is where the turn puts it.
fn turned_top_left(x: f32, y: f32, w: f32, h: f32, r: f32, rx: f32, ry: f32) -> (f32, f32) {
    if r == 0.0 {
        return (x, y);
    }
    let (cx, cy) = (x + w / 2.0 - rx, y + h / 2.0 - ry);
    let (sin, cos) = r.to_radians().sin_cos();
    let (tx, ty) = (cx * cos - cy * sin + rx, cx * sin + cy * cos + ry);
    (tx - w / 2.0, ty - h / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(json: &str) -> Layout {
        parse(&serde_json::from_str(json).unwrap(), "file").unwrap()
    }

    #[test]
    fn rows_offsets_sizes_and_decals() {
        let l = layout(
            r#"[{"name": "Tiny"},
                ["Esc", {"w": 1.5}, "Q", {"x": 0.5}, "W"],
                [{"y": 0.25, "w": 2}, "Shift", {"d": true}, "logo", "Z"]]"#,
        );
        assert_eq!(l.name, "Tiny");
        let at: Vec<(f32, f32, f32)> = l.keys.iter().map(|k| (k.x, k.y, k.w)).collect();
        assert_eq!(
            at,
            vec![
                (0.0, 0.0, 1.0),
                (1.0, 0.0, 1.5),
                (3.0, 0.0, 1.0),
                (0.0, 1.25, 2.0),
                // The decal takes its unit of space but isn't a key.
                (3.0, 1.25, 1.0),
            ]
        );
        assert_eq!(
            l.keys.iter().map(|k| k.col).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn a_turned_cluster_starts_at_its_pivot_and_turns_about_it() {
        let l = layout(r#"[["A"], [{"r": 90, "rx": 4, "ry": 0}, "T", "U"], ["V"]]"#);
        assert_eq!(l.name, "file", "no metadata: the file's own name");
        let k = |i: usize| &l.keys[i];
        assert_eq!((k(0).x, k(0).y, k(0).r), (0.0, 0.0, 0.0));
        // T unturned at (4, 0), centre (4.5, 0.5); turned 90° about (4, 0): centre (3.5, 0.5).
        assert!(
            (k(1).x - 3.0).abs() < 1e-4 && (k(1).y - 0.0).abs() < 1e-4,
            "{:?}",
            k(1)
        );
        assert_eq!(k(1).r, 90.0);
        // U one unit right of T before the turn: one unit down after it.
        assert!(
            (k(2).x - 3.0).abs() < 1e-4 && (k(2).y - 1.0).abs() < 1e-4,
            "{:?}",
            k(2)
        );
        // The next row starts back at rx, a unit down, still turned.
        assert!(
            (k(3).x - 2.0).abs() < 1e-4 && (k(3).y - 0.0).abs() < 1e-4,
            "{:?}",
            k(3)
        );
    }

    #[test]
    fn what_isnt_kle_says_so() {
        let bad = |json: &str| parse(&serde_json::from_str(json).unwrap(), "f").unwrap_err();
        assert!(bad(r#"{"name": "x"}"#).contains("list of rows"));
        assert!(bad(r#"[["A"], 5]"#).contains("row 2"));
        assert!(bad(r#"[["A", 5]]"#).contains("isn't a key"));
        let wall = format!("[[{}]]", vec!["\"k\""; MAX_KEYS + 1].join(","));
        assert!(bad(&wall).contains("over 512 keys"));
    }
}
