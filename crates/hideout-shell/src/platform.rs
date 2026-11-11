// SPDX-License-Identifier: MIT
//! The Win32 calls the shell needs: AUMID, DPI awareness, single instance, showing and placing
//! the window, and the monitor list.

use crate::placement::{Monitor, Rect};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
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

/// The main window's handle, any thread; 0 before it exists (lab control, #1003).
pub fn main_hwnd() -> isize {
    MAIN_HWND.load(Ordering::Acquire)
}

fn main_window() -> Option<HWND> {
    match MAIN_HWND.load(Ordering::Acquire) {
        0 => None,
        h => Some(HWND(h as *mut _)),
    }
}

// ---- Starting in the tray (#1012) ------------------------------------------------------------
//
// eframe 0.33 creates the root window hidden and shows it after its first painted frame
// (`post_rendering`), whatever the viewport builder says. A sign-in start must never show it,
// not even for a frame: while held, the subclass takes the show (`SWP_SHOWWINDOW`) and the
// activation out of every position change of the window, so it stays hidden and never takes the
// focus. The first real show (tray Open, a second start, Open <Module>) lets go first.

/// The window held hidden (0: none).
static HELD_HIDDEN: AtomicIsize = AtomicIsize::new(0);
const HOLD_SUBCLASS_ID: usize = 0x4845;

/// The flags a position change keeps while its window is held hidden: no show, no activation.
pub fn held_flags(flags: u32) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_SHOWWINDOW};
    (flags & !SWP_SHOWWINDOW.0) | SWP_NOACTIVATE.0
}

unsafe extern "system" fn hold_proc(
    h: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: LPARAM,
    _id: usize,
    held: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;
    use windows::Win32::UI::WindowsAndMessaging::{
        SET_WINDOW_POS_FLAGS, WINDOWPOS, WM_WINDOWPOSCHANGING,
    };
    // `held` is the `&'static AtomicIsize` given to `hold_in`.
    let held = &*(held as *const AtomicIsize);
    if msg == WM_WINDOWPOSCHANGING && held.load(Ordering::Acquire) == h.0 as isize {
        if let Some(pos) = (lp.0 as *mut WINDOWPOS).as_mut() {
            pos.flags = SET_WINDOW_POS_FLAGS(held_flags(pos.flags.0));
        }
    }
    DefSubclassProc(h, msg, wp, lp)
}

/// Keeps `h` hidden until [`let_go_hidden`] (a start in the tray). Call on its thread, before
/// its first frame.
pub fn hold_hidden(h: isize) {
    hold_in(&HELD_HIDDEN, h);
}

/// Holds `h` hidden while `held` names it. Tests pass their own cell, so a parallel test's
/// show (which lets go of [`HELD_HIDDEN`]) cannot end their hold (#1030).
fn hold_in(held: &'static AtomicIsize, h: isize) {
    use windows::Win32::UI::Shell::SetWindowSubclass;
    held.store(h, Ordering::Release);
    unsafe {
        let _ = SetWindowSubclass(
            hwnd(h),
            Some(hold_proc),
            HOLD_SUBCLASS_ID,
            held as *const AtomicIsize as usize,
        );
    }
}

/// Ends the hold: the window shows again when asked. Any thread.
pub fn let_go_hidden() {
    HELD_HIDDEN.store(0, Ordering::Release);
}

