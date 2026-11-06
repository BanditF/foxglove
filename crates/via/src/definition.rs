//! A VIA keyboard definition (the JSON VIA and Keychron publish): which keyboard it is, its
//! switch matrix, the physical layout and which QMK lighting it offers.
//!
//! Only what the device layer needs is read. The layout is KLE (keyboard-layout-editor) rows whose key
//! labels start with `row,col`; this is a small parser of our own.
//!
//! Layout options (#74): a key whose fourth label line is `group,choice` belongs to one choice of
//! a layout option (split backspace, ISO enter, bottom row). `layouts.labels` names the groups: a
//! string is an on/off toggle, an array is `[name, choice 0, choice 1, ...]`. The keyboard keeps
//! the chosen ones as one number (VIA's layout options value): each group in the fewest bits that
//! hold its choices, the first group in the highest bits. Choice 0 is the default; another
//! choice's keys are moved so its top-left key sits where choice 0's does, as VIA draws them.

use foxglove_devices::{Key, Layout};

use crate::client::Channel;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub rows: u16,
    pub cols: u16,
    /// The default layout: every key outside a layout option, and choice 0 of each option.
    /// [`Definition::layout_for`] gives the layout for the options a keyboard has chosen.
    pub layout: Layout,
    /// The layout options, in group order.
    pub options: Vec<LayoutOption>,
    /// The QMK lighting controls the definition offers, best first.
    pub lighting: Vec<Channel>,
    /// The QMK layout (`LAYOUT_*`) it was built from, when it came from a QMK keyboard.json /
    /// info.json ([`crate::qmk`]) instead of a VIA definition.
    pub qmk_layout: Option<String>,
    /// Every key, with the layout option choice it belongs to.
    keys: Vec<Placed>,
}

/// One layout option group (an entry of `layouts.labels`).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutOption {
    pub name: String,
    /// The choices' names; empty for an on/off toggle (off is choice 0).
    pub choices: Vec<String>,
}

impl LayoutOption {
    /// How many choices it has: 2 for a toggle.
    pub fn count(&self) -> u32 {
        if self.choices.is_empty() {
            2
        } else {
            self.choices.len() as u32
        }
    }

    /// The bits it takes in the layout options value: the fewest that hold its choices, at
    /// least one.
    fn bits(&self) -> u32 {
        (u32::BITS - (self.count() - 1).leading_zeros()).max(1)
    }
}

/// A key and the layout option choice (`group, choice`) it belongs to, if any.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Placed {
    pub(crate) key: Key,
    pub(crate) option: Option<(u8, u8)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    name: Option<String>,
    vendor_id: Option<String>,
    product_id: Option<String>,
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
    #[serde(default)]
    labels: Vec<Value>,
}

fn hex(s: &str) -> Result<u16, String> {
    u16::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
        .map_err(|_| format!("bad id {s:?}"))
}

