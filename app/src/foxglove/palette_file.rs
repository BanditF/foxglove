//! The shared palette file (`foxglove-palette` v1, docs/palette.md, #40): one set of colour
//! roles for the app theme, the scenes, the lights and the wallpapers. Bandit's is bundled as
//! `resources/palettes/bandit.json`; bandit-wallpapers vendors a copy.
//!
//! Roles are stored as sRGB `#rrggbb`. A role the file leaves out is derived: `highlight` is the
//! accent 0.08 lighter in Oklab, `deep` is the accent clamped to 3:1 on `paper` (the rule of
//! `FoxgloveLive.readable` in bandit-wallpapers), `accent_light` the accent clamped to 4.5:1 on
//! `paper` (text on light grounds). `base`, `ink`, `paper` and `paper_ink` fall back to Bandit's.
//!
//! The harmony palettes (docs/palette.md, "Harmony palettes", #871) add two optional accents:
//! `accent2` and `accent3`, each with a `_light` shade derived like `accent_light` when the file
//! leaves it out. `accent3` is only read when `accent2` is set. A palette without them is
//! Bandit's single-accent look, exactly as before: a consumer falls back to `accent`
//! ([`Roles::second`], [`Roles::third`]). Every bundled palette is [`bundled`], Bandit first.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use foxglove_scene::Rgb;

use super::scene_looks::hex;

pub const FORMAT: &str = "foxglove-palette";
pub const VERSION: u32 = 1;
/// Stops a palette may have.
pub const STOPS: std::ops::RangeInclusive<usize> = 2..=8;

/// The bundled Bandit palette, as shipped.
pub const BANDIT_JSON: &str = include_str!("../../resources/palettes/bandit.json");

/// The bundled harmony palettes (docs/palette.md), in the order the pages offer them after
/// Bandit.
pub const HARMONY_JSON: [&str; 4] = [
    include_str!("../../resources/palettes/bandit-steel.json"),
    include_str!("../../resources/palettes/bandit-dusk.json"),
    include_str!("../../resources/palettes/bandit-foxglove.json"),
    include_str!("../../resources/palettes/bandit-ember.json"),
];

/// Every role, resolved: what the file gives, the rest derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles {
    /// The palette's colour: links, focus, the lights.
    pub accent: Rgb,
    /// A lighter accent: hover, glints.
    pub highlight: Rgb,
    /// A darker accent for graphics on light grounds (3:1 on `paper`).
    pub deep: Rgb,
    /// The accent for text on light grounds (4.5:1 on `paper`).
    pub accent_light: Rgb,
    /// The dark ground (the scenes' night).
    pub base: Rgb,
    /// Ink on `base`.
    pub ink: Rgb,
    /// The light ground.
    pub paper: Rgb,
    /// Ink on `paper`.
    pub paper_ink: Rgb,
    /// The second accent, for dark grounds (harmony palettes); `None` in a single-accent one.
    pub accent2: Option<Rgb>,
    /// `accent2` for text on light grounds (4.5:1 on `paper`), when `accent2` is set.
    pub accent2_light: Option<Rgb>,
    /// The third accent, used most sparingly; only with `accent2`.
    pub accent3: Option<Rgb>,
    /// `accent3` for text on light grounds (4.5:1 on `paper`), when `accent3` is set.
    pub accent3_light: Option<Rgb>,
}

// The lights' palette effects use these (#871 P2, the next PR); until then only the tests do.
#[allow(dead_code)]
impl Roles {
    /// The second accent, else `accent`: the degrade-to-accent rule (docs/palette.md).
    pub fn second(&self) -> Rgb {
        self.accent2.unwrap_or(self.accent)
    }

    /// The third accent, else the second, else `accent`.
    pub fn third(&self) -> Rgb {
        self.accent3.unwrap_or_else(|| self.second())
    }

    /// Whether the palette has a second accent (a harmony palette).
    pub fn is_harmony(&self) -> bool {
        self.accent2.is_some()
    }
}

/// The roles as written: any may be missing. Roles this reader doesn't know are skipped.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileRoles {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deep: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_light: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ink: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paper: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paper_ink: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent2_light: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent3: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent3_light: Option<String>,
}

fn role(value: &Option<String>, name: &str) -> Result<Option<Rgb>, String> {
    value
        .as_deref()
        .map(|v| hex(v).map(Rgb).map_err(|e| format!("{name}: {e}")))
        .transpose()
}