/// Shows, restores and focuses the main window. Works while eframe draws nothing (a hidden
/// window gets no frames), so the tray and the second instance call it directly. Ends a hold
/// from a start in the tray.
pub fn show_main_window() {
    let_go_hidden();
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

/// The Menu key (or Shift+F10) in the flyout (#991): Windows sends its window `WM_CONTEXTMENU`
/// with no point. egui has no key for the Menu key, so the flyout's subclass notes it here and
/// the next flyout frame opens the focused tile's ··· menu.
static FLYOUT_MENU_KEY: AtomicBool = AtomicBool::new(false);
const MENU_KEY_SUBCLASS_ID: usize = 0x464D;

/// A keyboard `WM_CONTEXTMENU` has no point: `lParam` is (-1, -1).
pub fn context_menu_from_keyboard(lp: isize) -> bool {
    (lp as u64 & 0xFFFF_FFFF) == 0xFFFF_FFFF
}

unsafe extern "system" fn menu_key_proc(
    h: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;
    use windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU;
    if msg == WM_CONTEXTMENU && context_menu_from_keyboard(lp.0) {
        FLYOUT_MENU_KEY.store(true, Ordering::Release);
        // The frame that takes it, also while the main window is hidden (#1019).
        crate::flyout::kick();
        return windows::Win32::Foundation::LRESULT(0);
    }
    DefSubclassProc(h, msg, wp, lp)
}

/// Takes the flyout's Menu key press (one per press).
pub fn take_flyout_menu_key() -> bool {
    FLYOUT_MENU_KEY.swap(false, Ordering::AcqRel)
}

/// The flyout's window handle, any thread; 0 while none (lab control, #1003).
pub fn flyout_hwnd() -> isize {
    FLYOUT_HWND.load(Ordering::Acquire)
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
    if cached.is_none() {
        // A new flyout window: listen for its Menu key.
        use windows::Win32::UI::Shell::SetWindowSubclass;
        unsafe {
            let _ = SetWindowSubclass(h, Some(menu_key_proc), MENU_KEY_SUBCLASS_ID, 0);
        }
    }
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

/// What the window menu (Alt+Space) asked of a pop-out or of the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    KeepOnTop,
    DockBack,
    /// The main window's **Quit Hideout** (#1012.5): the same one ask as the tray's.
    Quit,
    /// The Menu key (or Shift+F10 that Windows turned into `WM_CONTEXTMENU`): egui has no key
    /// for it, so the frame queues it as Shift+F10 for the focused tab (#953).
    MenuKey,
}

const MENU_KEEP_ON_TOP: usize = 0x4810;
const MENU_DOCK_BACK: usize = 0x4820;
const MENU_QUIT: usize = 0x4830;
const SUBCLASS_ID: usize = 0x4844;

#[derive(Default)]
struct PopoutHooks {
    /// In a move or resize loop (between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`).
    moving: Vec<isize>,
    /// A move or resize ended since the frame code last asked.
    ended: Vec<isize>,
    menu: Vec<(isize, MenuCommand)>,
    waker: Option<Box<dyn Fn()>>,
    /// A pop-out moved by its title row (#1054.4): from its first `WM_MOVING` until the frame
    /// code takes the drop after `WM_EXITSIZEMOVE`.
    drag: Option<crate::drag::WindowDrag>,
    /// The last `WM_MOVING` had the pointer over the main window: the next one wakes the frame
    /// even when it leaves, so the bar goes away.
    drag_near: bool,
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
pub fn bring_forward(h: isize) {
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
        WM_CONTEXTMENU, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_MOVING, WM_NCDESTROY, WM_SYSCOMMAND,
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
                if let Some(d) = k.drag.as_mut().filter(|d| d.hwnd == id) {
                    // The drop: where the pointer is now (a keyboard move ends without one).
                    d.cursor = cursor_pos().unwrap_or(d.cursor);
                    d.ended = true;
                }
                k.drag_near = false;
                wake = true;
            }
            // Only a pop-out's own move (the main window has the same subclass); a move, not
            // a resize (`WM_SIZING`). One frame per pointer step while it is over Hideout, and
            // one more as it leaves, nothing elsewhere on the screen.
            WM_MOVING if id != main_hwnd() => {
                if let Some(p) = cursor_pos() {
                    let near = main_window_on_screen()
                        && main_window_any_rect().is_some_and(|r| r.contains(p));
                    wake = near || k.drag_near;
                    k.drag_near = near;
                    k.drag = Some(crate::drag::WindowDrag {
                        hwnd: id,
                        cursor: p,
                        ended: false,
                    });
                }
            }
            WM_SYSCOMMAND => {
                let cmd = match wp.0 & 0xFFF0 {
                    MENU_KEEP_ON_TOP => Some(MenuCommand::KeepOnTop),
                    MENU_DOCK_BACK => Some(MenuCommand::DockBack),
                    MENU_QUIT => Some(MenuCommand::Quit),
                    _ => None,
                };
                if let Some(c) = cmd {
                    k.menu.push((id, c));
                    wake = true;
                    handled = true;
                }
            }
            WM_CONTEXTMENU if context_menu_from_keyboard(lp.0) => {
                k.menu.push((id, MenuCommand::MenuKey));
                wake = true;
                handled = true;
            }
            WM_NCDESTROY => {
                k.moving.retain(|&m| m != id);
                k.ended.retain(|&m| m != id);
                if k.drag.is_some_and(|d| d.hwnd == id) {
                    k.drag = None;
                }
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
    unsafe {
        let _ = SetWindowSubclass(hwnd(h), Some(popout_proc), SUBCLASS_ID, 0);
    }
    add_above_close(
        h,
        &[
            (MENU_KEEP_ON_TOP, w!("Keep on top")),
            (MENU_DOCK_BACK, w!("Dock back\tCtrl+Shift+O")),
        ],
    );
    check_menu(h, keep_on_top);
}

/// Hooks the main window: the same subclass, for the Menu key and the window menu's
/// **Quit Hideout** (#1012.5, #953).
pub fn install_main_hooks(h: isize) {
    use windows::core::w;
    use windows::Win32::UI::Shell::SetWindowSubclass;
    unsafe {
        let _ = SetWindowSubclass(hwnd(h), Some(popout_proc), SUBCLASS_ID, 0);
    }
    add_above_close(h, &[(MENU_QUIT, w!("Quit Hideout"))]);
}

/// Where Hideout's own window-menu items go (#953 F2): just above the separator before Close,
/// so Close stays last and the order is the same in every window. `ids` are the menu's item ids
/// in order (0 for a separator). With no Close, at the end.
fn above_close(ids: &[u32], close: u32) -> usize {
    match ids.iter().position(|&id| id == close) {
        Some(at) if at > 0 && ids[at - 1] == 0 => at - 1,
        Some(at) => at,
        None => ids.len(),
    }
}

/// Puts a separator and `items` into `h`'s window menu at [`above_close`].
fn add_above_close(h: isize, items: &[(usize, windows::core::PCWSTR)]) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, GetMenuItemID, GetSystemMenu, InsertMenuW, MF_BYPOSITION, MF_SEPARATOR,
        MF_STRING, SC_CLOSE,
    };
    unsafe {
        let menu = GetSystemMenu(hwnd(h), false);
        if menu.is_invalid() {
            return;
        }
        let ids: Vec<u32> = (0..GetMenuItemCount(Some(menu)).max(0))
            .map(|i| GetMenuItemID(menu, i))
            .collect();
        let at = above_close(&ids, SC_CLOSE);
        let _ = InsertMenuW(menu, at as u32, MF_BYPOSITION | MF_SEPARATOR, 0, None);
        for (n, (id, text)) in items.iter().enumerate() {
            let _ = InsertMenuW(
                menu,
                (at + 1 + n) as u32,
                MF_BYPOSITION | MF_STRING,
                *id,
                *text,
            );
        }
    }
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

// ---- Drags between windows (#1054.4) ---------------------------------------------------------
//
// egui 0.33 can't see a drag cross its windows (docs/hideout-popouts-ux.md, "Follow-ups as
// built"), so: a pop-out's subclass notes `WM_MOVING` and the pointer while Windows moves it,
// the main frame maps that onto the strip; a tab dragged out of the main window gets a ghost card
// in an extra always-on-top viewport moved with `SetWindowPos`. All of it runs on window
// messages and the frames a pointer move brings; nothing polls.

/// A pop-out being moved by its title row now, or its drop once the move ended (taken then).
pub fn take_window_drag() -> Option<crate::drag::WindowDrag> {
    HOOKS.with(|k| {
        let mut k = k.borrow_mut();
        let d = k.drag?;
        if d.ended {
            k.drag = None;
        }
        Some(d)
    })
}

/// The pointer on screen, physical pixels.
pub fn cursor_pos() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// The main window's client corner on screen, while it shows (visible, not minimised).
pub fn main_client_origin() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    let h = main_window()?;
    unsafe {
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return None;
        }
        let mut p = POINT::default();
        if !ClientToScreen(h, &mut p).as_bool() {
            return None;
        }
        Some((p.x, p.y))
    }
}

