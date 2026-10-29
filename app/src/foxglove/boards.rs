//! Board profiles (#75): what Foxglove knows about a keyboard beyond what its firmware says,
//! as data. A profile names the board, how to recognise it (USB ids and product name; ZMK
//! boards share one vid:pid), its protocol and halves, the screens to expect (the firmware's
//! own descriptors win), its lights, the keymap source behind the layer-access hints, and what
//! its firmware build lets ZMK Studio do (`[studio]`, #580).
//!
//! Built-in profiles ship in `resources/boards`; more go in the `boards` folder next to
//! Foxglove's settings, which wins over a built-in one with the same id. Profiles are read when
//! a keyboard connects, never on a timer.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use foxglove_link::lamps::LampParts;
use serde::Deserialize;

const FORMAT: &str = "foxglove-board";

const BUILT_IN: &[(&str, &str)] = &[
    (
        "eyelash-corne.toml",
        include_str!("../../resources/boards/eyelash-corne.toml"),
    ),
    (
        "toucan2.toml",
        include_str!("../../resources/boards/toucan2.toml"),
    ),
    (
        "halcyon-corne-epaper.toml",
        include_str!("../../resources/boards/halcyon-corne-epaper.toml"),
    ),
];

/// Keymap sources a built-in profile can name.
const KEYMAPS: &[(&str, &str)] = &[
    (
        "eyelash_corne",
        include_str!("../../../config/eyelash_corne.keymap"),
    ),
    (
        "toucan",
        include_str!("../../../config-toucan2/toucan.keymap"),
    ),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Tested,
    #[default]
    Untested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Zmk,
    Via,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Match {
    pub vid: u16,
    pub pid: u16,
    /// The USB product name, as the firmware reports it (ZMK's `ZMK_KEYBOARD_NAME`).
    #[serde(default)]
    pub product: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Screen {
    /// `display`, `panel` (where the PC draws) or `art` (where art plays).
    pub kind: String,
    /// The part it sits on (informative; screens are named by role, #560).
    #[serde(default)]
    pub half: String,
    pub width: u16,
    pub height: u16,
    /// `1bit`; other formats aren't drawn yet.
    pub format: String,
    /// What the app calls it; the role's name ("Info screen", "Art screen") when absent (#560).
    #[serde(default)]
    pub name: Option<String>,
    /// Degrees clockwise from upright to how the screen stores its picture (0, 90, 180, 270):
    /// an art screen mounted on its side is 90.
    #[serde(default)]
    pub rotate: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Lighting {
    pub lamps: u16,
    /// Each part's LEDs in SET_FRAME order (the left half's first) when the halves differ, e.g.
    /// `[30, 24]`; empty shares `lamps` out evenly between the board's halves (#561).
    #[serde(default)]
    pub parts: Vec<u16>,
    #[serde(default)]
    pub layout: String,
    /// What each LED (each data pixel of the strip) lights, in chain order: the key it sits
    /// under, an index into [`Drawing::keys`]; or, for a pixel that drives several keys'
    /// LEDs together, the list of them; or `[]` for one that lights nothing (#608). Either
    /// one half's strip, which the other half mirrors (halves of the same count), or every LED
    /// of the board part by part, each part's into its own drawing. Scenes place LEDs by it
    /// (#529): a group at its keys' centre, and a dead one gets no colour.
    #[serde(default)]
    pub led_keys: Vec<Lamp>,
    /// [`LED_MAP_CHECKED`] once `led_keys` is confirmed on the board (a sitting, the dot test, or
    /// "Find my LEDs" placed them); [`LED_MAP_SHIPPED`] for a map that comes with the profile but
    /// no one has checked on a board yet (scenes use it, the app asks for the dot test once);
    /// anything else (`provisional`) and scenes light each half as one.
    #[serde(default)]
    pub led_map: String,
    /// Where "Find my LEDs" saw each LED (the user's override, the store's `[led_map]`): one
    /// entry per lamp in SET_FRAME order, a point on its half's drawing in key units, `None`
    /// for one the user couldn't see. Wins over `led_keys` when set.
    #[serde(skip)]
    pub led_points: Vec<Option<[f32; 2]>>,
}

/// What one LED of a board profile's map lights ([`Lighting::led_keys`]): the keys under it, as
/// indices into [`Drawing::keys`]. One key for a per-key LED, several for a pixel that drives a
/// group of keys together (the Eyelash Corne's columns, #608), none for a pixel that lights
/// nothing. In the profile a number is one key and a list a group: `led_keys = [20, 17]` and
/// `led_keys = [[15, 16, 17, 20], [12, 13, 14, 19], []]` both read.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(from = "LampEntry")]
pub struct Lamp(pub Vec<u8>);

/// A [`Lamp`] as the profile writes it.
#[derive(Deserialize)]
#[serde(untagged)]
enum LampEntry {
    One(u8),
    Group(Vec<u8>),
}

impl From<LampEntry> for Lamp {
    fn from(e: LampEntry) -> Self {
        match e {
            LampEntry::One(k) => Lamp(vec![k]),
            LampEntry::Group(keys) => Lamp(keys),
        }
    }
}

impl From<u8> for Lamp {
    fn from(k: u8) -> Self {
        Lamp(vec![k])
    }
}

impl Lamp {
    /// A one-key-per-LED chain as lamps.
    pub fn chain(keys: &[u8]) -> Vec<Lamp> {
        keys.iter().map(|&k| Lamp::from(k)).collect()
    }

    pub fn keys(&self) -> &[u8] {
        &self.0
    }

    /// It lights nothing: scenes and the dot test skip it.
    pub fn dead(&self) -> bool {
        self.0.is_empty()
    }

    /// Where it sits on a drawing's `keys`: its keys' centre, in key units. `None` for a dead
    /// lamp or one naming a key the drawing hasn't.
    pub fn center(&self, keys: &[egui::Rect]) -> Option<egui::Pos2> {
        if self.dead() {
            return None;
        }
        let mut sum = egui::Vec2::ZERO;
        for &k in &self.0 {
            sum += keys.get(usize::from(k))?.center().to_vec2();
        }
        Some((sum / self.0.len() as f32).to_pos2())
    }
}

impl Lighting {
    /// Whether scenes place each LED by this map: checked, or shipped with the profile.
    pub fn led_map_usable(&self) -> bool {
        self.led_map == LED_MAP_CHECKED || self.led_map == LED_MAP_SHIPPED
    }

    /// The LEDs by part: `parts` when it adds up to `lamps`, else `lamps` shared out evenly
    /// between `halves`. `None` for a profile whose counts a frame can't carry.
    pub fn lamp_parts(&self, halves: u8) -> Option<LampParts> {
        let given: Vec<usize> = self.parts.iter().map(|&n| usize::from(n)).collect();
        if !given.is_empty() && given.iter().sum::<usize>() == usize::from(self.lamps) {
            return LampParts::new(&given).ok();
        }
        LampParts::even(usize::from(self.lamps), usize::from(halves.max(1))).ok()
    }

    /// The keys LED by LED of part `part` (of `parts`): its share of a whole-board map, or the
    /// one-half map every half of that count mirrors. `None` when the map is neither.
    pub fn part_keys(&self, parts: &LampParts, part: usize) -> Option<&[Lamp]> {
        if self.led_keys.len() == parts.total() {
            return self.led_keys.get(parts.range(part));
        }
        (self.led_keys.len() == parts.count(part)).then_some(&self.led_keys[..])
    }
}

/// The keyboard's LEDs by part: the board profile's, else what firmware that doesn't report
/// them has (the Eyelash Corne's, `foxglove_link::lamps`), as one strip on a one-piece board.
pub fn lamp_parts(board: Option<&Board>) -> LampParts {
    if let Some(parts) = board.and_then(|b| b.lighting.as_ref()?.lamp_parts(b.halves)) {
        return parts;
    }
    let old = LampParts::old_firmware();
    match board {
        Some(b) if b.halves == 1 => LampParts::even(old.total(), 1).unwrap_or(old),
        _ => old,
    }
}

/// What is wrong with a profile's LED counts, if anything (#561): `parts` that don't add up to
/// `lamps`, or more LEDs than a frame addresses. Said once when the keyboard connects.
fn lamps_problem(board: &Board) -> Option<String> {
    let l = board.lighting.as_ref()?;
    let given: u32 = l.parts.iter().map(|&n| u32::from(n)).sum();
    if !l.parts.is_empty() && given != u32::from(l.lamps) {
        return Some(format!(
            "its parts {:?} add up to {given} LEDs, not its {} lamps: shared out evenly instead",
            l.parts, l.lamps
        ));
    }
    if l.lamp_parts(board.halves).is_some() {
        return None;
    }
    let given: Vec<usize> = l.parts.iter().map(|&n| usize::from(n)).collect();
    let refused = if given.is_empty() {
        LampParts::even(usize::from(l.lamps), usize::from(board.halves.max(1))).err()
    } else {
        LampParts::new(&given).err()
    };
    refused.map(|e| {
        format!(
            "its {} lamps on {} half(s) don't make a frame ({e}): the old firmware's count instead",
            l.lamps, board.halves
        )
    })
}

/// What a ZMK board's firmware build lets ZMK Studio do (#580), as the profile states it: the
/// build's Kconfig and keymap, which the board doesn't report until a Studio session is open.
/// The board's own answers win once it does (its keymap's `available_layers`, its lock state).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Studio {
    /// The build has `CONFIG_ZMK_STUDIO=y`: its keymap can be edited from the PC.
    #[serde(default)]
    pub editing: bool,
    /// `CONFIG_ZMK_STUDIO_LOCKING` (ZMK's default is on): writes wait for the unlock key.
    #[serde(default)]
    pub locking: bool,
    /// Keymap layers with `status = "reserved"`: how many Studio's "Add layer" can enable.
    /// Studio can't add a layer beyond these (it answers NO_SPACE).
    #[serde(default)]
    pub reserved_layers: u8,
    /// The ZMK release the firmware builds on (`v0.3.0`, `main`), for fixes only some have.
    #[serde(default)]
    pub zmk: String,
}

/// The most keys ZMK v0.3 saves from Studio: more fail to save until ZMK main (zmk #3431).
pub const STUDIO_SAVE_KEY_LIMIT: usize = 190;

impl Studio {
    /// Whether saving a keymap of `keys` keys from Studio fails on this build: more than
    /// [`STUDIO_SAVE_KEY_LIMIT`] on a ZMK release before the fix (v0.3 and older). Unknown
    /// releases are assumed fixed.
    pub fn save_fails(&self, keys: usize) -> bool {
        let release = self.zmk.trim().trim_start_matches('v');
        let old = release
            .split('.')
            .take(2)
            .map(|n| n.parse::<u32>().ok())
            .collect::<Option<Vec<_>>>()
            .is_some_and(|v| v.first() == Some(&0) && v.get(1).is_some_and(|m| *m <= 3));
        old && keys > STUDIO_SAVE_KEY_LIMIT
    }

    /// What the keymap editor should say about this build, for a keymap of `keys` keys (0 when
    /// not read yet): no spare layers to add, and a save that would fail.
    pub fn notes(&self, keys: usize) -> Vec<String> {
        let mut notes = Vec::new();
        if !self.editing {
            return notes;
        }
        if self.reserved_layers == 0 {
            notes.push(
                "This board's keymap reserves no spare layers, so layers can't be added here.                  Its firmware needs layers with status = \"reserved\"."
                    .into(),
            );
        }
        if self.save_fails(keys) {
            notes.push(format!(
                "Saving more than {STUDIO_SAVE_KEY_LIMIT} keys needs firmware built on ZMK main                  or later; this board's is on ZMK {}.",
                self.zmk.trim()
            ));
        }
        notes
    }

    /// One line for Troubleshooting: "editing on, unlocked, 3 spare layers".
    pub fn summary(&self) -> String {
        if !self.editing {
            return "editing off".into();
        }
        let lock = if self.locking {
            "unlock key needed"
        } else {
            "no unlock key"
        };
        let spare = match self.reserved_layers {
            0 => "no spare layers".to_string(),
            1 => "1 spare layer".to_string(),
            n => format!("{n} spare layers"),
        };
        format!("editing on, {lock}, {spare}")
    }
}

/// The widest grid a drawing may have: a whole full-size board.
const MAX_COLUMNS: u8 = 24;
/// The most keys a drawing may list.
const MAX_KEYS: usize = 160;

/// One part of a board (#559): a half of a split, the one piece, or a module. The first is the
/// one the PC talks to (ZMK's central).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PartSpec {
    pub id: String,
    /// What the app calls it: "Left half", "Keyboard", "Numpad".
    pub name: String,
}

/// [`Lighting::led_map`] for an LED map confirmed on the board.
pub const LED_MAP_CHECKED: &str = "checked";

/// [`Lighting::led_map`] for a map shipped with the profile and not yet checked on a board.
pub const LED_MAP_SHIPPED: &str = "shipped";

/// How the Keyboard page draws one part of the board (#392, #559), in key units: a grid of
/// columns with each column dropped by its stagger and a row of thumb keys under the inner
/// columns, or each key's place (`keys`, as KLE and QMK's `info.json` list them). On a split
/// board the first part is described and the second is its mirror image; a one-piece board's
/// drawing is the whole board.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Drawing {
    #[serde(default)]
    pub columns: u8,
    #[serde(default)]
    pub rows: u8,
    #[serde(default)]
    pub thumbs: u8,
    /// How far each column sits below the highest one, outer column first; missing ones are 0.
    #[serde(default)]
    pub stagger: Vec<f32>,
    /// Each key's top-left corner and width, `[x, y]` or `[x, y, w]` in key units, in key order.
    /// Wins over the grid when set.
    #[serde(default)]
    pub keys: Vec<Vec<f32>>,
}

