//! The palette of a wallpaper's image (#40): for "Lights follow the wallpaper" when the wallpaper
//! on screen has no `foxglove.json` palette (`wallpaper_manifest`), the accent comes from the
//! picture itself, as Windows takes its accent from the desktop wallpaper.
//!
//! - The image: the Windows desktop wallpaper's file (or `TranscodedWallpaper`, Windows' copy,
//!   when the file named in the registry is gone), or a Wallpaper Engine project's preview (its
//!   `project.json` › `preview`), or the file on screen itself when it is a picture.
//! - Bounded: a file over [`MAX_BYTES`] or [`MAX_SIDE`] pixels a side is skipped, decoding has an
//!   allocation limit, and the picture is shrunk to [`THUMB`] pixels a side before any colour
//!   work, so a 4K wallpaper costs the same as a small one past its decode.
//! - The palette: a few colours by k-means in Oklab (`foxglove_scene::Rgb::lab`, `palette`,
//!   #474), where distances look even. The accent is the colour with the most colour for its
//!   share of the picture; a picture without one (greys, near black) gives none, so the lights
//!   fall back to the Windows accent.
//! - Event-driven: the wallpaper service asks on each of its change passes (`wallpaper.rs`), on
//!   its own thread, and [`Images`] decodes again only when the image's path, write time or
//!   size changed. Nothing polls.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use foxglove_scene::Rgb;

/// Image files larger than this are skipped (a wallpaper picture is a few MiB).
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Pictures wider or taller than this are skipped.
pub const MAX_SIDE: u32 = 16_384;
/// The decoder's allocation limit.
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
/// The palette is taken from the picture shrunk to fit this many pixels a side.
pub const THUMB: u32 = 64;
/// How many colours the palette has at most.
const COLOURS: usize = 6;
/// k-means rounds: enough for a few thousand pixels to settle.
const ROUNDS: usize = 12;
/// Below this Oklab lightness a colour's hue is noise.
const MIN_LIGHTNESS: f32 = 0.25;
/// Below this Oklab chroma a colour reads as grey.
const MIN_CHROMA: f32 = 0.04;

/// One colour of a picture's palette and how much of the picture it covers (0..=1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatch {
    pub colour: Rgb,
    pub share: f32,
}

/// The picture's palette, most of the picture first: at most [`COLOURS`] colours.
pub fn palette(pixels: &[Rgb]) -> Vec<Swatch> {
    if pixels.is_empty() {
        return Vec::new();
    }
    let labs: Vec<[f32; 3]> = pixels.iter().map(|p| p.lab()).collect();
    let mut centres = seeds(&labs, COLOURS);
    let mut members = vec![0usize; labs.len()];
    for _ in 0..ROUNDS {
        for (m, p) in members.iter_mut().zip(&labs) {
            *m = nearest(&centres, p);
        }
        let mut sums = vec![([0.0f32; 3], 0usize); centres.len()];
        for (m, p) in members.iter().zip(&labs) {
            let (s, n) = &mut sums[*m];
            for (s, v) in s.iter_mut().zip(p) {
                *s += v;
            }
            *n += 1;
        }
        let moved: Vec<[f32; 3]> = sums
            .iter()
            .zip(&centres)
            .map(|((s, n), c)| {
                if *n == 0 {
                    *c
                } else {
                    s.map(|v| v / *n as f32)
                }
            })
            .collect();
        if moved == centres {
            break;
        }
        centres = moved;
    }
    let mut counts = vec![0usize; centres.len()];
    for m in &members {
        counts[*m] += 1;
    }
    let total = labs.len() as f32;
    let mut out: Vec<Swatch> = centres
        .iter()
        .zip(&counts)
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| Swatch {
            colour: Rgb::from_lab(*c),
            share: *n as f32 / total,
        })
        .collect();
    out.sort_by(|a, b| b.share.total_cmp(&a.share));
    out
}

/// The palette's accent: the colour with the most colour for the share of the picture it covers
/// (chroma times the square root of its share, so a large tint beats a speck of neon, and a
/// vivid area beats a large grey one). None when every colour is grey or too dark.
pub fn accent(palette: &[Swatch]) -> Option<Rgb> {
    palette
        .iter()
        .filter_map(|s| {
            let [l, a, b] = s.colour.lab();
            let chroma = a.hypot(b);
            (l >= MIN_LIGHTNESS && chroma >= MIN_CHROMA).then(|| (chroma * s.share.sqrt(), s))
        })
        .max_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, s)| s.colour)
}

/// Deterministic seeds: the mean, then each time the pixel farthest from every seed so far
/// (fewer when the picture has fewer distinct colours).
fn seeds(labs: &[[f32; 3]], k: usize) -> Vec<[f32; 3]> {
    let n = labs.len() as f32;
    let mut mean = [0.0f32; 3];
    for p in labs {
        for (m, v) in mean.iter_mut().zip(p) {
            *m += v / n;
        }
    }
    let mut out = vec![mean];
    let mut far: Vec<f32> = labs.iter().map(|p| dist(p, &mean)).collect();
    while out.len() < k {
        let Some((i, d)) = far
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        if d <= 1e-6 {
            break;
        }
        let c = labs[i];
        for (f, p) in far.iter_mut().zip(labs) {
            *f = f.min(dist(p, &c));
        }
        out.push(c);
    }
    out
}

