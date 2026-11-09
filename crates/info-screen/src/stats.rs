//! PC load for the left screen (#506): CPU, GPU and memory use, from Windows' own counters.
//!
//! - **CPU:** `\Processor Information(_Total)\% Processor Utility`, Task Manager's figure.
//! - **GPU:** `\GPU Engine(*)\Utilization Percentage`, summed per engine across processes, and
//!   the busiest engine wins, as Task Manager shows it.
//! - **Memory:** the share of physical memory in use (`GlobalMemoryStatusEx`).
//!
//! A [`Sampler`] exists only while the PC load layout is on the screen: the panel service makes
//! one when it starts drawing that layout, samples it about once a second, and drops it (closing
//! the counters) when the layout goes, Foxglove pauses, or the PC is away. Temperatures are out:
//! they need a kernel driver.

use std::collections::BTreeMap;

/// One reading: each value in percent, `None` while it isn't known (the first second, or a
/// counter this PC lacks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub cpu: Option<u8>,
    pub gpu: Option<u8>,
    pub ram: Option<u8>,
}

/// Where readings come from: Windows' counters, or a fake in tests.
pub trait Sample {
    fn sample(&mut self) -> Stats;
}

/// A percentage from a counter's value, clamped (Processor Utility can pass 100 on turbo).
pub fn percent(value: f64) -> Option<u8> {
    value
        .is_finite()
        .then(|| value.round().clamp(0.0, 100.0) as u8)
}

/// The GPU's load from the GPU Engine instances (`pid_…_luid_…_phys_…_eng_…_engtype_…`):
/// each engine's use summed over processes, then the busiest engine.
pub fn gpu_load(instances: &[(String, f64)]) -> Option<u8> {
    if instances.is_empty() {
        return None;
    }
    let mut engines: BTreeMap<&str, f64> = BTreeMap::new();
    for (name, value) in instances {
        let engine = name.find("luid_").map_or(name.as_str(), |i| &name[i..]);
        *engines.entry(engine).or_default() += value.max(0.0);
    }
    percent(engines.values().copied().fold(0.0, f64::max))
}

#[cfg(windows)]
pub use imp::Sampler;

