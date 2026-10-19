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
            GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO)
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
            });
        }
        BOOL(1)
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
