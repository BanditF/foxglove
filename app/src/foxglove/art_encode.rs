//! Builds an NVA1 art blob from a Flipper-style frame folder (`meta.txt` + `frame_N.png`), as
//! `scripts/flipper_art.py` does, so the art library can list and push source folders such as
//! `art-src\bandit\out\Bandit_idle` without Python (#59).
//!
//! Same rules as the script: each frame is fitted upright into the art screen (never upscaled,
//! centred), thresholded at 50% (ink = a set bit, #610; the keyboard shows ink in the panel
//! polarity's foreground, #666), stored turned as the screen is mounted, and each unique frame is
//! an RLE-packed XOR delta against the previous one. The screen's size and turn come from its
//! descriptor (#560).
//!
//! Polarity (#666): the drawing is the ink, whichever way round the source was drawn.
//! [`Polarity::Auto`] finds the background from the edge pixels of every frame of the set (a tie
//! goes to all their pixels): a light background makes the dark pixels the ink, a dark one the
//! light pixels (the source is inverted before the padding goes round it). The whole set goes one
//! way. Blobs that are already encoded (files, plugins) are normalised the same way by
//! [`normalise_blob`], which only flips their header's ink flag.

use std::path::Path;

use image::{imageops, GrayImage, Luma};
use serde::{Deserialize, Serialize};

/// Header byte 7 (art_relay.h `ART_RELAY_FLAG_INK`, #610): set bits are ink. Blobs without it
/// (from before #610) have the background bits set; the keyboard and `art_preview` invert them.
pub const FLAG_INK: u8 = 0x01;

/// Which way round a piece's ink is (#666), as the art picker and the plugin API set it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Polarity {
    /// The background decides: the majority of the edge pixels (a tie: of all of them).
    #[default]
    Auto,
    /// The dark pixels are the ink (a source drawn dark on light), whatever the background.
    AsIs,
    /// The light pixels are the ink (a source drawn light on dark).
    Invert,
}

impl Polarity {
    pub const ALL: [Polarity; 3] = [Polarity::Auto, Polarity::AsIs, Polarity::Invert];

    /// The picker's words.
    pub fn label(self) -> &'static str {
        match self {
            Polarity::Auto => "Auto",
            Polarity::AsIs => "As is",
            Polarity::Invert => "Invert",
        }
    }
}

/// Dark edge pixels, edge pixels, dark pixels and pixels, at 50%.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    edge_dark: u64,
    edge: u64,
    dark: u64,
    all: u64,
}

impl Counts {
    /// Counts `w` x `h` pixels, `dark(x, y)` saying which are dark (or ink).
    fn of(w: u32, h: u32, dark: impl Fn(u32, u32) -> bool) -> Counts {
        let mut c = Counts::default();
        for y in 0..h {
            for x in 0..w {
                let d = u64::from(dark(x, y));
                c.dark += d;
                if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                    c.edge += 1;
                    c.edge_dark += d;
                }
            }
        }
        c.all = u64::from(w) * u64::from(h);
        c
    }

    fn add(self, o: Counts) -> Counts {
        Counts {
            edge_dark: self.edge_dark + o.edge_dark,
            edge: self.edge + o.edge,
            dark: self.dark + o.dark,
            all: self.all + o.all,
        }
    }

    /// The background is dark: the edge's majority, or on a tie all the pixels'.
    fn dark_background(self) -> bool {
        if self.edge_dark * 2 != self.edge {
            self.edge_dark * 2 > self.edge
        } else {
            self.dark * 2 > self.all
        }
    }
}

/// Whether a set of fitted sources is drawn light on dark, so its light pixels are the ink.
fn should_invert(fitted: &[GrayImage], polarity: Polarity) -> bool {
    match polarity {
        Polarity::AsIs => false,
        Polarity::Invert => true,
        Polarity::Auto => fitted
            .iter()
            .map(|img| {
                Counts::of(img.width(), img.height(), |x, y| {
                    img.get_pixel(x, y).0[0] < 128
                })
            })
            .fold(Counts::default(), Counts::add)
            .dark_background(),
    }
}

