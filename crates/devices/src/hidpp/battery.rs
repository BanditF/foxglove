//! Battery reads over HID++ 2.0 into the generic [`Battery`]: Unified Battery (0x1004, newer
//! devices), Battery Level Status (0x1000, older ones) and Battery Voltage (0x1001, many
//! gaming mice and headsets). [`Hidpp::battery`] asks the newest the device has.
//!
//! Each feature's status reply has the same layout as the battery notification the device
//! sends on its own when the level or charging changes (function 0, software id 0), so the
//! `parse_*` functions serve both: read once on arrival, then follow the notifications.

use super::{Hidpp, HidppError, Result, Transport};
use crate::battery::{li_ion_percent, Battery, BatteryLevel, ChargeState};

/// Battery Level Status: GetBatteryLevelStatus (function 0) -> `[percent, next, status]`.
pub const BATTERY_STATUS: u16 = 0x1000;
/// Battery Voltage: GetBatteryVoltage (function 0) -> `[mV high, mV low, flags]`.
pub const BATTERY_VOLTAGE: u16 = 0x1001;
/// Unified Battery: GetCapabilities (function 0) -> `[levels, flags]`, GetStatus (function 1)
/// -> `[percent, level, charging, external power]`.
pub const UNIFIED_BATTERY: u16 = 0x1004;

/// What a 0x1004 device can report, from GetCapabilities.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnifiedCaps {
    /// Level steps it reports: bit 0 critical, 1 low, 2 good, 3 full.
    pub levels: u8,
    pub rechargeable: bool,
    /// It reports a percentage (state of charge).
    pub percent: bool,
}

impl UnifiedCaps {
    pub fn parse(p: &[u8]) -> Result<Self> {
        let [levels, flags, ..] = *p else {
            return Err(HidppError::Malformed("short 0x1004 capabilities"));
        };
        Ok(Self {
            levels,
            rechargeable: flags & 0x01 != 0,
            percent: flags & 0x02 != 0,
        })
    }
}

/// A 0x1004 GetStatus reply or status event.
pub fn parse_unified(caps: UnifiedCaps, p: &[u8]) -> Result<Battery> {
    let [soc, level_bits, charging, external, ..] = *p else {
        return Err(HidppError::Malformed("short 0x1004 status"));
    };
    let bits = level_bits & caps.levels;
    let level = [
        (0x08, BatteryLevel::Full),
        (0x04, BatteryLevel::Good),
        (0x02, BatteryLevel::Low),
        (0x01, BatteryLevel::Critical),
    ]
    .into_iter()
    .find(|(bit, _)| bits & bit != 0)
    .map(|(_, level)| level);
    let mut battery = Battery::new(match charging {
        0 => ChargeState::Discharging,
        1 => ChargeState::Charging,
        2 => ChargeState::ChargingSlow,
        3 => ChargeState::Full,
        4 => ChargeState::Error,
        _ => ChargeState::Unknown,
    });
    battery.percent = caps.percent.then_some(soc.min(100));
    battery.level = level;
    // 0 none, 1 cable, 2 wireless charger.
    battery.external_power = Some(external != 0);
    Ok(battery)
}

/// A 0x1000 GetBatteryLevelStatus reply or broadcast. Devices report 0 % when they don't know
/// (usually while charging), so that reads as no percentage.
pub fn parse_status(p: &[u8]) -> Result<Battery> {
    let [percent, _next, status, ..] = *p else {
        return Err(HidppError::Malformed("short 0x1000 status"));
    };
    let mut battery = Battery::new(match status {
        0 => ChargeState::Discharging,
        // Recharging, and charging in its final stage.
        1 | 2 => ChargeState::Charging,
        3 => ChargeState::Full,
        4 => ChargeState::ChargingSlow,
        // Invalid battery type, thermal error, other charging error.
        5..=7 => ChargeState::Error,
        _ => ChargeState::Unknown,
    });
    battery.percent = match battery.charge {
        ChargeState::Full => Some(100),
        _ if percent == 0 => None,
        _ => Some(percent.min(100)),
    };
    if battery.charge == ChargeState::Full {
        battery.level = Some(BatteryLevel::Full);
    }
    battery.external_power = match battery.charge {
        ChargeState::Discharging | ChargeState::Unknown => None,
        _ => Some(true),
    };
    Ok(battery)
}

