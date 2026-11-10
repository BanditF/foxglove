// SPDX-License-Identifier: MIT
//! The Win32 calls the shell needs: AUMID, DPI awareness, single instance, showing and placing
//! the window, and the monitor list.

use crate::placement::{Monitor, Rect};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{BOOL, HSTRING};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, RECT,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsIconic, IsWindowVisible, IsZoomed, SetForegroundWindow, SetWindowPos,
    ShowWindow, MONITORINFOF_PRIMARY, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
    SW_RESTORE, SW_SHOW,
};

/// Sets the AUMID and per-monitor v2 DPI awareness. Call first thing, before any window exists.
pub fn init_process(aumid: &str) {
    if let Err(e) = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(aumid)) } {
        log::warn!("AUMID {aumid}: {e}");
    }
    // Fails harmlessly if a manifest already set it.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// The first instance holds this for its lifetime; a second instance only signals it.
pub struct Instance {
    _mutex: HANDLE,
}

pub enum Claim {
    First(Instance, ShowSignal),
    /// Another hub on the same folder runs; it was asked to show its window.
    Second,
}

/// A named auto-reset event a second instance sets to bring the first one's window up.
pub struct ShowSignal(isize);

impl ShowSignal {
    /// Blocks a thread on the event and calls `on_show` each time it is set; never polls.
    pub fn listen(self, on_show: impl Fn() + Send + 'static) {
        let raw = self.0;
        std::thread::Builder::new()
            .name("hideout-show".into())
            .spawn(move || loop {
                let w = unsafe { WaitForSingleObject(HANDLE(raw as *mut _), INFINITE) };
                if w.0 != 0 {
                    log::warn!("show signal wait ended ({:#x})", w.0);
                    return;
                }
                on_show();
            })
            .ok();
    }
}

/// Claims the single instance for the hub folder named by `suffix` (see `HubPaths::instance_suffix`).
pub fn claim_instance(suffix: &str) -> std::io::Result<Claim> {
    let mutex_name = HSTRING::from(format!("Local\\BanditF.Hideout.Hub{suffix}"));
    let event_name = HSTRING::from(format!("Local\\BanditF.Hideout.Show{suffix}"));
    let mutex = unsafe { CreateMutexW(None, false, &mutex_name) }.map_err(io)?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(mutex) }.ok();
        if let Ok(ev) = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &event_name) } {
            unsafe {
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            }
        }
        return Ok(Claim::Second);
    }
    let event = unsafe { CreateEventW(None, false, false, &event_name) }.map_err(io)?;
    Ok(Claim::First(
        Instance { _mutex: mutex },
        ShowSignal(event.0 as isize),
    ))
}

fn io(e: windows::core::Error) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// The main window's handle, once eframe has made it; 0 before.
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

pub fn set_main_window(hwnd: isize) {
    MAIN_HWND.store(hwnd, Ordering::Release);
}

fn main_window() -> Option<HWND> {
    match MAIN_HWND.load(Ordering::Acquire) {
        0 => None,
        h => Some(HWND(h as *mut _)),
    }
}

