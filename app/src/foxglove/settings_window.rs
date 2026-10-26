//! The Settings window as a window of its own (an egui immediate viewport) instead of a panel
//! drawn inside the full-screen overlay. Like any app window it can be dragged to any display by
//! its title bar, and nothing behind it is dimmed or blocked. It opens where it was last left:
//! the same place on the same display, or centred on that display (or the overlay's, if that one
//! is gone too) when the old place is no longer on any screen.
//!
//! The overlay stays one topmost, click-through window on its display; only the keymap editor
//! still draws there. Where the window is, is read and set in physical pixels through Win32
//! ([`NativeWindow`]); the logic deciding it ([`restore`], [`Memory`]) is plain and tested
//! without a window.

use serde::{Deserialize, Serialize};

use crate::monitors::Monitor;

/// The window's title; the Win32 side finds the window by it.
pub const TITLE: &str = "Foxglove Settings";
/// The size it first opens at, in points (#385: room for the sidebar).
pub const DEFAULT_SIZE: egui::Vec2 = egui::vec2(1040.0, 660.0);
pub const MIN_SIZE: egui::Vec2 = egui::vec2(720.0, 420.0);
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
        .with_title(TITLE)
        .with_inner_size(DEFAULT_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_icon(icon.clone())
        .with_visible(!cfg!(target_os = "windows"))
}

/// Where the window was last: the display it was mostly on and its outer rectangle (left, top,
/// right, bottom) in physical desktop pixels.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub monitor: String,
    pub rect: Option<[i32; 4]>,
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

/// Where to open the window. The saved rectangle when enough of its title bar is on a display
/// (so it can still be grabbed); otherwise its size (or `size`, the size it was created at)
/// centred on the saved display, or on `fallback` (the overlay's display, else the main one)
/// when that display is gone. `None` when no display is known: leave it where Windows put it.
pub fn restore(
    saved: &Saved,
    monitors: &[Monitor],
    size: [i32; 2],
    fallback: &str,
) -> Option<[i32; 4]> {
    if let Some(r) = saved.rect {
        let grip = [r[0], r[1], r[2], r[1] + GRIP[1].min(r[3] - r[1])];
        let need = [GRIP[0].min(r[2] - r[0]), GRIP[1].min(r[3] - r[1])];
        let reachable = monitors.iter().any(|m| {
            let o = overlap(grip, m.work_area);
            o[0] >= need[0] && o[1] >= need[1]
        });
        if reachable {
            return Some(r);
        }
    }
    let monitor = monitors
        .iter()
        .find(|m| !saved.monitor.is_empty() && m.id == saved.monitor)
        .cloned()
        .or_else(|| crate::monitors::find(monitors, fallback))
        .or_else(|| monitors.first().cloned())?;
    let size = saved
        .rect
        .map(|r| [r[2] - r[0], r[3] - r[1]])
        .unwrap_or(size);
    Some(centred(size, monitor.work_area))
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
    last: Option<[i32; 4]>,
}

impl Memory {
    pub fn new(saved: Saved) -> Self {
        Self {
            saved,
            ..Self::default()
        }
    }

    /// A frame with the window open: the first one puts it in its place and shows it, the
    /// others note where it is now.
    pub fn frame(&mut self, window: &mut dyn NativeWindow, monitors: &[Monitor], fallback: &str) {
        if !self.placed {
            self.placed = true;
            let size = window
                .rect()
                .map(|r| [r[2] - r[0], r[3] - r[1]])
                .unwrap_or([1040, 660]);
            if let Some(to) = restore(&self.saved, monitors, size, fallback) {
                window.move_to(to);
            }
            window.show();
            return;
        }
        if let Some(r) = window.rect() {
            self.last = Some(r);
        }
    }