/// Keys in the plain grid drawn for a keyboard Foxglove knows nothing about.
const PLAIN_KEYS: usize = 40;

impl Default for Drawing {
    /// A plain grid, for a keyboard without a profile (or a profile without a drawing) whose
    /// keymap hasn't said how many keys it has yet. See [`Drawing::plain`].
    fn default() -> Self {
        Drawing::plain(PLAIN_KEYS)
    }
}

impl Drawing {
    /// A plain grid of `keys` keys, row after row, about three times as wide as it is tall:
    /// the shape for a board with no drawing, sized from its keymap.
    pub fn plain(keys: usize) -> Drawing {
        let keys = keys.clamp(1, MAX_KEYS);
        let rows = ((keys as f32 / 3.0).sqrt().round() as usize).clamp(1, 8);
        let columns = keys.div_ceil(rows);
        Drawing {
            columns: 0,
            rows: 0,
            thumbs: 0,
            stagger: Vec::new(),
            keys: (0..keys)
                .map(|i| vec![(i % columns) as f32, (i / columns) as f32])
                .collect(),
        }
    }

    /// Within reason, so a mistyped profile can't draw a wall of keys.
    fn bounded(&self) -> (u8, u8, u8) {
        (
            self.columns.clamp(1, MAX_COLUMNS),
            self.rows.clamp(1, 8),
            self.thumbs.min(self.columns.clamp(1, MAX_COLUMNS)),
        )
    }