/// Every top-level window on screen, top of the z order first, for [`crate::drag::top_at`]:
/// shown means visible, not minimised and not cloaked (another virtual desktop, a suspended
/// app). Read once per step of a pop-out drag over Hideout, never on a timer.
pub fn window_layers() -> Vec<crate::drag::Layer> {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        let out = &mut *(data.0 as *mut Vec<crate::drag::Layer>);
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_err() {
            return true.into();
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        );
        let shown = IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() && cloaked == 0;
        out.push(crate::drag::Layer {
            id: h.0 as isize,
            rect: Rect {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            },
            shown,
        });
        true.into()
    }
    let mut out: Vec<crate::drag::Layer> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// How far below the window's top its client area starts (Windows' caption), in
/// device-independent units at the window's own DPI.
pub fn client_top_dip(h: isize) -> f64 {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    let h = hwnd(h);
    let mut r = RECT::default();
    let mut p = POINT::default();
    unsafe {
        if GetWindowRect(h, &mut r).is_err() || !ClientToScreen(h, &mut p).as_bool() {
            return 0.0;
        }
        let dpi = GetDpiForWindow(h).max(96) as f64;
        f64::from(p.y - r.top) * 96.0 / dpi
    }
}

/// The drag's always-on-top windows (#1054.4): the ghost card of a tab drag, the insertion bar
/// shown above a pop-out dragged over the strip (the window itself covers the strip's own bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Ghost = 0,
    Bar = 1,
}

