//! Which HID interfaces speak HID++ and what batteries are behind them (#505's app wiring).
//!
//! - [`candidates`]: from one HID enumeration (plain data, so no device is opened), each
//!   Logitech device's HID++ collections, the long one (what Foxglove writes) with its short
//!   twin if it has one. The collections are data ([`LONG_COLLECTIONS`], [`SHORT_COLLECTION`]).
//! - [`batteries`]: over one [`Transport`], the device itself when it answers HID++ 2.0
//!   directly (a cable or Bluetooth link), else each device paired to it as a receiver (slots
//!   1-6) that is awake now. Every request is a "get" and each wait is bounded.

use std::time::Duration;

use super::{Hidpp, Result, Transport, DEVICE_DIRECT};
use crate::battery::Battery;
use crate::pc_lighting::HidEntry;

/// Logitech's USB vendor id: every HID++ device and receiver has it.
pub const VENDOR: u16 = 0x046D;
/// The long-report collections (usage page, usage): USB receivers and devices, Bluetooth.
pub const LONG_COLLECTIONS: [(u16, u16); 2] = [(0xFF00, 0x0002), (0xFF43, 0x0202)];
/// The short-report collection, where a receiver's HID++ 1.0 answers arrive on Windows.
pub const SHORT_COLLECTION: (u16, u16) = (0xFF00, 0x0001);
/// Device Name: GetDeviceNameCount (function 0), GetDeviceName (function 1, from an offset).
pub const DEVICE_NAME: u16 = 0x0005;
/// The longest a receiver slot is waited for: an absent or sleeping device's answer comes from
/// the receiver at once, so this is only for one that says nothing.
pub const SLOT_TIMEOUT: Duration = Duration::from_millis(500);
/// The longest name read (Device Name sends 16 characters a reply).
const MAX_NAME: usize = 64;

/// One HID++ interface to ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub vid: u16,
    pub pid: u16,
    /// The long-report collection.
    pub long: String,
    /// Its short-report twin, when the device has one.
    pub short: Option<String>,
    /// The HID product string (a receiver's own name; its devices name themselves).
    pub product: Option<String>,
}

/// Each HID++ interface in an enumeration, in its order.
pub fn candidates(hid: &[HidEntry]) -> Vec<Candidate> {
    let is = |e: &HidEntry, (page, usage): (u16, u16)| {
        e.vid == VENDOR && e.usage_page == page && e.usage == usage
    };
    hid.iter()
        .filter(|e| LONG_COLLECTIONS.iter().any(|&c| is(e, c)))
        .map(|long| Candidate {
            vid: long.vid,
            pid: long.pid,
            long: long.path.clone(),
            short: hid
                .iter()
                .find(|e| {
                    is(e, SHORT_COLLECTION)
                        && e.pid == long.pid
                        && interface(&e.path) == interface(&long.path)
                })
                .map(|e| e.path.clone()),
            product: long.product.clone(),
        })
        .collect()
}

/// A HID path without its collection part: Windows names each top-level collection of one
/// interface `...&col01#8&2ad9ed0a&0&0000#...`, `...&col02#8&2ad9ed0a&0&0001#...` (the
/// collection, and its number at the end of the instance id). Other paths as they are.
pub fn interface(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let mut parts: Vec<String> = lower.split('#').map(str::to_string).collect();
    let Some(at) = parts.iter().position(|p| p.contains("&col")) else {
        return lower;
    };
    let p = &parts[at];
    let col = p.find("&col").unwrap_or(p.len());
    let end = (col + 6).min(p.len());
    parts[at] = format!("{}{}", &p[..col], &p[end..]);
    if let Some(instance) = parts.get_mut(at + 1) {
        if let Some(last) = instance.rfind('&') {
            instance.truncate(last);
        }
    }
    parts.join("#")
}

/// One battery-powered device behind an interface.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// [`DEVICE_DIRECT`], or its receiver slot.
    pub index: u8,
    /// Its own name (Device Name), when it has the feature.
    pub name: Option<String>,
    pub battery: Battery,
}

/// Every battery behind one interface: the device, or a receiver's awake devices. A device
/// with no battery feature is left out; an error on one slot skips that slot only.
pub fn batteries<T: Transport + ?Sized>(transport: &mut T) -> Vec<Found> {
    let mut direct = Hidpp::new(&mut *transport, DEVICE_DIRECT).long_only();
    match direct.protocol_version() {
        Ok((major, _)) if major >= 2 => return read(direct).into_iter().collect(),
        // HID++ 1.0 at the direct index: a receiver.
        Ok(_) => {}
        Err(_) => return Vec::new(),
    }
    (1..=6u8)
        .filter_map(|slot| {
            let mut dev = Hidpp::new(&mut *transport, slot)
                .long_only()
                .with_timeout(SLOT_TIMEOUT);
            match dev.protocol_version() {
                Ok((major, _)) if major >= 2 => read(dev),
                // Nothing paired there, or it's asleep (the receiver says so at once), or no
                // answer within the slot's wait.
                _ => None,
            }
        })
        .collect()
}

fn read<T: Transport>(mut dev: Hidpp<T>) -> Option<Found> {
    let battery = dev.battery().ok().flatten()?;
    Some(Found {
        index: dev.device_index(),
        name: dev.name().ok().flatten(),
        battery,
    })
}