/// Shows, restores and focuses the main window. Works while eframe draws nothing (a hidden
/// window gets no frames), so the tray and the second instance call it directly.
pub fn show_main_window() {
    if let Some(h) = main_window() {
        unsafe {
            let _ = ShowWindow(
                h,
                if IsIconic(h).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            let _ = SetForegroundWindow(h);
        }
    }
}

pub fn hide_main_window() {
    if let Some(h) = main_window() {
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

/// False only while the main window exists and is hidden. Windows sends no `WM_PAINT` to a
/// hidden window, so winit's `request_redraw` brings no frame then (#935, see `wake`).
pub fn main_window_drawable() -> bool {
    main_window().is_none_or(|h| unsafe { IsWindowVisible(h).as_bool() })
}

/// True while the main window is shown and not minimised; false before it exists.
pub fn main_window_on_screen() -> bool {
    main_window().is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() })
}

/// The main window's rectangle, if it is in a normal state (not minimised or maximised).
pub fn main_window_rect() -> Option<Rect> {
    let h = main_window()?;
    unsafe {
        if IsIconic(h).as_bool() || IsZoomed(h).as_bool() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(h, &mut r).ok()?;
        Some(Rect {
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
        })
    }
}

/// Moves the window onto the target monitor first (Windows rescales it for that monitor's DPI),
/// then sets the exact rectangle on that monitor.
pub fn place_main_window(r: Rect) {
    let Some(h) = main_window() else { return };
    unsafe {
        let _ = SetWindowPos(
            h,
            None,
            r.x,
            r.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = SetWindowPos(h, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Every display, with its work area and scale.
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let out = &mut *(data.0 as *mut Vec<Monitor>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(m, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let w = info.monitorInfo.rcWork;
            let a = info.monitorInfo.rcMonitor;
            let len = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            out.push(Monitor {
                key: String::from_utf16_lossy(&info.szDevice[..len]),
                path: String::new(),
                area: Rect {
                    x: a.left,
                    y: a.top,
                    w: a.right - a.left,
                    h: a.bottom - a.top,
                },
                work: Rect {
                    x: w.left,
                    y: w.top,
                    w: w.right - w.left,
                    h: w.bottom - w.top,
                },
                scale: dx as f32 / 96.0,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        true.into()
    }
    let mut out: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    let paths = display_paths();
    for m in &mut out {
        if let Some((_, p)) = paths.iter().find(|(g, _)| g.eq_ignore_ascii_case(&m.key)) {
            m.path = p.clone();
        }
    }
    out
}

/// Each active display's GDI device name (`\.\DISPLAY1`) with its target's
/// `monitorDevicePath` (§2.5), from the display configuration.
fn display_paths() -> Vec<(String, String)> {
    use windows::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
        DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    };
    let text = |w: &[u16]| {
        let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..n])
    };
    let (mut np, mut nm) = (0u32, 0u32);
    unsafe {
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return Vec::new();
        }
        paths.truncate(np as usize);
        let mut out = Vec::new();
        for p in &paths {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) == 0
                && DisplayConfigGetDeviceInfo(&mut tgt.header) == 0
            {
                let path = text(&tgt.monitorDevicePath);
                if !path.is_empty() {
                    out.push((text(&src.viewGdiDeviceName), path));
                }
            }
        }
        out
    }
}

/// The main window as winit names it (its HWND), for `wake::Gate`.
pub fn main_window_id() -> Option<winit::window::WindowId> {
    main_window().map(|h| winit::window::WindowId::from(h.0 as usize as u64))
}

/// The tray flyout's window, once found by its title (eframe gives an immediate viewport no
/// handle); 0 while none.
static FLYOUT_HWND: AtomicIsize = AtomicIsize::new(0);

/// Finds this thread's window titled `title` (not the main window, nor one in `skip`).
fn find_own_window(title: &str, skip: &[isize]) -> Option<HWND> {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, GetWindowTextW};
    struct Find<'a> {
        title: Vec<u16>,
        main: isize,
        skip: &'a [isize],
        found: isize,
    }
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        let f = &mut *(data.0 as *mut Find);
        let mut buf = [0u16; 128];
        let n = GetWindowTextW(h, &mut buf) as usize;
        let id = h.0 as isize;
        if id != f.main && !f.skip.contains(&id) && buf[..n] == f.title[..] {
            f.found = h.0 as isize;
            return false.into();
        }
        true.into()
    }
    let mut f = Find {
        title: title.encode_utf16().collect(),
        main: MAIN_HWND.load(Ordering::Acquire),
        skip,
        found: 0,
    };
    unsafe {
        let _ = EnumThreadWindows(
            GetCurrentThreadId(),
            Some(each),
            LPARAM(&mut f as *mut _ as isize),
        );
    }
    (f.found != 0).then_some(HWND(f.found as *mut _))
}

/// The flyout window closed: forget its handle.
pub fn forget_flyout() {
    FLYOUT_HWND.store(0, Ordering::Release);
}

