//! The Settings window as a window of its own (an egui immediate viewport) instead of a panel
//! drawn inside the full-screen overlay. Like any app window it can be dragged to any display by
//! its title bar, and nothing behind it is dimmed or blocked. It opens where it was last left:
//! the same place on the same display, or centred on that display (or the one under the mouse,
//! if that one is gone too) when the old place is no longer on any screen. The first time, it
//! opens compact: [`DEFAULT_SIZE`], never more than [`MAX_SHARE`] of the display, centred on the
//! display under the mouse (else the main one).
//!
//! The overlay stays one topmost, click-through window on its display; only the keymap editor
//! still draws there. Where the window is, is read and set in physical pixels through Win32
//! ([`NativeWindow`]) but remembered in points (logical pixels) of its display, so it keeps its
//! size on displays with other scaling (here a 100 % and a 150 % one). The logic deciding it
//! ([`restore`], [`Memory`]) is plain and tested without a window.
//!
//! Crossing to a display with other scaling (WM_DPICHANGED), the window takes the rectangle
//! Windows suggests, which keeps its size in points and the pointer on the title bar; winit's own
//! rectangle differs from it, and Windows' move loop then pulls the window back over the border,
//! rescaling it back and forth. Its smallest size follows the scaling too. Placing writes
//! a size only once per opening, never in answer to a resize, so no resize can feed back.
//!
//! Minimizing it is closing it (the app lives in the tray): it saves its place and goes, the
//! overlay's preview and drag end with it, and the next opening is at its place, not minimized.

use serde::{Deserialize, Serialize};

use crate::monitors::Monitor;

/// The window's title in its title bar and the taskbar (#535).
pub const TITLE: &str = "Foxglove";
/// The title the window is made with, until the Win32 side has found it by this title and
/// renamed it to [`TITLE`]. The overlay's own window is titled "Foxglove" (eframe's app name),
/// so finding the settings window by [`TITLE`] could hook the overlay instead; this name is
/// the settings window's alone. The window starts hidden, so the rename lands before it shows.
pub const LOOKUP_TITLE: &str = "Foxglove Settings";
/// eframe's app name, which titles the overlay's window (`platform/eframe_host.rs`).
pub const OVERLAY_TITLE: &str = "Foxglove";
/// The size it first opens at, in points: compact but readable, room for the sidebar and a
/// page at the normal text size (#385). A remembered size wins over it.
pub const DEFAULT_SIZE: egui::Vec2 = egui::vec2(880.0, 600.0);
/// The smallest size, in points: sidebar labels and status chips still fit (#443's tests).
pub const MIN_SIZE: egui::Vec2 = egui::vec2(720.0, 420.0);
/// The first opening takes at most this share of the display's work area, each way.
pub const MAX_SHARE: f32 = 0.7;
/// The store section with the last place.
const SECTION: &str = "settings_window";
/// How much of the title bar, in pixels, must be on a display for the old place to be used.
const GRIP: [i32; 2] = [120, 24];

pub fn id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("foxglove-settings")
}

/// The native window's description. It starts hidden on Windows, so it never shows at the
/// default place before it's moved to the remembered one.
pub fn builder() -> egui::ViewportBuilder {
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    let icon = ICON.get_or_init(|| {
        std::sync::Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!("../../resources/icon.png"))
                .unwrap_or_default(),
        )
    });
    egui::ViewportBuilder::default()
        .with_title(LOOKUP_TITLE)
        .with_inner_size(DEFAULT_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_icon(icon.clone())
        .with_visible(!cfg!(target_os = "windows"))
}

/// Where the window was last: the display it was mostly on and its outer rectangle as left, top
/// (from that display's work-area corner), width and height, in points of that display.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub monitor: String,
    pub place: Option<[f32; 4]>,
    /// The first version's (#439) outer rectangle in physical desktop pixels: read while there's
    /// no `place` yet, never written.
    #[serde(skip_serializing)]
    pub rect: Option<[i32; 4]>,
}

