//! The keyboard's self-report (#705; firmware widgets/health.h, wire format docs/health.md):
//! reading each half's block (D7 13 HEALTH, HELLO bit 28), parsing it, and turning it into plain
//! rows, one per feature, each OK, a warning or failed with the reason in words.
//!
//! - [`Block::parse`]: the 96-byte block, no I/O
//! - [`read`]: one half's block over a [`Transport`] (four chunks of one copy)
//! - [`Thresholds`]: when a count is a warning or a failure; data, from `data/health.toml`
//! - [`rows`]: the view model, for the app's Keyboard health view, `health.read` and
//!   `foxglove-lab health`
//! - [`HealthRow`] / [`Level`]: the one row type every health source uses, the keyboard's and the
//!   app's features' (art keys, LampArray, zones, ...): see [`HealthRow`]

use std::time::Duration;

use crate::protocol::{self, REPORT};
use crate::Transport;

/// The newest block version this reader knows (firmware HEALTH_VERSION).
pub const VERSION: u8 = 4;
/// The block's size in version 1 (every block has at least these bytes).
pub const SIZE: usize = 96;
/// The block's size in version 2 (#719: the knob, bytes 96-103).
pub const SIZE_V2: usize = 104;
/// The block's size in version 3 (the Raw HID counts per transport, bytes 104-111).
pub const SIZE_V3: usize = 112;
/// The block's size in version 4 (each transport's last refusal's error code, bytes 112-115).
pub const SIZE_V4: usize = 116;
/// Block bytes per D7 93 reply (bytes 8-31).
pub const CHUNK: usize = 24;
/// Replies per block.
pub const CHUNKS: u8 = 4;

/// D7 13: chunk `chunk` of `half`'s block (0 left, 1 right). Chunk 0 takes a new copy.
pub fn request(half: u8, chunk: u8) -> Vec<u8> {
    vec![protocol::SETTINGS, protocol::settings::HEALTH, half, chunk]
}

/// How a row stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    /// The word the views and the lab print.
    pub fn word(self) -> &'static str {
        match self {
            Level::Ok => "OK",
            Level::Warn => "warning",
            Level::Fail => "failed",
        }
    }
}

/// One line of the Keyboard health view: a feature, how it stands and why, in plain words.
///
/// This is the shared row type (#705): the keyboard's block becomes these ([`rows`]), and an app
/// feature with health of its own (the art keys #701, LampArray, lighting zones, colour-aware
/// effects) returns these too, so the view, `health.read`, Copy diagnostics and `foxglove-lab
/// health` show them all the same way. In the app, a feature registers a function that returns
/// its rows (`app/src/foxglove/health.rs`, `register`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRow {
    /// A stable id for the group: "relay", "raw_hid", "screens", "lighting", "split", "pc_state",
    /// "polarity", "self_test", "firmware", or a feature's own ("screens.art_keys", ...).
    pub subsystem: String,
    /// What the row is about, as the user names it: "Right screen", "Link to the right half".
    pub item: String,
    pub level: Level,
    /// The reason, in plain words: "2 art pushes failed (relay timeout)".
    pub detail: String,
}

impl HealthRow {
    pub fn new(
        subsystem: impl Into<String>,
        item: impl Into<String>,
        level: Level,
        detail: impl Into<String>,
    ) -> HealthRow {
        HealthRow {
            subsystem: subsystem.into(),
            item: item.into(),
            level,
            detail: detail.into(),
        }
    }

    /// "Right screen: 2 art pushes failed (relay timeout)".
    pub fn line(&self) -> String {
        format!("{}: {}", self.item, self.detail)
    }
}

/// The worst level among `rows` (OK when there are none).
pub fn worst(rows: &[HealthRow]) -> Level {
    rows.iter().map(|r| r.level).max().unwrap_or(Level::Ok)
}

/// A boot self-test check's state (block bytes 24-27, bits 0-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Pending,
    Pass,
    Fail,
    NotApplicable,
}

/// One boot self-test result: its state and code (bits 2-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub state: CheckState,
    pub code: u8,
}

impl Check {
    fn of(b: u8) -> Check {
        Check {
            state: match b & 3 {
                0 => CheckState::Pending,
                1 => CheckState::Pass,
                2 => CheckState::Fail,
                _ => CheckState::NotApplicable,
            },
            code: b >> 2,
        }
    }
}

/// Block flags (byte 3): what the half has built in.
pub mod flags {
    pub const RELAY_CENTRAL: u8 = 1 << 0;
    pub const HID_STATS: u8 = 1 << 1;
    pub const HOST_LINK: u8 = 1 << 2;
    pub const RELAY_PERIPHERAL: u8 = 1 << 3;
    pub const LIGHTING: u8 = 1 << 4;
    pub const PC_STATE: u8 = 1 << 5;
    pub const POLARITY: u8 = 1 << 6;
    pub const DISPLAY: u8 = 1 << 7;
}

/// The knob's part of the block (version 2, bytes 96-103, the left; #719).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Knob {
    /// [`knob_flags`].
    pub flags: u8,
    /// The last event's kind (0 none, 1 turn, 2 click, 3 double click, 4 hold, 5 handed back).
    pub last: u8,
    /// Events sent to the PC.
    pub events: u16,
    /// Fallback keys the knob tapped.
    pub keys: u16,
    /// Holds the board dropped (a USB disconnect or an endpoint change).
    pub drops: u16,
}

