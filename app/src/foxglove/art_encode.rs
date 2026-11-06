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

    /// [`ArtTarget::of`], or why the screen can't take art (#340): a format other than 1 bit,
    /// or a size an NVA1 header can't hold.
    pub fn try_of(screen: &super::screen_roles::Screen) -> Result<ArtTarget, String> {
        let the = super::parts::upper_first(&screen.the());
        if screen.format != "1bit" {
            return Err(format!(
                "{the} takes {} pictures; art is 1-bit.",
                screen.format
            ));
        }
        ArtTarget::of(screen).ok_or_else(|| {
            let (w, h) = screen.upright();
            format!("{the} is {w}x{h}; art can be at most 255x255.")
        })
    }

    /// The older firmware's art screen (its board's profile, #563), else
    /// [`ArtTarget::FIRMWARE`].
    pub fn old_firmware() -> ArtTarget {
        use super::screen_roles::{first, Role};
        first(&super::screen_roles::old_firmware(), Role::Art)
            .and_then(ArtTarget::of)
            .unwrap_or(ArtTarget::FIRMWARE)
    }

    /// The connected keyboard's art target (#340): its art screen (the firmware's description,
    /// else its profile's); with no art screen said, the older firmware's. `Err` says why the
    /// keyboard's art screen can't take art.
    pub fn resolve() -> Result<ArtTarget, String> {
        #[cfg(test)]
        if let Some(screens) = TEST_SCREENS.with_borrow(|s| s.clone()) {
            return Self::resolve_in(Some(&screens));
        }
        Self::resolve_in(super::screen_roles::current().as_deref())
    }

    fn resolve_in(screens: Option<&[super::screen_roles::Screen]>) -> Result<ArtTarget, String> {
        use super::screen_roles::{first, Role};
        match screens.and_then(|s| first(s, Role::Art)) {
            Some(screen) => ArtTarget::try_of(screen),
            None => Ok(ArtTarget::old_firmware()),
        }
    }

    /// The connected keyboard's art screen, else the older firmware's (its board's profile,
    /// #563), else [`ArtTarget::FIRMWARE`].
    pub fn current() -> ArtTarget {
        ArtTarget::resolve().unwrap_or_else(|_| ArtTarget::old_firmware())
    }

    /// The art's size upright, as drawn.
    pub fn upright(self) -> (u32, u32) {
        if self.rotation % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

#[cfg(test)]
thread_local! {
    /// The previews' keyboard screens (#340), so a made-up board's art screen can be drawn
    /// without touching the process-wide board.
    pub static TEST_SCREENS: std::cell::RefCell<Option<Vec<super::screen_roles::Screen>>> =
        const { std::cell::RefCell::new(None) };
}

/// A folder holds one set when it has a `meta.txt`.
pub fn is_set(folder: &Path) -> bool {
    folder.join("meta.txt").is_file()
}

/// How a piece was made to fit the art screen (#340). The rule, the same for every board: art
/// drawn at the screen's size goes as it is; bigger art is scaled down evenly until it fits;
/// smaller art keeps its size. Either way it is centred and the rest is the background. Art is
/// never stretched one way more than the other, and never scaled up (so the Corne's art, and
/// what `scripts/flipper_art.py` writes, stay as they were).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Drawn at the screen's size.
    AsMade,
    /// From the set's `<w>x<h>` folder, drawn for this screen.
    Variant,
    /// Drawn smaller (upright `from`): centred at its size.
    Centred { from: (u32, u32) },
    /// Drawn bigger (upright `from`): scaled down to fit, centred.
    ScaledDown { from: (u32, u32) },
}

impl Fit {
    /// For a piece not made for `target`, the rule that fitted it, as the art tile's hover says.
    pub fn note(self, target: ArtTarget) -> Option<String> {
        let (uw, uh) = target.upright();
        let (from, how) = match self {
            Fit::AsMade | Fit::Variant => return None,
            Fit::Centred { from } => (from, "centred at its size"),
            Fit::ScaledDown { from } => (from, "scaled down evenly to fit, centred"),
        };
        Some(format!(
            "Drawn at {}x{}, not for this board's {uw}x{uh} art screen: {how}, with the \
             background round it. A {uw}x{uh} folder in the set is used instead when there is one.",
            from.0, from.1
        ))
    }

    /// How a source of upright `from` fits `target`.
    fn of(from: (u32, u32), target: ArtTarget) -> Fit {
        let (uw, uh) = target.upright();
        if from == (uw, uh) {
            Fit::AsMade
        } else if from.0 > uw || from.1 > uh {
            Fit::ScaledDown { from }
        } else {
            Fit::Centred { from }
        }
    }