/// An encoded blob normalised to `polarity` (#666): auto flips its ink flag when its ink, as the
/// keyboard shows it, is the background (the majority of the stored frames' edge; a tie: of all
/// their pixels); invert flips it; as is leaves it. Flipping the flag inverts every frame on the
/// keyboard and in `art_preview` (only the padding bits past the art's width differ, which nobody
/// sees). A blob that doesn't decode is left as it is: the push reports it.
pub fn normalise_blob(blob: &[u8], polarity: Polarity) -> Vec<u8> {
    let mut out = blob.to_vec();
    let flip = match polarity {
        Polarity::AsIs => false,
        Polarity::Invert => true,
        Polarity::Auto => blob_ink_is_background(blob).unwrap_or(false),
    };
    if flip && out.len() > 7 {
        out[7] ^= FLAG_INK;
    }
    out
}

/// Whether a blob's ink, as shown, covers its background; `None` when it doesn't decode.
fn blob_ink_is_background(blob: &[u8]) -> Option<bool> {
    let d = super::art_preview::decode(blob).ok()?;
    let stride = d.width.div_ceil(8);
    let (w, h) = (u32::try_from(d.width).ok()?, u32::try_from(d.height).ok()?);
    let counts = d
        .frames
        .iter()
        .map(|bits| {
            Counts::of(w, h, |x, y| {
                let (x, y) = (x as usize, y as usize);
                (bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0) == d.ink
            })
        })
        .fold(Counts::default(), Counts::add);
    Some(counts.dark_background())
}

/// The art screen a set is made for (#560): its stored buffer's size, and how many quarter turns
/// clockwise the upright art is turned to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtTarget {
    pub width: u32,
    pub height: u32,
    pub rotation: u8,
}

impl ArtTarget {
    /// The art format Foxglove's firmware builds today, for a keyboard that doesn't describe its
    /// art screen: 140x68 stored, mounted on its side (other sizes on the firmware: #340).
    pub const FIRMWARE: ArtTarget = ArtTarget {
        width: 140,
        height: 68,
        rotation: 1,
    };

    /// The target for a described art screen; `None` past what a blob's header holds (255).
    pub fn of(screen: &super::screen_roles::Screen) -> Option<ArtTarget> {
        let fits = |v: u16| (1..=255).contains(&v);
        (fits(screen.width) && fits(screen.height)).then(|| ArtTarget {
            width: u32::from(screen.width),
            height: u32::from(screen.height),
            rotation: screen.rotation % 4,
        })
    }

    /// The connected keyboard's art screen, else [`ArtTarget::FIRMWARE`].
    pub fn current() -> ArtTarget {
        super::screen_roles::current()
            .as_deref()
            .and_then(|s| super::screen_roles::first(s, super::screen_roles::Role::Art))
            .and_then(ArtTarget::of)
            .unwrap_or(ArtTarget::FIRMWARE)
    }