/// Each overlay's window once found by its title; 0 while none.
static OVERLAYS: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];

/// Finds an overlay's window and makes it a tool window that never takes the focus (out of
/// Alt+Tab, no activation on show). Returns it.
pub fn find_overlay(which: Overlay, title: &str, skip: &[isize]) -> Option<isize> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    let h = find_own_window(title, skip)?;
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let _ = SetWindowLongPtrW(
            h,
            GWL_EXSTYLE,
            ex | (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize,
        );
    }
    OVERLAYS[which as usize].store(h.0 as isize, Ordering::Release);
    Some(h.0 as isize)
}

/// The overlays' windows that are up (0 for none): never a drop target.
pub fn overlay_hwnds() -> [isize; 2] {
    [
        OVERLAYS[0].load(Ordering::Acquire),
        OVERLAYS[1].load(Ordering::Acquire),
    ]
}

/// The overlay is done: its window goes with egui's next frame.
pub fn forget_overlay(which: Overlay) {
    OVERLAYS[which as usize].store(0, Ordering::Release);
}

/// Moves an overlay's corner to `at` (physical pixels), sized `size` when given, above every
/// window, shown without taking the focus.
pub fn place_overlay(h: isize, at: (i32, i32), size: Option<(i32, i32)>) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOPMOST, SWP_SHOWWINDOW};
    let (w, ht) = size.unwrap_or((0, 0));
    let keep = if size.is_none() {
        SWP_NOSIZE
    } else {
        Default::default()
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd(h),
            Some(HWND_TOPMOST),
            at.0,
            at.1,
            w,
            ht,
            keep | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

// ---- Wake messages (#1018) -------------------------------------------------------------------
//
// The wake report splits its `other` wakes by window message. Two hooks feed
// `wake::note_message`, both on the UI thread and both a fixed-table count (no allocation, no
// wake): winit's message hook sees each message the loop takes from the queue (posted ones,
// WM_TIMER, WM_PAINT), and a thread-local WH_CALLWNDPROC hook sees each message another thread
// sends (UI Automation's WM_GETOBJECT, DWM, the shell), which `PeekMessageW` delivers without
// returning it. Messages the thread sends itself are a frame's or a handler's own, not a wake.

/// For winit's `with_msg_hook`: counts the message and lets winit dispatch it as before.
pub fn note_queued(msg: *const std::ffi::c_void) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::MSG;
    // Safety: winit hands its own `MSG` from `PeekMessageW`, valid for the call.
    if let Some(m) = unsafe { (msg as *const MSG).as_ref() } {
        crate::wake::note_message(crate::wake::MsgKey::of(
            m.message,
            m.hwnd.0 as isize,
            m.wParam.0,
            false,
        ));
    }
    false
}

