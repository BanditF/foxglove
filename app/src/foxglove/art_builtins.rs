//! Pictures of the pieces that come with the keyboard (#516): the first frame of each built-in
//! set, read from the firmware's own `custom_art.c` (the file the right half is built from,
//! next to the `custom_art.json` manifest `art_manifest` reads). No second copy of the art to
//! keep in step: a firmware art change is a change to this file.
//!
//! Only used when the manifest matches the keyboard (`art_manifest::sets`); a keyboard running
//! other firmware gets named placeholders instead of the wrong pictures.

use std::sync::{Arc, OnceLock};

const SOURCE: &str = include_str!("../../../boards/shields/nice_view_custom/widgets/custom_art.c");

/// Frames as stored by the firmware this file builds (its art format, 1 bit, MSB first).
const W: usize = super::art_encode::ArtTarget::FIRMWARE.width as usize;
const H: usize = super::art_encode::ArtTarget::FIRMWARE.height as usize;

/// A built-in's first frame, in `art_preview::first_frame`'s form (set bits white).
#[derive(Clone)]
pub struct Picture {
    /// A content key for the texture cache.
    pub key: u64,
    pub image: Arc<egui::ColorImage>,
}

/// The picture of built-in `index` on a keyboard reporting `set_count` built-ins.
pub fn picture(index: i8, set_count: u8) -> Option<Picture> {
    super::art_manifest::sets(set_count)?;
    let all = PICTURES.get_or_init(|| parse(SOURCE));
    if all.len() != usize::from(set_count) {
        return None;
    }
    all.get(usize::try_from(index).ok()?)?.clone()
}

static PICTURES: OnceLock<Vec<Option<Picture>>> = OnceLock::new();

/// The bytes of `static const uint8_t <name>[] = { 0x.., ... };`.
fn array(src: &str, name: &str) -> Option<Vec<u8>> {
    let head = format!("static const uint8_t {name}[] = {{");
    let start = src.find(&head)? + head.len();
    let body = &src[start..start + src[start..].find("};")?];
    body.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| match t.strip_prefix("0x") {
            Some(hex) => u8::from_str_radix(hex, 16).ok(),
            None => t.parse().ok(),
        })
        .collect()
}

/// Every set's first frame, in the order the firmware lists them.
fn parse(src: &str) -> Vec<Option<Picture>> {
    let count = src
        .split("const int custom_art_set_count = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    (0..count.min(128)).map(|n| first_frame(src, n)).collect()
}

/// Set `n`'s first frame: its unique frames are XOR deltas applied in turn, and the play
/// order's first entry names the one shown first.
fn first_frame(src: &str, n: usize) -> Option<Picture> {
    let first = *array(src, &format!("set_{n}_order"))?.first()?;
    let stride = W.div_ceil(8);
    let mut bits = vec![0u8; stride * H];
    for k in 0..=usize::from(first) {
        let delta = array(src, &format!("set_{n}_delta_{k}"))?;
        super::art_preview::apply(&delta, &mut bits).ok()?;
    }
    let key = super::art_library::blob_hash(&bits);
    let pixels = (0..H)
        .flat_map(|y| {
            let bits = &bits;
            (0..W).map(move |x| {
                if bits[y * stride + x / 8] & (0x80 >> (x % 8)) == 0 {
                    egui::Color32::BLACK
                } else {
                    egui::Color32::WHITE
                }
            })
        })
        .collect();
    Some(Picture {
        key,
        image: Arc::new(egui::ColorImage::new([W, H], pixels)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every built-in the bundled manifest names has a picture, and it isn't blank.
    #[test]
    fn every_built_in_has_a_picture() {
        let all = parse(SOURCE);
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../boards/shields/nice_view_custom/widgets/custom_art.json"
        ))
        .unwrap();
        let count = manifest["set_count"].as_u64().unwrap() as usize;
        assert_eq!(all.len(), count, "custom_art.c and custom_art.json agree");
        for (i, p) in all.iter().enumerate() {
            let p = p.as_ref().unwrap_or_else(|| panic!("set {i} decodes"));
            assert_eq!(p.image.size, [W, H]);
            let lit = p
                .image
                .pixels
                .iter()
                .filter(|c| **c == egui::Color32::WHITE)
                .count();
            assert!(lit > 0 && lit < W * H, "set {i} has a picture, not a blank");
        }
        assert!(picture(0, count as u8).is_some());
        assert!(
            picture(0, count as u8 + 1).is_none(),
            "other firmware: no pictures"
        );
        assert!(picture(-1, count as u8).is_none());
    }

    #[test]
    fn arrays_and_deltas_parse() {
        let src = "static const uint8_t set_0_delta_0[] = {\n    0x80, 0x00,\n};\n\
                   static const uint8_t set_0_order[] = {0};\n\
                   const int custom_art_set_count = 1;";
        assert_eq!(array(src, "set_0_delta_0"), Some(vec![0x80, 0x00]));
        assert_eq!(array(src, "set_0_order"), Some(vec![0]));
        assert_eq!(array(src, "set_1_order"), None);
        // 0x80 0x00 is not a whole frame: the decoder refuses it, so no picture.
        assert!(parse(src)[0].is_none());
    }
}