/// A 0x1001 GetBatteryVoltage reply or broadcast. The percentage is Foxglove's estimate from
/// the voltage, and only while discharging (a charger lifts the voltage).
pub fn parse_voltage(p: &[u8]) -> Result<Battery> {
    let [hi, lo, flags, ..] = *p else {
        return Err(HidppError::Malformed("short 0x1001 status"));
    };
    let millivolts = u16::from_be_bytes([hi, lo]);
    let external = flags & 0x80 != 0;
    let charge = if !external {
        ChargeState::Discharging
    } else {
        match flags & 0x07 {
            // Bit 4: trickle charging.
            0 if flags & 0x10 != 0 => ChargeState::ChargingSlow,
            0 => ChargeState::Charging,
            1 => ChargeState::Full,
            2 => ChargeState::NotCharging,
            _ => ChargeState::Unknown,
        }
    };
    let mut battery = Battery::new(charge);
    battery.millivolts = (millivolts != 0).then_some(millivolts);
    battery.external_power = Some(external);
    if flags & 0x20 != 0 {
        battery.level = Some(BatteryLevel::Critical);
    }
    match charge {
        ChargeState::Full => {
            battery.percent = Some(100);
            battery.level = Some(BatteryLevel::Full);
        }
        ChargeState::Discharging if millivolts != 0 => {
            battery.percent = Some(li_ion_percent(millivolts));
            battery.estimated = true;
        }
        _ => {}
    }
    Ok(battery)
}

