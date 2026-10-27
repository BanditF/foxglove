//! The calibration boundary for every [`LightingDevice`] driver: [`super::open`] wraps each
//! driver in [`Calibrated`], so every colour Foxglove sets passes through the device's (or the
//! zone's) calibration on its way out, and the driver itself never needs to know.
//!
//! The colours it reports ([`LightingDevice::colors`]) are the ones Foxglove asked for, so a
//! restore, an identify or a recalibration puts the same palette back through the current
//! calibration.

use super::{LightingDevice, ZoneSpec};
use crate::calibration::{self, Calibration};
use crate::{Result, Rgb};

/// A driver whose colours go through its device's calibration.
pub struct Calibrated {
    inner: Box<dyn LightingDevice>,
    /// The device key calibrations are stored under.
    key: String,
    /// Each zone's id, in the driver's zone order: a zone's own calibration is found by it.
    zone_ids: Vec<String>,
    /// The model's default, from the catalog.
    default: Option<Calibration>,
    /// What Foxglove asked each zone to show, before calibration.
    asked: Vec<Option<Rgb>>,
}

impl Calibrated {
    pub fn new(
        inner: Box<dyn LightingDevice>,
        key: String,
        zones: &[ZoneSpec],
        default: Option<Calibration>,
    ) -> Self {
        let n = inner.zones().len();
        Self {
            asked: vec![None; n],
            zone_ids: zones.iter().map(|z| z.id.to_string()).collect(),
            inner,
            key,
            default,
        }
    }

    /// The colour a zone is sent for `c`, through the calibration as it is now.
    pub fn calibrated(&self, zone: usize, c: Rgb) -> Rgb {
        calibration::global().rgb(
            &self.key,
            self.zone_ids.get(zone).map(String::as_str),
            self.default.as_ref(),
            c,
        )
    }
}

impl LightingDevice for Calibrated {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn zones(&self) -> Vec<String> {
        self.inner.zones()
    }

    fn colors(&self) -> Vec<Option<Rgb>> {
        self.asked.clone()
    }

