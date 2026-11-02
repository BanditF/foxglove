//! Windows Dynamic Lighting (HID LampArray) devices (#691), described from what the device
//! itself reports: its lamps, where they sit and what each is for. Systems, not products:
//! nothing here knows a vendor or a model; the host reads the attributes (on Windows, through
//! `Windows.Devices.Lights`) and this turns them into the zones every other light has, one per
//! lamp.

use super::{LampArrayInfo, ZoneRole, ZoneSpec};

/// The most lamps that get a zone each (zone ids are one byte). A larger array still takes one
/// colour on every lamp.
pub const MAX_LAMPS: usize = 256;

/// `LampPurposes` bits (HID Lighting and Illumination page, LampPurposes; WinRT `LampPurposes`).
pub mod purpose {
    pub const CONTROL: u32 = 0x01;
    pub const ACCENT: u32 = 0x02;
    pub const BRANDING: u32 = 0x04;
    pub const STATUS: u32 = 0x08;
    pub const ILLUMINATION: u32 = 0x10;
    pub const PRESENTATION: u32 = 0x20;
}

/// One lamp, as the device reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lamp {
    /// Its position on the device's front in millimetres (x right, y down from the bounding
    /// box's corner); `None` when the device gives none (every lamp at the origin).
    pub position: Option<[u16; 2]>,
    /// Its `LampPurposes` bits.
    pub purposes: u32,
    /// It takes a colour: it has red, green or blue levels and no fixed colour.
    pub colour: bool,
}

/// A LampArray as the host read it: who it is, its attributes and its lamps in index order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LampArrayDesc {
    pub vid: u16,
    pub pid: u16,
    pub info: LampArrayInfo,
    /// Its bounding box's front in millimetres (width, height); `None` when it gives none.
    pub size: Option<[u16; 2]>,
    pub lamps: Vec<Lamp>,
}

/// The plain word for a lamp's main purpose: "Logo", "Accent"... `None` when it gives none.
/// A lamp with several purposes is named by the most telling one.
pub fn purpose_name(purposes: u32) -> Option<&'static str> {
    [
        (purpose::BRANDING, "Logo"),
        (purpose::STATUS, "Status"),
        (purpose::ACCENT, "Accent"),
        (purpose::ILLUMINATION, "Light"),
        (purpose::PRESENTATION, "Display"),
        (purpose::CONTROL, "Key"),
    ]
    .into_iter()
    .find(|(bit, _)| purposes & bit != 0)
    .map(|(_, name)| name)
}

/// One zone per lamp (at most [`MAX_LAMPS`]), in index order: id = the lamp's index, named
/// "Lamp 1 · Logo" (counting from 1, as people do), at the lamp's position. An illumination
/// lamp lights an area, so a scene gives it the colour around it (ambient).
pub fn lamp_zones(lamps: &[Lamp]) -> Vec<ZoneSpec> {
    lamps
        .iter()
        .take(MAX_LAMPS)
        .enumerate()
        .map(|(i, lamp)| ZoneSpec {
            id: i as u8,
            name: match purpose_name(lamp.purposes) {
                Some(what) => format!("Lamp {} · {what}", i + 1),
                None => format!("Lamp {}", i + 1),
            },
            position: lamp.position,
            role: if lamp.purposes & purpose::ILLUMINATION != 0 {
                ZoneRole::Ambient
            } else {
                ZoneRole::Point
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lamp(position: Option<[u16; 2]>, purposes: u32) -> Lamp {
        Lamp {
            position,
            purposes,
            colour: true,
        }
    }

    #[test]
    fn each_lamp_is_a_zone_named_by_its_purpose() {
        // A mouse: a logo and a strip of accents; a headset-like light with no purposes.
        let zones = lamp_zones(&[
            lamp(Some([30, 70]), purpose::BRANDING),
            lamp(Some([10, 20]), purpose::ACCENT | purpose::CONTROL),
            lamp(None, 0),
            lamp(Some([0, 5]), purpose::ILLUMINATION),
        ]);
        let rows: Vec<_> = zones
            .iter()
            .map(|z| (z.id, z.name.as_str(), z.position, z.role))
            .collect();
        assert_eq!(
            rows,
            vec![
                (0, "Lamp 1 · Logo", Some([30, 70]), ZoneRole::Point),
                (1, "Lamp 2 · Accent", Some([10, 20]), ZoneRole::Point),
                (2, "Lamp 3", None, ZoneRole::Point),
                (3, "Lamp 4 · Light", Some([0, 5]), ZoneRole::Ambient),
            ]
        );
    }

    #[test]
    fn a_large_array_is_capped_at_one_byte_of_zone_ids() {
        let many = vec![lamp(None, purpose::CONTROL); 300];
        let zones = lamp_zones(&many);
        assert_eq!(zones.len(), MAX_LAMPS);
        assert_eq!(zones.last().unwrap().id, 255);
        assert_eq!(zones[0].name, "Lamp 1 · Key");
    }

    #[test]
    fn purposes_name_the_most_telling_one() {
        assert_eq!(purpose_name(0), None);
        assert_eq!(
            purpose_name(purpose::CONTROL | purpose::STATUS),
            Some("Status")
        );
        assert_eq!(purpose_name(purpose::PRESENTATION), Some("Display"));
    }
}
