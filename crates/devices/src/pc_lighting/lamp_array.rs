//! Windows Dynamic Lighting (HID LampArray) devices (#691), described from what the device
//! itself reports: its lamps, where they sit and what each is for. Systems, not products:
//! nothing here knows a vendor or a model; the host reads the attributes (on Windows, through
//! `Windows.Devices.Lights`) and this turns them into the zones every other light has, one per
//! lamp.

use super::{LampArrayInfo, LightingDevice, ZoneRole, ZoneSpec};
use crate::{DeviceError, Result, Rgb};

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

/// One LampArray as the host drives it: Windows' `LampArray` in the app, a fake in tests.
/// Lamp indices are the device's own (0-based, in its order).
pub trait LampHandle: Send {
    /// How many lamps the device has (may be more than [`MAX_LAMPS`]).
    fn lamp_count(&self) -> usize;
    /// Every lamp one colour.
    fn set_all(&mut self, color: Rgb) -> Result<()>;
    /// Some lamps, each its own colour: `(lamp index, colour)`.
    fn set_lamps(&mut self, lamps: &[(usize, Rgb)]) -> Result<()>;
}

/// A LampArray as a [`LightingDevice`]: one zone per lamp, a static colour each. What it
/// shows is the host's business (Windows decides which app's colours a device shows); the
/// driver remembers what Foxglove set, as every write-only driver does.
pub struct LampArrayDriver {
    name: String,
    zones: Vec<String>,
    colors: Vec<Option<Rgb>>,
    handle: Box<dyn LampHandle>,
}

impl LampArrayDriver {
    /// A driver over `handle` for these zones (one per lamp, from [`lamp_zones`]).
    pub fn new(name: String, zones: &[ZoneSpec], handle: Box<dyn LampHandle>) -> Result<Self> {
        if zones.is_empty()
            || zones
                .iter()
                .any(|z| usize::from(z.id) >= handle.lamp_count())
        {
            return Err(DeviceError::Backend(format!("{name}: lamps not read yet")));
        }
        Ok(Self {
            name,
            zones: zones.iter().map(|z| z.name.clone()).collect(),
            colors: vec![None; zones.len()],
            handle,
        })
    }
}

impl LightingDevice for LampArrayDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn zones(&self) -> Vec<String> {
        self.zones.clone()
    }

    fn colors(&self) -> Vec<Option<Rgb>> {
        self.colors.clone()
    }

    /// One colour on every lamp is one call (and reaches lamps past [`MAX_LAMPS`]); anything
    /// else names its lamps.
    fn set_zones(&mut self, colors: &[(usize, Rgb)]) -> Result<()> {
        if colors.iter().any(|&(i, _)| i >= self.zones.len()) {
            return Err(DeviceError::Unsupported);
        }
        let Some(&(_, first)) = colors.first() else {
            return Ok(());
        };
        let mut covered = vec![false; self.zones.len()];
        colors.iter().for_each(|&(i, _)| covered[i] = true);
        let whole = covered.iter().all(|c| *c) && colors.iter().all(|&(_, c)| c == first);
        if whole {
            self.handle.set_all(first)?;
        } else {
            self.handle.set_lamps(colors)?;
        }
        for &(i, c) in colors {
            self.colors[i] = Some(c);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// A fake LampArray: records each call.
    #[derive(Clone, Default)]
    pub(crate) struct FakeLamps {
        pub count: usize,
        pub calls: std::sync::Arc<std::sync::Mutex<Vec<Call>>>,
        pub fail: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum Call {
        All(Rgb),
        Lamps(Vec<(usize, Rgb)>),
    }

    impl LampHandle for FakeLamps {
        fn lamp_count(&self) -> usize {
            self.count
        }
        fn set_all(&mut self, color: Rgb) -> Result<()> {
            if self.fail {
                return Err(DeviceError::Disconnected);
            }
            self.calls.lock().unwrap().push(Call::All(color));
            Ok(())
        }
        fn set_lamps(&mut self, lamps: &[(usize, Rgb)]) -> Result<()> {
            if self.fail {
                return Err(DeviceError::Disconnected);
            }
            self.calls.lock().unwrap().push(Call::Lamps(lamps.to_vec()));
            Ok(())
        }
    }

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    #[test]
    fn one_colour_on_every_lamp_is_one_call_else_per_lamp() {
        let fake = FakeLamps {
            count: 3,
            ..Default::default()
        };
        let zones = lamp_zones(&vec![lamp(None, purpose::ACCENT); 3]);
        let mut d =
            LampArrayDriver::new("Desk strip".into(), &zones, Box::new(fake.clone())).unwrap();
        assert_eq!(
            d.zones(),
            vec!["Lamp 1 · Accent", "Lamp 2 · Accent", "Lamp 3 · Accent"]
        );
        d.set_all(RED).unwrap();
        d.set_zones(&[(1, BLUE)]).unwrap();
        d.set_zones(&[(0, RED), (1, BLUE), (2, RED)]).unwrap();
        assert_eq!(
            *fake.calls.lock().unwrap(),
            vec![
                Call::All(RED),
                Call::Lamps(vec![(1, BLUE)]),
                Call::Lamps(vec![(0, RED), (1, BLUE), (2, RED)]),
            ]
        );
        assert_eq!(d.colors(), vec![Some(RED), Some(BLUE), Some(RED)]);
        assert_eq!(d.set_zones(&[(3, RED)]), Err(DeviceError::Unsupported));
    }

    #[test]
    fn a_failed_write_keeps_the_colours_it_had_and_unread_lamps_dont_open() {
        let mut fake = FakeLamps {
            count: 2,
            ..Default::default()
        };
        let zones = lamp_zones(&[lamp(None, 0), lamp(None, 0)]);
        assert!(LampArrayDriver::new("x".into(), &[], Box::new(fake.clone())).is_err());
        let short = FakeLamps {
            count: 1,
            ..Default::default()
        };
        assert!(LampArrayDriver::new("x".into(), &zones, Box::new(short)).is_err());
        fake.fail = true;
        let mut d = LampArrayDriver::new("x".into(), &zones, Box::new(fake)).unwrap();
        assert!(d.set_all(RED).is_err());
        assert_eq!(d.colors(), vec![None, None]);
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
