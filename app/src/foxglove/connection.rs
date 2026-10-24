//! Connection safety, refreshed by events and explicit requests, never a polling timer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use crate::protocols::{ConnectionSpec, ZmkTransportConfig};
use digitalis::protocol::{ConnState, CAP_CONN_STATE};

#[derive(Clone, Default)]
struct Status {
    usb: Option<bool>,
    windows_ble: Option<bool>,
    firmware: Option<ConnState>,
    this_pc: Option<u8>,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    usb: None,
    windows_ble: None,
    firmware: None,
    this_pc: None,
});
static WAKE: OnceLock<mpsc::Sender<()>> = OnceLock::new();

static SETTINGS_OPEN: AtomicBool = AtomicBool::new(false);

pub fn settings_visibility(open: bool) {
    if open && !SETTINGS_OPEN.swap(open, Ordering::Relaxed) {
        refresh();
    }
    if !open {
        SETTINGS_OPEN.store(false, Ordering::Relaxed);
    }
}

pub fn refresh() {
    if let Some(tx) = WAKE.get() {
        let _ = tx.send(());
    }
}

pub fn start(ctx: &egui::Context) {
    WAKE.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let rx = Mutex::new(rx);
        let wake = tx.clone();
        let ctx = ctx.clone();
        let _ = super::services::spawn_supervised("connection-status", move || {
            let rx = rx.lock().unwrap_or_else(|e| e.into_inner());
            let (links_tx, links_rx) = mpsc::channel();
            let wake_link = wake.clone();
            let _links = super::link::subscribe_links(move |link| {
                let _ = links_tx.send(link);
                let _ = wake_link.send(());
            });
            #[cfg(target_os = "windows")]
            let _watcher = windows_status::watch(wake.clone()).ok();
            let mut subscriptions = Vec::new();
            let mut links: Vec<Arc<digitalis::link::Link>> = Vec::new();
            loop {
                while rx.try_recv().is_ok() {}
                while let Ok(link) = links_rx.try_recv() {
                    let tx = wake.clone();
                    subscriptions.push(link.subscribe_events(move |event| {
                        if matches!(event, digitalis::link::Event::State(_)) {
                            let _ = tx.send(());
                        }
                    }));
                    links.push(link);
                }
                let status = read();
                *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = status;
                ctx.request_repaint();
                if rx.recv().is_err() {
                    break;
                }
            }
        });
        tx
    });
}

fn profile_path(vid: u16, pid: u16) -> Option<std::path::PathBuf> {
    crate::settings::Settings::config_file_path()
        .map(|p| p.with_file_name(format!("bluetooth-{vid:04x}-{pid:04x}.txt")))
}

fn read() -> Status {
    let Some(ConnectionSpec::Zmk {
        vid,
        pid,
        transport,
    }) = super::last_device::remembered()
    else {
        return Status::default();
    };
    #[cfg(target_os = "windows")]
    let windows_ble = windows_status::connected(vid, pid).ok();
    #[cfg(not(target_os = "windows"))]
    let windows_ble = None;
    let firmware = super::link::cached(vid, pid).and_then(|link| {
        let hello = link.state().connected?.hello?;
        (hello.capabilities & CAP_CONN_STATE != 0)
            .then(|| digitalis::settings::conn_state(link.as_ref()).ok())
            .flatten()
    });
    let path = profile_path(vid, pid);
    let mut this_pc = path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse().ok());
    let learned = remember_profile(this_pc, windows_ble, firmware);
    if learned != this_pc {
        this_pc = learned;
        if let (Some(path), Some(profile)) = (path, this_pc) {
            let _ = std::fs::write(path, profile.to_string());
        }
    }
    Status {
        usb: Some(
            firmware.map_or(matches!(transport, ZmkTransportConfig::Serial(_)), |s| {
                s.usb_ready
            }),
        ),
        windows_ble,
        firmware,
        this_pc,
    }
}

