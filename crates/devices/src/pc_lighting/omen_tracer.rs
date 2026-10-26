//! HP OMEN case lighting board ("TracerLED", USB HID 103C:84FD), as in the OMEN 45L (#423).
//!
//! Written from protocol facts only (the report layout in #423), not from any other
//! project's code. Each static colour is one 59-byte output report for one zone:
//!
//! | Byte | Value |
//! |---|---|
//! | 0x00 | report id 0 |
//! | 0x02 | 0x12, protocol version |
//! | 0x03 | 0x01, static mode |
//! | 0x04, 0x05 | 1, 1 |
//! | 0x08 + 3·(zone − 1) | red, green, blue |
//! | 0x30 | 0x64, brightness 100 |
//! | 0x31 | 0x02 |
//! | 0x36 | the zone number |
//! | 0x37 | 0x01, the powered-on profile |
//!
//! The board can't be read back, so the driver keeps the colours it last set.

use super::{HidWriter, LightingDevice, ZoneSpec};
use crate::{DeviceError, Result, Rgb};

pub const REPORT_LEN: usize = 59;
const VERSION: u8 = 0x12;
const MODE_STATIC: u8 = 0x01;
const COLORS: usize = 0x08;
const BRIGHTNESS: usize = 0x30;
const BRIGHTNESS_FULL: u8 = 0x64;
const UNKNOWN_31: usize = 0x31;
const ZONE: usize = 0x36;
const PROFILE: usize = 0x37;
const PROFILE_POWERED_ON: u8 = 0x01;

/// Whether a zone's colour bytes fit before the brightness byte.
pub fn zone_ok(zone: u8) -> bool {
    zone >= 1 && COLORS + 3 * usize::from(zone) <= BRIGHTNESS
}

/// The static-colour report for one zone (1-based, as the board numbers them).
pub fn static_report(zone: u8, color: Rgb) -> Option<[u8; REPORT_LEN]> {
    if !zone_ok(zone) {
        return None;
    }
    let mut buf = [0u8; REPORT_LEN];
    buf[0x02] = VERSION;
    buf[0x03] = MODE_STATIC;
    buf[0x04] = 1;
    buf[0x05] = 1;
    let at = COLORS + 3 * usize::from(zone - 1);
    buf[at..at + 3].copy_from_slice(&[color.r, color.g, color.b]);
    buf[BRIGHTNESS] = BRIGHTNESS_FULL;
    buf[UNKNOWN_31] = 0x02;
    buf[ZONE] = zone;
    buf[PROFILE] = PROFILE_POWERED_ON;
    Some(buf)
}

/// The board as a [`LightingDevice`].
pub struct OmenTracer {
    name: String,
    zones: Vec<ZoneSpec>,
    colors: Vec<Option<Rgb>>,
    writer: Box<dyn HidWriter>,
}

impl OmenTracer {
    pub fn new(name: String, zones: Vec<ZoneSpec>, writer: Box<dyn HidWriter>) -> Result<Self> {
        if zones.is_empty() || zones.iter().any(|z| !zone_ok(z.id)) {
            return Err(DeviceError::Backend(format!("{name}: bad zone map")));
        }
        Ok(Self {
            name,
            colors: vec![None; zones.len()],
            zones,
            writer,
        })
    }
}

impl LightingDevice for OmenTracer {
    fn name(&self) -> &str {
        &self.name
    }

    fn zones(&self) -> Vec<String> {
        self.zones.iter().map(|z| z.name.clone()).collect()
    }

    fn colors(&self) -> Vec<Option<Rgb>> {
        self.colors.clone()
    }

