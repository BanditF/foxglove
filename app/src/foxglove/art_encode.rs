//! Builds an NVA1 art blob from a Flipper-style frame folder (`meta.txt` + `frame_N.png`), as
//! `scripts/flipper_art.py` does, so the art library can list and push source folders such as
//! `art-src\bandit\out\Bandit_idle` without Python (#59).
//!
//! Same rules as the script: each frame is fitted upright into 68x140 (never upscaled, centred),
//! thresholded at 50% (white = a set bit; the keyboard shows black source pixels lit), stored
//! turned 90° clockwise, and each unique frame is an RLE-packed XOR delta against the previous one.

use std::path::Path;

use image::{imageops, GrayImage, Luma};

/// The stored buffer: 140 wide, 68 high (the upright art is 68x140).
const ART_W: u32 = 140;
const ART_H: u32 = 68;

/// A folder holds one set when it has a `meta.txt`.
pub fn is_set(folder: &Path) -> bool {
    folder.join("meta.txt").is_file()
}

/// The folder's frames encoded as one pushable blob.
pub fn encode_folder(folder: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::read_to_string(folder.join("meta.txt")).map_err(|e| format!("meta.txt: {e}"))?;
    let value = |key: &str| {
        meta.lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| v.trim().to_owned())
    };
    let order: Vec<usize> = value("Frames order")
        .map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect())
        .filter(|o: &Vec<usize>| !o.is_empty())
        .unwrap_or_else(|| vec![0]);
    let fps: u8 = value("Frame rate").and_then(|v| v.parse().ok()).unwrap_or(2);
    let mut cache: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut frames = Vec::with_capacity(order.len());
    for &i in &order {
        if let Some((_, f)) = cache.iter().find(|(n, _)| *n == i) {
            frames.push(f.clone());
            continue;
        }
        let path = folder.join(format!("frame_{i}.png"));
        let img = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_luma8();
        let stored = to_bytes(&stored(&img));
        cache.push((i, stored.clone()));
        frames.push(stored);
    }
    encode(&frames, fps)
}

/// Fits upright art into 68x140 (as flipper_art's `fit`), thresholds it and turns it to the
/// stored orientation.
fn stored(img: &GrayImage) -> GrayImage {
    let (uw, uh) = (ART_H, ART_W);
    let mut img = img.clone();
    if img.width() > uw || img.height() > uh {
        let scale = f64::min(f64::from(uw) / f64::from(img.width()), f64::from(uh) / f64::from(img.height()));
        let w = ((f64::from(img.width()) * scale) as u32).max(1);
        let h = ((f64::from(img.height()) * scale) as u32).max(1);
        img = imageops::resize(&img, w, h, imageops::FilterType::Triangle);
    }
    let mut canvas = GrayImage::from_pixel(uw, uh, Luma([255]));
    imageops::overlay(&mut canvas, &img, i64::from((uw - img.width()) / 2), i64::from((uh - img.height()) / 2));
    for p in canvas.pixels_mut() {
        p.0[0] = if p.0[0] >= 128 { 255 } else { 0 };
    }
    imageops::rotate90(&canvas)
}

/// Rows MSB first, a set bit for white (flipper_art's `to_lvgl_bytes`).
fn to_bytes(img: &GrayImage) -> Vec<u8> {
    let stride = ART_W.div_ceil(8) as usize;
    let mut out = vec![0u8; stride * ART_H as usize];
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

/// One set as a blob: header, play order, delta offsets, deltas (art_relay.h).
pub fn encode(frames: &[Vec<u8>], fps: u8) -> Result<Vec<u8>, String> {
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
    blob.extend_from_slice(&[ART_W as u8, ART_H as u8, fps, 0]);
    blob.extend_from_slice(&(deltas.len() as u16).to_le_bytes());
    blob.extend_from_slice(&(order.len() as u16).to_le_bytes());
    blob.extend_from_slice(&order);
    let mut at = blob.len() + 2 * deltas.len();
    for d in &deltas {
        blob.extend_from_slice(&u16::try_from(at).map_err(|_| "too big for a blob")?.to_le_bytes());
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
        let f = to_bytes(&stored(&up));
        let blob = encode(&[f.clone(), f], 4).unwrap();
        assert_eq!(&blob[..8], b"NVA1\x8c\x44\x04\x00");
        assert_eq!(u16::from_le_bytes([blob[8], blob[9]]), 1, "one unique frame");
        assert_eq!(u16::from_le_bytes([blob[10], blob[11]]), 2, "played twice");
        let img = super::super::art_preview::first_frame(&blob).unwrap();
        assert_eq!(img.size, [140, 68]);
        // upright (5, 10) is stored at column 139 - 10, row 5; black source = a clear bit
        assert_eq!(img.pixels[5 * 140 + (139 - 10)], egui::Color32::BLACK);
        assert_eq!(img.pixels[50 * 140 + 50], egui::Color32::WHITE);
    }
}
