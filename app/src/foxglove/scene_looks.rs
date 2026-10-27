//! What the Scenes page (#487) chooses: a [`Look`] (one of the six effects and its colour or
//! palette, speed, direction and width) and the presets that name looks. Pure and saved:
//! - the chosen look in the store's `[scenes]` section, which a profile overrides (it is in
//!   `store::PROFILED`), so each profile keeps its own scene;
//! - the user's saved presets in `[scene_presets]`, the same in every profile.
//!
//! The built-in presets are [`built_in`]: first the looks matched to the twelve Bandit
//! wallpapers (#488, [`wallpaper_presets`]), then one per effect. The wallpapers' looks, their
//! palettes and their right-screen art come from `app/resources/scenes.json` (format
//! `foxglove-scenes` v1). Each works with no wallpaper and no other app: a look is only an
//! effect and its colours. With Wallpaper Engine showing that wallpaper, the scene's clock
//! reaches it through wallpaper sync (#493) when the scene starts or its look changes.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use foxglove_scene::{DeviceKind, Effect, Point, Rgb, RoomMap};

use super::art_cycle::Item;

/// The store section with the chosen look (profiled).
pub const SECTION: &str = "scenes";
/// The store section with the user's saved presets (not profiled).
pub const SAVED_SECTION: &str = "scene_presets";

/// The six effects, as the page names them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Scanline,
    Sweep,
    Ripple,
    Breathe,
    Gradient,
    /// A drifting noise field (the crate's `Noise`).
    Drift,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Scanline,
        Kind::Sweep,
        Kind::Ripple,
        Kind::Breathe,
        Kind::Gradient,
        Kind::Drift,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Scanline => "Scanline",
            Kind::Sweep => "Sweep",
            Kind::Ripple => "Ripple",
            Kind::Breathe => "Breathe",
            Kind::Gradient => "Gradient",
            Kind::Drift => "Drift",
        }
    }

    /// One line on what it does, for the effect pills' hover.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Scanline => "A band of colour crosses the room",
            Kind::Sweep => "A palette scrolls across the room",
            Kind::Ripple => "Rings of colour grow from the keyboard",
            Kind::Breathe => "Every light fades in and out together",
            Kind::Gradient => "A still palette across the room",
            Kind::Drift => "Soft patches of a palette drift through the room",
        }
    }

    /// Takes one colour (and a background); the others take a palette.
    pub fn uses_colour(self) -> bool {
        matches!(self, Kind::Scanline | Kind::Ripple | Kind::Breathe)
    }

    pub fn uses_speed(self) -> bool {
        self != Kind::Gradient
    }

    pub fn uses_direction(self) -> bool {
        matches!(self, Kind::Scanline | Kind::Sweep | Kind::Gradient)
    }

    pub fn uses_width(self) -> bool {
        matches!(self, Kind::Scanline | Kind::Ripple | Kind::Drift)
    }

    /// What the width slider is called for this effect.
    pub fn width_label(self) -> &'static str {
        if self == Kind::Drift {
            "Size"
        } else {
            "Width"
        }
    }
}

/// Which way a scanline, sweep or gradient runs, as the user sees the room.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Top to bottom: from the monitors down to the keys.
    #[default]
    Down,
    Up,
    Right,
    Left,
}

impl Direction {
    pub const ALL: [Direction; 4] = [
        Direction::Down,
        Direction::Up,
        Direction::Right,
        Direction::Left,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Direction::Down => "Down",
            Direction::Up => "Up",
            Direction::Right => "Right",
            Direction::Left => "Left",
        }
    }

    /// The crate's angle: 0 runs left to right, 90 top to bottom.
    pub fn angle_deg(self) -> f32 {
        match self {
            Direction::Right => 0.0,
            Direction::Down => 90.0,
            Direction::Left => 180.0,
            Direction::Up => 270.0,
        }
    }
}

/// What a one-colour effect shows away from its colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    /// A dim blue, so the room never goes fully dark.
    #[default]
    Night,
    Off,
}

impl Background {
    pub const ALL: [Background; 2] = [Background::Night, Background::Off];

    pub fn label(self) -> &'static str {
        match self {
            Background::Night => "Night",
            Background::Off => "Off",
        }
    }

    pub fn rgb(self) -> Rgb {
        match self {
            Background::Night => Rgb([6, 8, 18]),
            Background::Off => Rgb::BLACK,
        }
    }
}

