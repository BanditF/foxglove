//! How often each kind of light takes a scene's frames (#486), as data: the bundled
//! `crates/devices/light-rates.toml`, which `foxglove-lab rates` measures and rewrites. Scenes
//! read an interval per output from it; nothing here knows a vendor (a model's own entry is a
//! quirk in the data, keyed by VID/PID).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A Foxglove link keyboard's frames (SET_FRAME, D7 0C).
pub const KEYBOARD: &str = "keyboard";
/// Any Windows Dynamic Lighting (LampArray) device.
pub const LAMP_ARRAY: &str = "lamp-array";

/// The fastest interval a measurement recommends: scenes cap every device at 30 fps.
pub const FLOOR_MS: u32 = 34;
/// A recommendation's headroom over what the device took back to back: a scene's frames share
/// the link with everything else (keys, the panel, the vendor's own service).
pub const HEADROOM: f64 = 1.5;
/// More failed writes than this share, and the burst doesn't give a rate.
pub const MAX_ERROR_SHARE: f64 = 0.05;

/// What one bounded burst measured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    /// Writes accepted per second, back to back with one in flight.
    pub rate_hz: f64,
    /// Write latency (the call until the device or its service took it), median and 95th.
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub writes: u32,
    pub errors: u32,
    /// The lab process's CPU time over the burst, when the platform says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_ms: Option<u64>,
    pub seconds: f64,
    /// When (UTC, RFC 3339) and on which build of the device's firmware or driver, if known.
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl Measured {
    /// The interval scenes should use: the slower of the p95 latency and the sustained rate's
    /// period, with [`HEADROOM`], never under [`FLOOR_MS`]. `None` when too many writes failed
    /// (or none were accepted) to trust it.
    pub fn interval_ms(&self) -> Option<u32> {
        let ok = self.writes.saturating_sub(self.errors);
        if ok == 0
            || self.rate_hz <= 0.0
            || f64::from(self.errors) > f64::from(self.writes) * MAX_ERROR_SHARE
        {
            return None;
        }
        let period = 1000.0 / self.rate_hz;
        let ms = (period.max(self.p95_ms) * HEADROOM).ceil();
        Some((ms.min(f64::from(u32::MAX)) as u32).max(FLOOR_MS))
    }
}

/// One class of light (or one model of it) and its frame interval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightRate {
    /// [`KEYBOARD`], [`LAMP_ARRAY`] or a catalog protocol's id (`Protocol::id`).
    pub class: String,
    /// `vvvv:pppp` (lowercase hex): this model only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// `usb` or `ble`: the bus it was measured on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    pub interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<Measured>,
}

impl LightRate {
    /// The same light: class, model and bus.
    fn same(&self, other: &LightRate) -> bool {
        self.class == other.class
            && self.device == other.device
            && self.transport == other.transport
    }
}

/// Every light's rate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rates {
    #[serde(default, rename = "light")]
    pub lights: Vec<LightRate>,
}

/// The rates that ship with Foxglove.
pub const BUNDLED: &str = include_str!("../../light-rates.toml");

/// `vvvv:pppp` from a device key (`vvvv:pppp`, maybe with a serial), lowercase.
fn model(key: &str) -> Option<String> {
    let mut parts = key.split(':');
    let vid = parts.next().filter(|p| p.len() == 4)?;
    let pid = parts.next().filter(|p| p.len() == 4)?;
    Some(format!("{vid}:{pid}").to_ascii_lowercase())
}

impl Rates {
    /// Reads a rates file; every interval must be above zero and every model `vvvv:pppp`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let rates: Rates = toml::from_str(text).map_err(|e| e.to_string())?;
        for r in &rates.lights {
            if r.interval_ms == 0 {
                return Err(format!("{}: interval_ms must be above 0", r.class));
            }
            if let Some(d) = &r.device {
                if model(d).as_deref() != Some(d.as_str()) {
                    return Err(format!("{}: device {d:?} isn't vvvv:pppp", r.class));
                }
            }
        }
        Ok(rates)
    }

    /// The bundled rates. The tests check them, so they always parse.
    pub fn bundled() -> Self {
        Self::parse(BUNDLED).unwrap_or_default()
    }

    /// The interval for a light of `class`, the device `key` if known: that model's entries
    /// when it has any, else the class's own; the slowest of them. `None` when the data says
    /// nothing (the caller keeps its own default).
    pub fn interval(&self, class: &str, key: Option<&str>) -> Option<Duration> {
        let model = key.and_then(model);
        let of_class = || self.lights.iter().filter(|r| r.class == class);
        let own: Vec<&LightRate> = of_class()
            .filter(|r| r.device.is_some() && r.device == model)
            .collect();
        let pick = if own.is_empty() {
            of_class().filter(|r| r.device.is_none()).collect()
        } else {
            own
        };
        pick.iter()
            .map(|r| r.interval_ms)
            .max()
            .map(|ms| Duration::from_millis(u64::from(ms)))
    }

    /// Puts in a measured entry, replacing the one for the same class, model and bus. A
    /// measurement also replaces the class's (or model's) unmeasured default.
    pub fn put(&mut self, rate: LightRate) {
        if rate.measured.is_some() {
            self.lights.retain(|r| {
                !(r.measured.is_none() && r.class == rate.class && r.device == rate.device)
            });
        }
        match self.lights.iter_mut().find(|r| r.same(&rate)) {
            Some(r) => *r = rate,
            None => self.lights.push(rate),
        }
    }

    /// The file again: `header` (the existing file's leading comments) and every entry.
    pub fn to_toml(&self, header: &str) -> Result<String, String> {
        let body = toml::to_string(self).map_err(|e| e.to_string())?;
        let mut out = header.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&body);
        Ok(out)
    }
}