/// The Raw HID counts per transport (version 3, bytes 104-111, the left; version 4 adds the last
/// error codes, bytes 112-115). The block's `hid_sent` and `hid_failed` are the sums over USB and
/// Bluetooth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HidTransport {
    /// Reports the USB driver refused.
    pub usb_failed: u16,
    /// Bluetooth notifications queued.
    pub ble_sent: u16,
    /// Bluetooth notifies refused on a link where the PC listens.
    pub ble_failed: u16,
    /// Bluetooth reports nobody listened for: no link on the active profile, the PC hasn't
    /// turned the report's notifications on, or the link isn't encrypted yet. Normal while the
    /// PC uses USB, so never a warning.
    pub ble_skipped: u16,
    /// The last report the USB driver refused: its error code (a negative errno), 0 while it
    /// refused none or before version 4.
    pub usb_last_err: i16,
    /// The same for Bluetooth notifies refused on a link where the PC listens.
    pub ble_last_err: i16,
}

/// A Zephyr error code as the health rows say it: "-12 ENOMEM, no buffer free".
pub fn errno_words(err: i16) -> String {
    let what = match -i32::from(err) {
        1 => "EPERM, not allowed",
        5 => "EIO, I/O error",
        11 => "EAGAIN, not ready yet",
        12 => "ENOMEM, no buffer free",
        16 => "EBUSY, busy",
        19 => "ENODEV, no device",
        22 => "EINVAL, not turned on",
        105 => "ENOBUFS, no buffer free",
        116 => "ETIMEDOUT, timed out",
        128 => "ENOTCONN, not connected",
        _ => return err.to_string(),
    };
    format!("{err} {what}")
}

/// The knob byte (96): which knob behaviours the keymap has, and the hold.
pub mod knob_flags {
    pub const TURN: u8 = 1 << 0;
    pub const PRESS: u8 = 1 << 1;
    pub const HELD: u8 = 1 << 2;
    pub const ON_APP: u8 = 1 << 3;
}

/// One half's self-report block (docs/health.md, versions 1 to 3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Block {
    pub version: u8,
    /// 0 left, 1 right.
    pub half: u8,
    pub flags: u8,
    pub firmware: protocol::FirmwareVersion,
    pub sha: String,
    pub uptime_s: u32,
    /// Zephyr's RESET_* bits (hwinfo); 0 unknown.
    pub reset_cause: u32,
    /// Display, settings, relay, LEDs.
    pub self_test: [u8; 4],
    /// The left: HELLO's relay debug bytes for this link.
    pub relay_debug: [u8; 10],
    pub relay_timeouts: u16,
    pub relay_unsticks: u16,
    pub relay_ready: u16,
    pub relay_in_flight: u8,
    pub relay_discovery: i8,
    pub relay_refused: u16,
    pub hid_sent: u32,
    pub hid_queued: u16,
    pub hid_dropped: u16,
    pub hid_failed: u16,
    pub hid_reply_drops: u16,
    pub redraws: u32,
    pub art_ok: u16,
    pub art_failed: u16,
    pub art_last_error: u8,
    /// -1 a pushed set, -128 not known yet.
    pub art_set: i8,
    pub snap_errors: u16,
    pub light_applies: u16,
    pub light_syncs: u16,
    pub light_sync_fails: u16,
    pub rgb_mismatches: u16,
    pub last_effect: u8,
    pub underglow_effect: u8,
    pub split_drops: u16,
    pub split_up_at_s: u32,
    pub split_connected: bool,
    pub guard_drops: u8,
    pub pc_state: u8,
    pub pc_reason: u8,
    pub polarity: u8,
    pub polarity_right: u8,
    pub leds: u8,
    pub leds_right: u8,
    /// Version 2 (#719); `None` in a version 1 block.
    pub knob: Option<Knob>,
    /// Version 3; `None` in an older block.
    pub hid_transport: Option<HidTransport>,
}

