//! The hidapi transport and the device layer backend: QMK keyboards on their Raw HID interface
//! (usage page 0xFF60, usage 0x61), over USB; which ones is [`crate::discovery`]'s call.
//!
//! The backend opens devices read-only unless built with [`ViaBackend::allow_writes`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use foxglove_devices::{Backend, BackendId, Device, DeviceError, DeviceInfo, EventSink, Result};
use hidapi::{HidApi, HidDevice};

use crate::catalog::Catalog;
use crate::definition::Definition;
use crate::device::ViaDevice;
use crate::discovery::{Interface, Rules};
pub use crate::discovery::{USAGE, USAGE_PAGE};
use crate::protocol::{Guarded, Report, Transport, REPORT};

/// What names a keyboard without a trusted serial: where it's plugged in (#292), else the HID
/// path.
fn stable_path(hid_path: &str) -> String {
    crate::location::of(hid_path).unwrap_or_else(|| hid_path.to_string())
}

const REPLY_TIMEOUT: Duration = Duration::from_millis(500);

/// One VIA keyboard's Raw HID interface. Replies that don't echo the command (other apps'
/// traffic on the same interface) are skipped until the timeout.
pub struct HidTransport(Mutex<HidDevice>);

/// Reports left unread before a Vial read that are dropped first, at most.
const STALE_REPLIES: usize = 16;

impl HidTransport {
    /// Writes `report`, then returns the first reply `accept` takes, within the timeout.
    fn send(
        &self,
        report: &Report,
        drain: bool,
        accept: impl Fn(&Report) -> bool,
    ) -> Result<Report> {
        let dev = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if drain {
            // A late reply to an earlier read would pass for this one's data.
            let mut stale = [0u8; REPORT];
            for _ in 0..STALE_REPLIES {
                match dev.read_timeout(&mut stale, 0) {
                    Ok(n) if n > 0 => {}
                    _ => break,
                }
            }
        }
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
            if n > 0 && accept(&reply) {
                return Ok(reply);
            }
        }
    }
}

impl Transport for HidTransport {
    fn exchange(&self, report: &Report) -> Result<Report> {
        self.send(report, false, |reply| {
            reply[0] == report[0] || reply[0] == crate::protocol::UNHANDLED
        })
    }

    /// Vial's replies don't echo the command, so any reply is taken; stale ones are dropped
    /// first, and the XZ checksum catches the rest.
    fn exchange_any(&self, report: &Report) -> Result<Report> {
        self.send(report, true, |_| true)
    }
}

/// Finds VIA keyboards; backend id `via`. By default only boards with a loaded definition;
/// [`ViaBackend::generic`] adds every other QMK Raw HID board the catalog doesn't skip.
pub struct ViaBackend {
    definitions: Vec<Definition>,
    catalog: Catalog,
    generic: bool,
    writes: bool,
}

impl ViaBackend {
    pub fn new(definitions: Vec<Definition>) -> Self {
        ViaBackend {
            definitions,
            catalog: Catalog::bundled(),
            generic: false,
            writes: false,
        }
    }

    /// Uses another catalog than the bundled one.
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// Also finds QMK Raw HID boards without a loaded definition, as keyboards with lighting
    /// but no keymap. Opening one sends it VIA's protocol-version read, so a board that uses
    /// Raw HID for its own protocol sees one 0x01 report: such boards go in the catalog with
    /// `skip`.
    pub fn generic(mut self) -> Self {
        self.generic = true;
        self
    }

    /// Opens devices with keycode and lighting writes allowed. Off by default: turn on only
    /// with the user's OK for that keyboard. Has no effect without the `via-writes` feature
    /// (off in 0.2, #581).
    pub fn allow_writes(mut self) -> Self {
        self.writes = true;
        self
    }

    fn rules(&self) -> Rules<'_> {
        Rules {
            backend: self.id(),
            definitions: &self.definitions,
            catalog: &self.catalog,
            generic: self.generic,
        }
    }
}

fn interface(d: &hidapi::DeviceInfo) -> Interface {
    Interface {
        vid: d.vendor_id(),
        pid: d.product_id(),
        usage_page: d.usage_page(),
        usage: d.usage(),
        serial: d.serial_number().map(str::to_string),
        stable_path: stable_path(&d.path().to_string_lossy()),
    }
}

impl Backend for ViaBackend {
    fn id(&self) -> BackendId {
        BackendId("via".into())
    }

    fn discover(&self) -> Result<Vec<DeviceInfo>> {
        let api = HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
        // Only Raw HID interfaces get their location looked up.
        let interfaces: Vec<Interface> = api
            .device_list()
            .filter(|d| (d.usage_page(), d.usage()) == (USAGE_PAGE, USAGE))
            .map(interface)
            .collect();
        Ok(self.rules().plan(&interfaces))
    }

    fn open(&self, info: &DeviceInfo, _sink: EventSink) -> Result<Box<dyn Device>> {
        let rules = self.rules();
        let def = rules.definition(info.id.vid, info.id.pid).cloned();
        if def.is_none() && !self.generic {
            return Err(DeviceError::Unsupported);
        }
        let api = HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
        let path = api
            .device_list()
            .filter(|d| (d.usage_page(), d.usage()) == (USAGE_PAGE, USAGE))
            .find(|d| rules.is(&interface(d), info))
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
        Ok(Box::new(ViaDevice::open(
            info.clone(),
            def,
            link,
            &self.catalog,
        )?))
    }
}
