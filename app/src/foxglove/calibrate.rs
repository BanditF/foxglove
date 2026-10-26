//! Colour calibration's settings: every device's calibration in the store's `[calibration]`
//! section (versioned), loaded into the device layer's registry at startup so every driver
//! applies it (`foxglove_devices::calibration`).
//!
//! Calibration belongs to the device, never to a colour or a profile: it isn't in
//! `store::PROFILED`, and changing a palette or a profile leaves it alone.

use foxglove_devices::calibration::{self, Calibrations};

/// The store section.
pub const SECTION: &str = "calibration";

/// Reads the saved calibrations into the registry. Call once at startup, before anything
/// writes a colour.
pub fn load() {
    if let Some(all) = super::store::try_load::<Calibrations>(SECTION) {
        calibration::global().replace(all);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_devices::calibration::{Calibration, DeviceCalibration};

    #[test]
    fn calibration_is_not_part_of_a_profile() {
        assert!(!super::super::store::PROFILED.contains(&SECTION));
    }

    #[test]
    fn saved_calibration_survives_a_palette_change() {
        let _dir = super::super::store::test_config_dir();
        let key = "calibrate-test:0001";
        let cal = DeviceCalibration {
            name: "Test strip".into(),
            device: Calibration {
                gain: [1.0, 0.8, 0.6],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut all = Calibrations::default();
        all.devices.insert(key.into(), cal.clone());
        super::super::store::save(SECTION, &all).unwrap();
        // A new palette: the lights' colours are saved in their own section.
        let mut colours = super::super::pc_lights::Settings::default();
        colours.devices.entry(key.into()).or_default().colors =
            [("1".to_string(), "#5fc48e".to_string())].into();
        super::super::store::save("pc_lighting", &colours).unwrap();
        // Read back from the file, as the next start does.
        calibration::global().set_device(key, None);
        load();
        assert_eq!(calibration::global().device(key), Some(cal));
        let saved: Calibrations = super::super::store::try_load(SECTION).unwrap();
        assert_eq!(saved.version, calibration::VERSION);
    }
}