impl<T: Transport> Hidpp<T> {
    /// The device's own name, from Device Name (0x0005); `None` without the feature.
    pub fn name(&mut self) -> Result<Option<String>> {
        let Some(f) = self.feature(DEVICE_NAME)? else {
            return Ok(None);
        };
        let count = usize::from(self.request(f.index, 0, &[])?[0]).min(MAX_NAME);
        let mut bytes = Vec::with_capacity(count);
        while bytes.len() < count {
            let part = self.request(f.index, 1, &[bytes.len() as u8])?;
            let take = (count - bytes.len()).min(part.len());
            if take == 0 {
                break;
            }
            bytes.extend_from_slice(&part[..take]);
        }
        let name = String::from_utf8_lossy(&bytes)
            .trim_end_matches('\0')
            .trim()
            .to_string();
        Ok((!name.is_empty()).then_some(name))
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::Fake;
    use super::*;
    use crate::battery::ChargeState;

    fn entry(pid: u16, page: u16, usage: u16, path: &str) -> HidEntry {
        HidEntry {
            vid: VENDOR,
            pid,
            usage_page: page,
            usage,
            path: path.into(),
            product: Some("USB Receiver".into()),
            serial: None,
        }
    }

    #[test]
    fn candidates_are_the_long_collections_with_their_short_twins() {
        let hid = vec![
            entry(
                0xC52B,
                0xFF00,
                1,
                r"\\?\HID#VID_046D&PID_C52B&MI_02&Col01#8&1&0&0000#{x}",
            ),
            entry(
                0xC52B,
                0xFF00,
                2,
                r"\\?\HID#VID_046D&PID_C52B&MI_02&Col02#8&1&0&0001#{x}",
            ),
            entry(
                0xB023,
                0xFF43,
                0x0202,
                r"\\?\HID#{bt}_VID&0002046d_PID&b023&Col03#9#{x}",
            ),
            // A keyboard's ordinary collection, and another vendor's vendor page.
            entry(0xC52B, 0x0001, 6, "kbd"),
            HidEntry {
                vid: 0x1234,
                ..entry(1, 0xFF00, 2, "other")
            },
        ];
        let c = candidates(&hid);
        assert_eq!(c.len(), 2);
        assert!(c[0].long.contains("Col02"));
        assert!(c[0].short.as_deref().is_some_and(|s| s.contains("Col01")));
        assert_eq!(c[1].pid, 0xB023);
        assert_eq!(c[1].short, None, "Bluetooth: long reports only");
    }

    const UNIFIED: [(&str, &[&str]); 3] = [
        ("11 FF 00 0C 10 04", &["11 FF 00 0C 08 00 02"]),
        ("11 FF 08 0C", &["11 FF 08 0C 0F 03"]),
        ("11 FF 08 1C", &["11 FF 08 1C 49 04 00 00"]),
    ];

    #[test]
    fn a_direct_device_reports_its_battery_and_name() {
        let mut fake = Fake::default().expect("11 FF 00 1C 00 00 5A", &["11 FF 00 1C 04 02 5A"]);
        for (ask, replies) in UNIFIED {
            fake = fake.expect(ask, replies);
        }
        // Device Name at index 3: 6 characters, "MX Ink".
        let mut fake = fake
            .expect("11 FF 00 0C 00 05", &["11 FF 00 0C 03 00 01"])
            .expect("11 FF 03 0C", &["11 FF 03 0C 06"])
            .expect("11 FF 03 1C 00", &["11 FF 03 1C 4D 58 20 49 6E 6B"]);
        let found = batteries(&mut fake);
        assert!(fake.done());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].index, DEVICE_DIRECT);
        assert_eq!(found[0].name.as_deref(), Some("MX Ink"));
        assert_eq!(found[0].battery.percent, Some(73));
        assert_eq!(found[0].battery.charge, ChargeState::Discharging);
    }

    #[test]
    fn a_receiver_reports_each_awake_device() {
        // HID++ 1.0 at the direct index: a receiver. Slot 1 asleep, slot 2 a 0x1000 device
        // charging with no name feature, slots 3-6 empty.
        let mut fake = Fake::default()
            .expect("11 FF 00 1C 00 00 5A", &["10 FF 8F 00 1C 01 00"])
            .expect("11 01 00 1C 00 00 5A", &["10 01 8F 00 1C 09 00"])
            .expect("11 02 00 1C 00 00 5A", &["11 02 00 1C 04 02 5A"])
            .expect("11 02 00 0C 10 04", &["11 02 00 0C 00"])
            .expect("11 02 00 0C 10 00", &["11 02 00 0C 05 00 00"])
            .expect("11 02 05 0C", &["11 02 05 0C 32 00 01"])
            .expect("11 02 00 0C 00 05", &["11 02 00 0C 00"]);
        for slot in 3..=6 {
            fake = fake.expect(
                &format!("11 0{slot} 00 1C 00 00 5A"),
                &[&format!("10 0{slot} 8F 00 1C 08 00")],
            );
        }
        let found = batteries(&mut fake);
        assert!(fake.done());
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].index, found[0].name.as_deref()), (2, None));
        assert_eq!(found[0].battery.percent, Some(50));
        assert_eq!(found[0].battery.charge, ChargeState::Charging);
    }

    #[test]
    fn an_interface_that_says_nothing_has_no_battery() {
        let mut fake = Fake::default().expect("11 FF 00 1C 00 00 5A", &[]);
        assert!(batteries(&mut fake).is_empty());
        assert!(fake.done());
    }

    #[test]
    fn interface_paths_drop_the_collection() {
        let short = r"\\?\HID#VID_046D&PID_C52B&MI_02&Col01#8&2ad9ed0a&0&0000#{4d1e55b2}";
        let long = r"\\?\HID#VID_046D&PID_C52B&MI_02&Col02#8&2ad9ed0a&0&0001#{4d1e55b2}";
        let other = r"\\?\HID#VID_046D&PID_C52B&MI_02&Col02#8&3bbbbbbb&0&0001#{4d1e55b2}";
        assert_eq!(interface(short), interface(long));
        assert_ne!(interface(short), interface(other), "another receiver");
        assert_eq!(interface("plain"), "plain");
    }
}