fn dist(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

fn nearest(centres: &[[f32; 3]], p: &[f32; 3]) -> usize {
    centres
        .iter()
        .enumerate()
        .min_by(|a, b| dist(a.1, p).total_cmp(&dist(b.1, p)))
        .map_or(0, |(i, _)| i)
}

/// The picture at `path`, shrunk to fit [`THUMB`] pixels a side, as its opaque pixels. None for
/// a file that is missing, too large, or not a picture `image` reads.
pub fn thumbnail(path: &Path) -> Option<Vec<Rgb>> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    if meta.len() > MAX_BYTES {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    reader.format()?;
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let small = img.thumbnail(THUMB, THUMB).to_rgba8();
    Some(
        small
            .pixels()
            .filter(|p| p.0[3] >= 128)
            .map(|p| Rgb([p.0[0], p.0[1], p.0[2]]))
            .collect(),
    )
}

/// The accent of the picture at `path` (see [`accent`]).
pub fn accent_of(path: &Path) -> Option<Rgb> {
    // A decoder bug in one odd file shouldn't restart the whole wallpaper service.
    let pixels = std::panic::catch_unwind(|| thumbnail(path)).ok()??;
    accent(&palette(&pixels))
}

/// Whether `path`'s extension is a picture `image` reads.
fn is_picture(path: &Path) -> bool {
    image::ImageFormat::from_path(path).is_ok()
}

/// A Wallpaper Engine project's preview picture: `project.json` › `preview` in `dir`.
fn project_preview(dir: &Path) -> Option<PathBuf> {
    let project = dir.join("project.json");
    // project.json is small; a huge one isn't a project.
    if std::fs::metadata(&project).ok()?.len() > 1024 * 1024 {
        return None;
    }
    let text = std::fs::read_to_string(&project).ok()?;
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let name = v.get("preview")?.as_str()?.trim();
    // A name in the project's folder, never a path out of it.
    let p = Path::new(name);
    let plain = p.components().count() == 1 && !p.is_absolute();
    (plain && !name.is_empty()).then(|| dir.join(name))
}

/// The pictures that stand for the wallpaper `file`, best first: the file itself when it is a
/// picture, then its project's preview. `fallback` (Windows' copy of the desktop wallpaper) comes
/// last.
pub fn candidates(file: &str, fallback: Option<&Path>) -> Vec<PathBuf> {
    let f = PathBuf::from(file.trim().replace('\\', "/"));
    let mut out = Vec::new();
    if is_picture(&f) {
        out.push(f.clone());
    }
    if let Some(p) = super::wallpaper_manifest::folder_of(file).and_then(|d| project_preview(&d)) {
        out.push(p);
    }
    out.extend(fallback.map(Path::to_path_buf));
    out
}

/// What identifies one version of a picture: its path, write time and size.
type Key = (PathBuf, Option<SystemTime>, u64);

fn key(path: &Path) -> Option<Key> {
    let m = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some((path.to_path_buf(), m.modified().ok(), m.len()))
}

/// The wallpaper service's last picture and its accent: decoded again only when the picture
/// changed.
#[derive(Default)]
pub struct Images {
    last: Option<(Key, Option<Rgb>)>,
    /// How many pictures were decoded (for tests).
    decoded: usize,
}

impl Images {
    /// The accent of the first of `candidates` that exists, decoding it only when it isn't the
    /// picture asked last, unchanged.
    pub fn accent(&mut self, candidates: &[PathBuf]) -> Option<Rgb> {
        let k = candidates.iter().find_map(|p| key(p))?;
        if let Some((last, accent)) = &self.last {
            if *last == k {
                return *accent;
            }
        }
        self.decoded += 1;
        let accent = accent_of(&k.0);
        self.last = Some((k, accent));
        accent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("foxglove-wp-image-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A picture `w` x `h`: `paint` gives each pixel.
    fn picture(w: u32, h: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> image::RgbImage {
        image::RgbImage::from_fn(w, h, |x, y| image::Rgb(paint(x, y)))
    }

    fn hue(c: Rgb) -> u16 {
        crate::foxglove::accent::hue_sat(c.0[0], c.0[1], c.0[2]).0
    }

    /// A dark navy sky over a teal sea with a speck of hot pink: the teal is the accent (it is
    /// both colourful and large), not the navy (dark) or the pink (a speck).
    #[test]
    fn the_accent_is_a_colourful_large_area() {
        let (navy, teal, pink) = ([0x0b, 0x25, 0x45], [0x0f, 0xb5, 0xae], [0xff, 0x2d, 0x95]);
        let img = picture(200, 100, |x, y| {
            if x < 3 && y < 3 {
                pink
            } else if y < 55 {
                navy
            } else {
                teal
            }
        });
        let pixels: Vec<Rgb> = img.pixels().map(|p| Rgb(p.0)).collect();
        let p = palette(&pixels);
        assert!(p.len() <= COLOURS);
        assert!((p.iter().map(|s| s.share).sum::<f32>() - 1.0).abs() < 1e-3);
        assert_eq!(p[0].colour, Rgb(navy), "the largest first");
        let a = accent(&p).unwrap();
        assert!(hue(a).abs_diff(hue(Rgb(teal))) <= 2, "{a:?}");
    }

    /// Greys, and a near-black picture with a dim tint, give no accent: the lights fall back to
    /// the Windows accent.
    #[test]
    fn a_grey_or_dark_picture_has_no_accent() {
        let grey = picture(64, 64, |x, _| {
            let v = (x * 4) as u8;
            [v, v, v]
        });
        let px = |img: &image::RgbImage| img.pixels().map(|p| Rgb(p.0)).collect::<Vec<_>>();
        assert_eq!(accent(&palette(&px(&grey))), None);
        let dark = picture(64, 64, |_, _| [20, 5, 30]);
        assert_eq!(accent(&palette(&px(&dark))), None);
        assert!(palette(&[]).is_empty());
    }

    /// A large picture is shrunk before the palette; files are read by their bytes (Windows'
    /// `TranscodedWallpaper` has no extension); a file that isn't a picture gives nothing.
    #[test]
    fn pictures_are_shrunk_and_read_by_content() {
        let d = dir("thumb");
        let img = picture(1920, 1080, |x, _| {
            if x < 960 {
                [200, 40, 30]
            } else {
                [30, 60, 200]
            }
        });
        let png = d.join("wide.png");
        img.save(&png).unwrap();
        let px = thumbnail(&png).unwrap();
        assert_eq!(px.len() as u32, THUMB * 36, "64 x 36, the same shape");
        let bare = d.join("TranscodedWallpaper");
        std::fs::copy(&png, &bare).unwrap();
        assert_eq!(thumbnail(&bare), Some(px));
        let text = d.join("index.html");
        std::fs::write(&text, "<html></html>").unwrap();
        assert_eq!(thumbnail(&text), None);
        assert_eq!(thumbnail(&d.join("missing.jpg")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The picture for a wallpaper: the file when it is one, a Wallpaper Engine project's
    /// preview, then Windows' copy; a preview naming a path out of the project is ignored.
    #[test]
    fn a_wallpaper_is_found_as_its_picture_or_its_preview() {
        let d = dir("candidates");
        let project = d.join("reef");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("project.json"),
            "\u{feff}{\"title\":\"Reef\",\"preview\":\"preview.gif\"}",
        )
        .unwrap();
        let index = project.join("index.html").to_string_lossy().to_string();
        let fallback = d.join("TranscodedWallpaper");
        assert_eq!(
            candidates(&index, Some(&fallback)),
            vec![
                PathBuf::from(
                    project
                        .join("preview.gif")
                        .to_string_lossy()
                        .replace('\\', "/")
                ),
                fallback.clone()
            ]
        );
        let jpg = d.join("beach.jpg").to_string_lossy().to_string();
        assert_eq!(
            candidates(&jpg, None),
            vec![PathBuf::from(jpg.replace('\\', "/"))]
        );
        std::fs::write(
            project.join("project.json"),
            r#"{"preview":"../../secret.png"}"#,
        )
        .unwrap();
        assert!(candidates(&index, None).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The service's passes decode a picture once: again only when it is rewritten or another
    /// picture is on screen; a missing first candidate falls through to the next.
    #[test]
    fn a_picture_is_decoded_again_only_when_it_changed() {
        let d = dir("cache");
        let a = d.join("a.png");
        let b = d.join("b.png");
        picture(32, 32, |_, _| [0x0f, 0xb5, 0xae]).save(&a).unwrap();
        picture(32, 32, |_, _| [0xe0, 0x30, 0x20]).save(&b).unwrap();
        let mut images = Images::default();
        let teal = images.accent(&[d.join("gone.png"), a.clone()]).unwrap();
        for _ in 0..5 {
            assert_eq!(images.accent(std::slice::from_ref(&a)), Some(teal));
        }
        assert_eq!(images.decoded, 1);
        let red = images.accent(std::slice::from_ref(&b)).unwrap();
        assert_ne!(hue(red), hue(teal));
        assert_eq!(images.decoded, 2);
        // Rewritten in place, with other colours (and so another size).
        picture(48, 48, |_, _| [0x30, 0x40, 0xe0]).save(&b).unwrap();
        let blue = images.accent(std::slice::from_ref(&b)).unwrap();
        assert_eq!(images.decoded, 3);
        assert_ne!(hue(blue), hue(red));
        assert_eq!(images.accent(&[d.join("none.png")]), None);
        assert_eq!(images.decoded, 3, "nothing to decode");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Over the size limit: skipped before decoding.
    #[test]
    fn a_huge_file_is_skipped() {
        let d = dir("huge");
        let f = d.join("huge.png");
        let file = std::fs::File::create(&f).unwrap();
        file.set_len(MAX_BYTES + 1).unwrap();
        drop(file);
        assert_eq!(thumbnail(&f), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
