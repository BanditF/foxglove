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
//! - **Kept places by content:** the right half doesn't send a kept piece's bytes back, only its
//!   name, length and (#863) CRC. Its frames come from the PC piece with that CRC
//!   (`art_pieces::holding`; by name and length on firmware without CRCs). With none, the art
//!   goes without frames and says why (`missing`), so the wallpaper labels its placeholder.
//! - **Kept places this PC has no file for** (#878): on firmware with HELLO v2 `SLOT_READ`, the
//!   art service reads the place's bytes back once (`art_service::read_kept`, asked by the feed
//!   while a wallpaper watches), checks them against the place's CRC, and keeps them by CRC
//!   beside the library ([`keep`], `art/kept-<crc>.bin`, not in the library's list). A later
//!   place with the same bytes finds them there with no read.
//! - **Built-ins from the firmware's own source:** `art_builtins` reads `custom_art.c`, the file
//!   the right half is compiled from, so the feed's frames are the firmware's by construction;
//!   `built_in_frames_are_the_firmwares` checks every frame pixel for pixel.

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
/// the connected keyboard's art screen when the size is its, else the older firmware's (its
/// board's profile), else none.
pub fn turns_for(width: usize, height: usize) -> u8 {
    let is = |t: ArtTarget| t.width as usize == width && t.height as usize == height;
    [ArtTarget::current(), ArtTarget::old_firmware()]
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
    /// A kept place, by its name, size and CRC (#863; `None` from older firmware).
    Slot {
        name: String,
        len: u16,
        crc: Option<u32>,
    },
    /// A pushed set the right half plays that Foxglove can't name (#1044): no frames.
    Pushed,
}

impl Source {
    /// The feed's `art.from`: where the piece lives.
    pub fn from(&self) -> &'static str {
        match self {
            Source::File(..) | Source::Pushed => wire::FROM_FILE,
            Source::BuiltIn(..) => wire::FROM_BUILT_IN,
            Source::Slot { .. } => wire::FROM_KEPT,
        }
    }
}

/// What plays now (the same reading as the feed's `state.art` name).
pub fn source(s: &Snapshot) -> Option<Source> {
    if !s.connected {
        return None;
    }
    if let (Some(p), Some((_, hash))) = (s.showing_file(), s.sent.as_ref()) {
        return Some(Source::File(p.clone(), *hash));
    }
    if s.status.map(|st| super::art_words::showing(&st)) == Some(super::art_words::Showing::Sent) {
        return Some(Source::Pushed);
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
            crc: i.crc,
        })
}

/// The piece's animation as it was stored, from wherever it lives, or why Foxglove has none.
fn stored(src: &Source) -> Result<Stored, &'static str> {
    match src {
        Source::File(path, _) => super::art_service::pushed_bytes(path)
            .ok()
            .and_then(|b| Stored::from_blob(&b).ok())
            .ok_or(wire::MISSING_UNREADABLE),
        Source::Pushed => Err(wire::MISSING_PUSHED_UNKNOWN),
        Source::BuiltIn(index, count) => {
            super::art_builtins::animation(*index, *count).ok_or(wire::MISSING_OTHER_FIRMWARE)
        }
        Source::Slot { name, len, crc } => {
            let info = foxglove_link::protocol::SlotInfo {
                slot: 0,
                used: true,
                writes: 0,
                len: *len,
                name: name.clone(),
                crc: *crc,
            };
            kept_in(&info, &super::art_library::snapshot())
                .or_else(|why| or_read_back(why, *crc, kept_copy))
        }
    }
}

