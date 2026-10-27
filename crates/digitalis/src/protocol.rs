//! Reports to and from the keyboard (firmware: widgets/host_link.h, widgets/art_relay.h).

pub const USAGE_PAGE: u16 = 0xFF60;
pub const USAGE: u16 = 0x61;
pub const REPORT: usize = 32;

pub const PANEL: u8 = 0xD5;
pub const ART: u8 = 0xD6;
/// Keyboard settings, saved on the keyboard (host_link.h).
pub const SETTINGS: u8 = 0xD7;
// 0xD7-0xDF are reserved for host-link protocols; 0xD8-0xDF are unassigned.

/// HELLO capabilities, independent of the protocol version and current split connection.
pub mod capabilities {
    pub const PANEL: u32 = 1 << 0;
    pub const ART_RELAY: u32 = 1 << 1;
    pub const SETTINGS_RGB: u32 = 1 << 2;
    pub const ART_STATUS_EVENT: u32 = 1 << 3;
    pub const CONN_STATE: u32 = 1 << 4;
    /// Settings POSITION (D7 05), pre-GA test builds only.
    pub const POSITION: u32 = 1 << 5;
    /// Settings SCREEN_SNAP / SCREEN_READ (D7 06 / D7 07).
    pub const SCREEN: u32 = 1 << 6;
    /// INVOKE refuses &bt, &out, &bootloader, &sys_reset and &soft_off with status 6.
    pub const INVOKE_REFUSES: u32 = 1 << 7;
    /// D7 84 CONN_STATE also arrives unsolicited when the connection changes (after host traffic).
    pub const CONN_STATE_EVENT: u32 = 1 << 8;
    /// D7 83 RGB_STATE also arrives unsolicited when the underglow turns on or off.
    pub const RGB_STATE_EVENT: u32 = 1 << 11;
    /// Settings BUILD_INFO (D7 09); HELLO bytes 27-30 carry the left's version and profile.
    pub const BUILD_INFO: u32 = 1 << 12;
    /// Panel SCREENS (D5 06): the board's screens, panels and art areas.
    pub const SCREENS: u32 = 1 << 13;
    /// Settings FRAME (D7 0C): per-LED frames for both halves (#272).
    pub const FRAMES: u32 = 1 << 14;
    /// Settings BOOTLOADER (D7 08), pre-GA test builds only.
    pub const BOOTLOADER: u32 = 1 << 9;
    /// Art SAVE / SLOT / ERASE (D6 06 / 07 / 08), flash slots on the right half.
    pub const ART_SLOTS: u32 = 1 << 10;
    /// The panel stays as drawn until RELEASE, and keeps its last frame on link loss: the host
    /// sends no keep-alive (#279, #280).
    pub const PANEL_HOLD: u32 = 1 << 15;
    /// CONN_STATE (D7 84) byte 8 is the left half's own battery % (#373).
    pub const LEFT_BATTERY: u32 = 1 << 17;
    /// Settings BATTERY_DIM (D7 0E): the underglow's levels by power state, per half.
    pub const BATTERY_DIM: u32 = 1 << 18;
    /// Settings EFFECT (D7 0F): effects each half draws on its own from its lamp positions
    /// (#490), pre-GA test builds only for now.
    pub const EFFECTS: u32 = 1 << 19;
    /// The relayed right half's on/off (D7 83) and power source (D7 8E) changes arrive as
    /// events (#478): no extra read after an underglow change is needed.
    pub const RIGHT_EVENTS: u32 = 1 << 20;
}
pub const VERSION: u8 = 1;

pub mod panel {
    pub const HELLO: u8 = 0x01;
    pub const FILL: u8 = 0x02;
    pub const ROWS: u8 = 0x03;
    pub const PRESENT: u8 = 0x04;
    pub const RELEASE: u8 = 0x05;
    pub const SCREENS: u8 = 0x06;
    /// Byte-addressed rows (#289, firmware #294): y u16, offset u16, n, then n bytes. For rows
    /// wider than a ROWS report and panels taller than 255 rows.
    pub const ROWS_AT: u8 = 0x07;
    /// Bytes per ROWS_AT report, after its 7-byte header.
    pub const ROWS_AT_MAX: usize = super::REPORT - 7;
    /// Rows per ROWS report: what fits after the 4-byte header, at most 3.
    pub fn rows_per_report(stride: usize) -> usize {
        ((super::REPORT - 4) / stride).min(3)
    }
    /// Whether a panel needs ROWS_AT: a row wider than a ROWS report holds, or more rows
    /// than ROWS can address.
    pub fn needs_rows_at(stride: usize, rows: usize) -> bool {
        stride > super::REPORT - 4 || rows > 256
    }
}

pub mod art {
    pub const BEGIN: u8 = 0x01;
    pub const DATA: u8 = 0x02;
    pub const COMMIT: u8 = 0x03;
    pub const SELECT: u8 = 0x04;
    pub const STATUS: u8 = 0x05;
    /// Saves the pushed set now on the right into a flash slot (firmware #123).
    pub const SAVE: u8 = 0x06;
    /// One flash slot's info; replies D6 87 once the right has answered.
    pub const SLOT: u8 = 0x07;
    pub const ERASE: u8 = 0x08;
    pub const DATA_MAX: usize = 27;
    /// Bytes of a slot name.
    pub const NAME_LEN: usize = 16;
}

pub mod settings {
    /// Right half's underglow follows the left's on/off.
    pub const RGB_SYNC: u8 = 0x01;
    /// Press (and release) a behavior binding, as a key would.
    pub const INVOKE: u8 = 0x02;
    /// Both halves' underglow state.
    pub const RGB_STATE: u8 = 0x03;
    pub const CONN_STATE: u8 = 0x04;
    /// Two-step reboot into the UF2 bootloader (#111): ARM returns a nonce, TRIGGER(half,
    /// nonce) within 5 s acts on it.
    pub const BOOTLOADER: u8 = 0x08;
    /// A half's firmware version, profile and git SHA (firmware #133).
    pub const BUILD_INFO: u8 = 0x09;
    /// Per-LED frames for both halves (#272): see [`super::frame`].
    pub const FRAME: u8 = 0x0C;
    /// The underglow's levels by power state (on USB, on battery, low battery), per half.
    pub const BATTERY_DIM: u8 = 0x0E;
    /// Effects each half draws on its own (#490): see [`super::effect`].
    pub const EFFECT: u8 = 0x0F;
    /// The longest hold INVOKE accepts.
    pub const MAX_HOLD_MS: u16 = 10_000;
}

/// Whether `r` is the reply to (`proto`, `cmd`).
pub fn is_reply(r: &[u8], proto: u8, cmd: u8) -> bool {
    r.len() >= 2 && r[0] == proto && r[1] == 0x80 | cmd
}

pub fn hello() -> Vec<u8> {
    vec![PANEL, panel::HELLO, VERSION]
}

pub fn fill(ink: bool) -> Vec<u8> {
    vec![PANEL, panel::FILL, ink as u8]
}

/// ROWS: `rows` holds whole rows of `stride` bytes starting at row `y`.
pub fn rows(y: usize, rows: &[u8], stride: usize) -> Vec<u8> {
    let mut r = vec![PANEL, panel::ROWS, y as u8, (rows.len() / stride) as u8];
    r.extend_from_slice(rows);
    r
}

