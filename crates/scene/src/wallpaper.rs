//! The scene as a wallpaper reads it (#493, #494). Foxglove hands each Bandit wallpaper one short
//! text property, `foxglove_scene`, through Wallpaper Engine's command line; the wallpaper draws
//! its monitor's part of the effect from the shared clock, and nothing is streamed.
//!
//! The text is facts only (command lines are visible to other processes) and has no spaces or
//! quotes, so it passes through `-properties RAW~(...)~END` untouched:
//!
//! ```text
//! v=1;id=demo;fx=scanline;e=1761000000000;p=4000;a=90;w=80;c=f08a3c;bg=06080e;room=0,0,600,700;mon=0,0,600,340
//! ```
//!
//! - `e` the clock's epoch (Unix ms) and `p` the period (ms): `phase = ((now - e) mod p) / p`;
//! - `a` the direction in degrees (0 left to right, 90 top to bottom) and `w` the band's width (mm);
//! - `c` / `bg` the effect's colour and background (hex, no `#`);
//! - `room` the room's bounds and `mon` this monitor's rectangle in it (mm: x, y, w, h). Without
//!   `mon` the wallpaper takes its screen as the whole room.
//!
//! An empty `foxglove_scene` means no scene: the wallpaper runs on its own clock.
//! Both sides check the maths against `tests/wallpaper_vectors.json` (bandit-wallpapers keeps a
//! copy at `tools/scene_vectors.json`).

use crate::clock::Clock;
use crate::colour::Rgb;
use crate::effects::Effect;
use crate::space::Rect;

/// The payload's version; a wallpaper ignores a payload it doesn't know.
pub const VERSION: u32 = 1;

/// One monitor's view of a running scene.
#[derive(Clone, Debug, PartialEq)]
pub struct WallpaperScene {
    /// The scene's id: letters, digits and `-` only.
    pub id: String,
    /// The effect's kind (`scanline`, `sweep`, ...), as in [`Effect`]'s tag.
    pub fx: String,
    pub epoch_unix_ms: i64,
    pub period_ms: u32,
    pub angle_deg: f32,
    pub width_mm: f32,
    pub colour: Rgb,
    pub background: Rgb,
    pub room: Rect,
    pub monitor: Option<Rect>,
}

/// The parts of an effect a wallpaper can follow: kind, period, direction, width and two colours.
fn shape(effect: &Effect) -> (&'static str, u32, f32, f32, Rgb, Rgb) {
    let first = |s: &[Rgb]| s.first().copied().unwrap_or(Rgb::BLACK);
    let last = |s: &[Rgb]| s.last().copied().unwrap_or(Rgb::BLACK);
    match effect {
        Effect::Scanline {
            colour,
            background,
            angle_deg,
            width_mm,
            period_ms,
        } => (
            "scanline",
            *period_ms,
            *angle_deg,
            *width_mm,
            *colour,
            *background,
        ),
        Effect::Sweep {
            stops,
            angle_deg,
            period_ms,
        } => (
            "sweep",
            *period_ms,
            *angle_deg,
            0.0,
            first(stops),
            last(stops),
        ),
        Effect::Ripple {
            colour,
            background,
            width_mm,
            period_ms,
            ..
        } => ("ripple", *period_ms, 0.0, *width_mm, *colour, *background),
        Effect::Breathe {
            from,
            to,
            period_ms,
        } => ("breathe", *period_ms, 0.0, 0.0, *to, *from),
        Effect::Gradient { stops, angle_deg } => {
            ("gradient", 0, *angle_deg, 0.0, last(stops), first(stops))
        }
        Effect::Noise {
            stops, period_ms, ..
        } => ("noise", *period_ms, 0.0, 0.0, last(stops), first(stops)),
    }
}

/// Keeps an id to letters, digits and `-` (spaces become `-`), at most 32 characters.
pub fn clean_id(id: &str) -> String {
    let mut out: String = id
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(32)
        .collect();
    if out.is_empty() {
        out.push_str("scene");
    }
    out
}

fn hex(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c.0[0], c.0[1], c.0[2])
}

/// A number with at most one decimal, no trailing `.0`.
fn num(v: f32) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r:.1}")
    }
}

fn rect(r: &Rect) -> String {
    [r.x, r.y, r.w, r.h].map(num).join(",")
}

impl WallpaperScene {
    /// The scene as one monitor sees it. `monitor` is that monitor's rectangle in the room, when
    /// Foxglove knows which one it is.
    pub fn new(
        id: &str,
        effect: &Effect,
        clock: &Clock,
        room: Rect,
        monitor: Option<Rect>,
    ) -> Self {
        let (fx, period_ms, angle_deg, width_mm, colour, background) = shape(effect);
        WallpaperScene {
            id: clean_id(id),
            fx: fx.to_string(),
            epoch_unix_ms: clock.epoch_unix_ms(),
            period_ms,
            angle_deg,
            width_mm,
            colour,
            background,
            room,
            monitor,
        }
    }

