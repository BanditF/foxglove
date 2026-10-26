//! The hidapi transport and the device layer backend: keyboards whose VID/PID matches a loaded VIA
//! definition, on their Raw HID interface (usage page 0xFF60, usage 0x61), over USB.
//!
//! The backend opens devices read-only unless built with [`ViaBackend::allow_writes`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use foxglove_devices::{
    Backend, BackendId, Device, DeviceError, DeviceId, DeviceInfo, DeviceKind, EventSink, Result,
};
use hidapi::{HidApi, HidDevice};

use crate::definition::Definition;
use crate::device::ViaDevice;
use crate::protocol::{Guarded, Report, Transport, REPORT};

/// What names a keyboard without a trusted serial: where it's plugged in (#292), else the HID
/// path.
fn stable_path(hid_path: &str) -> String {
    crate::location::of(hid_path).unwrap_or_else(|| hid_path.to_string())
}

pub const USAGE_PAGE: u16 = 0xFF60;
pub const USAGE: u16 = 0x61;
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);

/// One VIA keyboard's Raw HID interface. Replies that don't echo the command (other apps'
/// traffic on the same interface) are skipped until the timeout.
pub struct HidTransport(Mutex<HidDevice>);

impl Transport for HidTransport {
    fn exchange(&self, report: &Report) -> Result<Report> {
        let dev = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = [0u8; REPORT + 1]; // byte 0 is the report id
        out[1..].copy_from_slice(report);
        dev.write(&out)
            .map_err(|e| DeviceError::Backend(e.to_string()))?;
        let end = Instant::now() + REPLY_TIMEOUT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(DeviceError::Timeout);
            }
            let mut reply = [0u8; REPORT];
            let n = dev
                .read_timeout(&mut reply, left.as_millis() as i32)
                .map_err(|_| DeviceError::Disconnected)?;
            if n > 0 && (reply[0] == report[0] || reply[0] == crate::protocol::UNHANDLED) {
                return Ok(reply);
            }
        }
    }
}

/// Finds keyboards matching the loaded definitions; backend id `via`.
pub struct ViaBackend {
    definitions: Vec<Definition>,
    writes: bool,
}

impl ViaBackend {
    pub fn new(definitions: Vec<Definition>) -> Self {
        ViaBackend {
            definitions,
            writes: false,
        }
    }

    /// Opens devices with keycode and lighting writes allowed. Off by default: turn on only
    /// with the user's OK for that keyboard.
    pub fn allow_writes(mut self) -> Self {
        self.writes = true;
        self
    }

    fn definition(&self, vid: u16, pid: u16) -> Option<&Definition> {
        self.definitions
            .iter()
            .find(|d| (d.vendor_id, d.product_id) == (vid, pid))
    }
}

impl Backend for ViaBackend {
    fn id(&self) -> BackendId {
        BackendId("via".into())
    }

    fn discover(&self) -> Result<Vec<DeviceInfo>> {
        let api = HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
        let mut seen: Vec<(DeviceId, String)> = Vec::new();
        let mut names = Vec::new();
        for d in api.device_list() {
            if d.usage_page() != USAGE_PAGE || d.usage() != USAGE {
                continue;
            }
            let Some(def) = self.definition(d.vendor_id(), d.product_id()) else {
                continue;
            };
            let path = stable_path(&d.path().to_string_lossy());
            seen.push((
                DeviceId::new(
                    self.id(),
                    d.vendor_id(),
                    d.product_id(),
                    d.serial_number(),
                    &path,
                ),
                path,
            ));
            names.push(def.name.clone());
        }
        // Two identical boards with the same serial are told apart by their paths (#292).
        foxglove_devices::untrust_shared_serials(&mut seen);
        let mut found: Vec<DeviceInfo> = Vec::new();
        for ((id, _), name) in seen.into_iter().zip(names) {
            if found.iter().all(|f| f.id != id) {
                found.push(DeviceInfo {
                    id,
                    backend: self.id(),
                    name,
                    kind: DeviceKind::Keyboard,
                    firmware: None,
                });
            }
        }
        Ok(found)
    }

    fn open(&self, info: &DeviceInfo, _sink: EventSink) -> Result<Box<dyn Device>> {
        let def = self
            .definition(info.id.vid, info.id.pid)
            .cloned()
            .ok_or(DeviceError::Unsupported)?;
        let api = HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
        let path = api
            .device_list()
            .find(|d| {
                let stable = stable_path(&d.path().to_string_lossy());
                // By serial, or by where it's plugged in when its serial was shared (#292).
                d.usage_page() == USAGE_PAGE
                    && d.usage() == USAGE
                    && (DeviceId::new(
                        self.id(),
                        d.vendor_id(),
                        d.product_id(),
                        d.serial_number(),
                        &stable,
                    ) == info.id
                        || DeviceId::new(self.id(), d.vendor_id(), d.product_id(), None, &stable)
                            == info.id)
            })
            .map(|d| d.path().to_owned())
            .ok_or(DeviceError::Disconnected)?;
        let dev = api
            .open_path(&path)
            .map_err(|e| DeviceError::Backend(e.to_string()))?;
        let transport = HidTransport(Mutex::new(dev));
        let link = if self.writes {
            Guarded::with_writes(transport)
        } else {
            Guarded::read_only(transport)
        };
        Ok(Box::new(ViaDevice::open(info.clone(), def, link)?))
    }
}