/// D5 07: `bytes` at byte `offset` of row `y` (at most [`panel::ROWS_AT_MAX`]).
pub fn rows_at(y: usize, offset: usize, bytes: &[u8]) -> Vec<u8> {
    let mut r = vec![PANEL, panel::ROWS_AT];
    r.extend_from_slice(&(y as u16).to_le_bytes());
    r.extend_from_slice(&(offset as u16).to_le_bytes());
    r.push(bytes.len() as u8);
    r.extend_from_slice(bytes);
    r
}

pub fn present() -> Vec<u8> {
    vec![PANEL, panel::PRESENT]
}

pub fn release() -> Vec<u8> {
    vec![PANEL, panel::RELEASE]
}

/// D5 06: the descriptor of screen `index` (read index 0 first for the count).
pub fn screens(index: u8) -> Vec<u8> {
    vec![PANEL, panel::SCREENS, index]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    Display,
    Panel,
    Art,
    Other(u8),
}

/// One entry of D5 86 (#160).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenDescriptor {
    pub count: u8,
    pub index: u8,
    pub id: u8,
    pub kind: ScreenKind,
    /// 0 = left / central, 1 = right.
    pub half: u8,
    /// The display a panel or art area sits on; `None` for a display.
    pub parent: Option<u8>,
    /// As the PC addresses the region.
    pub width: u16,
    pub height: u16,
    /// Origin on the parent display, as seen.
    pub x: u16,
    pub y: u16,
    /// 90-degree steps clockwise.
    pub rotation: u8,
    /// 0 = 1 bit per pixel, MSB first, 1 = ink; 1 (RGB565) and 2 (2-bit grey) are reserved:
    /// no board draws them yet, so the app treats them as unsupported (#289).
    pub format: u8,
    /// Bit 0 D5 rows, 1 D7 06/07 readback, 2 D6 art, 3 D6 slots, 4 D5 07 rows-at.
    pub ops: u16,
    pub stride: u16,
    pub rows_per_report: u8,
    pub readback_half: Option<u8>,
    pub version: u8,
}

impl ScreenDescriptor {
    pub const OP_ROWS: u16 = 1 << 0;
    pub const OP_READBACK: u16 = 1 << 1;
    pub const OP_ART: u16 = 1 << 2;
    pub const OP_SLOTS: u16 = 1 << 3;
    pub const OP_ROWS_AT: u16 = 1 << 4;

    pub const FORMAT_1BIT: u8 = 0;
    pub const FORMAT_RGB565: u8 = 1;
    pub const FORMAT_GREY2: u8 = 2;

    /// Its format in words, for the settings; reserved and unknown ones aren't drawn.
    pub fn format_name(&self) -> &'static str {
        match self.format {
            Self::FORMAT_1BIT => "1-bit",
            Self::FORMAT_RGB565 => "RGB565 (reserved, unsupported)",
            Self::FORMAT_GREY2 => "2-bit grey (reserved, unsupported)",
            _ => "unknown format (unsupported)",
        }
    }

    /// D5 86: status, count, index, then the 24-byte descriptor. `Err` carries the status
    /// (5 = index past the end).
    pub fn parse(r: &[u8; REPORT]) -> std::result::Result<ScreenDescriptor, u8> {
        if r[2] != 0 {
            return Err(r[2]);
        }
        let d = &r[5..29];
        let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        let opt = |b: u8| if b == 0xff { None } else { Some(b) };
        Ok(ScreenDescriptor {
            count: r[3],
            index: r[4],
            id: d[0],
            kind: match d[1] {
                0 => ScreenKind::Display,
                1 => ScreenKind::Panel,
                2 => ScreenKind::Art,
                k => ScreenKind::Other(k),
            },
            half: d[2],
            parent: opt(d[3]),
            width: u16_at(4),
            height: u16_at(6),
            x: u16_at(8),
            y: u16_at(10),
            rotation: d[12],
            format: d[13],
            ops: u16_at(14),
            stride: u16_at(16),
            rows_per_report: d[18],
            readback_half: opt(d[19]),
            version: d[20],
        })
    }
}

pub fn art_begin(len: u16, crc: u32) -> Vec<u8> {
    let mut r = vec![ART, art::BEGIN];
    r.extend_from_slice(&len.to_le_bytes());
    r.extend_from_slice(&crc.to_le_bytes());
    r
}

pub fn art_data(offset: u16, chunk: &[u8]) -> Vec<u8> {
    let mut r = vec![ART, art::DATA];
    r.extend_from_slice(&offset.to_le_bytes());
    r.push(chunk.len() as u8);
    r.extend_from_slice(chunk);
    r
}

pub fn art_commit() -> Vec<u8> {
    vec![ART, art::COMMIT]
}

pub fn art_select(set: i8) -> Vec<u8> {
    vec![ART, art::SELECT, set as u8]
}

pub fn art_status() -> Vec<u8> {
    vec![ART, art::STATUS]
}

/// The part of `name` a flash slot keeps: at most [`art::NAME_LEN`] bytes, cut on a character
/// boundary. The UI uses this to show the cut before saving rather than after.
pub fn art_name_fit(name: &str) -> &str {
    let mut end = name.len().min(art::NAME_LEN);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// SAVE: `name` is cut with [`art_name_fit`] and zero-padded.
pub fn art_save(slot: u8, name: &str) -> Vec<u8> {
    let mut r = vec![ART, art::SAVE, slot];
    r.extend_from_slice(art_name_fit(name).as_bytes());
    r.resize(3 + art::NAME_LEN, 0);
    r
}

pub fn art_erase(slot: u8) -> Vec<u8> {
    vec![ART, art::ERASE, slot]
}

pub fn art_slot(slot: u8) -> Vec<u8> {
    vec![ART, art::SLOT, slot]
}

/// One flash slot on the right half (the D6 87 reply).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotInfo {
    pub slot: u8,
    pub used: bool,
    /// Erase/write cycles so far.
    pub writes: u32,
    pub len: u16,
    pub name: String,
}

impl SlotInfo {
    /// D6 87: status, slot, used, writes u32, len u16, name[16]. `Err` carries the status
    /// (1 busy, 2 no link, 3 BLE error, 4 right without slots).
    pub fn parse(r: &[u8; REPORT]) -> std::result::Result<SlotInfo, u8> {
        if r[2] != 0 {
            return Err(r[2]);
        }
        let name = &r[11..11 + art::NAME_LEN];
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        Ok(SlotInfo {
            slot: r[3],
            used: r[4] != 0,
            writes: u32::from_le_bytes(r[5..9].try_into().unwrap()),
            len: u16::from_le_bytes([r[9], r[10]]),
            name: String::from_utf8_lossy(&name[..end]).into_owned(),
        })
    }
}

/// RGB_SYNC: `Some` turns "right follows left" on or off, `None` only reads it.
pub fn rgb_sync(set: Option<bool>) -> Vec<u8> {
    vec![SETTINGS, settings::RGB_SYNC, set.map_or(0xff, u8::from)]
}

/// INVOKE: press behavior `local_id` (ZMK Studio's id) with these parameters, release it after
/// `hold_ms` (0 = a tap).
pub fn invoke(local_id: u16, param1: u32, param2: u32, hold_ms: u16) -> Vec<u8> {
    let mut r = vec![SETTINGS, settings::INVOKE];
    r.extend_from_slice(&local_id.to_le_bytes());
    r.extend_from_slice(&param1.to_le_bytes());
    r.extend_from_slice(&param2.to_le_bytes());
    r.extend_from_slice(&hold_ms.min(settings::MAX_HOLD_MS).to_le_bytes());
    r
}

