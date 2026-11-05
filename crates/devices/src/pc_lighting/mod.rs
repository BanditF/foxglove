//! PC and peripheral lighting (#423): case boards, coolers and Dynamic Lighting devices that
//! aren't keyboards. Each one is found from a HID enumeration and gets a [`LightingDevice`]
//! driver when Foxglove can drive it.
//!
//! - [`catalog`]: the data-driven list of known boards (`lighting-catalog.toml`), and the
//!   user's own entries
//! - [`caps`]: what a device can do (colour, effects, brightness, screens), as chips
//! - [`zones`]: its real lighting resolution (#611): zones, dead lamps, and where that came from
//! - [`discover`]: matches a HID enumeration against the catalog and the LampArray usage;
//!   [`unknown`] lists the rest for "Add a device", and [`test_zone`] lights one zone of it
//! - [`lamp_array`]: Windows Dynamic Lighting (LampArray) devices' lamps, as zones
//! - [`omen_tracer`]: the HP OMEN case lighting board (TracerLED)
//! - [`openrgb`]: every device a running OpenRGB drives, through its SDK server (#471);
//!   opt-in, off by default
//! - [`calibrated`]: every opened driver's colours go through the device's colour calibration
//! - `hid` (feature `hid`): the hidapi enumeration and report writer
//!
//! Discovery is caller-driven: the host calls it on device-arrival events, never on a timer
//! (OpenRGB's sources: on its own DEVICE_LIST_UPDATED push).
//! Nothing streams: every write follows a call.

pub mod calibrated;
pub mod caps;
pub mod catalog;
#[cfg(feature = "hid")]
pub mod hid;
pub mod lamp_array;
pub mod omen_tracer;
pub mod openrgb;
pub mod zones;

use std::time::Duration;

use crate::calibration::Calibration;
use crate::{DeviceError, Result, Rgb};
pub use caps::{Color, LampArrayInfo, LightCaps, Screen};
pub use catalog::{
    CalibrationHint, Catalog, CatalogDevice, Conflict, DeviceKind, Protocol, ZoneRole, ZoneSpec,
};
pub use lamp_array::{Lamp, LampAccess, LampArrayDesc, LampArrayDriver, LampHandle};
pub use zones::{HealthLevel, Zone, ZoneHealth, ZoneKey, ZoneSource, Zones};

/// Windows Dynamic Lighting devices: HID usage page Lighting and Illumination, usage LampArray.
pub const LAMP_ARRAY_USAGE_PAGE: u16 = 0x0059;
pub const LAMP_ARRAY_USAGE: u16 = 0x0001;
/// The Raw HID usage page ZMK (Foxglove link), QMK and VIA keyboards use.
pub const RAW_HID_USAGE_PAGE: u16 = 0xFF60;

/// How Foxglove controls a found device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// A Foxglove driver writes to it.
    Driver(Protocol),
    /// A standard Windows Dynamic Lighting (HID LampArray) device.
    LampArray,
    /// Recognised, but Foxglove can't change its lights yet.
    NotYet,
    /// A controller a running OpenRGB drives, through its SDK server (#471, [`openrgb`]).
    OpenRgb,
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
#[derive(Clone, Debug, PartialEq)]
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
    /// What it can do: from the catalog, or from a LampArray's attributes once read.
    pub caps: LightCaps,
    /// Its catalog entry is the user's own (added by hand).
    pub local: bool,
    /// The model's default colour calibration, from the catalog.
    pub calibration: Option<Calibration>,
    /// The gains its first match starts from (linear light), from the catalog's hint for its
    /// model or class (#473); `None` starts neutral.
    pub calibration_hint: Option<[f32; 3]>,
    /// Its front in millimetres (width, height), from the catalog, for scenes' room map.
    pub size: Option<[u16; 2]>,
    /// What it is, from the catalog ("other" for a LampArray: Windows says its kind).
    pub kind: DeviceKind,
    /// Apps that may fight Foxglove over its lights, from the catalog.
    pub conflicts: Vec<Conflict>,
}

/// A HID device the catalog doesn't know and that isn't a LampArray: a candidate for "Add a
/// device".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unknown {
    pub vid: u16,
    pub pid: u16,
    pub name: String,
    /// Every usage page it has, vendor-defined ones (0xFF00 and up) first.
    pub usage_pages: Vec<u16>,
    /// The interface a zone test writes to: the first vendor-defined one, else the first.
    pub path: String,
}

