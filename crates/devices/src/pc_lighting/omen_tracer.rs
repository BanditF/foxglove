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
//!
//! The board's own effects (it plays them by itself, also with Foxglove closed) are the same
//! report with other values, one report per colour of the effect and zone:
//!
//! | Byte | Value |
//! |---|---|
//! | 0x03 | the effect: 0x06 breathing, 0x07 colour cycle, 0x08 blinking, 0x09 wave, 0x0A radial |
//! | 0x04 | how many colours the effect has (1 to 6; wave takes 6) |
//! | 0x05 | which of them this report carries, from 1 |
//! | 0x31 | 0x0A |
//! | 0x38 | 0x00, the colours in the reports (not one of the board's themes) |
//! | 0x39 | the speed: 0x01 slow, 0x02 medium, 0x03 fast |
//!
//! Foxglove starts them at the slow speed only and never starts blinking: the board's speeds
//! aren't timed yet, and no light Foxglove starts may flash faster than 3 times a second.
//! Not yet checked on a board (the sitting queue, #90).

use super::{HidWriter, LightingDevice, OnboardEffect, ZoneSpec};
use crate::{DeviceError, Result, Rgb};

pub const REPORT_LEN: usize = 59;
/// The highest zone number whose colour bytes fit before the brightness byte.
pub const MAX_ZONE: u8 = 13;
const VERSION: u8 = 0x12;
const MODE_STATIC: u8 = 0x01;
const COLORS: usize = 0x08;
const BRIGHTNESS: usize = 0x30;
const BRIGHTNESS_FULL: u8 = 0x64;
const UNKNOWN_31: usize = 0x31;
const ZONE: usize = 0x36;
const PROFILE: usize = 0x37;
const PROFILE_POWERED_ON: u8 = 0x01;
const TYPE_EFFECT: u8 = 0x0A;
const THEME: usize = 0x38;
const THEME_CUSTOM: u8 = 0x00;
const SPEED: usize = 0x39;
const SPEED_SLOW: u8 = 0x01;
/// The most colours an effect takes.
pub const MAX_EFFECT_COLORS: usize = 6;

/// An effect of the board's own that Foxglove starts.
struct Mode {
    /// Its name, as catalogs list it in `effects`.
    name: &'static str,
    mode: u8,
    /// It plays each zone's own colour (else a set of colours of its own).
    own_colours: bool,
    /// The number of colours it must be given; `None`: 1 to [`MAX_EFFECT_COLORS`].
    colors: Option<usize>,
}

/// The effects Foxglove starts. Blinking (0x08) isn't one: see the module's note.
const MODES: [Mode; 4] = [
    Mode {
        name: "breathing",
        mode: 0x06,
        own_colours: true,
        colors: None,
    },
    Mode {
        name: "colour cycle",
        mode: 0x07,
        own_colours: false,
        colors: None,
    },
    Mode {
        name: "wave",
        mode: 0x09,
        own_colours: false,
        colors: Some(MAX_EFFECT_COLORS),
    },
    Mode {
        name: "radial",
        mode: 0x0A,
        own_colours: false,
        colors: None,
    },
];

fn mode(name: &str) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.name == name)
}

