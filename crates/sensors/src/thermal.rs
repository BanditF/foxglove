//! System temperature from Windows' Thermal Zone Information counters
//! (`\Thermal Zone Information(*)\Temperature`, Kelvin). No admin, no driver, no elevation.
//!
//! These are the board's and case's ACPI zones, not the processor's package temperature, so
//! the value is published as `sensor.system.temp_c` (the hottest zone) and never as a CPU
//! reading. A PC without the counter, or a zone with no valid value, is absent: never 0.
//!
//! A [`ThermalSource`] holds its own open query, so it exists only while someone needs it.

use crate::{Metric, Reading, Source, SourceError, Zone};

/// Kelvin to whole degrees Celsius; `None` for a value that can't be a temperature.
pub fn celsius(kelvin: f64) -> Option<i64> {
    if !kelvin.is_finite() || kelvin <= 0.0 {
        return None;
    }
    let c = (kelvin - 273.15).round() as i64;
    (-50..=150).contains(&c).then_some(c)
}

/// The zone's short name from the counter instance (`\_TZ.TZ00` is "TZ00").
pub fn zone_name(instance: &str) -> String {
    let t = instance.trim_start_matches('\\');
    let t = match t.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("_tz.") => &t[4..],
        _ => t,
    };
    t.to_uppercase()
}

/// The zones from the counter's instances, hottest first (name on ties); an invalid value drops that zone.
pub fn zones_from(instances: &[(String, f64)]) -> Vec<Zone> {
    let mut zones: Vec<Zone> = instances
        .iter()
        .filter_map(|(name, k)| {
            let celsius = celsius(*k)?;
            let name = zone_name(name);
            (!name.is_empty()).then_some(Zone { name, celsius })
        })
        .collect();
    zones.sort_by(|a, b| b.celsius.cmp(&a.celsius).then_with(|| a.name.cmp(&b.name)));
    zones
}

/// What the zones make of a reading: the hottest as `SysTemp`, the list for the page. No zone,
/// no value.
pub fn reading_of(zones: Vec<Zone>) -> Reading {
    let mut r = Reading::new();
    if let Some(hottest) = zones.iter().map(|z| z.celsius).max() {
        r.set(Metric::SysTemp, hottest);
        r = r.with_zones(zones);
    }
    r
}

#[cfg(windows)]
mod imp {
    use super::zones_from;
    use crate::Zone;
    use windows::core::PCWSTR;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
        PDH_MORE_DATA,
    };

    const TEMPERATURE: &str = r"\Thermal Zone Information(*)\Temperature";

    pub struct Sampler {
        query: Option<PDH_HQUERY>,
        counter: Option<PDH_HCOUNTER>,
    }

    // SAFETY: PDH handles may be used from any thread; the sampler is used by one at a time.
    unsafe impl Send for Sampler {}

    impl Sampler {
        pub fn new() -> Self {
            let mut query = PDH_HQUERY::default();
            // SAFETY: a null data source is the live system; `query` receives the handle.
            if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
                return Sampler {
                    query: None,
                    counter: None,
                };
            }
            let wide: Vec<u16> = TEMPERATURE.encode_utf16().chain(Some(0)).collect();
            let mut counter = PDH_HCOUNTER::default();
            // SAFETY: `wide` is NUL-terminated and outlives the call; `query` is open.
            let status =
                unsafe { PdhAddEnglishCounterW(query, PCWSTR(wide.as_ptr()), 0, &mut counter) };
            Sampler {
                query: Some(query),
                counter: (status == 0).then_some(counter),
            }
        }

        pub fn read(&mut self) -> Vec<Zone> {
            let (Some(query), Some(counter)) = (self.query, self.counter) else {
                return Vec::new();
            };
            // SAFETY: the query is open while the sampler lives.
            if unsafe { PdhCollectQueryData(query) } != 0 {
                return Vec::new();
            }
            let (mut size, mut count) = (0u32, 0u32);
            // SAFETY: asks for the size only (no buffer).
            let status = unsafe {
                PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
            };
            if status != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            // u64 words keep the items aligned; the strings follow them in the same buffer.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            // SAFETY: `buffer` holds at least `size` bytes, as PDH asked.
            let status = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    Some(items),
                )
            };
            if status != 0 {
                return Vec::new();
            }
            let mut instances = Vec::with_capacity(count as usize);
            for i in 0..count as usize {
                // SAFETY: PDH wrote `count` items at the start of the buffer, and their name
                // pointers into it; the buffer lives until the end of this function.
                let item = unsafe { &*items.add(i) };
                if item.FmtValue.CStatus != 0 {
                    continue;
                }
                // SAFETY: a NUL-terminated name inside the buffer.
                let name = unsafe { item.szName.to_string() }.unwrap_or_default();
                // SAFETY: PDH_FMT_DOUBLE fills the double member.
                instances.push((name, unsafe { item.FmtValue.Anonymous.doubleValue }));
            }
            zones_from(&instances)
        }
    }

    impl Drop for Sampler {
        fn drop(&mut self) {
            if let Some(q) = self.query {
                // SAFETY: closes the query this sampler opened, once; its counter goes with it.
                unsafe { PdhCloseQuery(q) };
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::Zone;

    pub struct Sampler;

    impl Sampler {
        pub fn new() -> Self {
            Sampler
        }

        pub fn read(&mut self) -> Vec<Zone> {
            Vec::new()
        }
    }
}

/// The thermal zone counter as a [`Source`]: the hottest zone, or nothing.
pub struct ThermalSource(imp::Sampler);

impl ThermalSource {
    pub fn new() -> Self {
        ThermalSource(imp::Sampler::new())
    }
}

impl Default for ThermalSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Source for ThermalSource {
    fn read(&mut self) -> Result<Reading, SourceError> {
        Ok(reading_of(self.0.read()))
    }
}