/// The HID devices [`discover`] doesn't place, one per VID/PID, those with a vendor-defined
/// interface first (that's where bespoke lighting boards usually are).
pub fn unknown(catalog: &Catalog, hid: &[HidEntry]) -> Vec<Unknown> {
    let lamp_array = |vid, pid| {
        hid.iter().any(|o| {
            (o.vid, o.pid) == (vid, pid)
                && o.usage_page == LAMP_ARRAY_USAGE_PAGE
                && o.usage == LAMP_ARRAY_USAGE
        })
    };
    // Keyboards with a Raw HID interface (ZMK with Foxglove link, QMK, VIA) have their own
    // backends, and a test report must never reach them: keyboard first.
    let raw_hid_keyboard = |vid, pid| {
        hid.iter()
            .any(|o| (o.vid, o.pid) == (vid, pid) && o.usage_page == RAW_HID_USAGE_PAGE)
    };
    let mut out: Vec<Unknown> = Vec::new();
    for e in hid {
        if catalog.find(e.vid, e.pid).is_some()
            || lamp_array(e.vid, e.pid)
            || raw_hid_keyboard(e.vid, e.pid)
        {
            continue;
        }
        let product = e
            .product
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let vendor = e.usage_page >= 0xFF00;
        match out.iter_mut().find(|u| (u.vid, u.pid) == (e.vid, e.pid)) {
            Some(u) => {
                let first_vendor = vendor && u.usage_pages.iter().all(|p| *p < 0xFF00);
                if !u.usage_pages.contains(&e.usage_page) {
                    u.usage_pages.push(e.usage_page);
                }
                if first_vendor {
                    u.path = e.path.clone();
                }
                if let (true, Some(p)) = (u.name.starts_with("USB device "), product) {
                    u.name = p.to_string();
                }
            }
            None => out.push(Unknown {
                vid: e.vid,
                pid: e.pid,
                name: product.map_or_else(
                    || format!("USB device {:04x}:{:04x}", e.vid, e.pid),
                    str::to_string,
                ),
                usage_pages: vec![e.usage_page],
                path: e.path.clone(),
            }),
        }
    }
    for u in &mut out {
        u.usage_pages.sort_by_key(|p| (*p < 0xFF00, *p));
    }
    out.sort_by_key(|u| (u.usage_pages.iter().all(|p| *p < 0xFF00), u.vid, u.pid));
    out
}

/// Matches HID interfaces against the catalog, then lists LampArray devices the catalog
/// doesn't name. One entry per device: a board with several interfaces is listed once.
///
/// A keyboard with a Raw HID interface (ZMK with Foxglove link, QMK, VIA) is never listed here,
/// even when it is also a LampArray (#691, #88): its own link and the Keyboard card drive its
/// lights, and a second driver here (or a scene through Windows) would fight them. Same rule
/// as [`unknown`]: keyboard first, by VID/PID.
pub fn discover(catalog: &Catalog, hid: &[HidEntry]) -> Vec<Found> {
    let raw_hid_keyboard = |vid, pid| {
        hid.iter()
            .any(|o| (o.vid, o.pid) == (vid, pid) && o.usage_page == RAW_HID_USAGE_PAGE)
    };
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
                caps: known.caps(),
                local: known.local,
                calibration: known.calibration.clone(),
                calibration_hint: catalog.hint(known).map(|h| h.gain),
                size: known.size,
                kind: known.kind,
                conflicts: known.conflicts.clone(),
            });
        }
    }
    for entry in hid {
        let key = device_key(entry);
        if entry.usage_page != LAMP_ARRAY_USAGE_PAGE
            || entry.usage != LAMP_ARRAY_USAGE
            || found.iter().any(|f| f.key == key)
            || raw_hid_keyboard(entry.vid, entry.pid)
        {
            continue;
        }
        let name = lamp_array_name(hid, entry);
        found.push(Found {
            key,
            name,
            vid: entry.vid,
            pid: entry.pid,
            control: Control::LampArray,
            path: entry.path.clone(),
            zones: Vec::new(),
            // Per-LED until its attributes are read (lamp count, positions).
            caps: LightCaps {
                color: Color::PerLed,
                ..Default::default()
            },
            local: false,
            calibration: None,
            calibration_hint: None,
            size: None,
            kind: DeviceKind::Other,
            conflicts: Vec::new(),
        });
    }
    found
}

