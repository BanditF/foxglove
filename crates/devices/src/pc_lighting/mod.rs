//! PC and peripheral lighting (#423): case boards, coolers and Dynamic Lighting devices that
//! aren't keyboards. Each one is found from a HID enumeration and gets a [`LightingDevice`]
//! driver when Foxglove can drive it.
//!
//! - [`catalog`]: the data-driven list of known boards (`lighting-catalog.toml`)
//! - [`discover`]: matches a HID enumeration against the catalog and the LampArray usage
//! - [`omen_tracer`]: the HP OMEN case lighting board (TracerLED)
//! - `hid` (feature `hid`): the hidapi enumeration and report writer
//!
//! Discovery is caller-driven: the host calls it on device-arrival events, never on a timer.
//! Nothing streams: every write follows a call.

pub mod catalog;
#[cfg(feature = "hid")]
pub mod hid;
pub mod omen_tracer;

use std::time::Duration;

use crate::{DeviceError, Result, Rgb};
pub use catalog::{Catalog, CatalogDevice, Protocol, ZoneSpec};

/// Windows Dynamic Lighting devices: HID usage page Lighting and Illumination, usage LampArray.
pub const LAMP_ARRAY_USAGE_PAGE: u16 = 0x0059;
pub const LAMP_ARRAY_USAGE: u16 = 0x0001;

/// How Foxglove controls a found device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// A Foxglove driver writes to it.
    Driver(Protocol),
    /// A standard Windows Dynamic Lighting (HID LampArray) device.
    LampArray,
    /// Recognised, but Foxglove can't change its lights yet.
    NotYet,
}

/// One interface from a HID enumeration, in plain data so discovery tests need no hardware.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HidEntry {
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub path: String,
    pub product: Option<String>,
    pub serial: Option<String>,
}

/// A lighting device found by [`discover`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Stable across restarts: `vvvv:pppp`, plus the serial when the board has one.
    pub key: String,
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub control: Control,
    /// The interface a driver writes to.
    pub path: String,
    pub zones: Vec<ZoneSpec>,
}

/// Matches HID interfaces against the catalog, then lists LampArray devices the catalog
/// doesn't name. One entry per device: a board with several interfaces is listed once.
pub fn discover(catalog: &Catalog, hid: &[HidEntry]) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    for entry in hid {
        let key = device_key(entry);
        if found.iter().any(|f| f.key == key) {
            continue;
        }
        if let Some(known) = catalog.find(entry.vid, entry.pid) {
            found.push(Found {
                key,
                name: known.name.clone(),
                vid: entry.vid,
                pid: entry.pid,
                control: match known.protocol {
                    Protocol::None => Control::NotYet,
                    p => Control::Driver(p),
                },
                path: entry.path.clone(),
                zones: known.zones.clone(),
            });
        }
    }
    for entry in hid {
        let key = device_key(entry);
        if entry.usage_page != LAMP_ARRAY_USAGE_PAGE
            || entry.usage != LAMP_ARRAY_USAGE
            || found.iter().any(|f| f.key == key)
        {
            continue;
        }
        let name = entry
            .product
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map_or_else(
                || format!("Lighting device {:04x}:{:04x}", entry.vid, entry.pid),
                str::to_string,
            );
        found.push(Found {
            key,
            name,
            vid: entry.vid,
            pid: entry.pid,
            control: Control::LampArray,
            path: entry.path.clone(),
            zones: Vec::new(),
        });
    }
    found
}

fn device_key(entry: &HidEntry) -> String {
    let base = format!("{:04x}:{:04x}", entry.vid, entry.pid);
    match entry.serial.as_deref().filter(|s| crate::trusted_serial(s)) {
        Some(serial) => format!("{base}:{}", serial.trim()),
        None => base,
    }
}

/// Sends whole output reports to one device, opened for that call only.
pub trait HidWriter: Send {
    /// Writes each report in order (byte 0 is the report id).
    fn send(&mut self, reports: &[Vec<u8>]) -> Result<()>;
}

/// A lighting device Foxglove drives: named zones, a static colour per zone, off and identify.
/// Boards that can't be read back keep the colours Foxglove last set ([`Self::colors`]).
pub trait LightingDevice: Send {
    fn name(&self) -> &str;

    /// The zones' names, in the order [`Self::set_zones`] takes them.
    fn zones(&self) -> Vec<String>;

    /// The colour of each zone as Foxglove last set it; `None` before the first write.
    fn colors(&self) -> Vec<Option<Rgb>>;

