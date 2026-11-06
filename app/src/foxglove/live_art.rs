//! The right screen's piece for the live feed (`live_feed`, docs/live-feed.md): the frames it
//! plays, so a wallpaper can draw the literal art instead of a stand-in.
//!
//! - **One format:** whatever the piece came from (a library file, a built-in in the firmware,
//!   a kept place), it goes out as an NVA1 blob re-encoded **upright** (no turn, as the art is
//!   drawn), a set bit for ink, with its own play order and frames a second. Same delta + RLE
//!   format as the keyboard's, so a typical piece is a few KiB.
//! - **Once per change:** the hub sends it when the piece changes and to a wallpaper as it
//!   joins, never per frame; the wallpaper animates it on its own clock.
//! - **Bounded:** an upright blob over [`MAX_BLOB`] bytes goes out without its frames (the
//!   name only), and the wallpaper draws its stand-in.
//! - **Best effort for kept places:** the right half doesn't send a kept piece's bytes back, so
//!   its frames come from the library file of the same name and size, when there is one.

use std::path::PathBuf;

use base64::Engine;

use super::art_encode::ArtTarget;
use super::art_service::Snapshot;
use super::live_wire as wire;

/// The upright blob's limit: twice the right half's 16 KiB art slot (an upright re-encode packs a
/// little differently from the stored one), so about 32 KiB of base64 at most.
pub const MAX_BLOB: usize = 32 * 1024;

/// An animation as stored for its screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    /// The stored buffer's size (the art turned to the screen).
    pub width: usize,
    pub height: usize,
    /// Set bits are ink; without it the clear bits are (blobs from before #610).
    pub ink: bool,
    pub fps: u8,
    /// Indices into `unique`, in play order.
    pub order: Vec<u8>,
    /// Rows MSB first, `width.div_ceil(8)` bytes a row.
    pub unique: Vec<Vec<u8>>,
}

impl Stored {
    /// A blob's animation (`art_preview::decode`, plus its play order and rate).
    pub fn from_blob(blob: &[u8]) -> Result<Stored, &'static str> {
        let d = super::art_preview::decode(blob)?;
        let frames = usize::from(u16::from_le_bytes([blob[10], blob[11]]));
        Ok(Stored {
            width: d.width,
            height: d.height,
            ink: d.ink,
            fps: blob[6],
            order: blob[12..12 + frames].to_vec(),
            unique: d.frames,
        })
    }
}

/// How many quarter turns clockwise art stored at `width` x `height` was turned for its screen:
/// the connected keyboard's art screen when the size is its, else the firmware's, else none.
pub fn turns_for(width: usize, height: usize) -> u8 {
    let is = |t: ArtTarget| t.width as usize == width && t.height as usize == height;
    [ArtTarget::current(), ArtTarget::FIRMWARE]
        .into_iter()
        .find(|&t| is(t))
        .map_or(0, |t| t.rotation % 4)
}

