//! Connection safety, refreshed by events and explicit requests, never a polling timer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use crate::protocols::{ConnectionSpec, ZmkTransportConfig};
use foxglove_link::protocol::{ConnState, CAP_CONN_STATE};

#[derive(Clone, Default, PartialEq)]
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
static WAKE: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

static SETTINGS_OPEN: AtomicBool = AtomicBool::new(false);

pub fn settings_visibility(open: bool) {
    if open && !SETTINGS_OPEN.swap(open, Ordering::Relaxed) {
        super::keyboard_settings::settings_opened();
        if let Some(tx) = WAKE.get() {
            let _ = tx.send(true);
        }
    }
    if !open {
        SETTINGS_OPEN.store(false, Ordering::Relaxed);
    }
}

pub fn refresh() {
    if let Some(tx) = WAKE.get() {
        let _ = tx.send(false);
    }
}

pub fn start(ctx: &egui::Context) {
    if super::repaint_trace::disabled("status") {
        return;
    }
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
                let _ = wake_link.send(false);
            });
            #[cfg(target_os = "windows")]
            let mut watcher = None;
            #[cfg(target_os = "windows")]
            let mut watched = None;
            let mut subscriptions = Vec::new();
            let mut links: Vec<Arc<foxglove_link::link::Link>> = Vec::new();
            let mut force = true;
            let mut previous_link = None;
            let mut ble_before = None;
            loop {
                while let Ok(request) = rx.try_recv() {
                    force |= request;
                }
                #[cfg(target_os = "windows")]
                {
                    let spec = super::last_device::remembered();
                    if spec != watched {
                        watcher = windows_status::watch(wake.clone()).ok();
                        watched = spec;
                    }
                    let _ = &watcher;
                }
                while let Ok(link) = links_rx.try_recv() {
                    let tx = wake.clone();
                    subscriptions.push(link.subscribe_events(move |event| {
                        if matches!(event, foxglove_link::link::Event::State(_)) {
                            let _ = tx.send(false);
                        }
                    }));
                    links.push(link);
                }
                #[cfg(target_os = "windows")]
                let watched_ble = watcher.as_ref().and_then(windows_status::Watch::connected);
                #[cfg(not(target_os = "windows"))]
                let watched_ble = None;
                // A Bluetooth keyboard coming back may bring no HID arrival (its interfaces stay
                // while it's away), so its connect is the event that retries the links (#410).
                if watched_ble == Some(true) && ble_before != Some(true) {
                    super::link::reconnect();
                }
                ble_before = watched_ble;
                let status = read(force, &mut previous_link, watched_ble);
                // Repaint only for news: an unchanged status must not wake an idle UI (#100).
                let mut current = STATUS.lock().unwrap_or_else(|e| e.into_inner());
                if *current != status {
                    *current = status;
                    ctx.request_repaint();
                }
                drop(current);
                match rx.recv() {
                    Ok(request) => force = request,
                    Err(_) => break,
                }
            }
        });
        tx
    });
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn matches_interface(path: &str, vid: u16, pid: u16) -> bool {
    let path = path.to_ascii_lowercase();
    let token = |prefix: &str, value: u16| {
        let needle = format!("{prefix}{value:04x}");
        path.match_indices(&needle).any(|(i, _)| {
            path.as_bytes()
                .get(i + needle.len())
                .is_none_or(|c| !c.is_ascii_hexdigit())
        })
    };
    token("vid_", vid) && token("pid_", pid)
}

fn needs_firmware(
    settings_opened: bool,
    link_changed: bool,
    before: Option<bool>,
    after: Option<bool>,
) -> bool {
    settings_opened || link_changed || before != after
}

fn profile_path(vid: u16, pid: u16) -> Option<std::path::PathBuf> {
    crate::settings::Settings::config_file_path()
        .map(|p| p.with_file_name(format!("bluetooth-{vid:04x}-{pid:04x}.txt")))
}