/// A LampArray's name: the device's own, as its other interfaces give it (a virtual LampArray,
/// such as one a vendor's driver adds for a mouse, carries the driver's name, "HID VHF Driver"),
/// else the LampArray interface's, else its VID/PID.
fn lamp_array_name(hid: &[HidEntry], entry: &HidEntry) -> String {
    let named = |e: &HidEntry| {
        e.product
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
    };
    hid.iter()
        .filter(|o| (o.vid, o.pid) == (entry.vid, entry.pid))
        .filter(|o| o.usage_page != LAMP_ARRAY_USAGE_PAGE)
        .find_map(named)
        .or_else(|| named(entry))
        .unwrap_or_else(|| format!("Lighting device {:04x}:{:04x}", entry.vid, entry.pid))
}

/// A keyboard's own LampArray (#88, #754): Windows lists it under Dynamic Lighting, and
/// [`discover`] leaves it out because the keyboard's link drives its lights. Listed so the app
/// can say so instead of showing nothing for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardLampArray {
    /// [`Found::key`]'s form: `vvvv:pppp`, plus the serial when the interface has one.
    pub key: String,
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    /// Over Bluetooth LE (HID over GATT), not USB.
    pub bluetooth: bool,
}

/// HID over GATT's service in a Windows interface path: the interface is Bluetooth LE.
const HOGP_SERVICE: &str = "00001812-0000-1000-8000-00805f9b34fb";

/// The LampArray interfaces of keyboards with a Raw HID interface (the ones [`discover`] leaves
/// to the keyboard's link), one per interface key; over USB and Bluetooth they are two.
pub fn keyboard_lamp_arrays(hid: &[HidEntry]) -> Vec<KeyboardLampArray> {
    let raw_hid_keyboard = |vid, pid| {
        hid.iter()
            .any(|o| (o.vid, o.pid) == (vid, pid) && o.usage_page == RAW_HID_USAGE_PAGE)
    };
    let mut out: Vec<KeyboardLampArray> = Vec::new();
    for entry in hid {
        let key = device_key(entry);
        if entry.usage_page != LAMP_ARRAY_USAGE_PAGE
            || entry.usage != LAMP_ARRAY_USAGE
            || !raw_hid_keyboard(entry.vid, entry.pid)
            || out.iter().any(|k| k.key == key)
        {
            continue;
        }
        out.push(KeyboardLampArray {
            key,
            name: lamp_array_name(hid, entry),
            vid: entry.vid,
            pid: entry.pid,
            bluetooth: entry.path.to_ascii_lowercase().contains(HOGP_SERVICE),
        });
    }
    out
}

fn device_key(entry: &HidEntry) -> String {
    crate::calibration::device_key(entry.vid, entry.pid, entry.serial.as_deref())
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

    /// A Dynamic Lighting device: whether the host lets Foxglove drive it now (#691). `None`
    /// for devices Foxglove always drives itself.
    fn lamp_access(&self) -> Option<LampAccess> {
        None
    }

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
        self.identify_restoring(&[], wait)
    }

    /// [`Self::identify`], putting back `fallback[i]` on a zone whose colour Foxglove doesn't
    /// know (a write-only board set by something else: the user's saved colours), and white
    /// where that's missing too. The device never ends dark: the colours go back even when a
    /// blink failed, and a failed restore is tried once more after one step's wait.
    fn identify_restoring(
        &mut self,
        fallback: &[Option<Rgb>],
        wait: &mut dyn FnMut(Duration),
    ) -> Result<()> {
        let restore = restore_colors(&self.colors(), fallback);
        let mut blinked = Ok(());
        for step in 0..IDENTIFY_STEPS {
            blinked = self.set_all(if step % 2 == 0 { WHITE } else { BLACK });
            wait(IDENTIFY_STEP);
            if blinked.is_err() {
                break;
            }
        }
        let restored = self.set_zones(&restore).or_else(|_| {
            wait(IDENTIFY_STEP);
            self.set_zones(&restore)
        });
        restored.and(blinked)
    }
}

/// What a zone goes back to after a blink: the colour Foxglove knows, else `fallback`'s, else
/// white. Never dark unless that's the colour it had.
pub fn restore_colors(known: &[Option<Rgb>], fallback: &[Option<Rgb>]) -> Vec<(usize, Rgb)> {
    known
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.or(fallback.get(i).copied().flatten()).unwrap_or(WHITE)))
        .collect()
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

