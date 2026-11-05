//! Vial keyboards carry their own definition (#74): the same JSON as a VIA definition, kept
//! XZ-compressed in the firmware. Reading it gives a Vial board a keymap without a JSON file.
//!
//! Three reads, all after [`VIAL_PREFIX`] and all through [`Guarded::vial_read`]:
//! - `0x00` keyboard id: the Vial protocol version (u32, little-endian) and an 8-byte board uid;
//! - `0x01` definition size: the compressed size in bytes (u32, little-endian);
//! - `0x02` definition block: block `n` (u32, little-endian; the firmware reads the low 16 bits)
//!   is bytes `32n..32n+32` of the compressed definition, from byte 0 of the reply.
//!
//! Every read is bounded: the compressed size ([`MAX_COMPRESSED`], so at most [`MAX_BLOCKS`]
//! block reads), the decompressed size ([`MAX_JSON`]), each reply by the transport's timeout
//! and the whole read by a time budget ([`BUDGET`]). The XZ check (CRC64 in Vial's builds)
//! catches a block that went missing or came back out of order.
//!
//! Written from Vial's protocol notes (the Vial GUI's definition reader and vial-qmk's
//! handler, read for behaviour); no code was copied.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use foxglove_devices::{DeviceError, Result};

use crate::protocol::{Guarded, Transport, REPORT, UNHANDLED, VIAL_PREFIX};

const KEYBOARD_ID: u8 = 0x00;
const DEFINITION_SIZE: u8 = 0x01;
const DEFINITION_BLOCK: u8 = 0x02;

/// One definition block: a whole reply.
pub const BLOCK: usize = REPORT;
/// The largest compressed definition read. Real ones are a few KiB.
pub const MAX_COMPRESSED: u32 = 64 * 1024;
/// The most block reads one definition takes.
pub const MAX_BLOCKS: u32 = MAX_COMPRESSED / BLOCK as u32;
/// The largest decompressed definition accepted.
pub const MAX_JSON: usize = 1024 * 1024;
/// The time the whole definition read may take.
pub const BUDGET: Duration = Duration::from_secs(10);

/// What a Vial board says about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VialId {
    pub protocol: u32,
    pub uid: [u8; 8],
}

/// The board's Vial id; `Unsupported` when it isn't a Vial board (it marks the read unhandled,
/// echoes it, or reports protocol 0).
pub fn keyboard_id<T: Transport>(link: &Guarded<T>) -> Result<VialId> {
    let r = link.vial_read(KEYBOARD_ID, &[])?;
    if r[0] == UNHANDLED || r[..2] == [VIAL_PREFIX, KEYBOARD_ID] {
        return Err(DeviceError::Unsupported);
    }
    let protocol = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
    if protocol == 0 {
        return Err(DeviceError::Unsupported);
    }
    let mut uid = [0u8; 8];
    uid.copy_from_slice(&r[4..12]);
    Ok(VialId { protocol, uid })
}

/// The compressed definition's bytes, within the caps and `budget`.
pub fn compressed<T: Transport>(link: &Guarded<T>, budget: Duration) -> Result<Vec<u8>> {
    let end = Instant::now() + budget;
    let r = link.vial_read(DEFINITION_SIZE, &[])?;
    let size = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
    if size == 0 {
        return Err(DeviceError::Backend("the Vial definition is empty".into()));
    }
    if size > MAX_COMPRESSED {
        return Err(DeviceError::Backend(format!(
            "the Vial definition is {size} bytes, over the {MAX_COMPRESSED}-byte cap"
        )));
    }
    let size = size as usize;
    let blocks = size.div_ceil(BLOCK) as u32;
    debug_assert!(blocks <= MAX_BLOCKS);
    let mut out = Vec::with_capacity(size);
    for block in 0..blocks {
        if Instant::now() >= end {
            return Err(DeviceError::Timeout);
        }
        let r = link.vial_read(DEFINITION_BLOCK, &block.to_le_bytes())?;
        let take = (size - out.len()).min(BLOCK);
        out.extend_from_slice(&r[..take]);
    }
    Ok(out)
}

