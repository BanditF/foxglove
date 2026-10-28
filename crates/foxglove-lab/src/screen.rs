//! Screen snapshots from the firmware (D7 06 SCREEN_SNAP / D7 07 SCREEN_READ) and exact,
//! masked comparison. Both halves' screens are mounted portrait, so everything a person looks
//! at (PNGs, goldens, masks, regions) is in portrait 68x160 coordinates: the native 160x68
//! image rotated 90 degrees counterclockwise, portrait (x, y) = (native y, 159 - native x).
use crate::spec::Rect;
use anyhow::{bail, ensure, Result};
use foxglove_link::{protocol, Transport};
use image::{GrayImage, ImageFormat, Rgb, RgbImage};
use std::{io::Cursor, time::Duration};

/// Native panel size and image bytes (20 bytes per row, MSB first, 1 = black ink).
pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 68;
pub const BYTES: usize = 1360;
/// What a person sees.
pub const PORTRAIT_W: u32 = HEIGHT;
pub const PORTRAIT_H: u32 = WIDTH;
const SCALE: u32 = 4;

const SNAP: u8 = 0x06;
const READ: u8 = 0x07;

/// Why a snapshot couldn't be taken, by firmware status.
#[derive(Debug, PartialEq, Eq)]
pub enum SnapError {
    NoRightLink,
    RightTimeout,
    NotBuiltIn,
    BadRequest,
    NoSnapshot,
    Busy,
    Other(u8),
}
impl std::fmt::Display for SnapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoRightLink => "no right relay/screen link",
            Self::RightTimeout => "right half timed out",
            Self::NotBuiltIn => "screen readback not built into this firmware",
            Self::BadRequest => "bad request",
            Self::NoSnapshot => "no snapshot yet",
            Self::Busy => "still busy after retries",
            Self::Other(_) => "unknown status",
        })
    }
}
impl std::error::Error for SnapError {}
fn status_error(status: u8) -> SnapError {
    match status {
        1 => SnapError::Busy,
        2 => SnapError::NoRightLink,
        3 => SnapError::RightTimeout,
        4 => SnapError::NotBuiltIn,
        5 => SnapError::BadRequest,
        6 => SnapError::NoSnapshot,
        s => SnapError::Other(s),
    }
}

/// Takes a fresh snapshot of `half` (0 left, 1 right) and reads it out, checking the CRC.
/// Returns the native 1360-byte image.
pub fn snapshot(t: &impl Transport, half: u8) -> Result<Vec<u8>> {
    let request = |report: &[u8], cmd: u8, timeout| -> Result<[u8; protocol::REPORT]> {
        for _ in 0..50 {
            let r = t.request(report, protocol::SETTINGS, cmd, timeout)?;
            if r[2] != 1 {
                return Ok(r);
            }
            std::thread::sleep(Duration::from_millis(100)); // busy: capture or transfer running
        }
        Err(SnapError::Busy.into())
    };
    // The right reply waits for the relay transfer (up to 3 s in the firmware).
    let meta = request(
        &[protocol::SETTINGS, SNAP, half],
        SNAP,
        Duration::from_secs(6),
    )?;
    if meta[2] != 0 {
        return Err(status_error(meta[2]).into());
    }
    ensure!(meta[3] == half, "snapshot reply is for half {}", meta[3]);
    let (w, h) = (meta[6], meta[7]);
    let bytes = usize::from(u16::from_le_bytes([meta[8], meta[9]]));
    let crc = u32::from_le_bytes([meta[10], meta[11], meta[12], meta[13]]);
    ensure!(
        u32::from(w) == WIDTH && u32::from(h) == HEIGHT && bytes == BYTES,
        "unexpected snapshot {w}x{h}, {bytes} bytes"
    );
    let mut image = Vec::with_capacity(BYTES);
    while image.len() < BYTES {
        let offset = image.len() as u16;
        let [lo, hi] = offset.to_le_bytes();
        let r = request(
            &[protocol::SETTINGS, READ, half, lo, hi],
            READ,
            Duration::from_secs(2),
        )?;
        if r[2] != 0 {
            return Err(status_error(r[2]).into());
        }
        ensure!(
            r[3] == half && u16::from_le_bytes([r[4], r[5]]) == offset,
            "read reply for half {} offset {}, wanted {half}/{offset}",
            r[3],
            u16::from_le_bytes([r[4], r[5]])
        );
        let n = usize::from(r[6]);
        if n == 0 || 7 + n > protocol::REPORT || image.len() + n > BYTES {
            bail!("bad read length {n} at offset {offset}");
        }
        image.extend_from_slice(&r[7..7 + n]);
    }
    ensure!(
        crc32fast::hash(&image) == crc,
        "snapshot CRC mismatch (screen changed mid-read?)"
    );
    Ok(image)
}

