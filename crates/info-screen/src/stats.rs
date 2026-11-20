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

pub use foxglove_sensors::pdh::{gpu_load, percent};

use foxglove_sensors::pdh::PdhSampler;

/// The counters, shared with the sensors module (`foxglove-sensors`): the same code, so the
/// numbers are the ones the PC load layout always showed. Open only while that layout is on
/// screen.
#[derive(Default)]
pub struct Sampler(PdhSampler);

impl Sampler {
    /// Opens the counters and takes the first collection (rates need two). Without PDH
    /// (or a counter this PC lacks) those values stay unknown.
    pub fn new() -> Self {
        Sampler(PdhSampler::new())
    }
}

impl Sample for Sampler {
    fn sample(&mut self) -> Stats {
        let load = self.0.read();
        Stats {
            cpu: load.cpu,
            gpu: load.gpu,
            ram: load.ram,
        }
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
