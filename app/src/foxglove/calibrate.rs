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
/// writes a colour. A section this Foxglove can't read (a newer version, an unknown field) is
/// logged and left as it is: the devices run uncalibrated rather than half calibrated.
pub fn load() {
    load_into(calibration::global());
}

fn load_into(registry: &calibration::Registry) {
    let Some(section) = super::store::try_load::<toml::Table>(SECTION) else {
        return;
    };
    match read(section) {
        Ok(all) => registry.replace(all),
        Err(e) => eprintln!("Foxglove: [{SECTION}]: {e}; colours go out uncalibrated"),
    }
}

/// The section's calibrations, refusing a version newer than this Foxglove reads.
fn read(section: toml::Table) -> Result<Calibrations, String> {
    let version = section
        .get("version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(i64::from(calibration::VERSION));
    if version > i64::from(calibration::VERSION) {
        return Err(format!(
            "version {version} is newer than this Foxglove reads ({})",
            calibration::VERSION
        ));
    }
    toml::Value::Table(section)
        .try_into::<Calibrations>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_devices::calibration::{Calibration, DeviceCalibration};

    #[test]
    fn a_section_from_a_newer_foxglove_or_with_unknown_fields_is_not_read() {
        let parse = |text: &str| read(toml::from_str::<toml::Table>(text).unwrap());
        assert!(parse("version = 2\n").unwrap_err().contains("version 2"));
        let unknown = "version = 1\n[devices.\"1d50:615e\".device]\nsparkle = 1.0\n";
        assert!(parse(unknown).is_err());
        let ok = "version = 1\n[devices.\"1d50:615e\".device]\ngain = [1.0, 0.9, 0.8]\n";
        assert_eq!(
            parse(ok).unwrap().devices["1d50:615e"].device.gain,
            [1.0, 0.9, 0.8]
        );
        assert!(parse("").unwrap().devices.is_empty());
    }

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
        // Read back from the file, as the next start does (into a registry of its own, so
        // the process's, which other tests use, isn't replaced).
        let registry = calibration::Registry::default();
        load_into(&registry);
        assert_eq!(registry.device(key), Some(cal));
        let saved: Calibrations = super::super::store::try_load(SECTION).unwrap();
        assert_eq!(saved.version, calibration::VERSION);
    }
}