    /// What to save now (the window lost the focus, or is closing), if it moved since the last
    /// save.
    pub fn due_save(&mut self, monitors: &[Monitor]) -> Option<Saved> {
        let r = self.last?;
        if self.saved.rect == Some(r) {
            return None;
        }
        self.saved = Saved {
            monitor: monitor_of(r, monitors),
            rect: Some(r),
        };
        Some(self.saved.clone())
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
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self {
            memory: Memory::new(super::store::try_load(SECTION).unwrap_or_default()),
            focused: false,
            theme: None,
            #[cfg(target_os = "windows")]
            hwnd: None,
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
    /// title bar in the app's theme, and saves its place when it loses the focus.
    /// `overlay_monitor` is where it opens when it has no place of its own yet.
    pub fn frame(&mut self, ctx: &egui::Context, overlay_monitor: &str) {
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
                self.hwnd = imp::find(TITLE);
            }
            let monitors = crate::monitors::list();
            match self.hwnd {
                Some(hwnd) => {
                    let mut window = imp::Win32Window { hwnd, ctx };
                    self.memory.frame(&mut window, &monitors, overlay_monitor);
                }
                None => {
                    // Not found (it should be): show it where Windows put it.
                    eprintln!("Foxglove: can't find the Settings window to place it");
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
        #[cfg(not(target_os = "windows"))]
        let _ = overlay_monitor;
    }

    /// The window closed: remember where it was.
    pub fn closed(&mut self) {
        save(self.memory.closed(&crate::monitors::list()));
        self.focused = false;
        self.theme = None;
        #[cfg(target_os = "windows")]
        {
            self.hwnd = None;
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
    fn move_to(&mut self, _: [i32; 4]) {}
    fn show(&mut self) {
        imp::show(self.0);
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GetWindowRect, GetWindowTextW, IsIconic, IsZoomed, SetWindowPos,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

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

    /// This PC's displays: the main one, and a 4K one to its right.
    pub fn two_displays() -> Vec<Monitor> {
        vec![
            Monitor {
                id: r"\\.\DISPLAY1".into(),
                label: "Display 1 (2560x1440) - main".into(),
                primary: true,
                work_area: [0, 0, 2560, 1392],
            },
            Monitor {
                id: r"\\.\DISPLAY2".into(),
                label: "Display 2 (3840x2160)".into(),
                primary: false,
                work_area: [2560, 0, 6400, 2088],
            },
        ]
    }

    #[derive(Default)]
    struct Fake {
        rect: Option<[i32; 4]>,
        shown: bool,
        moves: Vec<[i32; 4]>,
    }

    impl NativeWindow for Fake {
        fn rect(&self) -> Option<[i32; 4]> {
            self.rect
        }
        fn move_to(&mut self, rect: [i32; 4]) {
            self.moves.push(rect);
            self.rect = Some(rect);
        }
        fn show(&mut self) {
            self.shown = true;
        }
    }

    #[test]
    fn a_saved_place_on_a_display_is_kept() {
        let saved = Saved {
            monitor: r"\\.\DISPLAY2".into(),
            rect: Some([3000, 200, 4560, 1190]),
        };
        assert_eq!(
            restore(&saved, &two_displays(), [1560, 990], ""),
            saved.rect
        );
        // Half off the right edge, but the title bar can still be grabbed: the user's choice.
        let edge = Saved {
            rect: Some([6000, 100, 7560, 1090]),
            ..saved
        };
        assert_eq!(restore(&edge, &two_displays(), [1560, 990], ""), edge.rect);
    }

    #[test]
    fn a_window_on_a_display_that_is_gone_comes_back_centred() {
        let saved = Saved {
            monitor: r"\\.\DISPLAY2".into(),
            rect: Some([3000, 200, 4560, 1190]),
        };
        let main_only = &two_displays()[..1];
        // Its size, centred on the overlay's display (here the main one).
        assert_eq!(
            restore(&saved, main_only, [1, 1], r"\\.\DISPLAY2"),
            Some([500, 201, 2060, 1191])
        );
    }

    #[test]
    fn a_window_off_every_screen_goes_back_to_its_display() {
        // The display is still there but the layout moved it: centred on that display.
        let saved = Saved {
            monitor: r"\\.\DISPLAY2".into(),
            rect: Some([9000, 200, 10560, 1190]),
        };
        assert_eq!(
            restore(&saved, &two_displays(), [1, 1], ""),
            Some([3700, 549, 5260, 1539])
        );
        // A title bar above the top edge can't be grabbed either.
        let above = Saved {
            rect: Some([3000, -500, 4560, 490]),
            ..saved
        };
        assert_eq!(
            restore(&above, &two_displays(), [1, 1], "").map(|r| r[1]),
            Some(549)
        );
    }

    #[test]
    fn the_first_opening_is_centred_on_the_overlays_display_and_fits() {
        let first = Saved::default();
        assert_eq!(
            restore(&first, &two_displays(), [1560, 990], r"\\.\DISPLAY2"),
            Some([3700, 549, 5260, 1539])
        );
        // Bigger than the display: shrunk to its work area.
        assert_eq!(
            restore(&first, &two_displays(), [9000, 9000], ""),
            Some([0, 0, 2560, 1392])
        );
        assert_eq!(restore(&first, &[], [1560, 990], ""), None);
    }

    #[test]
    fn the_keys_region_covers_every_pixel_of_the_keys() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.2, 20.0), egui::pos2(110.5, 80.25));
        assert_eq!(keys_region(rect, 1.5), [15, 30, 166, 121, 0]);
    }

    #[test]
    fn a_rectangle_belongs_to_the_display_it_is_mostly_on() {
        let d = two_displays();
        assert_eq!(monitor_of([2000, 0, 3000, 500], &d), r"\\.\DISPLAY1");
        assert_eq!(monitor_of([2400, 0, 3400, 500], &d), r"\\.\DISPLAY2");
        assert_eq!(monitor_of([-5000, 0, -4000, 500], &d), "");
    }

    #[test]
    fn memory_places_once_then_follows_the_window_and_saves_moves() {
        let d = two_displays();
        let mut memory = Memory::new(Saved::default());
        let mut window = Fake {
            rect: Some([100, 100, 1660, 1090]),
            ..Fake::default()
        };
        memory.frame(&mut window, &d, r"\\.\DISPLAY2");
        assert!(window.shown);
        assert_eq!(window.moves, vec![[3700, 549, 5260, 1539]]);
        // Not moved by the user yet: nothing to save beyond where it opened.
        memory.frame(&mut window, &d, "");
        assert_eq!(window.moves.len(), 1, "placed only once per opening");
        let opened = memory.due_save(&d);
        assert_eq!(opened.map(|s| s.monitor), Some(r"\\.\DISPLAY2".into()));
        assert_eq!(memory.due_save(&d), None, "saved once, not again");
        // Dragged to the main display.
        window.rect = Some([200, 100, 1760, 1090]);
        memory.frame(&mut window, &d, "");
        let moved = memory.closed(&d).expect("a move to save");
        assert_eq!(moved.monitor, r"\\.\DISPLAY1");
        assert_eq!(moved.rect, Some([200, 100, 1760, 1090]));
        // Reopened: back where it was closed.
        let mut again = Fake {
            rect: Some([0, 0, 1560, 990]),
            ..Fake::default()
        };
        memory.frame(&mut again, &d, r"\\.\DISPLAY2");
        assert_eq!(again.moves, vec![[200, 100, 1760, 1090]]);
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
        let saved = Saved {
            monitor: r"\\.\DISPLAY2".into(),
            rect: Some([3000, 200, 4560, 1190]),
        };
        crate::foxglove::store::save(SECTION, &saved).unwrap();
        assert_eq!(
            crate::foxglove::store::try_load::<Saved>(SECTION),
            Some(saved)
        );
    }
}