unsafe extern "system" fn sent_proc(
    code: i32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, InSendMessageEx, CWPSTRUCT, HC_ACTION, ISMEX_CALLBACK, ISMEX_NOTIFY,
        ISMEX_SEND,
    };
    // Another thread sent it. The hook's wParam can't say (Windows' own sends while this
    // thread creates a window come with 0, a cross-thread send with 1, measured); the receive
    // state can.
    let cross = ISMEX_SEND | ISMEX_NOTIFY | ISMEX_CALLBACK;
    if code == HC_ACTION as i32 && InSendMessageEx(None) & cross != 0 {
        if let Some(m) = (lp.0 as *const CWPSTRUCT).as_ref() {
            crate::wake::note_message(crate::wake::MsgKey::of(
                m.message,
                m.hwnd.0 as isize,
                m.wParam.0,
                true,
            ));
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

/// Installs the send hook on this (the UI) thread, for the process's life. A thread hook of the
/// process's own: no DLL, no admin, no other process touched.
pub fn install_message_hooks() {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_CALLWNDPROC};
    let hook =
        unsafe { SetWindowsHookExW(WH_CALLWNDPROC, Some(sent_proc), None, GetCurrentThreadId()) };
    if let Err(e) = hook {
        log::warn!("wake message hook failed: {e}");
    }
}

/// The name of a window message id, for the ones a hub's idle wakes plausibly carry.
pub fn message_name(id: u32) -> Option<&'static str> {
    Some(match id {
        0x0000 => "WM_NULL",
        0x0002 => "WM_DESTROY",
        0x0006 => "WM_ACTIVATE",
        0x0007 => "WM_SETFOCUS",
        0x0008 => "WM_KILLFOCUS",
        0x000F => "WM_PAINT",
        0x0010 => "WM_CLOSE",
        0x0011 => "WM_QUERYENDSESSION",
        0x0014 => "WM_ERASEBKGND",
        0x0015 => "WM_SYSCOLORCHANGE",
        0x0016 => "WM_ENDSESSION",
        0x0018 => "WM_SHOWWINDOW",
        0x001A => "WM_SETTINGCHANGE",
        0x001C => "WM_ACTIVATEAPP",
        0x001D => "WM_FONTCHANGE",
        0x001E => "WM_TIMECHANGE",
        0x0020 => "WM_SETCURSOR",
        0x0021 => "WM_MOUSEACTIVATE",
        0x0024 => "WM_GETMINMAXINFO",
        0x003D => "WM_GETOBJECT",
        0x0046 => "WM_WINDOWPOSCHANGING",
        0x0047 => "WM_WINDOWPOSCHANGED",
        0x004A => "WM_COPYDATA",
        0x007E => "WM_DISPLAYCHANGE",
        0x007F => "WM_GETICON",
        0x0083 => "WM_NCCALCSIZE",
        0x0084 => "WM_NCHITTEST",
        0x0085 => "WM_NCPAINT",
        0x0086 => "WM_NCACTIVATE",
        0x0088 => "WM_SYNCPAINT",
        0x00A0 => "WM_NCMOUSEMOVE",
        0x00FE => "WM_INPUT_DEVICE_CHANGE",
        0x00FF => "WM_INPUT",
        0x0100 => "WM_KEYDOWN",
        0x0101 => "WM_KEYUP",
        0x0102 => "WM_CHAR",
        0x0104 => "WM_SYSKEYDOWN",
        0x0105 => "WM_SYSKEYUP",
        0x0111 => "WM_COMMAND",
        0x0112 => "WM_SYSCOMMAND",
        0x0113 => "WM_TIMER",
        0x0118 => "WM_SYSTIMER",
        0x0200 => "WM_MOUSEMOVE",
        0x0201 => "WM_LBUTTONDOWN",
        0x0202 => "WM_LBUTTONUP",
        0x020A => "WM_MOUSEWHEEL",
        0x0218 => "WM_POWERBROADCAST",
        0x0219 => "WM_DEVICECHANGE",
        0x0281 => "WM_IME_SETCONTEXT",
        0x0282 => "WM_IME_NOTIFY",
        0x0283 => "WM_IME_CONTROL",
        0x02A1 => "WM_MOUSEHOVER",
        0x02A2 => "WM_NCMOUSELEAVE",
        0x02A3 => "WM_MOUSELEAVE",
        0x02B1 => "WM_WTSSESSION_CHANGE",
        0x02E0 => "WM_DPICHANGED",
        0x031A => "WM_THEMECHANGED",
        0x031E => "WM_DWMCOMPOSITIONCHANGED",
        0x031F => "WM_DWMNCRENDERINGCHANGED",
        0x0320 => "WM_DWMCOLORIZATIONCOLORCHANGED",
        0x0321 => "WM_DWMWINDOWMAXIMIZEDCHANGE",
        0x0323 => "WM_DWMSENDICONICTHUMBNAIL",
        0x0326 => "WM_DWMSENDICONICLIVEPREVIEWBITMAP",
        _ => return None,
    })
}