/// How INVOKE went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvokeStatus {
    Ok,
    UnknownBehavior,
    /// A held invoke is still running.
    Busy,
    BehaviorFailed,
    NotBuiltIn,
    BadLength,
    Refused,
    Other(u8),
}

impl InvokeStatus {
    pub fn parse(r: &[u8; REPORT]) -> InvokeStatus {
        match r[2] {
            0 => Self::Ok,
            1 => Self::UnknownBehavior,
            2 => Self::Busy,
            3 => Self::BehaviorFailed,
            4 => Self::NotBuiltIn,
            5 => Self::BadLength,
            6 => Self::Refused,
            n => Self::Other(n),
        }
    }
}

impl std::fmt::Display for InvokeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Ok => write!(f, "done"),
            Self::UnknownBehavior => write!(f, "the keyboard doesn't know that behavior"),
            Self::Busy => write!(f, "busy with a held action"),
            Self::BehaviorFailed => write!(f, "the behavior failed"),
            Self::NotBuiltIn => write!(f, "needs newer keyboard firmware"),
            Self::BadLength => write!(f, "malformed request"),
            Self::Refused => write!(f, "the keyboard refused this action"),
            Self::Other(n) => write!(f, "error {n}"),
        }
    }
}

pub fn rgb_state() -> Vec<u8> {
    vec![SETTINGS, settings::RGB_STATE]
}

/// One half's underglow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RgbHalf {
    pub on: bool,
    /// 0-359
    pub hue: u16,
    /// 0-100
    pub sat: u8,
    /// 0-100
    pub brightness: u8,
    /// 0 solid, 1 breathe, 2 spectrum, 3 swirl
    pub effect: u8,
    /// 1-5; ZMK's last saved value, which can lag a minute behind a change.
    pub speed: u8,
}

impl RgbHalf {
    fn parse(b: &[u8]) -> RgbHalf {
        RgbHalf {
            on: b[0] != 0,
            hue: u16::from_le_bytes([b[1], b[2]]),
            sat: b[3],
            brightness: b[4],
            effect: b[5],
            speed: b[6],
        }
    }
}

/// Both halves' underglow; the right half's as last relayed, `right_age` seconds ago.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbState {
    pub left: RgbHalf,
    /// `None` when there's no data from the right half (no relay link yet, or old firmware).
    pub right: Option<(RgbHalf, u8)>,
}

impl RgbState {
    pub fn parse(r: &[u8; REPORT]) -> RgbState {
        RgbState {
            left: RgbHalf::parse(&r[2..9]),
            right: (r[9] != 0xff).then(|| (RgbHalf::parse(&r[10..17]), r[9])),
        }
    }
}

/// The keyboard's answer to RGB_SYNC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbSync {
    /// Whether the right half's underglow follows the left's; `None` if the firmware hasn't
    /// got the feature.
    pub sync: Option<bool>,
    /// The left half's underglow is on.
    pub left_on: bool,
    /// 0 = right half not connected, 1 = connected, 2 = connected with the relay.
    pub right_link: u8,
}

impl RgbSync {
    /// The reply: sync (0/1, 0xff = not built in), left on, right link.
    pub fn parse(r: &[u8; REPORT]) -> RgbSync {
        RgbSync {
            sync: (r[2] != 0xff).then_some(r[2] != 0),
            left_on: r[3] != 0,
            right_link: r[4],
        }
    }
}

/// The underglow's levels by power state (D7 0E, firmware battery_dim.h), as percentages of the
/// brightness the user picked: each half goes by its own cable and charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryDimConfig {
    /// Off: `power` everywhere, whatever the cable or charge.
    pub enabled: bool,
    /// On USB power, 1-100.
    pub power: u8,
    /// On battery at or above `low_below`, 1-100.
    pub battery: u8,
    /// On battery below `low_below`, 1-100.
    pub low: u8,
    /// Charge percent, 0-100 (0: no low level).
    pub low_below: u8,
}

impl Default for BatteryDimConfig {
    /// The firmware's defaults: 100% on power, 80% on battery, 12% below 20% charge.
    fn default() -> Self {
        Self {
            enabled: true,
            power: 100,
            battery: 80,
            low: 12,
            low_below: 20,
        }
    }
}

impl BatteryDimConfig {
    /// What the firmware takes: levels 1-100, threshold 0-100.
    pub fn valid(&self) -> bool {
        [self.power, self.battery, self.low]
            .iter()
            .all(|l| (1..=100).contains(l))
            && self.low_below <= 100
    }

    fn bytes(&self) -> [u8; 5] {
        [
            self.enabled.into(),
            self.power,
            self.battery,
            self.low,
            self.low_below,
        ]
    }

    fn parse(b: &[u8]) -> Self {
        Self {
            enabled: b[0] != 0,
            power: b[1],
            battery: b[2],
            low: b[3],
            low_below: b[4],
        }
    }
}

/// BATTERY_DIM: reads the levels (`None`) or sets them.
pub fn battery_dim(set: Option<BatteryDimConfig>) -> Vec<u8> {
    match set {
        None => vec![SETTINGS, settings::BATTERY_DIM, 0],
        Some(c) => {
            let mut r = vec![SETTINGS, settings::BATTERY_DIM, 1];
            r.extend_from_slice(&c.bytes());
            r
        }
    }
}

/// One half's battery dim state now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryDimHalf {
    pub usb: bool,
    /// At the low-battery level.
    pub low: bool,
    /// Percent of the user's brightness the underglow is drawn at now.
    pub level: u8,
    /// The charge the level went by; `None` before the half's first reading.
    pub charge: Option<u8>,
}

impl BatteryDimHalf {
    fn parse(b: &[u8]) -> Self {
        Self {
            usb: b[0] & 1 != 0,
            low: b[0] & 2 != 0,
            level: b[1],
            charge: (b[2] != 0 && b[2] <= 100).then_some(b[2]),
        }
    }
}

/// The reply to BATTERY_DIM (D7 8E), which also comes unsolicited when a half's state changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryDim {
    /// 0 ok, 4 not built in, 5 values refused.
    pub status: u8,
    /// As in force on the left.
    pub config: BatteryDimConfig,
    pub left: BatteryDimHalf,
    /// The right half's, `age` seconds old; `None` without a relay link or with right firmware
    /// that doesn't report it.
    pub right: Option<(BatteryDimHalf, u8)>,
    /// The right half has the left's levels.
    pub right_synced: bool,
}

impl BatteryDim {
    pub fn parse(r: &[u8; REPORT]) -> Self {
        let right = (r[11] != 0xff).then(|| (BatteryDimHalf::parse(&r[12..15]), r[11]));
        Self {
            status: r[2],
            config: BatteryDimConfig::parse(&r[3..8]),
            left: BatteryDimHalf::parse(&r[8..11]),
            right,
            right_synced: right.is_some() && r[15] != 0,
        }
    }
}

/// SET_FRAME (D7 0C, #272): per-LED colours for both halves.
///
/// Lamps are index-ordered: 0-20 are the left strip's LEDs in chain order, 21-41 the right's.
/// Which key each LED sits under isn't known yet (the #90 row G sitting), so that map belongs in
/// the board profile, not here. Colours are plain RGB; each half's battery dim scales them.
pub mod frame {
    /// Flag: draw the frame (on the last report of one).
    pub const SHOW: u8 = 1 << 0;
    /// Flag: hand both halves back to their own effect.
    pub const RELEASE: u8 = 1 << 1;
    /// Lamps per report: 27 bytes of RGB after the 5-byte header.
    pub const MAX_LAMPS: usize = 9;
    /// Lamps per half on the Eyelash Corne.
    pub const HALF_LAMPS: usize = 21;
}