/// The palettes, by name: (name, stops in sRGB).
pub const PALETTES: [(&str, &[[u8; 3]]); 5] = [
    (
        "Bandit",
        &[[0xf0, 0x8a, 0x3c], [0xff, 0x5f, 0x3c], [6, 8, 18]],
    ),
    (
        "Forest",
        &[[0x5f, 0xc4, 0x8e], [0x1c, 0x4a, 0x33], [0xf0, 0xd2, 0x3c]],
    ),
    (
        "Steel",
        &[[0x7f, 0xb0, 0xe0], [0x14, 0x22, 0x3c], [0xe6, 0xe6, 0xe6]],
    ),
    (
        "Ember",
        &[[0xff, 0x5f, 0x3c], [0xf0, 0xd2, 0x3c], [0x5a, 0x10, 0x08]],
    ),
    (
        "Spectrum",
        &[
            [255, 40, 40],
            [255, 220, 40],
            [40, 220, 80],
            [40, 200, 255],
            [80, 60, 255],
            [230, 40, 220],
        ],
    ),
];

/// A palette's stops: one of [`PALETTES`] or a wallpaper's (`scenes.json`). An unknown name (a
/// palette since removed) is the first.
pub fn palette(name: &str) -> Vec<Rgb> {
    if let Some((_, stops)) = PALETTES.iter().find(|(n, _)| *n == name) {
        return stops.iter().map(|c| Rgb(*c)).collect();
    }
    if let Some(p) = scenes().palettes.iter().find(|p| p.name == name) {
        return p.stops.clone();
    }
    PALETTES[0].1.iter().map(|c| Rgb(*c)).collect()
}

/// The palette is one Foxglove knows (its own or a wallpaper's).
pub fn known_palette(name: &str) -> bool {
    PALETTES.iter().any(|(n, _)| *n == name) || scenes().palettes.iter().any(|p| p.name == name)
}

/// Speed 1 (slowest) to 10: one cycle's length, in milliseconds. Speed 5 is the demo's 4 s.
const PERIODS: [u32; 10] = [
    12_000, 9_000, 7_000, 5_500, 4_000, 3_000, 2_200, 1_600, 1_200, 1_000,
];

pub const SPEEDS: std::ops::RangeInclusive<u8> = 1..=10;
/// The width (and Drift's size), in millimetres.
pub const WIDTHS: std::ops::RangeInclusive<u16> = 20..=400;

pub fn period_ms(speed: u8) -> u32 {
    PERIODS[usize::from(speed.clamp(1, 10) - 1)]
}

/// One look: an effect and every parameter the page offers. Parameters the effect doesn't use
/// are kept, so switching effects and back keeps them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub kind: Kind,
    pub colour: [u8; 3],
    pub background: Background,
    pub palette: String,
    pub speed: u8,
    pub direction: Direction,
    pub width_mm: u16,
}

impl Default for Look {
    /// The demo's scanline (#485): Bandit orange, down the room, over night blue.
    fn default() -> Self {
        Look {
            kind: Kind::Scanline,
            colour: [0xf0, 0x8a, 0x3c],
            background: Background::Night,
            palette: "Bandit".into(),
            speed: 5,
            direction: Direction::Down,
            width_mm: 80,
        }
    }
}

/// Where a ripple starts: the keyboard's middle, else the room's.
fn ripple_origin(room: &RoomMap) -> Point {
    let keys = room
        .devices
        .iter()
        .filter(|d| {
            matches!(
                d.kind,
                DeviceKind::Keyboard | DeviceKind::KeyboardLeft | DeviceKind::KeyboardRight
            )
        })
        .map(|d| d.rect)
        .reduce(|a, b| a.union(&b));
    keys.unwrap_or(room.bounds).centre()
}

impl Look {
    /// The parameters within the page's ranges (a hand-edited file may hold anything).
    pub fn clamped(mut self) -> Look {
        self.speed = self.speed.clamp(*SPEEDS.start(), *SPEEDS.end());
        self.width_mm = self.width_mm.clamp(*WIDTHS.start(), *WIDTHS.end());
        if !known_palette(&self.palette) {
            self.palette = PALETTES[0].0.into();
        }
        self
    }