/// A version 4 block's error code at `at`; 0 in an older or shorter block.
fn last_err(b: &[u8], at: usize) -> i16 {
    if b[0] >= 4 && usize::from(b[2]) >= SIZE_V4 && b.len() >= SIZE_V4 {
        u16_at(b, at) as i16
    } else {
        0
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl Block {
    /// The block's bytes (at least [`SIZE`]). `Err` names what is wrong with them.
    pub fn parse(b: &[u8]) -> std::result::Result<Block, String> {
        if b.len() < SIZE {
            return Err(format!("{} bytes, a block has {SIZE}", b.len()));
        }
        if b[0] == 0 {
            return Err("not a block (version 0)".into());
        }
        if usize::from(b[2]) < SIZE {
            return Err(format!(
                "the block says {} bytes, version {} has {SIZE}",
                b[2], b[0]
            ));
        }
        let sha = &b[8..16];
        let end = sha.iter().position(|&c| c == 0).unwrap_or(sha.len());
        Ok(Block {
            version: b[0],
            half: b[1],
            flags: b[3],
            firmware: protocol::FirmwareVersion {
                major: b[4],
                minor: b[5],
                patch: b[6],
                profile: b[7],
            },
            sha: String::from_utf8_lossy(&sha[..end]).into_owned(),
            uptime_s: u32_at(b, 16),
            reset_cause: u32_at(b, 20),
            self_test: [b[24], b[25], b[26], b[27]],
            relay_debug: b[28..38].try_into().unwrap(),
            relay_timeouts: u16_at(b, 38),
            relay_unsticks: u16_at(b, 40),
            relay_ready: u16_at(b, 42),
            relay_in_flight: b[44],
            relay_discovery: b[45] as i8,
            relay_refused: u16_at(b, 46),
            hid_sent: u32_at(b, 48),
            hid_queued: u16_at(b, 52),
            hid_dropped: u16_at(b, 54),
            hid_failed: u16_at(b, 56),
            hid_reply_drops: u16_at(b, 58),
            redraws: u32_at(b, 60),
            art_ok: u16_at(b, 64),
            art_failed: u16_at(b, 66),
            art_last_error: b[68],
            art_set: b[69] as i8,
            snap_errors: u16_at(b, 70),
            light_applies: u16_at(b, 72),
            light_syncs: u16_at(b, 74),
            light_sync_fails: u16_at(b, 76),
            rgb_mismatches: u16_at(b, 78),
            last_effect: b[80],
            underglow_effect: b[81],
            split_drops: u16_at(b, 82),
            split_up_at_s: u32_at(b, 84),
            split_connected: b[88] != 0,
            guard_drops: b[89],
            pc_state: b[90],
            pc_reason: b[91],
            polarity: b[92],
            polarity_right: b[93],
            leds: b[94],
            leds_right: b[95],
            knob: (b[0] >= 2 && usize::from(b[2]) >= SIZE_V2 && b.len() >= SIZE_V2).then(|| Knob {
                flags: b[96],
                last: b[97],
                events: u16_at(b, 98),
                keys: u16_at(b, 100),
                drops: u16_at(b, 102),
            }),
            hid_transport: (b[0] >= 3 && usize::from(b[2]) >= SIZE_V3 && b.len() >= SIZE_V3).then(
                || HidTransport {
                    usb_failed: u16_at(b, 104),
                    ble_sent: u16_at(b, 106),
                    ble_failed: u16_at(b, 108),
                    ble_skipped: u16_at(b, 110),
                    usb_last_err: last_err(b, 112),
                    ble_last_err: last_err(b, 114),
                },
            ),
        })
    }

    /// The block as the firmware sends it (version 4 or 3 with the per-transport counts, version
    /// 2 with a knob, else version 1's 96 bytes): for tests, fixtures and recordings.
    pub fn to_bytes(&self) -> Vec<u8> {
        let size = if self.hid_transport.is_some() && self.version >= 4 {
            SIZE_V4
        } else if self.hid_transport.is_some() {
            SIZE_V3
        } else if self.knob.is_some() {
            SIZE_V2
        } else {
            SIZE
        };
        let mut b = vec![0u8; size];
        b[0] = self.version;
        b[1] = self.half;
        b[2] = size as u8;
        b[3] = self.flags;
        b[4..8].copy_from_slice(&[
            self.firmware.major,
            self.firmware.minor,
            self.firmware.patch,
            self.firmware.profile,
        ]);
        let sha = self.sha.as_bytes();
        let n = sha.len().min(8);
        b[8..8 + n].copy_from_slice(&sha[..n]);
        fn put16(b: &mut [u8], at: usize, v: u16) {
            b[at..at + 2].copy_from_slice(&v.to_le_bytes());
        }
        put16(&mut b, 38, self.relay_timeouts);
        put16(&mut b, 40, self.relay_unsticks);
        put16(&mut b, 42, self.relay_ready);
        put16(&mut b, 46, self.relay_refused);
        put16(&mut b, 52, self.hid_queued);
        put16(&mut b, 54, self.hid_dropped);
        put16(&mut b, 56, self.hid_failed);
        put16(&mut b, 58, self.hid_reply_drops);
        put16(&mut b, 64, self.art_ok);
        put16(&mut b, 66, self.art_failed);
        put16(&mut b, 70, self.snap_errors);
        put16(&mut b, 72, self.light_applies);
        put16(&mut b, 74, self.light_syncs);
        put16(&mut b, 76, self.light_sync_fails);
        put16(&mut b, 78, self.rgb_mismatches);
        put16(&mut b, 82, self.split_drops);
        b[16..20].copy_from_slice(&self.uptime_s.to_le_bytes());
        b[20..24].copy_from_slice(&self.reset_cause.to_le_bytes());
        b[24..28].copy_from_slice(&self.self_test);
        b[28..38].copy_from_slice(&self.relay_debug);
        b[44] = self.relay_in_flight;
        b[45] = self.relay_discovery as u8;
        b[48..52].copy_from_slice(&self.hid_sent.to_le_bytes());
        b[60..64].copy_from_slice(&self.redraws.to_le_bytes());
        b[68] = self.art_last_error;
        b[69] = self.art_set as u8;
        b[80] = self.last_effect;
        b[81] = self.underglow_effect;
        b[84..88].copy_from_slice(&self.split_up_at_s.to_le_bytes());
        b[88] = u8::from(self.split_connected);
        b[89] = self.guard_drops;
        b[90] = self.pc_state;
        b[91] = self.pc_reason;
        b[92] = self.polarity;
        b[93] = self.polarity_right;
        b[94] = self.leds;
        b[95] = self.leds_right;
        if let Some(k) = self.knob {
            b[96] = k.flags;
            b[97] = k.last;
            put16(&mut b, 98, k.events);
            put16(&mut b, 100, k.keys);
            put16(&mut b, 102, k.drops);
        }
        if let Some(h) = self.hid_transport {
            put16(&mut b, 104, h.usb_failed);
            put16(&mut b, 106, h.ble_sent);
            put16(&mut b, 108, h.ble_failed);
            put16(&mut b, 110, h.ble_skipped);
            if size >= SIZE_V4 {
                put16(&mut b, 112, h.usb_last_err as u16);
                put16(&mut b, 114, h.ble_last_err as u16);
            }
        }
        b
    }

    /// The D7 93 replies that carry this block (copy number `copy`), as the keyboard sends them.
    pub fn replies(&self, copy: u8) -> Vec<[u8; REPORT]> {
        let bytes = self.to_bytes();
        let chunks = bytes.len().div_ceil(CHUNK) as u8;
        (0..chunks)
            .map(|c| {
                let mut r = [0u8; REPORT];
                r[0] = protocol::SETTINGS;
                r[1] = 0x80 | protocol::settings::HEALTH;
                r[3] = self.half;
                r[4] = copy;
                r[5] = c;
                r[6] = chunks;
                r[7] = bytes.len() as u8;
                let at = usize::from(c) * CHUNK;
                let n = CHUNK.min(bytes.len() - at);
                r[8..8 + n].copy_from_slice(&bytes[at..at + n]);
                r
            })
            .collect()
    }

    /// Boot self-test check `i` (0 display, 1 settings, 2 relay, 3 LEDs).
    pub fn check(&self, i: usize) -> Check {
        Check::of(self.self_test.get(i).copied().unwrap_or(3))
    }

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    /// "Left" or "Right".
    pub fn half_name(&self) -> &'static str {
        if self.half == 0 {
            "Left"
        } else {
            "Right"
        }
    }
}

/// Why a half's block couldn't be read (D7 93's status, or the link).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// No HELLO bit 28: this firmware has no self-report.
    NotBuilt,
    /// 1: a read of the right is still running.
    Busy,
    /// 2: no relay link to the right half.
    NoRelay,
    /// 3: the right didn't answer the relay in time.
    RightTimeout,
    /// 4: not built in, or a right half whose firmware predates the self-report.
    OldFirmware,
    /// Anything else: a bad reply, a copy that changed mid-read, no reply.
    Other(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            ReadError::NotBuilt => "this firmware has no self-report (no HELLO bit 28)",
            ReadError::Busy => "a read of the right half is still running",
            ReadError::NoRelay => "no relay link to the right half",
            ReadError::RightTimeout => "the right half didn't answer the relay in time",
            ReadError::OldFirmware => "that half's firmware has no self-report",
            ReadError::Other(e) => e,
        })
    }
}