/// Places the flyout titled `title` for the tray icon at `icon` (physical pixels), `size` in
/// points on the icon's monitor, against the taskbar (`flyout::place`). `first` also rounds its
/// corners, shows it and brings it to the front, so focus loss can close it. False while the
/// window isn't there yet.
pub fn place_flyout(title: &str, icon: Rect, size: (f32, f32), first: bool) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, HWND_TOPMOST};
    let cached = match FLYOUT_HWND.load(Ordering::Acquire) {
        0 => None,
        h => Some(HWND(h as *mut _)).filter(|&h| unsafe { IsWindow(Some(h)).as_bool() }),
    };
    let Some(h) = cached.or_else(|| find_own_window(title, &[])) else {
        return false;
    };
    FLYOUT_HWND.store(h.0 as isize, Ordering::Release);
    let (cx, cy) = icon.center();
    let mon = unsafe { MonitorFromPoint(POINT { x: cx, y: cy }, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(mon, &mut info) }.as_bool() {
        return false;
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    let _ = unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    let scale = dx as f32 / 96.0;
    let rect = |r: RECT| Rect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    };
    let px = |v: f32| (v * scale).round() as i32;
    let r = crate::flyout::place(
        icon,
        rect(info.rcMonitor),
        rect(info.rcWork),
        (px(size.0), px(size.1)),
        px(crate::flyout::GAP),
    );
    unsafe {
        // Onto the icon's monitor first (Windows rescales it for that DPI), then the exact rect.
        let _ = SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            r.x,
            r.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), r.x, r.y, r.w, r.h, SWP_NOACTIVATE);
        if first {
            let round = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                h,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &round as *const _ as *const _,
                std::mem::size_of_val(&round) as u32,
            );
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
    }
    true
}

/// Opens `url` in the user's browser (ShellExecute "open").
pub fn open_url(url: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let r = unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(url),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value above 32 on success.
    if r.0 as isize <= 32 {
        log::warn!("open {url}: ShellExecute {}", r.0 as isize);
    }
}

/// Maps `path` read-only for the life of the process (fonts, which egui keeps for good). The
/// view stays valid after both handles close. `None` if the file can't be opened or is empty.
pub fn map_file(path: &std::path::Path) -> Option<&'static [u8]> {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFileSizeEx, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ,
        OPEN_EXISTING,
    };
    use windows::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY,
    };
    let file = unsafe {
        CreateFileW(
            &HSTRING::from(path.as_os_str()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;
    let mut len = 0i64;
    let sized = unsafe { GetFileSizeEx(file, &mut len) }.is_ok() && len > 0;
    let mapping = if sized {
        unsafe { CreateFileMappingW(file, None, PAGE_READONLY, 0, 0, None) }.ok()
    } else {
        None
    };
    unsafe { CloseHandle(file) }.ok();
    let mapping = mapping?;
    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
    unsafe { CloseHandle(mapping) }.ok();
    if view.Value.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(view.Value as *const u8, len as usize) })
}

/// The Windows file picker for a `kind = "file"` setting (contract §4.7 v1.4), modal to the hub
/// window: `filter` is the manifest's pattern list (`a*.dll;b?.dll`), offered with "All files".
/// The chosen full path, or `None` when cancelled or the picker failed (logged).
pub fn pick_file(filter: Option<&str>) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, SIGDN_FILESYSPATH,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let result = (|| -> windows::core::Result<Option<String>> {
        // SAFETY: COM calls on the UI thread; every wide string outlives the calls using it and
        // the path from GetDisplayName is freed after it is copied.
        unsafe {
            // The UI thread may already be initialised (S_FALSE or a different mode): either way
            // the dialog works, so the result is ignored.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetOptions(FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)?;
            let (name_f, spec_f, name_all, spec_all) = (
                wide("Matching files"),
                wide(filter.unwrap_or("*.*")),
                wide("All files"),
                wide("*.*"),
            );
            let specs = [
                COMDLG_FILTERSPEC {
                    pszName: PCWSTR(name_f.as_ptr()),
                    pszSpec: PCWSTR(spec_f.as_ptr()),
                },
                COMDLG_FILTERSPEC {
                    pszName: PCWSTR(name_all.as_ptr()),
                    pszSpec: PCWSTR(spec_all.as_ptr()),
                },
            ];
            let n = if filter.is_some() { 2 } else { 1 };
            dialog.SetFileTypes(&specs[specs.len() - n..])?;
            if dialog.Show(main_window()).is_err() {
                return Ok(None); // cancelled
            }
            let item = dialog.GetResult()?;
            let path = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let s = path.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(path.0 as *const _));
            Ok(s)
        }
    })();
    result.unwrap_or_else(|e| {
        log::warn!("file picker: {e}");
        None
    })
}

// ---- Pop-out windows (H2.11, contract §2.5) -------------------------------------------------
//
// eframe gives an immediate viewport no handle, so a pop-out's window is found by its title once
// (as the flyout's) and kept. A subclass tells the frame code when a move or resize ends
// (`WM_EXITSIZEMOVE`: placement is saved then, never during a drag or on a timer) and carries the
// window menu's Keep on top and Dock back. Both wake the UI through the waker, never a poll.

