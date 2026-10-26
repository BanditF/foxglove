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

/// One zone: the board's own zone number and what it lights.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneSpec {
    pub id: u8,
    pub name: String,
}

/// One known board.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDevice {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub protocol: Protocol,
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
            zones,
            color: None,
            effects: Vec::new(),
            brightness: false,
            screen: Vec::new(),
            calibration: None,
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
        if !self.zones.is_empty() {
            out.push_str("zones = [\n");
            for z in &self.zones {
                out.push_str(&format!(
                    "    {{ id = {}, name = {} }},\n",
                    z.id,
                    quote(&z.name)
                ));
            }
            out.push_str("]\n");
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
        assert_eq!(
            c.find(0x03f0, 0x7397).map(|d| d.protocol),
            Some(Protocol::None)
        );
        let cal = case
            .calibration
            .as_ref()
            .expect("the 45L's default calibration");
        assert!(cal.is_neutral(), "the 45L starts neutral");
        assert!(c.find(0x03f0, 0x7397).unwrap().calibration.is_none());
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
            }],
        );
        let mut local = Catalog::default();
        local.put(mine.clone());
        let text = local.to_toml();
        assert!(text.contains("vid = 0x103C"), "{text}");
        let back = Catalog::parse(&text).expect("its own output parses");
        assert_eq!(back.devices[0].name, mine.name);
        assert_eq!(back.devices[0].zones, mine.zones);
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