/// Physical pixels per point at a DPI (96 = 100 %).
pub fn scale_of(dpi: u32) -> f32 {
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// A window rectangle (physical desktop pixels) as a place on `monitor`: its corner in that
/// display's points, its size in points at the window's own scaling `dpi`.
pub fn to_points(rect: [i32; 4], monitor: &Monitor, dpi: u32) -> [f32; 4] {
    let s = monitor.scale();
    let w = scale_of(dpi);
    [
        (rect[0] - monitor.work_area[0]) as f32 / s,
        (rect[1] - monitor.work_area[1]) as f32 / s,
        (rect[2] - rect[0]) as f32 / w,
        (rect[3] - rect[1]) as f32 / w,
    ]
}

/// A place on `monitor` as a window rectangle in physical desktop pixels.
pub fn to_physical(place: [f32; 4], monitor: &Monitor) -> [i32; 4] {
    let s = monitor.scale();
    let px = |v: f32| (v * s).round() as i32;
    let left = monitor.work_area[0] + px(place[0]);
    let top = monitor.work_area[1] + px(place[1]);
    [left, top, left + px(place[2]), top + px(place[3])]
}

/// The size of the overlap of two rectangles: width, height (0 when apart).
fn overlap(a: [i32; 4], b: [i32; 4]) -> [i32; 2] {
    [
        (a[2].min(b[2]) - a[0].max(b[0])).max(0),
        (a[3].min(b[3]) - a[1].max(b[1])).max(0),
    ]
}

/// `size` centred in `area`, shrunk to fit it.
fn centred(size: [i32; 2], area: [i32; 4]) -> [i32; 4] {
    let w = size[0].min(area[2] - area[0]).max(1);
    let h = size[1].min(area[3] - area[1]).max(1);
    let left = area[0] + (area[2] - area[0] - w) / 2;
    let top = area[1] + (area[3] - area[1] - h) / 2;
    [left, top, left + w, top + h]
}

/// Enough of the title bar is on a display for it to be grabbed.
fn reachable(r: [i32; 4], monitors: &[Monitor]) -> bool {
    let grip = [r[0], r[1], r[2], r[1] + GRIP[1].min(r[3] - r[1])];
    let need = [GRIP[0].min(r[2] - r[0]), GRIP[1].min(r[3] - r[1])];
    monitors.iter().any(|m| {
        let o = overlap(grip, m.work_area);
        o[0] >= need[0] && o[1] >= need[1]
    })
}

/// The display a desktop point is on, by work area, else by whole-pixel distance to it ("" when
/// no display is known), so a pointer over the taskbar still counts.
pub fn monitor_at(point: [i32; 2], monitors: &[Monitor]) -> String {
    let dist = |m: &Monitor| {
        let a = m.work_area;
        let dx = (a[0] - point[0]).max(point[0] - (a[2] - 1)).max(0);
        let dy = (a[1] - point[1]).max(point[1] - (a[3] - 1)).max(0);
        i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
    };
    monitors
        .iter()
        .min_by_key(|m| dist(m))
        .map(|m| m.id.clone())
        .unwrap_or_default()
}

/// A first-opening size in points: `size`, at most [`MAX_SHARE`] of `monitor`'s work area each
/// way (but not below [`MIN_SIZE`]; the work area itself is the last limit, in [`centred`]).
pub fn compact(size: [f32; 2], monitor: &Monitor) -> [f32; 2] {
    let s = monitor.scale();
    let area = monitor.work_area;
    let cap = |v: f32, px: i32, min: f32| v.min((px as f32 / s * MAX_SHARE).max(min));
    [
        cap(size[0], area[2] - area[0], MIN_SIZE.x),
        cap(size[1], area[3] - area[1], MIN_SIZE.y),
    ]
}

/// Where to open the window, in physical desktop pixels. The saved place when enough of its
/// title bar is on a display (so it can still be grabbed); otherwise its remembered size
/// centred on the saved display, or on `fallback` (the display under the mouse, else the main
/// one) when that display is gone. With no remembered size, `size` in points made
/// [`compact`]. `None` when no display is known: leave it where Windows put it.
pub fn restore(
    saved: &Saved,
    monitors: &[Monitor],
    size: [f32; 2],
    fallback: &str,
) -> Option<[i32; 4]> {
    let home = monitors
        .iter()
        .find(|m| !saved.monitor.is_empty() && m.id == saved.monitor);
    let kept = match (saved.place, home) {
        (Some(place), Some(m)) => Some(to_physical(place, m)),
        (Some(_), None) => None,
        (None, _) => saved.rect,
    };
    if let Some(r) = kept.filter(|r| reachable(*r, monitors)) {
        return Some(r);
    }
    let monitor = home
        .cloned()
        .or_else(|| crate::monitors::find(monitors, fallback))
        .or_else(|| monitors.first().cloned())?;
    let size = saved
        .place
        .map(|p| [p[2], p[3]])
        .or_else(|| {
            let s = home.map_or(1.0, Monitor::scale);
            saved
                .rect
                .map(|r| [(r[2] - r[0]) as f32 / s, (r[3] - r[1]) as f32 / s])
        })
        .unwrap_or_else(|| compact(size, &monitor));
    let s = monitor.scale();
    let px = |v: f32| (v * s).round() as i32;
    Some(centred([px(size[0]), px(size[1])], monitor.work_area))
}

/// The display a rectangle is mostly on ("" when it's on none).
pub fn monitor_of(rect: [i32; 4], monitors: &[Monitor]) -> String {
    monitors
        .iter()
        .map(|m| {
            let o = overlap(rect, m.work_area);
            (i64::from(o[0]) * i64::from(o[1]), m)
        })
        .filter(|(area, _)| *area > 0)
        .max_by_key(|(area, _)| *area)
        .map(|(_, m)| m.id.clone())
        .unwrap_or_default()
}

/// The smallest outer size, in physical pixels, at a DPI: [`MIN_SIZE`] in points plus the
/// frame (`frame`, the title bar and borders at that DPI). egui gives winit the smallest size
/// once, in the pixels of the display the window was made on; on a display with other scaling
/// that would be too small or too big, and too big clamps the rectangle Windows suggests.
pub fn min_track_size(dpi: u32, frame: [i32; 2]) -> [i32; 2] {
    let s = scale_of(dpi);
    [
        (MIN_SIZE.x * s).ceil() as i32 + frame[0],
        (MIN_SIZE.y * s).ceil() as i32 + frame[1],
    ]
}

/// The window message asks to minimize it (title-bar button, taskbar, window menu), which
/// closes it instead.
pub fn minimize_closes(message: u32, wparam: usize) -> bool {
    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_MINIMIZE: usize = 0xF020;
    message == WM_SYSCOMMAND && wparam & 0xFFF0 == SC_MINIMIZE
}

/// The layer overlay's keys (window points) as the overlay window's hit region while Settings
/// is open: window pixels, rounded outward, square corners.
pub fn keys_region(rect: egui::Rect, ppp: f32) -> [i32; 5] {
    [
        (rect.left() * ppp).floor() as i32,
        (rect.top() * ppp).floor() as i32,
        (rect.right() * ppp).ceil() as i32,
        (rect.bottom() * ppp).ceil() as i32,
        0,
    ]
}

/// The open window, as far as placing it goes.
pub trait NativeWindow {
    /// Its outer rectangle in physical pixels; `None` while minimized or maximized (those aren't
    /// places to come back to).
    fn rect(&self) -> Option<[i32; 4]>;
    /// Its scaling as a DPI (that of the display Windows counts it on).
    fn dpi(&self) -> u32;
    /// Moves and sizes it (physical pixels).
    fn move_to(&mut self, rect: [i32; 4]);
    /// Shows and focuses it, once it's in place.
    fn show(&mut self);
}

/// Where the window goes and where it has been, between opening and closing.
#[derive(Debug, Default)]
pub struct Memory {
    saved: Saved,
    placed: bool,
    /// The last rectangle seen and the window's DPI then.
    last: Option<([i32; 4], u32)>,
}

impl Memory {
    pub fn new(saved: Saved) -> Self {
        Self {
            saved,
            ..Self::default()
        }
    }

    /// A frame with the window open: the first one puts it in its place and shows it, the
    /// others only note where it is now. It never moves or sizes the window after that, so a
    /// resize (a scaling change, the user) can't start another one.
    pub fn frame(&mut self, window: &mut dyn NativeWindow, monitors: &[Monitor], fallback: &str) {
        if !self.placed {
            self.placed = true;
            let s = scale_of(window.dpi());
            let size = window
                .rect()
                .map(|r| [(r[2] - r[0]) as f32 / s, (r[3] - r[1]) as f32 / s])
                .unwrap_or([DEFAULT_SIZE.x, DEFAULT_SIZE.y]);
            if let Some(to) = restore(&self.saved, monitors, size, fallback) {
                window.move_to(to);
            }
            window.show();
            return;
        }
        if let Some(r) = window.rect() {
            self.last = Some((r, window.dpi()));
        }
    }

    /// What to save now (the window lost the focus, or is closing), if it moved since the last
    /// save. Nothing while it's on no display.
    pub fn due_save(&mut self, monitors: &[Monitor]) -> Option<Saved> {
        let (r, dpi) = self.last?;
        let id = monitor_of(r, monitors);
        let monitor = monitors.iter().find(|m| m.id == id)?;
        let saved = Saved {
            monitor: id,
            place: Some(to_points(r, monitor, dpi)),
            rect: None,
        };
        if saved == self.saved {
            return None;
        }
        self.saved = saved.clone();
        Some(saved)
    }

    /// The window closed: what to save, and it's placed again when it next opens.
    pub fn closed(&mut self, monitors: &[Monitor]) -> Option<Saved> {
        let save = self.due_save(monitors);
        self.placed = false;
        self.last = None;
        save
    }
}

/// The Settings window's state in the app: its memory and, on Windows, its native handle.
pub struct SettingsWindow {
    memory: Memory,
    focused: bool,
    theme: Option<egui::Theme>,
    #[cfg(target_os = "windows")]
    hwnd: Option<isize>,
    /// The handle wasn't found this opening (said once in the log).
    #[cfg(target_os = "windows")]
    lost: bool,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self {
            memory: Memory::new(super::store::try_load(SECTION).unwrap_or_default()),
            focused: false,
            theme: None,
            #[cfg(target_os = "windows")]
            hwnd: None,
            #[cfg(target_os = "windows")]
            lost: false,
        }
    }
}