    /// The `foxglove_scene` text.
    pub fn encode(&self) -> String {
        let mut s = format!(
            "v={VERSION};id={};fx={};e={};p={};a={};w={};c={};bg={};room={}",
            self.id,
            self.fx,
            self.epoch_unix_ms,
            self.period_ms,
            num(self.angle_deg),
            num(self.width_mm),
            hex(self.colour),
            hex(self.background),
            rect(&self.room),
        );
        if let Some(m) = &self.monitor {
            s.push_str(";mon=");
            s.push_str(&rect(m));
        }
        s
    }

    /// Reads a `foxglove_scene` text back (tests, and the shared vectors). `None` for an empty
    /// text, another version or a missing field.
    pub fn decode(text: &str) -> Option<Self> {
        let mut fields = std::collections::BTreeMap::new();
        for part in text.trim().split(';').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=')?;
            fields.insert(k, v);
        }
        if fields.get("v")?.parse::<u32>().ok()? != VERSION {
            return None;
        }
        let colour = |k: &str| -> Option<Rgb> {
            let v = fields.get(k)?;
            if v.len() != 6 {
                return None;
            }
            let b = |i: usize| u8::from_str_radix(&v[i..i + 2], 16).ok();
            Some(Rgb([b(0)?, b(2)?, b(4)?]))
        };
        let rect = |k: &str| -> Option<Rect> {
            let n: Vec<f32> = fields
                .get(k)?
                .split(',')
                .map(|x| x.parse::<f32>().ok())
                .collect::<Option<_>>()?;
            (n.len() == 4).then(|| Rect::new(n[0], n[1], n[2], n[3]))
        };
        Some(WallpaperScene {
            id: fields.get("id")?.to_string(),
            fx: fields.get("fx")?.to_string(),
            epoch_unix_ms: fields.get("e")?.parse().ok()?,
            period_ms: fields.get("p")?.parse().ok()?,
            angle_deg: fields.get("a")?.parse().ok()?,
            width_mm: fields.get("w")?.parse().ok()?,
            colour: colour("c")?,
            background: colour("bg")?,
            room: rect("room")?,
            monitor: match fields.get("mon") {
                Some(_) => Some(rect("mon")?),
                None => None,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgb;

    fn demo() -> Effect {
        Effect::Scanline {
            colour: Rgb([0xf0, 0x8a, 0x3c]),
            background: Rgb([6, 8, 14]),
            angle_deg: 90.0,
            width_mm: 80.0,
            period_ms: 4000,
        }
    }

    #[test]
    fn a_scanline_encodes_to_one_short_line_without_spaces_or_quotes() {
        let s = WallpaperScene::new(
            "demo scanline",
            &demo(),
            &Clock::aligned(1_761_000_000_000),
            Rect::new(0.0, 0.0, 600.0, 700.25),
            Some(Rect::new(0.0, 0.0, 600.0, 337.5)),
        );
        let text = s.encode();
        assert_eq!(
            text,
            "v=1;id=demo-scanline;fx=scanline;e=1761000000000;p=4000;a=90;w=80;c=f08a3c;bg=06080e;room=0,0,600,700.3;mon=0,0,600,337.5"
        );
        assert!(!text.contains(' ') && !text.contains('"') && !text.contains('\\'));
        let back = WallpaperScene::decode(&text).unwrap();
        assert_eq!(back.encode(), text);
        assert_eq!(back.monitor, Some(Rect::new(0.0, 0.0, 600.0, 337.5)));
    }

    #[test]
    fn decode_refuses_empty_other_versions_and_broken_fields() {
        assert_eq!(WallpaperScene::decode(""), None);
        assert_eq!(WallpaperScene::decode("v=2;id=x"), None);
        assert_eq!(
            WallpaperScene::decode(
                "v=1;id=x;fx=scanline;e=1;p=1;a=0;w=0;c=zz0000;bg=000000;room=0,0,1,1"
            ),
            None
        );
        let no_mon = "v=1;id=x;fx=sweep;e=-5;p=0;a=0;w=0;c=000000;bg=ffffff;room=0,0,1,1";
        assert_eq!(WallpaperScene::decode(no_mon).unwrap().monitor, None);
    }

    #[test]
    fn every_effect_has_a_shape_and_ids_are_cleaned() {
        let clock = Clock::aligned(0);
        let room = Rect::new(0.0, 0.0, 1.0, 1.0);
        let stops = vec![Rgb([1, 2, 3]), Rgb([4, 5, 6])];
        let sweep = Effect::Sweep {
            stops: stops.clone(),
            angle_deg: 0.0,
            period_ms: 9000,
        };
        let s = WallpaperScene::new("", &sweep, &clock, room, None);
        assert_eq!(
            (s.id.as_str(), s.fx.as_str(), s.period_ms),
            ("scene", "sweep", 9000)
        );
        assert_eq!((s.colour, s.background), (Rgb([1, 2, 3]), Rgb([4, 5, 6])));
        let g = Effect::Gradient {
            stops,
            angle_deg: 45.0,
        };
        assert_eq!(
            WallpaperScene::new("x", &g, &clock, room, None).period_ms,
            0
        );
        assert_eq!(clean_id("  Night \"drive\"; v=2 "), "Night-drive-v2");
    }
}
