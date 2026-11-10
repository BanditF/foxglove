//! Opening Settings from outside a frame: the tray menu's "Open Foxglove" (on the tray-menu
//! thread) and a second launch (on the single-instance thread). The tray panel's "Open
//! Foxglove →" runs inside a frame and only logs here.
//!
//! "Sometimes nothing happens" (tray open): an open request only set a flag and woke the UI,
//! and the frame that took it showed nothing when Settings was already "open" (its window made,
//! but hidden): it only un-minimized and focused it, which shows no hidden window. Now a request
//! (1) wakes the root viewport, the one whose frame takes the flag (a plain
//! `ctx.request_repaint()` from another thread wakes whichever viewport the UI thread is in at
//! that moment, e.g. the hidden Settings one), (2) shows the window straight from the
//! requesting thread when its handle is known (`ShowWindowAsync`, safe from any thread), and
//! (3) the frame shows it again through winit (`SettingsWindow::reshow`).
//!
//! Each request logs "tray: open requested (<source>)" once, and the show it leads to logs
//! "settings window shown" once: nothing is logged per frame.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// The start argument for a start in the tray: Settings stays closed until the tray (or a second
/// launch) opens it, and a launch that finds Foxglove running leaves it as it is. Without it every
/// start opens Settings, as KeyPeek did: a start by hand, the Startup-folder entry at sign-in.
/// On-call's restarts use it (through a shortcut, since `explorer.exe <exe>` passes no
/// arguments), and so do the app's own relaunches (#346, the identity package).
pub const TRAY_ARG: &str = "--tray";

/// Whether these arguments (the program first, as `std::env::args_os`) ask for a start in the
/// tray ([`TRAY_ARG`]).
pub fn tray_start_in<S: AsRef<std::ffi::OsStr>>(args: impl IntoIterator<Item = S>) -> bool {
    args.into_iter().skip(1).any(|a| a.as_ref() == TRAY_ARG)
}

/// This process was started in the tray ([`TRAY_ARG`]).
pub fn tray_start() -> bool {
    tray_start_in(std::env::args_os())
}

/// Where an open request came from, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The tray menu's "Open Foxglove".
    TrayMenu,
    /// The tray panel's "Open Foxglove →".
    TrayPanel,
    /// A second launch of the app (#345).
    SecondLaunch,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::TrayMenu => "tray menu",
            Source::TrayPanel => "tray panel",
            Source::SecondLaunch => "second launch",
        }
    }
}

/// The log line for a request.
pub fn requested_line(source: Source) -> String {
    format!("tray: open requested ({})", source.label())
}

/// The log line for the show a request (or an opening) led to.
pub fn shown_line(how: &str) -> String {
    format!("settings window shown ({how})")
}

/// The Settings window's native handle while it's open and found (0: none), published by its
/// frame so a request on another thread can show it at once.
static HWND: AtomicIsize = AtomicIsize::new(0);
/// A request is waiting for its "shown" line.
static OWED: AtomicBool = AtomicBool::new(false);

/// The Settings window's handle, as its frame finds it (`None` when it closes).
pub fn publish(hwnd: Option<isize>) {
    HWND.store(hwnd.unwrap_or(0), Ordering::Release);
}

fn published() -> Option<isize> {
    match HWND.load(Ordering::Acquire) {
        0 => None,
        h => Some(h),
    }
}

/// What a request does, with its effects passed in so a test can run it with fakes and no
/// window: sets `flag` (the frame's "open Settings"), wakes the UI, and shows the window
/// directly when its handle is known. Returns the log line.
pub fn request_with(
    source: Source,
    flag: &AtomicBool,
    wake: &dyn Fn(),
    hwnd: Option<isize>,
    show_now: &dyn Fn(isize),
) -> String {
    flag.store(true, Ordering::Release);
    OWED.store(true, Ordering::Release);
    if let Some(h) = hwnd {
        show_now(h);
    }
    wake();
    requested_line(source)
}

/// A request from outside a frame (tray menu, second launch): logs it, sets `flag`, shows the
/// window if it's known, and wakes the UI.
pub fn request(source: Source, flag: &AtomicBool, wake: &dyn Fn()) {
    let line = request_with(source, flag, wake, published(), &show_native);
    eprintln!("{line}");
}

/// A request made inside a frame (the tray panel): only its log line; the frame opens it.
pub fn requested_in_frame(source: Source) {
    OWED.store(true, Ordering::Release);
    eprintln!("{}", requested_line(source));
}

/// The window was shown: logs it once per request (or opening, `opening`), never per frame.
pub fn shown(how: &str, opening: bool) {
    if OWED.swap(false, Ordering::AcqRel) || opening {
        eprintln!("{}", shown_line(how));
    }
}