    /// The fit of a set from the fits of its frames: the first that isn't as made.
    fn of_all(fits: impl IntoIterator<Item = Fit>) -> Fit {
        let fits: Vec<Fit> = fits.into_iter().collect();
        fits.iter()
            .copied()
            .find(|f| matches!(f, Fit::ScaledDown { .. }))
            .or_else(|| fits.iter().copied().find(|f| *f != Fit::AsMade))
            .unwrap_or(Fit::AsMade)
    }
}

/// The folder of a set drawn for `target` (#340): `<set>/<w>x<h>`, upright as drawn (e.g.
/// `Bandit_idle/144x112`), when it is a set.
pub fn variant(folder: &Path, target: ArtTarget) -> Option<std::path::PathBuf> {
    let (uw, uh) = target.upright();
    let v = folder.join(format!("{uw}x{uh}"));
    is_set(&v).then_some(v)
}

/// The folder's frames encoded as one pushable blob for `target`, its ink normalised to
/// `polarity` (#666).
pub fn encode_folder(
    folder: &Path,
    target: ArtTarget,
    polarity: Polarity,
) -> Result<Vec<u8>, String> {
    encode_set(folder, target, polarity).map(|(blob, _)| blob)
}

/// [`encode_folder`], plus how the set was fitted (#340): from its variant for `target` when it
/// has one, else from its own frames by the [`Fit`] rule.
pub fn encode_set(
    folder: &Path,
    target: ArtTarget,
    polarity: Polarity,
) -> Result<(Vec<u8>, Fit), String> {
    match variant(folder, target) {
        Some(v) => encode_frames(&v, target, polarity).map(|(blob, _)| (blob, Fit::Variant)),
        None => encode_frames(folder, target, polarity),
    }
}