/// Bumped each time a kept place's bytes read back land ([`keep`]), so the feed reads its piece
/// again.
static KEPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many read-back copies have landed this run.
pub fn kept_generation() -> u64 {
    KEPT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Where the bytes of the kept set with `crc` go once read back.
fn kept_path(crc: u32) -> Option<PathBuf> {
    super::art_library::folder().map(|d| d.join(super::art_library::kept_copy_name(crc)))
}

/// Whether this PC already holds the read-back bytes of the set with `crc`.
pub fn has_kept_copy(crc: u32) -> bool {
    kept_copy(crc).is_some()
}

/// The read-back bytes of the set with `crc`, when they are here and still match it.
fn kept_copy(crc: u32) -> Option<Vec<u8>> {
    let bytes = std::fs::read(kept_path(crc)?).ok()?;
    (foxglove_link::protocol::blob_crc(&bytes) == crc).then_some(bytes)
}

/// Keeps a kept place's bytes read back from the keyboard (#878). Bytes that don't match `crc`
/// (the place was written meanwhile, or the read went wrong) are dropped: the placeholder stays.
pub fn keep(crc: u32, bytes: &[u8]) -> Result<PathBuf, String> {
    if foxglove_link::protocol::blob_crc(bytes) != crc {
        return Err(format!(
            "the bytes read don't match the place's CRC {crc:08x}"
        ));
    }
    Stored::from_blob(bytes).map_err(|e| format!("the bytes read aren't art: {e}"))?;
    let path = kept_path(crc).ok_or("no settings folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    KEPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(path)
}

/// A kept place with no PC piece (`not_on_pc`): its bytes read back earlier, if `copy` has
/// them for its CRC; any other reason stands.
fn or_read_back(
    why: &'static str,
    crc: Option<u32>,
    copy: impl Fn(u32) -> Option<Vec<u8>>,
) -> Result<Stored, &'static str> {
    match crc {
        Some(crc) if why == wire::MISSING_NOT_ON_PC => copy(crc)
            .filter(|b| foxglove_link::protocol::blob_crc(b) == crc)
            .and_then(|b| Stored::from_blob(&b).ok())
            .ok_or(why),
        _ => Err(why),
    }
}

/// A kept place's animation from the pieces in `library` (and the sets bundled with the app).
fn kept_in(
    info: &foxglove_link::protocol::SlotInfo,
    library: &super::art_library::Library,
) -> Result<Stored, &'static str> {
    let pcs = super::art_pieces::pc_pieces(super::art_pieces::found(library).found, &|_| false);
    kept_stored(info, &pcs, |p| super::art_pieces::piece_blob(p, library))
}

/// A kept place's animation from the PC piece holding its bytes ([`super::art_pieces::holding`]),
/// its bytes read by `blob`: `not_on_pc` when this PC has no such piece.
fn kept_stored(
    info: &foxglove_link::protocol::SlotInfo,
    pcs: &[super::art_pieces::PcPiece],
    blob: impl Fn(&super::art_pieces::PcPiece) -> Option<Vec<u8>>,
) -> Result<Stored, &'static str> {
    let piece = super::art_pieces::holding(info, pcs).ok_or(wire::MISSING_NOT_ON_PC)?;
    let bytes = blob(&pcs[piece]).ok_or(wire::MISSING_UNREADABLE)?;
    Stored::from_blob(&bytes).map_err(|_| wire::MISSING_UNREADABLE)
}

/// The feed's `art` body for a piece by `name` from `src`: its upright frames, or none and why.
/// It plays at the art screen's pace (#833), as the keyboard plays it: a built-in or a kept
/// piece too, whose bytes the app didn't send.
pub fn art(src: &Source, name: String) -> wire::Art {
    body(src.from(), name, stored(src))
}

/// The `art` body for an animation read from `from`, paced and turned upright.
fn body(from: &'static str, name: String, stored: Result<Stored, &'static str>) -> wire::Art {
    let pace = super::art_encode::Pace::current();
    let nva = stored.and_then(|s| encoded(&pace.stored(s)).ok_or(wire::MISSING_TOO_BIG));
    wire::Art {
        name,
        from,
        missing: nva.as_ref().err().copied(),
        nva: nva.ok(),
    }
}