/// The comment lines (and blank lines) at the top of a rates file, to keep on a rewrite.
pub fn header(text: &str) -> String {
    text.lines()
        .take_while(|l| {
            let l = l.trim();
            l.is_empty() || l.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(rate_hz: f64, p95_ms: f64, writes: u32, errors: u32) -> Measured {
        Measured {
            rate_hz,
            p50_ms: p95_ms / 2.0,
            p95_ms,
            writes,
            errors,
            cpu_ms: Some(12),
            seconds: 10.0,
            at: "2026-11-05T00:00:00Z".into(),
            build: None,
        }
    }

    #[test]
    fn the_bundled_rates_parse_and_keep_todays_defaults() {
        let r = Rates::parse(BUNDLED).expect("bundled rates");
        let ms = |class, key| r.interval(class, key).map(|d| d.as_millis());
        assert_eq!(ms(KEYBOARD, None), Some(100));
        assert_eq!(ms("omen-tracer", Some("103c:84fd")), Some(66));
        assert_eq!(ms(LAMP_ARRAY, Some("046d:c08b:SERIAL")), Some(50));
        assert_eq!(ms("laser", None), None, "unknown: the caller's default");
        for p in super::super::Protocol::ALL {
            if !p.zones().is_empty() {
                assert!(r.interval(p.id(), None).is_some(), "{} has a rate", p.id());
            }
        }
    }

    #[test]
    fn a_models_own_entry_wins_and_the_slowest_bus_counts() {
        let text = r#"
            [[light]]
            class = "lamp-array"
            interval_ms = 50
            [[light]]
            class = "lamp-array"
            device = "046d:c08b"
            interval_ms = 80
            [[light]]
            class = "keyboard"
            transport = "usb"
            interval_ms = 40
            [[light]]
            class = "keyboard"
            transport = "ble"
            interval_ms = 90
        "#;
        let r = Rates::parse(text).unwrap();
        let ms = |class, key| r.interval(class, key).unwrap().as_millis();
        assert_eq!(
            ms(LAMP_ARRAY, Some("046D:C08B:abc")),
            80,
            "any case, any serial"
        );
        assert_eq!(ms(LAMP_ARRAY, Some("2a5f:0101")), 50);
        assert_eq!(ms(LAMP_ARRAY, None), 50);
        assert_eq!(ms(KEYBOARD, None), 90, "BLE is the slower one");
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(Rates::parse("[[light]]\nclass = \"keyboard\"\ninterval_ms = 0\n").is_err());
        assert!(Rates::parse(
            "[[light]]\nclass = \"lamp-array\"\ndevice = \"46d:c08b\"\ninterval_ms = 50\n"
        )
        .is_err());
        assert!(
            Rates::parse("[[light]]\nclass = \"keyboard\"\ninterval_ms = 50\nfps = 3\n").is_err()
        );
    }

    #[test]
    fn a_measurement_becomes_an_interval_with_headroom() {
        // 40 writes a second back to back, p95 10 ms: 25 ms * 1.5, rounded up.
        assert_eq!(measured(40.0, 10.0, 400, 0).interval_ms(), Some(38));
        // The latency is the slower one: 60 ms * 1.5.
        assert_eq!(measured(100.0, 60.0, 400, 1).interval_ms(), Some(90));
        // Very fast: still 30 fps at most.
        assert_eq!(measured(900.0, 1.0, 400, 0).interval_ms(), Some(FLOOR_MS));
        // Too many errors, or nothing accepted: no rate.
        assert_eq!(measured(40.0, 10.0, 100, 6).interval_ms(), None);
        assert_eq!(measured(0.0, 0.0, 10, 10).interval_ms(), None);
    }

    #[test]
    fn a_rewrite_keeps_the_header_and_replaces_the_same_light() {
        let mut r = Rates::bundled();
        let n = r.lights.len();
        let rate = LightRate {
            class: KEYBOARD.into(),
            device: None,
            transport: Some("usb".into()),
            interval_ms: 38,
            measured: Some(measured(40.0, 10.0, 400, 0)),
        };
        r.put(rate.clone());
        r.put(LightRate {
            interval_ms: 39,
            ..rate.clone()
        });
        assert_eq!(
            r.lights.len(),
            n,
            "the default and the same bus's entry: replaced"
        );
        let text = r.to_toml(&header(BUNDLED)).unwrap();
        assert!(text.starts_with("# How often each kind of light"), "{text}");
        let back = Rates::parse(&text).expect("its own output parses");
        assert_eq!(back, r);
        assert_eq!(back.interval(KEYBOARD, None).unwrap().as_millis(), 39);
        // BLE measured too: the slower bus counts.
        r.put(LightRate {
            transport: Some("ble".into()),
            interval_ms: 95,
            ..rate
        });
        assert_eq!(r.interval(KEYBOARD, None).unwrap().as_millis(), 95);
        assert_eq!(
            r.interval("omen-tracer", None).unwrap().as_millis(),
            66,
            "the rest stay"
        );
        assert_eq!(header("[[light]]\n# not a header"), "");
    }
}
