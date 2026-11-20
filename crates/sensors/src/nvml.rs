//! GPU temperature, graphics clock, power and fan from NVIDIA's NVML.
//!
//! `nvml.dll` is loaded at run time (`LoadLibraryExW`, System32 only), never linked: a PC
//! without the driver, without an NVIDIA GPU, or where the sandbox denies the load simply has no
//! GPU readings. That is `unavailable`, never an error to the subscriber and never 0.

use crate::{Metric, Reading, Source, SourceError};

/// The DLL's name, looked up in System32 only.
pub const DLL: &str = "nvml.dll";

/// What a source needs from NVML; a fake in tests. Dropping the backend shuts NVML down.
pub trait Backend {
    /// GPUs NVML sees.
    fn device_count(&mut self) -> Result<u32, String>;
    fn temperature_c(&mut self) -> Result<u32, String>;
    fn graphics_clock_mhz(&mut self) -> Result<u32, String>;
    /// NVML reports power in milliwatts.
    fn power_mw(&mut self) -> Result<u32, String>;
    /// Fan speed in RPM. Older drivers have no such call: an error, so the fan stays
    /// unavailable (NVML's classic fan call is a percent, which is not published as RPM).
    fn fan_rpm(&mut self) -> Result<u32, String>;
}

/// The first GPU's readings. A value NVML can't give is left out, never 0.
pub struct NvmlSource<B: Backend>(B);

impl<B: Backend> NvmlSource<B> {
    pub fn new(backend: B) -> Self {
        NvmlSource(backend)
    }
}

impl NvmlSource<Nvml> {
    /// Loads the real NVML, or says why not.
    pub fn load(dll: &str) -> Result<Self, SourceError> {
        Nvml::load(dll).map(NvmlSource)
    }
}

impl<B: Backend> Source for NvmlSource<B> {
    fn read(&mut self) -> Result<Reading, SourceError> {
        match self.0.device_count() {
            Ok(0) => return Err(SourceError("NVML sees no GPU".into())),
            Ok(_) => {}
            Err(e) => return Err(SourceError(e)),
        }
        let mut reading = Reading::new();
        let b = &mut self.0;
        for (metric, value) in [
            (Metric::GpuTemp, b.temperature_c().map(i64::from)),
            (Metric::GpuClock, b.graphics_clock_mhz().map(i64::from)),
            // Milliwatts to whole watts, rounded.
            (
                Metric::GpuPower,
                b.power_mw().map(|mw| (i64::from(mw) + 500) / 1000),
            ),
            (Metric::GpuFan, b.fan_rpm().map(i64::from)),
        ] {
            if let Ok(v) = value {
                reading.set(metric, v);
            }
        }
        Ok(reading)
    }
}

/// The real GPU's source: [`NvmlSource::load`], or one that always reports unavailable when the
/// load fails, so the engine's code path is the same either way.
pub fn source_or_unavailable(dll: &str) -> Box<dyn Source> {
    match NvmlSource::load(dll) {
        Ok(s) => Box::new(s),
        Err(e) => Box::new(Unavailable(e)),
    }
}

struct Unavailable(SourceError);

impl Source for Unavailable {
    fn read(&mut self) -> Result<Reading, SourceError> {
        Err(self.0.clone())
    }
}

#[cfg(windows)]
pub use real::Nvml;

#[cfg(windows)]
mod real {
    use super::Backend;
    use crate::SourceError;
    use std::ffi::c_void;
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    type Device = *mut c_void;
    type Init = unsafe extern "C" fn() -> i32;
    type Count = unsafe extern "C" fn(*mut u32) -> i32;
    type Handle = unsafe extern "C" fn(u32, *mut Device) -> i32;
    type Read = unsafe extern "C" fn(Device, *mut u32) -> i32;
    type Typed = unsafe extern "C" fn(Device, u32, *mut u32) -> i32;
    type Fan = unsafe extern "C" fn(Device, *mut FanInfo) -> i32;

    /// `nvmlFanSpeedInfo_t`; `version` is `sizeof | (1 << 24)` (`nvmlFanSpeedInfo_v1`).
    #[repr(C)]
    struct FanInfo {
        version: u32,
        fan: u32,
        speed: u32,
    }

    pub struct Nvml {
        module: HMODULE,
        shutdown: Init,
        temperature: Typed,
        clock: Typed,
        power: Read,
        fan: Option<Fan>,
        /// The first GPU, null when NVML sees none.
        device: Device,
    }

    // SAFETY: NVML calls are thread-safe; the engine uses a source from one thread at a time.
    unsafe impl Send for Nvml {}

    fn err(what: &str, detail: impl std::fmt::Display) -> SourceError {
        SourceError(format!("NVML: {what}: {detail}"))
    }