/// A pop-out's place as Windows has it, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WinPlace {
    /// Where it is now (the snapped rectangle while snapped).
    pub actual: Rect,
    /// Its restored rectangle (screen coordinates).
    pub normal: Rect,
    pub maximized: bool,
    pub minimized: bool,
}

/// What the window menu (Alt+Space) asked of a pop-out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    KeepOnTop,
    DockBack,
}

const MENU_KEEP_ON_TOP: usize = 0x4810;
const MENU_DOCK_BACK: usize = 0x4820;
const SUBCLASS_ID: usize = 0x4844;

#[derive(Default)]
struct PopoutHooks {
    /// In a move or resize loop (between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`).
    moving: Vec<isize>,
    /// A move or resize ended since the frame code last asked.
    ended: Vec<isize>,
    menu: Vec<(isize, MenuCommand)>,
    waker: Option<Box<dyn Fn()>>,
}

thread_local! {
    static HOOKS: std::cell::RefCell<PopoutHooks> = std::cell::RefCell::new(PopoutHooks::default());
}

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut _)
}

/// Finds this thread's pop-out window titled `title`, not one of `skip` (other pop-outs).
pub fn find_popout(title: &str, skip: &[isize]) -> Option<isize> {
    let mut skip = skip.to_vec();
    skip.push(FLYOUT_HWND.load(Ordering::Acquire));
    find_own_window(title, &skip).map(|h| h.0 as isize)
}

/// Whether `h` is still a window.
pub fn window_alive(h: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    unsafe { IsWindow(Some(hwnd(h))).as_bool() }
}

/// Moves `h` onto the target screen first (Windows rescales it for that screen's DPI), then sets
/// the exact rectangle (physical pixels).
pub fn place_window(h: isize, r: Rect) {
    let h = hwnd(h);
    unsafe {
        let _ = SetWindowPos(
            h,
            None,
            r.x,
            r.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = SetWindowPos(h, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Shows `h` normal or maximised; `activate` brings it forward with the focus (a pop out by the
/// user), else it shows without taking the focus (a restore).
pub fn show_window(h: isize, maximized: bool, activate: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE};
    let h = hwnd(h);
    unsafe {
        let cmd = match (maximized, activate) {
            (true, _) => SW_SHOWMAXIMIZED,
            (false, true) => SW_SHOW,
            (false, false) => SW_SHOWNOACTIVATE,
        };
        let _ = ShowWindow(h, cmd);
        if activate {
            let _ = SetForegroundWindow(h);
        }
    }
}

/// Keep on top: above other windows (never above an exclusive full-screen app, which Windows
/// doesn't allow), without taking the focus.
pub fn set_topmost(h: isize, on: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOMOVE};
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    unsafe {
        let _ = SetWindowPos(
            hwnd(h),
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    check_menu(h, on);
}

/// Brings `h` forward (restoring it when minimised): a click on its placeholder tab.
pub fn focus_window(h: isize) {
    let h = hwnd(h);
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
    }
}

/// The window's place: its rectangle now and its restored one, maximised, minimised.
pub fn window_place(h: isize) -> Option<WinPlace> {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, WINDOWPLACEMENT};
    let h = hwnd(h);
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    let mut r = RECT::default();
    unsafe {
        GetWindowPlacement(h, &mut wp).ok()?;
        GetWindowRect(h, &mut r).ok()?;
    }
    let rect = |r: RECT| Rect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    };
    // `rcNormalPosition` is in workspace coordinates: off from the screen's by the primary work
    // area's corner when the taskbar is at the top or the left.
    let (dx, dy) = monitors()
        .iter()
        .find(|m| m.primary)
        .map(|m| (m.work.x - m.area.x, m.work.y - m.area.y))
        .unwrap_or((0, 0));
    let mut normal = rect(wp.rcNormalPosition);
    normal.x += dx;
    normal.y += dy;
    let (minimized, maximized) = unsafe { (IsIconic(h).as_bool(), IsZoomed(h).as_bool()) };
    Some(WinPlace {
        actual: rect(r),
        normal,
        maximized,
        minimized,
    })
}

/// The main window's rectangle in any state (the anchor a new pop-out opens next to).
pub fn main_window_any_rect() -> Option<Rect> {
    let h = main_window()?;
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r).ok()? };
    Some(Rect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    })
}