    fn set_zones(&mut self, colors: &[(usize, Rgb)]) -> Result<()> {
        let mut reports = Vec::with_capacity(colors.len());
        for &(index, color) in colors {
            let zone = self.zones.get(index).ok_or(DeviceError::Unsupported)?;
            let report = static_report(zone.id, color).ok_or(DeviceError::Unsupported)?;
            reports.push(report.to_vec());
        }
        if reports.is_empty() {
            return Ok(());
        }
        self.writer.send(&reports)?;
        for &(index, color) in colors {
            self.colors[index] = Some(color);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::pc_lighting::{Catalog, BLACK, IDENTIFY_STEPS, WHITE};
    use std::sync::{Arc, Mutex};

    /// A mocked HID writer: keeps every report, or fails every send.
    #[derive(Clone, Default)]
    pub(crate) struct Recorder {
        pub sent: Arc<Mutex<Vec<Vec<u8>>>>,
        pub fail: bool,
    }

    impl HidWriter for Recorder {
        fn send(&mut self, reports: &[Vec<u8>]) -> Result<()> {
            if self.fail {
                return Err(DeviceError::Disconnected);
            }
            self.sent.lock().unwrap().extend(reports.iter().cloned());
            Ok(())
        }
    }

    fn case(rec: &Recorder) -> OmenTracer {
        let zones = Catalog::bundled()
            .find(0x103c, 0x84fd)
            .unwrap()
            .zones
            .clone();
        OmenTracer::new("case".into(), zones, Box::new(rec.clone())).unwrap()
    }

    /// The report the PowerShell test wrote, byte for byte, built independently here.
    fn expected(zone: u8, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut buf = vec![0u8; 59];
        buf[2] = 0x12;
        buf[3] = 0x01;
        buf[4] = 1;
        buf[5] = 1;
        let i = usize::from(zone - 1);
        buf[0x08 + i * 3] = r;
        buf[0x09 + i * 3] = g;
        buf[0x0A + i * 3] = b;
        buf[0x30] = 0x64;
        buf[0x31] = 0x02;
        buf[0x36] = zone;
        buf[0x37] = 0x01;
        buf
    }

    const TEAL: Rgb = Rgb {
        r: 0x00,
        g: 0x80,
        b: 0x80,
    };

    #[test]
    fn each_45l_zone_gets_its_exact_report() {
        let rec = Recorder::default();
        let mut dev = case(&rec);
        let colors: Vec<_> = (0..5)
            .map(|i| {
                (
                    i,
                    Rgb {
                        r: 10 + i as u8,
                        g: 20,
                        b: 30,
                    },
                )
            })
            .collect();
        dev.set_zones(&colors).unwrap();
        let sent = rec.sent.lock().unwrap().clone();
        let want: Vec<_> = [1u8, 2, 5, 6, 7]
            .iter()
            .zip(10u8..)
            .map(|(&zone, r)| expected(zone, r, 20, 30))
            .collect();
        assert_eq!(sent, want);
        assert!(sent.iter().all(|r| r.len() == 59 && r[0] == 0));
    }

    #[test]
    fn the_zone_byte_and_colour_slot_move_together() {
        let r = static_report(7, TEAL).unwrap();
        assert_eq!(r[0x36], 7);
        assert_eq!(&r[0x1A..0x1D], &[0x00, 0x80, 0x80]);
        assert!(r[0x08..0x1A].iter().all(|&b| b == 0), "other zones stay 0");
        assert!(static_report(0, TEAL).is_none());
        assert!(static_report(13, TEAL).is_some(), "0x2c..0x2f");
        assert!(static_report(14, TEAL).is_none(), "would hit 0x30");
    }

    #[test]
    fn off_writes_black_and_remembers_it() {
        let rec = Recorder::default();
        let mut dev = case(&rec);
        assert_eq!(dev.colors(), vec![None; 5]);
        dev.off().unwrap();
        let sent = rec.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 5);
        assert_eq!(sent[4], expected(7, 0, 0, 0));
        assert_eq!(dev.colors(), vec![Some(BLACK); 5]);
    }

    #[test]
    fn identify_blinks_then_restores_what_it_had() {
        let rec = Recorder::default();
        let mut dev = case(&rec);
        dev.set_zones(&[(0, TEAL)]).unwrap();
        rec.sent.lock().unwrap().clear();
        let mut waits = 0;
        dev.identify(&mut |_| waits += 1).unwrap();
        assert_eq!(waits, IDENTIFY_STEPS);
        let sent = rec.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 5 * IDENTIFY_STEPS + 5);
        assert_eq!(sent[0], expected(1, 255, 255, 255), "first step: white");
        assert_eq!(sent[5], expected(1, 0, 0, 0), "second step: dark");
        let restore = &sent[5 * IDENTIFY_STEPS..];
        assert_eq!(restore[0], expected(1, 0x00, 0x80, 0x80), "teal is back");
        assert_eq!(
            restore[1],
            expected(2, 255, 255, 255),
            "never set: ends white"
        );
        assert_eq!(dev.colors()[0], Some(TEAL));
        assert_eq!(dev.colors()[1], Some(WHITE));
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let rec = Recorder {
            fail: true,
            ..Default::default()
        };
        let mut dev = case(&rec);
        assert_eq!(dev.set_all(TEAL), Err(DeviceError::Disconnected));
        assert_eq!(dev.colors(), vec![None; 5]);
        assert_eq!(dev.set_zones(&[(9, TEAL)]), Err(DeviceError::Unsupported));
    }
}