/// The definition JSON from its compressed bytes: XZ, as Vial builds it, or a bare LZMA
/// stream. Over [`MAX_JSON`] decompressed is an error.
pub fn decompress(data: &[u8]) -> Result<String> {
    const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
    let mut out = Capped(Vec::new());
    let mut input: &[u8] = data;
    let done = if data.starts_with(&XZ_MAGIC) {
        // lzma-rs keeps an XZ block's whole output in memory before writing it out, so the cap
        // is checked on the sizes the stream declares, before decoding.
        match xz_unpacked_size(data) {
            Some(n) if n > MAX_JSON => {
                return Err(DeviceError::Backend(format!(
                    "the Vial definition is {n} bytes decompressed, over the {MAX_JSON}-byte cap"
                )))
            }
            Some(_) => {}
            None => {
                return Err(DeviceError::Backend(
                    "the Vial definition isn't an XZ stream Foxglove reads".into(),
                ))
            }
        }
        lzma_rs::xz_decompress(&mut input, &mut out)
    } else {
        let options = lzma_rs::decompress::Options {
            memlimit: Some(MAX_JSON),
            ..Default::default()
        };
        lzma_rs::lzma_decompress_with_options(&mut input, &mut out, &options)
    };
    done.map_err(|e| DeviceError::Backend(format!("the Vial definition doesn't unpack: {e}")))?;
    String::from_utf8(out.0)
        .map_err(|_| DeviceError::Backend("the Vial definition isn't UTF-8 text".into()))
}

/// The decompressed size an XZ stream declares, from its LZMA2 chunk headers, walked without
/// decoding; stops counting once past [`MAX_JSON`]. `None` when it isn't laid out as XZ with
/// LZMA2 blocks (the XZ file format, sections 2-3; LZMA2's chunk headers). The decoder still
/// checks everything (headers, checksums, the index) afterwards.
fn xz_unpacked_size(d: &[u8]) -> Option<usize> {
    let byte = |at: usize| d.get(at).copied();
    let be16 = |at: usize| Some(usize::from(u16::from_be_bytes([byte(at)?, byte(at + 1)?])));
    // The stream flags' check type sets the check's size after each block.
    let check = match byte(7)? & 0x0F {
        0 => 0,
        t => 4 << ((t - 1) / 3),
    };
    let mut at = 12;
    let mut total = 0usize;
    loop {
        let start = at;
        match byte(at)? {
            0 => return Some(total), // the index
            size => at += (usize::from(size) + 1) * 4,
        }
        loop {
            let control = byte(at)?;
            at += 1;
            match control {
                0x00 => break,
                // An uncompressed chunk.
                0x01 | 0x02 => {
                    let n = be16(at)? + 1;
                    total += n;
                    at += 2 + n;
                }
                // An LZMA chunk; from 0xC0 with a properties byte.
                0x80.. => {
                    total += (usize::from(control & 0x1F) << 16) + be16(at)? + 1;
                    at += 4 + usize::from(control >= 0xC0) + be16(at + 2)? + 1;
                }
                _ => return None,
            }
            if total > MAX_JSON {
                return Some(total);
            }
        }
        // Block padding to a multiple of four, then the check.
        at += (4 - (at - start) % 4) % 4 + check;
    }
}

/// A writer that refuses to grow past [`MAX_JSON`].
struct Capped(Vec<u8>);

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0.len() + buf.len() > MAX_JSON {
            return Err(io::Error::other(format!(
                "over the {MAX_JSON}-byte cap decompressed"
            )));
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Why a Vial board's definition couldn't be read, as a kind the settings can say in plain
/// words (#74). The technical reason (`Source::Missing`'s `why`) stays for the health view and
/// the log; [`Unreadable::of`] sorts it from the messages this module and the device make, which
/// the tests pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unreadable {
    /// Over a size cap, compressed or unpacked.
    TooLarge,
    /// The read ran out of time, or the board stopped answering.
    TimedOut,
    /// The board says it has an empty definition.
    Empty,
    /// Not laid out as an XZ stream Foxglove reads.
    UnknownFormat,
    /// It doesn't unpack (a failed XZ/CRC check, blocks out of order) or isn't text.
    Damaged,
    /// It unpacks, but isn't a definition Foxglove understands.
    Invalid,
    /// The board stopped handling Vial's reads part way.
    NoAnswer,
    /// Anything else.
    Other,
}