impl FileRoles {
    /// The written accent, if any.
    pub fn accent(&self) -> Result<Option<Rgb>, String> {
        role(&self.accent, "accent")
    }

    /// Every role around `accent`: the written ones, the rest derived. `fallback` gives
    /// `base`, `ink`, `paper` and `paper_ink` when they're missing; without one they're required.
    pub fn resolve(&self, accent: Rgb, fallback: Option<&Roles>) -> Result<Roles, String> {
        let ground = |value: &Option<String>, name: &str, from: fn(&Roles) -> Rgb| match (
            role(value, name)?,
            fallback,
        ) {
            (Some(c), _) => Ok(c),
            (None, Some(f)) => Ok(from(f)),
            (None, None) => Err(format!("{name} is missing")),
        };
        let base = ground(&self.base, "base", |r| r.base)?;
        let ink = ground(&self.ink, "ink", |r| r.ink)?;
        let paper = ground(&self.paper, "paper", |r| r.paper)?;
        let paper_ink = ground(&self.paper_ink, "paper_ink", |r| r.paper_ink)?;
        // An optional accent and its light shade (derived like `accent_light`).
        let extra = |value: &Option<String>, light: &Option<String>, name: &str| {
            let Some(c) = role(value, name)? else {
                return Ok::<_, String>((None, None));
            };
            let shade =
                role(light, &format!("{name}_light"))?.unwrap_or_else(|| readable(c, paper, 4.5));
            Ok((Some(c), Some(shade)))
        };
        let (accent2, accent2_light) = extra(&self.accent2, &self.accent2_light, "accent2")?;
        // `accent3` only counts with `accent2` (docs/palette.md).
        let (accent3, accent3_light) = match accent2 {
            Some(_) => extra(&self.accent3, &self.accent3_light, "accent3")?,
            None => (None, None),
        };
        Ok(Roles {
            accent,
            highlight: role(&self.highlight, "highlight")?.unwrap_or_else(|| lighter(accent)),
            deep: role(&self.deep, "deep")?.unwrap_or_else(|| readable(accent, paper, 3.0)),
            accent_light: role(&self.accent_light, "accent_light")?
                .unwrap_or_else(|| readable(accent, paper, 4.5)),
            base,
            ink,
            paper,
            paper_ink,
            accent2,
            accent2_light,
            accent3,
            accent3_light,
        })
    }
}

/// A palette file, read.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteFile {
    pub name: String,
    pub roles: Roles,
    pub stops: Vec<Rgb>,
    /// Which area budget its accents follow (docs/palette.md, "Bold tier").
    #[allow(dead_code)] // the lights' key map reads it (#871 P2)
    pub tier: Tier,
}

/// A palette's tier (the optional `tier` field): `harmony`, the default, keeps `accent2` to
/// 15 % and `accent3` to 3 %; `bold` lets them lead (35 % / 5 %) and lights more keys in them.
/// A tier this reader doesn't know reads as `harmony`, the quieter one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tier {
    #[default]
    Harmony,
    Bold,
}

impl Tier {
    fn of(label: Option<&str>) -> Tier {
        match label {
            Some("bold") => Tier::Bold,
            _ => Tier::Harmony,
        }
    }
}

#[derive(Deserialize)]
struct File {
    format: String,
    version: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    roles: FileRoles,
    #[serde(default)]
    stops: Vec<String>,
    #[serde(default)]
    tier: Option<String>,
}