/// FRAME reports for lamps `start..start + colours.len()`, [`frame::MAX_LAMPS`] per report,
/// with SHOW on the last. Lamps not sent keep their colour from the last frame.
pub fn frame(start: u8, colours: &[[u8; 3]]) -> Vec<Vec<u8>> {
    let chunks: Vec<&[[u8; 3]]> = colours.chunks(frame::MAX_LAMPS).collect();
    let mut at = start;
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let flags = if i + 1 == chunks.len() {
                frame::SHOW
            } else {
                0
            };
            let mut r = vec![SETTINGS, settings::FRAME, flags, at, chunk.len() as u8];
            r.extend(chunk.iter().flatten());
            at = at.wrapping_add(chunk.len() as u8);
            r
        })
        .collect()
}

/// Whether `r` is SET_FRAME's hand-back event (D7 8C with byte 6 = 1), which the keyboard sends
/// on its own after its timeout, a USB unplug or an endpoint change.
pub fn is_frame_event(r: &[u8]) -> bool {
    is_reply(r, SETTINGS, settings::FRAME) && r.get(6).is_some_and(|&e| e != 0)
}

/// FRAME RELEASE: both halves go back to their own effect.
pub fn frame_release() -> Vec<u8> {
    vec![SETTINGS, settings::FRAME, frame::RELEASE]
}

/// Whether the right half draws frames (the D7 8C reply's byte 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameRight {
    /// No relay link to the right yet.
    NoLink,
    Draws,
    /// The right's firmware can't: it keeps its own effect.
    Unsupported,
}

/// The reply to FRAME (D7 8C), and the hand-back event (`event`) the keyboard sends on its own
/// after its timeout, a USB unplug or an endpoint change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameReply {
    /// 0 ok, 4 not built in, 5 bad request.
    pub status: u8,
    /// [`FrameReply::OWNER_HOST_LINK`] while the PC's frames show; 0 is the keyboard's effect.
    pub owner: u8,
    /// Seconds without a FRAME report before the keyboard takes its lights back; 0 = never.
    pub timeout_s: u8,
    pub right: FrameRight,
    /// Sent by the keyboard on its own, not a reply.
    pub event: bool,
}

impl FrameReply {
    /// The owner byte when the host link has the lights.
    pub const OWNER_HOST_LINK: u8 = 3;

    pub fn parse(r: &[u8; REPORT]) -> Self {
        Self {
            status: r[2],
            owner: r[3],
            timeout_s: r[4],
            right: match r[5] {
                1 => FrameRight::Draws,
                2 => FrameRight::Unsupported,
                _ => FrameRight::NoLink,
            },
            event: r[6] != 0,
        }
    }

    /// Whether the PC's frames are showing.
    pub fn held(&self) -> bool {
        self.owner == Self::OWNER_HOST_LINK
    }
}

/// SET_EFFECT (D7 0F, #490): an effect sent once that each half draws from its LEDs' positions
/// and the scene's clock (`crates/scene`: the same maths, held to its vectors in the firmware's
/// simulator). Nothing streams: it runs on with the app closed until [`effect_release`], a
/// SET_FRAME, or a reboot.
pub mod effect {
    pub const RELEASE: u8 = 0;
    pub const PLACE: u8 = 1;
    pub const EFFECT: u8 = 2;
    /// Colours an effect carries at most (a sweep's stops).
    pub const MAX_COLOURS: usize = 4;
    /// The longest period (and clock) the wire carries: 24 bits of milliseconds.
    pub const MAX_PERIOD_MS: u32 = 0xFF_FFFF;
}

/// The effects the keyboard draws, in crates/scene's terms (effects.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    /// A soft band of colours[0] over colours[1], crossing the room once per period.
    Scanline = 1,
    /// 1-4 stops scrolling across the room, wrapping, once per period.
    Sweep = 2,
    /// A ring of colours[0] over colours[1] growing from the origin to the far corner.
    Ripple = 3,
    /// colours[0] to colours[1] and back once per period.
    Breathe = 4,
}

/// One effect, in the wire's units.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub kind: EffectKind,
    /// Scanline, ripple: colour, background. Breathe: from, to. Sweep: 1-4 stops.
    pub colours: Vec<[u8; 3]>,
    /// The scene's time modulo the period, at the moment this is sent (`SceneTime::ms`
    /// `.rem_euclid(period)`); 0 for a still effect.
    pub clock_ms: u32,
    /// 0 stands still; at most [`effect::MAX_PERIOD_MS`].
    pub period_ms: u32,
    /// Degrees: 0 left to right, 90 top to bottom.
    pub angle_deg: f32,
    /// The band's or ring's width in mm.
    pub width_mm: f32,
    /// The ripple's centre, room mm.
    pub origin_mm: [i16; 2],
}

/// A rectangle in room mm: x, y, w, h.
pub type MmRect = [i16; 4];

/// EFFECT PLACE: the room's bounds and where each half sits in it (crates/scene's room map).
pub fn effect_place(room: MmRect, left: MmRect, right: MmRect) -> Vec<u8> {
    let mut r = vec![SETTINGS, settings::EFFECT, effect::PLACE];
    for v in room.iter().chain(&left).chain(&right) {
        r.extend(v.to_le_bytes());
    }
    r
}

/// EFFECT: draws `e` on both halves. `None` when it doesn't fit the wire (colour count, period).
pub fn effect(e: &Effect) -> Option<Vec<u8>> {
    let n = e.colours.len();
    let colours_ok = match e.kind {
        EffectKind::Sweep => (1..=effect::MAX_COLOURS).contains(&n),
        _ => n == 2,
    };
    if !colours_ok || e.period_ms > effect::MAX_PERIOD_MS {
        return None;
    }
    let clock = if e.period_ms == 0 {
        0
    } else {
        e.clock_ms % e.period_ms
    };
    let angle = if e.angle_deg.is_finite() {
        ((e.angle_deg.rem_euclid(360.0) * 100.0).round() as u32 % 36_000) as u16
    } else {
        0
    };
    let width = if e.width_mm.is_finite() {
        (e.width_mm.max(0.0) * 10.0)
            .round()
            .min(f32::from(u16::MAX)) as u16
    } else {
        0
    };
    let mut r = vec![
        SETTINGS,
        settings::EFFECT,
        effect::EFFECT,
        e.kind as u8,
        n as u8,
    ];
    r.extend(&clock.to_le_bytes()[..3]);
    r.extend(&e.period_ms.to_le_bytes()[..3]);
    r.extend(angle.to_le_bytes());
    r.extend(width.to_le_bytes());
    r.extend(e.origin_mm[0].to_le_bytes());
    r.extend(e.origin_mm[1].to_le_bytes());
    r.extend(e.colours.iter().flatten());
    Some(r)
}

/// EFFECT RELEASE: both halves back to their own effect.
pub fn effect_release() -> Vec<u8> {
    vec![SETTINGS, settings::EFFECT, effect::RELEASE]
}

/// The reply to EFFECT (D7 8F).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectReply {
    /// 0 ok, 4 not built in, 5 bad request.
    pub status: u8,
    /// [`FrameReply::OWNER_HOST_LINK`] while the PC's effect shows; 0 is the keyboard's own.
    pub owner: u8,
    /// The kind running ([`EffectKind`] as a byte), 0 for none.
    pub kind: u8,
    /// Whether the right half draws it (as for frames).
    pub right: FrameRight,
}