impl Unreadable {
    /// The kind of a `Source::Missing` reason.
    pub fn of(why: &str) -> Self {
        let has = |s: &str| why.contains(s);
        if has("doesn't parse") {
            Unreadable::Invalid
        } else if has("-byte cap") {
            Unreadable::TooLarge
        } else if has("is empty") {
            Unreadable::Empty
        } else if has("isn't an XZ stream") {
            Unreadable::UnknownFormat
        } else if has("doesn't unpack") || has("isn't UTF-8") {
            Unreadable::Damaged
        } else if why == DeviceError::Timeout.to_string() {
            Unreadable::TimedOut
        } else if why == DeviceError::Unsupported.to_string() {
            Unreadable::NoAnswer
        } else {
            Unreadable::Other
        }
    }

    /// A short phrase in plain words, for the settings.
    pub fn plain(self) -> &'static str {
        match self {
            Unreadable::TooLarge => "it's too large",
            Unreadable::TimedOut => "it took too long to answer",
            Unreadable::Empty => "it has none stored",
            Unreadable::UnknownFormat => "it's stored in a format Foxglove doesn't read",
            Unreadable::Damaged => "its stored data was damaged",
            Unreadable::Invalid => "Foxglove doesn't understand its layout",
            Unreadable::NoAnswer => "it stopped answering Vial's reads",
            Unreadable::Other => "something went wrong reading it",
        }
    }
}