fn save(saved: Option<Saved>) {
    if let Some(saved) = saved {
        if let Err(e) = super::store::save(SECTION, &saved) {
            eprintln!("Foxglove: can't save where the Settings window is: {e}");
        }
    }
}

impl SettingsWindow {
    /// Runs inside the window's viewport each frame: places it on its first frame, keeps its
    /// title bar in the app's theme, and saves its place when it loses the focus. With no
    /// place of its own yet it opens on the display under the mouse (else the main one). True
    /// when it should close: it was minimized (by a path the window hook doesn't catch, e.g.
    /// Win+M).
    pub fn frame(&mut self, ctx: &egui::Context) -> bool {
        let theme = ctx.theme();
        if self.theme != Some(theme) {
            self.theme = Some(theme);
            // The native title bar follows the app's theme, not the system's.
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match theme {
                egui::Theme::Dark => egui::SystemTheme::Dark,
                egui::Theme::Light => egui::SystemTheme::Light,
            }));
        }
        #[cfg(target_os = "windows")]
        {
            if self.hwnd.is_none() {
                self.hwnd = imp::find(LOOKUP_TITLE);
                if let Some(hwnd) = self.hwnd {
                    imp::hook(hwnd);
                    // Found by its own name: now it can take the app's.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(TITLE.into()));
                }
            }
            let monitors = crate::monitors::list();
            // Read only while it isn't placed yet: the first frame of an opening.
            let under_mouse = if self.memory.placed {
                String::new()
            } else {
                imp::cursor()
                    .map(|p| monitor_at(p, &monitors))
                    .unwrap_or_default()
            };
            let overlay_monitor = under_mouse.as_str();
            match self.hwnd {
                Some(hwnd) => {
                    let mut window = imp::Win32Window { hwnd, ctx };
                    self.memory.frame(&mut window, &monitors, overlay_monitor);
                }
                None => {
                    // Not found (it should be): show it where Windows put it.
                    if !self.lost {
                        self.lost = true;
                        eprintln!("Foxglove: can't find the Settings window to place it");
                    }
                    self.memory
                        .frame(&mut Shown(ctx), &monitors, overlay_monitor);
                }
            }
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if self.focused && !focused {
                save(self.memory.due_save(&monitors));
            }
            self.focused = focused;
        }
        ctx.input(|i| i.viewport().minimized) == Some(true)
    }

    /// The window closed: remember where it was, and end an open colour calibrator.
    pub fn closed(&mut self) {
        super::calibrate_ui::cancel();
        super::led_map_ui::cancel();
        save(self.memory.closed(&crate::monitors::list()));
        self.focused = false;
        self.theme = None;
        #[cfg(target_os = "windows")]
        {
            self.hwnd = None;
            self.lost = false;
        }
    }
}