    /// The effect this look runs, over `room` (a ripple starts at its keyboard).
    pub fn effect(&self, room: &RoomMap) -> Effect {
        let l = self.clone().clamped();
        let colour = Rgb(l.colour);
        let background = l.background.rgb();
        let period_ms = period_ms(l.speed);
        let width_mm = f32::from(l.width_mm);
        let angle_deg = l.direction.angle_deg();
        let stops = palette(&l.palette);
        match l.kind {
            Kind::Scanline => Effect::Scanline {
                colour,
                background,
                angle_deg,
                width_mm,
                period_ms,
            },
            Kind::Sweep => Effect::Sweep {
                stops,
                angle_deg,
                period_ms,
            },
            Kind::Ripple => Effect::Ripple {
                origin: ripple_origin(room),
                colour,
                background,
                width_mm,
                period_ms,
            },
            Kind::Breathe => Effect::Breathe {
                from: background,
                to: colour,
                period_ms,
            },
            Kind::Gradient => Effect::Gradient { stops, angle_deg },
            Kind::Drift => Effect::Noise {
                stops,
                scale_mm: width_mm,
                // Drift is slow at heart: one feature's worth takes four cycles.
                period_ms: period_ms.saturating_mul(4),
                seed: 7,
            },
        }
    }
}

/// A named look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub look: Look,
}

fn preset(name: &str, look: Look) -> Preset {
    Preset {
        name: name.into(),
        look,
    }
}

/// The presets that ship with Foxglove, the wallpapers' first (#488).
pub fn built_in() -> Vec<Preset> {
    let swatch = |name: &str| {
        super::lights_page::SWATCHES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or([255, 255, 255], |(_, c)| *c)
    };
    let mut out = wallpaper_presets();
    out.extend([
        preset("Bandit scanline", Look::default()),
        preset(
            "Ripple from the keys",
            Look {
                kind: Kind::Ripple,
                speed: 6,
                width_mm: 60,
                ..Look::default()
            },
        ),
        preset(
            "Ember breathe",
            Look {
                kind: Kind::Breathe,
                colour: swatch("Ember"),
                background: Background::Off,
                speed: 3,
                ..Look::default()
            },
        ),
        preset(
            "Steel sweep",
            Look {
                kind: Kind::Sweep,
                palette: "Steel".into(),
                speed: 4,
                direction: Direction::Right,
                ..Look::default()
            },
        ),
        preset(
            "Forest drift",
            Look {
                kind: Kind::Drift,
                palette: "Forest".into(),
                speed: 3,
                width_mm: 250,
                ..Look::default()
            },
        ),
        preset(
            "Spectrum gradient",
            Look {
                kind: Kind::Gradient,
                palette: "Spectrum".into(),
                direction: Direction::Right,
                ..Look::default()
            },
        ),
    ]);
    out
}

/// The presets made for each Bandit wallpaper (#488), in the file's order.
pub fn wallpaper_presets() -> Vec<Preset> {
    scenes().looks.iter().map(|w| w.preset.clone()).collect()
}

/// The wallpaper look this is, if it is one.
pub fn wallpaper_look(look: &Look) -> Option<&'static WallpaperLook> {
    scenes().looks.iter().find(|w| w.preset.look == *look)
}

/// The built-in scenes file: the Bandit wallpapers' palettes and looks.
const SCENES_JSON: &str = include_str!("../../resources/scenes.json");
pub const SCENES_FORMAT: &str = "foxglove-scenes";
pub const SCENES_VERSION: u32 = 1;

/// A wallpaper's right-screen art, tried in this order: the keyboard's built-in set, the
/// wallpaper's own file in the art folder (bandit-wallpapers installs it there), the copy that
/// ships with Foxglove, then another built-in set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Art {
    pub builtin: Option<String>,
    pub library: Option<String>,
    pub bundled: Option<String>,
    pub fallback: Option<String>,
}

impl Art {
    /// The art to show. `library` finds a file in the art folder; `bundled` copies a set that
    /// ships with Foxglove into it.
    pub fn item(
        &self,
        library: impl Fn(&str) -> Option<PathBuf>,
        bundled: impl Fn(&str) -> Option<PathBuf>,
    ) -> Option<Item> {
        if let Some(b) = &self.builtin {
            return Some(Item::BuiltIn(b.clone()));
        }
        self.library
            .as_deref()
            .and_then(library)
            .or_else(|| self.bundled.as_deref().and_then(bundled))
            .map(Item::File)
            .or_else(|| self.fallback.clone().map(Item::BuiltIn))
    }
}

