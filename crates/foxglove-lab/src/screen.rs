//! Screen capture abstraction and exact masked comparison of 160x68, MSB-first frames.
use crate::spec::Rect;
use anyhow::{ensure, Result};
use image::{GrayImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;

pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 68;
pub const BYTES: usize = 1360;

/// Future firmware adapters provide a consistent snapshot; no guessed commands are sent.
pub trait Capture {
    fn capture(&mut self, half: &str) -> Result<Vec<u8>>;
}

pub fn bitmap(bits: &[u8]) -> Result<GrayImage> {
    ensure!(bits.len() == BYTES, "screen must contain 1360 bytes");
    Ok(GrayImage::from_fn(WIDTH, HEIGHT, |x, y| {
        image::Luma([
            if bits[(y * 20 + x / 8) as usize] & (0x80 >> (x % 8)) != 0 {
                0
            } else {
                255
            },
        ])
    }))
}
pub fn png(image: &GrayImage) -> Result<Vec<u8>> {
    let scaled = image::imageops::resize(image, WIDTH * 4, HEIGHT * 4, image::imageops::Nearest);
    let mut out = Cursor::new(Vec::new());
    scaled.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

pub struct Comparison {
    pub different: usize,
    pub actual: Vec<u8>,
    pub diff: Vec<u8>,
}

/// Goldens are native 160x68 or exported 640x272 black/white PNGs.
pub fn compare(
    capture: &mut impl Capture,
    half: &str,
    golden: &[u8],
    mask: &[Rect],
) -> Result<Comparison> {
    let actual = bitmap(&capture.capture(half)?)?;
    let mut golden = image::load_from_memory_with_format(golden, ImageFormat::Png)?.to_luma8();
    if golden.dimensions() == (WIDTH * 4, HEIGHT * 4) {
        // Ensure exported pixels really are uniform 4x4 blocks before reducing.
        let native = image::imageops::resize(&golden, WIDTH, HEIGHT, image::imageops::Nearest);
        ensure!(
            image::imageops::resize(&native, WIDTH * 4, HEIGHT * 4, image::imageops::Nearest)
                == golden,
            "golden has nonuniform scaled pixels"
        );
        golden = native;
    }
    ensure!(
        golden.dimensions() == (WIDTH, HEIGHT),
        "golden must be 160x68 or 640x272"
    );
    ensure!(
        golden.pixels().all(|p| p[0] == 0 || p[0] == 255),
        "golden must be black/white"
    );
    let mut different = 0;
    let diff = RgbImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let masked = mask.iter().any(|r| {
            x >= r.x
                && x < r.x.saturating_add(r.width)
                && y >= r.y
                && y < r.y.saturating_add(r.height)
        });
        if !masked && actual[(x, y)] != golden[(x, y)] {
            different += 1;
            Rgb([255, 0, 0])
        } else {
            Rgb([actual[(x, y)][0]; 3])
        }
    });
    let diff = image::imageops::resize(&diff, WIDTH * 4, HEIGHT * 4, image::imageops::Nearest);
    let mut out = Cursor::new(Vec::new());
    diff.write_to(&mut out, ImageFormat::Png)?;
    Ok(Comparison {
        different,
        actual: png(&actual)?,
        diff: out.into_inner(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake(Vec<u8>);
    impl Capture for Fake {
        fn capture(&mut self, _: &str) -> Result<Vec<u8>> {
            Ok(self.0.clone())
        }
    }
    #[test]
    fn bit_order_scale_mask_and_diff() {
        let mut b = vec![0; BYTES];
        b[0] = 0x81;
        b[20] = 0x80;
        let image = bitmap(&b).unwrap();
        assert_eq!(image[(0, 0)][0], 0);
        assert_eq!(image[(1, 0)][0], 255);
        assert_eq!(image[(7, 0)][0], 0);
        assert_eq!(image[(0, 1)][0], 0);
        let white = png(&bitmap(&vec![0; BYTES]).unwrap()).unwrap();
        let c = compare(&mut Fake(b.clone()), "left", &white, &[]).unwrap();
        assert_eq!(c.different, 3);
        assert_eq!(image::load_from_memory(&c.diff).unwrap().width(), 640);
        assert_eq!(
            compare(
                &mut Fake(b.clone()),
                "left",
                &white,
                &[Rect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 2
                }]
            )
            .unwrap()
            .different,
            0
        );
        assert_eq!(
            compare(&mut Fake(b), "left", &png(&image).unwrap(), &[])
                .unwrap()
                .different,
            0
        );
        assert!(bitmap(&[0]).is_err());
    }
}