    /// The art's size upright, as drawn.
    fn upright(self) -> (u32, u32) {
        if self.rotation % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// A folder holds one set when it has a `meta.txt`.
pub fn is_set(folder: &Path) -> bool {
    folder.join("meta.txt").is_file()
}

/// The folder's frames encoded as one pushable blob for `target`, its ink normalised to
/// `polarity` (#666).
pub fn encode_folder(
    folder: &Path,
    target: ArtTarget,
    polarity: Polarity,
) -> Result<Vec<u8>, String> {
    let meta =
        std::fs::read_to_string(folder.join("meta.txt")).map_err(|e| format!("meta.txt: {e}"))?;
    let value = |key: &str| {
        meta.lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| v.trim().to_owned())
    };
    let order: Vec<usize> = value("Frames order")
        .map(|v| {
            v.split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect()
        })
        .filter(|o: &Vec<usize>| !o.is_empty())
        .unwrap_or_else(|| vec![0]);
    let fps: u8 = value("Frame rate")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let mut cache: Vec<(usize, GrayImage)> = Vec::new();
    let mut sources = Vec::with_capacity(order.len());
    for &i in &order {
        if let Some((_, f)) = cache.iter().find(|(n, _)| *n == i) {
            sources.push(f.clone());
            continue;
        }
        let path = folder.join(format!("frame_{i}.png"));
        let img = image::open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_luma8();
        let img = fitted(&img, target);
        cache.push((i, img.clone()));
        sources.push(img);
    }
    // The whole set one way (as flipper_art's should_invert, over the play order's frames).
    let invert = should_invert(&sources, polarity);
    let frames: Vec<Vec<u8>> = sources
        .iter()
        .map(|img| to_bytes(&placed(img, target, invert), target))
        .collect();
    encode(&frames, fps, target)
}

/// Upright art scaled down to fit the target (as flipper_art's `fitted`), before the padding.
fn fitted(img: &GrayImage, target: ArtTarget) -> GrayImage {
    let (uw, uh) = target.upright();
    if img.width() > uw || img.height() > uh {
        let scale = f64::min(
            f64::from(uw) / f64::from(img.width()),
            f64::from(uh) / f64::from(img.height()),
        );
        let w = ((f64::from(img.width()) * scale) as u32).max(1);
        let h = ((f64::from(img.height()) * scale) as u32).max(1);
        return imageops::resize(img, w, h, imageops::FilterType::Triangle);
    }
    img.clone()
}

/// Fits upright art into the target (as flipper_art's `fit`), thresholds it and turns it to the
/// stored orientation.
#[cfg(test)]
fn stored(img: &GrayImage, target: ArtTarget) -> GrayImage {
    placed(&fitted(img, target), target, false)
}

/// A fitted source centred on the field (inverted first when it's drawn light on dark, so the
/// padding stays the field), thresholded and turned to the stored orientation.
fn placed(img: &GrayImage, target: ArtTarget, invert: bool) -> GrayImage {
    let (uw, uh) = target.upright();
    let mut img = img.clone();
    if invert {
        imageops::invert(&mut img);
    }
    let mut canvas = GrayImage::from_pixel(uw, uh, Luma([255]));
    imageops::overlay(
        &mut canvas,
        &img,
        i64::from((uw - img.width()) / 2),
        i64::from((uh - img.height()) / 2),
    );
    for p in canvas.pixels_mut() {
        p.0[0] = if p.0[0] >= 128 { 255 } else { 0 };
    }
    match target.rotation % 4 {
        1 => imageops::rotate90(&canvas),
        2 => imageops::rotate180(&canvas),
        3 => imageops::rotate270(&canvas),
        _ => canvas,
    }
}

/// Rows MSB first, a set bit for ink, a black pixel here (flipper_art's `to_lvgl_bytes`).
fn to_bytes(img: &GrayImage, target: ArtTarget) -> Vec<u8> {
    let stride = target.width.div_ceil(8) as usize;
    let mut out = vec![0u8; stride * target.height as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[0] == 0 {
            out[y as usize * stride + x as usize / 8] |= 0x80 >> (x % 8);
        }
    }
    out
}

/// Byte RLE (see flipper_art.py `rle`): control byte = op << 6 | (n - 1), n = 1..64.
/// 00 literal bytes follow, 01 zeros, 10 0xff, 11 the next byte repeated.
fn rle(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut lit: Vec<u8> = Vec::new();
    let flush = |out: &mut Vec<u8>, lit: &mut Vec<u8>| {
        for chunk in lit.chunks(64) {
            out.push((chunk.len() - 1) as u8);
            out.extend_from_slice(chunk);
        }
        lit.clear();
    };
    let mut i = 0;
    while i < data.len() {
        let mut j = i;
        while j < data.len() && data[j] == data[i] && j - i < 64 {
            j += 1;
        }
        let n = j - i;
        if n >= 3 || (n == 2 && (data[i] == 0 || data[i] == 0xff)) {
            flush(&mut out, &mut lit);
            match data[i] {
                0 => out.push(0x40 | (n - 1) as u8),
                0xff => out.push(0x80 | (n - 1) as u8),
                v => out.extend_from_slice(&[0xc0 | (n - 1) as u8, v]),
            }
            i = j;
        } else {
            lit.push(data[i]);
            i += 1;
        }
    }
    flush(&mut out, &mut lit);
    out
}

/// One set as a blob for `target`: header, play order, delta offsets, deltas (art_relay.h).
pub fn encode(frames: &[Vec<u8>], fps: u8, target: ArtTarget) -> Result<Vec<u8>, String> {
    let (Ok(w), Ok(h)) = (u8::try_from(target.width), u8::try_from(target.height)) else {
        return Err(format!(
            "a {}x{} screen is bigger than a blob describes",
            target.width, target.height
        ));
    };
    let mut unique: Vec<&Vec<u8>> = Vec::new();
    let mut order = Vec::with_capacity(frames.len());
    for f in frames {
        let i = unique.iter().position(|u| *u == f).unwrap_or_else(|| {
            unique.push(f);
            unique.len() - 1
        });
        order.push(u8::try_from(i).map_err(|_| "more than 256 unique frames")?);
    }
    if unique.is_empty() || order.len() > 255 {
        return Err("a set needs 1 to 255 frames".into());
    }
    let mut prev = vec![0u8; unique[0].len()];
    let deltas: Vec<Vec<u8>> = unique
        .iter()
        .map(|data| {
            let x: Vec<u8> = data.iter().zip(&prev).map(|(a, b)| a ^ b).collect();
            prev.clone_from(data);
            rle(&x)
        })
        .collect();
    let mut blob = b"NVA1".to_vec();
    blob.extend_from_slice(&[w, h, fps, FLAG_INK]);
    blob.extend_from_slice(&(deltas.len() as u16).to_le_bytes());
    blob.extend_from_slice(&(order.len() as u16).to_le_bytes());
    blob.extend_from_slice(&order);
    let mut at = blob.len() + 2 * deltas.len();
    for d in &deltas {
        blob.extend_from_slice(
            &u16::try_from(at)
                .map_err(|_| "too big for a blob")?
                .to_le_bytes(),
        );
        at += d.len();
    }
    for d in &deltas {
        blob.extend_from_slice(d);
    }
    if blob.len() > 0xffff {
        return Err(format!("{} bytes is more than a blob can hold", blob.len()));
    }
    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_ops_match_the_script() {
        assert_eq!(rle(&[0, 0, 0]), vec![0x42]);
        assert_eq!(rle(&[0xff, 0xff]), vec![0x81]);
        assert_eq!(rle(&[7, 7, 7, 7]), vec![0xc3, 7]);
        assert_eq!(rle(&[1, 2]), vec![0x01, 1, 2]);
        assert_eq!(rle(&[5; 70]), vec![0xff, 5, 0xc5, 5], "runs split at 64");
    }

    #[test]
    fn blobs_decode_with_the_previewer() {
        // a white frame with one black square, then the same frame again (one unique frame)
        let mut up = GrayImage::from_pixel(68, 140, Luma([255]));
        for y in 10..20 {
            for x in 5..15 {
                up.put_pixel(x, y, Luma([0]));
            }
        }
        let corne = ArtTarget::FIRMWARE;
        let f = to_bytes(&stored(&up, corne), corne);
        // #610: ink is a set bit, the field and the padding past column 139 are clear.
        // Columns 120-129 of row 5: byte 16 holds 128-135, so its top two bits.
        assert_eq!(f[5 * 18 + 16], 0xc0, "upright (5, 10..20) is ink");
        assert_eq!(
            f.iter().map(|b| b.count_ones()).sum::<u32>(),
            100,
            "the square only"
        );
        let blob = encode(&[f.clone(), f], 4, corne).unwrap();
        assert_eq!(&blob[..8], b"NVA1\x8c\x44\x04\x01", "flags: ink bits");
        assert_eq!(
            u16::from_le_bytes([blob[8], blob[9]]),
            1,
            "one unique frame"
        );
        assert_eq!(u16::from_le_bytes([blob[10], blob[11]]), 2, "played twice");
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.size, [140, 68]);
        // upright (5, 10) is stored at column 139 - 10, row 5; the black source square is ink,
        // shown lit on a dark field as on the keyboard (#610)
        assert_eq!(img.pixels[5 * 140 + (139 - 10)], egui::Color32::WHITE);
        assert_eq!(img.pixels[50 * 140 + 50], egui::Color32::BLACK);
    }