/// The subclass: move/resize loop tracking and the window menu's own items.
unsafe extern "system" fn popout_proc(
    h: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;
    use windows::Win32::UI::WindowsAndMessaging::{
        WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_SYSCOMMAND,
    };
    let id = h.0 as isize;
    let mut wake = false;
    let mut handled = false;
    HOOKS.with(|k| {
        let mut k = k.borrow_mut();
        match msg {
            WM_ENTERSIZEMOVE => k.moving.push(id),
            WM_EXITSIZEMOVE => {
                k.moving.retain(|&m| m != id);
                k.ended.push(id);
                wake = true;
            }
            WM_SYSCOMMAND => {
                let cmd = match wp.0 & 0xFFF0 {
                    MENU_KEEP_ON_TOP => Some(MenuCommand::KeepOnTop),
                    MENU_DOCK_BACK => Some(MenuCommand::DockBack),
                    _ => None,
                };
                if let Some(c) = cmd {
                    k.menu.push((id, c));
                    wake = true;
                    handled = true;
                }
            }
            WM_NCDESTROY => {
                k.moving.retain(|&m| m != id);
                k.ended.retain(|&m| m != id);
            }
            _ => {}
        }
    });
    if wake {
        HOOKS.with(|k| {
            if let Some(w) = k.borrow().waker.as_ref() {
                w();
            }
        });
    }
    if handled {
        return windows::Win32::Foundation::LRESULT(0);
    }
    DefSubclassProc(h, msg, wp, lp)
}

/// Sets the waker the subclass calls when a move ends or a menu item is picked (the UI thread's
/// `request_repaint`).
pub fn set_popout_waker(w: Box<dyn Fn()>) {
    HOOKS.with(|k| k.borrow_mut().waker = Some(w));
}

/// Hooks a pop-out window: the subclass and the window menu's Keep on top and Dock back.
pub fn install_popout_hooks(h: isize, keep_on_top: bool) {
    use windows::core::w;
    use windows::Win32::UI::Shell::SetWindowSubclass;
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, GetSystemMenu, MF_SEPARATOR, MF_STRING,
    };
    unsafe {
        let _ = SetWindowSubclass(hwnd(h), Some(popout_proc), SUBCLASS_ID, 0);
        let menu = GetSystemMenu(hwnd(h), false);
        if !menu.is_invalid() {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, MENU_KEEP_ON_TOP, w!("Keep on top"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_DOCK_BACK,
                w!("Dock back\tCtrl+Shift+O"),
            );
        }
    }
    check_menu(h, keep_on_top);
}

fn check_menu(h: isize, on: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        CheckMenuItem, GetSystemMenu, MF_BYCOMMAND, MF_CHECKED, MF_UNCHECKED,
    };
    unsafe {
        let menu = GetSystemMenu(hwnd(h), false);
        if !menu.is_invalid() {
            let state = if on { MF_CHECKED } else { MF_UNCHECKED };
            CheckMenuItem(menu, MENU_KEEP_ON_TOP as u32, (MF_BYCOMMAND | state).0);
        }
    }
}

/// Whether `h` is in a move or resize loop now (its place isn't saved then).
pub fn in_size_move(h: isize) -> bool {
    HOOKS.with(|k| k.borrow().moving.contains(&h))
}

/// Whether a move or resize of `h` ended since the last call.
pub fn take_size_move_ended(h: isize) -> bool {
    HOOKS.with(|k| {
        let mut k = k.borrow_mut();
        let had = k.ended.contains(&h);
        k.ended.retain(|&m| m != h);
        had
    })
}

/// The window menu's picks since the last call.
pub fn take_menu_commands() -> Vec<(isize, MenuCommand)> {
    HOOKS.with(|k| std::mem::take(&mut k.borrow_mut().menu))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_file_read_only() {
        let dir = std::env::temp_dir().join(format!("hideout-map-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let full = dir.join("font.bin");
        std::fs::write(&full, b"glyphs").unwrap();
        assert_eq!(map_file(&full), Some(&b"glyphs"[..]));
        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(map_file(&empty), None);
        assert_eq!(map_file(&dir.join("missing.bin")), None);
        // The mapping of font.bin stays for the process (as fonts do), so the folder stays too.
        let _ = std::fs::remove_file(&empty);
    }
}
