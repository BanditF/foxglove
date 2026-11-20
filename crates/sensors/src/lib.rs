//! Read-only PC sensors as `sensor.*` facts (OMEN monitoring, steps 1-2).
//!
//! - [`Source`] is where readings come from: Windows' PDH counters ([`pdh`]), NVIDIA's NVML
//!   ([`nvml`]), or a fake in tests.
//! - [`Engine`] owns the sampling rule: nothing exists or runs until something subscribes to the
//!   namespace, the last unsubscribe drops every source, and game mode stops (or slows) it.
//!   It holds no thread or timer; the host asks [`Engine::next_due`] for the one deadline.
//! - A value that can't be read is withdrawn, never published as 0, and its group's
//!   `sensor.<group>.state` says `unavailable`.
//!
//! - [`host`] is what the program does with the hub's demand and game mode: it starts and stops
//!   the engine, and hands back its writes.
//!
//! No kernel driver, no device, no key events, no control: it only reads.

mod engine;
pub mod host;
pub mod nvml;
pub mod pdh;
pub mod ui;

pub use engine::{Config, Engine, GuardedSink, Sink, SourceFactory, SubId, ValueOut};

/// The facts' namespace. The one place the word appears: keys, the write permission and the
/// subscription match all derive from it, so a rename is this line.
pub const NAMESPACE: &str = "sensor";

/// The permission the module asks for, from [`NAMESPACE`].
pub fn write_permission() -> String {
    format!("facts.write:{NAMESPACE}.*")
}

/// A sensor group: one `state` fact each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Cpu,
    Gpu,
    Ram,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Cpu, Group::Gpu, Group::Ram];

    pub fn id(self) -> &'static str {
        match self {
            Group::Cpu => "cpu",
            Group::Gpu => "gpu",
            Group::Ram => "ram",
        }
    }

    /// `sensor.<group>.state`: `ok`, `unavailable` or `unknown`.
    pub fn state_key(self) -> String {
        format!("{NAMESPACE}.{}.state", self.id())
    }
}

/// One published value. Every value is a whole number in the unit named by [`Metric::unit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Metric {
    CpuLoad,
    GpuLoad,
    RamUsed,
    GpuTemp,
    GpuClock,
    GpuPower,
    GpuFan,
    /// The fan's duty in percent: its own fact, never published as RPM.
    GpuFanPct,
}

impl Metric {
    pub const ALL: [Metric; 8] = [
        Metric::CpuLoad,
        Metric::GpuLoad,
        Metric::RamUsed,
        Metric::GpuTemp,
        Metric::GpuClock,
        Metric::GpuPower,
        Metric::GpuFan,
        Metric::GpuFanPct,
    ];

    pub fn group(self) -> Group {
        match self {
            Metric::CpuLoad => Group::Cpu,
            Metric::GpuLoad
            | Metric::GpuTemp
            | Metric::GpuClock
            | Metric::GpuPower
            | Metric::GpuFan
            | Metric::GpuFanPct => Group::Gpu,
            Metric::RamUsed => Group::Ram,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Metric::CpuLoad | Metric::GpuLoad => "load_pct",
            Metric::RamUsed => "used_pct",
            Metric::GpuTemp => "temp_c",
            Metric::GpuClock => "clock_mhz",
            Metric::GpuPower => "power_w",
            Metric::GpuFan => "fan_rpm",
            Metric::GpuFanPct => "fan_pct",
        }
    }

    /// The unit, as the key's suffix says it.
    pub fn unit(self) -> &'static str {
        match self {
            Metric::CpuLoad | Metric::GpuLoad | Metric::RamUsed | Metric::GpuFanPct => "%",
            Metric::GpuTemp => "C",
            Metric::GpuClock => "MHz",
            Metric::GpuPower => "W",
            Metric::GpuFan => "RPM",
        }
    }

    /// The fact's key, e.g. `sensor.gpu.temp_c`.
    pub fn key(self) -> String {
        format!("{NAMESPACE}.{}.{}", self.group().id(), self.name())
    }
}

/// The values a source could read this time. A metric it couldn't read is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading(Vec<(Metric, i64)>);

impl Reading {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, metric: Metric, value: i64) -> Self {
        self.set(metric, value);
        self
    }

    pub fn set(&mut self, metric: Metric, value: i64) {
        self.0.retain(|(m, _)| *m != metric);
        self.0.push((metric, value));
    }

    pub fn get(&self, metric: Metric) -> Option<i64> {
        self.0.iter().find(|(m, _)| *m == metric).map(|(_, v)| *v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Metric, i64)> + '_ {
        self.0.iter().copied()
    }
}

/// A read that failed; the metrics it owns are then unavailable, not 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError(pub String);

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where readings come from. Dropping a source releases what it holds (PDH query, NVML).
pub trait Source {
    fn read(&mut self) -> Result<Reading, SourceError>;
}

/// After every source is dropped: returns the heap's free pages to Windows and trims the working
/// set, so the idle process is as small as it can be. Without this the allocator keeps the
/// pages the PDH buffers used (freed, but still resident). It cannot release what Windows
/// itself keeps for the process (PDH's loaded performance providers and their handles).
///
/// What to expect (test plan A22, measured on the lab PC): after demand drops to 0 the working set
/// falls (36 MB while sampling, about 2 MB after) but about 54 extra handles remain (143 against
/// 89 before the first demand), flat: they do not grow with later demand cycles. Hypothesis: state
/// that the PDH and NVML DLLs keep once loaded. Accepted; there is no fallback (no unloading the
/// DLLs, no restarting the process), and a change that makes the count grow is a bug.
pub fn trim_memory() {
    #[cfg(windows)]
    // SAFETY: both calls take the process's own pseudo-handle or default heap; failure only
    // means nothing was trimmed.
    unsafe {
        use windows::Win32::System::Memory::{GetProcessHeap, HeapCompact, HEAP_FLAGS};
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        if let Ok(heap) = GetProcessHeap() {
            HeapCompact(heap, HEAP_FLAGS(0));
        }
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Whether a facts pattern (`facts.subscribe`) hears this namespace: the subtree, a key or
/// subtree inside it, or every key. Same pattern rules as the facts store.
pub fn pattern_wants(pattern: &str) -> bool {
    let prefix = format!("{NAMESPACE}.");
    pattern == "*" || pattern.starts_with(&prefix)
}