// Keep an established identity: another profile can be connected to a different host.
fn remember_profile(
    saved: Option<u8>,
    windows_ble: Option<bool>,
    state: Option<ConnState>,
) -> Option<u8> {
    saved.or_else(|| {
        let state = state?;
        (windows_ble == Some(true)
            && state.connected
            && state.bonded
            && state.active_profile < state.profile_count)
            .then_some(state.active_profile)
    })
}

fn unsafe_to_unplug(usb: bool, ble: Option<bool>, active: Option<u8>, this_pc: Option<u8>) -> bool {
    usb && (ble == Some(false) || matches!((active, this_pc), (Some(a), Some(p)) if a != p))
}

pub fn row(ui: &mut egui::Ui) {
    start(ui.ctx());
    let s = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Connection");
        ui.label(match s.usb {
            Some(true) => "on USB",
            Some(false) => "on Bluetooth",
            None => "unavailable",
        });
        ui.label(match s.windows_ble {
            Some(true) => "Windows Bluetooth link: connected",
            Some(false) => "Windows Bluetooth link: not connected",
            None => "Windows Bluetooth link: unknown",
        });
        if let Some(f) = s.firmware {
            ui.label(format!(
                "active profile {} · {} · endpoint {}",
                f.active_profile,
                if f.connected {
                    "connected"
                } else {
                    "not connected"
                },
                if f.endpoint == 0 { "USB" } else { "Bluetooth" }
            ));
        }
        if let Some(p) = s.this_pc {
            ui.label(format!("this PC: profile {p}"));
        }
    });
    if unsafe_to_unplug(
        s.usb == Some(true),
        s.windows_ble,
        s.firmware.map(|f| f.active_profile),
        s.this_pc,
    ) {
        let profile = s.this_pc.map_or("for this PC".into(), |p| p.to_string());
        ui.colored_label(ui.visuals().warn_fg_color,
            format!("Unplugging now leaves the keyboard unable to type - select profile {profile} with NUMBER + S/D/F/G"));
    }
}

#[cfg(target_os = "windows")]
mod windows_status {
    use std::sync::mpsc::Sender;
    use windows::core::{GUID, PCWSTR};
    use windows::Devices::Enumeration::{
        DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher,
    };
    use windows::Foundation::TypedEventHandler;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::Properties::DEVPROP_TYPE_BOOLEAN;
    use windows::Win32::Foundation::DEVPROPKEY;

    // DeviceWatcher includes property changes, unlike arrival/removal-only notifications.
    pub struct Watch(DeviceWatcher);
    impl Drop for Watch {
        fn drop(&mut self) {
            let _ = self.0.Stop();
        }
    }

