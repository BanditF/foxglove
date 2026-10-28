use std::error::Error;
use std::io::{Read, Write};
use std::time::Duration;
use zmk_studio_api::proto::zmk::{core, keymap};
use zmk_studio_api::transport::{serial::SerialTransport, BleDiscoveryMode, PlatformBleTransport};
use zmk_studio_api::{ClientError, ResolvedLayer, StudioClient};

pub struct ZmkSerialDevice {
    pub port_name: String,
    pub vid: u16,
    pub pid: u16,
    pub product: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZmkBleDevice {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZmkTransport {
    SerialPort(String),
    BleDevice(String),
}

#[derive(Debug)]
pub struct DeviceLocked;

impl std::fmt::Display for DeviceLocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ZMK Studio device is locked")
    }
}

impl Error for DeviceLocked {}

pub fn scan_serial_ports() -> Vec<ZmkSerialDevice> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };

    ports
        .into_iter()
        .filter_map(|p| {
            if let serialport::SerialPortType::UsbPort(usb) = &p.port_type {
                Some(ZmkSerialDevice {
                    port_name: p.port_name,
                    vid: usb.vid,
                    pid: usb.pid,
                    product: usb.product.clone(),
                })
            } else {
                None
            }
        })
        .collect()
}

pub fn scan_ble_devices() -> Result<Vec<ZmkBleDevice>, Box<dyn Error>> {
    if !bluetooth_scan_available() {
        return Ok(Vec::new());
    }

    let devices =
        StudioClient::<PlatformBleTransport>::list_ble_devices_with_mode(BleDiscoveryMode::Any)?;
    Ok(devices
        .into_iter()
        .map(|device| {
            let device_id = device.device_id;
            // Use local_name when available so matching against HID product strings
            // is stable across platforms (display_name may include backend-specific IDs).
            let display_name = device
                .local_name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| device_id.clone());
            ZmkBleDevice {
                device_id,
                display_name,
            }
        })
        .collect())
}

#[cfg(target_os = "windows")]
fn bluetooth_scan_available() -> bool {
    windows_bluetooth_radio_is_on().unwrap_or(true)
}

#[cfg(not(target_os = "windows"))]
fn bluetooth_scan_available() -> bool {
    true
}

#[cfg(target_os = "windows")]
fn windows_bluetooth_radio_is_on() -> windows::core::Result<bool> {
    use windows::Devices::Bluetooth::BluetoothAdapter;
    use windows::Devices::Radios::RadioState;

    let Some(adapter) = BluetoothAdapter::GetDefaultAsync()?.join().ok() else {
        return Ok(false);
    };
    let radio = adapter.GetRadioAsync()?.join()?;
    Ok(radio.State()? == RadioState::On)
}

use std::collections::{HashMap, HashSet};
use zmk_studio_api::{BehaviorBindingParametersSet, BehaviorRole};

pub struct ZmkData {
    pub physical_layouts: keymap::PhysicalLayouts,
    pub resolved_layers: Vec<ResolvedLayer>,
    pub supported_behaviors: HashSet<BehaviorRole>,
    pub behavior_metadata: HashMap<BehaviorRole, Vec<BehaviorBindingParametersSet>>,
    /// Foxglove: the keymap's raw bindings (behavior ids and parameters), to invoke them.
    pub raw_keymap: keymap::Keymap,
}

/// Foxglove (#578): opens Studio links on `transport` for the edit session, locked or not (the
/// session handles the lock: `crate::foxglove::studio_session`).
pub fn studio_opener(transport: ZmkTransport) -> crate::foxglove::studio_session::Opener {
    Box::new(move || open_studio_link(&transport))
}

