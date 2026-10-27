//! The lighting catalog: boards Foxglove recognises by VID/PID, their protocol, zone names and
//! what they can do. The bundled one is `crates/devices/lighting-catalog.toml`; the user's own
//! entries (added by hand on the Lights page) sit in a local file of the same format and win
//! over the bundled ones. Adding a board that speaks a known protocol is a data change.

use serde::Deserialize;

use super::caps::{Color, LightCaps, Screen};
use crate::calibration::Calibration;

/// The driver a catalog board needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    /// HP OMEN case lighting board (TracerLED): 59-byte static colour reports per zone.
    OmenTracer,
    /// Recognised, no driver yet.
    None,
}

impl Protocol {
    /// Every protocol, for the manual add's choice.
    pub const ALL: [Protocol; 2] = [Protocol::OmenTracer, Protocol::None];

    /// The catalog's name for it.
    pub fn id(self) -> &'static str {
        match self {
            Protocol::OmenTracer => "omen-tracer",
            Protocol::None => "none",
        }
    }

    /// What the page calls it.
    pub fn label(self) -> &'static str {
        match self {
            Protocol::OmenTracer => "HP OMEN case board (TracerLED)",
            Protocol::None => "Unknown: list it, don't drive it",
        }
    }

    /// The zone numbers a zone test can light; none when Foxglove can't write to it.
    pub fn zones(self) -> Vec<u8> {
        match self {
            Protocol::OmenTracer => (1..=super::omen_tracer::MAX_ZONE).collect(),
            Protocol::None => Vec::new(),
        }
    }
}

/// How a zone takes a scene's light (`foxglove_scene::LightRole`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ZoneRole {
    /// Lit at its position: a fan, a logo, a strip.
    #[default]
    Point,
    /// An area light (a case's interior light): the average of the device's other zones,
    /// fading in and out with them.
    Ambient,
}

/// What kind of device a catalog entry is, for scenes' room map and the page: the catalog says
/// it, not the driver (one protocol can drive a case and a strip).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    /// A PC case's front: fans, logo, light strips.
    Case,
    /// A CPU or liquid cooler (its pump head).
    Cooler,
    /// A light strip or bar.
    Strip,
    /// A mouse, headset or other peripheral.
    Peripheral,
    #[default]
    Other,
}

impl DeviceKind {
    /// The catalog's name for it.
    pub fn id(self) -> &'static str {
        match self {
            DeviceKind::Case => "case",
            DeviceKind::Cooler => "cooler",
            DeviceKind::Strip => "strip",
            DeviceKind::Peripheral => "peripheral",
            DeviceKind::Other => "other",
        }
    }
}

/// Another app that drives the same device and may put its own colours back over Foxglove's
/// (a vendor's lighting tool). Its processes are checked only with a scan, never on a timer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conflict {
    /// Its name, as the page shows it.
    pub app: String,
    /// Its exe file names, matched exactly (any case).
    pub processes: Vec<String>,
    /// How to remove it or turn it off, shown on hover.
    #[serde(default)]
    pub hint: Option<String>,
}

/// One zone: the board's own zone number and what it lights.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneSpec {
    pub id: u8,
    pub name: String,
    /// Where the zone sits on the device's front, in millimetres from its top-left corner
    /// (x right, y down), for scenes' room map (#483). Unset zones are spread evenly.
    #[serde(default)]
    pub position: Option<[u16; 2]>,
    /// `role = "ambient"` for an area light; a point light when unset.
    #[serde(default)]
    pub role: ZoneRole,
}

/// One known board.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDevice {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub protocol: Protocol,
    /// What it is (`kind = "case"`); "other" when unset.
    #[serde(default)]
    pub kind: DeviceKind,
    /// Apps that may fight Foxglove over its lights (`[[device.conflict]]`).
    #[serde(default, rename = "conflict")]
    pub conflicts: Vec<Conflict>,
    #[serde(default)]
    pub zones: Vec<ZoneSpec>,
    /// How colour is addressed; per-zone when zones are listed, else none.
    #[serde(default)]
    pub color: Option<Color>,
    /// Onboard effects by name.
    #[serde(default)]
    pub effects: Vec<String>,
    #[serde(default)]
    pub brightness: bool,
    #[serde(default)]
    pub screen: Vec<Screen>,
    /// The model's default colour calibration, until the user calibrates their own unit.
    #[serde(default)]
    pub calibration: Option<Calibration>,
    /// The device's front face in millimetres (width, height), for scenes' room map (#483).
    #[serde(default)]
    pub size: Option<[u16; 2]>,
    /// From the user's own catalog (added by hand), not the bundled one.
    #[serde(skip)]
    pub local: bool,
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut q = String::from('"');
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            c if c.is_control() => q.push(' '),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

