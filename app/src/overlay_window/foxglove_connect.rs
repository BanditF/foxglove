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

#[cfg(target_os = "windows")]
type SerialScanResult = (ConnectionSpec, Option<ConnectionSpec>);

#[derive(Default)]
pub(super) struct SerialWatch {
    #[cfg(target_os = "windows")]
    notification: Option<device_watch::DeviceWatch>,
    #[cfg(target_os = "windows")]
    registration_attempted: bool,
    #[cfg(target_os = "windows")]
    was_eligible: bool,
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
    pub(super) fn watch_serial_connection(&mut self, _ctx: &egui::Context) {
        #[cfg(target_os = "windows")]
        self.watch_serial_connection_windows();
        #[cfg(not(target_os = "windows"))]
        let _ = &self.serial_watch;
    }

    #[cfg(target_os = "windows")]
    fn watch_serial_connection_windows(&mut self) {
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
        let entering = eligible && !self.serial_watch.was_eligible;
        self.serial_watch.was_eligible = eligible;
        if !eligible {
            return;
        }
        if !self.serial_watch.registration_attempted {
            self.serial_watch.registration_attempted = true;
            self.serial_watch.notification = device_watch::DeviceWatch::new(self.ui_wake.clone());
        }
        let Some(spec @ ConnectionSpec::Zmk { .. }) = self.session.last_spec.clone() else {
            return;
        };
        if self.serial_watch.pending.is_some() {
            return;
        }
        let arrived = self
            .serial_watch
            .notification
            .as_ref()
            .is_some_and(device_watch::DeviceWatch::take_arrival);
        if !entering && !arrived {
            return;
        }
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

#[cfg(target_os = "windows")]
mod device_watch {
    use crate::ui_wake::UiWake;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Register_Notification, CM_Unregister_Notification, CM_NOTIFY_ACTION,
        CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
        CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CR_SUCCESS, HCMNOTIFICATION,
    };

    struct Context {
        arrived: AtomicBool,
        wake: UiWake,
    }

    pub(super) struct DeviceWatch {
        handle: HCMNOTIFICATION,
        context: Box<Context>,
    }

    impl DeviceWatch {
        pub(super) fn new(wake: UiWake) -> Option<Self> {
            let context = Box::new(Context {
                arrived: AtomicBool::new(false),
                wake,
            });
            let mut filter = CM_NOTIFY_FILTER {
                cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
                FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
                ..Default::default()
            };
            filter.u.DeviceInterface.ClassGuid =
                windows::core::GUID::from_u128(0x86e0d1e0_8089_11d0_9ce4_08003e301f73);
            let mut handle = HCMNOTIFICATION::default();
            // The boxed context stays at this address until unregister has drained callbacks.
            let result = unsafe {
                CM_Register_Notification(
                    &filter,
                    Some((&*context as *const Context).cast()),
                    Some(on_notification),
                    &mut handle,
                )
            };
            if result != CR_SUCCESS {
                return None;
            }
            Some(Self { handle, context })
        }

        pub(super) fn take_arrival(&self) -> bool {
            self.context.arrived.swap(false, Ordering::Relaxed)
        }
    }

    impl Drop for DeviceWatch {
        fn drop(&mut self) {
            // Unregister waits for callbacks before the context is freed.
            unsafe { CM_Unregister_Notification(self.handle) };
        }
    }

    unsafe extern "system" fn on_notification(
        _handle: HCMNOTIFICATION,
        context: *const core::ffi::c_void,
        action: CM_NOTIFY_ACTION,
        _event: *const CM_NOTIFY_EVENT_DATA,
        _size: u32,
    ) -> u32 {
        if action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL {
            // Registered with a stable Box, kept alive until unregister returns.
            let context = unsafe { &*context.cast::<Context>() };
            context.arrived.store(true, Ordering::Relaxed);
            context.wake.request_repaint();
        }
        0
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