/// A message kind's text for the wake report: `0x0113 WM_TIMER#6007 tray`. Registered messages
/// (0xC000 and up) by their registered name, the WM_USER and WM_APP ranges as an offset; the
/// window as `main`, `tray`, `flyout`, `thread` (a thread message) or its class name. Runs once
/// per reported kind, never per message.
pub fn message_label(key: crate::wake::MsgKey) -> String {
    use windows::Win32::System::DataExchange::GetClipboardFormatNameW;
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, IsWindow};
    let id = key.msg & !crate::wake::SENT;
    let name = match message_name(id) {
        Some(n) => n.to_string(),
        None if id >= 0xC000 => {
            let mut buf = [0u16; 128];
            let n = unsafe { GetClipboardFormatNameW(id, &mut buf) }.max(0) as usize;
            if n > 0 {
                String::from_utf16_lossy(&buf[..n])
            } else {
                "registered".to_string()
            }
        }
        None if id >= 0x8000 => format!("WM_APP+{}", id - 0x8000),
        None if id >= 0x0400 => format!("WM_USER+{}", id - 0x0400),
        None => "?".to_string(),
    };
    let window = match key.hwnd {
        0 => "thread".to_string(),
        h if h == main_hwnd() => "main".to_string(),
        h if h == crate::tray::message_window() => "tray".to_string(),
        h if h == flyout_hwnd() => "flyout".to_string(),
        h => {
            let mut buf = [0u16; 64];
            let n = if unsafe { IsWindow(Some(hwnd(h))) }.as_bool() {
                unsafe { GetClassNameW(hwnd(h), &mut buf) }.max(0) as usize
            } else {
                0
            };
            if n > 0 {
                format!("'{}'", String::from_utf16_lossy(&buf[..n]))
            } else {
                "gone".to_string()
            }
        }
    };
    let mut s = format!("0x{id:04X} {name}");
    if key.arg != 0 {
        s.push_str(&format!("#{}", key.arg));
    }
    s.push(' ');
    s.push_str(&window);
    if key.msg & crate::wake::SENT != 0 {
        s.push_str(" sent");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hideouts_menu_items_go_just_above_closes_separator() {
        const CLOSE: u32 = 0xF060;
        // Restore, Move, Size, Minimize, Maximize, separator, Close: before the separator.
        let menu = [0xF120, 0xF010, 0xF000, 0xF020, 0xF030, 0, CLOSE];
        assert_eq!(above_close(&menu, CLOSE), 5);
        assert_eq!(
            above_close(&[0xF010, CLOSE], CLOSE),
            1,
            "no separator: above Close"
        );
        assert_eq!(above_close(&[0xF010], CLOSE), 1, "no Close: at the end");
        assert_eq!(above_close(&[CLOSE], CLOSE), 0);
    }

    #[test]
    fn a_held_position_change_neither_shows_nor_activates() {
        use windows::Win32::UI::WindowsAndMessaging::{SWP_NOMOVE, SWP_SHOWWINDOW};
        let f = held_flags(SWP_SHOWWINDOW.0 | SWP_NOMOVE.0);
        assert_eq!(f & SWP_SHOWWINDOW.0, 0);
        assert_ne!(f & SWP_NOACTIVATE.0, 0);
        assert_ne!(f & SWP_NOMOVE.0, 0, "the rest kept");
    }

    /// A tray start never shows the main window (#1012): eframe's show after the first frame,
    /// and any other show, leave a held window hidden. The window is a tool window far off
    /// screen that never activates, so even a failing run puts nothing on the desktop.
    /// It holds through its own cell: other tests call `show_main_window`, which lets go of
    /// the process-wide hold at any moment (#1030).
    #[test]
    fn a_held_window_stays_hidden_when_shown() {
        use windows::core::w;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos,
            SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNORMAL, WNDCLASSW, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW,
        };
        unsafe extern "system" fn proc_(
            h: HWND,
            m: u32,
            wp: windows::Win32::Foundation::WPARAM,
            lp: LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            DefWindowProcW(h, m, wp, lp)
        }
        unsafe {
            let inst = GetModuleHandleW(None).unwrap();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: inst.into(),
                lpszClassName: w!("HideoutHoldTest"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let h = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("HideoutHoldTest"),
                w!("hold test"),
                WS_OVERLAPPEDWINDOW,
                -32000,
                -32000,
                200,
                100,
                None,
                None,
                Some(inst.into()),
                None,
            )
            .unwrap();
            static HELD: AtomicIsize = AtomicIsize::new(0);
            hold_in(&HELD, h.0 as isize);
            // winit's `set_visible(true)` and a plain show.
            let _ = ShowWindow(h, SW_SHOW);
            let _ = ShowWindow(h, SW_SHOWNORMAL);
            let _ = SetWindowPos(
                h,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            assert!(!IsWindowVisible(h).as_bool(), "held: never shown");
            HELD.store(0, Ordering::Release);
            let_go_hidden();
            assert_eq!(
                HELD_HIDDEN.load(Ordering::Acquire),
                0,
                "let go for the first show"
            );
            let _ = DestroyWindow(h);
        }
    }

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