impl EffectReply {
    pub fn parse(r: &[u8; REPORT]) -> Self {
        Self {
            status: r[2],
            owner: r[3],
            kind: r[4],
            right: match r[5] {
                1 => FrameRight::Draws,
                2 => FrameRight::Unsupported,
                _ => FrameRight::NoLink,
            },
        }
    }

    /// Whether the PC's effect is showing.
    pub fn running(&self) -> bool {
        self.owner == FrameReply::OWNER_HOST_LINK
    }
}

/// What the keyboard said to HELLO.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hello {
    pub version: u8,
    pub width: usize,
    pub height: usize,
    /// 0 = right half not connected, 1 = connected, 2 = connected with the art relay.
    pub right_link: u8,
    pub right_battery: Option<u8>,
    /// How the left's search for the right's relay service went (art_relay_discovery).
    pub discovery: i8,
    pub discovery_tries: u8,
    /// art_relay_debug: chrcs, ends, last ret, ATT err, timeouts, answered, in flight,
    /// security, MTU u16
    pub debug: [u8; 10],
    /// Zero for older firmware, which pads the appended field with zeros.
    pub capabilities: u32,
    /// Firmware panel timeout in seconds; zero keeps the frame indefinitely.
    pub panel_timeout_s: u16,
    /// The left's firmware version (bytes 27-30); `None` from firmware older than that.
    pub firmware: Option<FirmwareVersion>,
}

/// A half's firmware version and build profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    /// 0 = development build with test hooks, 1 = GA.
    pub profile: u8,
}

impl FirmwareVersion {
    fn parse(b: &[u8]) -> Option<FirmwareVersion> {
        (b[..4] != [0, 0, 0, 0]).then(|| FirmwareVersion {
            major: b[0],
            minor: b[1],
            patch: b[2],
            profile: b[3],
        })
    }
}

impl std::fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.profile == 0 {
            write!(f, " (dev)")?;
        }
        Ok(())
    }
}

pub fn build_info(half: u8) -> Vec<u8> {
    vec![SETTINGS, settings::BUILD_INFO, half]
}

/// One half's build (the D7 89 reply).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfo {
    /// 0 = left, 1 = right.
    pub half: u8,
    pub version: FirmwareVersion,
    /// Short git SHA.
    pub sha: String,
}

impl BuildInfo {
    /// D7 89: status, half, major, minor, patch, profile, sha[8]. `Err` carries the status
    /// (1 busy, 2 no relay link, 3 relay error, 4 right firmware without it, 5 bad half).
    pub fn parse(r: &[u8; REPORT]) -> std::result::Result<BuildInfo, u8> {
        if r[2] != 0 {
            return Err(r[2]);
        }
        let sha = &r[8..16];
        let end = sha.iter().position(|&b| b == 0).unwrap_or(sha.len());
        Ok(BuildInfo {
            half: r[3],
            version: FirmwareVersion {
                major: r[4],
                minor: r[5],
                patch: r[6],
                profile: r[7],
            },
            sha: String::from_utf8_lossy(&sha[..end]).into_owned(),
        })
    }
}

impl Hello {
    /// The HELLO reply: ver, panel w, h, x, y, right link, right battery %, discovery, tries,
    /// then relay debug counters, capability bits (u32 LE), panel timeout seconds (u16 LE).
    pub fn parse(r: &[u8; REPORT]) -> Hello {
        Hello {
            version: r[2],
            width: r[3] as usize,
            height: r[4] as usize,
            right_link: r[7],
            // 0xff: no level yet; 0: ZMK's value before the right's first reading.
            right_battery: (1..=100).contains(&r[8]).then_some(r[8]),
            discovery: r[9] as i8,
            discovery_tries: r[10],
            debug: r[11..21].try_into().unwrap(),
            capabilities: u32::from_le_bytes(r[21..25].try_into().unwrap()),
            panel_timeout_s: u16::from_le_bytes(r[25..27].try_into().unwrap()),
            firmware: FirmwareVersion::parse(&r[27..31]),
        }
    }

    pub fn discovery_text(&self) -> String {
        match self.discovery {
            0 => "not started".into(),
            1 => "running".into(),
            2 => "service not found".into(),
            3 => "service found, characteristics missing".into(),
            4 => "found".into(),
            e => format!("bt_gatt_discover error {e}"),
        }
    }
}

/// The art relay's report (art_relay_report in art_relay_central.c).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArtStatus {
    pub state: u8,
    pub error: u8,
    pub length: u16,
    pub pushed: u16,
    pub link: u8,
    pub right_state: u8,
    pub right_received: u16,
    pub right_capacity: u16,
    pub right_set: i8,
    pub right_set_count: u8,
    /// Right status version 3 and later: flash slots (0 = none) and which hold a set (bit n).
    /// While slot n plays, `right_set` is `right_set_count + n`.
    pub flash_slots: u8,
    pub flash_used: u8,
}

pub const LEFT_STATES: [&str; 5] = ["idle", "receiving", "pushing", "done", "failed"];
pub const ERRORS: [&str; 8] = [
    "none",
    "right half not connected or has no relay",
    "too big",
    "busy",
    "data out of range",
    "CRC mismatch on the left",
    "Bluetooth write failed",
    "right half rejected it",
];
pub const RIGHT_STATES: [&str; 9] = [
    "built-in",
    "receiving",
    "playing pushed set",
    "bad CRC",
    "bad blob",
    "too big",
    "saving",
    "saved",
    "save failed",
];

/// The right half's art states (art_relay.h `art_relay_right_state`).
pub mod right_state {
    pub const BUILT_IN: u8 = 0;
    pub const PLAYING: u8 = 2;
    pub const SAVING: u8 = 6;
    pub const SAVED: u8 = 7;
    pub const SAVE_FAILED: u8 = 8;
}

/// Left-half relay states (enum art_relay_left_state).
pub mod left_state {
    pub const IDLE: u8 = 0;
    pub const RECEIVING: u8 = 1;
    pub const PUSHING: u8 = 2;
    pub const DONE: u8 = 3;
    pub const FAILED: u8 = 4;
}

fn name(table: &[&'static str], i: u8) -> &'static str {
    table.get(i as usize).copied().unwrap_or("?")
}

impl ArtStatus {
    /// The reply to any 0xD6 command: state, error, length u16, pushed u16, link, then
    /// struct art_relay_status (version, state, received u16, capacity u16, set, set_count).
    pub fn parse(r: &[u8; REPORT]) -> ArtStatus {
        let s = &r[2..];
        ArtStatus {
            state: s[0],
            error: s[1],
            length: u16::from_le_bytes([s[2], s[3]]),
            pushed: u16::from_le_bytes([s[4], s[5]]),
            link: s[6],
            right_state: s[8],
            right_received: u16::from_le_bytes([s[9], s[10]]),
            right_capacity: u16::from_le_bytes([s[11], s[12]]),
            right_set: s[13] as i8,
            right_set_count: s[14],
            // After the version-2 underglow (7 bytes): flash_slots, flash_used.
            flash_slots: if s[7] >= 3 { s[22] } else { 0 },
            flash_used: if s[7] >= 3 { s[23] } else { 0 },
        }
    }
}

impl std::fmt::Display for ArtStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "left: {}, {}/{} bytes pushed, error: {}; right: {}",
            name(&LEFT_STATES, self.state),
            self.pushed,
            self.length,
            name(&ERRORS, self.error),
            match self.link {
                0 => "not connected".to_string(),
                1 => "connected, no relay (old firmware?)".to_string(),
                _ => format!(
                    "{}, set {}, {} built-in, slot {} bytes",
                    name(&RIGHT_STATES, self.right_state),
                    self.right_set,
                    self.right_set_count,
                    self.right_capacity
                ),
            }
        )
    }
}