/// The native image as a person sees it: 68 wide, 160 tall, black ink on white.
pub fn portrait(bits: &[u8]) -> Result<GrayImage> {
    ensure!(bits.len() == BYTES, "screen must contain 1360 bytes");
    let native = GrayImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let ink = bits[(y * 20 + x / 8) as usize] & (0x80 >> (x % 8)) != 0;
        image::Luma([if ink { 0 } else { 255 }])
    });
    Ok(image::imageops::rotate270(&native))
}

/// A PNG scaled up 4x so single pixels are visible in the report.
pub fn png(image: &GrayImage) -> Result<Vec<u8>> {
    let (w, h) = image.dimensions();
    let scaled = image::imageops::resize(image, w * SCALE, h * SCALE, image::imageops::Nearest);
    let mut out = Cursor::new(Vec::new());
    scaled.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn inside(rects: &[Rect], x: u32, y: u32) -> bool {
    rects.iter().any(|r| {
        x >= r.x && x < r.x.saturating_add(r.width) && y >= r.y && y < r.y.saturating_add(r.height)
    })
}

/// Pixels that differ between two portrait images, counting only inside `region` (all when
/// None).
pub fn changed(a: &GrayImage, b: &GrayImage, region: Option<&Rect>) -> usize {
    a.enumerate_pixels()
        .filter(|(x, y, p)| {
            region.is_none_or(|r| inside(std::slice::from_ref(r), *x, *y)) && b[(*x, *y)] != **p
        })
        .count()
}

/// A binary PBM (P4): width, height and the packed rows (MSB first, rows padded to bytes).
pub fn parse_pbm(data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let text_end = data
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .map(|(i, _)| i)
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("not a P4 PBM"))?;
    let header = std::str::from_utf8(&data[..text_end])?;
    let mut parts = header.split_whitespace();
    ensure!(parts.next() == Some("P4"), "not a P4 PBM");
    let w: u32 = parts.next().unwrap_or_default().parse()?;
    let h: u32 = parts.next().unwrap_or_default().parse()?;
    let bits = data[text_end + 1..].to_vec();
    ensure!(
        w > 0 && h > 0 && bits.len() == (w.div_ceil(8) * h) as usize,
        "PBM size doesn't match {w}x{h}"
    );
    Ok((w, h, bits))
}

/// Where a packed w x h frame (1 = ink) appears exactly in a portrait capture, if anywhere.
pub fn find(image: &GrayImage, w: u32, h: u32, bits: &[u8]) -> Option<(u32, u32)> {
    let (iw, ih) = image.dimensions();
    if w > iw || h > ih {
        return None;
    }
    let stride = w.div_ceil(8);
    let ink = |x: u32, y: u32| bits[(y * stride + x / 8) as usize] & (0x80 >> (x % 8)) != 0;
    (0..=ih - h)
        .flat_map(|oy| (0..=iw - w).map(move |ox| (ox, oy)))
        .find(|&(ox, oy)| {
            (0..h).all(|y| (0..w).all(|x| (image[(ox + x, oy + y)][0] == 0) == ink(x, y)))
        })
}

pub struct Comparison {
    pub different: usize,
    pub diff: Vec<u8>,
}

