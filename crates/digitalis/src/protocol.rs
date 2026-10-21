//! Reports to and from the keyboard (firmware: widgets/host_link.h, widgets/art_relay.h).

pub const USAGE_PAGE: u16 = 0xFF60;
pub const USAGE: u16 = 0x61;
pub const REPORT: usize = 32;

pub const PANEL: u8 = 0xD5;
pub const ART: u8 = 0xD6;
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
}

impl Hello {
    /// The HELLO reply: ver, panel w, h, x, y, right link, right battery %, discovery, tries,
    /// then the relay debug counters.
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
        assert_eq!((h.version, h.width, h.height), (1, 64, 52));
        assert_eq!((h.right_link, h.right_battery), (2, Some(93)));
        assert_eq!(h.discovery_text(), "found");
        assert_eq!(h.discovery_tries, 1);
        assert_eq!(u16::from_le_bytes([h.debug[8], h.debug[9]]), 247);

        r[8] = 0xff;
        r[9] = (-5i8) as u8;
        let h = Hello::parse(&r);
        assert_eq!(h.right_battery, None);
        assert_eq!(h.discovery_text(), "bt_gatt_discover error -5");
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
