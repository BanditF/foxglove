//! The hidapi transport: the keyboard's Raw HID interface (usage page 0xFF60, usage 0x61).

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use hidapi::{BusType, HidApi, HidDevice};

use crate::protocol::{self, Hello, REPORT};
use crate::Transport;

/// Which keyboard to use: a VID/PID (the one picked in the app), else the first Raw HID interface
/// that answers HELLO, optionally only products whose name contains `product`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    pub vid_pid: Option<(u16, u16)>,
    pub product: Option<String>,
    /// Also take a keyboard that doesn't answer HELLO (firmware without the host link), for
    /// apps that only listen, like the overlay. Its `hello` is then `None`.
    pub without_host_link: bool,
}

impl Match {
    pub(crate) fn accepts(&self, info: &hidapi::DeviceInfo) -> bool {
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
    bus_type: BusType,
    pub product: String,
    /// What it said to HELLO; `None` for a keyboard without the host link (see [`Match`]).
    pub hello: Option<Hello>,
}

impl Keyboard {
    /// Finds the first Raw HID interface that answers HELLO, optionally only products whose name
    /// contains `product`.
    pub fn open(api: &mut HidApi, product: Option<&str>) -> Result<Keyboard> {
        Self::open_matching(api, &Match { product: product.map(str::to_string), ..Match::default() })
    }

    pub fn open_matching(api: &mut HidApi, m: &Match) -> Result<Keyboard> {
        api.refresh_devices()?;
        let mut tried = 0;
        let mut devices: Vec<_> = api.device_list().collect();
        devices.sort_by_key(|info| prefer_usb(info.bus_type()));
        for info in devices {
            if info.usage_page() != protocol::USAGE_PAGE || info.usage() != protocol::USAGE {
                continue;
            }
            if !m.accepts(info) {
                continue;
            }
            let name = info.product_string().unwrap_or("").to_string();
            tried += 1;
            let Ok(dev) = info.open_device(api) else { continue };
            let mut kb = Keyboard { dev, bus_type: info.bus_type(), product: name, hello: None };
            // A reply now and then goes missing while other reports are in flight, so ask again;
            // a keyboard that may lack the host link gets one try, to connect quickly.
            let tries = if m.without_host_link { 1 } else { 3 };
            kb.hello = (0..tries).find_map(|_| kb.say_hello().ok());
            if kb.hello.is_some() || m.without_host_link {
                return Ok(kb);
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

    #[cfg(windows)]
    pub(crate) fn input(&self) -> Result<Input> {
        Input::open(self.dev.get_device_info()?.path())
    }

    #[cfg(not(windows))]
    pub(crate) fn read_timeout(&self, buf: &mut [u8], timeout: i32) -> Result<usize> {
        Ok(self.dev.read_timeout(buf, timeout)?)
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
        if use_output_report(self.bus_type, cfg!(windows)) {
            // Windows WriteFile can silently drop Bluetooth output reports.
            self.dev.send_output_report(&buf)?;
        } else {
            self.dev.write(&buf)?;
        }
        Ok(())
    }

    fn request(&self, report: &[u8], proto: u8, cmd: u8, timeout: Duration) -> Result<[u8; REPORT]> {
        self.drain();
        self.send(report)?;
        let end = Instant::now() + timeout;
        let mut buf = [0u8; REPORT];
        while Instant::now() < end {
            let left = end.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32;
            let n = self.dev.read_timeout(&mut buf, left.max(1))?;
            if protocol::is_reply(&buf[..n], proto, cmd) {
                return Ok(buf);
            }
        }
        Err(anyhow!("no reply from the keyboard"))
    }
}

fn prefer_usb(bus: BusType) -> bool {
    !matches!(bus, BusType::Usb)
}

fn use_output_report(bus_type: BusType, windows: bool) -> bool {
    windows && matches!(bus_type, BusType::Bluetooth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_precedes_stale_bluetooth_interfaces() {
        assert!(!prefer_usb(BusType::Usb));
        assert!(prefer_usb(BusType::Bluetooth));
        assert!(prefer_usb(BusType::Unknown));
    }

    #[test]
    fn output_report_only_for_windows_bluetooth() {
        for bus in [BusType::Unknown, BusType::Usb, BusType::Bluetooth, BusType::I2c, BusType::Spi] {
            assert_eq!(use_output_report(bus, true), matches!(bus, BusType::Bluetooth));
            assert!(!use_output_report(bus, false));
        }
    }
}

#[cfg(windows)]
#[path = "hid_input_windows.rs"]
mod input;
#[cfg(windows)]
pub(crate) use input::Input;