/// Tests one zone of a board for the manual add: blinks it (white, dark, three times), then
/// sets it to `restore` (white when unknown, never dark), even when a blink write failed. Run
/// only on the user's click.
pub fn test_zone(
    protocol: Protocol,
    zone: u8,
    restore: Option<Rgb>,
    writer: Box<dyn HidWriter>,
    wait: &mut dyn FnMut(Duration),
) -> Result<()> {
    let spec = vec![ZoneSpec {
        id: zone,
        name: format!("zone {zone}"),
        position: None,
        role: Default::default(),
    }];
    let mut d: Box<dyn LightingDevice> = match protocol {
        Protocol::OmenTracer => Box::new(omen_tracer::OmenTracer::new(
            "zone test".into(),
            spec,
            writer,
        )?),
        Protocol::None => return Err(DeviceError::Unsupported),
    };
    let mut blink = || -> Result<()> {
        for step in 0..IDENTIFY_STEPS {
            d.set_all(if step % 2 == 0 { WHITE } else { BLACK })?;
            wait(IDENTIFY_STEP);
        }
        Ok(())
    };
    let blinked = blink();
    let back = restore.unwrap_or(WHITE);
    let restored = d.set_all(back).or_else(|_| {
        wait(IDENTIFY_STEP);
        d.set_all(back)
    });
    blinked.and(restored)
}

/// Opens the driver for a found device, writing through `writer`. Every driver comes wrapped
/// in [`calibrated::Calibrated`], so its colours pass through the device's calibration.
pub fn open(found: &Found, writer: Box<dyn HidWriter>) -> Result<Box<dyn LightingDevice>> {
    let driver: Box<dyn LightingDevice> = match found.control {
        Control::Driver(Protocol::OmenTracer) => Box::new(omen_tracer::OmenTracer::new(
            found.name.clone(),
            found.zones.clone(),
            writer,
        )?),
        _ => return Err(DeviceError::Unsupported),
    };
    Ok(Box::new(calibrated::Calibrated::new(
        driver,
        found.key.clone(),
        &found.zones,
        found.calibration.clone(),
    )))
}

