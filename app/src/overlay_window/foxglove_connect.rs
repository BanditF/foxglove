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
