//! What Foxglove knows about QMK vendors and boards beyond the VIA protocol (#74): names for
//! boards without a VIA definition, boards that use Raw HID for something else, and lighting
//! scheme overrides. The bundled catalog is `crates/via/qmk-catalog.toml`.

use serde::Deserialize;

use crate::client::Scheme;

pub const BUNDLED: &str = include_str!("../qmk-catalog.toml");

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Catalog {
    pub lighting: LightingRules,
    #[serde(default, rename = "vendor")]
    pub vendors: Vec<Vendor>,
    #[serde(default, rename = "board")]
    pub boards: Vec<Board>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LightingRules {
    /// The first VIA protocol version with custom-value lighting channels.
    pub custom_channels_from: u16,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Vendor {
    pub vid: u16,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Board {
    pub vid: u16,
    pub pid: u16,
    pub name: Option<String>,
    #[serde(default)]
    pub skip: bool,
    pub reason: Option<String>,
    pub lighting: Option<LightingOverride>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightingOverride {
    Custom,
    Legacy,
    None,
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Catalog, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// The catalog that ships with Foxglove.
    pub fn bundled() -> Catalog {
        Catalog::parse(BUNDLED).expect("the bundled QMK catalog parses")
    }

    pub fn board(&self, vid: u16, pid: u16) -> Option<&Board> {
        self.boards.iter().find(|b| (b.vid, b.pid) == (vid, pid))
    }

    /// Why a board is never probed with VIA, if it isn't.
    pub fn skipped(&self, vid: u16, pid: u16) -> Option<&str> {
        self.board(vid, pid)
            .filter(|b| b.skip)
            .map(|b| b.reason.as_deref().unwrap_or("listed as not VIA"))
    }

    /// A name for a board without a VIA definition: the catalog's, else the vendor's name and
    /// the ids, else just the ids.
    pub fn name(&self, vid: u16, pid: u16) -> String {
        if let Some(name) = self.board(vid, pid).and_then(|b| b.name.clone()) {
            return name;
        }
        match self.vendors.iter().find(|v| v.vid == vid) {
            Some(v) => format!("{} keyboard {vid:04x}:{pid:04x}", v.name),
            None => format!("QMK keyboard {vid:04x}:{pid:04x}"),
        }
    }

    /// How a board's lighting values are addressed (`None`: don't read lighting).
    pub fn scheme(&self, vid: u16, pid: u16, protocol_version: u16) -> Option<Scheme> {
        match self.board(vid, pid).and_then(|b| b.lighting) {
            Some(LightingOverride::Custom) => Some(Scheme::Custom),
            Some(LightingOverride::Legacy) => Some(Scheme::Legacy),
            Some(LightingOverride::None) => None,
            None if protocol_version >= self.lighting.custom_channels_from => Some(Scheme::Custom),
            None => Some(Scheme::Legacy),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_skips_zmk() {
        let c = Catalog::bundled();
        assert_eq!(c.lighting.custom_channels_from, 12);
        assert!(c.skipped(0x1D50, 0x615E).unwrap().contains("ZMK"));
        assert!(c.skipped(0x3434, 0x0250).is_none());
        assert_eq!(c.name(0x3434, 0x0250), "Keychron K5 Pro (ANSI, RGB)");
        assert_eq!(c.name(0x3434, 0x0999), "Keychron keyboard 3434:0999");
        assert_eq!(c.name(0xFEED, 0x0001), "QMK keyboard feed:0001");
    }

    #[test]
    fn scheme_follows_the_protocol_version_unless_the_board_says_otherwise() {
        let mut c = Catalog::bundled();
        assert_eq!(c.scheme(1, 2, 12), Some(Scheme::Custom));
        assert_eq!(c.scheme(1, 2, 13), Some(Scheme::Custom));
        assert_eq!(c.scheme(1, 2, 9), Some(Scheme::Legacy), "Vial reports 9");
        c.boards.push(Board {
            vid: 1,
            pid: 2,
            lighting: Some(LightingOverride::None),
            ..Board::default()
        });
        assert_eq!(c.scheme(1, 2, 12), None);
        c.boards.last_mut().unwrap().lighting = Some(LightingOverride::Legacy);
        assert_eq!(c.scheme(1, 2, 12), Some(Scheme::Legacy));
    }
}