    /// #610: `scripts/flipper_art.py` and this encoder write the same blob for the same folder
    /// (`tests/art/ink-fixture`, two frames at the art screen's size). The Python side checks
    /// the same file in `scripts/test_flipper_art.py`.
    #[test]
    fn the_script_and_the_app_write_the_same_blob() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/art");
        let blob = encode_folder(
            &root.join("ink-fixture"),
            ArtTarget::FIRMWARE,
            Polarity::Auto,
        )
        .unwrap();
        let script = std::fs::read(root.join("ink-fixture.bin")).unwrap();
        assert_eq!(blob, script);
        assert_eq!(blob[7], FLAG_INK);
    }

    fn fixture(name: &str, polarity: Polarity) -> Vec<u8> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/art");
        encode_folder(&root.join(name), ArtTarget::FIRMWARE, polarity).unwrap()
    }

    /// #666: `dark-fixture` is `ink-fixture`'s negative, a light drawing on a dark field. Auto
    /// makes the same blob of both, the one the script writes (`scripts/test_flipper_art.py`
    /// checks the same); the override goes the way it says.
    #[test]
    fn a_dark_background_source_makes_the_same_blob_as_a_light_one() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/art");
        let script = std::fs::read(root.join("ink-fixture.bin")).unwrap();
        assert_eq!(fixture("dark-fixture", Polarity::Auto), script);
        assert_eq!(
            fixture("ink-fixture", Polarity::AsIs),
            script,
            "light: as is"
        );
        assert_eq!(fixture("dark-fixture", Polarity::Invert), script);
        let as_is = fixture("dark-fixture", Polarity::AsIs);
        assert_ne!(as_is, script);
        assert_eq!(as_is, fixture("ink-fixture", Polarity::Invert));
        // Inverted, the field is ink and the padding past column 139 stays clear.
        let img = super::super::art_preview::first_frame(&as_is).unwrap();
        assert_eq!(img.pixels[50 * 140 + (139 - 50)], egui::Color32::WHITE);
        let d = super::super::art_preview::decode(&as_is).unwrap();
        assert!(d.frames[0].chunks(18).all(|row| row[17] & 0x0f == 0));
    }

    /// #666 file fixtures for the hardware push checks: `dark-fixture.bin` is the dark-field
    /// version written as is (its field is the ink). Pushed as a file, auto flips it to show as
    /// `ink-fixture.bin` does; invert of the light one shows as the dark one as is.
    #[test]
    fn the_file_fixtures_normalise_to_the_same_picture() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/art");
        let light = std::fs::read(root.join("ink-fixture.bin")).unwrap();
        let dark = std::fs::read(root.join("dark-fixture.bin")).unwrap();
        let shown = |b: &[u8]| {
            super::super::art_preview::first_frame(b)
                .unwrap()
                .pixels
                .clone()
        };
        assert_ne!(shown(&dark), shown(&light), "the dark file as it is");
        assert_eq!(shown(&normalise_blob(&dark, Polarity::Auto)), shown(&light));
        assert_eq!(normalise_blob(&light, Polarity::Auto), light);
        assert_eq!(
            shown(&normalise_blob(&light, Polarity::Invert)),
            shown(&dark)
        );
    }

    fn img(
        w: u32,
        h: u32,
        field: u8,
        dark_box: Option<(u32, u32, u32, u32)>,
        fill: u8,
    ) -> GrayImage {
        let mut i = GrayImage::from_pixel(w, h, Luma([field]));
        if let Some((x0, y0, x1, y1)) = dark_box {
            for y in y0..y1 {
                for x in x0..x1 {
                    i.put_pixel(x, y, Luma([fill]));
                }
            }
        }
        i
    }

    /// #666: auto reads the edge, then on a tie the whole; a set goes one way (as the script's
    /// `test_auto_reads_the_edge_then_the_whole`).
    #[test]
    fn auto_reads_the_edge_then_the_whole() {
        let big = img(68, 140, 255, Some((1, 1, 67, 139)), 0);
        assert!(
            !should_invert(&[big], Polarity::Auto),
            "light field, big dark drawing"
        );
        let small = img(68, 140, 0, Some((20, 20, 30, 30)), 255);
        assert!(should_invert(&[small], Polarity::Auto), "dark field");
        let half = img(68, 140, 255, Some((0, 0, 34, 140)), 0);
        assert!(
            !should_invert(&[half], Polarity::Auto),
            "an edge tie, half dark: as is"
        );
        let more = img(68, 140, 255, Some((0, 0, 35, 140)), 0);
        assert!(should_invert(&[more], Polarity::Auto));
        let light = img(68, 140, 255, Some((20, 20, 30, 30)), 0);
        let dark = img(68, 140, 0, None, 0);
        assert!(!should_invert(
            &[light.clone(), light.clone(), dark.clone()],
            Polarity::Auto
        ));
        assert!(!should_invert(&[dark], Polarity::AsIs));
        assert!(should_invert(&[light], Polarity::Invert));
    }

    /// #666: an encoded blob (a file, a plugin's) is normalised by its ink flag alone: auto flips
    /// one whose ink is its background, and leaves one that's right; the override goes its way.
    #[test]
    fn blobs_are_normalised_by_their_ink_flag() {
        let good = fixture("ink-fixture", Polarity::Auto);
        let wrong = fixture("dark-fixture", Polarity::AsIs);
        assert_eq!(normalise_blob(&good, Polarity::Auto), good);
        let fixed = normalise_blob(&wrong, Polarity::Auto);
        assert_eq!(fixed[7], 0, "flag flipped");
        assert_eq!(fixed[8..], wrong[8..], "nothing else changes");
        let a = super::super::art_preview::first_frame(&fixed).unwrap();
        let b = super::super::art_preview::first_frame(&good).unwrap();
        assert_eq!(
            a.pixels, b.pixels,
            "shows as the light-background source does"
        );
        assert_eq!(normalise_blob(&wrong, Polarity::AsIs), wrong);
        assert_eq!(normalise_blob(&good, Polarity::Invert)[7], 0);
        assert_eq!(
            normalise_blob(b"junk", Polarity::Auto),
            b"junk",
            "left for the push to refuse"
        );
    }

    /// #666: the built-ins in the firmware need nothing: auto keeps each as it is.
    #[test]
    fn the_built_ins_are_already_normalised() {
        let src = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../boards/shields/nice_view_custom/widgets/custom_art.c"),
        )
        .unwrap();
        let mut sets = 0;
        for si in 0.. {
            let head = format!("static const uint8_t set_{si}_delta_0[] = {{");
            let Some(start) = src.find(&head) else {
                break;
            };
            let body = &src[start + head.len()..];
            let body = &body[..body.find("};").unwrap()];
            let delta: Vec<u8> = body
                .split(',')
                .filter_map(|t| u8::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok())
                .collect();
            let mut bits = vec![0u8; 18 * 68];
            super::super::art_preview::apply(&delta, &mut bits).unwrap();
            let c = Counts::of(140, 68, |x, y| {
                bits[y as usize * 18 + x as usize / 8] & (0x80 >> (x % 8)) != 0
            });
            assert!(
                !c.dark_background(),
                "built-in set {si}: auto would invert it"
            );
            sets += 1;
        }
        assert!(sets > 10, "found the built-ins ({sets})");
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;
    use crate::foxglove::fixtures::{second_desk, users_desk};
    use crate::foxglove::screen_roles::{first, of_board, Role};

    /// A folder of one frame: a white canvas with a black square at upright (5..15, 10..20).
    fn folder(name: &str, w: u32, h: u32) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("foxglove-art-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("meta.txt"), "Frame rate: 4\nFrames order: 0 0\n").unwrap();
        let mut img = GrayImage::from_pixel(w, h, Luma([255]));
        for y in 10..20 {
            for x in 5..15 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        img.save(d.join("frame_0.png")).unwrap();
        d
    }

    /// #560: the Corne's art screen from its profile is the firmware's format, byte for byte.
    #[test]
    fn the_cornes_art_screen_encodes_as_before() {
        let corne = of_board(&users_desk::board());
        let target = ArtTarget::of(first(&corne, Role::Art).unwrap()).unwrap();
        assert_eq!(target, ArtTarget::FIRMWARE);
        let d = folder("corne", 68, 140);
        let blob = encode_folder(&d, target, Polarity::Auto).unwrap();
        assert_eq!(&blob[..6], b"NVA1\x8c\x44");
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.size, [140, 68]);
        assert_eq!(img.pixels[5 * 140 + (139 - 10)], egui::Color32::WHITE);
        let _ = std::fs::remove_dir_all(d);
    }

    /// #560: the second desk's 160x80 art screen, upright: the set is made at its size, not
    /// turned, and the header says so.
    #[test]
    fn a_wide_upright_art_screen_gets_art_at_its_size() {
        let plank = of_board(&second_desk::board());
        let target = ArtTarget::of(first(&plank, Role::Art).unwrap()).unwrap();
        assert_eq!(
            target,
            ArtTarget {
                width: 160,
                height: 80,
                rotation: 0
            }
        );
        let d = folder("plank", 160, 80);
        let blob = encode_folder(&d, target, Polarity::Auto).unwrap();
        assert_eq!(&blob[..6], b"NVA1\xa0\x50");
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.size, [160, 80]);
        // Upright: the square is where it was drawn.
        assert_eq!(img.pixels[12 * 160 + 7], egui::Color32::WHITE);
        assert_eq!(img.pixels[50 * 160 + 100], egui::Color32::BLACK);
        // Too big for a blob's header: refused, not cut.
        let huge = ArtTarget {
            width: 320,
            height: 80,
            rotation: 0,
        };
        assert!(encode(&[vec![0; 40 * 80]], 1, huge).is_err());
        let _ = std::fs::remove_dir_all(d);
    }
}
