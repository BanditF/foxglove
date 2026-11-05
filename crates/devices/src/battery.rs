//! One battery reading, whatever reported it (#505): a percentage when the device knows one, a
//! coarse level when that's all it has, whether it's charging, and the cell voltage when the
//! device reports that instead. Protocol drivers (`hidpp`, later others) fill it in; the app
//! shows it next to the halves' and the Bluetooth devices' levels.

/// A coarse level, for devices that report steps instead of a percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BatteryLevel {
    Critical,
    Low,
    Good,
    Full,
}

/// What the battery is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeState {
    Discharging,
    Charging,
    /// Charging below its normal rate (a weak charger or cable).
    ChargingSlow,
    Full,
    /// On external power but not charging (paused, or done for another reason).
    NotCharging,
    /// The device reports a charging fault (wrong cell, too hot, other).
    Error,
    Unknown,
}

impl ChargeState {
    /// Taking charge now.
    pub fn charging(self) -> bool {
        matches!(self, ChargeState::Charging | ChargeState::ChargingSlow)
    }
}

/// One battery reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Battery {
    /// 0-100, when the device reports one (or [`Battery::estimated`] from the voltage).
    pub percent: Option<u8>,
    /// `percent` is Foxglove's estimate from the voltage, not the device's own figure.
    pub estimated: bool,
    pub level: Option<BatteryLevel>,
    pub charge: ChargeState,
    /// On a cable or charger, when the device says.
    pub external_power: Option<bool>,
    /// Cell voltage, for devices that report it.
    pub millivolts: Option<u16>,
}

impl Battery {
    /// A reading with only a charge state; the rest unknown.
    pub fn new(charge: ChargeState) -> Self {
        Self {
            percent: None,
            estimated: false,
            level: None,
            charge,
            external_power: None,
            millivolts: None,
        }
    }

    /// The percentage to show: the device's, else one from its level step.
    pub fn display_percent(&self) -> Option<u8> {
        self.percent.or(match self.level? {
            BatteryLevel::Critical => 5,
            BatteryLevel::Low => 20,
            BatteryLevel::Good => 60,
            BatteryLevel::Full => 100,
        }
        .into())
    }
}

/// A rough state of charge for a single lithium-ion cell from its voltage under light load:
/// linear between a few textbook points (3.5 V empty, 4.2 V full). Good to a band, not a
/// figure; [`Battery::estimated`] marks readings that use it.
pub fn li_ion_percent(millivolts: u16) -> u8 {
    const POINTS: [(u16, u8); 8] = [
        (3500, 0),
        (3600, 8),
        (3700, 25),
        (3800, 48),
        (3900, 65),
        (4000, 80),
        (4100, 92),
        (4200, 100),
    ];
    let (lo_mv, _) = POINTS[0];
    if millivolts <= lo_mv {
        return 0;
    }
    for pair in POINTS.windows(2) {
        let ((v0, p0), (v1, p1)) = (pair[0], pair[1]);
        if millivolts <= v1 {
            let span = u32::from(v1 - v0);
            let into = u32::from(millivolts - v0);
            let rise = u32::from(p1 - p0);
            return p0 + ((into * rise + span / 2) / span) as u8;
        }
    }
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voltage_estimate_is_monotonic_and_clamped() {
        assert_eq!(li_ion_percent(3000), 0);
        assert_eq!(li_ion_percent(3500), 0);
        assert_eq!(li_ion_percent(3800), 48);
        assert_eq!(li_ion_percent(3850), 57);
        assert_eq!(li_ion_percent(4200), 100);
        assert_eq!(li_ion_percent(4400), 100);
        let mut last = 0;
        for mv in (3400..4300).step_by(10) {
            let p = li_ion_percent(mv);
            assert!(p >= last, "{mv} mV gave {p} after {last}");
            last = p;
        }
    }

    #[test]
    fn display_percent_falls_back_to_the_level() {
        let mut b = Battery::new(ChargeState::Discharging);
        assert_eq!(b.display_percent(), None);
        b.level = Some(BatteryLevel::Low);
        assert_eq!(b.display_percent(), Some(20));
        b.percent = Some(42);
        assert_eq!(b.display_percent(), Some(42));
        assert!(ChargeState::ChargingSlow.charging());
        assert!(!ChargeState::Full.charging());
    }
}