impl<T: Transport> Hidpp<T> {
    /// The battery, from the newest battery feature the device has; `None` without one.
    /// Read-only: two requests at most after the feature lookups.
    pub fn battery(&mut self) -> Result<Option<Battery>> {
        if let Some(f) = self.feature(UNIFIED_BATTERY)? {
            let caps = UnifiedCaps::parse(&self.request(f.index, 0, &[])?)?;
            return parse_unified(caps, &self.request(f.index, 1, &[])?).map(Some);
        }
        if let Some(f) = self.feature(BATTERY_STATUS)? {
            return parse_status(&self.request(f.index, 0, &[])?).map(Some);
        }
        if let Some(f) = self.feature(BATTERY_VOLTAGE)? {
            return parse_voltage(&self.request(f.index, 0, &[])?).map(Some);
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::Fake;
    use super::super::{Error20, DEVICE_DIRECT};
    use super::*;

    const ASK_1004: &str = "10 FF 00 0C 10 04 00";
    const ASK_1000: &str = "10 FF 00 0C 10 00 00";
    const ASK_1001: &str = "10 FF 00 0C 10 01 00";
    const ABSENT: &str = "11 FF 00 0C 00 00 00";

    #[test]
    fn unified_battery_with_a_percentage() {
        // Index 8. Capabilities: all four levels, rechargeable + state of charge. Status: 73 %,
        // good, discharging, no external power.
        let fake = Fake::default()
            .expect(ASK_1004, &["11 FF 00 0C 08 00 02"])
            .expect("10 FF 08 0C", &["11 FF 08 0C 0F 03"])
            .expect("10 FF 08 1C", &["11 FF 08 1C 49 04 00 00"]);
        let mut dev = Hidpp::new(fake, DEVICE_DIRECT);
        let b = dev.battery().unwrap().unwrap();
        assert_eq!(b.percent, Some(73));
        assert_eq!(b.level, Some(BatteryLevel::Good));
        assert_eq!(b.charge, ChargeState::Discharging);
        assert_eq!(b.external_power, Some(false));
        assert!(!b.estimated);
        assert!(dev.into_transport().done());
    }

    #[test]
    fn unified_battery_levels_only_and_charging() {
        // Behind receiver slot 2: levels critical/low/full only, no percentage; status says
        // full+good bits, charging on a cable. Only supported bits count: full.
        let fake = Fake::default()
            .expect("10 02 00 0C 10 04 00", &["10 02 00 0C 05 00 00"])
            .expect("10 02 05 0C", &["10 02 05 0C 0B 01 00"])
            .expect("10 02 05 1C", &["11 02 05 1C 00 0C 01 01"]);
        let b = Hidpp::new(fake, 2).battery().unwrap().unwrap();
        assert_eq!(b.percent, None);
        assert_eq!(b.level, Some(BatteryLevel::Full));
        assert_eq!(b.charge, ChargeState::Charging);
        assert_eq!(b.external_power, Some(true));
        assert_eq!(b.display_percent(), Some(100));
    }

    #[test]
    fn battery_level_status_fallback() {
        // No 0x1004; 0x1000 at index 4: 50 %, next step 20 %, discharging.
        let fake = Fake::default()
            .expect(ASK_1004, &[ABSENT])
            .expect(ASK_1000, &["11 FF 00 0C 04 00 00"])
            .expect("10 FF 04 0C", &["10 FF 04 0C 32 14 00"]);
        let b = Hidpp::new(fake, DEVICE_DIRECT).battery().unwrap().unwrap();
        assert_eq!(b.percent, Some(50));
        assert_eq!(b.charge, ChargeState::Discharging);
        assert_eq!(b.external_power, None);
    }

    #[test]
    fn battery_level_status_states() {
        // Charging reports 0 %: unknown, not empty.
        let b = parse_status(&[0, 0, 1]).unwrap();
        assert_eq!((b.percent, b.charge), (None, ChargeState::Charging));
        let b = parse_status(&[0, 0, 3]).unwrap();
        assert_eq!((b.percent, b.charge), (Some(100), ChargeState::Full));
        assert_eq!(
            parse_status(&[90, 50, 4]).unwrap().charge,
            ChargeState::ChargingSlow
        );
        assert_eq!(
            parse_status(&[90, 50, 6]).unwrap().charge,
            ChargeState::Error
        );
        assert!(parse_status(&[1, 2]).is_err());
    }

    #[test]
    fn battery_voltage_fallback_estimates_while_discharging() {
        // 0x1001 at index 6: 3850 mV (0x0F0A), discharging.
        let fake = Fake::default()
            .expect(ASK_1004, &[ABSENT])
            .expect(ASK_1000, &[ABSENT])
            .expect(ASK_1001, &["11 FF 00 0C 06 00 02"])
            .expect("10 FF 06 0C", &["11 FF 06 0C 0F 0A 00"]);
        let b = Hidpp::new(fake, DEVICE_DIRECT).battery().unwrap().unwrap();
        assert_eq!(b.millivolts, Some(3850));
        assert_eq!(b.percent, Some(57));
        assert!(b.estimated);
        assert_eq!(b.external_power, Some(false));
    }

    #[test]
    fn battery_voltage_flags() {
        // On a cable, charging; 4100 mV: no estimate while charging.
        let b = parse_voltage(&[0x10, 0x04, 0x80]).unwrap();
        assert_eq!(b.charge, ChargeState::Charging);
        assert_eq!(b.percent, None);
        assert_eq!(b.millivolts, Some(4100));
        // Trickle charging.
        let b = parse_voltage(&[0x10, 0x04, 0x90]).unwrap();
        assert_eq!(b.charge, ChargeState::ChargingSlow);
        // Charge complete.
        let b = parse_voltage(&[0x10, 0x68, 0x81]).unwrap();
        assert_eq!((b.charge, b.percent), (ChargeState::Full, Some(100)));
        // Discharging, critical flag.
        let b = parse_voltage(&[0x0D, 0xAC, 0x20]).unwrap();
        assert_eq!(b.level, Some(BatteryLevel::Critical));
        assert_eq!(b.percent, Some(0));
    }

    #[test]
    fn no_battery_feature() {
        let fake = Fake::default()
            .expect(ASK_1004, &[ABSENT])
            .expect(ASK_1000, &[ABSENT])
            .expect(ASK_1001, &[ABSENT]);
        assert_eq!(Hidpp::new(fake, DEVICE_DIRECT).battery(), Ok(None));
    }

    #[test]
    fn a_failing_read_is_an_error_not_a_level() {
        let fake = Fake::default()
            .expect(ASK_1004, &["11 FF 00 0C 03 00 00"])
            .expect("10 FF 03 0C", &["11 FF FF 03 0C 04"]);
        assert_eq!(
            Hidpp::new(fake, DEVICE_DIRECT).battery(),
            Err(HidppError::Feature(Error20::HardwareError))
        );
    }
}