    /// A function of the loaded module, cast to the signature NVML documents.
    macro_rules! sym {
        ($module:expr, $name:literal, $ty:ty) => {{
            // SAFETY: a NUL-terminated name; the address is only called through NVML's signature.
            let p = unsafe { GetProcAddress($module, PCSTR(concat!($name, "\0").as_ptr())) };
            // SAFETY: both are function pointers; NVML documents this signature.
            p.map(|f| unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f)
            })
        }};
    }

    impl Nvml {
        pub fn load(dll: &str) -> Result<Nvml, SourceError> {
            let wide: Vec<u16> = dll.encode_utf16().chain(Some(0)).collect();
            // SAFETY: NUL-terminated name; System32 only, so a planted copy is never picked up.
            let module = unsafe {
                LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
            }
            .map_err(|e| err("load", e))?;
            Self::bind(module).inspect_err(|_| {
                // SAFETY: the module was loaded above and nothing else holds it.
                let _ = unsafe { FreeLibrary(module) };
            })
        }

        fn bind(module: HMODULE) -> Result<Nvml, SourceError> {
            let missing = |n: &str| err("missing function", n);
            let init = sym!(module, "nvmlInit_v2", Init).ok_or_else(|| missing("nvmlInit_v2"))?;
            let shutdown =
                sym!(module, "nvmlShutdown", Init).ok_or_else(|| missing("nvmlShutdown"))?;
            let count = sym!(module, "nvmlDeviceGetCount_v2", Count)
                .ok_or_else(|| missing("nvmlDeviceGetCount_v2"))?;
            let handle = sym!(module, "nvmlDeviceGetHandleByIndex_v2", Handle)
                .ok_or_else(|| missing("nvmlDeviceGetHandleByIndex_v2"))?;
            let temperature = sym!(module, "nvmlDeviceGetTemperature", Typed)
                .ok_or_else(|| missing("nvmlDeviceGetTemperature"))?;
            let clock = sym!(module, "nvmlDeviceGetClockInfo", Typed)
                .ok_or_else(|| missing("nvmlDeviceGetClockInfo"))?;
            let power = sym!(module, "nvmlDeviceGetPowerUsage", Read)
                .ok_or_else(|| missing("nvmlDeviceGetPowerUsage"))?;
            let fan = sym!(module, "nvmlDeviceGetFanSpeedRPM", Fan);
            // SAFETY: nvmlInit_v2 takes no arguments.
            let code = unsafe { init() };
            if code != 0 {
                return Err(err("init", code));
            }
            let mut device: Device = std::ptr::null_mut();
            let mut devices = 0u32;
            // SAFETY: NVML is initialised; `devices` receives the count.
            if unsafe { count(&mut devices) } == 0 && devices > 0 {
                let mut first: Device = std::ptr::null_mut();
                // SAFETY: index 0 of `devices`.
                if unsafe { handle(0, &mut first) } == 0 {
                    device = first;
                }
            }
            Ok(Nvml {
                module,
                shutdown,
                temperature,
                clock,
                power,
                fan,
                device,
            })
        }

        fn typed(&self, f: Typed, what: &str) -> Result<u32, String> {
            let mut v = 0u32;
            // SAFETY: a valid device handle; kind 0 is NVML_TEMPERATURE_GPU / NVML_CLOCK_GRAPHICS.
            let code = unsafe { f(self.device, 0, &mut v) };
            if code == 0 {
                Ok(v)
            } else {
                Err(format!("{what}: {code}"))
            }
        }
    }

    impl Backend for Nvml {
        fn device_count(&mut self) -> Result<u32, String> {
            Ok(u32::from(!self.device.is_null()))
        }

        fn temperature_c(&mut self) -> Result<u32, String> {
            self.typed(self.temperature, "temperature")
        }

        fn graphics_clock_mhz(&mut self) -> Result<u32, String> {
            self.typed(self.clock, "clock")
        }

        fn power_mw(&mut self) -> Result<u32, String> {
            let mut v = 0u32;
            // SAFETY: a valid device handle and an out value of the right size.
            let code = unsafe { (self.power)(self.device, &mut v) };
            if code == 0 {
                Ok(v)
            } else {
                Err(format!("power: {code}"))
            }
        }

        fn fan_rpm(&mut self) -> Result<u32, String> {
            let f = self.fan.ok_or("no RPM call in this driver")?;
            let mut info = FanInfo {
                version: std::mem::size_of::<FanInfo>() as u32 | (1 << 24),
                fan: 0,
                speed: 0,
            };
            // SAFETY: a valid device handle and the struct NVML documents.
            let code = unsafe { f(self.device, &mut info) };
            if code == 0 {
                Ok(info.speed)
            } else {
                Err(format!("fan: {code}"))
            }
        }
    }

    impl Drop for Nvml {
        fn drop(&mut self) {
            // SAFETY: shuts down what `bind` initialised, then releases the DLL; once.
            unsafe {
                (self.shutdown)();
                let _ = FreeLibrary(self.module);
            }
        }
    }
}

/// Elsewhere there is no NVML.
#[cfg(not(windows))]
pub struct Nvml;

#[cfg(not(windows))]
impl Nvml {
    pub fn load(_dll: &str) -> Result<Nvml, SourceError> {
        Err(SourceError("NVML: not Windows".into()))
    }
}

#[cfg(not(windows))]
impl Backend for Nvml {
    fn device_count(&mut self) -> Result<u32, String> {
        Ok(0)
    }
    fn temperature_c(&mut self) -> Result<u32, String> {
        Err("not Windows".into())
    }
    fn graphics_clock_mhz(&mut self) -> Result<u32, String> {
        Err("not Windows".into())
    }
    fn power_mw(&mut self) -> Result<u32, String> {
        Err("not Windows".into())
    }
    fn fan_rpm(&mut self) -> Result<u32, String> {
        Err("not Windows".into())
    }
}