    /// Sets a static colour on some zones (`(index, colour)`, index into [`Self::zones`]).
    fn set_zones(&mut self, colors: &[(usize, Rgb)]) -> Result<()>;

    /// One colour on every zone.
    fn set_all(&mut self, color: Rgb) -> Result<()> {
        let all: Vec<_> = (0..self.zones().len()).map(|i| (i, color)).collect();
        self.set_zones(&all)
    }

    /// Every zone dark.
    fn off(&mut self) -> Result<()> {
        self.set_all(BLACK)
    }

    /// Blinks the whole device briefly so the user can see which one it is, then restores the
    /// colours it had (white where Foxglove never set one). `wait` sleeps between steps; it's a
    /// bounded sequence of [`IDENTIFY_STEPS`], not a timer.
    fn identify(&mut self, wait: &mut dyn FnMut(Duration)) -> Result<()> {
        let before = self.colors();
        for step in 0..IDENTIFY_STEPS {
            self.set_all(if step % 2 == 0 { WHITE } else { BLACK })?;
            wait(IDENTIFY_STEP);
        }
        let restore: Vec<_> = before
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.unwrap_or(WHITE)))
            .collect();
        self.set_zones(&restore)
    }
}

pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
pub const WHITE: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
/// Identify: white, dark, white, dark, white, dark, then the colours back.
pub const IDENTIFY_STEPS: usize = 6;
pub const IDENTIFY_STEP: Duration = Duration::from_millis(250);

/// Opens the driver for a found device, writing through `writer`.
pub fn open(found: &Found, writer: Box<dyn HidWriter>) -> Result<Box<dyn LightingDevice>> {
    match found.control {
        Control::Driver(Protocol::OmenTracer) => Ok(Box::new(omen_tracer::OmenTracer::new(
            found.name.clone(),
            found.zones.clone(),
            writer,
        )?)),
        _ => Err(DeviceError::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(vid: u16, pid: u16, page: u16, usage: u16, path: &str) -> HidEntry {
        HidEntry {
            vid,
            pid,
            usage_page: page,
            usage,
            path: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn finds_the_case_the_cooler_and_lamp_arrays_once_each() {
        let catalog = Catalog::bundled();
        let mouse = HidEntry {
            product: Some("G502 Proteus Spectrum".into()),
            ..entry(0x046d, 0xc332, 0x59, 1, "vhf")
        };
        let hid = vec![
            entry(0x046d, 0xc332, 1, 2, "mouse"),
            entry(0x103c, 0x84fd, 1, 0, "tracer"),
            entry(0x03f0, 0x7397, 1, 2, "pump-mouse"),
            entry(0x03f0, 0x7397, 0xff00, 1, "pump-vendor"),
            mouse,
            entry(0x1234, 0x5678, 0x59, 1, "unnamed"),
            entry(0x1234, 0x0001, 0x59, 2, "not-a-lamp-array"),
        ];
        let found = discover(&catalog, &hid);
        let rows: Vec<_> = found
            .iter()
            .map(|f| (f.name.as_str(), f.control, f.path.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "HP OMEN 45L case lights",
                    Control::Driver(Protocol::OmenTracer),
                    "tracer"
                ),
                ("HP OMEN liquid cooler", Control::NotYet, "pump-mouse"),
                ("G502 Proteus Spectrum", Control::LampArray, "vhf"),
                ("Lighting device 1234:5678", Control::LampArray, "unnamed"),
            ]
        );
        assert_eq!(found[0].key, "103c:84fd");
        assert_eq!(found[0].zones.len(), 5);
    }

    #[test]
    fn two_boards_with_their_own_serials_are_two_devices() {
        let catalog = Catalog::bundled();
        let a = HidEntry {
            serial: Some("A1B2C3D4".into()),
            ..entry(0x103c, 0x84fd, 1, 0, "a")
        };
        let b = HidEntry {
            serial: Some("Z9Y8X7W6".into()),
            ..entry(0x103c, 0x84fd, 1, 0, "b")
        };
        let found = discover(&catalog, &[a, b]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].key, "103c:84fd:A1B2C3D4");
    }

    #[test]
    fn only_driven_devices_open() {
        let catalog = Catalog::bundled();
        let found = discover(
            &catalog,
            &[
                entry(0x03f0, 0x7397, 1, 2, "pump"),
                entry(0x1234, 0x5678, 0x59, 1, "lamp"),
            ],
        );
        for f in &found {
            let writer = Box::new(omen_tracer::tests::Recorder::default());
            assert!(matches!(open(f, writer), Err(DeviceError::Unsupported)));
        }
    }
}
