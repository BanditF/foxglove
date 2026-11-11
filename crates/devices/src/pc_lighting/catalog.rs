//! The lighting catalog: boards Foxglove recognises by VID/PID, their protocol, zone names and
//! what they can do. The bundled one is `crates/devices/lighting-catalog.toml`; the user's own
//! entries (added by hand on the Lights page) sit in a local file of the same format and win
//! over the bundled ones. Adding a board that speaks a known protocol is a data change.

use serde::Deserialize;

use super::caps::{Color, LightCaps, Screen};
use super::zones::{Zone, ZoneSource, Zones};
use crate::calibration::Calibration;

/// The driver a catalog board needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    /// HP OMEN case lighting board (TracerLED): 59-byte static colour reports per zone.
    OmenTracer,
    /// Witmod-firmware LCD pump caps (#614): a stub that sends nothing until the ring's command
    /// is known (`[device.witmod]`, [`super::witmod_lcd`]).
    WitmodLcd,
    /// Recognised, no driver yet.
    None,
}

impl Protocol {
    /// Every protocol the manual add offers: a zone test writes its reports, so only the ones
    /// that can light a zone, and "none". The witmod-lcd stub isn't one.
    pub const ALL: [Protocol; 2] = [Protocol::OmenTracer, Protocol::None];

