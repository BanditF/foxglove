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
            Some(p) => info
                .product_string()
                .unwrap_or("")
                .to_lowercase()
                .contains(&p.to_lowercase()),
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
        Self::open_matching(
            api,
            &Match {
                product: product.map(str::to_string),
                ..Match::default()
            },
        )
    }

    pub fn open_matching(api: &mut HidApi, m: &Match) -> Result<Keyboard> {
        api.refresh_devices()?;
        let api: &HidApi = api;
        let mut devices: Vec<_> = api
            .device_list()
            .filter(|info| {
                info.usage_page() == protocol::USAGE_PAGE
                    && info.usage() == protocol::USAGE
                    && m.accepts(info)
            })
            .collect();
        devices.sort_by_key(|info| prefer_usb(info.bus_type()));
        let tried = devices.len();
        let opened = devices.into_iter().filter_map(|info| {
            Some(Keyboard {
                dev: info.open_device(api).ok()?,
                bus_type: info.bus_type(),
                product: info.product_string().unwrap_or("").to_string(),
                hello: None,
            })
        });
        // A reply now and then goes missing while other reports are in flight, so ask again;
        // a keyboard that may lack the host link gets one try per interface, to connect quickly.
        let tries = if m.without_host_link { 1 } else { 3 };
        let picked = pick(opened, m.without_host_link, |kb| {
            (0..tries).find_map(|_| kb.say_hello().ok())
        });
        if let Some((mut kb, hello)) = picked {
            kb.hello = hello;
            return Ok(kb);
        }
        if tried == 0 {
            bail!("no Raw HID keyboard (usage page 0xFF60) found");
        }
        bail!(
            "found {tried} Raw HID interface(s), but none answered: flash the host-panel firmware"
        )
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

    /// The bus of the interface this keyboard was opened on.
    pub fn bus(&self) -> crate::link::Bus {
        bus_of(self.bus_type)
    }

    pub fn say_hello(&self) -> Result<Hello> {
        let r = self.request(
            &protocol::hello(),
            protocol::PANEL,
            protocol::panel::HELLO,
            Duration::from_millis(500),
        )?;
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

    fn request(
        &self,
        report: &[u8],
        proto: u8,
        cmd: u8,
        timeout: Duration,
    ) -> Result<[u8; REPORT]> {
        self.drain();
        self.send(report)?;
        let end = Instant::now() + timeout;
        let mut buf = [0u8; REPORT];
        while Instant::now() < end {
            let left = end
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as i32;
            let n = self.dev.read_timeout(&mut buf, left.max(1))?;
            if protocol::is_reply(&buf[..n], proto, cmd) {
                return Ok(buf);
            }
        }
        Err(anyhow!("no reply from the keyboard"))
    }
}

/// The interface to use, from `candidates` in preference order (USB first): the first that
/// answers HELLO, else (`without_host_link`) the first one at all, with no HELLO.
///
/// The first answer wins over the first interface: a keyboard on USB and Bluetooth at once can
/// answer on only one of them. Firmware up to 0.1 sends Raw HID replies over every transport,
/// and its USB side drops them for good once one USB transfer is lost (a PC restart or sleep
/// while a reply is in flight), so HELLO goes unanswered on USB and is answered on Bluetooth.
/// Taking the silent USB interface left the app with "no host link".
fn pick<K>(
    candidates: impl IntoIterator<Item = K>,
    without_host_link: bool,
    mut hello: impl FnMut(&K) -> Option<Hello>,
) -> Option<(K, Option<Hello>)> {
    let mut silent = None;
    for kb in candidates {
        if let Some(h) = hello(&kb) {
            return Some((kb, Some(h)));
        }
        if without_host_link && silent.is_none() {
            silent = Some(kb);
        }
    }
    silent.map(|kb| (kb, None))
}

fn bus_of(bus: BusType) -> crate::link::Bus {
    match bus {
        BusType::Usb => crate::link::Bus::Usb,
        BusType::Bluetooth => crate::link::Bus::Bluetooth,
        _ => crate::link::Bus::Unknown,
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

    /// HELLO from the 0.1 release firmware (e255d32, dev profile) on the left half: bytes 0-15
    /// as the keyboard sent them on 2026-10-26, the rest as its `hello_reply` builds them
    /// (capabilities 0x3FFF, panel timeout 0, version 0.1.0 dev).
    const HELLO_FW_0_1_0: [u8; REPORT] = [
        0xd5, 0x81, 0x01, 0x40, 0x34, 0x02, 0x6a, 0x02, 0x64, 0x04, 0x01, 0x03, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x00,
    ];

    /// An interface for [`pick`]: its bus, and whether it answers HELLO.
    #[derive(Clone, Copy)]
    struct Iface {
        bus: BusType,
        answers: bool,
    }

    fn hello_0_1(i: &Iface) -> Option<Hello> {
        i.answers.then(|| Hello::parse(&HELLO_FW_0_1_0))
    }

    fn ifaces(usb: bool, ble: bool) -> [Iface; 2] {
        [
            Iface {
                bus: BusType::Usb,
                answers: usb,
            },
            Iface {
                bus: BusType::Bluetooth,
                answers: ble,
            },
        ]
    }

    #[test]
    fn hello_from_0_1_firmware_parses_to_a_usable_panel() {
        let h = Hello::parse(&HELLO_FW_0_1_0);
        assert!(protocol::is_reply(
            &HELLO_FW_0_1_0,
            protocol::PANEL,
            protocol::panel::HELLO
        ));
        assert_eq!(h.version, protocol::VERSION);
        assert_eq!((h.width, h.height), (64, 52));
        assert_eq!(h.right_link, 2);
        assert_eq!(h.right_battery, Some(100));
        for cap in [
            protocol::capabilities::PANEL,
            protocol::capabilities::CONN_STATE,
            protocol::capabilities::BUILD_INFO,
            protocol::capabilities::SCREENS,
        ] {
            assert_ne!(h.capabilities & cap, 0, "capability {cap:#x}");
        }
        // Bits added after 0.1 are clear: nothing may depend on them to connect.
        assert_eq!(h.capabilities & protocol::capabilities::PANEL_HOLD, 0);
        assert_eq!(h.capabilities & protocol::capabilities::LEFT_BATTERY, 0);
        let fw = h.firmware.expect("0.1 reports its version");
        assert_eq!((fw.major, fw.minor, fw.patch, fw.profile), (0, 1, 0, 0));
    }

    #[test]
    fn a_silent_usb_interface_loses_to_bluetooth_that_answers() {
        // 0.1 firmware on USB and Bluetooth at once, its USB replies stuck (2026-10-26).
        for without_host_link in [true, false] {
            let (kb, hello) = pick(ifaces(false, true), without_host_link, hello_0_1)
                .expect("the Bluetooth interface answers");
            assert!(matches!(kb.bus, BusType::Bluetooth));
            assert_eq!(hello.map(|h| (h.width, h.height)), Some((64, 52)));
        }
    }

    #[test]
    fn usb_still_wins_when_it_answers_and_silence_falls_back_to_the_first() {
        let (kb, _) = pick(ifaces(true, true), true, hello_0_1).unwrap();
        assert!(matches!(kb.bus, BusType::Usb));
        // No host link anywhere (a plain ZMK board, or a stale Bluetooth interface): the
        // overlay still gets the first interface, without HELLO.
        let (kb, hello) = pick(ifaces(false, false), true, hello_0_1).unwrap();
        assert!(matches!(kb.bus, BusType::Usb));
        assert!(hello.is_none());
        assert!(pick(ifaces(false, false), false, hello_0_1).is_none());
    }

    /// The route the link reports, from fake interfaces: USB whenever it answers HELLO,
    /// Bluetooth only when USB is silent, and the label follows the picked interface.
    #[test]
    fn the_route_prefers_usb_that_answers_and_reports_the_picked_bus() {
        for (usb, ble, route) in [
            (true, true, Some("USB")),
            (true, false, Some("USB")),
            (false, true, Some("BLE")),
            (false, false, None),
        ] {
            let mut ifaces = ifaces(usb, ble);
            // Enumeration order doesn't matter: the opener sorts USB first.
            ifaces.reverse();
            ifaces.sort_by_key(|i| prefer_usb(i.bus));
            let picked = pick(ifaces, false, hello_0_1);
            assert_eq!(
                picked.map(|(kb, _)| bus_of(kb.bus).label()),
                route,
                "usb answers {usb}, ble answers {ble}"
            );
        }
    }

    #[test]
    fn usb_precedes_stale_bluetooth_interfaces() {
        assert!(!prefer_usb(BusType::Usb));
        assert!(prefer_usb(BusType::Bluetooth));
        assert!(prefer_usb(BusType::Unknown));
    }

    #[test]
    fn output_report_only_for_windows_bluetooth() {
        for bus in [
            BusType::Unknown,
            BusType::Usb,
            BusType::Bluetooth,
            BusType::I2c,
            BusType::Spi,
        ] {
            assert_eq!(
                use_output_report(bus, true),
                matches!(bus, BusType::Bluetooth)
            );
            assert!(!use_output_report(bus, false));
        }
    }
}

#[cfg(windows)]
#[path = "hid_input_windows.rs"]
mod input;
#[cfg(windows)]
pub(crate) use input::Input;