fn open_studio_link(
    transport: &ZmkTransport,
) -> Result<crate::foxglove::studio_session::Link, Box<dyn Error>> {
    use crate::foxglove::studio_session::Link;
    match transport {
        ZmkTransport::SerialPort(port_name) => {
            let line = SerialTransport::open(port_name).map_err(|e| {
                format!(
                    "Failed to open serial port '{port_name}': {e}. \
                     The port may be in use by another application such as ZMK Studio."
                )
            })?;
            // As `StudioClient::open_serial` does.
            let mut link = Link::new(line);
            link.client().drain_stale_input(Duration::from_secs(1));
            Ok(link)
        }
        ZmkTransport::BleDevice(device_id) => {
            let line = PlatformBleTransport::connect_device(device_id).map_err(|e| {
                format!(
                    "Failed to connect to BLE device '{device_id}': {e}. \
                     Make sure the keyboard is paired in your OS Bluetooth settings."
                )
            })?;
            // As `StudioClient::open_ble` does.
            let mut link = Link::new(line);
            link.client().set_call_timeout(Duration::from_secs(8));
            Ok(link)
        }
    }
}

/// Reads the keymap over `transport`. Foxglove: over USB the keyboard's answers are recorded
/// and returned too, for the saved keymap (`crate::foxglove::keymap_cache`).
pub fn fetch_zmk_data(
    transport: &ZmkTransport,
) -> Result<(ZmkData, Option<crate::foxglove::keymap_cache::Recording>), Box<dyn Error>> {
    match transport {
        ZmkTransport::SerialPort(port_name) => {
            let (client, recording) = crate::foxglove::keymap_cache::open_serial(port_name)
                .map_err(|e| {
                    format!(
                        "Failed to open serial port '{port_name}': {e}. \
                         The port may be in use by another application such as ZMK Studio."
                    )
                })?;
            fetch_zmk_data_from_client(client)
                .map(|data| (data, Some(recording)))
                .map_err(|e| add_timeout_hint(e, "USB"))
        }
        ZmkTransport::BleDevice(device_id) => {
            let client =
                StudioClient::<PlatformBleTransport>::open_ble(device_id).map_err(|e| {
                    format!(
                        "Failed to connect to BLE device '{device_id}': {e}. \
                     Make sure the keyboard is paired in your OS Bluetooth settings."
                    )
                })?;
            fetch_zmk_data_from_client(client)
                .map(|data| (data, None))
                .map_err(|e| add_timeout_hint(e, "Bluetooth"))
        }
    }
}

fn add_timeout_hint(error: Box<dyn Error>, transport_name: &str) -> Box<dyn Error> {
    if matches!(
        error.downcast_ref::<ClientError>(),
        Some(ClientError::Timeout { .. })
    ) {
        return format!(
            "The keyboard did not respond over {transport_name}. \
             ZMK disables this interface while the keyboard sends its keystrokes \
             elsewhere. Switch the keyboard's output to {transport_name} \
             (the \u{2018}&out\u{2019} key) and try again."
        )
        .into();
    }

    error
}

pub(crate) fn fetch_zmk_data_from_client<T: Read + Write>(
    mut client: StudioClient<T>,
) -> Result<ZmkData, Box<dyn Error>> {
    let lock_state = client.get_lock_state()?;
    if lock_state == core::LockState::ZmkStudioCoreLockStateLocked {
        drop(client);
        return Err(Box::new(DeviceLocked));
    }

    let physical_layouts = client.get_physical_layouts()?;

    let resolved_layers = client.resolve_keymap()?;
    // Foxglove: the same keymap with raw behavior ids (the same request, so a saved keymap
    // can answer it too).
    let raw_keymap = client.get_keymap()?;

    let supported_behaviors = client.supported_roles().unwrap_or_default();
    let behavior_metadata = client.behavior_metadata().unwrap_or_default();

    // Drop the ZMK RPC connection and give transport time to settle before
    // the caller opens any other handle (e.g. HID).
    drop(client);
    std::thread::sleep(Duration::from_millis(100));

    Ok(ZmkData {
        physical_layouts,
        resolved_layers,
        supported_behaviors,
        behavior_metadata,
        raw_keymap,
    })
}