    /// The catalog's name for it.
    pub fn id(self) -> &'static str {
        match self {
            Protocol::OmenTracer => "omen-tracer",
            Protocol::WitmodLcd => "witmod-lcd",
            Protocol::None => "none",
        }
    }

    /// What the page calls it.
    pub fn label(self) -> &'static str {
        match self {
            Protocol::OmenTracer => "HP OMEN case board (TracerLED)",
            Protocol::WitmodLcd => "Witmod LCD pump cap",
            Protocol::None => "Unknown: list it, don't drive it",
        }
    }

    /// The zone numbers a zone test can light; none when Foxglove can't write to it.
    pub fn zones(self) -> Vec<u8> {
        match self {
            Protocol::OmenTracer => (1..=super::omen_tracer::MAX_ZONE).collect(),
            Protocol::WitmodLcd | Protocol::None => Vec::new(),
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

/// How a device's thumbnail is drawn (#803), when the catalog says (`shape = "mouse"`). Unset,
/// the thumbnail follows what Windows says the device is (its LampArray kind), then the entry's
/// [`DeviceKind`]. A class, never a product: no entry gets its own drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// Keys: the keyboard's own layout when Foxglove knows it, else a plain board.
    Keyboard,
    /// A case's front: a ring per fan zone, a bar per strip zone.
    Case,
    /// One ring (a pump head).
    Cooler,
    /// A bar with a dot per LED.
    Strip,
    Mouse,
    Headset,
    /// A game controller.
    Controller,
    /// Only its lamps, each where it sits.
    Lamps,
}

impl Shape {
    /// Every shape, in the catalog's order.
    pub const ALL: [Shape; 8] = [
        Shape::Keyboard,
        Shape::Case,
        Shape::Cooler,
        Shape::Strip,
        Shape::Mouse,
        Shape::Headset,
        Shape::Controller,
        Shape::Lamps,
    ];

    /// The catalog's name for it.
    pub fn id(self) -> &'static str {
        match self {
            Shape::Keyboard => "keyboard",
            Shape::Case => "case",
            Shape::Cooler => "cooler",
            Shape::Strip => "strip",
            Shape::Mouse => "mouse",
            Shape::Headset => "headset",
            Shape::Controller => "controller",
            Shape::Lamps => "lamps",
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

/// A starting white balance for the match calibrator (#473): the gains a device's first match
/// starts from, instead of neutral, so the white step starts close. Only a starting point: it
/// changes nothing a device shows until the user confirms a match. Named once in the catalog
/// (`[[calibration_hint]]`) and used by a device class (`kinds`) or a model
/// (`calibration_hint = "<id>"` on its entry).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationHint {
    /// What entries name it by.
    pub id: String,
    /// Each channel's gain in linear light, as [`Calibration::gain`] (1 = unchanged).
    pub gain: [f32; 3],
    /// The device classes that start from it unless their entry names another.
    #[serde(default)]
    pub kinds: Vec<DeviceKind>,
}

/// An entry's `calibration_hint` that opts out of its class's hint.
pub const NO_HINT: &str = "none";

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
    /// How its thumbnail is drawn (`shape = "strip"`), when its kind doesn't say it well.
    #[serde(default)]
    pub shape: Option<Shape>,
    /// Apps that may fight Foxglove over its lights (`[[device.conflict]]`).
    #[serde(default, rename = "conflict")]
    pub conflicts: Vec<Conflict>,
    #[serde(default)]
    pub zones: Vec<ZoneSpec>,
    /// How colour is addressed; per-zone when zones are listed, else none.
    #[serde(default)]
    pub color: Option<Color>,
    /// Onboard effects by name. On a device Foxglove can't drive yet, the first is the one it
    /// shows on its own (the page says so).
    #[serde(default)]
    pub effects: Vec<String>,
    #[serde(default)]
    pub brightness: bool,
    #[serde(default)]
    pub screen: Vec<Screen>,
    /// The model's default colour calibration, until the user calibrates their own unit.
    #[serde(default)]
    pub calibration: Option<Calibration>,
    /// The model's starting gains for its first match (a [`CalibrationHint`] id), over its
    /// class's; `"none"` for neither. Unset: its class's, if any.
    #[serde(default)]
    pub calibration_hint: Option<String>,
    /// The device's front face in millimetres (width, height), for scenes' room map (#483).
    #[serde(default)]
    pub size: Option<[u16; 2]>,
    /// Named from a vendor's software, not yet seen on real hardware.
    #[serde(default)]
    pub unconfirmed: bool,
    /// The witmod-lcd protocol's numbers (`[device.witmod]`); only on that protocol.
    #[serde(default)]
    pub witmod: Option<super::witmod_lcd::Params>,
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
            shape: None,
            conflicts: Vec::new(),
            zones,
            color: None,
            effects: Vec::new(),
            brightness: false,
            screen: Vec::new(),
            calibration: None,
            calibration_hint: None,
            size: None,
            unconfirmed: false,
            witmod: None,
            local: true,
        }
    }

    /// Whether Foxglove can change its lights: its protocol has a driver, and that driver
    /// knows the command it needs (a witmod-lcd cap only once its ring op is known).
    pub fn controllable(&self) -> bool {
        match self.protocol {
            Protocol::OmenTracer => true,
            Protocol::WitmodLcd => self.witmod.as_ref().is_some_and(|w| w.ring_known()),
            Protocol::None => false,
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
            zones: (color == Color::PerZone).then(|| self.zone_map()),
            ..Default::default()
        }
    }

    /// Its zones as areas (#611): from Foxglove's catalog, or the user's own check when they
    /// added the device by hand (they named each zone as it lit).
    fn zone_map(&self) -> Zones {
        Zones {
            list: self
                .zones
                .iter()
                .map(|z| Zone {
                    area: Some(z.name.clone()),
                    ..Default::default()
                })
                .collect(),
            dead: vec![0],
            source: if self.local {
                ZoneSource::Check
            } else {
                ZoneSource::Catalog
            },
            reported: None,
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
        if let Some(shape) = self.shape {
            out.push_str(&format!("shape = \"{}\"\n", shape.id()));
        }
        if let Some(hint) = &self.calibration_hint {
            out.push_str(&format!("calibration_hint = {}\n", quote(hint)));
        }
        if let Some([w, h]) = self.size {
            out.push_str(&format!("size = [{w}, {h}]\n"));
        }
        if self.unconfirmed {
            out.push_str("unconfirmed = true\n");
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
        if let Some(w) = &self.witmod {
            out.push_str(&w.to_toml());
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
    /// Starting white balances for the match calibrator, by id.
    #[serde(default, rename = "calibration_hint")]
    pub hints: Vec<CalibrationHint>,
}

/// The catalog that ships with Foxglove.
pub const BUNDLED: &str = include_str!("../../lighting-catalog.toml");

impl Catalog {
    /// Reads a catalog and checks that every zone id is one its protocol can address.
    pub fn parse(text: &str) -> Result<Self, String> {
        let catalog: Catalog = toml::from_str(text).map_err(|e| e.to_string())?;
        for (i, h) in catalog.hints.iter().enumerate() {
            if h.id.trim().is_empty() || h.id == NO_HINT {
                return Err(format!("calibration hint {:?} needs another id", h.id));
            }
            if catalog.hints[..i].iter().any(|o| o.id == h.id) {
                return Err(format!("calibration hint {} is listed twice", h.id));
            }
            if !h
                .gain
                .iter()
                .all(|g| g.is_finite() && (0.0..=2.0).contains(g))
            {
                return Err(format!("calibration hint {}: gains go from 0 to 2", h.id));
            }
        }
        for d in &catalog.devices {
            for z in &d.zones {
                let ok = match d.protocol {
                    Protocol::OmenTracer => super::omen_tracer::zone_ok(z.id),
                    Protocol::WitmodLcd => super::witmod_lcd::zone_ok(z.id),
                    Protocol::None => true,
                };
                if !ok {
                    return Err(format!("{}: zone {} is out of range", d.name, z.id));
                }
            }
            if d.protocol == Protocol::OmenTracer && d.zones.is_empty() {
                return Err(format!("{}: a driven board needs zones", d.name));
            }
            match (d.protocol, &d.witmod) {
                (Protocol::WitmodLcd, None) => {
                    return Err(format!("{}: witmod-lcd needs [device.witmod]", d.name));
                }
                (Protocol::WitmodLcd, Some(w)) => {
                    w.check().map_err(|e| format!("{}: {e}", d.name))?;
                }
                (_, Some(_)) => {
                    return Err(format!("{}: [device.witmod] is for witmod-lcd", d.name));
                }
                (_, None) => {}
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

    /// The gains a device's first match starts from: its entry's hint, else its class's, else
    /// none (neutral). A name no hint has is none too: a local entry can name one the
    /// bundled catalog dropped.
    pub fn hint(&self, device: &CatalogDevice) -> Option<&CalibrationHint> {
        match device.calibration_hint.as_deref() {
            Some(id) => self.hints.iter().find(|h| h.id == id),
            None => self.hints.iter().find(|h| h.kinds.contains(&device.kind)),
        }
    }

    /// The user's own entries over this catalog: a local entry replaces a bundled one for the
    /// same VID/PID, and a local hint a bundled one with the same id.
    pub fn with_local(mut self, local: &Catalog) -> Self {
        for h in &local.hints {
            self.hints.retain(|b| b.id != h.id);
            self.hints.push(h.clone());
        }
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
        for h in &self.hints {
            let kinds: Vec<String> = h.kinds.iter().map(|k| quote(k.id())).collect();
            out.push_str(&format!(
                "\n[[calibration_hint]]\nid = {}\ngain = [{:?}, {:?}, {:?}]\n",
                quote(&h.id),
                h.gain[0],
                h.gain[1],
                h.gain[2]
            ));
            if !kinds.is_empty() {
                out.push_str(&format!("kinds = [{}]\n", kinds.join(", ")));
            }
        }
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
            Some(Protocol::WitmodLcd)
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
        // No bundled model starts from a hint: their first matches start neutral, as before.
        assert!(c.devices.iter().all(|d| c.hint(d).is_none()));
    }

    /// #614: the LCD pump caps are a class in the catalog: zones, the effect they keep, the
    /// vendor helper that holds them, and a second model known only from that helper.
    #[test]
    fn the_bundled_lcd_coolers_are_data() {
        let c = Catalog::bundled();
        let cap = c.find(0x03f0, 0x7397).expect("the 45L's pump cap");
        assert_eq!(cap.kind, DeviceKind::Cooler);
        let zones: Vec<_> = cap.zones.iter().map(|z| (z.id, z.name.as_str())).collect();
        assert_eq!(zones, [(1, "Pump ring"), (2, "Radiator fans")]);
        assert_eq!(cap.effects, ["rainbow"]);
        assert!(!cap.unconfirmed);
        assert!(!cap.controllable(), "no ring command known yet");
        let helper = cap
            .conflicts
            .iter()
            .find(|k| k.processes.iter().any(|p| p == "OMENLCDDisplayHelper.exe"))
            .expect("HP's LCD helper");
        assert!(helper.hint.is_some());
        let other = c.find(0x03f0, 0x02be).expect("the second LCD cooler");
        assert_eq!(
            (other.kind, other.protocol),
            (DeviceKind::Cooler, Protocol::None)
        );
        assert!(other.unconfirmed);
        assert!(!other.controllable());
        // Nothing else is unconfirmed, and the flag survives the user's own file.
        assert_eq!(c.devices.iter().filter(|d| d.unconfirmed).count(), 1);
        let back = Catalog::parse(&other.to_toml()).unwrap();
        assert!(back.devices[0].unconfirmed);
        // Its protocol table belongs to its protocol.
        assert!(Catalog::parse(&cap.to_toml().replace("witmod-lcd", "none")).is_err());
    }

    #[test]
    fn the_bundled_hints_are_fastleds_presets() {
        let c = Catalog::bundled();
        let gain = |id: &str| c.hints.iter().find(|h| h.id == id).unwrap().gain;
        let byte = |g: [f32; 3]| g.map(|v| (v * 255.0).round() as u8);
        assert_eq!(byte(gain("typical-smd5050")), [0xff, 0xb0, 0xf0]);
        assert_eq!(byte(gain("typical-8mm-pixel")), [0xff, 0xe0, 0x8c]);
        // A strip with no hint of its own starts from the strip class's.
        let strip = CatalogDevice {
            kind: DeviceKind::Strip,
            ..CatalogDevice::new("Strip".into(), 1, 2, Protocol::None, vec![])
        };
        assert_eq!(c.hint(&strip).unwrap().id, "typical-smd5050");
    }

    #[test]
    fn hints_come_from_the_model_then_its_class() {
        let text = r#"
            [[device]]
            name = "Strip"
            vid = 1
            pid = 2
            protocol = "none"
            kind = "strip"

            [[device]]
            name = "Pixel strip"
            vid = 1
            pid = 3
            protocol = "none"
            kind = "strip"
            calibration_hint = "pixel"

            [[device]]
            name = "Plain strip"
            vid = 1
            pid = 4
            protocol = "none"
            kind = "strip"
            calibration_hint = "none"

            [[device]]
            name = "Case"
            vid = 1
            pid = 5
            protocol = "none"
            kind = "case"

            [[calibration_hint]]
            id = "strip"
            gain = [1.0, 0.7, 0.9]
            kinds = ["strip"]

            [[calibration_hint]]
            id = "pixel"
            gain = [1.0, 0.9, 0.5]
        "#;
        let c = Catalog::parse(text).unwrap();
        let hint = |pid: u16| c.hint(c.find(1, pid).unwrap()).map(|h| h.gain);
        assert_eq!(hint(2), Some([1.0, 0.7, 0.9]), "its class's");
        assert_eq!(hint(3), Some([1.0, 0.9, 0.5]), "its own, over its class's");
        assert_eq!(hint(4), None, "opted out");
        assert_eq!(hint(5), None, "no hint: neutral, as before");
        // Discovery carries the gains to the found device.
        let entry = |pid: u16| super::super::HidEntry {
            vid: 1,
            pid,
            ..Default::default()
        };
        let found = super::super::discover(&c, &[entry(3), entry(5)]);
        assert_eq!(found[0].calibration_hint, Some([1.0, 0.9, 0.5]));
        assert_eq!(found[1].calibration_hint, None);
        // Bad hints are refused.
        let bad = |gain: &str, id: &str| {
            Catalog::parse(&format!(
                "[[calibration_hint]]\nid = \"{id}\"\ngain = {gain}\n"
            ))
        };
        assert!(bad("[1.0, 1.0, 1.0]", "ok").is_ok());
        assert!(bad("[1.0, 3.0, 1.0]", "x").is_err());
        assert!(bad("[1.0, -0.1, 1.0]", "x").is_err());
        assert!(bad("[1.0, 1.0]", "x").is_err());
        assert!(bad("[1.0, 1.0, 1.0]", "none").is_err());
        assert!(bad("[1.0, 1.0, 1.0]", " ").is_err());
        let twice =
            format!("{text}\n[[calibration_hint]]\nid = \"pixel\"\ngain = [1.0, 1.0, 1.0]\n");
        assert!(Catalog::parse(&twice).unwrap_err().contains("twice"));
        // The user's own file keeps its hints and an entry's choice, and they win over the
        // bundled ones with the same id.
        let mut local = Catalog {
            hints: vec![CalibrationHint {
                id: "typical-smd5050".into(),
                gain: [1.0, 0.5, 0.5],
                kinds: vec![DeviceKind::Strip, DeviceKind::Peripheral],
            }],
            ..Default::default()
        };
        let mut mine = CatalogDevice::new("Mine".into(), 9, 9, Protocol::None, vec![]);
        mine.calibration_hint = Some("typical-8mm-pixel".into());
        local.put(mine);
        let back = Catalog::parse(&local.to_toml()).unwrap();
        assert_eq!(back.hints, local.hints);
        assert_eq!(
            back.devices[0].calibration_hint.as_deref(),
            Some("typical-8mm-pixel")
        );
        let merged = Catalog::bundled().with_local(&back);
        assert_eq!(merged.hints.len(), 2);
        let mine = merged.find(9, 9).unwrap();
        assert_eq!(merged.hint(mine).unwrap().gain, [1.0, 0.8784, 0.549]);
        let strip = CatalogDevice {
            kind: DeviceKind::Strip,
            ..CatalogDevice::new("Strip".into(), 1, 2, Protocol::None, vec![])
        };
        assert_eq!(merged.hint(&strip).unwrap().gain, [1.0, 0.5, 0.5]);
    }

    #[test]
    fn shape_is_read_written_and_optional() {
        let entry = |shape: &str| {
            format!("[[device]]\nname = \"Thing\"\nvid = 1\npid = 2\nprotocol = \"none\"\n{shape}")
        };
        for shape in Shape::ALL {
            let c = Catalog::parse(&entry(&format!("shape = \"{}\"\n", shape.id())))
                .expect("a shape parses");
            assert_eq!(c.devices[0].shape, Some(shape));
            let back = Catalog::parse(&c.devices[0].to_toml()).expect("written back");
            assert_eq!(back.devices[0].shape, Some(shape), "{}", shape.id());
        }
        let none = Catalog::parse(&entry("")).unwrap();
        assert_eq!(none.devices[0].shape, None);
        assert!(!none.devices[0].to_toml().contains("shape"));
        // A class, never a product.
        assert!(Catalog::parse(&entry("shape = \"proteus\"\n")).is_err());
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
        // Added by hand: the user named each zone as it lit, so it is their check (#611).
        let mine_caps = case.caps();
        assert_eq!(mine_caps.zones.unwrap().source, super::ZoneSource::Check);
        assert_eq!(mine_caps.color, Color::PerZone);
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
        // Its real resolution (#611): five areas, from the catalog.
        assert_eq!(case.summary(false).unwrap(), "5 lighting zones");
        let zones = case.zones.as_ref().unwrap();
        assert_eq!(zones.source, super::ZoneSource::Catalog);
        assert_eq!(zones.list[4].area.as_deref(), Some("Front top fan"));
        let cooler = c.find(0x03f0, 0x7397).unwrap().caps();
        // Its ring and fan chain as two zones (#614), and the effect it keeps.
        assert_eq!((cooler.color, cooler.count), (Color::PerZone, 2));
        assert_eq!(cooler.effects, ["rainbow"]);
        assert_eq!(cooler.screens.len(), 1);
        assert!(cooler.screens[0].images && cooler.screens[0].animations);
        assert_eq!(Protocol::OmenTracer.zones().len(), 13);
        assert!(Protocol::None.zones().is_empty());
    }
}