/// `watched_ble`: what the Bluetooth watcher's own events last said (#353). It wins over a fresh
/// device-property read, which can lag the event that woke us and then stays wrong, since no
/// further event comes.
fn read(
    force: bool,
    previous_link: &mut Option<(u16, u16, bool, u64)>,
    watched_ble: Option<bool>,
) -> Status {
    let Some(ConnectionSpec::Zmk {
        vid,
        pid,
        transport,
    }) = super::last_device::remembered()
    else {
        return Status::default();
    };
    #[cfg(target_os = "windows")]
    let windows_ble = watched_ble.or_else(|| windows_status::connected(vid, pid).ok());
    #[cfg(not(target_os = "windows"))]
    let windows_ble = watched_ble;
    let old = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let link = super::link::cached(vid, pid);
    let state = link.as_ref().map(|l| l.state()).unwrap_or_default();
    let key = (vid, pid, state.connected.is_some(), state.generation);
    let query = needs_firmware(
        force,
        *previous_link != Some(key),
        old.windows_ble,
        windows_ble,
    );
    *previous_link = Some(key);
    let firmware = if !query {
        old.firmware
    } else {
        link.and_then(|link| {
            let hello = link.state().connected?.hello?;
            (hello.capabilities & CAP_CONN_STATE != 0)
                .then(|| foxglove_link::settings::conn_state(link.as_ref()).ok())
                .flatten()
        })
    };
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

/// Bluetooth profiles as the left screen counts them (from 1); the firmware numbers them from 0.
fn shown(profile: u8) -> u8 {
    profile.saturating_add(1)
}

/// How to switch to a profile on the keyboard: SYSTEM (hold both middle thumbs + Tab), then
/// Q, W, E, R or T for profiles 1 to 5 (#94).
fn switch_hint(profile: Option<u8>) -> String {
    const KEYS: [&str; 5] = ["Q", "W", "E", "R", "T"];
    let key = profile
        .and_then(|p| KEYS.get(usize::from(p)))
        .copied()
        .unwrap_or("Q/W/E/R/T");
    format!("hold both middle thumbs and Tab (SYSTEM), then press {key}")
}

fn unsafe_to_unplug(usb: bool, ble: Option<bool>, active: Option<u8>, this_pc: Option<u8>) -> bool {
    usb && (ble == Some(false) || matches!((active, this_pc), (Some(a), Some(p)) if a != p))
}

/// The connection as the status strip shows it (#385).
pub struct Summary {
    pub usb: Option<bool>,
    /// The active Bluetooth profile, from 0.
    pub profile: Option<u8>,
    /// The profile this PC is paired on, from 0.
    pub this_pc: Option<u8>,
    /// Set when unplugging now leaves the keyboard unable to type: how to fix it.
    pub unplug_warning: Option<String>,
}

pub fn summary(ctx: &egui::Context) -> Summary {
    start(ctx);
    let s = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Summary {
        usb: s.usb,
        profile: s.firmware.map(|f| f.active_profile),
        this_pc: s.this_pc,
        unplug_warning: unplug_warning(&s),
    }
}

fn unplug_warning(s: &Status) -> Option<String> {
    unsafe_to_unplug(
        s.usb == Some(true),
        s.windows_ble,
        s.firmware.map(|f| f.active_profile),
        s.this_pc,
    )
    .then(|| {
        let profile = s
            .this_pc
            .map_or("for this PC".into(), |p| shown(p).to_string());
        format!(
            "Unplugging now leaves the keyboard unable to type - select Bluetooth slot {profile}: {}",
            switch_hint(s.this_pc)
        )
    })
}

/// The link to this PC in plain words, for the Keyboard page (#386): no "endpoint" or
/// "active profile", profiles counted from 1 as the left screen counts them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plain {
    /// (what, how it is), top to bottom.
    pub rows: Vec<(String, String)>,
    /// When unplugging now leaves the keyboard unable to type: how to fix it.
    pub unplug_fix: Option<String>,
}

pub fn plain(ctx: &egui::Context) -> Plain {
    start(ctx);
    let s = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    plain_words(&s, &super::parts::Parts::current())
}

/// The rows; the first names the part the PC talks to ("Left half", "Keyboard", #559).
fn plain_words(s: &Status, parts: &super::parts::Parts) -> Plain {
    let mut rows = vec![(
        parts.name(0),
        match s.usb {
            Some(true) => "on USB",
            Some(false) => "on Bluetooth",
            None => "not connected",
        }
        .to_string(),
    )];
    rows.push((
        "Bluetooth to this PC".into(),
        match s.windows_ble {
            Some(true) => "connected",
            Some(false) => "not connected",
            None => "not known yet",
        }
        .to_string(),
    ));
    if let Some(f) = s.firmware {
        let whose = match s.this_pc {
            Some(p) if p == f.active_profile => " · this PC",
            Some(_) => " · another PC",
            None => "",
        };
        let waiting = if f.connected {
            ""
        } else {
            " · waiting for its PC"
        };
        rows.push((
            "Bluetooth slot".into(),
            format!("{}{whose}{waiting}", shown(f.active_profile)),
        ));
        if let Some(p) = s.this_pc.filter(|p| *p != f.active_profile) {
            rows.push(("This PC's slot".into(), shown(p).to_string()));
        }
    }
    let unplug_fix = unsafe_to_unplug(
        s.usb == Some(true),
        s.windows_ble,
        s.firmware.map(|f| f.active_profile),
        s.this_pc,
    )
    .then(|| {
        let profile = s
            .this_pc
            .map_or("for this PC".into(), |p| shown(p).to_string());
        format!(
            "Select Bluetooth slot {profile} first: {}.",
            switch_hint(s.this_pc)
        )
    });
    Plain { rows, unplug_fix }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Default)]
