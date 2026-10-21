//! The hidapi transport: the keyboard's Raw HID interface (usage page 0xFF60, usage 0x61).

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use hidapi::{HidApi, HidDevice};

use crate::protocol::{self, Hello, REPORT};
use crate::Transport;

/// Which keyboard to use: a VID/PID (the one picked in the app), else the first Raw HID interface
/// that answers HELLO, optionally only products whose name contains `product`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    pub vid_pid: Option<(u16, u16)>,
    pub product: Option<String>,
}

impl Match {
    fn accepts(&self, info: &hidapi::DeviceInfo) -> bool {
        if let Some((vid, pid)) = self.vid_pid {
            if (info.vendor_id(), info.product_id()) != (vid, pid) {
                return false;
            }
        }
        match &self.product {
            Some(p) => info.product_string().unwrap_or("").to_lowercase().contains(&p.to_lowercase()),
            None => true,
        }
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
        Self::open_matching(api, &Match { vid_pid: None, product: product.map(str::to_string) })
    }

    pub fn open_matching(api: &mut HidApi, m: &Match) -> Result<Keyboard> {
        api.refresh_devices()?;
        let mut tried = 0;
        for info in api.device_list() {
            if info.usage_page() != protocol::USAGE_PAGE || info.usage() != protocol::USAGE {
                continue;
            }
            if !m.accepts(info) {
                continue;
            }
            let name = info.product_string().unwrap_or("").to_string();
            tried += 1;
            let Ok(dev) = info.open_device(api) else { continue };
            let mut kb = Keyboard { dev, product: name, hello: Hello::default() };
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

    /// Drops input reports already waiting (other apps' traffic, stale replies).
    fn drain(&self) {
        let mut buf = [0u8; REPORT];
        while matches!(self.dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
    }

    pub(crate) fn device(&self) -> &HidDevice {
        &self.dev
    }

    pub fn say_hello(&self) -> Result<Hello> {
        let r = self.request(&protocol::hello(), protocol::PANEL, protocol::panel::HELLO, Duration::from_millis(500))?;
        Ok(Hello::parse(&r))
    }
}

impl Transport for Keyboard {
    fn send(&self, data: &[u8]) -> Result<()> {
        let mut buf = [0u8; REPORT + 1]; // byte 0 is the report ID
        let n = data.len().min(REPORT);
        buf[1..1 + n].copy_from_slice(&data[..n]);
        self.dev.write(&buf)?;
        Ok(())
    }

    fn request(&self, report: &[u8], proto: u8, cmd: u8, timeout: Duration) -> Result<[u8; REPORT]> {
        self.drain();
        self.send(report)?;
        let end = Instant::now() + timeout;
        let mut buf = [0u8; REPORT];
        while Instant::now() < end {
            let left = end.saturating_duration_since(Instant::now()).as_millis() as i32;
            let n = self.dev.read_timeout(&mut buf, left.max(1))?;
            if protocol::is_reply(&buf[..n], proto, cmd) {
                return Ok(buf);
            }
        }
        Err(anyhow!("no reply from the keyboard"))
    }
}
