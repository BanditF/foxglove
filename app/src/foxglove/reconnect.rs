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