#[cfg(test)]
mod tests {
    //! The bytes hostpanel v0.1 sent and read; the firmware depends on them.
    use super::*;

    #[test]
    fn panel_reports() {
        assert_eq!(hello(), [0xD5, 0x01, 0x01]);
        assert_eq!(fill(true), [0xD5, 0x02, 0x01]);
        assert_eq!(present(), [0xD5, 0x04]);
        assert_eq!(release(), [0xD5, 0x05]);
        let data: Vec<u8> = (0..24).collect();
        let mut want = vec![0xD5, 0x03, 49, 3];
        want.extend_from_slice(&data);
        assert_eq!(rows(49, &data, 8), want);
        assert_eq!(panel::rows_per_report(8), 3);
        assert_eq!(panel::rows_per_report(9), 3);
        assert_eq!(panel::rows_per_report(10), 2);
    }

    #[test]
    fn art_reports() {
        assert_eq!(
            art_begin(0x1234, 0xAABBCCDD),
            [0xD6, 0x01, 0x34, 0x12, 0xDD, 0xCC, 0xBB, 0xAA]
        );
        assert_eq!(
            art_data(0x0102, &[7, 8, 9]),
            [0xD6, 0x02, 0x02, 0x01, 3, 7, 8, 9]
        );
        assert_eq!(art_commit(), [0xD6, 0x03]);
        assert_eq!(art_select(-1), [0xD6, 0x04, 0xFF]);
        assert_eq!(art_select(5), [0xD6, 0x04, 5]);
        assert_eq!(art_status(), [0xD6, 0x05]);
    }

    #[test]
    fn settings_reports() {
        assert_eq!(rgb_sync(None), [0xD7, 0x01, 0xFF]);
        assert_eq!(rgb_sync(Some(true)), [0xD7, 0x01, 1]);
        assert_eq!(rgb_sync(Some(false)), [0xD7, 0x01, 0]);
        let mut r = [0u8; REPORT];
        r[..5].copy_from_slice(&[0xD7, 0x81, 1, 1, 2]);
        assert_eq!(
            RgbSync::parse(&r),
            RgbSync {
                sync: Some(true),
                left_on: true,
                right_link: 2
            }
        );
        r[2] = 0xff;
        assert_eq!(RgbSync::parse(&r).sync, None);

        assert_eq!(
            invoke(0x0102, 0x0A0B_0C0D, 2, 2100),
            [0xD7, 0x02, 0x02, 0x01, 0x0D, 0x0C, 0x0B, 0x0A, 2, 0, 0, 0, 0x34, 0x08]
        );
        assert_eq!(invoke(1, 0, 0, 60_000)[12..14], 10_000u16.to_le_bytes());
        r[..4].copy_from_slice(&[0xD7, 0x82, 2, 0]);
        assert_eq!(InvokeStatus::parse(&r), InvokeStatus::Busy);

        assert_eq!(rgb_state(), [0xD7, 0x03]);
        let mut r = [0u8; REPORT];
        r[..17].copy_from_slice(&[
            0xD7, 0x83, 1, 0x68, 0x01, 80, 50, 3, 2, 4, 0, 0x2C, 0x01, 100, 20, 0, 1,
        ]);
        let s = RgbState::parse(&r);
        assert_eq!(
            s.left,
            RgbHalf {
                on: true,
                hue: 360,
                sat: 80,
                brightness: 50,
                effect: 3,
                speed: 2
            }
        );
        assert_eq!(
            s.right,
            Some((
                RgbHalf {
                    on: false,
                    hue: 300,
                    sat: 100,
                    brightness: 20,
                    effect: 0,
                    speed: 1
                },
                4
            ))
        );
        r[9] = 0xff;
        assert_eq!(RgbState::parse(&r).right, None);
    }

    #[test]
    fn effect_reports() {
        let place = effect_place([0, 0, 400, 100], [10, 5, 120, 90], [270, -5, 120, 90]);
        assert_eq!(place.len(), 27);
        assert_eq!(&place[..7], [0xD7, 0x0F, 1, 0, 0, 0, 0]);
        assert_eq!(&place[19..23], [14, 1, 0xFB, 0xFF]);
        let scan = Effect {
            kind: EffectKind::Scanline,
            colours: vec![[0xF0, 0x8A, 0x3C], [6, 8, 14]],
            clock_ms: 7_250,
            period_ms: 3_000,
            angle_deg: -90.0,
            width_mm: 60.04,
            origin_mm: [0, 0],
        };
        let r = effect(&scan).unwrap();
        assert_eq!(r.len(), 25);
        assert!(r.len() <= REPORT);
        // Clock 7250 mod 3000 = 1250; angle -90 = 270.00; width 60.0 mm = 600.
        assert_eq!(
            &r[..19],
            [
                0xD7, 0x0F, 2, 1, 2, 0xE2, 0x04, 0, 0xB8, 0x0B, 0, 0x78, 0x69, 0x58, 0x02, 0, 0, 0,
                0
            ]
        );
        assert_eq!(&r[19..], [0xF0, 0x8A, 0x3C, 6, 8, 14]);
        // A sweep of four stops fills the report; five, or a breathe of three, don't fit.
        let sweep = Effect {
            kind: EffectKind::Sweep,
            colours: vec![[1, 2, 3]; 4],
            period_ms: 0,
            clock_ms: 99,
            ..scan.clone()
        };
        let r = effect(&sweep).unwrap();
        assert_eq!(r.len(), REPORT - 1);
        assert_eq!(&r[5..8], [0, 0, 0], "a still effect's clock is 0");
        assert!(effect(&Effect {
            colours: vec![[0; 3]; 5],
            ..sweep.clone()
        })
        .is_none());
        assert!(effect(&Effect {
            kind: EffectKind::Breathe,
            colours: vec![[0; 3]; 3],
            ..scan.clone()
        })
        .is_none());
        assert!(effect(&Effect {
            period_ms: effect::MAX_PERIOD_MS + 1,
            ..scan.clone()
        })
        .is_none());
        assert_eq!(effect_release(), [0xD7, 0x0F, 0]);

        let mut r = [0u8; REPORT];
        r[..6].copy_from_slice(&[0xD7, 0x8F, 0, 3, 1, 1]);
        let reply = EffectReply::parse(&r);
        assert!(reply.running());
        assert_eq!((reply.kind, reply.right), (1, FrameRight::Draws));
        r[3] = 0;
        r[5] = 2;
        let back = EffectReply::parse(&r);
        assert!(!back.running());
        assert_eq!(back.right, FrameRight::Unsupported);
    }