/// Opens the driver for a found Dynamic Lighting device over the host's handle to it (#691),
/// wrapped in [`calibrated::Calibrated`] like every driver. Its lamps must have been read (its
/// zones are its lamps).
pub fn open_lamp_array(
    found: &Found,
    handle: Box<dyn LampHandle>,
) -> Result<Box<dyn LightingDevice>> {
    if found.control != Control::LampArray {
        return Err(DeviceError::Unsupported);
    }
    let driver = LampArrayDriver::new(found.name.clone(), &found.zones, handle)?;
    Ok(Box::new(calibrated::Calibrated::new(
        Box::new(driver),
        found.key.clone(),
        &found.zones,
        found.calibration.clone(),
    )))
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

    /// #691: a keyboard that is also a LampArray (the Corne with #88's firmware: Raw HID for
    /// Foxglove link plus usage page 0x59) is the Keyboard card's, never a second light here.
    #[test]
    fn a_keyboard_that_is_also_a_lamp_array_is_not_listed_twice() {
        let catalog = Catalog::bundled();
        let hid = vec![
            entry(0x1d50, 0x615e, 1, 6, "corne-keys"),
            entry(0x1d50, 0x615e, 0xff60, 0x61, "corne-raw"),
            entry(0x1d50, 0x615e, 0x59, 1, "corne-lamps"),
            entry(0x1234, 0x5678, 0x59, 1, "strip"),
        ];
        let found = discover(&catalog, &hid);
        let keys: Vec<_> = found.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, vec!["1234:5678"]);
        assert!(unknown(&catalog, &hid).is_empty());
    }

    /// #754: the keyboard's own LampArray, left out above, is still listed as the keyboard's,
    /// over USB and over Bluetooth, named as the keyboard names itself.
    #[test]
    fn a_keyboards_own_lamp_arrays_are_listed_as_the_keyboards() {
        let named = |e: HidEntry, name: &str| HidEntry {
            product: Some(name.into()),
            ..e
        };
        let usb = HidEntry {
            serial: Some("1E89EF085AA8A635".into()),
            ..entry(
                0x1d50,
                0x615e,
                0x59,
                1,
                r"\\?\hid#vid_1d50&pid_615e&mi_04#7&1",
            )
        };
        let ble = entry(
            0x1d50,
            0x615e,
            0x59,
            1,
            r"\\?\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev_vid&011d50_pid&615e#9&2",
        );
        let hid = vec![
            named(entry(0x1d50, 0x615e, 1, 6, "corne-keys"), "Eyelash Corne"),
            entry(0x1d50, 0x615e, 0xff60, 0x61, "corne-raw"),
            usb,
            ble,
            entry(0x1234, 0x5678, 0x59, 1, "strip"),
        ];
        let k = keyboard_lamp_arrays(&hid);
        let rows: Vec<_> = k
            .iter()
            .map(|k| (k.key.as_str(), k.name.as_str(), k.bluetooth))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("1d50:615e:1E89EF085AA8A635", "Eyelash Corne", false),
                ("1d50:615e", "Eyelash Corne", true),
            ]
        );
        assert!(
            keyboard_lamp_arrays(&hid[2..]).is_empty(),
            "no Raw HID, no keyboard"
        );
    }

    /// A virtual LampArray (a vendor driver's, "HID VHF Driver") is named as its device is.
    #[test]
    fn a_lamp_array_takes_its_devices_own_name() {
        let catalog = Catalog::bundled();
        let hid = vec![
            HidEntry {
                product: Some("HID VHF Driver".into()),
                ..entry(0x046d, 0xc332, 0x59, 1, "vhf")
            },
            HidEntry {
                product: Some("G502 Proteus Spectrum".into()),
                ..entry(0x046d, 0xc332, 1, 2, "mouse")
            },
            HidEntry {
                product: Some("HID VHF Driver".into()),
                ..entry(0x1234, 0x5678, 0x59, 1, "alone")
            },
        ];
        let names: Vec<_> = discover(&catalog, &hid)
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, vec!["G502 Proteus Spectrum", "HID VHF Driver"]);
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
    fn unknown_devices_are_listed_once_vendor_pages_first() {
        let catalog = Catalog::bundled();
        let hid = vec![
            entry(0x046d, 0xc332, 1, 2, "mouse"),
            entry(0x046d, 0xc332, 0x59, 1, "vhf"),
            entry(0x103c, 0x84fd, 1, 0, "tracer"),
            entry(0x045e, 0x0001, 1, 6, "kbd"),
            HidEntry {
                product: Some("Desk strip".into()),
                ..entry(0x1209, 0x0001, 1, 0, "strip-std")
            },
            entry(0x1209, 0x0001, 0xff00, 1, "strip-vendor"),
            entry(0x1d50, 0x615e, 1, 6, "corne-keys"),
            entry(0x1d50, 0x615e, 0xff60, 0x61, "corne-raw"),
        ];
        let u = unknown(&catalog, &hid);
        let rows: Vec<_> = u
            .iter()
            .map(|u| (u.name.as_str(), u.usage_pages.clone(), u.path.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Desk strip", vec![0xff00, 1], "strip-vendor"),
                ("USB device 045e:0001", vec![1], "kbd"),
            ],
            "the case and the LampArray mouse are placed; a Raw HID keyboard is never offered"
        );
    }

    #[test]
    fn found_devices_carry_their_caps() {
        let catalog = Catalog::bundled();
        let found = discover(
            &catalog,
            &[
                entry(0x103c, 0x84fd, 1, 0, "tracer"),
                entry(0x046d, 0xc332, 0x59, 1, "vhf"),
            ],
        );
        assert_eq!(found[0].caps.count, 5);
        assert_eq!(found[1].caps.color, Color::PerLed);
        assert_eq!(found[1].caps.count, 0, "unknown until read");
    }

    #[test]
    fn a_zone_test_blinks_one_zone_and_puts_it_back() {
        let rec = omen_tracer::tests::Recorder::default();
        test_zone(
            Protocol::OmenTracer,
            2,
            None,
            Box::new(rec.clone()),
            &mut |_| {},
        )
        .unwrap();
        let unknown = rec.sent.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            &unknown[0x0B..0x0E],
            &[255, 255, 255],
            "unknown: white, not dark"
        );
        let rec = omen_tracer::tests::Recorder::default();
        let mut waits = 0;
        test_zone(
            Protocol::OmenTracer,
            3,
            Some(Rgb { r: 1, g: 2, b: 3 }),
            Box::new(rec.clone()),
            &mut |_| waits += 1,
        )
        .unwrap();
        let sent = rec.sent.lock().unwrap().clone();
        assert_eq!(waits, IDENTIFY_STEPS);
        assert_eq!(sent.len(), IDENTIFY_STEPS + 1);
        assert!(sent.iter().all(|r| r[0x36] == 3), "only zone 3");
        assert_eq!(&sent[0][0x0E..0x11], &[255, 255, 255]);
        assert_eq!(&sent[IDENTIFY_STEPS][0x0E..0x11], &[1, 2, 3], "restored");
        let none = test_zone(Protocol::None, 1, None, Box::new(rec.clone()), &mut |_| {});
        assert_eq!(none, Err(DeviceError::Unsupported));
        let failing = omen_tracer::tests::Recorder {
            fail: true,
            ..Default::default()
        };
        assert!(test_zone(
            Protocol::OmenTracer,
            1,
            None,
            Box::new(failing),
            &mut |_| {}
        )
        .is_err());
    }

    /// A board whose writes fail for the first `fail` sends.
    struct Flaky {
        sent: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
        fail: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        fail_at: usize,
    }

    impl HidWriter for Flaky {
        fn send(&mut self, reports: &[Vec<u8>]) -> Result<()> {
            let n = self.fail.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n == self.fail_at {
                return Err(DeviceError::Disconnected);
            }
            self.sent.lock().unwrap().extend(reports.iter().cloned());
            Ok(())
        }
    }

    fn case_with(writer: Box<dyn HidWriter>) -> Box<dyn LightingDevice> {
        let found = discover(
            &Catalog::bundled(),
            &[entry(0x103c, 0x84fd, 1, 0, "tracer")],
        );
        open(&found[0], writer).unwrap()
    }

    fn last_colors(sent: &[Vec<u8>]) -> Vec<(u8, [u8; 3])> {
        sent[sent.len() - 5..]
            .iter()
            .map(|r| {
                let at = 0x08 + 3 * usize::from(r[0x36] - 1);
                (r[0x36], [r[at], r[at + 1], r[at + 2]])
            })
            .collect()
    }

    #[test]
    fn identify_with_no_prior_state_restores_the_saved_or_default_colours() {
        let rec = omen_tracer::tests::Recorder::default();
        let mut d = case_with(Box::new(rec.clone()));
        let saved = Rgb {
            r: 0xf0,
            g: 0x8a,
            b: 0x3c,
        };
        // Nothing set by Foxglove yet; the user saved zone 1 and zone 7 only.
        d.identify_restoring(&[Some(saved), None, None, None, Some(saved)], &mut |_| {})
            .unwrap();
        let sent = rec.sent.lock().unwrap().clone();
        assert_eq!(
            last_colors(&sent),
            vec![
                (1, [0xf0, 0x8a, 0x3c]),
                (2, [255, 255, 255]),
                (5, [255, 255, 255]),
                (6, [255, 255, 255]),
                (7, [0xf0, 0x8a, 0x3c]),
            ],
            "saved colours where there are some, white elsewhere: never dark"
        );
        assert_eq!(d.colors()[0], Some(saved), "and now it knows them");
    }

    #[test]
    fn a_failed_blink_still_puts_the_colours_back() {
        let sent = std::sync::Arc::default();
        let flaky = Flaky {
            sent: std::sync::Arc::clone(&sent),
            fail: std::sync::Arc::default(),
            // The second blink (dark) fails: the lights must not stay off.
            fail_at: 1,
        };
        let mut d = case_with(Box::new(flaky));
        assert!(d.identify(&mut |_| {}).is_err(), "the failure is reported");
        let sent = sent.lock().unwrap().clone();
        assert!(last_colors(&sent)
            .iter()
            .all(|(_, c)| *c == [255, 255, 255]));
        // A restore that fails once is tried again.
        let sent = std::sync::Arc::default();
        let flaky = Flaky {
            sent: std::sync::Arc::clone(&sent),
            fail: std::sync::Arc::default(),
            fail_at: IDENTIFY_STEPS,
        };
        let mut d = case_with(Box::new(flaky));
        let mut waits = 0;
        d.identify(&mut |_| waits += 1).unwrap();
        assert_eq!(
            waits,
            IDENTIFY_STEPS + 1,
            "one step's wait before the retry"
        );
        let sent = sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 5 * IDENTIFY_STEPS + 5);
        assert!(last_colors(&sent)
            .iter()
            .all(|(_, c)| *c == [255, 255, 255]));
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