impl std::error::Error for ReadError {}

fn status_error(status: u8) -> ReadError {
    match status {
        1 => ReadError::Busy,
        2 => ReadError::NoRelay,
        3 => ReadError::RightTimeout,
        4 => ReadError::OldFirmware,
        5 => ReadError::Other("the keyboard refused the request (status 5)".into()),
        6 => ReadError::Other("no copy to read yet (status 6)".into()),
        s => ReadError::Other(format!("status {s}")),
    }
}

/// How long chunk 0 may take: the right's comes over the relay, spaced, under the link's latency
/// (about a second; docs/health.md says wait up to 8 s).
pub fn wait(half: u8) -> Duration {
    if half == 0 {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(8)
    }
}

/// Puts the replies of one read together: chunk 0 first, then 1-3 of the same copy. `None` until
/// all have come; `Err` for an error status or a copy that changed.
#[derive(Default)]
pub struct Assembler {
    bytes: Vec<u8>,
    copy: Option<u8>,
    got: u8,
}

impl Assembler {
    /// Takes one D7 93 reply for `half`, chunk `chunk`.
    pub fn take(
        &mut self,
        half: u8,
        chunk: u8,
        r: &[u8; REPORT],
    ) -> std::result::Result<Option<Block>, ReadError> {
        if !protocol::is_reply(r, protocol::SETTINGS, protocol::settings::HEALTH) {
            return Err(ReadError::Other("not a HEALTH reply".into()));
        }
        if r[2] != 0 {
            return Err(status_error(r[2]));
        }
        if r[3] != half || r[5] != chunk {
            return Err(ReadError::Other(format!(
                "asked for half {half} chunk {chunk}, got half {} chunk {}",
                r[3], r[5]
            )));
        }
        let chunks = r[6].max(1);
        let len = usize::from(r[7]);
        if chunk == 0 {
            self.bytes = vec![0; len.max(SIZE)];
            self.copy = Some(r[4]);
            self.got = 0;
        } else if self.copy != Some(r[4]) {
            return Err(ReadError::Other(
                "the block changed during the read (another reader?)".into(),
            ));
        }
        let at = usize::from(chunk) * CHUNK;
        let n = CHUNK.min(self.bytes.len().saturating_sub(at));
        self.bytes[at..at + n].copy_from_slice(&r[8..8 + n]);
        self.got += 1;
        if self.got < chunks {
            return Ok(None);
        }
        Block::parse(&self.bytes)
            .map(Some)
            .map_err(ReadError::Other)
    }
}