    /// The listed keys, each within reason; `None` for a grid drawing.
    fn placed(&self) -> Option<Vec<egui::Rect>> {
        if self.keys.is_empty() {
            return None;
        }
        let at = |v: Option<&f32>, max: f32| {
            v.copied()
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, max)
        };
        Some(
            self.keys
                .iter()
                .take(MAX_KEYS)
                .map(|k| {
                    let w = k
                        .get(2)
                        .copied()
                        .filter(|w| w.is_finite())
                        .unwrap_or(1.0)
                        .clamp(0.5, 10.0);
                    egui::Rect::from_min_size(
                        egui::pos2(at(k.first(), 40.0), at(k.get(1), 12.0)),
                        egui::vec2(w, 1.0),
                    )
                })
                .collect(),
        )
    }

    fn drop_of(&self, column: usize) -> f32 {
        self.stagger
            .get(column)
            .copied()
            .filter(|d| d.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, 2.0)
    }

    /// One part's keys, each a rectangle in key units from the part's top-left corner; `right`
    /// mirrors them (a split board's second half).
    pub fn keys(&self, right: bool) -> Vec<egui::Rect> {
        let mut keys = self.placed().unwrap_or_else(|| self.grid_keys());
        if right {
            let (w, _) = self.size();
            for k in &mut keys {
                *k = egui::Rect::from_min_size(egui::pos2(w - k.max.x, k.min.y), k.size());
            }
        }
        keys
    }

    fn grid_keys(&self) -> Vec<egui::Rect> {
        let (columns, rows, thumbs) = self.bounded();
        let mut keys = Vec::new();
        for c in 0..usize::from(columns) {
            for r in 0..usize::from(rows) {
                keys.push(egui::Rect::from_min_size(
                    egui::pos2(c as f32, r as f32 + self.drop_of(c)),
                    egui::vec2(1.0, 1.0),
                ));
            }
        }
        // The thumbs sit under the inner columns, a little below the lowest of them.
        let (first, below) = self.thumb_row();
        for t in 0..usize::from(thumbs) {
            keys.push(egui::Rect::from_min_size(
                egui::pos2((first + t) as f32 + 0.5, below),
                egui::vec2(1.0, 1.0),
            ));
        }
        keys
    }

    /// The key grid as [`keys`](Self::keys) draws it: columns, rows and thumbs. Key `c * rows + r`
    /// is column `c` (outer first), row `r` (top first); the thumbs follow, outer first. Listed
    /// keys count as one row.
    pub fn grid(&self) -> (usize, usize, usize) {
        if let Some(keys) = self.placed() {
            return (keys.len(), 1, 0);
        }
        let (c, r, t) = self.bounded();
        (usize::from(c), usize::from(r), usize::from(t))
    }

    /// Where the thumb row starts and how far down it sits.
    fn thumb_row(&self) -> (usize, f32) {
        let (columns, rows, thumbs) = self.bounded();
        let first = usize::from(columns - thumbs);
        let below = (first..usize::from(columns))
            .map(|c| self.drop_of(c))
            .fold(0.0, f32::max)
            + f32::from(rows)
            + 0.25;
        (first, below)
    }

    /// The part's width and height in key units.
    pub fn size(&self) -> (f32, f32) {
        if let Some(keys) = self.placed() {
            let r = keys.iter().fold(egui::Rect::NOTHING, |a, k| a.union(*k));
            return (r.max.x.max(1.0), r.max.y.max(1.0));
        }
        let (columns, rows, thumbs) = self.bounded();
        let lowest = (0..usize::from(columns))
            .map(|c| self.drop_of(c))
            .fold(0.0, f32::max);
        let grid = f32::from(rows) + lowest;
        if thumbs == 0 {
            return (f32::from(columns), grid);
        }
        (
            f32::from(columns) + 0.5,
            (self.thumb_row().1 + 1.0).max(grid),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Board {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: Status,
    pub protocol: Protocol,
    /// Shorthand for the parts: 1 is one piece, 2 a split with a left and a right half (the
    /// default, for profiles from before `[[parts]]`). `[[parts]]` wins and sets it.
    #[serde(default = "two")]
    pub halves: u8,
    /// The board's parts by name (#559), the one the PC talks to first.
    #[serde(default)]
    pub parts: Vec<PartSpec>,
    #[serde(default)]
    pub keymap: Option<String>,
    #[serde(rename = "match")]
    pub matches: Vec<Match>,
    #[serde(default)]
    pub screens: Vec<Screen>,
    #[serde(default)]
    pub lighting: Option<Lighting>,
    /// How the Keyboard page draws it (#392); a plain key grid when absent ([`drawing`]).
    #[serde(default)]
    pub drawing: Option<Drawing>,
    /// What its firmware lets ZMK Studio do (#580); unknown when absent.
    #[serde(default)]
    pub studio: Option<Studio>,
    /// The folder of a user profile, for its keymap file.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

fn two() -> u8 {
    2
}

impl Board {
    pub fn parse(text: &str) -> Result<Board, String> {
        let mut b: Board = toml::from_str(text).map_err(|e| e.to_string())?;
        if b.format != FORMAT || b.version != 1 {
            return Err(format!("not a {FORMAT} v1 profile"));
        }
        if b.matches.is_empty() {
            return Err("a profile needs at least one [[match]]".into());
        }
        // The host link reaches two parts: the one the PC talks to and its other half. More
        // `[[parts]]` name modules the link can't address yet; the lights count two.
        if !b.parts.is_empty() {
            b.halves = u8::try_from(b.parts.len().min(2)).unwrap_or(2);
        }
        Ok(b)
    }

    /// Its parts, by name (#559).
    pub fn parts(&self) -> super::parts::Parts {
        super::parts::Parts::of(Some(self))
    }

    fn matches(&self, vid: u16, pid: u16, product: &str) -> bool {
        self.matches.iter().any(|m| {
            (m.vid, m.pid) == (vid, pid)
                && m.product.as_deref().is_none_or(|p| same_name(p, product))
        })
    }

    /// The panel the PC draws on, when the firmware doesn't describe its screens.
    pub fn panel(&self) -> Option<&Screen> {
        self.screens
            .iter()
            .find(|s| s.kind == "panel" && s.format == "1bit")
    }

    /// Its keymap source: built in by name, or a `.keymap` file next to a user profile.
    pub fn keymap_source(&self) -> Option<String> {
        let name = self.keymap.as_deref()?;
        if let Some((_, text)) = KEYMAPS.iter().find(|(n, _)| *n == name) {
            return Some((*text).to_string());
        }
        let file = self.dir.as_ref()?.join(format!("{name}.keymap"));
        std::fs::read_to_string(file).ok()
    }
}

/// Product names compare by their letters and digits only, in any case: over BLE ZMK reports
/// `Eyelash_Corne` for the `Eyelash Corne` it names over USB.
fn same_name(a: &str, b: &str) -> bool {
    let key = |s: &str| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    key(a) == key(b)
}

/// The built-in profiles, then the user's (from `dir`), a user one replacing a built-in one
/// with the same id.
fn load(dir: Option<&Path>) -> Vec<Board> {
    let mut all: Vec<Board> = BUILT_IN
        .iter()
        .filter_map(|(file, text)| {
            Board::parse(text)
                .map_err(|e| eprintln!("Foxglove: board profile {file}: {e}"))
                .ok()
        })
        .collect();
    let user = dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten();
    for entry in user.filter(|e| e.path().extension().is_some_and(|x| x == "toml")) {
        match std::fs::read_to_string(entry.path())
            .map_err(|e| e.to_string())
            .and_then(|t| Board::parse(&t))
        {
            Ok(mut b) => {
                b.dir = dir.map(Path::to_path_buf);
                all.retain(|a| a.id != b.id);
                all.push(b);
            }
            Err(e) => eprintln!("Foxglove: board profile {}: {e}", entry.path().display()),
        }
    }
    all
}

/// The user's boards folder: `boards` next to Foxglove's settings.
pub fn user_dir() -> Option<PathBuf> {
    Some(super::store::path()?.parent()?.join("boards"))
}

/// The profile for a keyboard, from the profiles as they are now.
pub fn find(vid: u16, pid: u16, product: &str) -> Option<Board> {
    let mut board = find_in(&load(user_dir().as_deref()), vid, pid, product)?;
    super::led_map::apply(&mut board);
    Some(board)
}

fn find_in(all: &[Board], vid: u16, pid: u16, product: &str) -> Option<Board> {
    // A later profile (the user's) wins over an earlier one.
    all.iter()
        .rev()
        .find(|b| b.matches(vid, pid, product))
        .cloned()
}

static CURRENT: Mutex<Option<Board>> = Mutex::new(None);

/// A keyboard connected: looks its profile up once (profiles are read here, not on a timer).
pub fn connected(vid: u16, pid: u16, product: &str) {
    let board = find(vid, pid, product);
    if board.is_none() {
        eprintln!("Foxglove: no board profile for {vid:04x}:{pid:04x} \"{product}\"; using only what the firmware says");
    }
    if let Some(problem) = board.as_ref().and_then(lamps_problem) {
        eprintln!(
            "Foxglove: the board profile {}: {problem}",
            board.as_ref().map_or("", |b| b.id.as_str())
        );
    }
    super::led_map::note_current(board.as_ref());
    // Its keymap tells its key count again; the last keyboard's doesn't carry over.
    KEYMAP_KEYS.store(0, std::sync::atomic::Ordering::Relaxed);
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = board;
}

/// Every built-in keymap source, for a keyboard whose profile isn't known yet.
pub fn built_in_keymaps() -> impl Iterator<Item = &'static str> {
    KEYMAPS.iter().map(|(_, text)| *text)
}

/// The connected keyboard's profile, if it has one.
pub fn current() -> Option<Board> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// How many keys the connected keyboard's keymap has (its active physical layout), for the
/// drawing of a board without one; 0 before a keymap was read.
static KEYMAP_KEYS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A keymap was read: the plain drawing takes its key count.
pub fn keymap_keys(n: usize) {
    KEYMAP_KEYS.store(n, std::sync::atomic::Ordering::Relaxed);
}

/// What the keymap editor should say about the connected board's Studio build (#580), from its
/// profile: empty without a profile or a `[studio]` table.
pub fn studio_notes() -> Vec<String> {
    current()
        .and_then(|b| b.studio)
        .map(|s| s.notes(KEYMAP_KEYS.load(std::sync::atomic::Ordering::Relaxed)))
        .unwrap_or_default()
}

/// How a board is drawn: its profile's drawing, else a plain grid of its keymap's keys (#559),
/// shared out between a split's halves (the drawing is one half).
pub fn drawing(board: Option<&Board>) -> Drawing {
    plain_or_profile(
        board,
        KEYMAP_KEYS.load(std::sync::atomic::Ordering::Relaxed),
    )
}

fn plain_or_profile(board: Option<&Board>, keys: usize) -> Drawing {
    if let Some(d) = board.and_then(|b| b.drawing.clone()) {
        return d;
    }
    let drawn = board.map_or(1, |b| usize::from(b.halves.clamp(1, 2)));
    match keys {
        0 => Drawing::default(),
        n => Drawing::plain(n.div_ceil(drawn)),
    }
}

/// "Find my LEDs" saved a map: the connected keyboard's profile takes it at once.
pub fn led_map_changed() {
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(b) = current.as_mut() {
        super::led_map::apply(b);
    }
    super::led_map::note_current(current.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #608: a lamp is a key, a group of keys, or dead (`[]`), and an old one-key-per-LED map
    /// reads as before.
    #[test]
    fn led_keys_take_keys_groups_and_dead_lamps() {
        let parse = |map: &str| {
            let text = format!(
                "format = \"foxglove-board\"\nversion = 1\nid = \"t\"\nname = \"T\"\n\
                 protocol = \"zmk\"\n[[match]]\nvid = 1\npid = 2\n\
                 [lighting]\nlamps = 3\nled_keys = {map}\n"
            );
            Board::parse(&text).unwrap().lighting.unwrap().led_keys
        };
        assert_eq!(parse("[2, 0, 1]"), Lamp::chain(&[2, 0, 1]));
        let grouped = parse("[[0, 1], [2], []]");
        assert_eq!(grouped[0].keys(), [0, 1]);
        assert_eq!(grouped[1], Lamp::from(2));
        assert!(grouped[2].dead() && !grouped[0].dead());
        let keys = [
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
            egui::Rect::from_min_size(egui::pos2(0.0, 2.0), egui::vec2(1.0, 1.0)),
        ];
        assert_eq!(
            grouped[0].center(&keys),
            Some(egui::pos2(0.5, 1.5)),
            "the group's centre"
        );
        assert_eq!(grouped[1].center(&keys), None, "a key the drawing hasn't");
        assert_eq!(grouped[2].center(&keys), None, "dead");
    }

    #[test]
    fn the_built_in_profiles_parse_and_match_their_boards() {
        let all = load(None);
        assert_eq!(all.len(), BUILT_IN.len(), "every built-in profile parses");
        let corne = find_in(&all, 0x1d50, 0x615e, "Eyelash Corne").unwrap();
        assert_eq!((corne.status, corne.halves), (Status::Tested, 2));
        assert_eq!(corne.panel().map(|p| (p.width, p.height)), Some((64, 52)));
        assert!(corne
            .keymap_source()
            .is_some_and(|k| k.contains("zmk,keymap")));
        assert_eq!(corne.lighting.as_ref().map(|l| l.lamps), Some(42));
        // The shipped LED map (#607, #608): 21 pixels a half, the first 6 each a column of 3
        // keys plus its thumb key where it has one, inner column first; pixels 6-20 light
        // nothing. Every key once.
        let lighting = corne.lighting.clone().unwrap();
        assert_eq!(lighting.led_keys.len(), 21);
        assert_eq!(
            lighting.led_keys[0].keys(),
            [15, 16, 17, 20],
            "T, G, B and Space"
        );
        assert!(lighting.led_keys[6..].iter().all(Lamp::dead));
        let mut keys: Vec<u8> = lighting.led_keys.iter().flat_map(|l| l.0.clone()).collect();
        keys.sort_unstable();
        assert_eq!(keys, (0..21).collect::<Vec<u8>>());
        assert_eq!(lighting.led_map, LED_MAP_SHIPPED);
        let toucan = find_in(&all, 0x1d50, 0x615e, "Toucan").unwrap();
        assert_eq!(
            (
                toucan.status,
                toucan.screens[0].width,
                toucan.screens[0].height
            ),
            (Status::Untested, 144, 168)
        );
        assert!(toucan.keymap_source().is_some());
        let halcyon = find_in(&all, 0x1d50, 0x615e, "halcyon corne").unwrap();
        assert_eq!(
            (halcyon.status, halcyon.screens.len()),
            (Status::Untested, 2)
        );
        assert!(
            halcyon.keymap_source().is_none(),
            "no keymap source: no hints"
        );
        // The name as ZMK reports it over BLE, and in other spellings.
        for name in [
            "Eyelash_Corne",
            "eyelash-corne",
            " EYELASH CORNE ",
            "EyelashCorne",
        ] {
            let b = find_in(&all, 0x1d50, 0x615e, name);
            assert_eq!(b.map(|b| b.id), Some(corne.id.clone()), "{name}");
        }
        // ZMK's shared ids alone match nothing.
        assert!(find_in(&all, 0x1d50, 0x615e, "Some Other Board").is_none());
        assert!(find_in(&all, 0x3434, 0x0250, "Eyelash Corne").is_none());
    }

    /// The Studio facts (#580): each built-in ZMK profile with a keymap states them, our boards
    /// reserve spare layers, and the notes say what a build can't do.
    #[test]
    fn studio_facts_come_from_the_profile() {
        let all = load(None);
        let studio = |name: &str| find_in(&all, 0x1d50, 0x615e, name).and_then(|b| b.studio);
        let corne = studio("Eyelash Corne").expect("the Corne states its Studio facts");
        assert_eq!(
            (corne.editing, corne.locking, corne.reserved_layers),
            (true, false, 3)
        );
        assert_eq!(corne.summary(), "editing on, no unlock key, 3 spare layers");
        assert!(corne.notes(48).is_empty());
        let toucan = studio("Toucan").expect("the Toucan2 states its Studio facts");
        assert_eq!(
            (toucan.editing, toucan.locking, toucan.reserved_layers),
            (true, true, 2)
        );
        // The reserved layers in each keymap source match what the profile says.
        for b in all.iter().filter(|b| b.studio.is_some()) {
            let source = b.keymap_source().expect("a keymap source");
            let reserved = source.matches("status = \"reserved\"").count();
            assert_eq!(
                reserved,
                usize::from(b.studio.as_ref().unwrap().reserved_layers),
                "{}",
                b.id
            );
        }
        let none = Studio {
            editing: true,
            zmk: "v0.3.0".into(),
            ..Studio::default()
        };
        assert_eq!(none.notes(48).len(), 1, "no spare layers");
        assert!(none.notes(48)[0].contains("reserves no spare layers"));
        assert!(none.save_fails(191) && !none.save_fails(190));
        assert_eq!(none.notes(200).len(), 2);
        for fixed in ["main", "v0.4.0", "0.4", ""] {
            let s = Studio {
                zmk: fixed.into(),
                ..none.clone()
            };
            assert!(!s.save_fails(500), "{fixed}");
        }
        let off = Studio::default();
        assert!(off.notes(500).is_empty(), "no Studio, nothing to say");
        assert_eq!(off.summary(), "editing off");
    }

    /// The LED counts by part (#561): the profile's own split, an even share, a one-piece
    /// board's single strip, and a profile whose counts don't hold up, which is said.
    #[test]
    fn lamp_parts_come_from_the_profile() {
        let all = load(None);
        let mut b = find_in(&all, 0x1d50, 0x615e, "Eyelash Corne").unwrap();
        let counts = |b: &Board| lamp_parts(Some(b)).counts().collect::<Vec<_>>();
        assert_eq!(counts(&b), [21, 21]);
        assert_eq!(lamps_problem(&b), None);
        let l = b.lighting.as_mut().unwrap();
        (l.lamps, l.parts) = (54, vec![30, 24]);
        assert_eq!(counts(&b), [30, 24]);
        b.lighting.as_mut().unwrap().parts = vec![30, 30];
        assert_eq!(counts(&b), [27, 27], "parts that don't add up: shared out");
        assert!(lamps_problem(&b).is_some_and(|p| p.contains("add up to 60")));
        let l = b.lighting.as_mut().unwrap();
        (l.lamps, l.parts) = (300, Vec::new());
        assert_eq!(counts(&b), [21, 21], "past a frame: the old firmware's");
        assert!(lamps_problem(&b).is_some_and(|p| p.contains("300")));
        b.lighting.as_mut().unwrap().lamps = 0;
        assert!(lamps_problem(&b).is_some_and(|p| p.contains("at least one part")));
        b.halves = 1;
        b.lighting = None;
        assert_eq!(counts(&b), [42], "one strip on a one-piece board");
        assert_eq!(lamp_parts(None).counts().collect::<Vec<_>>(), [21, 21]);
    }

    #[test]
    fn the_corne_draws_from_its_profile() {
        let all = load(None);
        let corne = find_in(&all, 0x1d50, 0x615e, "Eyelash Corne").unwrap();
        let d = corne.drawing.expect("the Corne's profile has a drawing");
        let left = d.keys(false);
        let right = d.keys(true);
        assert_eq!(
            (left.len(), right.len()),
            (21, 21),
            "3 rows of 6 and 3 thumbs a half"
        );
        let (w, h) = d.size();
        for k in left.iter().chain(&right) {
            assert!(
                k.min.x >= 0.0 && k.min.y >= 0.0 && k.max.x <= w && k.max.y <= h,
                "{k:?} inside {w}x{h}"
            );
        }
        // The column stagger: the middle finger's column sits higher than the pinky's.
        assert!(left[3 * 3].min.y < left[0].min.y);
        // The thumbs sit under the grid, on the inner side (right of the left half).
        let thumbs = &left[18..];
        assert!(thumbs.iter().all(|t| t.min.y >= 3.0 && t.min.x >= 3.0));
        // The right half is the left one mirrored.
        for (l, r) in left.iter().zip(&right) {
            assert_eq!((r.min.y, r.min.x), (l.min.y, w - l.max.x));
        }
        // A mistyped drawing stays within bounds.
        let wild = Drawing {
            columns: 200,
            rows: 0,
            thumbs: 50,
            stagger: vec![f32::NAN, 99.0],
            keys: vec![],
        };
        assert_eq!(wild.keys(false).len(), 24 + 24);
    }

    /// #559: no profile (or no drawing) is a plain grid of the keymap's keys, not a Corne.
    #[test]
    fn a_board_without_a_drawing_is_a_plain_grid_of_its_keys() {
        for n in [36, 42, 61, 104] {
            let d = Drawing::plain(n);
            let keys = d.keys(false);
            assert_eq!(keys.len(), n);
            let (w, h) = d.size();
            assert!(w > h, "{n} keys: wider than tall ({w}x{h})");
            assert!(keys.iter().all(|k| k.max.x <= w && k.max.y <= h));
            assert_eq!(d.grid(), (n, 1, 0));
        }
        assert_eq!(Drawing::default().keys(false).len(), PLAIN_KEYS);
        assert_ne!(Drawing::default(), corne_drawing(), "not a Corne");
        // A drawing that lists its keys, as KLE does: widths and places kept, wild ones bounded.
        let listed: Drawing =
            toml::from_str("keys = [[0, 0], [1, 0, 1.5], [0, 1, 2.75], [999, -4, nan]]").unwrap();
        let keys = listed.keys(false);
        assert_eq!(keys.len(), 4);
        assert_eq!(keys[1].width(), 1.5);
        assert_eq!(keys[3].min, egui::pos2(40.0, 0.0));
        assert_eq!(listed.size(), (41.0, 2.0));
    }

    fn corne_drawing() -> Drawing {
        let all = load(None);
        find_in(&all, 0x1d50, 0x615e, "Eyelash Corne")
            .unwrap()
            .drawing
            .unwrap()
    }

    /// #559: a profile from before `[[parts]]` reads as it did; `[[parts]]` names them.
    #[test]
    fn old_profiles_keep_two_halves_and_parts_set_them() {
        let old = "format = \"foxglove-board\"\nversion = 1\nid = \"x\"\nname = \"X\"\nprotocol = \"zmk\"\n[[match]]\nvid = 1\npid = 2\n";
        let b = Board::parse(old).unwrap();
        assert_eq!((b.halves, b.parts.len()), (2, 0));
        let names: Vec<String> = (0..2).map(|i| b.parts().name(i)).collect();
        assert_eq!(names, ["Left half", "Right half"]);
        let with = format!(
            "{old}[[parts]]\nid = \"main\"\nname = \"Main board\"\n[[parts]]\nid = \"pad\"\nname = \"Numpad\"\n[[parts]]\nid = \"dial\"\nname = \"Dial\"\n"
        );
        let b = Board::parse(&with).unwrap();
        assert_eq!(b.halves, 2, "the link reaches two parts");
        assert_eq!(b.parts().len(), 3);
        assert_eq!(b.parts().name(1), "Numpad");
        // A split without a drawing: each half a plain grid of half the keymap's keys.
        let split = Board::parse(old).unwrap();
        assert_eq!(plain_or_profile(Some(&split), 42).keys(false).len(), 21);
        assert_eq!(plain_or_profile(None, 42).keys(false).len(), 42);
        assert_eq!(b.parts().the(0), "the main board");
    }

    #[test]
    fn a_user_profile_adds_a_board_or_replaces_a_built_in_one() {
        let dir = std::env::temp_dir().join(format!("foxglove-boards-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mine.toml"),
            "format = \"foxglove-board\"\nversion = 1\nid = \"eyelash-corne\"\nname = \"My Corne\"\nprotocol = \"zmk\"\nkeymap = \"mine\"\n[[match]]\nvid = 0x1d50\npid = 0x615e\nproduct = \"Eyelash Corne\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("mine.keymap"),
            "/ { keymap { compatible = \"zmk,keymap\"; }; };",
        )
        .unwrap();
        std::fs::write(dir.join("broken.toml"), "id = 3").unwrap();
        let all = load(Some(&dir));
        let corne = find_in(&all, 0x1d50, 0x615e, "Eyelash Corne").unwrap();
        assert_eq!(
            (corne.name.as_str(), corne.status),
            ("My Corne", Status::Untested)
        );
        assert!(
            corne.keymap_source().unwrap().contains("zmk,keymap"),
            "its keymap file next to it"
        );
        assert_eq!(
            all.iter().filter(|b| b.id == "eyelash-corne").count(),
            1,
            "replaced, not doubled"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