/// The reports that start one of the board's own effects on one zone (1-based), at the slow
/// speed: one per colour. `None` for an effect Foxglove doesn't start, a zone out of range or
/// a number of colours the effect can't take.
pub fn effect_reports(zone: u8, effect: &str, colors: &[Rgb]) -> Option<Vec<[u8; REPORT_LEN]>> {
    let m = mode(effect)?;
    let n = colors.len();
    if !zone_ok(zone) || n == 0 || n > MAX_EFFECT_COLORS || m.colors.is_some_and(|c| c != n) {
        return None;
    }
    let at = COLORS + 3 * usize::from(zone - 1);
    let reports = colors
        .iter()
        .zip(1u8..)
        .map(|(color, index)| {
            let mut buf = [0u8; REPORT_LEN];
            buf[0x02] = VERSION;
            buf[0x03] = m.mode;
            buf[0x04] = n as u8;
            buf[0x05] = index;
            buf[at..at + 3].copy_from_slice(&[color.r, color.g, color.b]);
            buf[BRIGHTNESS] = BRIGHTNESS_FULL;
            buf[UNKNOWN_31] = TYPE_EFFECT;
            buf[ZONE] = zone;
            buf[PROFILE] = PROFILE_POWERED_ON;
            buf[THEME] = THEME_CUSTOM;
            buf[SPEED] = SPEED_SLOW;
            buf
        })
        .collect();
    Some(reports)
}

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
    /// The board's own effects this board has (its catalog's `effects`) that Foxglove starts.
    effects: Vec<OnboardEffect>,
    /// The effect Foxglove last started, until the next static colour.
    effect: Option<String>,
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
            effects: Vec::new(),
            effect: None,
        })
    }

    /// The board's own effects, by the names its catalog entry lists: the ones Foxglove
    /// starts are offered, in that order.
    pub fn with_effects(mut self, names: &[String]) -> Self {
        self.effects = names
            .iter()
            .filter_map(|n| mode(n))
            .map(|m| OnboardEffect {
                name: m.name.to_string(),
                own_colours: m.own_colours,
            })
            .collect();
        self
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
        // A static colour ends the board's own effect on that zone.
        self.effect = None;
        for &(index, color) in colors {
            self.colors[index] = Some(color);
        }
        Ok(())
    }

    fn effects(&self) -> Vec<OnboardEffect> {
        self.effects.clone()
    }

    fn effect(&self) -> Option<String> {
        self.effect.clone()
    }

    fn set_effect(&mut self, name: &str, colors: &[(usize, Vec<Rgb>)]) -> Result<()> {
        if !self.effects.iter().any(|e| e.name == name) {
            return Err(DeviceError::Unsupported);
        }
        let mut reports = Vec::new();
        for (index, zone_colors) in colors {
            let zone = self.zones.get(*index).ok_or(DeviceError::Unsupported)?;
            let zone_reports =
                effect_reports(zone.id, name, zone_colors).ok_or(DeviceError::Unsupported)?;
            reports.extend(zone_reports.iter().map(|r| r.to_vec()));
        }
        if reports.is_empty() {
            return Ok(());
        }
        self.writer.send(&reports)?;
        self.effect = Some(name.to_string());
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

    const SIX: [Rgb; 6] = crate::pc_lighting::SPECTRUM;

    /// Breathing on the top fan (zone 7) in teal, byte for byte, built independently here.
    #[test]
    fn breathing_is_one_slow_report_per_zone_in_its_own_colour() {
        let rec = Recorder::default();
        let mut dev = case(&rec).with_effects(&["breathing".into()]);
        dev.set_effect("breathing", &[(4, vec![TEAL])]).unwrap();
        let mut want = vec![0u8; 59];
        want[0x02] = 0x12;
        want[0x03] = 0x06;
        want[0x04] = 1;
        want[0x05] = 1;
        want[0x1A..0x1D].copy_from_slice(&[0x00, 0x80, 0x80]);
        want[0x30] = 0x64;
        want[0x31] = 0x0A;
        want[0x36] = 7;
        want[0x37] = 0x01;
        want[0x38] = 0x00;
        want[0x39] = 0x01;
        assert_eq!(rec.sent.lock().unwrap().clone(), vec![want]);
        assert_eq!(dev.effect().as_deref(), Some("breathing"));
        // The zone colours Foxglove knows are untouched; a static colour ends the effect.
        assert_eq!(dev.colors(), vec![None; 5]);
        dev.set_zones(&[(0, TEAL)]).unwrap();
        assert_eq!(dev.effect(), None);
    }

    #[test]
    fn a_many_colour_effect_sends_each_colour_numbered_from_one() {
        for (name, mode) in [("colour cycle", 0x07), ("wave", 0x09), ("radial", 0x0A)] {
            let reports = effect_reports(2, name, &SIX).unwrap();
            assert_eq!(reports.len(), 6, "{name}");
            for (i, r) in reports.iter().enumerate() {
                assert_eq!(
                    (r[0x03], r[0x04], r[0x05]),
                    (mode, 6, i as u8 + 1),
                    "{name}"
                );
                assert_eq!(&r[0x0B..0x0E], &[SIX[i].r, SIX[i].g, SIX[i].b], "zone 2");
                assert_eq!(
                    (r[0x31], r[0x36], r[0x38], r[0x39]),
                    (0x0A, 2, 0, 1),
                    "{name}"
                );
                assert_eq!((r[0x02], r[0x30], r[0x37]), (0x12, 0x64, 0x01));
            }
        }
    }

    #[test]
    fn only_slow_known_effects_with_colours_that_fit_are_built() {
        assert!(
            effect_reports(1, "blinking", &[TEAL]).is_none(),
            "never started"
        );
        assert!(effect_reports(1, "strobe", &[TEAL]).is_none());
        assert!(effect_reports(1, "breathing", &[]).is_none());
        assert!(effect_reports(1, "breathing", &[TEAL; 7]).is_none());
        assert!(
            effect_reports(1, "wave", &[TEAL; 5]).is_none(),
            "wave takes six"
        );
        assert!(effect_reports(14, "breathing", &[TEAL]).is_none());
        for m in &MODES {
            let n = m.colors.unwrap_or(1);
            for r in effect_reports(1, m.name, &vec![TEAL; n]).unwrap() {
                assert_eq!(r[SPEED], SPEED_SLOW, "{}", m.name);
            }
        }
    }

    #[test]
    fn a_board_offers_its_catalogs_effects_that_foxglove_starts() {
        let rec = Recorder::default();
        let names = Catalog::bundled()
            .find(0x103c, 0x84fd)
            .unwrap()
            .effects
            .clone();
        let mut dev = case(&rec).with_effects(&names);
        let offered: Vec<_> = dev.effects().into_iter().map(|e| e.name).collect();
        assert_eq!(offered, ["breathing", "colour cycle", "wave", "radial"]);
        assert!(dev.effects()[0].own_colours && !dev.effects()[1].own_colours);
        // Not offered, not sent: blinking, an unknown name, a zone the board lacks.
        assert_eq!(
            dev.set_effect("blinking", &[(0, vec![TEAL])]),
            Err(DeviceError::Unsupported)
        );
        assert_eq!(
            dev.set_effect("breathing", &[(9, vec![TEAL])]),
            Err(DeviceError::Unsupported)
        );
        assert!(rec.sent.lock().unwrap().is_empty());
        // A board whose catalog lists none offers none.
        let mut plain = case(&rec);
        assert!(plain.effects().is_empty());
        assert_eq!(
            plain.set_effect("breathing", &[(0, vec![TEAL])]),
            Err(DeviceError::Unsupported)
        );
    }

    #[test]
    fn a_failed_effect_write_keeps_what_was_running() {
        let rec = Recorder {
            fail: true,
            ..Default::default()
        };
        let mut dev = case(&rec).with_effects(&["breathing".into()]);
        assert_eq!(
            dev.set_effect("breathing", &[(0, vec![TEAL])]),
            Err(DeviceError::Disconnected)
        );
        assert_eq!(dev.effect(), None);
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