impl CatalogDevice {
    /// A new entry for the manual add.
    pub fn new(name: String, vid: u16, pid: u16, protocol: Protocol, zones: Vec<ZoneSpec>) -> Self {
        Self {
            name,
            vid,
            pid,
            protocol,
            kind: DeviceKind::Other,
            conflicts: Vec::new(),
            zones,
            color: None,
            effects: Vec::new(),
            brightness: false,
            screen: Vec::new(),
            calibration: None,
            size: None,
            local: true,
        }
    }

    /// What the board can do, as the catalog says.
    pub fn caps(&self) -> LightCaps {
        let color = self.color.unwrap_or(if self.zones.is_empty() {
            Color::None
        } else {
            Color::PerZone
        });
        LightCaps {
            color,
            count: if color == Color::PerZone {
                self.zones.len() as u32
            } else {
                0
            },
            effects: self.effects.clone(),
            brightness: self.brightness,
            screens: self.screen.clone(),
            ..Default::default()
        }
    }

    /// The entry as catalog TOML, to save, share or contribute. The manual add sets the
    /// name, ids, protocol and zones, so those are what's written.
    pub fn to_toml(&self) -> String {
        let mut out = format!(
            "[[device]]\nname = {}\nvid = 0x{:04X}\npid = 0x{:04X}\nprotocol = \"{}\"\n",
            quote(&self.name),
            self.vid,
            self.pid,
            self.protocol.id()
        );
        if self.kind != DeviceKind::Other {
            out.push_str(&format!("kind = \"{}\"\n", self.kind.id()));
        }
        if let Some([w, h]) = self.size {
            out.push_str(&format!("size = [{w}, {h}]\n"));
        }
        if !self.zones.is_empty() {
            out.push_str("zones = [\n");
            for z in &self.zones {
                let at = z
                    .position
                    .map(|[x, y]| format!(", position = [{x}, {y}]"))
                    .unwrap_or_default();
                let role = match z.role {
                    ZoneRole::Point => "",
                    ZoneRole::Ambient => ", role = \"ambient\"",
                };
                out.push_str(&format!(
                    "    {{ id = {}, name = {}{at}{role} }},\n",
                    z.id,
                    quote(&z.name)
                ));
            }
            out.push_str("]\n");
        }
        for c in &self.conflicts {
            let processes: Vec<String> = c.processes.iter().map(|p| quote(p)).collect();
            out.push_str(&format!(
                "[[device.conflict]]\napp = {}\nprocesses = [{}]\n",
                quote(&c.app),
                processes.join(", ")
            ));
            if let Some(hint) = &c.hint {
                out.push_str(&format!("hint = {}\n", quote(hint)));
            }
        }
        out
    }
}

/// Every known board.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(default, rename = "device")]
    pub devices: Vec<CatalogDevice>,
}

/// The catalog that ships with Foxglove.
pub const BUNDLED: &str = include_str!("../../lighting-catalog.toml");

impl Catalog {
    /// Reads a catalog and checks that every zone id is one its protocol can address.
    pub fn parse(text: &str) -> Result<Self, String> {
        let catalog: Catalog = toml::from_str(text).map_err(|e| e.to_string())?;
        for d in &catalog.devices {
            for z in &d.zones {
                let ok = match d.protocol {
                    Protocol::OmenTracer => super::omen_tracer::zone_ok(z.id),
                    Protocol::None => true,
                };
                if !ok {
                    return Err(format!("{}: zone {} is out of range", d.name, z.id));
                }
            }
            if d.protocol == Protocol::OmenTracer && d.zones.is_empty() {
                return Err(format!("{}: a driven board needs zones", d.name));
            }
        }
        Ok(catalog)
    }

    /// The bundled catalog. It is checked by the tests, so it always parses.
    pub fn bundled() -> Self {
        Self::parse(BUNDLED).unwrap_or_default()
    }