/// Reads `half`'s block (0 left, 1 right): chunk 0, then the rest of that copy. Ask only with
/// HELLO bit 28 ([`protocol::capabilities::HEALTH`]); firmware without it doesn't answer.
pub fn read(t: &impl Transport, half: u8) -> std::result::Result<Block, ReadError> {
    let mut a = Assembler::default();
    let mut chunks = CHUNKS;
    let mut chunk = 0;
    while chunk < chunks {
        let r = t
            .request(
                &request(half, chunk),
                protocol::SETTINGS,
                protocol::settings::HEALTH,
                if chunk == 0 {
                    wait(half)
                } else {
                    Duration::from_secs(1)
                },
            )
            .map_err(|e| ReadError::Other(format!("{e:#}")))?;
        if chunk == 0 && r[2] == 0 {
            chunks = r[6].max(1);
        }
        if let Some(block) = a.take(half, chunk, &r)? {
            return Ok(block);
        }
        chunk += 1;
    }
    Err(ReadError::Other("the block ended early".into()))
}

/// Both halves' blocks as one read gave them.
#[derive(Clone, Debug)]
pub struct Report {
    pub left: std::result::Result<Block, ReadError>,
    pub right: std::result::Result<Block, ReadError>,
}

/// Both halves, the left first; `capabilities` is HELLO's (bit 28 says the keyboard has it).
pub fn read_both(t: &impl Transport, capabilities: u32) -> Report {
    if capabilities & protocol::capabilities::HEALTH == 0 {
        return Report {
            left: Err(ReadError::NotBuilt),
            right: Err(ReadError::NotBuilt),
        };
    }
    let left = read(t, 0);
    let right = read(t, 1);
    Report { left, right }
}

/// When a count is a warning or a failure: each counter's two levels, from data
/// (`data/health.toml`, built in; [`Thresholds::parse`] reads another). A level of 0 never fires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thresholds {
    entries: Vec<(String, u32, u32)>,
}

/// The shipped thresholds.
pub const THRESHOLDS: &str = include_str!("../data/health.toml");

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds::parse(THRESHOLDS).expect("data/health.toml")
    }
}

impl Thresholds {
    /// Lines of `name = warn, fail`; `#` starts a comment.
    pub fn parse(text: &str) -> std::result::Result<Thresholds, String> {
        let mut entries = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let bad = || format!("line {}: expected `name = warn, fail`", i + 1);
            let (name, levels) = line.split_once('=').ok_or_else(bad)?;
            let (warn, fail) = levels.split_once(',').ok_or_else(bad)?;
            let warn: u32 = warn.trim().parse().map_err(|_| bad())?;
            let fail: u32 = fail.trim().parse().map_err(|_| bad())?;
            entries.push((name.trim().to_string(), warn, fail));
        }
        Ok(Thresholds { entries })
    }

    /// The level of `count` for counter `name` (OK for a name without levels).
    pub fn level(&self, name: &str, count: u32) -> Level {
        let Some((_, warn, fail)) = self.entries.iter().find(|(n, _, _)| n == name) else {
            return Level::Ok;
        };
        if *fail > 0 && count >= *fail {
            Level::Fail
        } else if *warn > 0 && count >= *warn {
            Level::Warn
        } else {
            Level::Ok
        }
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Uptime in words: "3 h 12 min", "45 s".
pub fn duration_words(s: u32) -> String {
    let (d, h, m) = (s / 86_400, s / 3600 % 24, s / 60 % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{s} s")
    }
}

/// Zephyr's reset cause bits (drivers/hwinfo.h) in words, the most telling first.
pub fn reset_words(cause: u32) -> String {
    // RESET_PIN 0, SOFTWARE 1, BROWNOUT 2, POR 3, WATCHDOG 4, DEBUG 5, SECURITY 6,
    // LOW_POWER_WAKE 7, CPU_LOCKUP 8, PARITY 9, PLL 10, CLOCK 11, HARDWARE 12, USER 13,
    // TEMPERATURE 14.
    const NAMES: [(u32, &str); 11] = [
        (1 << 4, "the watchdog"),
        (1 << 8, "a CPU lockup (crash)"),
        (1 << 2, "a brownout (low battery or power)"),
        (
            1 << 1,
            "a software reset (a flash, the bootloader or a reset key)",
        ),
        (1 << 0, "the reset button"),
        (1 << 7, "waking from sleep"),
        (1 << 3, "power on"),
        (1 << 5, "the debugger"),
        (1 << 14, "temperature"),
        (1 << 12, "the hardware"),
        (1 << 13, "the user"),
    ];
    if cause == 0 {
        return "not known".into();
    }
    NAMES
        .iter()
        .find(|(bit, _)| cause & bit != 0)
        .map_or_else(|| format!("0x{cause:x}"), |(_, n)| (*n).to_string())
}

/// The left's last art error (enum art_relay_error) in words, for "art pushes failed (...)".
fn art_error_words(e: u8) -> &'static str {
    match e {
        1 => "no relay link to the right",
        2 => "too big",
        3 => "busy",
        4 => "data out of range",
        5 => "the PC's data didn't match its CRC",
        6 => "relay timeout or write failure",
        7 => "the right refused the art",
        8 => "the PC stopped sending",
        _ => "unknown error",
    }
}

