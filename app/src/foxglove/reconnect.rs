//! Transport selection after losing a serial keyboard.

use crate::device_discovery::{DeviceKind, DiscoveredDevice};
use crate::protocols::{ConnectionSpec, ZmkTransportConfig};

pub fn ble_replacement(
    spec: &ConnectionSpec,
    previous: Option<&ConnectionSpec>,
    devices: &[DiscoveredDevice],
) -> Option<ConnectionSpec> {
    let ConnectionSpec::Zmk {
        vid,
        pid,
        transport: ZmkTransportConfig::Serial(_),
    } = spec
    else {
        return None;
    };
    if let Some(
        saved @ ConnectionSpec::Zmk {
            vid: saved_vid,
            pid: saved_pid,
            transport: ZmkTransportConfig::Ble(_),
        },
    ) = previous
    {
        if saved_vid == vid && saved_pid == pid {
            return Some(saved.clone());
        }
    }
    let id = devices.iter().find_map(|d| {
        (d.kind == DeviceKind::Zmk && d.vid == *vid && d.pid == *pid)
            .then_some(d.ble_device_id.as_ref())
            .flatten()
    })?;
    Some(ConnectionSpec::Zmk {
        vid: *vid,
        pid: *pid,
        transport: ZmkTransportConfig::Ble(id.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(vid: u16, pid: u16, transport: ZmkTransportConfig) -> ConnectionSpec {
        ConnectionSpec::Zmk {
            vid,
            pid,
            transport,
        }
    }

    fn device(vid: u16, pid: u16, kind: DeviceKind) -> DiscoveredDevice {
        DiscoveredDevice {
            base_name: "keyboard".into(),
            vid,
            pid,
            kind,
            serial_port: None,
            ble_device_id: Some("discovered".into()),
        }
    }

    #[test]
    fn previous_identity_wins_over_discovery() {
        let serial = spec(1, 2, ZmkTransportConfig::Serial("COM3".into()));
        let ble = spec(1, 2, ZmkTransportConfig::Ble("original".into()));
        assert_eq!(
            ble_replacement(&serial, Some(&ble), &[device(1, 2, DeviceKind::Zmk)]),
            Some(ble)
        );
    }

    #[test]
    fn discovery_requires_zmk_and_both_ids() {
        let serial = spec(1, 2, ZmkTransportConfig::Serial("COM3".into()));
        let wrong = spec(1, 3, ZmkTransportConfig::Ble("wrong".into()));
        let mut devices = vec![
            device(1, 3, DeviceKind::Zmk),
            device(4, 2, DeviceKind::Zmk),
            device(1, 2, DeviceKind::Qmk),
        ];
        assert_eq!(ble_replacement(&serial, Some(&wrong), &devices), None);
        devices.push(device(1, 2, DeviceKind::Zmk));
        assert_eq!(
            ble_replacement(&serial, Some(&wrong), &devices),
            Some(spec(1, 2, ZmkTransportConfig::Ble("discovered".into())))
        );
    }

    #[test]
    fn absent_ble_leaves_serial_unchanged_and_other_specs_are_ignored() {
        let serial = spec(1, 2, ZmkTransportConfig::Serial("COM3".into()));
        assert_eq!(ble_replacement(&serial, None, &[]), None);
        assert_eq!(ble_replacement(&ConnectionSpec::Mock, None, &[]), None);
        let ble = spec(1, 2, ZmkTransportConfig::Ble("original".into()));
        assert_eq!(
            ble_replacement(&ble, None, &[device(1, 2, DeviceKind::Zmk)]),
            None
        );
    }
}

/// Check the current interfaces once; arrival notifications and the reconnect loop retry later.
pub fn require_hid(vid: u16, pid: u16) -> Result<(), String> {
    let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
    let present = api.device_list().any(|d| {
        hid_matches(
            (vid, pid),
            (d.vendor_id(), d.product_id()),
            d.usage_page(),
            d.usage(),
        )
    });
    present
        .then_some(())
        .ok_or_else(|| format!("No HID interface present for {vid:04x}:{pid:04x}"))
}

fn hid_matches(expected: (u16, u16), actual: (u16, u16), page: u16, usage: u16) -> bool {
    expected == actual
        && page == digitalis::protocol::USAGE_PAGE
        && usage == digitalis::protocol::USAGE
}

#[cfg(test)]
mod hid_tests {
    use super::*;

    #[test]
    fn matches_identity_and_raw_usage_without_transport_or_path() {
        assert!(hid_matches((1, 2), (1, 2), 0xff60, 0x61));
        for (ids, page, usage) in [
            ((1, 3), 0xff60, 0x61),
            ((3, 2), 0xff60, 0x61),
            ((1, 2), 1, 0x61),
            ((1, 2), 0xff60, 1),
        ] {
            assert!(!hid_matches((1, 2), ids, page, usage));
        }
    }
}

/// An arrival scan may supersede an in-flight attempt, but never a newer device selection.
pub fn scan_is_current(current: Option<&ConnectionSpec>, scanned: &ConnectionSpec) -> bool {
    current == Some(scanned)
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn arrival_can_supersede_pending_attempt_but_stale_scan_cannot() {
        let ble = ConnectionSpec::Zmk {
            vid: 1,
            pid: 2,
            transport: ZmkTransportConfig::Ble("keyboard".into()),
        };
        let usb = ConnectionSpec::Zmk {
            vid: 1,
            pid: 2,
            transport: ZmkTransportConfig::Serial("COM3".into()),
        };
        assert!(scan_is_current(Some(&ble), &ble));
        assert!(!scan_is_current(Some(&usb), &ble));
        assert!(!scan_is_current(None, &ble));
    }
}
