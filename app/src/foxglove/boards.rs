//! Board profiles (#75): what Foxglove knows about a keyboard beyond what its firmware says,
//! as data. A profile names the board, how to recognise it (USB ids and product name; ZMK
//! boards share one vid:pid), its protocol and halves, the screens to expect (the firmware's
//! own descriptors win), its lights, and the keymap source behind the layer-access hints.
//!
//! Built-in profiles ship in `resources/boards`; more go in the `boards` folder next to
//! Foxglove's settings, which wins over a built-in one with the same id. Profiles are read when
//! a keyboard connects, never on a timer.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    pub half: String,
    pub width: u16,
    pub height: u16,
    /// `1bit`; other formats aren't drawn yet.
    pub format: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Lighting {
    pub lamps: u16,
    #[serde(default)]
    pub layout: String,
}

/// How the Keyboard page draws one half of the board (#392), in key units: a grid of columns
/// with each column dropped by its stagger, and a row of thumb keys under the inner columns.
/// The left half is described; the right one is its mirror image.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Drawing {
    pub columns: u8,
    pub rows: u8,
    #[serde(default)]
    pub thumbs: u8,
    /// How far each column sits below the highest one, outer column first; missing ones are 0.
    #[serde(default)]
    pub stagger: Vec<f32>,
}

impl Default for Drawing {
    /// A Corne-shaped half, for a keyboard without a profile (or a profile without a drawing).
    fn default() -> Self {
        Drawing {
            columns: 6,
            rows: 3,
            thumbs: 3,
            stagger: vec![0.5, 0.5, 0.25, 0.0, 0.125, 0.25],
        }
    }
}

impl Drawing {
    /// Within reason, so a mistyped profile can't draw a wall of keys.
    fn bounded(&self) -> (u8, u8, u8) {
        (
            self.columns.clamp(1, 10),
            self.rows.clamp(1, 8),
            self.thumbs.min(self.columns.clamp(1, 10)),
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

    /// One half's keys, each a 1×1 rectangle in key units from the half's top-left corner.
    pub fn keys(&self, right: bool) -> Vec<egui::Rect> {
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
        if right {
            let (w, _) = self.size();
            for k in &mut keys {
                *k = egui::Rect::from_min_size(egui::pos2(w - k.max.x, k.min.y), k.size());
            }
        }
        keys
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

    /// The half's width and height in key units.
    pub fn size(&self) -> (f32, f32) {
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
    #[serde(default = "two")]
    pub halves: u8,
    #[serde(default)]
    pub keymap: Option<String>,
    #[serde(rename = "match")]
    pub matches: Vec<Match>,
    #[serde(default)]
    pub screens: Vec<Screen>,
    #[serde(default)]
    pub lighting: Option<Lighting>,
    /// How the Keyboard page draws it (#392); a Corne shape when absent.
    #[serde(default)]
    pub drawing: Option<Drawing>,
    /// The folder of a user profile, for its keymap file.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

fn two() -> u8 {
    2
}

impl Board {
    pub fn parse(text: &str) -> Result<Board, String> {
        let b: Board = toml::from_str(text).map_err(|e| e.to_string())?;
        if b.format != FORMAT || b.version != 1 {
            return Err(format!("not a {FORMAT} v1 profile"));
        }
        if b.matches.is_empty() {
            return Err("a profile needs at least one [[match]]".into());
        }
        Ok(b)
    }

    fn matches(&self, vid: u16, pid: u16, product: &str) -> bool {
        self.matches.iter().any(|m| {
            (m.vid, m.pid) == (vid, pid)
                && m.product
                    .as_deref()
                    .is_none_or(|p| p.eq_ignore_ascii_case(product.trim()))
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
    find_in(&load(user_dir().as_deref()), vid, pid, product)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // ZMK's shared ids alone match nothing.
        assert!(find_in(&all, 0x1d50, 0x615e, "Some Other Board").is_none());
        assert!(find_in(&all, 0x3434, 0x0250, "Eyelash Corne").is_none());
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
        // No drawing in a profile: a Corne shape; a mistyped one stays within bounds.
        assert_eq!(Drawing::default().keys(false).len(), 21);
        let wild = Drawing {
            columns: 200,
            rows: 0,
            thumbs: 50,
            stagger: vec![f32::NAN, 99.0],
        };
        assert_eq!(wild.keys(false).len(), 10 + 10);
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