/// One Bandit wallpaper's look.
#[derive(Clone, Debug, PartialEq)]
pub struct WallpaperLook {
    /// The wallpaper's id in bandit-wallpapers (its folder name, as `wallpapers.json` has it).
    pub wallpaper: String,
    /// What the look takes from the wallpaper, for the preset's hover.
    pub motif: String,
    pub preset: Preset,
    pub art: Art,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NamedPalette {
    pub name: String,
    pub stops: Vec<Rgb>,
}

/// `scenes.json`, read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scenes {
    pub palettes: Vec<NamedPalette>,
    pub looks: Vec<WallpaperLook>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenesFile {
    format: String,
    version: u32,
    #[serde(default, rename = "about")]
    _about: String,
    palettes: Vec<FilePalette>,
    presets: Vec<FilePreset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePalette {
    name: String,
    stops: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePreset {
    wallpaper: String,
    name: String,
    motif: String,
    look: FileLook,
    #[serde(default)]
    art: Art,
}

/// A look as the file writes it: colours in hex, and only what the effect uses.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLook {
    kind: Kind,
    colour: Option<String>,
    background: Option<Background>,
    palette: Option<String>,
    speed: Option<u8>,
    direction: Option<Direction>,
    width_mm: Option<u16>,
}

fn hex(s: &str) -> Result<[u8; 3], String> {
    let h = s
        .strip_prefix('#')
        .filter(|h| h.len() == 6 && h.is_ascii())
        .ok_or_else(|| format!("{s:?} is not #rrggbb"))?;
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|e| format!("{s:?}: {e}"));
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

/// Reads a scenes file: every colour, palette, range and name checked.
pub fn parse_scenes(text: &str) -> Result<Scenes, String> {
    let file: ScenesFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if file.format != SCENES_FORMAT || file.version != SCENES_VERSION {
        return Err(format!("not {SCENES_FORMAT} v{SCENES_VERSION}"));
    }
    let mut palettes: Vec<NamedPalette> = Vec::new();
    for p in file.palettes {
        if p.stops.len() < 2 {
            return Err(format!("palette {} needs two colours", p.name));
        }
        if PALETTES.iter().any(|(n, _)| *n == p.name) || palettes.iter().any(|q| q.name == p.name) {
            return Err(format!("palette {} is named twice", p.name));
        }
        let stops = p
            .stops
            .iter()
            .map(|s| hex(s).map(Rgb))
            .collect::<Result<_, _>>()?;
        palettes.push(NamedPalette {
            name: p.name,
            stops,
        });
    }
    let mut looks: Vec<WallpaperLook> = Vec::new();
    for p in file.presets {
        let name = p.name.trim().to_string();
        if name.is_empty() || name.chars().count() > NAME_MAX {
            return Err(format!("preset name {:?}", p.name));
        }
        if looks
            .iter()
            .any(|w| w.preset.name.eq_ignore_ascii_case(&name) || w.wallpaper == p.wallpaper)
        {
            return Err(format!("{name} ({}) is there twice", p.wallpaper));
        }
        let f = p.look;
        let d = Look::default();
        let look = Look {
            kind: f.kind,
            colour: f
                .colour
                .as_deref()
                .map(hex)
                .transpose()?
                .unwrap_or(d.colour),
            background: f.background.unwrap_or(d.background),
            palette: f.palette.unwrap_or(d.palette),
            speed: f.speed.unwrap_or(d.speed),
            direction: f.direction.unwrap_or(d.direction),
            width_mm: f.width_mm.unwrap_or(d.width_mm),
        };
        let palette_known = PALETTES.iter().any(|(n, _)| *n == look.palette)
            || palettes.iter().any(|q| q.name == look.palette);
        if !palette_known || !SPEEDS.contains(&look.speed) || !WIDTHS.contains(&look.width_mm) {
            return Err(format!("{name}: palette, speed or width out of range"));
        }
        looks.push(WallpaperLook {
            wallpaper: p.wallpaper,
            motif: p.motif,
            preset: Preset { name, look },
            art: p.art,
        });
    }
    Ok(Scenes { palettes, looks })
}

/// The scenes file that ships with Foxglove, read once (a test checks it reads).
pub fn scenes() -> &'static Scenes {
    static SCENES: OnceLock<Scenes> = OnceLock::new();
    SCENES.get_or_init(|| {
        parse_scenes(SCENES_JSON).unwrap_or_else(|e| {
            eprintln!("Foxglove: the built-in scenes file doesn't read: {e}");
            Scenes::default()
        })
    })
}

/// The `[scenes]` section: the chosen look.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chosen {
    pub look: Look,
    /// A wallpaper's look also shows that wallpaper's right-screen art while it runs.
    pub with_art: bool,
}