#[cfg(windows)]
mod imp {
    use super::{gpu_load, percent, Sample, Stats};
    use windows::core::PCWSTR;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT, PDH_FMT_COUNTERVALUE,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    };
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    /// `PDH_FMT_NOCAP100`: Processor Utility passes 100 on turbo; [`percent`] clamps it.
    const NOCAP100: u32 = 0x8000;
    const CPU: &str = r"\Processor Information(_Total)\% Processor Utility";
    const GPU: &str = r"\GPU Engine(*)\Utilization Percentage";

    /// The open counters; closed when dropped.
    pub struct Sampler {
        /// `None` when PDH couldn't open a query: memory alone then.
        query: Option<PDH_HQUERY>,
        cpu: Option<PDH_HCOUNTER>,
        gpu: Option<PDH_HCOUNTER>,
    }

    // SAFETY: PDH handles may be used from any thread; the sampler is used by one at a time.
    unsafe impl Send for Sampler {}

    impl Default for Sampler {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Sampler {
        /// Opens the counters and takes the first collection (rates need two). Without PDH
        /// (or a counter this PC lacks) those values stay unknown.
        pub fn new() -> Self {
            let mut query = PDH_HQUERY::default();
            // SAFETY: a null data source is the live system; `query` receives the handle.
            if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
                return Sampler {
                    query: None,
                    cpu: None,
                    gpu: None,
                };
            }
            let add = |path: &str| {
                let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
                let mut counter = PDH_HCOUNTER::default();
                // SAFETY: `wide` is NUL-terminated and outlives the call; `query` is open.
                let status =
                    unsafe { PdhAddEnglishCounterW(query, PCWSTR(wide.as_ptr()), 0, &mut counter) };
                (status == 0).then_some(counter)
            };
            let sampler = Sampler {
                query: Some(query),
                cpu: add(CPU),
                gpu: add(GPU),
            };
            // SAFETY: the query is open.
            unsafe { PdhCollectQueryData(query) };
            sampler
        }

        fn cpu(&self) -> Option<u8> {
            let counter = self.cpu?;
            let mut value = PDH_FMT_COUNTERVALUE::default();
            // SAFETY: a counter of this open query; `value` is written on success.
            let status = unsafe {
                PdhGetFormattedCounterValue(
                    counter,
                    PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP100),
                    None,
                    &mut value,
                )
            };
            // SAFETY: PDH_FMT_DOUBLE fills the double member.
            (status == 0 && value.CStatus == 0)
                .then(|| percent(unsafe { value.Anonymous.doubleValue }))?
        }

        fn gpu(&self) -> Option<u8> {
            let counter = self.gpu?;
            let (mut size, mut count) = (0u32, 0u32);
            // SAFETY: asks for the size only (no buffer).
            let status = unsafe {
                PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
            };
            if status != PDH_MORE_DATA || size == 0 {
                return None;
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
                return None;
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
            // Instances exist but none was valid yet: not known, rather than 0%.
            if instances.is_empty() && count > 0 {
                return None;
            }
            gpu_load(&instances).or(Some(0))
        }
    }

    fn ram() -> Option<u8> {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is sized as the call requires.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(status.dwMemoryLoad.min(100) as u8)
    }

    impl Sample for Sampler {
        fn sample(&mut self) -> Stats {
            // SAFETY: the query is open while the sampler lives.
            let collected = self
                .query
                .is_some_and(|q| unsafe { PdhCollectQueryData(q) } == 0);
            Stats {
                cpu: collected.then(|| self.cpu()).flatten(),
                gpu: collected.then(|| self.gpu()).flatten(),
                ram: ram(),
            }
        }
    }

    impl Drop for Sampler {
        fn drop(&mut self) {
            if let Some(q) = self.query {
                // SAFETY: closes the query this sampler opened, once; its counters go with it.
                unsafe { PdhCloseQuery(q) };
            }
        }
    }
}

/// Elsewhere there are no counters: every value stays unknown.
#[cfg(not(windows))]
#[derive(Default)]
pub struct Sampler;

#[cfg(not(windows))]
impl Sampler {
    pub fn new() -> Self {
        Sampler
    }
}

#[cfg(not(windows))]
impl Sample for Sampler {
    fn sample(&mut self) -> Stats {
        Stats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_are_clamped_and_rounded() {
        assert_eq!(percent(37.4), Some(37));
        assert_eq!(percent(37.5), Some(38));
        assert_eq!(
            percent(131.0),
            Some(100),
            "Processor Utility passes 100 on turbo"
        );
        assert_eq!(percent(-1.0), Some(0));
        assert_eq!(percent(f64::NAN), None);
    }

    #[test]
    fn the_gpu_is_its_busiest_engine() {
        let i = |name: &str, v| (name.to_string(), v);
        let instances = [
            // Two processes on the 3D engine: 30 + 25.
            i("pid_100_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            i("pid_200_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            // Video decode alone: 40.
            i(
                "pid_300_luid_0x0_0x1_phys_0_eng_3_engtype_VideoDecode",
                40.0,
            ),
            i("pid_100_luid_0x0_0x1_phys_0_eng_1_engtype_Copy", 2.0),
        ];
        assert_eq!(gpu_load(&instances), Some(55));
        assert_eq!(gpu_load(&[]), None);
        assert_eq!(
            gpu_load(&[i("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 180.0)]),
            Some(100)
        );
    }

    /// This PC's counters, read-only: `cargo test -p foxglove-panel live_stats -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads this PC's counters"]
    fn live_stats() {
        let mut s = Sampler::new();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let stats = s.sample();
        eprintln!("{stats:?}");
        assert!(stats.ram.is_some());
    }
}