fn encode_frames(
    folder: &Path,
    target: ArtTarget,
    polarity: Polarity,
) -> Result<(Vec<u8>, Fit), String> {
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
    let mut fits = Vec::new();
    for &i in &order {
        if let Some((_, f)) = cache.iter().find(|(n, _)| *n == i) {
            sources.push(f.clone());
            continue;
        }
        let path = folder.join(format!("frame_{i}.png"));
        let img = image::open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_luma8();
        fits.push(Fit::of(img.dimensions(), target));
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
    Ok((encode(&frames, fps, target)?, Fit::of_all(fits)))
}

/// The start of the error when a piece can't go on the art screen (#340); the reason follows.
pub const DOES_NOT_FIT: &str = "doesn't fit the art screen: ";

/// A blob as it is shown and sent to the art screen `target` (#340, [`fit_blob`]). A blob that
/// doesn't decode goes as it is, for the push to refuse as before; one that can't be made for
/// the screen (or a screen that can't take art) is an error starting [`DOES_NOT_FIT`].
pub fn for_screen(
    blob: &[u8],
    target: &Result<ArtTarget, String>,
) -> Result<(Vec<u8>, Fit), String> {
    if super::art_preview::decode(blob).is_err() {
        return Ok((blob.to_vec(), Fit::AsMade));
    }
    let target = target.as_ref().map_err(|e| format!("{DOES_NOT_FIT}{e}"))?;
    fit_blob(blob, *target).map_err(|e| format!("{DOES_NOT_FIT}{e}"))
}

/// How many quarter turns a blob stored at `width` x `height` was made with: the older
/// firmware's art screen's when it is that size (the art made before #340), else none.
fn blob_turns(width: u32, height: u32) -> u8 {
    let old = ArtTarget::old_firmware();
    if (old.width, old.height) == (width, height) {
        old.rotation
    } else {
        0
    }
}

/// An encoded blob (a library file, a bundled set) for `target` (#340): as it is when it was
/// made at the target's stored size (so a blob for this screen is never touched), else its
/// frames upright, fitted by the [`Fit`] rule and encoded again, with the same play order and
/// rate. `Err` when it doesn't decode or can't be encoded for the screen.
pub fn fit_blob(blob: &[u8], target: ArtTarget) -> Result<(Vec<u8>, Fit), String> {
    let d = super::art_preview::decode(blob).map_err(str::to_owned)?;
    let (w, h) = (d.width as u32, d.height as u32);
    if (w, h) == (target.width, target.height) {
        return Ok((blob.to_vec(), Fit::AsMade));
    }
    let turns = blob_turns(w, h);
    let stored = super::live_art::Stored::from_blob(blob).map_err(str::to_owned)?;
    // Upright, ink black on white (a source as drawn), so the fit treats it as any source.
    let up = super::live_art::upright(&stored, turns)?;
    let u = super::art_preview::decode(&up).map_err(str::to_owned)?;
    let (uw, uh) = (u.width as u32, u.height as u32);
    let stride = u.width.div_ceil(8);
    let images: Vec<GrayImage> = u
        .frames
        .iter()
        .map(|bits| {
            GrayImage::from_fn(uw, uh, |x, y| {
                let (x, y) = (x as usize, y as usize);
                let set = bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0;
                Luma([if set == u.ink { 0 } else { 255 }])
            })
        })
        .collect();
    let unique: Vec<Vec<u8>> = images
        .iter()
        .map(|img| to_bytes(&placed(&fitted(img, target), target, false), target))
        .collect();
    let frames: Vec<Vec<u8>> = stored
        .order
        .iter()
        .map(|&i| {
            unique
                .get(usize::from(i))
                .cloned()
                .ok_or("a frame past the set")
        })
        .collect::<Result<_, _>>()?;
    Ok((
        encode(&frames, stored.fps, target)?,
        Fit::of((uw, uh), target),
    ))
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
        // One built-in since #671 (Bandit_Emblem); every set the file declares was checked.
        let declared = format!("const int custom_art_set_count = {sets};");
        assert!(
            sets >= 1 && src.contains(&declared),
            "found the built-ins ({sets})"
        );
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

    /// A made-up board's art screen (#340): `w` x `h` stored, `rotation` turns, `format`.
    fn screen(w: u16, h: u16, rotation: u8, format: &str) -> Screen {
        Screen {
            id: 0,
            role: Role::Art,
            name: "Art screen".into(),
            width: w,
            height: h,
            format: format.into(),
            rotation,
        }
    }

    use crate::foxglove::screen_roles::Screen;

    /// Ink pixels in a blob's first frame, as shown.
    fn ink(blob: &[u8]) -> usize {
        let img = super::super::art_preview::first_frame(blob).unwrap();
        img.pixels
            .iter()
            .filter(|p| **p == egui::Color32::WHITE)
            .count()
    }

    /// #340: the Corne's art comes out byte for byte as before: every bundled blob and the
    /// fixtures go to its art screen untouched, and a folder made at its size is as made.
    #[test]
    fn the_cornes_art_is_byte_identical() {
        let corne = of_board(&users_desk::board());
        let target = ArtTarget::resolve_in(Some(&corne));
        assert_eq!(target, Ok(ArtTarget::FIRMWARE));
        assert_eq!(ArtTarget::resolve_in(None), Ok(ArtTarget::FIRMWARE));
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/art");
        let fixtures = ["ink-fixture.bin", "dark-fixture.bin"]
            .map(|f| std::fs::read(root.join(f)).unwrap())
            .to_vec();
        let bundled = super::super::art_pieces::BUNDLED
            .iter()
            .map(|(_, b, _)| b.to_vec());
        for blob in fixtures.into_iter().chain(bundled) {
            assert_eq!(
                for_screen(&blob, &target).unwrap(),
                (blob.clone(), Fit::AsMade)
            );
        }
        let (blob, fit) = encode_set(
            &root.join("ink-fixture"),
            ArtTarget::FIRMWARE,
            Polarity::Auto,
        )
        .unwrap();
        assert_eq!(blob, std::fs::read(root.join("ink-fixture.bin")).unwrap());
        assert_eq!(fit, Fit::AsMade);
        assert_eq!(fit.note(ArtTarget::FIRMWARE), None);
    }

    /// #340: a screen that can't take art is refused with the reason, never cut or drawn wrong.
    #[test]
    fn a_screen_that_cant_take_art_says_why() {
        let wide = ArtTarget::resolve_in(Some(&[screen(320, 80, 0, "1bit")])).unwrap_err();
        assert!(
            wide.contains("320x80") && wide.contains("255x255"),
            "{wide}"
        );
        let colour = ArtTarget::resolve_in(Some(&[screen(80, 160, 0, "rgb565")])).unwrap_err();
        assert!(
            colour.contains("rgb565") && colour.contains("1-bit"),
            "{colour}"
        );
        let fury = super::super::art_pieces::BUNDLED[0].1;
        let refused = for_screen(fury, &Err(wide.clone())).unwrap_err();
        assert_eq!(refused, format!("{DOES_NOT_FIT}{wide}"));
        assert_eq!(
            super::super::art_words::problem(&refused, None),
            super::super::art_words::Problem::DoesNotFit
        );
        // A file that isn't art goes as it is, for the push to refuse as before.
        assert_eq!(
            for_screen(b"junk", &Err(wide)).unwrap(),
            (b"junk".to_vec(), Fit::AsMade)
        );
    }

    /// #340: a one-piece board's 144x112 upright art screen (a Toucan-like target): Corne-sized
    /// art is too tall, so it is scaled down evenly to fit and centred, and says so.
    #[test]
    fn taller_art_is_scaled_down_on_a_wide_screen() {
        let target = ArtTarget::try_of(&screen(144, 112, 0, "1bit")).unwrap();
        let d = folder("toucan", 68, 140);
        let (blob, fit) = encode_set(&d, target, Polarity::Auto).unwrap();
        assert_eq!(&blob[..6], b"NVA1\x90\x70");
        assert_eq!(fit, Fit::ScaledDown { from: (68, 140) });
        let note = fit.note(target).unwrap();
        assert!(
            note.contains("68x140") && note.contains("144x112"),
            "{note}"
        );
        // The bundled Corne art, fitted the same way: the screen's size, the same play order.
        let fury = super::super::art_encode::normalise_blob(
            super::super::art_pieces::BUNDLED[0].1,
            Polarity::Auto,
        );
        let (fitted, fit) = fit_blob(&fury, target).unwrap();
        assert_eq!(fit, Fit::ScaledDown { from: (68, 140) });
        let d2 = super::super::art_preview::decode(&fitted).unwrap();
        assert_eq!((d2.width, d2.height), (144, 112));
        assert_eq!(fitted[10..12], fury[10..12], "as many frames played");
        assert_eq!(fitted[6], fury[6], "at the same rate");
        // Uniform: the picture is 112 tall and (68 * 112 / 140 =) 54 wide, centred, so the
        // columns outside 45..99 are the background.
        let img = super::super::art_preview::first_frame(&fitted).unwrap();
        let lit_at = |x: usize| (0..112).any(|y| img.pixels[y * 144 + x] == egui::Color32::WHITE);
        assert!((0..45).chain(99..144).all(|x| !lit_at(x)));
        let _ = std::fs::remove_dir_all(d);
    }

    /// #340: a taller e-paper-like 88x184 art screen: Corne art fits, so it keeps its size,
    /// centred with the background round it; every ink pixel survives.
    #[test]
    fn smaller_art_is_centred_at_its_size() {
        let target = ArtTarget::try_of(&screen(88, 184, 0, "1bit")).unwrap();
        let fury = super::super::art_encode::normalise_blob(
            super::super::art_pieces::BUNDLED[0].1,
            Polarity::Auto,
        );
        let (fitted, fit) = fit_blob(&fury, target).unwrap();
        assert_eq!(fit, Fit::Centred { from: (68, 140) });
        assert_eq!(&fitted[..6], b"NVA1\x58\xb8");
        assert_eq!(ink(&fitted), ink(&fury), "nothing scaled, nothing lost");
        // A folder made at 68x140 does the same; the upright square sits 10 in and 22 down.
        let d = folder("halcyon", 68, 140);
        let (blob, fit) = encode_set(&d, target, Polarity::Auto).unwrap();
        assert_eq!(fit, Fit::Centred { from: (68, 140) });
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.pixels[(22 + 10) * 88 + 10 + 5], egui::Color32::WHITE);
        assert_eq!(img.pixels[(22 + 9) * 88 + 10 + 5], egui::Color32::BLACK);
        let _ = std::fs::remove_dir_all(d);
    }

    /// #340: a set with a folder drawn for the board's screen (`<w>x<h>`, upright) uses it.
    #[test]
    fn a_sets_variant_for_the_screen_wins() {
        let target = ArtTarget::try_of(&screen(144, 112, 0, "1bit")).unwrap();
        let d = folder("variant", 68, 140);
        let v = d.join("144x112");
        std::fs::create_dir_all(&v).unwrap();
        std::fs::write(v.join("meta.txt"), "Frame rate: 4\nFrames order: 0\n").unwrap();
        GrayImage::from_pixel(144, 112, Luma([0]))
            .save(v.join("frame_0.png"))
            .unwrap();
        assert_eq!(variant(&d, target), Some(v));
        let (blob, fit) = encode_set(&d, target, Polarity::AsIs).unwrap();
        assert_eq!(fit, Fit::Variant);
        assert_eq!(fit.note(target), None, "made for it");
        assert_eq!(ink(&blob), 144 * 112, "the variant's all-ink frame");
        // The Corne has no 68x140 folder in it: its own frames, as made.
        assert_eq!(variant(&d, ArtTarget::FIRMWARE), None);
        let _ = std::fs::remove_dir_all(d);
    }
}