/// Reads a palette file. `Ok(None)` for a later version, which this reader skips as a whole.
pub fn parse(text: &str, fallback: Option<&Roles>) -> Result<Option<PaletteFile>, String> {
    let f: File = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if f.format != FORMAT {
        return Err(format!("format {:?} isn't {FORMAT}", f.format));
    }
    if f.version != VERSION {
        return Ok(None);
    }
    if f.name.trim().is_empty() {
        return Err("the palette has no name".into());
    }
    if !STOPS.contains(&f.stops.len()) {
        return Err(format!("{} stops, not 2 to 8", f.stops.len()));
    }
    let stops = f
        .stops
        .iter()
        .map(|s| hex(s).map(Rgb).map_err(|e| format!("stops: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let accent = f.roles.accent()?.ok_or("accent is missing")?;
    Ok(Some(PaletteFile {
        name: f.name,
        roles: f.roles.resolve(accent, fallback)?,
        stops,
        tier: Tier::of(f.tier.as_deref()),
    }))
}

/// The bundled Bandit palette: the app theme's, the built-in scene palette's and the last
/// fallback for any role. Its tests keep it readable and complete.
pub fn bandit() -> &'static PaletteFile {
    static BANDIT: LazyLock<PaletteFile> = LazyLock::new(|| {
        parse(BANDIT_JSON, None)
            .ok()
            .flatten()
            .expect("the bundled bandit.json is a complete v1 palette")
    });
    &BANDIT
}

/// Every bundled palette: Bandit first (the default), then the harmony palettes. Read once, at
/// first use; a bundled file that doesn't read is left out (its test fails), never a panic.
pub fn bundled() -> &'static [PaletteFile] {
    static ALL: LazyLock<Vec<PaletteFile>> = LazyLock::new(|| {
        let mut out = vec![bandit().clone()];
        for text in HARMONY_JSON {
            match parse(text, Some(&bandit().roles)) {
                Ok(Some(p)) => out.push(p),
                Ok(None) => {}
                Err(e) => eprintln!("Foxglove: a bundled palette doesn't read: {e}"),
            }
        }
        out
    });
    &ALL
}

/// A bundled palette by its name.
#[allow(dead_code)] // the lights' palette effects (#871 P2)
pub fn by_name(name: &str) -> Option<&'static PaletteFile> {
    bundled().iter().find(|p| p.name == name)
}

/// The accent 0.08 lighter in Oklab.
fn lighter(c: Rgb) -> Rgb {
    let [l, a, b] = c.lab();
    Rgb::from_lab([(l + 0.08).min(1.0), a, b])
}

fn channel(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(c: Rgb) -> f32 {
    let [r, g, b] = c.0;
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// The WCAG contrast ratio of two colours.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn to_hsl(c: Rgb) -> [f32; 3] {
    let [r, g, b] = c.0.map(|v| v as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let (l, d) = ((max + min) / 2.0, max - min);
    if d == 0.0 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d + 6.0) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h * 60.0, s, l]
}

fn from_hsl([h, s, l]: [f32; 3]) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb([byte(r), byte(g), byte(b)])
}