/// Shows and fronts the Settings window from any thread. `ShowWindowAsync` only posts to the
/// window's own (UI) thread, so it never blocks on or re-enters a frame; a stale handle fails.
#[cfg(target_os = "windows")]
fn show_native(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, ShowWindowAsync, SW_SHOW,
    };
    let h = HWND(hwnd as *mut core::ffi::c_void);
    let mut pid = 0u32;
    // SAFETY: plain queries and a posted show on a window handle; a stale one just fails.
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
        // Only our own window: a handle Windows reused for another app's is left alone.
        if pid != GetCurrentProcessId() {
            return;
        }
        let _ = ShowWindowAsync(h, SW_SHOW);
    }
    // The click on the tray gave this app the foreground right; else Windows decides (focus.rs).
    super::focus::to_foreground(hwnd);
}

#[cfg(not(target_os = "windows"))]
fn show_native(_hwnd: isize) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};

    /// An On-call restart (`foxglove.exe --tray`) starts in the tray; a start with no arguments
    /// (by hand, the Startup-folder entry) opens Settings; the program's own name never counts.
    #[test]
    fn only_the_tray_argument_starts_in_the_tray() {
        assert!(tray_start_in([r"C:pp-mainoxglove.exe", "--tray"]));
        assert!(tray_start_in(["foxglove.exe", "--other", "--tray"]));
        assert!(!tray_start_in([r"C:pp-mainoxglove.exe"]));
        assert!(
            !tray_start_in(["--tray"]),
            "the program itself is not an argument"
        );
        assert!(!tray_start_in(["foxglove.exe", "--TRAY-ish", "tray"]));
        assert!(!tray_start_in(Vec::<String>::new()));
    }

    /// The tray menu's request runs on its own thread with no frame running: it sets the flag
    /// and wakes the UI there and then, and shows a known window at once.
    #[test]
    fn a_request_off_the_ui_thread_wakes_and_shows_without_a_frame() {
        let flag = Arc::new(AtomicBool::new(false));
        let wakes = Arc::new(AtomicUsize::new(0));
        let shows = Arc::new(Mutex::new(Vec::new()));
        let line = std::thread::spawn({
            let (flag, wakes, shows) = (flag.clone(), wakes.clone(), shows.clone());
            move || {
                request_with(
                    Source::TrayMenu,
                    &flag,
                    &|| {
                        wakes.fetch_add(1, Ordering::SeqCst);
                    },
                    Some(67864),
                    &|h| shows.lock().unwrap().push(h),
                )
            }
        })
        .join()
        .unwrap();
        assert_eq!(line, "tray: open requested (tray menu)");
        assert!(flag.load(Ordering::SeqCst), "the frame will open Settings");
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "one wake per request");
        assert_eq!(*shows.lock().unwrap(), vec![67864], "shown at once, once");
    }

    /// With no window yet (it's made by the frame), the request only flags and wakes.
    #[test]
    fn with_no_window_yet_the_frame_opens_it() {
        let flag = AtomicBool::new(false);
        let wakes = AtomicUsize::new(0);
        let shows = Mutex::new(Vec::new());
        let line = request_with(
            Source::SecondLaunch,
            &flag,
            &|| {
                wakes.fetch_add(1, Ordering::SeqCst);
            },
            None,
            &|h| shows.lock().unwrap().push(h),
        );
        assert_eq!(line, "tray: open requested (second launch)");
        assert!(flag.load(Ordering::SeqCst));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert!(shows.lock().unwrap().is_empty());
    }

    /// The UI's wake reaches the root viewport (whose frame takes the flag) even while the UI
    /// thread is inside the Settings viewport's pass: egui's `request_repaint()` asks for the
    /// viewport on top of the context's shared stack, which then is the hidden Settings one.
    #[test]
    fn the_wake_reaches_the_root_viewport_from_inside_the_settings_pass() {
        let ctx = egui::Context::default();
        let woken = Arc::new(Mutex::new(Vec::new()));
        ctx.set_request_repaint_callback({
            let woken = woken.clone();
            move |info| woken.lock().unwrap().push(info.viewport_id)
        });
        let wake = crate::ui_wake::UiWake::from_ctx(&ctx);
        let settings = super::super::settings_window::id();
        let mut input = egui::RawInput {
            viewport_id: settings,
            ..Default::default()
        };
        input.viewports.insert(settings, Default::default());
        let mut inside = None;
        let mut out = ctx.run_ui(input, |ui| {
            inside = Some(ui.ctx().viewport_id());
            woken.lock().unwrap().clear();
            // The tray-menu thread's wake while this pass runs.
            let wake = wake.clone();
            std::thread::spawn(move || wake.request_repaint())
                .join()
                .unwrap();
        });
        out.textures_delta.clear();
        assert_eq!(
            inside,
            Some(settings),
            "the pass is the Settings viewport's"
        );
        let woken = woken.lock().unwrap();
        assert!(
            woken.contains(&egui::ViewportId::ROOT),
            "the root viewport is woken: {woken:?}"
        );
        assert!(!woken.contains(&settings), "not the hidden Settings one");
    }

    #[test]
    fn the_lines_name_their_source() {
        assert_eq!(
            requested_line(Source::TrayPanel),
            "tray: open requested (tray panel)"
        );
        assert_eq!(shown_line("reopened"), "settings window shown (reopened)");
    }
}
