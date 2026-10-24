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

        #[cfg(target_os = "windows")]
        if matches!(transport, ZmkTransportConfig::Ble(_)) {
            self.serial_watch.previous_ble = Some(ConnectionSpec::Zmk {
                vid,
                pid,
                transport: transport.clone(),
            });
        }

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

#[cfg(target_os = "windows")]
type SerialScanResult = (ConnectionSpec, Option<ConnectionSpec>);

#[derive(Default)]
pub(super) struct SerialWatch {
    #[cfg(target_os = "windows")]
    notification: Option<crate::foxglove::device_watch::DeviceWatch>,
    #[cfg(target_os = "windows")]
    hid_notification: Option<crate::foxglove::device_watch::DeviceWatch>,
    #[cfg(target_os = "windows")]
    previous_ble: Option<ConnectionSpec>,
    #[cfg(target_os = "windows")]
    registration_attempted: bool,
    #[cfg(target_os = "windows")]
    was_eligible: bool,
    #[cfg(target_os = "windows")]
    scanned_attempt: Option<Instant>,
    #[cfg(target_os = "windows")]
    pending: Option<std::sync::mpsc::Receiver<SerialScanResult>>,
}

#[cfg(any(target_os = "windows", test))]
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
    pub(super) fn serial_scan_pending(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.serial_watch.pending.is_some()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    pub(super) fn watch_serial_connection(&mut self, _ctx: &egui::Context) {
        crate::foxglove::connection::start(_ctx);
        crate::foxglove::connection::settings_visibility(self.ui.settings_visible);
        #[cfg(target_os = "windows")]
        self.watch_serial_connection_windows();
        #[cfg(not(target_os = "windows"))]
        let _ = &self.serial_watch;
    }

    #[cfg(target_os = "windows")]
    fn watch_serial_connection_windows(&mut self) {
        if let Some(
            spec @ ConnectionSpec::Zmk {
                transport: ZmkTransportConfig::Ble(_),
                ..
            },
        ) = &self.session.last_spec
        {
            self.serial_watch.previous_ble = Some(spec.clone());
        }
        let eligible = matches!(
            self.session.connection,
            AppConnectionState::Reconnecting { .. }
        ) || matches!(
            (&self.session.connection, &self.session.last_spec),
            (
                AppConnectionState::Connected { .. },
                Some(ConnectionSpec::Zmk { .. })
            )
        );
        if !self.serial_watch.registration_attempted {
            self.serial_watch.registration_attempted = true;
            self.serial_watch.notification =
                crate::foxglove::device_watch::DeviceWatch::new(self.ui_wake.clone(), false);
            self.serial_watch.hid_notification =
                crate::foxglove::device_watch::DeviceWatch::new(self.ui_wake.clone(), true);
        }
        let identity = match self.session.last_spec.as_ref() {
            Some(ConnectionSpec::Zmk { vid, pid, .. }) => Some((*vid, *pid)),
            _ => None,
        };
        for watch in [
            &self.serial_watch.notification,
            &self.serial_watch.hid_notification,
        ]
        .into_iter()
        .flatten()
        {
            watch.set_identity(identity);
        }
        let hid_changed = self
            .serial_watch
            .hid_notification
            .as_ref()
            .is_some_and(crate::foxglove::device_watch::DeviceWatch::take_change);
        let changed = self
            .serial_watch
            .notification
            .as_ref()
            .is_some_and(crate::foxglove::device_watch::DeviceWatch::take_change);
        if changed || hid_changed {
            self.serial_watch.pending = None;
            crate::foxglove::link::reconnect();
            crate::foxglove::connection::refresh();
            if let AppConnectionState::Reconnecting { next_attempt_at } =
                &mut self.session.connection
            {
                *next_attempt_at = Instant::now();
            }
        }
        if let Some(rx) = &self.serial_watch.pending {
            match rx.try_recv() {
                Ok((original, replacement)) => {
                    self.serial_watch.pending = None;
                    if eligible
                        && crate::foxglove::reconnect::scan_is_current(
                            self.session.last_spec.as_ref(),
                            &original,
                        )
                    {
                        if let Some(spec) = replacement {
                            crate::foxglove::link::transport_changed(&spec);
                            // Discard the old receiver so a stale attempt cannot win the race.
                            self.connect.pending_connect = None;
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
        let entering = eligible && !self.serial_watch.was_eligible;
        self.serial_watch.was_eligible = eligible;
        if !eligible {
            return;
        }
        let Some(spec @ ConnectionSpec::Zmk { .. }) = self.session.last_spec.clone() else {
            return;
        };
        if self.serial_watch.pending.is_some() {
            return;
        }
        let retry = match self.session.connection {
            AppConnectionState::Reconnecting { next_attempt_at }
                if Instant::now() >= next_attempt_at
                    && self.serial_watch.scanned_attempt != Some(next_attempt_at) =>
            {
                self.serial_watch.scanned_attempt = Some(next_attempt_at);
                true
            }
            _ => false,
        };
        if !entering && !changed && !hid_changed && !retry {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.serial_watch.pending = Some(rx);
        let wake = self.ui_wake.clone();
        let previous_ble = self.serial_watch.previous_ble.clone();
        crate::foxglove::services::spawn_named("serial-scan", move || {
            let ports = crate::protocols::zmk_rpc::scan_serial_ports();
            let replacement = serial_replacement(&spec, &ports).or_else(|| {
                let ConnectionSpec::Zmk {
                    transport: ZmkTransportConfig::Serial(port),
                    ..
                } = &spec
                else {
                    return None;
                };
                if ports.iter().any(|p| p.port_name == *port) {
                    return None;
                }
                crate::foxglove::reconnect::ble_replacement(&spec, previous_ble.as_ref(), &[])
                    .or_else(|| {
                        crate::foxglove::reconnect::ble_replacement(
                            &spec,
                            None,
                            &crate::device_discovery::discover_devices(),
                        )
                    })
            });
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