/// The upright blob as base64, within [`MAX_BLOB`].
pub fn encoded(s: &Stored) -> Option<String> {
    let blob = upright(s, turns_for(s.width, s.height)).ok()?;
    (blob.len() <= MAX_BLOB).then(|| base64::engine::general_purpose::STANDARD.encode(blob))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1044: a pushed set Foxglove has no record of (set -1 after a restart) goes out as a
    /// piece without frames and says why, instead of no art at all.
    #[test]
    fn an_unknown_pushed_set_says_pushed_unknown() {
        let s = Snapshot {
            connected: true,
            status: Some(foxglove_link::protocol::ArtStatus {
                link: 2,
                right_state: foxglove_link::protocol::right_state::PLAYING,
                right_set: -1,
                right_set_count: 6,
                ..Default::default()
            }),
            ..Snapshot::default()
        };
        let src = source(&s).expect("a piece");
        assert_eq!(src, Source::Pushed);
        // On the wire, as the feed sends it (#1075: the feed named it "" while the API's words
        // were right, and this test used the API's).
        let (state, art) = super::super::live_feed::art_messages(&s);
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        let art: serde_json::Value = serde_json::from_str(&art).unwrap();
        assert_eq!(
            art,
            serde_json::json!({"t": "art", "art": {"name": "A piece from this PC", "from": "file",
                "nva": null, "missing": "pushed_unknown"}})
        );
        assert_eq!(
            state["art"],
            serde_json::Value::Null,
            "no state.art name for it"
        );
    }

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
            crc: None,
        }];
        s.status = Some(ArtStatus {
            right_set: 1,
            ..s.status.unwrap()
        });
        assert_eq!(
            source(&s),
            Some(Source::Slot {
                name: "Fox run".into(),
                len: 900,
                crc: None,
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

    /// Upright pixel (x, y) of a frame stored a quarter turn clockwise (140x68, the Corne's).
    fn stored_pixel(bits: &[u8], x: usize, y: usize) -> bool {
        let (sx, sy) = (139 - y, x);
        bits[sy * 140usize.div_ceil(8) + sx / 8] & (0x80 >> (sx % 8)) != 0
    }

    /// The feed's built-in frames are the firmware's: `custom_art.c` (the file the right half
    /// is compiled from) decoded on its own and turned upright here equals every frame the
    /// wallpaper gets, in the firmware's play order and at its rate, pixel for pixel.
    #[test]
    fn built_in_frames_are_the_firmwares() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../boards/shields/nice_view_custom/widgets/custom_art.json"
        ))
        .unwrap();
        let count = manifest["set_count"].as_u64().unwrap() as u8;
        let firmware =
            include_str!("../../../boards/shields/nice_view_custom/widgets/custom_art.c");
        // The firmware's own arrays, read here without art_builtins.
        let arr = |name: &str| -> Option<Vec<u8>> {
            let head = format!("static const uint8_t {name}[] = {{");
            let at = firmware.find(&head)? + head.len();
            let body = &firmware[at..at + firmware[at..].find("};")?];
            Some(
                body.split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| match t.strip_prefix("0x") {
                        Some(hex) => u8::from_str_radix(hex, 16).unwrap(),
                        None => t.parse().unwrap(),
                    })
                    .collect(),
            )
        };
        for i in 0..count {
            let set = &manifest["sets"][usize::from(i)];
            let a = art(
                &Source::BuiltIn(i as i8, count),
                set["label"].as_str().unwrap().into(),
            );
            assert_eq!((a.from, a.missing), (wire::FROM_BUILT_IN, None), "set {i}");
            let blob = base64::engine::general_purpose::STANDARD
                .decode(a.nva.expect("frames"))
                .unwrap();
            let sent = super::super::art_preview::decode(&blob).unwrap();
            assert_eq!((sent.width, sent.height, sent.ink), (68, 140, true));
            assert_eq!(u64::from(blob[6]), set["fps"].as_u64().unwrap(), "the rate");
            let mut bits = vec![0u8; 140usize.div_ceil(8) * 68];
            let mut firmware_frames = Vec::new();
            while let Some(delta) = arr(&format!("set_{i}_delta_{}", firmware_frames.len())) {
                super::super::art_preview::apply(&delta, &mut bits).unwrap();
                firmware_frames.push(bits.clone());
            }
            // The play order, step by step (the upright blob numbers its unique frames anew).
            let order = arr(&format!("set_{i}_order")).unwrap();
            let sent_order = &blob[12..12 + usize::from(u16::from_le_bytes([blob[10], blob[11]]))];
            assert_eq!(
                sent_order.len(),
                order.len(),
                "every step of the play order"
            );
            for (step, (&fw, &up)) in order.iter().zip(sent_order).enumerate() {
                let (fw, up) = (
                    &firmware_frames[usize::from(fw)],
                    &sent.frames[usize::from(up)],
                );
                for y in 0..140 {
                    for x in 0..68 {
                        let got = up[y * 9 + x / 8] & (0x80 >> (x % 8)) != 0;
                        assert_eq!(
                            got,
                            stored_pixel(fw, x, y),
                            "set {i} step {step} ({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    fn found(
        name: &str,
        origin: super::super::art_pieces::Origin,
        crc: u32,
        bytes: usize,
    ) -> super::super::art_pieces::Found {
        super::super::art_pieces::Found {
            origin,
            name: name.into(),
            image: std::sync::Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK])),
            bytes,
            key: u64::from(crc),
            crcs: vec![crc],
            plugin: false,
            fitted: None,
        }
    }

    fn slot(name: &str, len: u16, crc: Option<u32>) -> foxglove_link::protocol::SlotInfo {
        foxglove_link::protocol::SlotInfo {
            slot: 0,
            used: true,
            writes: 1,
            len,
            name: name.into(),
            crc,
        }
    }

    /// #863: a kept place's frames come from the PC piece with its CRC whatever it is named; a
    /// CRC no piece has gets no frames, not a name-alike's (it was paced or changed since);
    /// older firmware (no CRC) falls back to the name and length.
    #[test]
    fn kept_places_match_by_content() {
        use super::super::art_pieces::{pc_pieces, Origin, PcPiece};
        let corne = ArtTarget::FIRMWARE;
        let blob = |dots: &[(usize, usize)]| {
            let f = stored_with(dots, true);
            super::super::art_encode::encode(&[f], 4, corne).unwrap()
        };
        let (orbit, radar) = (blob(&[(1, 1)]), blob(&[(2, 2), (3, 3)]));
        let pcs = pc_pieces(
            vec![
                found(
                    "orbit",
                    Origin::Library("orbit.bin".into()),
                    0x1111,
                    orbit.len(),
                ),
                found(
                    "radar",
                    Origin::Library("radar.bin".into()),
                    0x2222,
                    radar.len(),
                ),
            ],
            &|_| false,
        );
        let bytes = |p: &PcPiece| {
            Some(if p.name == "orbit" {
                orbit.clone()
            } else {
                radar.clone()
            })
        };
        let dots = |s: Stored| {
            let up = upright(&s, 1).unwrap();
            (pixel(&up, 0, 2, 2), pixel(&up, 0, 1, 1))
        };
        let len = u16::try_from(orbit.len()).unwrap();
        // Named after orbit, holding radar's bytes: radar's frames.
        let s = kept_stored(&slot("orbit", len, Some(0x2222)), &pcs, bytes).unwrap();
        assert_eq!(dots(s), (true, false));
        // A CRC nobody has: no frames, even with orbit's name and length.
        assert_eq!(
            kept_stored(&slot("orbit", len, Some(0x9999)), &pcs, bytes),
            Err(wire::MISSING_NOT_ON_PC)
        );
        // No CRC (older firmware): by name and length.
        let s = kept_stored(&slot("orbit", len, None), &pcs, bytes).unwrap();
        assert_eq!(dots(s), (false, true));
        assert_eq!(
            kept_stored(&slot("orbit", 1, None), &pcs, bytes),
            Err(wire::MISSING_NOT_ON_PC)
        );
        // The body says why there are no frames.
        let a = body(
            wire::FROM_KEPT,
            "Radar".into(),
            Err(wire::MISSING_NOT_ON_PC),
        );
        assert_eq!((a.nva, a.missing), (None, Some(wire::MISSING_NOT_ON_PC)));
    }

    /// #878: a kept place no PC piece holds draws from its bytes read back, but only bytes that
    /// match its CRC; without a copy (or without a CRC, older firmware) the placeholder stays.
    #[test]
    fn kept_places_draw_from_bytes_read_back() {
        use foxglove_link::protocol::blob_crc;
        let f = stored_with(&[(4, 4)], true);
        let blob = super::super::art_encode::encode(&[f], 4, ArtTarget::FIRMWARE).unwrap();
        let crc = blob_crc(&blob);
        let copy = |c: u32| (c == crc).then(|| blob.clone());
        let s = or_read_back(wire::MISSING_NOT_ON_PC, Some(crc), copy).unwrap();
        assert!(pixel(&upright(&s, 1).unwrap(), 0, 4, 4));
        // No copy for this CRC, no CRC at all, or another reason: as before.
        assert_eq!(
            or_read_back(wire::MISSING_NOT_ON_PC, Some(crc ^ 1), copy),
            Err(wire::MISSING_NOT_ON_PC)
        );
        assert_eq!(
            or_read_back(wire::MISSING_NOT_ON_PC, None, copy),
            Err(wire::MISSING_NOT_ON_PC)
        );
        assert_eq!(
            or_read_back(wire::MISSING_UNREADABLE, Some(crc), copy),
            Err(wire::MISSING_UNREADABLE)
        );
        // A copy whose bytes no longer match the CRC is not drawn.
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert_eq!(
            or_read_back(wire::MISSING_NOT_ON_PC, Some(crc), |_| Some(bad.clone())),
            Err(wire::MISSING_NOT_ON_PC)
        );
        // A read whose bytes don't match the place's CRC is dropped before it is kept.
        assert!(keep(crc ^ 1, &blob).unwrap_err().contains("CRC"));
    }

    /// A bundled set kept on the keyboard is found by its CRC through the app's own pieces, and
    /// its frames go out. With `FOXGLOVE_LIVE_ART_DUMP` set to a folder, the feed's real `art`
    /// messages (built-in, kept and matched, kept not on this PC, pushed) are written there for
    /// the wallpaper's review frames (bandit-wallpapers tools/fixtures).
    #[test]
    fn real_pieces_in_the_feeds_form() {
        use foxglove_link::protocol::blob_crc;
        let library = super::super::art_library::Library::default();
        let made = |name: &str| {
            let (_, bytes, _) = super::super::art_pieces::BUNDLED
                .iter()
                .find(|b| b.0 == name)
                .unwrap();
            let bytes = super::super::art_encode::normalise_blob(bytes, Default::default());
            super::super::art_encode::for_screen(&bytes, &Ok(ArtTarget::FIRMWARE))
                .unwrap()
                .0
        };
        let hearth = made("hearth.bin");
        let hearth_len = u16::try_from(hearth.len()).unwrap();
        let kept = body(
            wire::FROM_KEPT,
            "Hearth kept".into(),
            kept_in(
                &slot("Hearth kept", hearth_len, Some(blob_crc(&hearth))),
                &library,
            ),
        );
        assert_eq!(kept.missing, None, "found by CRC under another name");
        assert!(kept.nva.is_some());
        let unmatched = body(
            wire::FROM_KEPT,
            "Night shift".into(),
            kept_in(&slot("Night shift", 3000, Some(0xdead_beef)), &library),
        );
        assert_eq!(unmatched.missing, Some(wire::MISSING_NOT_ON_PC));
        let pushed = body(
            wire::FROM_FILE,
            "fury".into(),
            Stored::from_blob(&made("fury.bin")).map_err(|_| wire::MISSING_UNREADABLE),
        );
        assert!(pushed.nva.is_some());
        let built_in = art(&Source::BuiltIn(0, 1), "Bandit emblem".into());
        assert!(built_in.nva.is_some());
        if let Some(dir) = std::env::var_os("FOXGLOVE_LIVE_ART_DUMP") {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            for (file, a) in [
                ("builtin", built_in),
                ("kept", kept),
                ("kept-unmatched", unmatched),
                ("pushed", pushed),
            ] {
                let json = wire::Message::Art { art: Some(a) }.json();
                std::fs::write(dir.join(format!("feed-art-{file}.json")), json).unwrap();
            }
        }
    }
}
