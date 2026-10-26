/// A display the overlay window can be placed on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Monitor {
    /// Stable OS identifier persisted in the settings (e.g. `\\.\DISPLAY2` on Windows).
    pub id: String,
    /// Human-readable name for the settings UI.
    pub label: String,
    pub primary: bool,
    /// Work area (excluding the taskbar) in physical pixels: left, top, right, bottom.
    pub work_area: [i32; 4],
    /// Its scaling as a DPI (96 = 100 %, 144 = 150 %).
    pub dpi: u32,
}

impl Monitor {
    /// Physical pixels per point (logical pixel) on this display.
    pub fn scale(&self) -> f32 {
        if self.dpi == 0 {
            1.0
        } else {
            self.dpi as f32 / 96.0
        }
    }
}

/// Lists the connected displays. Empty on platforms where moving the overlay
/// between displays is not supported.
#[cfg(target_os = "windows")]
pub fn list() -> Vec<Monitor> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };

    const MONITORINFOF_PRIMARY: u32 = 1;

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let monitors = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let ok = unsafe {
            GetMonitorInfoW(
                monitor,
                &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
            )
        };
        if ok.as_bool() {
            let len = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            let id = String::from_utf16_lossy(&info.szDevice[..len]);
            let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
            let full = info.monitorInfo.rcMonitor;
            let work = info.monitorInfo.rcWork;
            let number = id.trim_start_matches(r"\\.\DISPLAY");
            let label = format!(
                "Display {number} ({}x{}){}",
                full.right - full.left,
                full.bottom - full.top,
                if primary { " - main" } else { "" }
            );
            monitors.push(Monitor {
                id,
                label,
                primary,
                work_area: [work.left, work.top, work.right, work.bottom],
                dpi: dpi_of(monitor),
            });
        }
        BOOL(1)
    }

    // The effective DPI (the scaling set in Windows), 96 when it can't be read. The process is
    // per-monitor DPI aware (winit), so this is the display's own.
    fn dpi_of(monitor: HMONITOR) -> u32 {
        use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
        let (mut x, mut y) = (0u32, 0u32);
        // SAFETY: `monitor` comes from the enumeration; the outputs are ours.
        match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
            Ok(()) if x > 0 => x,
            _ => 96,
        }
    }

    let mut monitors: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut monitors as *mut Vec<Monitor> as isize),
        );
    }
    monitors.sort_by(|a, b| a.label.cmp(&b.label));
    monitors
}

#[cfg(not(target_os = "windows"))]
pub fn list() -> Vec<Monitor> {
    Vec::new()
}

/// The display matching `id`, falling back to the main display when `id` is
/// empty or that display is no longer connected.
pub fn find(monitors: &[Monitor], id: &str) -> Option<Monitor> {
    monitors
        .iter()
        .find(|m| !id.is_empty() && m.id == id)
        .or_else(|| monitors.iter().find(|m| m.primary))
        .cloned()
}