/// The right's art state when it refused a blob.
fn right_art_words(state: u8) -> &'static str {
    match state {
        3 => "bad CRC",
        4 => "bad art data",
        5 => "too big",
        _ => "refused",
    }
}

fn check_rows(b: &Block, out: &mut Vec<HealthRow>) {
    let side = b.half_name();
    let names = [
        format!("{side} screen"),
        format!("{side} saved settings"),
        if b.half == 0 {
            "Relay to the right half".to_string()
        } else {
            "Right half's relay".to_string()
        },
        format!("{side} lights"),
    ];
    for (i, name) in names.into_iter().enumerate() {
        let c = b.check(i);
        let (level, detail) = match (i, c.state, c.code) {
            (_, CheckState::NotApplicable, _) => continue,
            (_, CheckState::Pending, _) => (Level::Warn, "still starting up".to_string()),
            (0, CheckState::Pass, _) => (Level::Ok, "started".into()),
            (0, CheckState::Fail, 1) => (
                Level::Fail,
                "the display didn't start (device not ready)".into(),
            ),
            (0, CheckState::Fail, _) => {
                (Level::Fail, "the screen didn't come up within 10 s".into())
            }
            (1, CheckState::Pass, _) => (Level::Ok, "loaded".into()),
            (1, CheckState::Fail, _) => (Level::Fail, "not loaded 30 s after start".into()),
            (2, CheckState::Pass, _) if b.half == 0 => (Level::Ok, "found after start".into()),
            (2, CheckState::Pass, _) => (Level::Ok, "the left found it".into()),
            (2, CheckState::Fail, 1) => (
                Level::Warn,
                "found, but late (the right half came up after the left?)".into(),
            ),
            (2, CheckState::Fail, 2) => {
                (Level::Fail, "no link to the other half after start".into())
            }
            (2, CheckState::Fail, _) => (Level::Fail, "linked, but the relay wasn't found".into()),
            (3, CheckState::Pass, _) => (Level::Ok, format!("{} LEDs ready", b.leds)),
            (3, CheckState::Fail, 1) => (Level::Fail, "the LED strip didn't start".into()),
            (3, CheckState::Fail, _) => (Level::Fail, "no LEDs in this half's chain".into()),
            (_, _, code) => (Level::Warn, format!("check {i} code {code}")),
        };
        out.push(HealthRow::new("self_test", name, level, detail));
    }
}

fn count_row(
    t: &Thresholds,
    out: &mut Vec<HealthRow>,
    subsystem: &str,
    item: String,
    name: &str,
    count: u32,
    words: String,
) {
    let level = t.level(name, count);
    if level != Level::Ok {
        out.push(HealthRow::new(subsystem, item, level, words));
    }
}

