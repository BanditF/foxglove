//! The keyboard's Raw HID interface and the host link protocol (firmware: widgets/host_link.h).

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use hidapi::{HidApi, HidDevice};

pub const USAGE_PAGE: u16 = 0xFF60;
pub const USAGE: u16 = 0x61;
pub const REPORT: usize = 32;

pub const PANEL: u8 = 0xD5;
pub const ART: u8 = 0xD6;
pub const VERSION: u8 = 1;

pub mod panel {
    pub const HELLO: u8 = 0x01;
    #[allow(dead_code)]
    pub const FILL: u8 = 0x02;
    pub const ROWS: u8 = 0x03;
    pub const PRESENT: u8 = 0x04;
    pub const RELEASE: u8 = 0x05;
}

pub mod art {
    pub const BEGIN: u8 = 0x01;
    pub const DATA: u8 = 0x02;
    pub const COMMIT: u8 = 0x03;
    pub const SELECT: u8 = 0x04;
    pub const STATUS: u8 = 0x05;
    pub const DATA_MAX: usize = 27;
}

/// What the keyboard said to HELLO.
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Copy, Debug)]
pub struct ArtStatus {
    pub state: u8,
    pub error: u8,
    pub length: u16,
    pub pushed: u16,
    pub link: u8,
    pub right_state: u8,
    #[allow(dead_code)]
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

fn name(table: &[&'static str], i: u8) -> &'static str {
    table.get(i as usize).copied().unwrap_or("?")
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

pub struct Keyboard {
    dev: HidDevice,
    pub product: String,
    pub hello: Hello,
}

impl Keyboard {
    /// Finds the first Raw HID interface that answers HELLO, optionally only products whose name
    /// contains `product`.
    pub fn open(api: &mut HidApi, product: Option<&str>) -> Result<Keyboard> {
        api.refresh_devices()?;
        let mut tried = 0;
        for info in api.device_list() {
            if info.usage_page() != USAGE_PAGE || info.usage() != USAGE {
                continue;
            }
            let name = info.product_string().unwrap_or("").to_string();
            if let Some(p) = product {
                if !name.to_lowercase().contains(&p.to_lowercase()) {
                    continue;
                }
            }
            tried += 1;
            let Ok(dev) = info.open_device(api) else { continue };
            let mut kb = Keyboard {
                dev,
                product: name,
                hello: Hello {
                    version: 0,
                    width: 0,
                    height: 0,
                    right_link: 0,
                    right_battery: None,
                    discovery: 0,
                    discovery_tries: 0,
                    debug: [0; 10],
                },
            };
            // A reply now and then goes missing while other reports are in flight; ask again.
            for _ in 0..3 {
                if let Ok(hello) = kb.say_hello() {
                    kb.hello = hello;
                    return Ok(kb);
                }
            }
        }
        if tried == 0 {
            bail!("no Raw HID keyboard (usage page 0xFF60) found");
        }
        bail!("found {tried} Raw HID interface(s), but none answered: flash the host-panel firmware")
    }

    pub fn send(&self, data: &[u8]) -> Result<()> {
        let mut buf = [0u8; REPORT + 1]; // byte 0 is the report ID
        buf[1..1 + data.len().min(REPORT)].copy_from_slice(&data[..data.len().min(REPORT)]);
        self.dev.write(&buf)?;
        Ok(())
    }

    /// Waits for a reply to (proto, cmd), skipping everything else (KeyPeek's reports, say).
    pub fn reply(&self, proto: u8, cmd: u8, timeout: Duration) -> Result<[u8; REPORT]> {
        let end = Instant::now() + timeout;
        let mut buf = [0u8; REPORT];
        while Instant::now() < end {
            let left = end.saturating_duration_since(Instant::now()).as_millis() as i32;
            let n = self.dev.read_timeout(&mut buf, left.max(1))?;
            if n >= 2 && buf[0] == proto && buf[1] == 0x80 | cmd {
                return Ok(buf);
            }
        }
        Err(anyhow!("no reply from the keyboard"))
    }

    fn drain(&self) {
        let mut buf = [0u8; REPORT];
        while matches!(self.dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
    }

    pub fn say_hello(&self) -> Result<Hello> {
        self.drain();
        self.send(&[PANEL, panel::HELLO, VERSION])?;
        let r = self.reply(PANEL, panel::HELLO, Duration::from_millis(500))?;
        Ok(Hello {
            version: r[2],
            width: r[3] as usize,
            height: r[4] as usize,
            right_link: r[7],
            right_battery: (r[8] != 0xff).then_some(r[8]),
            discovery: r[9] as i8,
            discovery_tries: r[10],
            debug: r[11..21].try_into().unwrap(),
        })
    }

    /// Sends the rows that differ from `last` (all of them without one), then PRESENT.
    pub fn show(&self, frame: &[u8], last: Option<&[u8]>, stride: usize) -> Result<usize> {
        let rows = frame.len() / stride;
        let changed = |y: usize| match last {
            Some(l) => frame[y * stride..(y + 1) * stride] != l[y * stride..(y + 1) * stride],
            None => true,
        };
        let per_report = ((REPORT - 4) / stride).min(3);
        let mut sent = 0;
        let mut y = 0;
        while y < rows {
            if !changed(y) {
                y += 1;
                continue;
            }
            let n = per_report.min(rows - y);
            let mut r = vec![PANEL, panel::ROWS, y as u8, n as u8];
            r.extend_from_slice(&frame[y * stride..(y + n) * stride]);
            self.send(&r)?;
            sent += 1;
            y += n;
        }
        self.send(&[PANEL, panel::PRESENT])?;
        Ok(sent + 1)
    }

    pub fn release(&self) -> Result<()> {
        self.send(&[PANEL, panel::RELEASE])
    }

    fn art_status_from(r: &[u8; REPORT]) -> ArtStatus {
        let s = &r[2..];
        ArtStatus {
            state: s[0],
            error: s[1],
            length: u16::from_le_bytes([s[2], s[3]]),
            pushed: u16::from_le_bytes([s[4], s[5]]),
            link: s[6],
            // struct art_relay_status: version, state, received u16, capacity u16, set, set_count
            right_state: s[8],
            right_received: u16::from_le_bytes([s[9], s[10]]),
            right_capacity: u16::from_le_bytes([s[11], s[12]]),
            right_set: s[13] as i8,
            right_set_count: s[14],
        }
    }

    pub fn art_command(&self, data: &[u8]) -> Result<ArtStatus> {
        self.drain();
        self.send(data)?;
        let r = self.reply(ART, data[1], Duration::from_millis(1000))?;
        Ok(Self::art_status_from(&r))
    }

    pub fn art_status(&self) -> Result<ArtStatus> {
        self.art_command(&[ART, art::STATUS])
    }

    /// Sends an art blob to the left half and has it pushed to the right; returns once the right
    /// half confirms or the relay fails.
    pub fn push_art(&self, blob: &[u8], mut progress: impl FnMut(&ArtStatus)) -> Result<ArtStatus> {
        if blob.len() > u16::MAX as usize {
            bail!("blob is {} bytes; the most is {}", blob.len(), u16::MAX);
        }
        let crc = crc32fast::hash(blob);
        let mut begin = vec![ART, art::BEGIN];
        begin.extend_from_slice(&(blob.len() as u16).to_le_bytes());
        begin.extend_from_slice(&crc.to_le_bytes());
        let st = self.art_command(&begin)?;
        if st.state != 1 {
            bail!("the left half refused the blob: {st}");
        }
        for (i, chunk) in blob.chunks(art::DATA_MAX).enumerate() {
            let mut r = vec![ART, art::DATA];
            r.extend_from_slice(&((i * art::DATA_MAX) as u16).to_le_bytes());
            r.push(chunk.len() as u8);
            r.extend_from_slice(chunk);
            self.send(&r)?;
        }
        let mut st = self.art_command(&[ART, art::COMMIT])?;
        let end = Instant::now() + Duration::from_secs(120);
        while st.state == 2 && Instant::now() < end {
            progress(&st);
            std::thread::sleep(Duration::from_millis(250));
            st = self.art_status()?;
        }
        if st.state != 3 {
            bail!("push failed: {st}");
        }
        Ok(st)
    }

    pub fn select_art(&self, set: i8) -> Result<ArtStatus> {
        let mut st = self.art_command(&[ART, art::SELECT, set as u8])?;
        let end = Instant::now() + Duration::from_secs(10);
        while st.state == 2 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(100));
            st = self.art_status()?;
        }
        Ok(st)
    }
}