    #[test]
    fn frame_reports() {
        // A whole frame: 42 lamps in 5 reports, SHOW on the last.
        let lamps: Vec<[u8; 3]> = (0..42u8).map(|i| [i, 0, 255 - i]).collect();
        let reports = frame(0, &lamps);
        assert_eq!(reports.len(), 5);
        assert_eq!(
            reports
                .iter()
                .map(|r| (r[2], r[3], r[4]))
                .collect::<Vec<_>>(),
            [(0, 0, 9), (0, 9, 9), (0, 18, 9), (0, 27, 9), (1, 36, 6)]
        );
        assert!(reports.iter().all(|r| r.len() <= REPORT));
        assert_eq!(&reports[1][..8], [0xD7, 0x0C, 0, 9, 9, 9, 0, 246]);
        // One lamp that changed: one report that shows.
        assert_eq!(
            frame(30, &[[0, 0, 255]]),
            [vec![0xD7, 0x0C, 1, 30, 1, 0, 0, 255]]
        );
        assert_eq!(frame_release(), [0xD7, 0x0C, 2]);

        let mut r = [0u8; REPORT];
        r[..6].copy_from_slice(&[0xD7, 0x8C, 0, 3, 30, 1]);
        let reply = FrameReply::parse(&r);
        assert_eq!(
            reply,
            FrameReply {
                status: 0,
                owner: 3,
                timeout_s: 30,
                right: FrameRight::Draws,
                event: false
            }
        );
        assert!(reply.held());
        // The timeout's hand-back: the keyboard's own effect, sent unasked.
        r[..7].copy_from_slice(&[0xD7, 0x8C, 0, 0, 30, 2, 1]);
        let back = FrameReply::parse(&r);
        assert!(!back.held() && back.event);
        assert_eq!(back.right, FrameRight::Unsupported);
        assert!(is_frame_event(&r));
        r[6] = 0;
        assert!(!is_frame_event(&r));
    }

    #[test]
    fn battery_dim_reports() {
        assert_eq!(battery_dim(None), [0xD7, 0x0E, 0]);
        assert_eq!(
            battery_dim(Some(BatteryDimConfig::default())),
            [0xD7, 0x0E, 1, 1, 100, 80, 12, 20]
        );
        let off = BatteryDimConfig {
            enabled: false,
            ..Default::default()
        };
        assert_eq!(battery_dim(Some(off))[3], 0);
        assert!(BatteryDimConfig::default().valid());
        assert!(!BatteryDimConfig {
            low: 0,
            ..Default::default()
        }
        .valid());
        assert!(!BatteryDimConfig {
            low_below: 101,
            ..Default::default()
        }
        .valid());

        // Left on USB, no reading yet; right on battery at 15%, at the low level, 3 s old, in
        // sync.
        let mut r = [0u8; REPORT];
        r[..16].copy_from_slice(&[
            0xD7, 0x8E, 0, 1, 100, 80, 12, 20, 1, 100, 0, 3, 2, 12, 15, 1,
        ]);
        let d = BatteryDim::parse(&r);
        assert_eq!(d.status, 0);
        assert_eq!(d.config, BatteryDimConfig::default());
        assert_eq!(
            d.left,
            BatteryDimHalf {
                usb: true,
                low: false,
                level: 100,
                charge: None
            }
        );
        assert_eq!(
            d.right,
            Some((
                BatteryDimHalf {
                    usb: false,
                    low: true,
                    level: 12,
                    charge: Some(15)
                },
                3
            ))
        );
        assert!(d.right_synced);
        r[11] = 0xff;
        let d = BatteryDim::parse(&r);
        assert_eq!(d.right, None);
        assert!(!d.right_synced, "no right, nothing to be in sync with");
    }

    #[test]
    fn replies_match_on_proto_and_command() {
        assert!(is_reply(&[0xD5, 0x81], PANEL, panel::HELLO));
        assert!(!is_reply(&[0xD5, 0x01], PANEL, panel::HELLO));
        assert!(!is_reply(&[0xFF, 0x81], PANEL, panel::HELLO)); // KeyPeek's layer report
        assert!(!is_reply(&[0xD6], ART, art::STATUS));
    }

    #[test]
    fn parses_hello() {
        let mut r = [0u8; REPORT];
        r[..11].copy_from_slice(&[0xD5, 0x81, 1, 64, 52, 2, 106, 2, 93, 4, 1]);
        r[11..21].copy_from_slice(&[3, 1, 0, 0, 0, 5, 0, 2, 247, 0]);
        let h = Hello::parse(&r);
        assert_eq!(h.capabilities, 0);
        assert_eq!((h.version, h.width, h.height), (1, 64, 52));
        assert_eq!((h.right_link, h.right_battery), (2, Some(93)));
        assert_eq!(h.discovery_text(), "found");
        assert_eq!(h.capabilities, 0);
        assert_eq!(h.discovery_tries, 1);
        assert_eq!(u16::from_le_bytes([h.debug[8], h.debug[9]]), 247);

        r[8] = 0xff;
        r[9] = (-5i8) as u8;
        let h = Hello::parse(&r);
        assert_eq!(h.right_battery, None);
        assert_eq!(h.discovery_text(), "bt_gatt_discover error -5");
    }

    #[test]
    fn hello_timeout_is_optional_and_little_endian() {
        let mut r = [0; REPORT];
        assert_eq!(Hello::parse(&r).panel_timeout_s, 0);
        r[25..27].copy_from_slice(&3600u16.to_le_bytes());
        assert_eq!(Hello::parse(&r).panel_timeout_s, 3600);
    }

    #[test]
    fn parses_screen_descriptors() {
        assert_eq!(screens(1), [0xD5, 0x06, 1]);
        // The Corne's host panel, as host_link.c describes it.
        let mut r = [0u8; REPORT];
        r[..5].copy_from_slice(&[0xD5, 0x86, 0, 4, 1]);
        r[5..29].copy_from_slice(&[
            1, 1, 0, 0, 64, 0, 52, 0, 2, 0, 106, 0, 0, 0, 1, 0, 8, 0, 3, 0xff, 1, 0, 0, 0,
        ]);
        let s = ScreenDescriptor::parse(&r).unwrap();
        assert_eq!(
            (s.count, s.index, s.id, s.kind),
            (4, 1, 1, ScreenKind::Panel)
        );
        assert_eq!(
            (s.parent, s.width, s.height, s.x, s.y),
            (Some(0), 64, 52, 2, 106)
        );
        assert_eq!(
            (s.ops, s.stride, s.rows_per_report),
            (ScreenDescriptor::OP_ROWS, 8, 3)
        );
        assert_eq!((s.readback_half, s.version), (None, 1));
        r[2] = 5;
        assert_eq!(ScreenDescriptor::parse(&r), Err(5));
    }

    #[test]
    fn parses_hello_capabilities() {
        let mut r = [0u8; REPORT];
        let caps = capabilities::PANEL
            | capabilities::ART_RELAY
            | capabilities::SETTINGS_RGB
            | capabilities::ART_STATUS_EVENT
            | (1 << 31); // Preserve unknown future bits.
        r[21..25].copy_from_slice(&caps.to_le_bytes());
        assert_eq!(Hello::parse(&r).capabilities, caps);
        r[21..25].copy_from_slice(&capabilities::PANEL.to_le_bytes());
        assert_eq!(Hello::parse(&r).capabilities, capabilities::PANEL);
    }

    #[test]
    fn parses_art_status() {
        let mut r = [0u8; REPORT];
        // state pushing, no error, 4000 bytes, 1500 pushed, link 2,
        // right: version 1, receiving, 1450 received, 16384 capacity, set -1, 34 sets
        r[..17].copy_from_slice(&[
            0xD6, 0x85, 2, 0, 0xA0, 0x0F, 0xDC, 0x05, 2, 1, 1, 0xAA, 0x05, 0x00, 0x40, 0xFF, 34,
        ]);
        let s = ArtStatus::parse(&r);
        assert_eq!(
            s,
            ArtStatus {
                state: 2,
                error: 0,
                length: 4000,
                pushed: 1500,
                link: 2,
                right_state: 1,
                right_received: 1450,
                right_capacity: 16384,
                right_set: -1,
                right_set_count: 34,
                flash_slots: 0,
                flash_used: 0,
            }
        );
        assert_eq!(
            s.to_string(),
            "left: pushing, 1500/4000 bytes pushed, error: none; right: receiving, set -1, 34 built-in, slot 16384 bytes"
        );
    }
}