impl Chosen {
    /// The right-screen art a running scene of this look shows, if any.
    pub fn art(&self) -> Option<&'static Art> {
        self.with_art
            .then(|| wallpaper_look(&self.look))
            .flatten()
            .map(|w| &w.art)
    }
}

/// The `[scene_presets]` section: the user's own presets, in the order saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub presets: Vec<Preset>,
}

/// The longest preset name the page takes.
pub const NAME_MAX: usize = 40;

/// Why a name can't be saved, or `None` when it can.
pub fn name_problem(name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Type a name first");
    }
    if name.chars().count() > NAME_MAX {
        return Some("Keep the name to 40 letters");
    }
    if built_in().iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
        return Some("A built-in preset has that name");
    }
    None
}

impl Saved {
    /// Adds a preset, or replaces the one with that name. False when the name isn't allowed.
    pub fn save(&mut self, name: &str, look: &Look) -> bool {
        if name_problem(name).is_some() {
            return false;
        }
        let name = name.trim();
        let p = preset(name, look.clone());
        match self
            .presets
            .iter_mut()
            .find(|x| x.name.eq_ignore_ascii_case(name))
        {
            Some(old) => *old = p,
            None => self.presets.push(p),
        }
        true
    }

    pub fn remove(&mut self, name: &str) {
        self.presets.retain(|p| p.name != name);
    }
}

/// The preset this look is, if any: built-in first, then the user's.
pub fn named<'a>(look: &Look, built_in: &'a [Preset], saved: &'a [Preset]) -> Option<&'a str> {
    built_in
        .iter()
        .chain(saved)
        .find(|p| p.look == *look)
        .map(|p| p.name.as_str())
}

pub fn load_chosen() -> Chosen {
    let mut c: Chosen = super::store::try_load(SECTION).unwrap_or_default();
    c.look = c.look.clamped();
    c
}

pub fn load_saved() -> Saved {
    let mut s: Saved = super::store::try_load(SAVED_SECTION).unwrap_or_default();
    for p in &mut s.presets {
        p.look = p.look.clone().clamped();
    }
    s
}

pub fn save_chosen(c: &Chosen) {
    if let Err(e) = super::store::save(SECTION, c) {
        eprintln!("Foxglove: can't save the scene: {e}");
    }
}

