//! Foxglove (#801): the layer overlay follows its view's keyboard (its pin, else the focused
//! keyboard; `foxglove::focus_pins`). Checked each frame against the last keyboard it named, so
//! it acts once per change, never against a keyboard the user connected by hand meanwhile; the
//! focus watcher wakes a frame on each change, so nothing polls.

use super::state::AppConnectionState;
use super::OverlayApp;
use crate::device_discovery::DeviceKind;
use crate::foxglove::focus_pins::{overlay_plan, OverlayPlan};
use crate::foxglove::keyboard_focus::{self as kf, View};
use crate::protocols::{ConnectionSpec, ZmkTransportConfig};

impl OverlayApp {
    pub(super) fn follow_focus(&mut self) {
        if self.connect.pending_connect.is_some() {
            // A connect in flight: looked at again when it finishes (its result wakes a frame).
            return;
        }
        let view = kf::for_view(View::Overlay);
        let key = view.as_ref().map(|k| k.key.clone());
        if key == self.focus_seen {
            return;
        }
        self.focus_seen = key;
        let showing = match &self.session.last_spec {
            Some(ConnectionSpec::Zmk { vid, pid, .. }) => Some((*vid, *pid)),
            _ => None,
        };
        let OverlayPlan::Switch { vid, pid } = overlay_plan(view.as_ref(), showing) else {
            return;
        };
        // The same device the reconnect loop would take: a serial port first (keymap read
        // fresh), else Bluetooth.
        let same = |d: &&crate::device_discovery::DiscoveredDevice| {
            d.kind == DeviceKind::Zmk && d.vid == vid && d.pid == pid
        };
        let devices = &self.connect.available_devices;
        let Some(i) = devices
            .iter()
            .position(|d| same(&d) && d.serial_port.is_some())
            .or_else(|| devices.iter().position(|d| same(&d)))
        else {
            eprintln!(
                "Foxglove: the overlay's keyboard is now {vid:04x}:{pid:04x}, not found for its keymap yet"
            );
            return;
        };
        let transport = match (&devices[i].serial_port, &devices[i].ble_device_id) {
            (Some(port), _) => ZmkTransportConfig::Serial(port.clone()),
            (None, Some(ble)) => ZmkTransportConfig::Ble(ble.clone()),
            (None, None) => return,
        };
        self.select_device(i);
        self.close_editor();
        self.session.reopen = None;
        self.session.last_spec = Some(ConnectionSpec::Zmk {
            vid,
            pid,
            transport,
        });
        // Drops the old keyboard's connection; the reconnect loop connects the new one.
        self.session.connection = AppConnectionState::reconnect_now();
        eprintln!(
            "Foxglove: the overlay follows its keyboard to {}",
            view.map_or_else(String::new, |k| k.name)
        );
    }
}
