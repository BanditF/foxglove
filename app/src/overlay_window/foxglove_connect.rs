//! Foxglove: connect to the last keyboard by itself on start.

use super::state::AppConnectionState;
use super::OverlayApp;
use crate::device_discovery::DeviceKind;
use crate::protocols::{ConnectionSpec, ZmkTransportConfig};
use std::time::Instant;

impl OverlayApp {
    /// Once, before the first connect: hands the remembered keyboard to the reconnect loop
    /// (`maintain_connection`), which connects and quietly retries until it works. Over USB the
    /// keymap is read fresh; on Bluetooth only, the saved one is used.
    pub(super) fn auto_connect(&mut self) {
        if self.session.ever_connected
            || self.session.last_spec.is_some()
            || !matches!(self.session.connection, AppConnectionState::Disconnected)
        {
            return;
        }
        let Some(ConnectionSpec::Zmk {
            vid,
            pid,
            transport,
        }) = crate::foxglove::last_device::remembered()
        else {
            return;
        };

        // Prefer what's there now: a serial port (keymap read fresh), else Bluetooth.
        let same = |d: &&crate::device_discovery::DiscoveredDevice| {
            d.kind == DeviceKind::Zmk && d.vid == vid && d.pid == pid
        };
        let found = self
            .connect
            .available_devices
            .iter()
            .position(|d| same(&d) && d.serial_port.is_some());
        let found = found.or_else(|| self.connect.available_devices.iter().position(|d| same(&d)));
        let transport = match found.map(|i| &self.connect.available_devices[i]) {
            Some(d) => match (&d.serial_port, &d.ble_device_id) {
                (Some(port), _) => ZmkTransportConfig::Serial(port.clone()),
                (None, Some(ble)) => ZmkTransportConfig::Ble(ble.clone()),
                (None, None) => transport,
            },
            None => transport,
        };
        if let Some(i) = found {
            self.select_device(i);
        }

        self.session.last_spec = Some(ConnectionSpec::Zmk {
            vid,
            pid,
            transport,
        });
        self.session.connection = AppConnectionState::Reconnecting {
            next_attempt_at: Instant::now(),
        };
    }
}

const SERIAL_SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

type SerialScanResult = (ConnectionSpec, Option<ConnectionSpec>);

#[derive(Default)]
pub(super) struct SerialWatch {
    next_scan_at: Option<Instant>,
    pending: Option<std::sync::mpsc::Receiver<SerialScanResult>>,
}

fn serial_replacement(
    spec: &ConnectionSpec,
    ports: &[crate::protocols::zmk_rpc::ZmkSerialDevice],
) -> Option<ConnectionSpec> {
    let ConnectionSpec::Zmk { vid, pid, .. } = spec else {
        return None;
    };
    let port = ports.iter().find(|p| p.vid == *vid && p.pid == *pid)?;
    let replacement = ConnectionSpec::Zmk {
        vid: *vid,
        pid: *pid,
        transport: ZmkTransportConfig::Serial(port.port_name.clone()),
    };
    (replacement != *spec).then_some(replacement)
}

impl OverlayApp {
    pub(super) fn watch_serial_connection(&mut self, ctx: &egui::Context) {
        // Let an in-flight connection finish before changing its spec or dropping it.
        if self.connect.pending_connect.is_some() {
            return;
        }
        let eligible = matches!(
            self.session.connection,
            AppConnectionState::Reconnecting { .. }
        ) || matches!(
            (&self.session.connection, &self.session.last_spec),
            (
                AppConnectionState::Connected { .. },
                Some(ConnectionSpec::Zmk {
                    transport: ZmkTransportConfig::Ble(_),
                    ..
                })
            )
        );
        if let Some(rx) = &self.serial_watch.pending {
            match rx.try_recv() {
                Ok((original, replacement)) => {
                    self.serial_watch.pending = None;
                    if eligible && self.session.last_spec.as_ref() == Some(&original) {
                        if let Some(spec) = replacement {
                            self.close_editor();
                            self.session.last_spec = Some(spec);
                            self.session.reopen = None;
                            self.session.connection = AppConnectionState::Reconnecting {
                                next_attempt_at: Instant::now(),
                            };
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.serial_watch.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if !eligible {
            return;
        }
        let Some(spec @ ConnectionSpec::Zmk { .. }) = self.session.last_spec.clone() else {
            return;
        };
        let now = Instant::now();
        let next = self.serial_watch.next_scan_at.unwrap_or(now);
        if self.serial_watch.pending.is_some() {
            return;
        }
        ctx.request_repaint_after(next.saturating_duration_since(now));
        if now < next {
            return;
        }
        self.serial_watch.next_scan_at = Some(now + SERIAL_SCAN_INTERVAL);
        ctx.request_repaint_after(SERIAL_SCAN_INTERVAL);
        let (tx, rx) = std::sync::mpsc::channel();
        self.serial_watch.pending = Some(rx);
        let wake = self.ui_wake.clone();
        std::thread::spawn(move || {
            let replacement =
                serial_replacement(&spec, &crate::protocols::zmk_rpc::scan_serial_ports());
            let _ = tx.send((spec, replacement));
            wake.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocols::zmk_rpc::ZmkSerialDevice;

    fn port(vid: u16, pid: u16, name: &str) -> ZmkSerialDevice {
        ZmkSerialDevice {
            vid,
            pid,
            port_name: name.into(),
            product: None,
        }
    }

    #[test]
    fn serial_replacement_requires_both_ids() {
        let spec = ConnectionSpec::Zmk {
            vid: 1,
            pid: 2,
            transport: ZmkTransportConfig::Ble("keyboard".into()),
        };
        assert_eq!(serial_replacement(&spec, &[]), None);
        assert_eq!(
            serial_replacement(&spec, &[port(1, 3, "A"), port(4, 2, "B")]),
            None
        );
        assert_eq!(
            serial_replacement(&spec, &[port(4, 2, "B"), port(1, 2, "C")]),
            Some(ConnectionSpec::Zmk {
                vid: 1,
                pid: 2,
                transport: ZmkTransportConfig::Serial("C".into()),
            })
        );
    }

    #[test]
    fn serial_replacement_updates_changed_port_only() {
        let spec = ConnectionSpec::Zmk {
            vid: 1,
            pid: 2,
            transport: ZmkTransportConfig::Serial("A".into()),
        };
        assert_eq!(serial_replacement(&spec, &[port(1, 2, "A")]), None);
        assert!(serial_replacement(&spec, &[port(1, 2, "B")]).is_some());
        assert_eq!(
            serial_replacement(&ConnectionSpec::Mock, &[port(1, 2, "A")]),
            None
        );
    }
}
