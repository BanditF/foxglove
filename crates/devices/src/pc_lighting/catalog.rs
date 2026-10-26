//! The lighting catalog: boards Foxglove recognises by VID/PID, their protocol and zone names.
//! The bundled one is `crates/devices/lighting-catalog.toml`; adding a board that speaks a known
//! protocol is a data change.

use serde::Deserialize;

/// The driver a catalog board needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    /// HP OMEN case lighting board (TracerLED): 59-byte static colour reports per zone.
    OmenTracer,
    /// Recognised, no driver yet.
    None,
}

/// One zone: the board's own zone number and what it lights.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneSpec {
    pub id: u8,
    pub name: String,
}

/// One known board.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDevice {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub protocol: Protocol,
    #[serde(default)]
    pub zones: Vec<ZoneSpec>,
}

/// Every known board.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
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
}