fn half_rows(b: &Block, t: &Thresholds, out: &mut Vec<HealthRow>) {
    let side = b.half_name();
    out.push(HealthRow::new(
        "firmware",
        format!("{side} half"),
        Level::Ok,
        format!(
            "firmware {} {}, up {}, last reset: {}",
            b.firmware,
            b.sha,
            duration_words(b.uptime_s),
            reset_words(b.reset_cause)
        ),
    ));
    check_rows(b, out);

    // Screens and art (art pushes go to the right: the left's counts are the right screen's)
    let art_screen = if b.half == 0 && b.has(flags::RELAY_CENTRAL) {
        "Right screen".to_string()
    } else {
        format!("{side} screen")
    };
    let art_why = if b.half == 0 {
        art_error_words(b.art_last_error)
    } else {
        right_art_words(b.art_last_error)
    };
    count_row(
        t,
        out,
        "screens",
        art_screen,
        "art_failed",
        u32::from(b.art_failed),
        format!(
            "{} ({art_why})",
            plural(
                u32::from(b.art_failed),
                "art push failed",
                "art pushes failed"
            )
        ),
    );
    count_row(
        t,
        out,
        "screens",
        format!("{side} screen"),
        "snap_errors",
        u32::from(b.snap_errors),
        plural(
            u32::from(b.snap_errors),
            "screen snapshot failed",
            "screen snapshots failed",
        ),
    );

    // Relay
    if b.has(flags::RELAY_CENTRAL) {
        let item = "Relay to the right half".to_string();
        if b.relay_in_flight >= 2 {
            out.push(HealthRow::new(
                "relay",
                item.clone(),
                Level::Fail,
                "stuck: both request slots are held by unanswered requests",
            ));
        }
        count_row(
            t,
            out,
            "relay",
            item.clone(),
            "relay_timeouts",
            u32::from(b.relay_timeouts),
            plural(
                u32::from(b.relay_timeouts),
                "request timed out",
                "requests timed out",
            ),
        );
        count_row(
            t,
            out,
            "relay",
            item.clone(),
            "relay_unsticks",
            u32::from(b.relay_unsticks),
            format!(
                "the link was dropped {} to free a stuck request",
                plural(u32::from(b.relay_unsticks), "time", "times")
            ),
        );
        count_row(
            t,
            out,
            "relay",
            item,
            "relay_refused",
            u32::from(b.relay_refused),
            plural(
                u32::from(b.relay_refused),
                "request failed",
                "requests failed",
            ),
        );
    } else if b.has(flags::RELAY_PERIPHERAL) {
        count_row(
            t,
            out,
            "relay",
            "Right half's relay".into(),
            "relay_refused",
            u32::from(b.relay_refused),
            plural(u32::from(b.relay_refused), "op refused", "ops refused"),
        );
    }

    // Raw HID (the left)
    if b.half == 0 {
        let item = "Reports to the PC".to_string();
        count_row(
            t,
            out,
            "raw_hid",
            item.clone(),
            "hid_reply_drops",
            u32::from(b.hid_reply_drops),
            format!(
                "{} (the reply queue was full)",
                plural(
                    u32::from(b.hid_reply_drops),
                    "reply dropped",
                    "replies dropped"
                )
            ),
        );
        if b.has(flags::HID_STATS) {
            count_row(
                t,
                out,
                "raw_hid",
                item.clone(),
                "hid_dropped",
                u32::from(b.hid_dropped),
                format!(
                    "{} (the USB queue was full, or USB reset)",
                    plural(
                        u32::from(b.hid_dropped),
                        "report dropped",
                        "reports dropped"
                    )
                ),
            );
            let failed_words = match b.hid_transport {
                Some(h) => {
                    let mut words = format!(
                        "{}: {} by USB, {} by Bluetooth",
                        plural(u32::from(b.hid_failed), "report refused", "reports refused"),
                        h.usb_failed,
                        h.ble_failed
                    );
                    // Version 4: why, as the last refusal on each transport said it.
                    for (n, err, by) in [
                        (h.usb_failed, h.usb_last_err, "USB"),
                        (h.ble_failed, h.ble_last_err, "Bluetooth"),
                    ] {
                        if n > 0 && err != 0 {
                            words += &format!("; {by}'s last: {}", errno_words(err));
                        }
                    }
                    words
                }
                None => plural(
                    u32::from(b.hid_failed),
                    "report refused by USB or Bluetooth",
                    "reports refused by USB or Bluetooth",
                ),
            };
            count_row(
                t,
                out,
                "raw_hid",
                item.clone(),
                "hid_failed",
                u32::from(b.hid_failed),
                failed_words,
            );
            // Information, never a warning: Bluetooth reports with no PC listening there.
            if let Some(h) = b.hid_transport.filter(|h| h.ble_skipped > 0) {
                let skipped = format!(
                    "{} skipped (no PC listening on Bluetooth)",
                    plural(
                        u32::from(h.ble_skipped),
                        "Bluetooth report",
                        "Bluetooth reports"
                    )
                );
                // Alone, it also stands for the group's "no problems" line.
                let detail = if out.iter().any(|r| r.subsystem == "raw_hid") {
                    skipped
                } else {
                    format!("no problems since start; {skipped}")
                };
                out.push(HealthRow::new("raw_hid", item, Level::Ok, detail));
            }
        }
    }

    // Lighting
    if b.has(flags::LIGHTING) && b.half == 0 {
        count_row(
            t,
            out,
            "lighting",
            "Right lights".into(),
            "light_sync_fails",
            u32::from(b.light_sync_fails),
            plural(
                u32::from(b.light_sync_fails),
                "update to the right half failed",
                "updates to the right half failed",
            ),
        );
        count_row(
            t,
            out,
            "lighting",
            "Right lights".into(),
            "rgb_mismatches",
            u32::from(b.rgb_mismatches),
            format!(
                "showed another colour than the left sent {}",
                plural(u32::from(b.rgb_mismatches), "time", "times")
            ),
        );
    }

    // Split link
    count_row(
        t,
        out,
        "split",
        "Link between the halves".into(),
        "split_drops",
        u32::from(b.split_drops),
        format!(
            "{side} half saw it drop {}",
            plural(u32::from(b.split_drops), "time", "times")
        ),
    );

    // The knob (#719): what the board says about its hold; the app adds its own rows.
    if let Some(k) = b
        .knob
        .filter(|k| k.flags & (knob_flags::TURN | knob_flags::PRESS) != 0)
    {
        out.push(HealthRow::new("knob", "Knob", Level::Ok, knob_words(&k)));
    }
}

