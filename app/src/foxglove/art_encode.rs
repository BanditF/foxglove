//! Builds an NVA1 art blob from a Flipper-style frame folder (`meta.txt` + `frame_N.png`), as
//! `scripts/flipper_art.py` does, so the art library can list and push source folders such as
//! `art-src\bandit\out\Bandit_idle` without Python (#59).
//!
//! Same rules as the script: each frame is fitted upright into the art screen (never upscaled,
//! centred), thresholded at 50% (white = a set bit; the keyboard shows black source pixels lit),
//! stored turned as the screen is mounted, and each unique frame is an RLE-packed XOR delta
//! against the previous one. The screen's size and turn come from its descriptor (#560).

use std::path::Path;

use image::{imageops, GrayImage, Luma};

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

/// The folder's frames encoded as one pushable blob for `target`.
pub fn encode_folder(folder: &Path, target: ArtTarget) -> Result<Vec<u8>, String> {
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
    let mut cache: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut frames = Vec::with_capacity(order.len());
    for &i in &order {
        if let Some((_, f)) = cache.iter().find(|(n, _)| *n == i) {
            frames.push(f.clone());
            continue;
        }
        let path = folder.join(format!("frame_{i}.png"));
        let img = image::open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_luma8();
        let stored = to_bytes(&stored(&img, target), target);
        cache.push((i, stored.clone()));
        frames.push(stored);
    }
    encode(&frames, fps, target)
}

/// Fits upright art into the target (as flipper_art's `fit`), thresholds it and turns it to the
/// stored orientation.
fn stored(img: &GrayImage, target: ArtTarget) -> GrayImage {
    let (uw, uh) = target.upright();
    let mut img = img.clone();
    if img.width() > uw || img.height() > uh {
        let scale = f64::min(
            f64::from(uw) / f64::from(img.width()),
            f64::from(uh) / f64::from(img.height()),
        );
        let w = ((f64::from(img.width()) * scale) as u32).max(1);
        let h = ((f64::from(img.height()) * scale) as u32).max(1);
        img = imageops::resize(&img, w, h, imageops::FilterType::Triangle);
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

/// Rows MSB first, a set bit for white (flipper_art's `to_lvgl_bytes`).
fn to_bytes(img: &GrayImage, target: ArtTarget) -> Vec<u8> {
    let stride = target.width.div_ceil(8) as usize;
    let mut out = vec![0u8; stride * target.height as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[0] != 0 {
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
    blob.extend_from_slice(&[w, h, fps, 0]);
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
        let blob = encode(&[f.clone(), f], 4, corne).unwrap();
        assert_eq!(&blob[..8], b"NVA1\x8c\x44\x04\x00");
        assert_eq!(
            u16::from_le_bytes([blob[8], blob[9]]),
            1,
            "one unique frame"
        );
        assert_eq!(u16::from_le_bytes([blob[10], blob[11]]), 2, "played twice");
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.size, [140, 68]);
        // upright (5, 10) is stored at column 139 - 10, row 5; black source = a clear bit
        assert_eq!(img.pixels[5 * 140 + (139 - 10)], egui::Color32::BLACK);
        assert_eq!(img.pixels[50 * 140 + 50], egui::Color32::WHITE);
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
        let blob = encode_folder(&d, target).unwrap();
        assert_eq!(&blob[..6], b"NVA1\x8c\x44");
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.size, [140, 68]);
        assert_eq!(img.pixels[5 * 140 + (139 - 10)], egui::Color32::BLACK);
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
        let blob = encode_folder(&d, target).unwrap();
        assert_eq!(&blob[..6], b"NVA1\xa0\x50");
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.size, [160, 80]);
        // Upright: the square is where it was drawn.
        assert_eq!(img.pixels[12 * 160 + 7], egui::Color32::BLACK);
        assert_eq!(img.pixels[50 * 160 + 100], egui::Color32::WHITE);
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