impl Definition {
    /// A VIA definition file: it names the keyboard and gives its USB ids.
    pub fn parse(json: &str) -> Result<Definition, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let field = |v: &Option<String>, what: &str| v.clone().ok_or(format!("no {what}"));
        let name = field(&raw.name, "name")?;
        let ids = (
            hex(&field(&raw.vendor_id, "vendorId")?)?,
            hex(&field(&raw.product_id, "productId")?)?,
        );
        Definition::build(raw, name, ids)
    }

    /// A definition a keyboard keeps itself (Vial's): the board's own USB ids count, and the
    /// JSON's name, else `name`, since Vial's definitions may leave both out.
    pub fn from_board(json: &str, vid: u16, pid: u16, name: &str) -> Result<Definition, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let name = raw.name.clone().unwrap_or_else(|| name.to_string());
        Definition::build(raw, name, (vid, pid))
    }

    /// A definition file, VIA's or QMK's, told apart by what's in it (#74): a VIA definition
    /// has `layouts.keymap` (KLE rows); a QMK keyboard.json / info.json has `usb`, a matrix or
    /// `LAYOUT_*` layouts. A QMK file is completed from the info.json files of the folders above
    /// it, as QMK's build does ([`crate::qmk::load`]).
    pub fn load(path: &std::path::Path) -> Result<Definition, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        // Windows editors may save a byte order mark, which JSON doesn't allow.
        let text = text.trim_start_matches('\u{feff}');
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if value.pointer("/layouts/keymap").is_some() {
            Definition::parse(text)
        } else if crate::qmk::looks_like(&value) {
            crate::qmk::load(path, text)
        } else {
            Err("neither a VIA definition nor a QMK keyboard.json / info.json".into())
        }
    }

    fn build(
        raw: Raw,
        name: String,
        (vendor_id, product_id): (u16, u16),
    ) -> Result<Definition, String> {
        let keys = kle_keys(&raw.layouts.keymap)?;
        // VIA's built-in lighting menus (`qmk_backlight_rgblight` names two).
        let text: String = raw
            .menus
            .iter()
            .chain(raw.lighting.as_ref())
            .map(Value::to_string)
            .collect();
        let lighting = [
            ("qmk_rgb_matrix", Channel::RgbMatrix),
            ("rgblight", Channel::RgbLight), // qmk_rgblight, qmk_backlight_rgblight
            ("qmk_led_matrix", Channel::LedMatrix),
            ("qmk_backlight", Channel::Backlight),
        ]
        .into_iter()
        .filter(|(menu, _)| text.contains(menu))
        .map(|(_, c)| c)
        .collect();
        let options = options(&raw.layouts.labels, &keys);
        Definition::assemble(
            name,
            (vendor_id, product_id),
            (raw.matrix.rows, raw.matrix.cols),
            keys,
            options,
            lighting,
            None,
        )
    }

    /// The definition from its parts, VIA's or QMK's: every key inside the matrix, the default
    /// layout's top-left corner as the origin (as in VIA).
    pub(crate) fn assemble(
        name: String,
        (vendor_id, product_id): (u16, u16),
        (rows, cols): (u16, u16),
        mut keys: Vec<Placed>,
        options: Vec<LayoutOption>,
        lighting: Vec<Channel>,
        qmk_layout: Option<String>,
    ) -> Result<Definition, String> {
        for Placed { key: k, .. } in &keys {
            if k.row >= rows || k.col >= cols {
                return Err(format!(
                    "key {},{} is outside the {rows}x{cols} matrix",
                    k.row, k.col
                ));
            }
        }
        let (x0, y0) = keys
            .iter()
            .filter(|p| p.option.is_none_or(|(_, c)| c == 0))
            .fold((f32::INFINITY, f32::INFINITY), |(x, y), p| {
                (x.min(p.key.x), y.min(p.key.y))
            });
        if x0.is_finite() && y0.is_finite() {
            for p in &mut keys {
                (p.key.x, p.key.y) = (p.key.x - x0, p.key.y - y0);
            }
        }
        let mut d = Definition {
            lighting,
            name: name.clone(),
            vendor_id,
            product_id,
            rows,
            cols,
            layout: Layout { name, keys: vec![] },
            options,
            qmk_layout,
            keys,
        };
        d.layout = d.layout_for(0);
        Ok(d)
    }

    /// The choice of each layout option in a layout options value (as the keyboard reports it),
    /// in group order. The last group is in the lowest bits; a choice past a group's last is 0.
    pub fn choices(&self, value: u32) -> Vec<u8> {
        let mut shift = 0u32;
        let mut out: Vec<u8> = self
            .options
            .iter()
            .rev()
            .map(|o| {
                let bits = o.bits().min(8);
                let c = u64::from(value).checked_shr(shift).unwrap_or(0) & ((1 << bits) - 1);
                shift += bits;
                if c < u64::from(o.count()) {
                    c as u8
                } else {
                    0
                }
            })
            .collect();
        out.reverse();
        out
    }

    /// Whether a layout options value is one this definition can mean, for a write (#723): no
    /// bits above its groups, and each group's choice one it has. Without options only 0 is.
    pub fn valid_layout_options(&self, value: u32) -> bool {
        let mut shift = 0u32;
        let mut rebuilt = 0u64;
        for (o, c) in self.options.iter().zip(self.choices(value)).rev() {
            rebuilt |= u64::from(c) << shift;
            shift += o.bits().min(8);
        }
        shift <= 32 && rebuilt == u64::from(value)
    }

    /// The chosen layout options in words, for a layout options value: a toggle that is on by
    /// its name ("Split Backspace"), a group with choices as "name: choice" ("Enter: 2u"). Off
    /// toggles are left out.
    pub fn chosen_names(&self, value: u32) -> Vec<String> {
        self.options
            .iter()
            .zip(self.choices(value))
            .filter_map(|(o, c)| match o.choices.get(usize::from(c)) {
                Some(choice) => Some(format!("{}: {choice}", o.name)),
                None => (c != 0).then(|| o.name.clone()),
            })
            .collect()
    }

    /// The layout for a layout options value: the keys outside any option and, of each option,
    /// the chosen choice's keys, moved so its top-left key is where choice 0's is.
    pub fn layout_for(&self, value: u32) -> Layout {
        let chosen = self.choices(value);
        // A choice's top-left key: the topmost, then the leftmost.
        let pivot = |g: u8, c: u8| {
            self.keys
                .iter()
                .filter(|p| p.option == Some((g, c)))
                .map(|p| (p.key.x, p.key.y))
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)))
        };
        let keys = self
            .keys
            .iter()
            .filter_map(|p| match p.option {
                None => Some(p.key.clone()),
                Some((g, c)) if chosen.get(usize::from(g)).copied().unwrap_or(0) != c => None,
                Some((_, 0)) => Some(p.key.clone()),
                Some((g, c)) => {
                    let mut k = p.key.clone();
                    if let (Some(to), Some(from)) = (pivot(g, 0), pivot(g, c)) {
                        (k.x, k.y) = (k.x + to.0 - from.0, k.y + to.1 - from.1);
                    }
                    Some(k)
                }
            })
            .collect();
        Layout {
            name: self.layout.name.clone(),
            keys,
        }
    }
}