struct BleEvents {
    enumerated: bool,
    devices: std::collections::HashMap<String, Option<bool>>,
}

#[cfg(any(target_os = "windows", test))]
impl BleEvents {
    fn added(&mut self, id: String, matches: bool, connected: Option<bool>) -> bool {
        if !matches {
            return false;
        }
        let previous = self.devices.insert(id, connected);
        self.enumerated && previous != Some(connected)
    }

    fn updated(&mut self, id: &str, connected: Option<bool>) -> bool {
        let Some(value) = connected else {
            return false;
        };
        let Some(previous) = self.devices.get_mut(id) else {
            return false;
        };
        let changed = *previous != Some(value);
        *previous = Some(value);
        self.enumerated && changed
    }

    fn removed(&mut self, id: &str) -> bool {
        self.devices.remove(id).is_some() && self.enumerated
    }

    /// Whether the keyboard's Bluetooth endpoint is connected, by the events seen so far: any
    /// matching endpoint connected wins; nothing known before enumeration completes.
    fn connected(&self) -> Option<bool> {
        if !self.enumerated {
            return None;
        }
        let mut known = self.devices.values().flatten().copied();
        let first = known.next()?;
        Some(first || known.any(|c| c))
    }
}

/// Foxglove (#504): the container ids of the remembered keyboard's devices, so the peripheral
/// batteries leave out the keyboard (its halves have their own lines). Enumerates the present
/// devices once: call it off the UI thread.
#[cfg(target_os = "windows")]
pub(crate) fn keyboard_containers() -> Vec<u128> {
    match super::last_device::remembered() {
        Some(crate::protocols::ConnectionSpec::Zmk { vid, pid, .. }) => {
            windows_status::containers(vid, pid)
                .into_iter()
                .map(|g| g.to_u128())
                .collect()
        }
        _ => Vec::new(),
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
    use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_GUID};
    use windows::Win32::Foundation::DEVPROPKEY;

    // DeviceWatcher includes property changes, unlike arrival/removal-only notifications.
    pub struct Watch(
        DeviceWatcher,
        std::sync::Arc<std::sync::Mutex<super::BleEvents>>,
    );
    impl Drop for Watch {
        fn drop(&mut self) {
            let _ = self.0.Stop();
        }
    }
    impl Watch {
        pub fn connected(&self) -> Option<bool> {
            self.1.lock().unwrap_or_else(|e| e.into_inner()).connected()
        }
    }

    pub fn watch(tx: Sender<bool>) -> windows::core::Result<Watch> {
        use windows::core::Interface;
        use windows::Foundation::IPropertyValue;
        let spec = super::super::last_device::remembered();
        let Some(crate::protocols::ConnectionSpec::Zmk {
            vid,
            pid,
            transport,
        }) = spec
        else {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(0x80070057u32 as i32),
                "No remembered keyboard",
            ));
        };
        let saved_id = match transport {
            crate::protocols::ZmkTransportConfig::Ble(id) => Some(id),
            _ => None,
        };
        // AEP ids contain Bluetooth addresses, while the VID/PID lives on a BTHLE devnode.
        // Their container id joins the two without matching other devices on the radio.
        let known_containers = containers(vid, pid);
        let state = std::sync::Arc::new(std::sync::Mutex::new(super::BleEvents::default()));
        let properties = windows_collections::IIterable::from(vec![
            windows::core::HSTRING::from("System.Devices.Aep.IsConnected"),
            windows::core::HSTRING::from("System.Devices.Aep.ContainerId"),
        ]);
        let watcher = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
            &windows::core::HSTRING::from(
                // Paired endpoints only: an unpaired AEP query makes Windows scan for every
                // Bluetooth LE device in range, with thread-pool callbacks for each (#100).
                r#"System.Devices.Aep.ProtocolId:="{BB7BB05E-5972-42B5-94FC-76EAA7084D49}" AND System.Devices.Aep.IsPaired:=System.StructuredQueryType.Boolean#True"#,
            ),
            &properties,
            DeviceInformationKind::AssociationEndpoint,
        )?;
        let added = tx.clone();
        let added_state = state.clone();
        watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
            move |_, info| {
                let Some(info) = info.as_ref() else {
                    return Ok(());
                };
                let id = info.Id()?.to_string();
                let props = info.Properties()?;
                let container = props
                    .Lookup(&"System.Devices.Aep.ContainerId".into())
                    .ok()
                    .and_then(|v| v.cast::<IPropertyValue>().ok())
                    .and_then(|v| v.GetGuid().ok());
                let matches = saved_id
                    .as_ref()
                    .is_some_and(|saved| saved.eq_ignore_ascii_case(&id))
                    || matches_identity(&id, vid, pid)
                    || container.is_some_and(|c| {
                        known_containers.contains(&c) || containers(vid, pid).contains(&c)
                    });
                let connected = props
                    .Lookup(&"System.Devices.Aep.IsConnected".into())
                    .ok()
                    .and_then(|v| v.cast::<IPropertyValue>().ok())
                    .and_then(|v| v.GetBoolean().ok());
                if added_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .added(id, matches, connected)
                {
                    let _ = added.send(false);
                }
                Ok(())
            },
        ))?;
        let removed = tx.clone();
        let removed_state = state.clone();
        watcher.Removed(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, info| {
                if let Some(info) = info.as_ref() {
                    if removed_state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .removed(&info.Id()?.to_string())
                    {
                        let _ = removed.send(false);
                    }
                }
                Ok(())
            }),
        )?;
        let updated = tx.clone();
        let updated_state = state.clone();
        watcher.Updated(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, info| {
                let Some(info) = info.as_ref() else {
                    return Ok(());
                };
                let connected = info
                    .Properties()?
                    .Lookup(&"System.Devices.Aep.IsConnected".into())
                    .ok()
                    .and_then(|v| v.cast::<IPropertyValue>().ok())
                    .and_then(|v| v.GetBoolean().ok());
                if updated_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .updated(&info.Id()?.to_string(), connected)
                {
                    let _ = updated.send(false);
                }
                Ok(())
            }),
        )?;
        let enumerated = state.clone();
        watcher.EnumerationCompleted(&TypedEventHandler::<
            DeviceWatcher,
            windows::core::IInspectable,
        >::new(move |_, _| {
            enumerated
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .enumerated = true;
            let _ = tx.send(false);
            Ok(())
        }))?;
        watcher.Start()?;
        Ok(Watch(watcher, state))
    }

    pub(crate) fn containers(vid: u16, pid: u16) -> Vec<GUID> {
        let mut found = Vec::new();
        unsafe {
            let Ok(set) =
                SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
            else {
                return found;
            };
            let mut index = 0;
            loop {
                let mut info = SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInfo(set, index, &mut info).is_err() {
                    break;
                }
                index += 1;
                let mut id = [0u16; 2048];
                if SetupDiGetDeviceInstanceIdW(set, &info, Some(&mut id), None).is_err() {
                    continue;
                }
                let end = id.iter().position(|&c| c == 0).unwrap_or(id.len());
                if !matches_identity(&String::from_utf16_lossy(&id[..end]), vid, pid) {
                    continue;
                }
                let key = DEVPROPKEY {
                    fmtid: GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c),
                    pid: 2,
                };
                let mut kind = Default::default();
                let mut value = [0u8; 16];
                if SetupDiGetDevicePropertyW(set, &info, &key, &mut kind, Some(&mut value), None, 0)
                    .is_ok()
                    && kind == DEVPROP_TYPE_GUID
                {
                    let container = std::ptr::read_unaligned(value.as_ptr().cast::<GUID>());
                    if container != GUID::zeroed() {
                        found.push(container);
                    }
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
        }
        found
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

    fn conn(active_profile: u8, connected: bool) -> ConnState {
        ConnState {
            active_profile,
            connected,
            bonded: true,
            endpoint: 0,
            usb_ready: true,
            profile_count: 5,
        }
    }

    #[test]
    fn the_link_in_plain_words() {
        let corne = crate::foxglove::fixtures::users_desk::board().parts();
        let none = plain_words(&Status::default(), &corne);
        assert_eq!(
            none.rows,
            vec![
                ("Left half".into(), "not connected".into()),
                ("Bluetooth to this PC".into(), "not known yet".into())
            ]
        );
        // A one-piece board (#559): the keyboard, never a half.
        let plank = crate::foxglove::fixtures::second_desk::board().parts();
        let one = plain_words(&Status::default(), &plank);
        assert_eq!(one.rows[0], ("Keyboard".into(), "not connected".into()));
        assert!(one
            .rows
            .iter()
            .all(|(a, b)| !format!("{a} {b}").contains("half")));
        assert_eq!(none.unplug_fix, None);

        let ok = plain_words(
            &Status {
                usb: Some(true),
                windows_ble: Some(true),
                firmware: Some(conn(0, true)),
                this_pc: Some(0),
            },
            &corne,
        );
        assert_eq!(ok.rows[0].1, "on USB");
        assert_eq!(ok.rows[2], ("Bluetooth slot".into(), "1 · this PC".into()));
        assert_eq!(ok.rows.len(), 3);
        assert_eq!(ok.unplug_fix, None);

        let other = plain_words(
            &Status {
                usb: Some(true),
                windows_ble: Some(false),
                firmware: Some(conn(1, false)),
                this_pc: Some(0),
            },
            &corne,
        );
        assert_eq!(other.rows[2].1, "2 · another PC · waiting for its PC");
        assert_eq!(other.rows[3], ("This PC's slot".into(), "1".into()));
        let fix = other.unplug_fix.clone().expect("unsafe to unplug");
        assert!(
            fix.starts_with("Select Bluetooth slot 1 first: hold both middle thumbs"),
            "{fix}"
        );

        // No jargon on the page (#386).
        for p in [none, ok, other] {
            for (what, how) in &p.rows {
                let text = format!("{what} {how}").to_lowercase();
                for word in ["endpoint", "active profile", "unavailable", "link:"] {
                    assert!(!text.contains(word), "{text}");
                }
            }
        }
    }

    #[test]
    fn profiles_show_from_one_with_their_switch_key() {
        assert_eq!(shown(0), 1);
        assert_eq!(
            switch_hint(Some(0)),
            "hold both middle thumbs and Tab (SYSTEM), then press Q"
        );
        assert!(switch_hint(Some(4)).ends_with("press T"));
        assert!(switch_hint(None).ends_with("press Q/W/E/R/T"));
        assert!(switch_hint(Some(9)).ends_with("press Q/W/E/R/T"));
    }
    #[test]
    fn filters_ble_events_and_enumeration() {
        let mut events = BleEvents::default();
        assert!(!events.added("mouse".into(), false, Some(true)));
        assert!(!events.added("keyboard".into(), true, Some(false)));
        assert!(!events.updated("keyboard", Some(true)));
        events.enumerated = true;
        assert!(!events.updated("mouse", Some(false)));
        assert!(!events.updated("keyboard", None));
        assert!(!events.updated("keyboard", Some(true)));
        assert!(events.updated("keyboard", Some(false)));
        assert!(!events.updated("keyboard", Some(false)));
        assert!(!events.removed("mouse"));
        assert!(events.removed("keyboard"));
        assert!(!events.removed("keyboard"));
        assert!(events.added("keyboard".into(), true, Some(true)));
        assert!(!events.added("keyboard".into(), true, Some(true)));
    }

    #[test]
    fn the_watchers_own_events_say_whether_bluetooth_is_connected() {
        let mut events = BleEvents::default();
        events.added("keyboard".into(), true, Some(true));
        assert_eq!(
            events.connected(),
            None,
            "nothing until enumeration completes"
        );
        events.enumerated = true;
        assert_eq!(events.connected(), Some(true));
        events.added("stale pairing".into(), true, Some(false));
        assert_eq!(
            events.connected(),
            Some(true),
            "one connected endpoint is enough"
        );
        events.updated("keyboard", Some(false));
        assert_eq!(events.connected(), Some(false));
        events.added("mouse".into(), false, Some(true));
        assert_eq!(events.connected(), Some(false), "other devices don't count");
        events.removed("keyboard");
        events.removed("stale pairing");
        assert_eq!(
            events.connected(),
            None,
            "no endpoint: ask the device instead"
        );
    }

    #[test]
    fn filters_interface_paths_by_both_complete_ids() {
        let path = r"\\?\HID#VID_1234&PID_5678&MI_00#instance";
        assert!(matches_interface(path, 0x1234, 0x5678));
        assert!(!matches_interface(path, 0x1235, 0x5678));
        assert!(!matches_interface(path, 0x1234, 0x5679));
        assert!(!matches_interface(
            &path.replace("PID_5678", "PID_56789"),
            0x1234,
            0x5678
        ));
        assert!(!matches_interface("", 0x1234, 0x5678));
    }

    #[test]
    fn keyboard_requests_require_settings_link_or_windows_change() {
        assert!(!needs_firmware(false, false, Some(true), Some(true)));
        assert!(!needs_firmware(false, false, None, None));
        assert!(needs_firmware(true, false, Some(true), Some(true)));
        assert!(needs_firmware(false, true, Some(true), Some(true)));
        assert!(needs_firmware(false, false, Some(true), Some(false)));
    }

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