/// HELLO capability for connection state queries.
pub const CAP_CONN_STATE: u32 = capabilities::CONN_STATE;

// BOOTLOADER layout (host_link.h, #111):
//   ARM      D7 08 00                    -> D7 88 status nonce u32 LE
//   TRIGGER  D7 08 01 half nonce u32 LE  -> D7 88 status half
// status: 0 ok, 2 no relay link (right), 3 relay timeout, 4 not built in, 5 bad request,
// 6 wrong/expired nonce. The left reboots ~300 ms after its TRIGGER reply.
pub const BOOTLOADER_ARM: u8 = 0x00;
pub const BOOTLOADER_TRIGGER: u8 = 0x01;

pub fn bootloader_arm() -> Vec<u8> {
    vec![SETTINGS, settings::BOOTLOADER, BOOTLOADER_ARM]
}

/// `half`: 0 left, 1 right.
pub fn bootloader_trigger(half: u8, nonce: u32) -> Vec<u8> {
    let mut r = vec![SETTINGS, settings::BOOTLOADER, BOOTLOADER_TRIGGER, half];
    r.extend_from_slice(&nonce.to_le_bytes());
    r
}

pub fn conn_state() -> Vec<u8> {
    vec![SETTINGS, settings::CONN_STATE]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnState {
    pub active_profile: u8,
    pub connected: bool,
    pub bonded: bool,
    /// 0 = USB, 1 = BLE.
    pub endpoint: u8,
    pub usb_ready: bool,
    pub profile_count: u8,
}

/// The left half's battery % from a CONN_STATE reply, when the firmware sends it
/// ([`capabilities::LEFT_BATTERY`]; older firmware pads that byte with zero). 0 is "no reading
/// yet": ZMK reports 0 until its first battery sample, right after a boot or a flash.
pub fn left_battery(r: &[u8; REPORT], capabilities: u32) -> Option<u8> {
    (capabilities & capabilities::LEFT_BATTERY != 0 && (1..=100).contains(&r[8])).then_some(r[8])
}

impl ConnState {
    pub fn parse(r: &[u8; REPORT]) -> Self {
        Self {
            active_profile: r[2],
            connected: r[3] != 0,
            bonded: r[4] != 0,
            endpoint: r[5],
            usb_ready: r[6] != 0,
            profile_count: r[7],
        }
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    #[test]
    fn connection_wire_format_and_capabilities() {
        assert_eq!(conn_state(), [0xd7, 4]);
        let mut r = [0; REPORT];
        r[..8].copy_from_slice(&[0xd7, 0x84, 3, 1, 0, 1, 0, 5]);
        assert_eq!(
            ConnState::parse(&r),
            ConnState {
                active_profile: 3,
                connected: true,
                bonded: false,
                endpoint: 1,
                usb_ready: false,
                profile_count: 5,
            }
        );
        assert_eq!(left_battery(&r, 0), None, "old firmware: no left level");
        r[8] = 87;
        assert_eq!(left_battery(&r, capabilities::LEFT_BATTERY), Some(87));
        assert_eq!(
            left_battery(&r, 0),
            None,
            "without the bit, byte 8 means nothing"
        );
        r[8] = 0xff;
        assert_eq!(left_battery(&r, capabilities::LEFT_BATTERY), None);
        r[8] = 0;
        assert_eq!(
            left_battery(&r, capabilities::LEFT_BATTERY),
            None,
            "0 is ZMK's value before its first battery reading"
        );
        r[8] = 0;
        r[21..25].copy_from_slice(&0x12345610u32.to_le_bytes());
        assert_eq!(Hello::parse(&r).capabilities, 0x12345610);
        assert_ne!(Hello::parse(&r).capabilities & CAP_CONN_STATE, 0);
        r[2] = 6;
        assert_eq!(InvokeStatus::parse(&r), InvokeStatus::Refused);
        assert!(InvokeStatus::Refused.to_string().contains("refused"));
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;

    #[test]
    fn slot_reports_round_trip() {
        let save = art_save(2, "fox ünïcode name too long");
        assert_eq!(&save[..3], &[ART, art::SAVE, 2]);
        assert_eq!(save.len(), 3 + art::NAME_LEN);
        assert!(std::str::from_utf8(save[3..].split(|&b| b == 0).next().unwrap()).is_ok());
        assert_eq!(art_name_fit("hearth-moongarden"), "hearth-moongarde");
        assert_eq!(art_name_fit("short"), "short");
        assert_eq!(art_name_fit("fifteen chars!!ü"), "fifteen chars!!"); // ü would cross byte 16
        assert_eq!(art_name_fit("exactly 16 chars"), "exactly 16 chars");
        assert_eq!(art_erase(1), [ART, art::ERASE, 1]);
        let mut r = [0u8; REPORT];
        r[..11].copy_from_slice(&[0xD6, 0x87, 0, 3, 1, 5, 0, 0, 0, 0x10, 0x27]);
        r[11..14].copy_from_slice(b"fox");
        assert_eq!(
            SlotInfo::parse(&r),
            Ok(SlotInfo {
                slot: 3,
                used: true,
                writes: 5,
                len: 10000,
                name: "fox".into()
            })
        );
        r[2] = 4;
        assert_eq!(SlotInfo::parse(&r), Err(4));
    }

    #[test]
    fn status_v3_reports_flash_slots() {
        let mut r = [0u8; REPORT];
        r[..17].copy_from_slice(&[0xD6, 0x85, 0, 0, 0, 0, 0, 0, 2, 3, 2, 0, 0, 0, 0x40, 18, 17]);
        r[24] = 8;
        r[25] = 0b101;
        let s = ArtStatus::parse(&r);
        assert_eq!((s.flash_slots, s.flash_used, s.right_set), (8, 0b101, 18));
        r[9] = 2; // version 2: the bytes are underglow, not slots
        assert_eq!(ArtStatus::parse(&r).flash_slots, 0);
    }
}

#[cfg(test)]
mod build_info_tests {
    use super::*;

    #[test]
    fn build_info_and_hello_version_parse() {
        let mut r = [0u8; REPORT];
        r[..16].copy_from_slice(&[
            0xD7, 0x89, 0, 1, 0, 3, 2, 1, b'a', b'b', b'c', b'1', b'2', b'3', b'4', 0,
        ]);
        let b = BuildInfo::parse(&r).unwrap();
        assert_eq!(
            (b.half, b.version.to_string(), b.sha.as_str()),
            (1, "0.3.2".into(), "abc1234")
        );
        r[2] = 4;
        assert_eq!(BuildInfo::parse(&r), Err(4));
        let mut h = [0u8; REPORT];
        assert_eq!(Hello::parse(&h).firmware, None, "older firmware");
        h[27..31].copy_from_slice(&[1, 0, 0, 0]);
        assert_eq!(
            Hello::parse(&h).firmware.unwrap().to_string(),
            "1.0.0 (dev)"
        );
        assert_eq!(build_info(0), [0xD7, 0x09, 0]);
    }
}
