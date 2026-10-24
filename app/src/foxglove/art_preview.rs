//! NVA1 thumbnail decoding. The stored bitmap is row-major, MSB first.

pub fn first_frame(blob: &[u8]) -> Result<egui::ColorImage, &'static str> {
    let bad = "Invalid NVA1 art blob";
    if blob.len() < 12 || blob.len() > 65535 || &blob[..4] != b"NVA1" {
        return Err(bad);
    }
    let w = blob[4] as usize;
    let h = blob[5] as usize;
    let unique = u16::from_le_bytes([blob[8], blob[9]]) as usize;
    let frames = u16::from_le_bytes([blob[10], blob[11]]) as usize;
    let table = 12 + frames;
    let end = table + unique * 2;
    if w == 0 || h == 0 || unique == 0 || unique > 256 || frames == 0 || end >= blob.len() {
        return Err(bad);
    }
    if blob[12..table].iter().any(|&n| n as usize >= unique) {
        return Err(bad);
    }
    let offsets: Vec<usize> = blob[table..end]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
        .collect();
    if offsets[0] != end
        || offsets.iter().any(|&n| n >= blob.len())
        || offsets.windows(2).any(|p| p[0] >= p[1])
    {
        return Err(bad);
    }
    let stride = w.div_ceil(8);
    let mut bits = vec![0; stride * h];
    let mut first = None;
    for (index, &start) in offsets.iter().enumerate() {
        let stop = offsets.get(index + 1).copied().unwrap_or(blob.len());
        apply(&blob[start..stop], &mut bits)?;
        if index == blob[12] as usize {
            first = Some(bits.clone());
        }
    }
    let bits = first.ok_or(bad)?;
    let pixels = (0..h)
        .flat_map(|y| {
            let bits = &bits;
            (0..w).map(move |x| {
                if bits[y * stride + x / 8] & (0x80 >> (x % 8)) == 0 {
                    egui::Color32::BLACK
                } else {
                    egui::Color32::WHITE
                }
            })
        })
        .collect();
    Ok(egui::ColorImage::new([w, h], pixels))
}

fn apply(mut data: &[u8], bits: &mut [u8]) -> Result<(), &'static str> {
    let bad = "Invalid NVA1 delta";
    let mut pos = 0;
    while let Some((&control, rest)) = data.split_first() {
        data = rest;
        let count = (control & 63) as usize + 1;
        let out = bits.get_mut(pos..pos + count).ok_or(bad)?;
        match control >> 6 {
            0 => {
                let literal = data.get(..count).ok_or(bad)?;
                for (b, v) in out.iter_mut().zip(literal) {
                    *b ^= v;
                }
                data = &data[count..];
            }
            1 => {}
            2 => {
                for b in out {
                    *b ^= 255;
                }
            }
            _ => {
                let (&value, rest) = data.split_first().ok_or(bad)?;
                data = rest;
                for b in out {
                    *b ^= value;
                }
            }
        }
        pos += count;
    }
    if pos != bits.len() {
        return Err(bad);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_ops_and_xor() {
        let mut bits = [0x55; 5];
        apply(&[0, 0x12, 0x40, 0x80, 0xc1, 0x34], &mut bits).unwrap();
        assert_eq!(bits, [0x47, 0x55, 0xaa, 0x61, 0x61]);
        assert!(apply(&[0], &mut bits).is_err());
        assert!(apply(&[0x7f], &mut bits).is_err());
        assert!(apply(&[], &mut bits).is_err());
    }
    #[test]
    fn play_order_and_truncated_blobs() {
        let blob = [
            b'N', b'V', b'A', b'1', 8, 1, 1, 0, 2, 0, 1, 0, 1, 17, 0, 18, 0, 0x80, 0x80,
        ];
        let image = first_frame(&blob).unwrap();
        assert!(image.pixels.iter().all(|p| *p == egui::Color32::BLACK));
        for end in 0..blob.len() {
            assert!(first_frame(&blob[..end]).is_err());
        }
        let mut invalid = blob;
        invalid[12] = 2;
        assert!(first_frame(&invalid).is_err());
    }
}
