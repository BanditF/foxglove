//! A VIA keyboard definition (the JSON VIA and Keychron publish): which keyboard it is, its
//! switch matrix, the physical layout and whether it has RGB Matrix lighting.
//!
//! Only what Palm needs is read. The layout is KLE (keyboard-layout-editor) rows whose key
//! labels start with `row,col`; this is a small parser of our own.

use palm::{Key, Layout};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub rows: u16,
    pub cols: u16,
    pub layout: Layout,
    /// The definition offers QMK RGB Matrix controls.
    pub rgb_matrix: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    name: String,
    vendor_id: String,
    product_id: String,
    matrix: Matrix,
    layouts: Layouts,
    #[serde(default)]
    menus: Vec<Value>,
    #[serde(default)]
    lighting: Option<Value>,
}

#[derive(Deserialize)]
struct Matrix {
    rows: u16,
    cols: u16,
}

#[derive(Deserialize)]
struct Layouts {
    keymap: Vec<Value>,
}

fn hex(s: &str) -> Result<u16, String> {
    u16::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16).map_err(|_| format!("bad id {s:?}"))
}

impl Definition {
    pub fn parse(json: &str) -> Result<Definition, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let keys = kle_keys(&raw.layouts.keymap)?;
        for k in &keys {
            if k.row >= raw.matrix.rows || k.col >= raw.matrix.cols {
                return Err(format!("key {},{} is outside the {}x{} matrix", k.row, k.col, raw.matrix.rows, raw.matrix.cols));
            }
        }
        let mentions_rgb_matrix = |v: &Value| v.to_string().contains("qmk_rgb_matrix");
        Ok(Definition {
            rgb_matrix: raw.menus.iter().any(mentions_rgb_matrix) || raw.lighting.as_ref().is_some_and(mentions_rgb_matrix),
            name: raw.name.clone(),
            vendor_id: hex(&raw.vendor_id)?,
            product_id: hex(&raw.product_id)?,
            rows: raw.matrix.rows,
            cols: raw.matrix.cols,
            layout: Layout { name: raw.name, keys },
        })
    }
}

/// Keys from KLE rows: strings are keys (label `row,col` first), objects set the next key's
/// offset (`x`, `y`) and size (`w`, `h`). Keys whose label isn't `row,col` (decals, layout
/// options) are skipped.
fn kle_keys(rows: &[Value]) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    let mut y = 0.0f32;
    for row in rows {
        let Value::Array(items) = row else {
            continue; // KLE metadata object
        };
        let mut x = 0.0f32;
        let (mut w, mut h) = (1.0f32, 1.0f32);
        let mut decal = false;
        for item in items {
            match item {
                Value::Object(o) => {
                    let num = |k: &str| o.get(k).and_then(Value::as_f64).map(|v| v as f32);
                    x += num("x").unwrap_or(0.0);
                    y += num("y").unwrap_or(0.0);
                    w = num("w").unwrap_or(w);
                    h = num("h").unwrap_or(h);
                    decal = o.get("d").and_then(Value::as_bool).unwrap_or(false);
                }
                Value::String(label) => {
                    let first = label.split('\n').next().unwrap_or("");
                    let pos = first.split_once(',').and_then(|(r, c)| Some((r.trim().parse().ok()?, c.trim().parse().ok()?)));
                    if let (Some((row, col)), false) = (pos, decal) {
                        keys.push(Key { row, col, x, y, w, h, r: 0.0 });
                    }
                    x += w;
                    (w, h, decal) = (1.0, 1.0, false);
                }
                _ => return Err("unexpected KLE item".into()),
            }
        }
        y += 1.0;
    }
    Ok(keys)
}

#[cfg(test)]
pub(crate) const SAMPLE: &str = r#"{
  "name": "Sample K5-like",
  "vendorId": "0x3434",
  "productId": "0x0250",
  "matrix": {"rows": 2, "cols": 4},
  "menus": ["qmk_rgb_matrix"],
  "layouts": {"keymap": [
    {"name": "metadata"},
    ["0,0", {"w": 1.5}, "0,1", "0,2", {"x": 0.25}, "0,3"],
    [{"y": 0.5, "w": 2.25}, "1,0", {"d": true}, "decal", "1,1\n\n\n0,0", "1,2"]
  ]}
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ids_matrix_layout_and_lighting() {
        let d = Definition::parse(SAMPLE).unwrap();
        assert_eq!((d.vendor_id, d.product_id, d.rows, d.cols), (0x3434, 0x0250, 2, 4));
        assert!(d.rgb_matrix);
        let k = |r, c| d.layout.keys.iter().find(|k| k.row == r && k.col == c).cloned().unwrap();
        assert_eq!((k(0, 1).x, k(0, 1).w), (1.0, 1.5));
        assert_eq!(k(0, 3).x, 3.75);
        assert_eq!((k(1, 0).y, k(1, 0).w), (1.5, 2.25));
        assert_eq!(k(1, 1).x, 3.25, "the decal took space but isn't a key");
        assert_eq!(d.layout.keys.len(), 7);
    }

    #[test]
    fn rejects_keys_outside_the_matrix() {
        let bad = SAMPLE.replace("\"1,2\"", "\"5,2\"");
        assert!(Definition::parse(&bad).unwrap_err().contains("outside"));
    }
}