/// The layout option groups: as `layouts.labels` names them, and for a group or choice that a
/// key uses but the labels don't name, numbered ones (at least a toggle's two choices), so the
/// value still unpacks.
fn options(labels: &[Value], keys: &[Placed]) -> Vec<LayoutOption> {
    let text = |v: &Value| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    };
    let mut options: Vec<LayoutOption> = labels
        .iter()
        .map(|l| match l {
            Value::Array(items) => LayoutOption {
                name: items.first().map(text).unwrap_or_default(),
                choices: items.iter().skip(1).map(text).collect(),
            },
            other => LayoutOption {
                name: text(other),
                choices: vec![],
            },
        })
        .collect();
    for (g, c) in keys.iter().filter_map(|p| p.option) {
        let g = usize::from(g);
        while options.len() <= g {
            options.push(LayoutOption {
                name: format!("Option {}", options.len()),
                choices: vec![],
            });
        }
        let o = &mut options[g];
        if u32::from(c) >= o.count() {
            let named = o.choices.len();
            o.choices
                .extend((named..=usize::from(c)).map(|c| c.to_string()));
        }
    }
    options
}

/// Keys from KLE rows: strings are keys (label `row,col` first, `group,choice` on the fourth
/// line for a layout option's keys), objects set the next key's offset (`x`, `y`) and size
/// (`w`, `h`). Keys whose label isn't `row,col` (decals, option labels) are skipped.
fn kle_keys(rows: &[Value]) -> Result<Vec<Placed>, String> {
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
                    let lines: Vec<&str> = label.split('\n').collect();
                    if let (Some((row, col)), false) = (pair(lines[0]), decal) {
                        keys.push(Placed {
                            key: Key {
                                row,
                                col,
                                x,
                                y,
                                w,
                                h,
                                r: 0.0,
                            },
                            option: lines.get(3).and_then(|l| pair(l)),
                        });
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

/// `a,b` as two numbers.
fn pair<N: std::str::FromStr>(s: &str) -> Option<(N, N)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
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

/// A board with three layout options, the way VIA definitions draw them: the alternatives to the
/// right of (or below) the default keys.
/// - group 0, "Split Backspace" (toggle): one 2u key (0,3) or two 1u keys (0,3 and 1,3)
/// - group 1, "Enter" (3 choices, 2 bits): 1,2 as 1u, 1.5u or 2u
/// - group 2, "Split Space" (toggle): 2,0 as one 3u key, or 2,0 and 2,1 as 1.5u each
#[cfg(test)]
pub(crate) const OPTIONS: &str = r#"{
  "name": "Options board",
  "vendorId": "0xFEED",
  "productId": "0x0002",
  "matrix": {"rows": 3, "cols": 4},
  "layouts": {
    "labels": ["Split Backspace", ["Enter", "1u", "1.5u", "2u"], "Split Space"],
    "keymap": [
      ["0,0", "0,1", "0,2", {"w": 2}, "0,3\n\n\n0,0", {"x": 1}, "0,3\n\n\n0,1", "1,3\n\n\n0,1"],
      ["1,0", "1,1", "1,2\n\n\n1,0", {"x": 1, "w": 1.5}, "1,2\n\n\n1,1", {"x": 1, "w": 2}, "1,2\n\n\n1,2"],
      [{"w": 3}, "2,0\n\n\n2,0"],
      [{"y": 0.5, "w": 1.5}, "2,0\n\n\n2,1", {"w": 1.5}, "2,1\n\n\n2,1"]
    ]
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ids_matrix_layout_and_lighting() {
        let d = Definition::parse(SAMPLE).unwrap();
        assert_eq!(
            (d.vendor_id, d.product_id, d.rows, d.cols),
            (0x3434, 0x0250, 2, 4)
        );
        assert_eq!(d.lighting, vec![Channel::RgbMatrix]);
        let both = Definition::parse(&SAMPLE.replace("qmk_rgb_matrix", "qmk_backlight_rgblight"));
        assert_eq!(
            both.unwrap().lighting,
            vec![Channel::RgbLight, Channel::Backlight]
        );
        let k = |r, c| {
            d.layout
                .keys
                .iter()
                .find(|k| k.row == r && k.col == c)
                .cloned()
                .unwrap()
        };
        assert_eq!((k(0, 1).x, k(0, 1).w), (1.0, 1.5));
        assert_eq!(k(0, 3).x, 3.75);
        assert_eq!((k(1, 0).y, k(1, 0).w), (1.5, 2.25));
        assert_eq!(k(1, 1).x, 3.25, "the decal took space but isn't a key");
        assert_eq!(d.layout.keys.len(), 7);
    }

    /// Vial's definitions may leave out the name and ids: the board's own count, and a file
    /// still needs them.
    #[test]
    fn a_boards_own_definition_takes_the_boards_ids() {
        let bare = r#"{"matrix": {"rows": 1, "cols": 2}, "lighting": "qmk_rgblight",
            "layouts": {"keymap": [["0,0", "0,1"]]}}"#;
        let d = Definition::from_board(bare, 0xFEED, 0x6060, "QMK keyboard feed:6060").unwrap();
        assert_eq!(
            (
                d.name.as_str(),
                d.vendor_id,
                d.product_id,
                d.layout.keys.len()
            ),
            ("QMK keyboard feed:6060", 0xFEED, 0x6060, 2)
        );
        assert_eq!(d.lighting, vec![Channel::RgbLight]);
        assert!(Definition::parse(bare).unwrap_err().contains("name"));
        let named = Definition::from_board(SAMPLE, 0x1234, 0x0001, "fallback").unwrap();
        assert_eq!(
            (named.name.as_str(), named.vendor_id),
            ("Sample K5-like", 0x1234),
            "the board's ids win over the JSON's"
        );
    }

    #[test]
    fn rejects_keys_outside_the_matrix() {
        let bad = SAMPLE.replace("\"1,2\"", "\"5,2\"");
        assert!(Definition::parse(&bad).unwrap_err().contains("outside"));
        let bad = OPTIONS.replace("1,3\\n\\n\\n0,1", "1,9\\n\\n\\n0,1");
        assert!(
            Definition::parse(&bad).unwrap_err().contains("outside"),
            "an alternative's keys are checked too"
        );
    }

    #[test]
    fn reads_the_layout_option_groups() {
        let d = Definition::parse(OPTIONS).unwrap();
        let names: Vec<_> = d.options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["Split Backspace", "Enter", "Split Space"]);
        assert_eq!(d.options[1].choices, ["1u", "1.5u", "2u"]);
        assert_eq!(
            d.options.iter().map(|o| o.bits()).collect::<Vec<_>>(),
            [1, 2, 1]
        );
    }

    /// The first group in the highest bits: 0b1101 is 1 | 10 | 1: split backspace,
    /// 2u enter, split space.
    #[test]
    fn unpacks_the_layout_options_value() {
        let d = Definition::parse(OPTIONS).unwrap();
        assert_eq!(d.choices(0), [0, 0, 0]);
        assert_eq!(d.choices(0b1101), [1, 2, 1]);
        assert_eq!(d.choices(0b0010), [0, 1, 0]);
        assert_eq!(d.choices(0b0110), [0, 0, 0], "no fourth enter: the default");
        assert_eq!(
            d.choices(0xFFFF_FFF0),
            [0, 0, 0],
            "bits above the groups are ignored"
        );
    }

    /// #723: a layout options write must be a value the definition can mean.
    #[test]
    fn only_meaningful_layout_options_values_are_valid() {
        let d = Definition::parse(OPTIONS).unwrap();
        for v in [0, 0b0001, 0b0010, 0b0100, 0b1101, 0b1000] {
            assert!(d.valid_layout_options(v), "{v:#b}");
        }
        for v in [0b0110, 0b1110, 0b1_0000, 0xFFFF_FFF0, u32::MAX] {
            assert!(!d.valid_layout_options(v), "{v:#b}");
        }
        let mut none = Definition::parse(SAMPLE).unwrap();
        none.options.clear();
        assert!(none.valid_layout_options(0) && !none.valid_layout_options(1));
    }

    fn keys(l: &Layout) -> Vec<(u16, u16, f32, f32, f32)> {
        let mut v: Vec<_> = l
            .keys
            .iter()
            .map(|k| (k.row, k.col, k.x, k.y, k.w))
            .collect();
        v.sort_by_key(|k| (k.0, k.1));
        v
    }

    #[test]
    fn the_default_layout_has_choice_zero_of_each_option() {
        let d = Definition::parse(OPTIONS).unwrap();
        assert_eq!(d.layout, d.layout_for(0));
        assert_eq!(
            keys(&d.layout),
            [
                (0, 0, 0.0, 0.0, 1.0),
                (0, 1, 1.0, 0.0, 1.0),
                (0, 2, 2.0, 0.0, 1.0),
                (0, 3, 3.0, 0.0, 2.0),
                (1, 0, 0.0, 1.0, 1.0),
                (1, 1, 1.0, 1.0, 1.0),
                (1, 2, 2.0, 1.0, 1.0),
                (2, 0, 0.0, 2.0, 3.0),
            ]
        );
    }

    /// Each matrix position once, and the alternatives moved onto the default keys' place.
    #[test]
    fn a_chosen_option_replaces_the_default_keys_in_place() {
        let d = Definition::parse(OPTIONS).unwrap();
        assert_eq!(
            keys(&d.layout_for(0b1101)),
            [
                (0, 0, 0.0, 0.0, 1.0),
                (0, 1, 1.0, 0.0, 1.0),
                (0, 2, 2.0, 0.0, 1.0),
                (0, 3, 3.0, 0.0, 1.0),
                (1, 0, 0.0, 1.0, 1.0),
                (1, 1, 1.0, 1.0, 1.0),
                (1, 2, 2.0, 1.0, 2.0),
                (1, 3, 4.0, 0.0, 1.0),
                (2, 0, 0.0, 2.0, 1.5),
                (2, 1, 1.5, 2.0, 1.5),
            ]
        );
        // The 1.5u enter, drawn further right in the file, lands on the default's place too.
        let enter: Vec<_> = keys(&d.layout_for(0b0010))
            .into_iter()
            .filter(|k| (k.0, k.1) == (1, 2))
            .collect();
        assert_eq!(enter, [(1, 2, 2.0, 1.0, 1.5)]);
    }

    /// Labels missing (some definitions only mark the keys): the groups and choices the keys
    /// use still unpack, each a toggle unless a key uses a later choice.
    #[test]
    fn keys_marked_without_labels_still_unpack() {
        let raw = OPTIONS.replace(
            r#""labels": ["Split Backspace", ["Enter", "1u", "1.5u", "2u"], "Split Space"],"#,
            "",
        );
        let d = Definition::parse(&raw).unwrap();
        assert_eq!(
            d.options
                .iter()
                .map(LayoutOption::count)
                .collect::<Vec<_>>(),
            [2, 3, 2]
        );
        assert_eq!(d.choices(0b1101), [1, 2, 1]);
        assert_eq!(d.options[0].name, "Option 0");
    }

    /// #797: the chosen options in words, for the drawing's hover: toggles on by name, a group
    /// with choices as "name: choice".
    #[test]
    fn chosen_options_in_words() {
        let d = Definition::parse(OPTIONS).unwrap();
        assert_eq!(d.chosen_names(0), ["Enter: 1u"]);
        assert_eq!(
            d.chosen_names(0b1101),
            ["Split Backspace", "Enter: 2u", "Split Space"]
        );
    }

    #[test]
    fn a_definition_without_options_is_unchanged_by_the_value() {
        let d = Definition::parse(SAMPLE).unwrap();
        // SAMPLE's 1,1 is marked 0,0 without labels: one toggle group.
        assert_eq!(d.options.len(), 1);
        assert_eq!(keys(&d.layout_for(0)), keys(&d.layout));
        assert_eq!(d.layout_for(1).keys.len(), 6, "choice 1 has no keys");
    }
}