/// "held by Foxglove, on an app; 12 events, 30 keys, 1 hold dropped".
pub fn knob_words(k: &Knob) -> String {
    let hold = if k.flags & knob_flags::HELD == 0 {
        "not held: the volume keys and mute"
    } else if k.flags & knob_flags::ON_APP != 0 {
        "held by Foxglove, on an app"
    } else {
        "held by Foxglove, on master"
    };
    format!(
        "{hold}; {}, {}, {}",
        plural(u32::from(k.events), "event", "events"),
        plural(u32::from(k.keys), "key", "keys"),
        plural(u32::from(k.drops), "hold dropped", "holds dropped")
    )
}

/// The Keyboard health view's rows for one read of both halves, the worst first within each
/// group. `t`: [`Thresholds::default`] unless a test or a user file says otherwise.
pub fn rows(report: &Report, t: &Thresholds) -> Vec<HealthRow> {
    let mut out = Vec::new();
    match &report.left {
        Ok(b) => half_rows(b, t, &mut out),
        Err(ReadError::NotBuilt) => {
            out.push(HealthRow::new(
                "firmware",
                "Keyboard",
                Level::Warn,
                "this firmware doesn't report its health: flash a newer build",
            ));
            return out;
        }
        Err(e) => out.push(HealthRow::new(
            "firmware",
            "Left half",
            Level::Fail,
            format!("couldn't read its health: {e}"),
        )),
    }
    match &report.right {
        Ok(b) => half_rows(b, t, &mut out),
        Err(ReadError::OldFirmware) => out.push(HealthRow::new(
            "firmware",
            "Right half",
            Level::Warn,
            "its firmware doesn't report its health: flash both halves",
        )),
        Err(ReadError::NotBuilt) => {}
        Err(e) => out.push(HealthRow::new(
            "relay",
            "Right half",
            Level::Fail,
            format!("couldn't read its health: {e}"),
        )),
    }
    // Across the halves
    if let (Ok(l), Ok(r)) = (&report.left, &report.right) {
        if l.sha != r.sha || l.firmware != r.firmware {
            out.push(HealthRow::new(
                "firmware",
                "Both halves",
                Level::Warn,
                format!(
                    "different firmware: left {} {}, right {} {}",
                    l.firmware, l.sha, r.firmware, r.sha
                ),
            ));
        }
        if l.leds_right != 0 && r.leds != 0 && l.leds_right != r.leds {
            out.push(HealthRow::new(
                "lighting",
                "Right lights",
                Level::Fail,
                format!(
                    "the left drives {} LEDs on the right, the right has {}",
                    l.leds_right, r.leds
                ),
            ));
        }
        if l.has(flags::POLARITY)
            && r.has(flags::POLARITY)
            && l.polarity <= 1
            && l.polarity_right != l.polarity
        {
            out.push(HealthRow::new(
                "polarity",
                "Right screen",
                Level::Warn,
                format!(
                    "hasn't confirmed the {} theme yet",
                    if l.polarity == 0 { "dark" } else { "light" }
                ),
            ));
        }
    }
    if let Ok(l) = &report.left {
        if l.has(flags::RELAY_CENTRAL) && !l.split_connected {
            out.push(HealthRow::new(
                "split",
                "Link between the halves",
                Level::Fail,
                "the right half isn't connected",
            ));
        }
        if l.has(flags::PC_STATE) && l.pc_reason != 0xff {
            out.push(HealthRow::new(
                "pc_state",
                "Stay awake",
                Level::Ok,
                pc_reason_words(l.pc_reason),
            ));
        }
    }
    // A group with no row of its own is fine: say so, so every feature has its line.
    for (subsystem, item) in [
        ("screens", "Screens"),
        ("relay", "Relay to the right half"),
        ("raw_hid", "Reports to the PC"),
        ("lighting", "Lights"),
        ("split", "Link between the halves"),
    ] {
        let any_read = report.left.is_ok() || report.right.is_ok();
        if any_read && !out.iter().any(|r| r.subsystem == subsystem) {
            out.push(HealthRow::new(
                subsystem,
                item,
                Level::Ok,
                "no problems since start",
            ));
        }
    }
    out
}

/// D7 91's reason byte in words (protocol::PcState::reason_text, said for the view).
fn pc_reason_words(reason: u8) -> &'static str {
    match reason {
        0 => "held awake: the PC is on",
        1 => "lit for 10 minutes after the last key (on USB power)",
        2 => "not held: on battery, or the PC's USB is asleep",
        3 => "not held: the PC is away",
        4 => "not held: no PC lease and no USB idle time running",
        _ => "unknown",
    }
}

/// One line for the log: the worst level and the rows that aren't OK.
pub fn summary(rows: &[HealthRow]) -> String {
    let bad: Vec<String> = rows
        .iter()
        .filter(|r| r.level != Level::Ok)
        .map(|r| format!("{} [{}]", r.line(), r.level.word()))
        .collect();
    if bad.is_empty() {
        format!("keyboard health OK ({} checks)", rows.len())
    } else {
        format!("keyboard health {}: {}", worst(rows).word(), bad.join("; "))
    }
}

/// The rows that are failed, as lines; what the app logs again when this set changes.
pub fn failed_lines(rows: &[HealthRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.level == Level::Fail)
        .map(HealthRow::line)
        .collect()
}

#[cfg(test)]
mod tests;
