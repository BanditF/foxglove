//! The thermal zone counter's conversions (no PDH, no hardware).

use foxglove_sensors::thermal::{celsius, reading_of, zone_name, zones_from};
use foxglove_sensors::{Metric, Zone};

#[test]
fn kelvin_becomes_whole_celsius_and_nonsense_is_dropped() {
    assert_eq!(celsius(301.0), Some(28));
    assert_eq!(celsius(311.0), Some(38));
    assert_eq!(celsius(273.15), Some(0));
    for bad in [0.0, -5.0, f64::NAN, f64::INFINITY, 1.0, 900.0] {
        assert_eq!(celsius(bad), None, "{bad}");
    }
}

#[test]
fn zone_names_drop_the_acpi_prefix() {
    assert_eq!(zone_name("\\_TZ.TZ00"), "TZ00");
    assert_eq!(zone_name("\\_tz.hptz"), "HPTZ");
    assert_eq!(zone_name("CPUZ"), "CPUZ");
}

#[test]
fn the_hottest_zone_is_the_value_and_a_bad_zone_is_left_out() {
    let zones = zones_from(&[
        ("\\_TZ.TZ00".into(), 301.0),
        ("\\_TZ.HPTZ".into(), 311.0),
        ("\\_TZ.BAD".into(), 0.0),
    ]);
    assert_eq!(
        zones,
        vec![
            Zone {
                name: "HPTZ".into(),
                celsius: 38
            },
            Zone {
                name: "TZ00".into(),
                celsius: 28
            },
        ]
    );
    let r = reading_of(zones);
    let values: Vec<_> = r.iter().collect();
    assert_eq!(values, vec![(Metric::SysTemp, 38)]);
    assert_eq!(r.zones().len(), 2);
}

#[test]
fn no_zone_means_no_value_never_zero() {
    let r = reading_of(zones_from(&[]));
    assert_eq!(r.iter().count(), 0);
    assert!(r.zones().is_empty());
    let r = reading_of(zones_from(&[("\\_TZ.TZ00".into(), 0.0)]));
    assert_eq!(r.iter().count(), 0);
}

#[test]
fn the_fact_is_a_system_temperature_not_a_cpu_one() {
    assert_eq!(Metric::SysTemp.key(), "sensor.system.temp_c");
    assert_eq!(Metric::SysTemp.unit(), "C");
}