/// `json` XZ-compressed, as a Vial board stores it (for fakes).
#[cfg(test)]
pub(crate) fn xz(json: &str) -> Vec<u8> {
    let mut out = Vec::new();
    lzma_rs::xz_compress(&mut json.as_bytes(), &mut out).unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::fake::{leak, FakeQmk, FakeVial};
    use crate::definition::OPTIONS;
    use crate::protocol;

    fn vial_board(blob: Vec<u8>) -> &'static FakeQmk {
        leak(FakeQmk {
            protocol_version: 9,
            layers: 4,
            vial: Some(FakeVial {
                protocol: 6,
                uid: *b"BANDIT01",
                blob,
            }),
            ..FakeQmk::default()
        })
    }

    fn sent_blocks(f: &FakeQmk) -> Vec<u8> {
        f.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[..2] == [VIAL_PREFIX, DEFINITION_BLOCK])
            .map(|r| r[2])
            .collect()
    }

    #[test]
    fn reads_and_unpacks_the_boards_definition() {
        let blob = xz(OPTIONS);
        let f = vial_board(blob.clone());
        let link = Guarded::read_only(f);
        let id = keyboard_id(&link).unwrap();
        let json = decompress(&compressed(&link, BUDGET).unwrap()).unwrap();
        assert_eq!(
            id,
            VialId {
                protocol: 6,
                uid: *b"BANDIT01"
            }
        );
        assert_eq!(json, OPTIONS);
        let blocks = sent_blocks(f);
        assert_eq!(
            blocks.len(),
            blob.len().div_ceil(BLOCK),
            "one read per block"
        );
        assert_eq!(blocks, (0..blocks.len() as u8).collect::<Vec<_>>());
        let sent = f.sent.lock().unwrap();
        assert!(sent.iter().all(|r| !protocol::is_write(r)), "{sent:x?}");
    }

    #[test]
    fn a_board_that_isnt_vial_is_unsupported() {
        // Plain VIA firmware marks the Vial prefix unhandled.
        let f = leak(FakeQmk {
            protocol_version: 12,
            ..FakeQmk::default()
        });
        let link = Guarded::read_only(f);
        assert_eq!(keyboard_id(&link), Err(DeviceError::Unsupported));
        // Firmware that echoes what it doesn't know.
        struct Echo;
        impl Transport for Echo {
            fn exchange(&self, r: &protocol::Report) -> Result<protocol::Report> {
                Ok(*r)
            }
        }
        assert_eq!(
            keyboard_id(&Guarded::read_only(Echo)),
            Err(DeviceError::Unsupported)
        );
        // Vial protocol 0.
        let zero = leak(FakeQmk {
            vial: Some(FakeVial {
                protocol: 0,
                uid: [0; 8],
                blob: vec![],
            }),
            ..FakeQmk::default()
        });
        assert_eq!(
            keyboard_id(&Guarded::read_only(zero)),
            Err(DeviceError::Unsupported)
        );
    }

    /// The size cap: a board claiming more than [`MAX_COMPRESSED`] gets no block read at all.
    #[test]
    fn an_oversized_definition_is_refused_before_any_block_is_read() {
        let f = vial_board(vec![0; MAX_COMPRESSED as usize + 1]);
        let link = Guarded::read_only(f);
        let e = compressed(&link, BUDGET).unwrap_err();
        assert!(
            matches!(&e, DeviceError::Backend(m) if m.contains("cap")),
            "{e:?}"
        );
        assert!(sent_blocks(f).is_empty());
        // At the cap it's read, in exactly MAX_BLOCKS reads.
        let f = vial_board(vec![0; MAX_COMPRESSED as usize]);
        let link = Guarded::read_only(f);
        assert_eq!(
            compressed(&link, BUDGET).unwrap().len(),
            MAX_COMPRESSED as usize
        );
        assert_eq!(sent_blocks(f).len() as u32, MAX_BLOCKS);
    }

    #[test]
    fn an_empty_definition_is_an_error() {
        let f = vial_board(vec![]);
        assert!(matches!(
            compressed(&Guarded::read_only(f), BUDGET),
            Err(DeviceError::Backend(_))
        ));
    }

    /// The time budget: spent, the read stops before the next block.
    #[test]
    fn the_read_stops_when_its_time_is_up() {
        let f = vial_board(xz(OPTIONS));
        let link = Guarded::read_only(f);
        assert_eq!(compressed(&link, Duration::ZERO), Err(DeviceError::Timeout));
        assert!(sent_blocks(f).is_empty());
    }

    /// A board that stops answering mid-read ends it with the transport's timeout.
    #[test]
    fn silence_mid_read_is_a_timeout() {
        let f = vial_board(xz(OPTIONS));
        *f.vial_silent_from.lock().unwrap() = Some(1);
        let link = Guarded::read_only(f);
        assert_eq!(compressed(&link, BUDGET), Err(DeviceError::Timeout));
        assert_eq!(sent_blocks(f), [0, 1], "nothing after the silent block");
    }

    #[test]
    fn corrupt_or_oversized_data_doesnt_unpack() {
        // Vial's builds carry a CRC64: a damaged block doesn't pass for the definition.
        let mut blob = include_bytes!("../testdata/vial-small.json.xz").to_vec();
        // (Byte 30, the range coder's first, carries nothing: lzma-rs skips it.)
        for at in [31, 60, blob.len() - 30] {
            blob[at] ^= 0x55;
            let r = decompress(&blob);
            assert!(matches!(r, Err(DeviceError::Backend(_))), "{at}: {r:?}");
            blob[at] ^= 0x55;
        }
        // Blocks swapped, as a reply mix-up would give.
        let (a, b) = (24, 24 + BLOCK);
        let mut swapped = blob.clone();
        swapped[a..a + 8].copy_from_slice(&blob[b..b + 8]);
        swapped[b..b + 8].copy_from_slice(&blob[a..a + 8]);
        assert!(matches!(decompress(&swapped), Err(DeviceError::Backend(_))));
        assert!(matches!(
            decompress(b"not compressed"),
            Err(DeviceError::Backend(_))
        ));
        let big = "x".repeat(MAX_JSON + 1);
        let e = decompress(&xz(&big)).unwrap_err();
        assert!(
            matches!(&e, DeviceError::Backend(m) if m.contains("cap")),
            "{e:?}"
        );
        assert_eq!(decompress(&xz(&big[1..])).unwrap().len(), MAX_JSON);
    }

    /// Streams made by Python's `lzma.compress`, as Vial's build compresses definitions: real
    /// LZMA2 chunks and a CRC64 check.
    #[test]
    fn unpacks_a_definition_compressed_like_vials_build() {
        let json = decompress(include_bytes!("../testdata/vial-small.json.xz")).unwrap();
        assert!(json.starts_with(r#"{"name":"Real XZ""#), "{json}");
    }

    /// 440 bytes that unpack to 2 MiB are refused from their headers, before any decoding.
    #[test]
    fn a_small_stream_that_unpacks_huge_is_refused_before_decoding() {
        let bomb = include_bytes!("../testdata/vial-bomb.xz");
        assert!(xz_unpacked_size(bomb).unwrap() > MAX_JSON);
        let e = decompress(bomb).unwrap_err();
        assert!(
            matches!(&e, DeviceError::Backend(m) if m.contains("cap")),
            "{e:?}"
        );
        let small = include_bytes!("../testdata/vial-small.json.xz");
        assert_eq!(xz_unpacked_size(small), Some(84));
        assert_eq!(xz_unpacked_size(&small[..20]), None, "cut short");
    }

    /// Each way a read fails sorts into the kind the settings say in plain words, from the
    /// errors the reads really give.
    #[test]
    fn each_failure_has_a_plain_kind() {
        let kind = |e: DeviceError| Unreadable::of(&e.to_string());
        let read = |blob: Vec<u8>| {
            compressed(&Guarded::read_only(vial_board(blob)), BUDGET)
                .and_then(|b| decompress(&b))
                .unwrap_err()
        };
        assert_eq!(kind(read(vec![])), Unreadable::Empty);
        assert_eq!(
            kind(read(vec![0; MAX_COMPRESSED as usize + 1])),
            Unreadable::TooLarge
        );
        let bomb = include_bytes!("../testdata/vial-bomb.xz");
        assert_eq!(kind(decompress(bomb).unwrap_err()), Unreadable::TooLarge);
        let big = "x".repeat(MAX_JSON + 1);
        assert_eq!(
            kind(decompress(&xz(&big)).unwrap_err()),
            Unreadable::TooLarge
        );
        let f = vial_board(xz(OPTIONS));
        *f.vial_silent_from.lock().unwrap() = Some(1);
        assert_eq!(
            kind(compressed(&Guarded::read_only(f), BUDGET).unwrap_err()),
            Unreadable::TimedOut
        );
        let mut blob = include_bytes!("../testdata/vial-small.json.xz").to_vec();
        blob[60] ^= 0x55;
        assert_eq!(kind(decompress(&blob).unwrap_err()), Unreadable::Damaged);
        assert_eq!(
            kind(decompress(b"not compressed").unwrap_err()),
            Unreadable::Damaged
        );
        let small = include_bytes!("../testdata/vial-small.json.xz");
        let mut cut = small.to_vec();
        cut[12] = 0x01; // a block header that runs past the end
        cut.truncate(20);
        assert_eq!(
            kind(decompress(&cut).unwrap_err()),
            Unreadable::UnknownFormat
        );
        let mut bytes = Vec::new();
        lzma_rs::xz_compress(&mut &[0xFFu8, 0xFE][..], &mut bytes).unwrap();
        assert_eq!(kind(decompress(&bytes).unwrap_err()), Unreadable::Damaged);
        assert_eq!(
            Unreadable::of("the Vial definition doesn't parse: missing field `matrix`"),
            Unreadable::Invalid
        );
        assert_eq!(kind(DeviceError::Unsupported), Unreadable::NoAnswer);
        assert_eq!(kind(DeviceError::Busy("x".into())), Unreadable::Other);
        assert_eq!(
            Unreadable::Damaged.plain(),
            "its stored data was damaged",
            "the coordinator's example"
        );
    }

    #[test]
    fn a_bare_lzma_stream_unpacks_too() {
        let mut blob = Vec::new();
        lzma_rs::lzma_compress(&mut OPTIONS.as_bytes(), &mut blob).unwrap();
        assert_eq!(decompress(&blob).unwrap(), OPTIONS);
    }
}