    fn set_zones(&mut self, colors: &[(usize, Rgb)]) -> Result<()> {
        let sent: Vec<_> = colors
            .iter()
            .map(|&(i, c)| (i, self.calibrated(i, c)))
            .collect();
        self.inner.set_zones(&sent)?;
        for &(i, c) in colors {
            if let Some(slot) = self.asked.get_mut(i) {
                *slot = Some(c);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::DeviceCalibration;
    use crate::pc_lighting::omen_tracer::tests::Recorder;
    use crate::pc_lighting::{discover, open, Catalog, HidEntry};

    fn case(key_serial: &str, rec: &Recorder) -> Box<dyn LightingDevice> {
        let found = discover(
            &Catalog::bundled(),
            &[HidEntry {
                vid: 0x103c,
                pid: 0x84fd,
                usage_page: 1,
                path: "tracer".into(),
                serial: Some(key_serial.into()),
                ..Default::default()
            }],
        );
        open(&found[0], Box::new(rec.clone())).unwrap()
    }

    fn sent_color(r: &[u8]) -> [u8; 3] {
        let zone = usize::from(r[0x36]);
        let at = 0x08 + 3 * (zone - 1);
        [r[at], r[at + 1], r[at + 2]]
    }

    const ORANGE: Rgb = Rgb {
        r: 0xf0,
        g: 0x8a,
        b: 0x3c,
    };

    #[test]
    fn a_neutral_device_gets_exactly_what_was_asked() {
        let rec = Recorder::default();
        let mut d = case("NEUTRAL01", &rec);
        d.set_all(ORANGE).unwrap();
        let sent = rec.sent.lock().unwrap().clone();
        assert!(sent.iter().all(|r| sent_color(r) == [0xf0, 0x8a, 0x3c]));
    }

    #[test]
    fn every_zone_goes_through_its_calibration_and_a_zone_override_wins() {
        let key = "103c:84fd:ZONED001";
        let rec = Recorder::default();
        let mut d = case("ZONED001", &rec);
        let no_blue = Calibration {
            gain: [1.0, 1.0, 0.0],
            ..Default::default()
        };
        let no_red = Calibration {
            gain: [0.0, 1.0, 1.0],
            ..Default::default()
        };
        calibration::global().set_device(
            key,
            Some(DeviceCalibration {
                device: no_blue,
                // Zone 5 is the front bottom fan (index 2).
                zones: [("5".to_string(), no_red)].into(),
                ..Default::default()
            }),
        );
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        d.set_all(white).unwrap();
        let sent: Vec<_> = rec
            .sent
            .lock()
            .unwrap()
            .iter()
            .map(|r| (r[0x36], sent_color(r)))
            .collect();
        assert_eq!(
            sent,
            vec![
                (1, [255, 255, 0]),
                (2, [255, 255, 0]),
                (5, [0, 255, 255]),
                (6, [255, 255, 0]),
                (7, [255, 255, 0]),
            ]
        );
        assert_eq!(
            d.colors(),
            vec![Some(white); 5],
            "it reports what was asked"
        );
        calibration::global().set_device(key, None);
    }

    #[test]
    fn a_palette_change_keeps_the_calibration_and_identify_restores_through_it() {
        let key = "103c:84fd:PALETTE1";
        let rec = Recorder::default();
        let mut d = case("PALETTE1", &rec);
        let dim = DeviceCalibration {
            device: Calibration {
                brightness: 0.25,
                ..Default::default()
            },
            ..Default::default()
        };
        calibration::global().set_device(key, Some(dim.clone()));
        let expect = |c: Rgb| {
            let c = dim.device.apply_rgb(c);
            [c.r, c.g, c.b]
        };
        d.set_all(ORANGE).unwrap();
        let pine = Rgb {
            r: 0x5f,
            g: 0xc4,
            b: 0x8e,
        };
        d.set_zones(&[(0, pine)]).unwrap();
        let sent = rec.sent.lock().unwrap().clone();
        assert_eq!(sent_color(&sent[0]), expect(ORANGE));
        assert_eq!(
            sent_color(&sent[5]),
            expect(pine),
            "the new colour, same calibration"
        );
        assert_eq!(calibration::global().device(key), Some(dim.clone()));
        rec.sent.lock().unwrap().clear();
        d.identify(&mut |_| {}).unwrap();
        let sent = rec.sent.lock().unwrap().clone();
        let back = &sent[sent.len() - 5..];
        assert_eq!(sent_color(&back[0]), expect(pine));
        assert_eq!(sent_color(&back[1]), expect(ORANGE));
        assert_eq!(d.colors()[0], Some(pine));
        calibration::global().set_device(key, None);
    }

    #[test]
    fn the_model_default_applies_until_the_unit_has_its_own() {
        let rec = Recorder::default();
        let inner = crate::pc_lighting::omen_tracer::OmenTracer::new(
            "case".into(),
            vec![ZoneSpec {
                id: 1,
                name: "Diamond".into(),
                position: None,
            }],
            Box::new(rec.clone()),
        )
        .unwrap();
        let model = Calibration {
            gain: [1.0, 1.0, 0.5],
            ..Default::default()
        };
        let key = "103c:84fd:MODEL001";
        let mut d = Calibrated::new(
            Box::new(inner),
            key.into(),
            &[ZoneSpec {
                id: 1,
                name: "Diamond".into(),
                position: None,
            }],
            Some(model.clone()),
        );
        d.set_all(ORANGE).unwrap();
        let first = sent_color(&rec.sent.lock().unwrap()[0]);
        let m = model.apply_rgb(ORANGE);
        assert_eq!(first, [m.r, m.g, m.b]);
        calibration::global().set_device(key, Some(DeviceCalibration::default()));
        d.set_all(ORANGE).unwrap();
        let second = sent_color(&rec.sent.lock().unwrap()[1]);
        assert_eq!(second, [0xf0, 0x8a, 0x3c], "the unit's own (neutral) wins");
        calibration::global().set_device(key, None);
    }
}