/// Compares a portrait capture with a golden PNG (68x160, or 272x640 as exported by `png`),
/// ignoring masked rectangles. The diff image marks differing pixels red.
pub fn compare(actual: &GrayImage, golden: &[u8], mask: &[Rect]) -> Result<Comparison> {
    let mut golden = image::load_from_memory_with_format(golden, ImageFormat::Png)?.to_luma8();
    let (w, h) = (PORTRAIT_W, PORTRAIT_H);
    if golden.dimensions() == (w * SCALE, h * SCALE) {
        let native = image::imageops::resize(&golden, w, h, image::imageops::Nearest);
        ensure!(
            image::imageops::resize(&native, w * SCALE, h * SCALE, image::imageops::Nearest)
                == golden,
            "golden has nonuniform scaled pixels"
        );
        golden = native;
    }
    ensure!(
        golden.dimensions() == (w, h),
        "golden must be 68x160 or 272x640 (portrait)"
    );
    ensure!(
        golden.pixels().all(|p| p[0] == 0 || p[0] == 255),
        "golden must be black/white"
    );
    let mut different = 0;
    let diff = RgbImage::from_fn(w, h, |x, y| {
        if !inside(mask, x, y) && actual[(x, y)] != golden[(x, y)] {
            different += 1;
            Rgb([255, 0, 0])
        } else {
            Rgb([actual[(x, y)][0]; 3])
        }
    });
    let diff = image::imageops::resize(&diff, w * SCALE, h * SCALE, image::imageops::Nearest);
    let mut out = Cursor::new(Vec::new());
    diff.write_to(&mut out, ImageFormat::Png)?;
    Ok(Comparison {
        different,
        diff: out.into_inner(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_matches_the_firmware_formula() {
        // portrait (x, y) = (native y, 159 - native x)
        let mut b = vec![0; BYTES];
        b[0] = 0x80; // native (0, 0)
        b[2 * 20 + 19] = 0x01; // native (159, 2)
        let p = portrait(&b).unwrap();
        assert_eq!(p.dimensions(), (68, 160));
        assert_eq!(p[(0, 159)][0], 0);
        assert_eq!(p[(2, 0)][0], 0);
        assert_eq!(p.pixels().filter(|q| q[0] == 0).count(), 2);
        assert!(portrait(&[0]).is_err());
    }

    #[test]
    fn compare_masks_and_scaled_goldens() {
        let mut b = vec![0; BYTES];
        b[0] = 0x80;
        let actual = portrait(&b).unwrap();
        let white = png(&portrait(&vec![0; BYTES]).unwrap()).unwrap();
        let c = compare(&actual, &white, &[]).unwrap();
        assert_eq!(c.different, 1);
        assert_eq!(image::load_from_memory(&c.diff).unwrap().width(), 272);
        let mask = Rect {
            x: 0,
            y: 150,
            width: 10,
            height: 10,
        };
        assert_eq!(compare(&actual, &white, &[mask]).unwrap().different, 0);
        assert_eq!(
            compare(&actual, &png(&actual).unwrap(), &[])
                .unwrap()
                .different,
            0
        );
    }

    #[test]
    fn finds_a_dumped_frame_on_the_screen() {
        let mut bits = vec![0; BYTES];
        // native (159 - 10, 20) is portrait (20, 10)
        let x = 149;
        bits[20 * 20 + x / 8] |= 0x80 >> (x % 8);
        let screen = portrait(&bits).unwrap();
        let mut pbm = b"P4\n8 2\n".to_vec();
        pbm.extend([0x00, 0x08]); // ink at (4, 1)
        let (w, h, frame) = parse_pbm(&pbm).unwrap();
        assert_eq!(find(&screen, w, h, &frame), Some((16, 9)));
        assert!(parse_pbm(b"P4\n8 2\n\x00").is_err());
        let empty = portrait(&vec![0; BYTES]).unwrap();
        assert_eq!(find(&empty, w, h, &frame), None);
    }

    #[test]
    fn change_counts_only_inside_the_region() {
        let a = portrait(&vec![0; BYTES]).unwrap();
        let mut bits = vec![0; BYTES];
        bits[0] = 0x80; // portrait (0, 159)
        let b = portrait(&bits).unwrap();
        assert_eq!(changed(&a, &b, None), 1);
        let top = Rect {
            x: 0,
            y: 0,
            width: 68,
            height: 20,
        };
        assert_eq!(changed(&a, &b, Some(&top)), 0);
    }
}