    pub fn find(&self, vid: u16, pid: u16) -> Option<&CatalogDevice> {
        self.devices.iter().find(|d| (d.vid, d.pid) == (vid, pid))
    }

    /// The user's own entries over this catalog: a local entry replaces a bundled one for the
    /// same VID/PID.
    pub fn with_local(mut self, local: &Catalog) -> Self {
        for d in &local.devices {
            let mut d = d.clone();
            d.local = true;
            self.devices.retain(|b| (b.vid, b.pid) != (d.vid, d.pid));
            self.devices.push(d);
        }
        self
    }

    /// Adds or replaces (same VID/PID) one entry.
    pub fn put(&mut self, device: CatalogDevice) {
        self.devices
            .retain(|d| (d.vid, d.pid) != (device.vid, device.pid));
        self.devices.push(device);
    }

    /// The whole catalog as TOML, for the user's local file.
    pub fn to_toml(&self) -> String {
        let mut out = String::from(
            "# Your own lighting devices, added in Foxglove (Lights > PC & peripherals > Add a device).\n\
             # Same format as crates/devices/lighting-catalog.toml: share an entry, or contribute it there.\n",
        );
        for d in &self.devices {
            out.push('\n');
            out.push_str(&d.to_toml());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_catalog_parses_with_the_45l_zone_map() {
        let c = Catalog::parse(BUNDLED).expect("bundled catalog");
        let case = c.find(0x103c, 0x84fd).expect("the 45L");
        assert_eq!(case.protocol, Protocol::OmenTracer);
        let zones: Vec<_> = case.zones.iter().map(|z| (z.id, z.name.as_str())).collect();
        assert_eq!(
            zones,
            vec![
                (1, "Diamond display"),
                (2, "Interior light"),
                (5, "Front bottom fan"),
                (6, "Front middle fan"),
                (7, "Front top fan"),
            ]
        );
        // The interior light fills the case: an area light, not a point (the fans are).
        let ambient: Vec<u8> = case
            .zones
            .iter()
            .filter(|z| z.role == ZoneRole::Ambient)
            .map(|z| z.id)
            .collect();
        assert_eq!(ambient, [2]);
        assert_eq!(
            c.find(0x03f0, 0x7397).map(|d| d.protocol),
            Some(Protocol::None)
        );
        let cal = case
            .calibration
            .as_ref()
            .expect("the 45L's default calibration");
        assert!(cal.is_neutral(), "the 45L starts neutral");
        // Scenes' room map (#483): the front fans run 7, 6, 5 top to bottom, the diamond on top.
        assert_eq!(case.size, Some([205, 475]));
        let y = |id: u8| {
            case.zones
                .iter()
                .find(|z| z.id == id)
                .unwrap()
                .position
                .unwrap()[1]
        };
        assert!(y(1) < y(7) && y(7) < y(6) && y(6) < y(5) && y(5) < 475);
        assert!(case.zones.iter().all(|z| z.position.is_some()));
        assert!(c.find(0x03f0, 0x7397).unwrap().calibration.is_none());
        // What it is, and the vendor tool that fights it, are data (#558).
        assert_eq!(case.kind, DeviceKind::Case);
        assert_eq!(case.conflicts.len(), 1);
        let tool = &case.conflicts[0];
        assert!(!tool.processes.is_empty() && tool.hint.is_some());
        assert_eq!(c.find(0x03f0, 0x7397).unwrap().kind, DeviceKind::Cooler);
    }

    #[test]
    fn kind_and_conflicts_are_read_and_default_to_none() {
        let text = r#"
            [[device]]
            name = "Strip"
            vid = 1
            pid = 2
            protocol = "none"
            kind = "strip"
            [[device.conflict]]
            app = "Glow"
            processes = ["Glow.exe", "GlowSvc.exe"]
            [[device.conflict]]
            app = "Other glow"
            processes = ["Other.exe"]
            hint = "Turn it off in its tray icon."

            [[device]]
            name = "Plain"
            vid = 1
            pid = 3
            protocol = "none"
        "#;
        let c = Catalog::parse(text).unwrap();
        let strip = c.find(1, 2).unwrap();
        assert_eq!(strip.kind, DeviceKind::Strip);
        assert_eq!(strip.conflicts.len(), 2);
        assert_eq!(strip.conflicts[0].hint, None);
        assert_eq!(strip.conflicts[1].processes, ["Other.exe"]);
        let plain = c.find(1, 3).unwrap();
        assert_eq!((plain.kind, plain.conflicts.len()), (DeviceKind::Other, 0));
        assert!(Catalog::parse(&text.replace("\"strip\"", "\"lamp\"")).is_err());
        // The kind survives the user's own file.
        let mut mine = CatalogDevice::new("Mine".into(), 9, 9, Protocol::None, vec![]);
        mine.kind = DeviceKind::Case;
        mine.conflicts = strip.conflicts.clone();
        let back = Catalog::parse(&mine.to_toml()).unwrap();
        assert_eq!(back.devices[0].kind, DeviceKind::Case);
        assert_eq!(
            back.devices[0].conflicts, strip.conflicts,
            "and its conflicts"
        );
        // The user's whole file, an entry with zones and conflicts, reads back the same.
        let mut local = Catalog::default();
        let mut zoned = mine.clone();
        zoned.zones = vec![ZoneSpec {
            id: 1,
            name: "Logo".into(),
            position: None,
            role: ZoneRole::Point,
        }];
        local.put(zoned.clone());
        let again = Catalog::parse(&local.to_toml()).unwrap();
        assert_eq!(again.devices[0].zones, zoned.zones);
        assert_eq!(again.devices[0].conflicts, zoned.conflicts);
    }

    #[test]
    fn bad_catalogs_are_refused() {
        let bad_zone = r#"
            [[device]]
            name = "x"
            vid = 1
            pid = 2
            protocol = "omen-tracer"
            zones = [{ id = 0, name = "nothing" }]
        "#;
        assert!(Catalog::parse(bad_zone).unwrap_err().contains("zone 0"));
        let unknown = r#"
            [[device]]
            name = "x"
            vid = 1
            pid = 2
            protocol = "laser"
        "#;
        assert!(Catalog::parse(unknown).is_err());
        let no_zones = r#"
            [[device]]
            name = "x"
            vid = 1
            pid = 2
            protocol = "omen-tracer"
        "#;
        assert!(Catalog::parse(no_zones).is_err());
    }

    #[test]
    fn a_manual_entry_round_trips_and_overrides_the_bundled_one() {
        let mine = CatalogDevice::new(
            "My \"odd\" case \\ 2".into(),
            0x103c,
            0x84fd,
            Protocol::OmenTracer,
            vec![ZoneSpec {
                id: 3,
                name: "Logo".into(),
                position: Some([40, 12]),
                role: ZoneRole::Ambient,
            }],
        );
        let mut local = Catalog::default();
        local.put(CatalogDevice {
            size: Some([200, 450]),
            ..mine.clone()
        });
        let text = local.to_toml();
        assert!(text.contains("vid = 0x103C"), "{text}");
        let back = Catalog::parse(&text).expect("its own output parses");
        assert_eq!(back.devices[0].name, mine.name);
        assert_eq!(back.devices[0].zones, mine.zones, "positions and roles too");
        assert_eq!(back.devices[0].size, Some([200, 450]));
        let merged = Catalog::bundled().with_local(&back);
        let case = merged.find(0x103c, 0x84fd).unwrap();
        assert!(case.local);
        assert_eq!(case.zones.len(), 1);
        assert_eq!(case.caps().count, 1);
        assert!(merged.find(0x03f0, 0x7397).is_some(), "the rest stay");
        local.put(CatalogDevice::new(
            "renamed".into(),
            0x103c,
            0x84fd,
            Protocol::None,
            vec![],
        ));
        assert_eq!(local.devices.len(), 1, "same VID/PID replaces");
    }

    #[test]
    fn caps_come_from_the_catalog() {
        let c = Catalog::bundled();
        let case = c.find(0x103c, 0x84fd).unwrap().caps();
        assert_eq!(case.color, Color::PerZone);
        assert_eq!(case.count, 5);
        assert!(case.brightness);
        assert_eq!(case.effects.len(), 5);
        let cooler = c.find(0x03f0, 0x7397).unwrap().caps();
        assert_eq!(cooler.color, Color::None);
        assert_eq!(cooler.screens.len(), 1);
        assert!(cooler.screens[0].images && cooler.screens[0].animations);
        assert_eq!(Protocol::OmenTracer.zones().len(), 13);
        assert!(Protocol::None.zones().is_empty());
    }
}