/// A window that can only be shown (no handle to place it with).
#[cfg(target_os = "windows")]
struct Shown<'a>(&'a egui::Context);

#[cfg(target_os = "windows")]
impl NativeWindow for Shown<'_> {
    fn rect(&self) -> Option<[i32; 4]> {
        None
    }
    fn dpi(&self) -> u32 {
        96
    }
    fn move_to(&mut self, _: [i32; 4]) {}
    fn show(&mut self) {
        imp::show(self.0);
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GetWindowLongW, GetWindowRect, GetWindowTextW, IsIconic, IsZoomed,
        PostMessageW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, MINMAXINFO, SWP_NOACTIVATE, SWP_NOSIZE,
        SWP_NOZORDER, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DPICHANGED, WM_GETMINMAXINFO,
        WM_NCDESTROY,
    };

    /// The mouse pointer in physical desktop pixels, for where a first opening goes.
    pub fn cursor() -> Option<[i32; 2]> {
        let mut p = POINT::default();
        // SAFETY: GetCursorPos writes the POINT we own; nothing is kept.
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) }.ok()?;
        Some([p.x, p.y])
    }

    /// This thread's top-level window titled `title` (the viewport's window, made by winit on
    /// the UI thread). Found once per opening.
    pub fn find(title: &str) -> Option<isize> {
        struct Search {
            title: Vec<u16>,
            found: Option<isize>,
        }
        unsafe extern "system" fn each(window: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the `Search` below, alive for the whole enumeration.
            let search = unsafe { &mut *(data.0 as *mut Search) };
            let mut buf = [0u16; 64];
            // SAFETY: `window` comes from the enumeration; the buffer is ours.
            let len = unsafe { GetWindowTextW(window, &mut buf) };
            if usize::try_from(len).ok() == Some(search.title.len())
                && buf[..search.title.len()] == search.title[..]
            {
                search.found = Some(window.0 as isize);
                return false.into();
            }
            true.into()
        }
        let mut search = Search {
            title: title.encode_utf16().collect(),
            found: None,
        };
        // SAFETY: the callback only reads window titles into its own buffer and writes into
        // `search`, which outlives the call.
        unsafe {
            let _ = EnumThreadWindows(
                GetCurrentThreadId(),
                Some(each),
                LPARAM(&mut search as *mut Search as isize),
            );
        }
        search.found
    }

    /// Our subclass of the window (any number; one per window).
    const HOOK: usize = 0x466F_7853;

    /// Hooks the window's messages (see `proc`). Once per opening; the hook removes itself when
    /// the window is destroyed. Setting it again on the same window only replaces it.
    pub fn hook(hwnd: isize) {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: the window belongs to this (the UI) thread, as subclassing requires.
        if !unsafe { SetWindowSubclass(hwnd, Some(proc), HOOK, 0) }.as_bool() {
            eprintln!("Foxglove: can't hook the Settings window (scaling, minimize)");
        }
    }

    /// The frame (title bar and borders) at a DPI: what the outer size adds to the inner one.
    fn frame(hwnd: HWND, dpi: u32) -> [i32; 2] {
        let mut r = RECT::default();
        // SAFETY: plain queries on a live window; `r` is ours.
        unsafe {
            let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
            let ex = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
            if AdjustWindowRectExForDpi(&mut r, style, false, ex, dpi).is_err() {
                return [0, 0];
            }
        }
        [r.right - r.left, r.bottom - r.top]
    }

    /// The Settings window's messages, before winit's:
    /// - minimize: closes it instead (the app lives in the tray);
    /// - WM_DPICHANGED: winit takes the new scaling, then the window takes Windows' suggested
    ///   rectangle (same size in points, the pointer kept on the title bar, on the new display),
    ///   so the move loop and the window agree and it doesn't flip back across the border;
    /// - WM_GETMINMAXINFO: the smallest size at the window's current scaling.
    ///
    /// Only Windows' own messages drive these; none of them writes the size in answer to a
    /// resize of ours.
    unsafe extern "system" fn proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        // SAFETY (whole body): Windows calls this on the window's own thread with valid
        // arguments for `message`; the pointers in `lparam` are valid for the call.
        unsafe {
            if super::minimize_closes(message, wparam.0) {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            match message {
                WM_DPICHANGED => {
                    let result = DefSubclassProc(hwnd, message, wparam, lparam);
                    let r = *(lparam.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    result
                }
                WM_GETMINMAXINFO => {
                    let result = DefSubclassProc(hwnd, message, wparam, lparam);
                    let dpi = GetDpiForWindow(hwnd);
                    if dpi > 0 {
                        let [x, y] = super::min_track_size(dpi, frame(hwnd, dpi));
                        let info = &mut *(lparam.0 as *mut MINMAXINFO);
                        info.ptMinTrackSize = POINT { x, y };
                    }
                    result
                }
                WM_NCDESTROY => {
                    let _ = RemoveWindowSubclass(hwnd, Some(proc), HOOK);
                    DefSubclassProc(hwnd, message, wparam, lparam)
                }
                _ => DefSubclassProc(hwnd, message, wparam, lparam),
            }
        }
    }

    pub fn show(ctx: &egui::Context) {
        // Through winit, so it knows the window is visible.
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub struct Win32Window<'a> {
        pub hwnd: isize,
        pub ctx: &'a egui::Context,
    }

    impl Win32Window<'_> {
        fn hwnd(&self) -> HWND {
            HWND(self.hwnd as *mut core::ffi::c_void)
        }
    }

    impl super::NativeWindow for Win32Window<'_> {
        fn rect(&self) -> Option<[i32; 4]> {
            let hwnd = self.hwnd();
            // SAFETY: plain queries on the Settings window, alive while its viewport is shown.
            unsafe {
                if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
                    return None;
                }
                let mut r = RECT::default();
                GetWindowRect(hwnd, &mut r).ok()?;
                Some([r.left, r.top, r.right, r.bottom])
            }
        }

        fn dpi(&self) -> u32 {
            // SAFETY: as above.
            match unsafe { GetDpiForWindow(self.hwnd()) } {
                0 => 96,
                dpi => dpi,
            }
        }

        fn move_to(&mut self, [left, top, right, bottom]: [i32; 4]) {
            let hwnd = self.hwnd();
            // SAFETY: as above. Two steps, as for the overlay: arriving on a display with other
            // scaling rescales the window (WM_DPICHANGED), so move it there first, then size it.
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    left,
                    top,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    left,
                    top,
                    right - left,
                    bottom - top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        fn show(&mut self) {
            show(self.ctx);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// The window says "Foxglove" (#535), but it's found by a name the overlay's window can't
    /// have: the lookup can't hook the overlay, whose window eframe titles "Foxglove" too.
    #[test]
    fn the_settings_window_is_found_by_a_name_of_its_own() {
        assert_eq!(TITLE, "Foxglove");
        assert_ne!(LOOKUP_TITLE, OVERLAY_TITLE);
        assert_ne!(LOOKUP_TITLE, TITLE);
        assert_eq!(
            builder().title.as_deref(),
            Some(LOOKUP_TITLE),
            "made with the lookup name, renamed once found"
        );
    }

    const D1: &str = r"\\.\DISPLAY1";
    const D2: &str = r"\\.\DISPLAY2";

    /// This PC's displays: the main one at 100 %, and a 4K one at 150 % to its right.
    pub fn two_displays() -> Vec<Monitor> {
        vec![
            Monitor {
                id: D1.into(),
                label: "Display 1 (2560x1440) - main".into(),
                primary: true,
                work_area: [0, 0, 2560, 1392],
                dpi: 96,
            },
            Monitor {
                id: D2.into(),
                label: "Display 2 (3840x2160)".into(),
                primary: false,
                work_area: [2560, 0, 6400, 2088],
                dpi: 144,
            },
        ]
    }

    /// A window that scales like a per-monitor-aware one under Windows: its DPI is that of the
    /// display it's mostly on, and crossing to another scaling takes Windows' suggested
    /// rectangle (the same size in points, the left edge kept).
    #[derive(Default)]
    struct Fake {
        rect: Option<[i32; 4]>,
        dpi: u32,
        shown: bool,
        moves: Vec<[i32; 4]>,
    }

    impl Fake {
        fn at(rect: [i32; 4], dpi: u32) -> Self {
            Self {
                rect: Some(rect),
                dpi,
                ..Self::default()
            }
        }

        /// Windows moves it (a drag, or `move_to`): it rescales when its display's scaling
        /// differs. Returns how often it rescaled.
        fn windows_moves(&mut self, to: [i32; 4], monitors: &[Monitor]) -> usize {
            let mut r = to;
            let mut rescales = 0;
            // Each rescale is one WM_DPICHANGED; a flip back and forth would show as many.
            for _ in 0..8 {
                let id = monitor_of(r, monitors);
                let Some(m) = monitors.iter().find(|m| m.id == id) else {
                    break;
                };
                let dpi = NativeWindow::dpi(self);
                if m.dpi == dpi {
                    break;
                }
                let k = m.dpi as f32 / dpi as f32;
                let w = ((r[2] - r[0]) as f32 * k).round() as i32;
                let h = ((r[3] - r[1]) as f32 * k).round() as i32;
                r = [r[0], r[1], r[0] + w, r[1] + h];
                self.dpi = m.dpi;
                rescales += 1;
            }
            self.rect = Some(r);
            rescales
        }
    }

    impl NativeWindow for Fake {
        fn rect(&self) -> Option<[i32; 4]> {
            self.rect
        }
        fn dpi(&self) -> u32 {
            if self.dpi == 0 {
                96
            } else {
                self.dpi
            }
        }
        fn move_to(&mut self, rect: [i32; 4]) {
            self.moves.push(rect);
            // As `Win32Window::move_to`: move (maybe rescaling), then size exactly.
            self.windows_moves(rect, &two_displays());
            self.rect = Some(rect);
        }
        fn show(&mut self) {
            self.shown = true;
        }
    }

    fn place(monitor: &str, place: [f32; 4]) -> Saved {
        Saved {
            monitor: monitor.into(),
            place: Some(place),
            rect: None,
        }
    }

    #[test]
    fn a_saved_place_on_a_display_is_kept_in_its_pixels() {
        let d = two_displays();
        // 440 points from the 150 % display's corner, 1040 x 660 points: 1.5 times that.
        let saved = place(D2, [440.0, 200.0, 1040.0, 660.0]);
        assert_eq!(
            restore(&saved, &d, [1.0, 1.0], ""),
            Some([2560 + 660, 300, 2560 + 660 + 1560, 300 + 990])
        );
        // The same place on the 100 % display: the same points, as pixels.
        let main = place(D1, [440.0, 200.0, 1040.0, 660.0]);
        assert_eq!(
            restore(&main, &d, [1.0, 1.0], ""),
            Some([440, 200, 1480, 860])
        );
        // Half off the right edge, but the title bar can still be grabbed: the user's choice.
        let edge = place(D2, [2400.0, 100.0, 1040.0, 660.0]);
        assert_eq!(restore(&edge, &d, [1.0, 1.0], "").map(|r| r[0]), Some(6160));
    }

    #[test]
    fn a_window_on_a_display_that_is_gone_comes_back_centred_at_its_size_in_points() {
        let saved = place(D2, [440.0, 200.0, 1040.0, 660.0]);
        let main_only = &two_displays()[..1];
        // 1040 x 660 points on the 100 % main display, centred.
        assert_eq!(
            restore(&saved, main_only, [1.0, 1.0], D2),
            Some([760, 366, 1800, 1026])
        );
    }

    #[test]
    fn a_window_off_every_screen_goes_back_to_its_display() {
        // The display is still there but the layout moved it: centred on that display.
        let saved = place(D2, [5000.0, 200.0, 1040.0, 660.0]);
        assert_eq!(
            restore(&saved, &two_displays(), [1.0, 1.0], ""),
            Some([3700, 549, 5260, 1539])
        );
        // A title bar above the top edge can't be grabbed either.
        let above = place(D2, [440.0, -500.0, 1040.0, 660.0]);
        assert_eq!(
            restore(&above, &two_displays(), [1.0, 1.0], "").map(|r| r[1]),
            Some(549)
        );
    }

    #[test]
    fn the_first_opening_is_compact_and_centred_on_the_display_under_the_mouse() {
        let first = Saved::default();
        let d = two_displays();
        let size = [DEFAULT_SIZE.x, DEFAULT_SIZE.y];
        // The mouse on the 150 % display: 880 x 600 points are 1320 x 900 pixels there.
        let under = monitor_at([4000, 1000], &d);
        assert_eq!(under, D2);
        assert_eq!(
            restore(&first, &d, size, &under),
            Some([3820, 594, 5140, 1494])
        );
        // No mouse position: the main display.
        assert_eq!(restore(&first, &d, size, ""), Some([840, 396, 1720, 996]));
        // Never more than 70 % of the work area, each way.
        assert_eq!(
            restore(&first, &d, [9000.0, 9000.0], ""),
            Some([384, 209, 2176, 1183])
        );
        assert_eq!(restore(&first, &[], size, ""), None);
    }

    #[test]
    fn a_small_display_gets_at_most_70_percent_but_not_below_the_minimum() {
        let small = Monitor {
            id: D1.into(),
            label: String::new(),
            primary: true,
            work_area: [0, 0, 1366, 728],
            dpi: 96,
        };
        // 70 % of 728 is 509.6: the height gives way, the width fits.
        assert_eq!(
            compact([DEFAULT_SIZE.x, DEFAULT_SIZE.y], &small),
            [880.0, 509.6]
        );
        let tiny = Monitor {
            work_area: [0, 0, 800, 480],
            ..small.clone()
        };
        assert_eq!(compact([880.0, 600.0], &tiny), [MIN_SIZE.x, MIN_SIZE.y]);
        // And a remembered size isn't made compact: it wins.
        let remembered = place(D1, [9000.0, 9000.0, 1200.0, 700.0]);
        let r = restore(&remembered, &[small], [1.0, 1.0], "").unwrap();
        assert_eq!((r[2] - r[0], r[3] - r[1]), (1200, 700));
    }

    #[test]
    fn the_mouse_over_the_taskbar_still_names_its_display() {
        let d = two_displays();
        assert_eq!(monitor_at([100, 1420], &d), D1, "below D1's work area");
        assert_eq!(monitor_at([2559, 10], &d), D1);
        assert_eq!(monitor_at([2560, 10], &d), D2);
        assert_eq!(monitor_at([0, 0], &[]), "");
    }

    #[test]
    fn the_first_versions_pixel_place_still_opens_there() {
        let old: Saved =
            serde_json::from_str(r#"{"monitor":"\\\\.\\DISPLAY2","rect":[3000,200,4560,1190]}"#)
                .unwrap();
        assert_eq!(
            restore(&old, &two_displays(), [1.0, 1.0], ""),
            Some([3000, 200, 4560, 1190])
        );
        // And it isn't written back: only the place in points is.
        let json = serde_json::to_string(&place(D1, [1.0, 2.0, 3.0, 4.0])).unwrap();
        assert!(!json.contains("rect"), "{json}");
    }

    #[test]
    fn the_keys_region_covers_every_pixel_of_the_keys() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.2, 20.0), egui::pos2(110.5, 80.25));
        assert_eq!(keys_region(rect, 1.5), [15, 30, 166, 121, 0]);
    }

    #[test]
    fn a_rectangle_belongs_to_the_display_it_is_mostly_on() {
        let d = two_displays();
        assert_eq!(monitor_of([2000, 0, 3000, 500], &d), D1);
        assert_eq!(monitor_of([2400, 0, 3400, 500], &d), D2);
        assert_eq!(monitor_of([-5000, 0, -4000, 500], &d), "");
    }

    #[test]
    fn points_round_trip_at_100_and_150_percent() {
        let d = two_displays();
        for (m, r) in [
            (&d[0], [200, 100, 1240, 760]),
            (&d[1], [3000, 201, 4560, 1191]),
        ] {
            let p = to_points(r, m, m.dpi);
            assert_eq!(to_physical(p, m), r, "{}", m.id);
        }
        // 1560 x 990 pixels at 150 % is 1040 x 660 points.
        let p = to_points([3000, 201, 4560, 1191], &d[1], 144);
        assert_eq!([p[2], p[3]], [1040.0, 660.0]);
    }

    #[test]
    fn dragging_between_100_and_150_percent_keeps_the_size_in_points() {
        let d = two_displays();
        let mut memory = Memory::new(place(D1, [200.0, 100.0, 1040.0, 660.0]));
        let mut window = Fake::at([0, 0, 1040, 660], 96);
        memory.frame(&mut window, &d, "");
        assert_eq!(window.moves, vec![[200, 100, 1240, 760]]);
        // Dragged right until it's mostly on the 150 % display: one rescale, to 1.5 times.
        assert_eq!(window.windows_moves([2100, 100, 3140, 760], &d), 1);
        assert_eq!(window.rect, Some([2100, 100, 3660, 1090]));
        memory.frame(&mut window, &d, "");
        let saved = memory.due_save(&d).expect("moved");
        assert_eq!(saved.monitor, D2);
        assert_eq!(saved.place.map(|p| [p[2], p[3]]), Some([1040.0, 660.0]));
        // And back: one rescale, to the size it had.
        assert_eq!(window.windows_moves([300, 100, 1860, 1090], &d), 1);
        assert_eq!(window.rect, Some([300, 100, 1340, 760]));
        memory.frame(&mut window, &d, "");
        let back = memory.closed(&d).expect("moved back");
        assert_eq!(back, place(D1, [300.0, 100.0, 1040.0, 660.0]));
    }

    #[test]
    fn reopening_on_the_150_percent_display_keeps_its_size_with_one_rescale() {
        let d = two_displays();
        // Saved there at 1040 x 660 points; the new window is made on the 100 % display.
        let mut memory = Memory::new(place(D2, [440.0, 200.0, 1040.0, 660.0]));
        let mut window = Fake::at([0, 0, 1040, 660], 96);
        memory.frame(&mut window, &d, "");
        assert_eq!(window.moves, vec![[3220, 300, 4780, 1290]]);
        assert_eq!(window.dpi, 144);
        memory.frame(&mut window, &d, "");
        assert_eq!(memory.closed(&d), None, "unchanged: nothing to write");
    }

    #[test]
    fn resizes_never_move_or_size_the_window_again() {
        // No loop: whatever happens to the window after it's placed (rescales back and forth,
        // the user resizing), the memory only reads it, and writes the store only on a save.
        let d = two_displays();
        let mut memory = Memory::new(Saved::default());
        let mut window = Fake::at([0, 0, 1040, 660], 96);
        memory.frame(&mut window, &d, D1);
        let placed = window.moves.clone();
        for i in 0..50 {
            let x = if i % 2 == 0 { 2100 } else { 1000 };
            window.windows_moves([x, 100, x + 1040, 760], &d);
            memory.frame(&mut window, &d, D1);
        }
        assert_eq!(window.moves, placed, "placed once per opening, never again");
    }

    #[test]
    fn the_smallest_size_follows_the_scaling() {
        assert_eq!(min_track_size(96, [16, 39]), [736, 459]);
        assert_eq!(min_track_size(144, [22, 52]), [1102, 682]);
        // Windows' suggested rectangle for a window at its smallest, moved from 100 % to 150 %,
        // is never clamped by it (the frame scales too).
        let at_100 = min_track_size(96, [16, 39]);
        let suggested = [at_100[0] * 3 / 2, at_100[1] * 3 / 2];
        let at_150 = min_track_size(144, [24, 58]);
        assert!(suggested[0] >= at_150[0] && suggested[1] >= at_150[1]);
    }

    #[test]
    fn minimizing_closes_it_and_nothing_else_does() {
        const WM_SYSCOMMAND: u32 = 0x0112;
        assert!(minimize_closes(WM_SYSCOMMAND, 0xF020));
        // The title-bar button sends it with the low bits set.
        assert!(minimize_closes(WM_SYSCOMMAND, 0xF022));
        assert!(!minimize_closes(WM_SYSCOMMAND, 0xF030), "maximize");
        assert!(
            !minimize_closes(WM_SYSCOMMAND, 0xF060),
            "close goes its own way"
        );
        assert!(!minimize_closes(0x0010, 0xF020), "not a system command");
    }

    #[test]
    fn a_minimized_window_asks_to_close() {
        // However it got minimized (Win+M, Win+Down), the frame that sees it says close, from
        // the viewport's own state (winit's resize event), with no window here.
        crate::foxglove::store::test_config_dir();
        let ctx = egui::Context::default();
        let mut window = SettingsWindow::default();
        let mut close = |minimized: bool| {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .minimized = Some(minimized);
            let mut asked = false;
            let mut out = ctx.run_ui(input, |ui| asked = window.frame(ui.ctx()));
            out.textures_delta.clear();
            asked
        };
        assert!(!close(false));
        assert!(close(true));
    }

    #[test]
    fn minimized_or_maximized_is_not_a_place() {
        let d = two_displays();
        let mut memory = Memory::new(Saved::default());
        let mut window = Fake::default();
        memory.frame(&mut window, &d, "");
        assert!(window.shown, "shown even without a rectangle");
        window.rect = None;
        memory.frame(&mut window, &d, "");
        assert_eq!(memory.closed(&d), None);
    }

    #[test]
    fn the_place_round_trips_through_the_store() {
        crate::foxglove::store::test_config_dir();
        let saved = place(D2, [440.0, 200.0, 1040.0, 660.0]);
        crate::foxglove::store::save(SECTION, &saved).unwrap();
        assert_eq!(
            crate::foxglove::store::try_load::<Saved>(SECTION),
            Some(saved)
        );
    }
}
