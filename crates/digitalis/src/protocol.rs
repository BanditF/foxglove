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
    /// Settings BOOTLOADER (D7 08), pre-GA test builds only.
    pub const BOOTLOADER: u32 = 1 << 9;
}
pub const VERSION: u8 = 1;

pub mod panel {
    pub const HELLO: u8 = 0x01;
    pub const FILL: u8 = 0x02;
    pub const ROWS: u8 = 0x03;
    pub const PRESENT: u8 = 0x04;
    pub const RELEASE: u8 = 0x05;
    /// Rows per ROWS report: what fits after the 4-byte header, at most 3.
    pub fn rows_per_report(stride: usize) -> usize {
        ((super::REPORT - 4) / stride).min(3)
    }
}

pub mod art {
    pub const BEGIN: u8 = 0x01;
    pub const DATA: u8 = 0x02;
    pub const COMMIT: u8 = 0x03;
    pub const SELECT: u8 = 0x04;
    pub const STATUS: u8 = 0x05;
    pub const DATA_MAX: usize = 27;
}

pub mod settings {
    /// Right half's underglow follows the left's on/off.
    pub const RGB_SYNC: u8 = 0x01;
    /// Press (and release) a behavior binding, as a key would.
    pub const INVOKE: u8 = 0x02;
    /// Both halves' underglow state.
    pub const RGB_STATE: u8 = 0x03;
    pub const CONN_STATE: u8 = 0x04;
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

pub fn present() -> Vec<u8> {
    vec![PANEL, panel::PRESENT]
}

pub fn release() -> Vec<u8> {
    vec![PANEL, panel::RELEASE]
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
            right_battery: (r[8] != 0xff).then_some(r[8]),
            discovery: r[9] as i8,
            discovery_tries: r[10],
            debug: r[11..21].try_into().unwrap(),
            capabilities: u32::from_le_bytes(r[21..25].try_into().unwrap()),
            panel_timeout_s: u16::from_le_bytes(r[25..27].try_into().unwrap()),
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
pub const RIGHT_STATES: [&str; 6] =
    ["built-in", "receiving", "playing pushed set", "bad CRC", "bad blob", "too big"];

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
        assert_eq!(art_data(0x0102, &[7, 8, 9]), [0xD6, 0x02, 0x02, 0x01, 3, 7, 8, 9]);
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
            RgbSync { sync: Some(true), left_on: true, right_link: 2 }
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
        r[..17].copy_from_slice(&[0xD7, 0x83, 1, 0x68, 0x01, 80, 50, 3, 2, 4, 0, 0x2C, 0x01, 100, 20, 0, 1]);
        let s = RgbState::parse(&r);
        assert_eq!(s.left, RgbHalf { on: true, hue: 360, sat: 80, brightness: 50, effect: 3, speed: 2 });
        assert_eq!(s.right, Some((RgbHalf { on: false, hue: 300, sat: 100, brightness: 20, effect: 0, speed: 1 }, 4)));
        r[9] = 0xff;
        assert_eq!(RgbState::parse(&r).right, None);
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

impl ConnState {
    pub fn parse(r: &[u8; REPORT]) -> Self {
        Self {
            active_profile: r[2], connected: r[3] != 0, bonded: r[4] != 0,
            endpoint: r[5], usb_ready: r[6] != 0, profile_count: r[7],
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
        assert_eq!(ConnState::parse(&r), ConnState {
            active_profile: 3, connected: true, bonded: false, endpoint: 1,
            usb_ready: false, profile_count: 5,
        });
        r[21..25].copy_from_slice(&0x12345610u32.to_le_bytes());
        assert_eq!(Hello::parse(&r).capabilities, 0x12345610);
        assert_ne!(Hello::parse(&r).capabilities & CAP_CONN_STATE, 0);
        r[2] = 6;
        assert_eq!(InvokeStatus::parse(&r), InvokeStatus::Refused);
        assert!(InvokeStatus::Refused.to_string().contains("refused"));
    }
}