    pub fn watch(tx: Sender<()>) -> windows::core::Result<Watch> {
        let properties = windows_collections::IIterable::from(vec![windows::core::HSTRING::from(
            "System.Devices.Aep.IsConnected",
        )]);
        let watcher = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
            &windows::core::HSTRING::from(
                r#"System.Devices.Aep.ProtocolId:="{BB7BB05E-5972-42B5-94FC-76EAA7084D49}""#,
            ),
            &properties,
            DeviceInformationKind::AssociationEndpoint,
        )?;
        let added = tx.clone();
        watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
            move |_, _| {
                let _ = added.send(());
                Ok(())
            },
        ))?;
        let removed = tx.clone();
        watcher.Removed(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, _| {
                let _ = removed.send(());
                Ok(())
            }),
        )?;
        watcher.Updated(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, _| {
                let _ = tx.send(());
                Ok(())
            }),
        )?;
        watcher.Start()?;
        Ok(Watch(watcher))
    }

    pub fn connected(vid: u16, pid: u16) -> windows::core::Result<bool> {
        // Match the BTHLE device identity, not a Bluetooth radio or the USB HID interface.
        unsafe {
            let set =
                SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)?;
            let result = scan(set, vid, pid);
            let _ = SetupDiDestroyDeviceInfoList(set);
            result
        }
    }

    unsafe fn scan(set: HDEVINFO, vid: u16, pid: u16) -> windows::core::Result<bool> {
        let mut index = 0;
        let mut unknown = None;
        loop {
            let mut info = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if unsafe { SetupDiEnumDeviceInfo(set, index, &mut info) }.is_err() {
                break;
            }
            index += 1;
            let mut id = [0u16; 2048];
            if unsafe { SetupDiGetDeviceInstanceIdW(set, &info, Some(&mut id), None) }.is_err() {
                continue;
            }
            let id = String::from_utf16_lossy(
                &id[..id.iter().position(|&c| c == 0).unwrap_or(id.len())],
            );
            if !matches_identity(&id, vid, pid) {
                continue;
            }
            let key = DEVPROPKEY {
                fmtid: GUID::from_u128(0x83da6326_97a6_4088_9453_a1923f573b29),
                pid: 15,
            };
            let mut kind = Default::default();
            let mut value = [0u8];
            match unsafe {
                SetupDiGetDevicePropertyW(set, &info, &key, &mut kind, Some(&mut value), None, 0)
            } {
                Ok(()) if kind == DEVPROP_TYPE_BOOLEAN => {
                    if value[0] != 0 {
                        return Ok(true);
                    }
                }
                Err(e) => unknown = Some(e),
                _ => {}
            }
        }
        if let Some(e) = unknown {
            Err(e)
        } else {
            Ok(false)
        }
    }

    fn matches_identity(id: &str, vid: u16, pid: u16) -> bool {
        let id = id.to_ascii_lowercase();
        id.starts_with("bthledevice\\")
            && [
                format!("vid&{vid:04x}_"),
                format!("vid&01{vid:04x}_"),
                format!("vid&02{vid:04x}_"),
            ]
            .iter()
            .any(|part| id.contains(part))
            && id.contains(&format!("pid&{pid:04x}_"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn matches_only_keyboard_ble_identity() {
            let id = "BTHLEDEVICE\\{service}_Dev_VID&021234_PID&5678_REV&0001";
            assert!(matches_identity(id, 0x1234, 0x5678));
            assert!(!matches_identity(id, 0x1234, 0x5679));
            assert!(!matches_identity(
                &id.replace("BTHLEDEVICE", "HID"),
                0x1234,
                0x5678
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn learns_only_a_connected_bonded_profile_and_preserves_this_pc() {
        let state = ConnState {
            active_profile: 2,
            connected: true,
            bonded: true,
            endpoint: 0,
            usb_ready: true,
            profile_count: 4,
        };
        assert_eq!(remember_profile(None, Some(true), Some(state)), Some(2));
        assert_eq!(remember_profile(Some(1), Some(true), Some(state)), Some(1));
        assert_eq!(remember_profile(None, Some(false), Some(state)), None);
        assert_eq!(remember_profile(None, None, Some(state)), None);
        assert_eq!(remember_profile(None, Some(true), None), None);
        assert_eq!(
            remember_profile(
                None,
                Some(true),
                Some(ConnState {
                    bonded: false,
                    ..state
                })
            ),
            None
        );
        assert_eq!(
            remember_profile(
                None,
                Some(true),
                Some(ConnState {
                    connected: false,
                    ..state
                })
            ),
            None
        );
        assert_eq!(
            remember_profile(
                None,
                Some(true),
                Some(ConnState {
                    profile_count: 2,
                    ..state
                })
            ),
            None
        );
    }

    #[test]
    fn warning_requires_usb_and_a_missing_link_or_wrong_profile() {
        assert!(unsafe_to_unplug(true, Some(false), None, None));
        assert!(unsafe_to_unplug(true, Some(true), Some(2), Some(1)));
        assert!(!unsafe_to_unplug(true, Some(true), Some(1), Some(1)));
        assert!(!unsafe_to_unplug(false, Some(false), Some(2), Some(1)));
        assert!(!unsafe_to_unplug(true, None, None, None));
    }
}