/// The animation upright, as an NVA1 blob with the ink bit set (`art_encode::encode`).
pub fn upright(s: &Stored, turns: u8) -> Result<Vec<u8>, String> {
    let (sw, sh) = (s.width, s.height);
    let (uw, uh) = if turns % 2 == 1 { (sh, sw) } else { (sw, sh) };
    let (sstride, ustride) = (sw.div_ceil(8), uw.div_ceil(8));
    // Upright (x, y) is stored at (sx, sy): the inverse of `art_encode::placed`'s turn.
    let at = |x: usize, y: usize| match turns % 4 {
        1 => (uh - 1 - y, x),
        2 => (uw - 1 - x, uh - 1 - y),
        3 => (y, uw - 1 - x),
        _ => (x, y),
    };
    let turn = |bits: &Vec<u8>| -> Vec<u8> {
        let mut out = vec![0u8; ustride * uh];
        for y in 0..uh {
            for x in 0..uw {
                let (sx, sy) = at(x, y);
                let set = bits
                    .get(sy * sstride + sx / 8)
                    .is_some_and(|b| b & (0x80 >> (sx % 8)) != 0);
                if set == s.ink {
                    out[y * ustride + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        out
    };
    let unique: Vec<Vec<u8>> = s.unique.iter().map(turn).collect();
    let frames: Vec<Vec<u8>> = s
        .order
        .iter()
        .map(|&i| {
            unique
                .get(usize::from(i))
                .cloned()
                .ok_or("a frame past the set")
        })
        .collect::<Result<_, _>>()?;
    let target = ArtTarget {
        width: uw as u32,
        height: uh as u32,
        rotation: 0,
    };
    super::art_encode::encode(&frames, s.fps, target)
}

/// Where the piece on the right screen comes from, cheap to compare on every state change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A library file the right half plays from memory, with a hash of the bytes sent.
    File(PathBuf, u64),
    /// Built-in `index` on firmware with `count` built-ins.
    BuiltIn(i8, u8),
    /// A kept place, by its name and size.
    Slot { name: String, len: u16 },
}

/// What plays now (the same reading as the feed's `state.art` name).
pub fn source(s: &Snapshot) -> Option<Source> {
    if !s.connected {
        return None;
    }
    if let (Some(p), Some((_, hash))) = (s.showing_file(), s.sent.as_ref()) {
        return Some(Source::File(p.clone(), *hash));
    }
    let set = s.showing_built_in()?;
    let count = s.status.map_or(0, |st| st.right_set_count);
    let built_ins = i8::try_from(count).unwrap_or(i8::MAX);
    if set < built_ins || count == 0 {
        return (set >= 0).then_some(Source::BuiltIn(set, count));
    }
    let slot = u8::try_from(set - built_ins).ok()?;
    s.slots
        .iter()
        .find(|i| i.slot == slot && i.used)
        .map(|i| Source::Slot {
            name: i.name.clone(),
            len: i.len,
        })
}

/// The piece's animation as it was stored, from wherever it lives.
fn stored(src: &Source) -> Option<Stored> {
    match src {
        Source::File(path, _) => {
            Stored::from_blob(&super::art_service::pushed_bytes(path).ok()?).ok()
        }
        Source::BuiltIn(index, count) => super::art_builtins::animation(*index, *count),
        Source::Slot { name, len } => {
            // A kept place holds what was sent: the library file of that name and size.
            let lib = super::art_library::snapshot();
            let entry = lib.entries.iter().find(|e| {
                let stem = super::art_service::file_name(&e.path);
                e.bytes == usize::from(*len) && (stem == *name || stem.starts_with(name.as_str()))
            })?;
            Stored::from_blob(&super::art_service::pushed_bytes(&entry.path).ok()?).ok()
        }
    }
}

/// The feed's `art` body for a piece by `name` from `src`: its upright frames, or none.
pub fn art(src: &Source, name: String) -> wire::Art {
    let nva = stored(src).and_then(|s| encoded(&s));
    wire::Art { name, nva }
}

/// The upright blob as base64, within [`MAX_BLOB`].
pub fn encoded(s: &Stored) -> Option<String> {
    let blob = upright(s, turns_for(s.width, s.height)).ok()?;
    (blob.len() <= MAX_BLOB).then(|| base64::engine::general_purpose::STANDARD.encode(blob))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 140x68 stored frame (the Corne's screen, a quarter turn) with ink at upright (x, y).
    fn stored_with(points: &[(usize, usize)], ink: bool) -> Vec<u8> {
        let (sw, sh, uh) = (140, 68, 140);
        let stride = sw / 8 + 1;
        let mut bits = vec![if ink { 0 } else { 0xff }; stride * sh];
        for &(x, y) in points {
            let (sx, sy) = (uh - 1 - y, x);
            bits[sy * stride + sx / 8] ^= 0x80 >> (sx % 8);
        }
        bits
    }

    fn pixel(blob: &[u8], frame: usize, x: usize, y: usize) -> bool {
        let d = super::super::art_preview::decode(blob).unwrap();
        let w = d.width;
        d.frames[frame][y * w.div_ceil(8) + x / 8] & (0x80 >> (x % 8)) != 0
    }

    /// The literal frames come out upright and pixel-exact, ink as a set bit, with the set's own
    /// order and rate.
    #[test]
    fn frames_turn_upright_pixel_exact() {
        let a = stored_with(&[(0, 0), (67, 0), (5, 10), (67, 139)], true);
        let b = stored_with(&[(1, 1)], true);
        let s = Stored {
            width: 140,
            height: 68,
            ink: true,
            fps: 6,
            order: vec![0, 1, 0],
            unique: vec![a, b],
        };
        let blob = upright(&s, 1).unwrap();
        assert_eq!(
            &blob[..8],
            b"NVA1\x44\x8c\x06\x01",
            "68x140 upright, 6 fps, ink"
        );
        assert_eq!(&blob[10..15], &[3, 0, 0, 1, 0], "the play order kept");
        let d = super::super::art_preview::decode(&blob).unwrap();
        let ink: usize = d.frames[0].iter().map(|b| b.count_ones() as usize).sum();
        assert_eq!(ink, 4, "only the four points");
        for (x, y) in [(0, 0), (67, 0), (5, 10), (67, 139)] {
            assert!(pixel(&blob, 0, x, y), "({x}, {y})");
        }
        assert!(pixel(&blob, 1, 1, 1) && !pixel(&blob, 1, 0, 0));
    }

    /// A blob from before #610 (clear bits are ink) reads the same as an inked one.
    #[test]
    fn old_polarity_is_normalised() {
        let inked = Stored {
            width: 140,
            height: 68,
            ink: true,
            fps: 4,
            order: vec![0],
            unique: vec![stored_with(&[(3, 4)], true)],
        };
        let old = Stored {
            ink: false,
            unique: vec![stored_with(&[(3, 4)], false)],
            ..inked.clone()
        };
        assert_eq!(upright(&inked, 1), upright(&old, 1));
    }

    /// A file's blob round-trips: stored by the encoder, read back upright as drawn.
    #[test]
    fn a_blob_reads_back_as_drawn() {
        let corne = ArtTarget::FIRMWARE;
        let frame = stored_with(&[(10, 20), (11, 20)], true);
        let blob = super::super::art_encode::encode(&[frame.clone(), frame], 4, corne).unwrap();
        let s = Stored::from_blob(&blob).unwrap();
        assert_eq!(
            (s.width, s.height, s.fps, s.order.clone()),
            (140, 68, 4, vec![0, 0])
        );
        let up = upright(&s, turns_for(s.width, s.height)).unwrap();
        assert!(pixel(&up, 0, 10, 20) && pixel(&up, 0, 11, 20) && !pixel(&up, 0, 12, 20));
        assert!(encoded(&s).is_some_and(|b| b.len() < 400));
    }

    /// Every built-in decodes whole, and its upright blob fits the bound.
    #[test]
    fn built_ins_send_their_frames() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../boards/shields/nice_view_custom/widgets/custom_art.json"
        ))
        .unwrap();
        let count = manifest["set_count"].as_u64().unwrap() as u8;
        for i in 0..count {
            let s = super::super::art_builtins::animation(i as i8, count).unwrap();
            let set = &manifest["sets"][usize::from(i)];
            assert_eq!(s.order.len() as u64, set["frames"].as_u64().unwrap());
            assert_eq!(u64::from(s.fps), set["fps"].as_u64().unwrap());
            let b64 = encoded(&s).expect("within the bound");
            assert!(b64.len() <= MAX_BLOB.div_ceil(3) * 4);
        }
        assert!(super::super::art_builtins::animation(0, count + 1).is_none());
    }

    /// Past the bound: no frames, the name only.
    #[test]
    fn too_big_goes_without_frames() {
        // 255 unique frames of noise: far past 32 KiB however it packs.
        let mut seed = 1u32;
        let unique: Vec<Vec<u8>> = (0..255)
            .map(|_| {
                (0..18 * 68)
                    .map(|_| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        (seed >> 16) as u8
                    })
                    .collect()
            })
            .collect();
        let s = Stored {
            width: 140,
            height: 68,
            ink: true,
            fps: 8,
            order: (0..255).collect(),
            unique,
        };
        assert!(encoded(&s).is_none());
    }

    #[test]
    fn the_source_follows_what_plays() {
        use foxglove_link::protocol::{right_state, ArtStatus, SlotInfo};
        let mut s = Snapshot::default();
        assert_eq!(source(&s), None, "no keyboard");
        s.connected = true;
        s.status = Some(ArtStatus {
            right_state: 0,
            right_set: 0,
            right_set_count: 1,
            ..Default::default()
        });
        assert_eq!(source(&s), Some(Source::BuiltIn(0, 1)));
        s.slots = vec![SlotInfo {
            slot: 0,
            used: true,
            writes: 1,
            len: 900,
            name: "Fox run".into(),
        }];
        s.status = Some(ArtStatus {
            right_set: 1,
            ..s.status.unwrap()
        });
        assert_eq!(
            source(&s),
            Some(Source::Slot {
                name: "Fox run".into(),
                len: 900
            })
        );
        s.sent = Some((PathBuf::from("C:/art/Fox run.bin"), 7));
        s.status = Some(ArtStatus {
            right_state: right_state::PLAYING,
            ..s.status.unwrap()
        });
        assert_eq!(
            source(&s),
            Some(Source::File(PathBuf::from("C:/art/Fox run.bin"), 7))
        );
    }
}