/// The colour with the same hue and saturation (HSL), darkened or lightened only as far as it
/// takes to reach `min` contrast on `ground`: `FoxgloveLive.readable` in bandit-wallpapers.
pub fn readable(colour: Rgb, ground: Rgb, min: f32) -> Rgb {
    let ok = |c: Rgb| contrast(c, ground) >= min;
    if ok(colour) {
        return colour;
    }
    let [h, s, l] = to_hsl(colour);
    let pale = luminance(ground) > 0.18;
    let at = |x: f32| from_hsl([h, s, x]);
    let (mut near, mut far) = (l, if pale { 0.0 } else { 1.0 });
    if !ok(at(far)) {
        return at(far);
    }
    for _ in 0..24 {
        let mid = (near + far) / 2.0;
        if ok(at(mid)) {
            far = mid;
        } else {
            near = mid;
        }
    }
    // 8-bit rounding can land a hair under; step on until it holds.
    let mut x = far;
    let mut out = at(x);
    for _ in 0..20 {
        if ok(out) {
            break;
        }
        x = if pale {
            (x - 0.005).max(0.0)
        } else {
            (x + 0.005).min(1.0)
        };
        out = at(x);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::{scene_looks, theme};

    fn rgb(c: egui::Color32) -> Rgb {
        Rgb([c.r(), c.g(), c.b()])
    }

    #[test]
    fn the_bundled_file_is_complete() {
        let b = bandit();
        assert_eq!(b.name, "Bandit");
        let raw: serde_json::Value = serde_json::from_str(BANDIT_JSON).unwrap();
        let roles = raw["roles"].as_object().unwrap();
        for name in [
            "accent",
            "highlight",
            "accent_light",
            "base",
            "ink",
            "paper",
            "paper_ink",
        ] {
            assert!(roles.contains_key(name), "bandit.json lacks {name}");
        }
        // The app's night ground is `base`; the wallpapers' differs on purpose (docs/palette.md).
        assert_eq!(roles["app_ground"], roles["base"]);
        assert_ne!(roles["wall_ground"], roles["app_ground"]);
        // `deep` is derived (Hub, #40 Q2), and holds 3:1 on paper.
        assert!(!roles.contains_key("deep"));
        assert!(contrast(b.roles.deep, b.roles.paper) >= 3.0);
    }

    /// The theme and the scenes take their Bandit colours from the file (#40(c) step 5; step 1
    /// checked that the file held the colours they had, so the move changed nothing on screen).
    #[test]
    fn the_theme_and_the_bandit_scene_palette_equal_the_file() {
        let r = bandit().roles;
        assert_eq!(rgb(theme::DARK.accent), r.accent);
        assert_eq!(rgb(theme::DARK.accent_hover), r.highlight);
        assert_eq!(rgb(theme::LIGHT.accent), r.accent_light);
        let (name, stops) = &scene_looks::PALETTES[0];
        assert_eq!(*name, bandit().name, "saved looks name it");
        assert_eq!(
            stops.iter().map(|c| Rgb(*c)).collect::<Vec<_>>(),
            bandit().stops
        );
        assert_eq!(scene_looks::Background::Night.rgb(), r.base);
        assert_eq!(Rgb(scene_looks::Look::default().colour), r.accent);
    }

    #[test]
    fn missing_roles_are_derived_and_unknown_ones_skipped() {
        let text = r##"{ "format": "foxglove-palette", "version": 1, "name": "Reef",
            "roles": { "accent": "#0fb5ae", "glow": "#ffffff" },
            "stops": ["#0fb5ae", "#0b2545"], "later": true }"##;
        let p = parse(text, Some(&bandit().roles)).unwrap().unwrap();
        let r = p.roles;
        assert_eq!(r.accent, Rgb([0x0f, 0xb5, 0xae]));
        assert!(r.highlight.lab()[0] > r.accent.lab()[0]);
        assert!(contrast(r.deep, r.paper) >= 3.0);
        assert!(contrast(r.accent_light, r.paper) >= 4.5);
        assert_eq!(r.base, bandit().roles.base);
        assert_eq!(r.paper_ink, bandit().roles.paper_ink);
        // Without a fallback the grounds are required.
        assert!(parse(text, None).is_err());
    }

    #[test]
    fn later_versions_are_skipped_and_bad_files_refused() {
        let v2 = BANDIT_JSON.replace("\"version\": 1", "\"version\": 2");
        assert_eq!(parse(&v2, None), Ok(None));
        for (text, why) in [
            (BANDIT_JSON.replace("foxglove-palette", "other"), "format"),
            (BANDIT_JSON.replace("\"#f08a3c\",", "\"orange\","), "stop"),
            (
                BANDIT_JSON.replace("\"accent\": \"#f08a3c\",", ""),
                "accent",
            ),
            (BANDIT_JSON.replace("\"#ffb070\"", "\"#fff\""), "highlight"),
            (
                BANDIT_JSON.replace(", \"#ff5f3c\", \"#060812\"", ""),
                "one stop",
            ),
        ] {
            assert!(parse(&text, None).is_err(), "{why}");
        }
    }

    /// Either Bandit orange (#40 Q1) drops in by editing only the file: with the hand-picked
    /// shades taken out, the derived ones still meet the theme's contrast rules.
    #[test]
    fn either_orange_drops_in() {
        for orange in ["#f08a3c", "#e5732b"] {
            let text = BANDIT_JSON
                .replace("\"#f08a3c\"", &format!("\"{orange}\""))
                .replace("\"highlight\": \"#ffb070\",", "")
                .replace("\"accent_light\": \"#b4540f\",", "");
            let r = parse(&text, None).unwrap().unwrap().roles;
            assert_eq!(Rgb(hex(orange).unwrap()), r.accent);
            assert!(contrast(r.accent_light, Rgb::WHITE) >= 4.5, "{orange}");
            assert!(contrast(r.accent_light, r.paper) >= 4.5, "{orange}");
            assert!(contrast(r.deep, r.paper) >= 3.0, "{orange}");
        }
    }

    /// An old file (Bandit's, without the new roles) resolves exactly as before: no second or
    /// third accent, and every consumer falls back to `accent`.
    #[test]
    fn a_file_without_the_new_roles_has_one_accent() {
        let r = bandit().roles;
        assert_eq!(r.accent2, None);
        assert_eq!(r.accent2_light, None);
        assert_eq!(r.accent3, None);
        assert!(!r.is_harmony());
        assert_eq!(r.second(), r.accent);
        assert_eq!(r.third(), r.accent);
        assert_eq!(bundled()[0], *bandit(), "Bandit is first");
    }

    /// Every bundled palette reads, keeps Bandit's orange and holds the contrast pairs
    /// scripts/palette_contrast.py checks (docs/palette.md).
    #[test]
    fn every_bundled_palette_reads_and_keeps_bandits_orange() {
        let all = bundled();
        assert_eq!(
            all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            [
                "Bandit",
                "Bandit Steel",
                "Bandit Dusk",
                "Bandit Foxglove",
                "Bandit Ember"
            ]
        );
        let b = bandit().roles;
        for p in &all[1..] {
            let r = p.roles;
            let n = &p.name;
            assert_eq!(r.accent, b.accent, "{n}");
            assert_eq!(r.highlight, b.highlight, "{n}");
            assert_eq!(r.accent_light, b.accent_light, "{n}");
            assert!(r.is_harmony(), "{n}");
            assert!(contrast(r.ink, r.base) >= 4.5, "{n} ink");
            assert!(contrast(r.accent, r.base) >= 4.5, "{n} accent");
            assert!(contrast(r.second(), r.base) >= 4.5, "{n} accent2");
            assert!(contrast(r.accent2_light.unwrap(), r.paper) >= 4.5, "{n}");
            if let Some(a3) = r.accent3 {
                assert!(contrast(a3, r.base) >= 4.5, "{n} accent3");
                assert!(contrast(r.accent3_light.unwrap(), r.paper) >= 4.5, "{n}");
            }
            assert!(contrast(r.paper_ink, r.paper) >= 4.5, "{n} paper ink");
            assert!(contrast(r.deep, r.paper) >= 3.0, "{n} deep");
            assert!(STOPS.contains(&p.stops.len()), "{n}");
        }
        let dusk = by_name("Bandit Dusk").unwrap().roles;
        assert_eq!(dusk.accent3, Some(Rgb([0x99, 0x9e, 0xe6])));
        assert_eq!(dusk.accent3_light, Some(Rgb([0x39, 0x41, 0xac])));
        assert_eq!(by_name("Bandit Steel").unwrap().roles.accent3, None);
        assert!(by_name("Nope").is_none());
    }

    #[test]
    fn light_shades_are_derived_and_accent3_needs_accent2() {
        let text = r##"{ "format": "foxglove-palette", "version": 1, "name": "Reef",
            "roles": { "accent": "#f08a3c", "accent2": "#62bcb6", "accent3": "#999ee6" },
            "stops": ["#f08a3c", "#62bcb6"] }"##;
        let r = parse(text, Some(&bandit().roles)).unwrap().unwrap().roles;
        assert_eq!(r.accent2, Some(Rgb([0x62, 0xbc, 0xb6])));
        assert!(contrast(r.accent2_light.unwrap(), r.paper) >= 4.5);
        assert!(contrast(r.accent3_light.unwrap(), r.paper) >= 4.5);
        assert_eq!(r.third(), Rgb([0x99, 0x9e, 0xe6]));
        // Without accent2, accent3 is skipped: the palette has one accent.
        let lone = text.replace(r##""accent2": "#62bcb6", "##, "");
        let r = parse(&lone, Some(&bandit().roles)).unwrap().unwrap().roles;
        assert_eq!((r.accent2, r.accent3, r.accent3_light), (None, None, None));
        assert_eq!(r.third(), r.accent);
        // The optional tier: bold when it says so, harmony when absent or unknown.
        let tiered = |label: &str| {
            let t = text.replace(
                r##""name": "Reef","##,
                &format!(r##""name": "Reef", "tier": "{label}","##),
            );
            parse(&t, Some(&bandit().roles)).unwrap().unwrap().tier
        };
        assert_eq!(tiered("bold"), Tier::Bold);
        assert_eq!(tiered("loud"), Tier::Harmony);
        assert_eq!(
            parse(text, Some(&bandit().roles)).unwrap().unwrap().tier,
            Tier::Harmony
        );
        assert_eq!(bandit().tier, Tier::Harmony);
        // A bad optional role refuses the file, as a bad required one does.
        let bad = text.replace(r##""#62bcb6", "accent3""##, r##""teal", "accent3""##);
        assert!(parse(&bad, Some(&bandit().roles)).is_err());
    }

    #[test]
    fn readable_keeps_a_colour_that_already_reads() {
        let ground = Rgb([0x09, 0x09, 0x0a]);
        let orange = Rgb([0xf0, 0x8a, 0x3c]);
        assert_eq!(readable(orange, ground, 3.0), orange);
        let dark = readable(orange, Rgb::WHITE, 4.5);
        assert!(contrast(dark, Rgb::WHITE) >= 4.5);
        assert!(to_hsl(dark)[2] < to_hsl(orange)[2]);
        let light = readable(Rgb([0x30, 0x10, 0x00]), ground, 3.0);
        assert!(contrast(light, ground) >= 3.0);
    }
}