pub fn save_saved(s: &Saved) {
    if let Err(e) = super::store::save(SAVED_SECTION, s) {
        eprintln!("Foxglove: can't save the scene presets: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_scene::{DeviceInfo, Layout, SceneTime};

    #[test]
    fn the_wallpapers_come_first_then_the_demo_scene() {
        let room = RoomMap::default();
        let all = built_in();
        let n = wallpaper_presets().len();
        assert_eq!(all[..n], wallpaper_presets()[..]);
        assert_eq!(all[n].name, "Bandit scanline");
        assert_eq!(
            all[n].look.effect(&room),
            super::super::scene_run::demo_effect()
        );
    }

    /// The twelve wallpapers in bandit-wallpapers (`dist/keyboard/manifest.json`).
    const WALLPAPERS: [&str; 12] = [
        "hud",
        "bandit-radar",
        "bandit-skyline",
        "courier",
        "fury-print",
        "fury-chase",
        "fury-pattern",
        "fury-stars",
        "fury-forest",
        "hearth-cauldron",
        "hearth-moongarden",
        "hearth-wheel",
    ];

    #[test]
    fn the_scenes_file_has_a_look_for_every_bandit_wallpaper() {
        let s = parse_scenes(SCENES_JSON).expect("scenes.json reads");
        assert_eq!(&s, scenes());
        let ids: Vec<&str> = s.looks.iter().map(|w| w.wallpaper.as_str()).collect();
        assert_eq!(ids, WALLPAPERS);
        let room = RoomMap::default();
        let bundled: Vec<&str> = super::super::art_pieces::BUNDLED
            .iter()
            .map(|b| b.0)
            .collect();
        for w in &s.looks {
            let look = &w.preset.look;
            assert_eq!(&look.clone().clamped(), look, "{}: in range", w.wallpaper);
            assert!(known_palette(&look.palette));
            assert!(!w.motif.is_empty());
            // Every look runs on its own: with no room, no wallpaper and no other app.
            let effect = look.effect(&room);
            let _ = effect.colour(Point::new(10.0, 10.0), SceneTime(1234), &room.bounds);
            // Its art: a built-in set, or the wallpaper's file with a bundled copy and a set
            // to fall back to.
            let a = &w.art;
            assert!(a.library.as_deref() == Some(&format!("{}.bin", w.wallpaper)));
            assert!(
                a.builtin.is_some() || (a.bundled.is_some() && a.fallback.is_some()),
                "{}",
                w.wallpaper
            );
            if let Some(b) = &a.bundled {
                assert!(bundled.contains(&b.as_str()), "{b} ships with Foxglove");
            }
        }
        // The wallpapers' palettes are their own, and each is used.
        for p in &s.palettes {
            assert!(s.looks.iter().any(|w| w.preset.look.palette == p.name));
            assert_eq!(palette(&p.name), p.stops);
        }
        let names: Vec<_> = built_in()
            .into_iter()
            .map(|p| p.name.to_lowercase())
            .collect();
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(names.len(), unique.len(), "no two built-ins share a name");
    }

    #[test]
    fn a_bad_scenes_file_is_refused() {
        let ok = r##"{"format":"foxglove-scenes","version":1,"palettes":[{"name":"P","stops":["#000000","#ffffff"]}],
            "presets":[{"wallpaper":"w","name":"W","motif":"m","look":{"kind":"sweep","palette":"P"}}]}"##;
        let s = parse_scenes(ok).unwrap();
        assert_eq!(s.looks[0].preset.look.palette, "P");
        assert_eq!(s.looks[0].preset.look.speed, Look::default().speed);
        for (bad, why) in [
            (ok.replace("version\":1", "version\":2"), "version"),
            (ok.replace("#ffffff", "#fffff"), "hex"),
            (
                ok.replace("\"palette\":\"P\"", "\"palette\":\"Q\""),
                "palette",
            ),
            (
                ok.replace("\"kind\":\"sweep\"", "\"kind\":\"sweep\",\"speed\":11"),
                "speed",
            ),
            (
                ok.replace("\"name\":\"P\"", "\"name\":\"Forest\""),
                "a palette name taken",
            ),
            (ok.replace("\"motif\"", "\"motive\""), "an unknown field"),
        ] {
            assert!(parse_scenes(&bad).is_err(), "{why}");
        }
    }

    #[test]
    fn wallpaper_art_is_tried_in_order() {
        let none = |_: &str| None;
        let file = |n: &str| Some(PathBuf::from(format!("art/{n}")));
        let built = Art {
            builtin: Some("Bandit_Idle".into()),
            library: Some("hud.bin".into()),
            ..Art::default()
        };
        assert_eq!(
            built.item(file, file),
            Some(Item::BuiltIn("Bandit_Idle".into()))
        );
        let fury = Art {
            builtin: None,
            library: Some("fury-chase.bin".into()),
            bundled: Some("fury.bin".into()),
            fallback: Some("Bandit_Emblem".into()),
        };
        assert_eq!(
            fury.item(file, file),
            Some(Item::File("art/fury-chase.bin".into())),
            "the wallpaper's own art when bandit-wallpapers installed it"
        );
        assert_eq!(
            fury.item(none, file),
            Some(Item::File("art/fury.bin".into())),
            "else the bundled copy"
        );
        assert_eq!(
            fury.item(none, none),
            Some(Item::BuiltIn("Bandit_Emblem".into()))
        );
        assert_eq!(Art::default().item(file, file), None);
    }

    #[test]
    fn only_a_wallpaper_look_with_art_on_brings_art() {
        let hud = wallpaper_presets()[0].clone();
        let mut c = Chosen {
            look: hud.look.clone(),
            with_art: false,
        };
        assert_eq!(c.art(), None, "off by default");
        c.with_art = true;
        assert_eq!(c.art(), Some(&scenes().looks[0].art));
        assert_eq!(wallpaper_look(&hud.look).unwrap().wallpaper, "hud");
        c.look.speed = 9;
        assert_eq!(c.art(), None, "an edited look is the user's own");
    }

    #[test]
    fn every_effect_has_a_look_and_presets_cover_all_six() {
        let room = RoomMap::default();
        let kinds: Vec<Kind> = built_in().iter().map(|p| p.look.kind).collect();
        for k in Kind::ALL {
            assert!(kinds.contains(&k), "{k:?} has no preset");
            let look = Look {
                kind: k,
                ..Look::default()
            };
            let name = format!("{:?}", look.effect(&room));
            let want = match k {
                Kind::Drift => "Noise".to_string(),
                k => format!("{k:?}"),
            };
            assert!(name.starts_with(&want), "{k:?}: {name}");
        }
        let names: Vec<_> = built_in().into_iter().map(|p| p.name).collect();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique);
    }

    #[test]
    fn speed_direction_and_width_reach_the_effect() {
        let room = RoomMap::default();
        let look = Look {
            speed: 10,
            direction: Direction::Left,
            width_mm: 200,
            ..Look::default()
        };
        match look.effect(&room) {
            Effect::Scanline {
                angle_deg,
                width_mm,
                period_ms,
                ..
            } => assert_eq!((angle_deg, width_mm, period_ms), (180.0, 200.0, 1000)),
            e => panic!("{e:?}"),
        }
        assert_eq!(period_ms(1), 12_000);
        assert_eq!(period_ms(0), 12_000, "clamped");
        assert_eq!(period_ms(99), 1_000, "clamped");
        assert!(PERIODS.windows(2).all(|w| w[0] > w[1]), "faster each step");
    }

    #[test]
    fn a_ripple_starts_at_the_keyboard() {
        let devices = [
            DeviceInfo::whole("keyboard:left", "L", DeviceKind::KeyboardLeft),
            DeviceInfo::whole("keyboard:right", "R", DeviceKind::KeyboardRight),
        ];
        let room = RoomMap::build(&[], &devices, &Layout::default());
        let keys = room.devices[0].rect.union(&room.devices[1].rect).centre();
        let look = Look {
            kind: Kind::Ripple,
            ..Look::default()
        };
        match look.effect(&room) {
            Effect::Ripple { origin, .. } => assert_eq!(origin, keys),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn a_hand_edited_look_is_brought_into_range() {
        let look: Look =
            toml::from_str("kind = \"drift\"\nspeed = 0\nwidth_mm = 9000\npalette = \"Gone\"")
                .unwrap();
        let l = look.clamped();
        assert_eq!((l.kind, l.speed, l.width_mm), (Kind::Drift, 1, 400));
        assert_eq!(l.palette, "Bandit");
        assert_eq!(palette("Gone"), palette("Bandit"));
        let unknown: Result<Look, _> = toml::from_str("kind = \"laser\"");
        assert!(
            unknown.is_err(),
            "an unknown effect fails, and try_load falls back"
        );
    }

    #[test]
    fn saving_presets_replaces_by_name_and_refuses_bad_names() {
        let mut s = Saved::default();
        let a = Look::default();
        let b = Look {
            speed: 9,
            ..Look::default()
        };
        assert!(s.save("  Desk  ", &a));
        assert!(s.save("desk", &b), "the same name, any case, replaces");
        assert_eq!(s.presets.len(), 1);
        assert_eq!(
            (s.presets[0].name.as_str(), s.presets[0].look.speed),
            ("desk", 9),
            "the newest spelling"
        );
        assert!(!s.save("", &a));
        assert!(!s.save("Bandit scanline", &a), "built-in names are taken");
        assert!(!s.save(&"x".repeat(41), &a));
        assert_eq!(named(&b, &built_in(), &s.presets), Some("desk"));
        assert_eq!(named(&a, &built_in(), &s.presets), Some("Bandit scanline"));
        s.remove("desk");
        assert!(s.presets.is_empty());
    }

    /// One test for both sections: the store tests share one settings file. Which of them a
    /// profile overrides is `store::PROFILED`'s (its own tests cover the override itself).
    #[test]
    fn the_scene_is_profiled_and_saved_presets_are_not() {
        use super::super::store;
        let _dir = store::test_config_dir();
        assert!(store::PROFILED.contains(&SECTION));
        assert!(!store::PROFILED.contains(&SAVED_SECTION));
        let chosen = Chosen {
            look: Look {
                kind: Kind::Sweep,
                palette: "Night sky".into(),
                ..Look::default()
            },
            with_art: true,
        };
        save_chosen(&chosen);
        let mut saved = Saved::default();
        saved.save("Mine", &Look::default());
        save_saved(&saved);
        // A restart reads the file again.
        assert_eq!(load_chosen(), chosen);
        assert_eq!(load_saved(), saved);
    }
}
