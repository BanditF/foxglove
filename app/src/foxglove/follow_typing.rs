//! "Follow the keyboard I type on" (C4, #802; docs/design/device-focus-layouts-thumbnails-
//! lights-declutter.md, section 1 and decision 4): with it on, the focused keyboard
//! (`keyboard_focus`) moves to the keyboard the user types on. Off by default.
//!
//! - **Privacy:** Foxglove learns only *which* keyboard typed. The Raw Input listener asks
//!   Windows for each event's header alone (`RID_HEADER`: the event's type, size and device
//!   handle), never the event's data, so no key ever reaches Foxglove; the handle becomes the
//!   device's path and its VID/PID, and nothing else is kept or logged. A source test holds this.
//! - **Events only:** `WM_INPUT` on a message-only window (`RIDEV_INPUTSINK`, so typing in any
//!   app counts), whose thread sleeps in `GetMessageW` between events. No timer, no polling. The
//!   listener runs only while the setting is on.
//! - **No flicker** ([`Follower`]): the focus moves only after [`FOLLOW_AFTER`] events in a row
//!   from another keyboard (about three keys: a key sends one event going down and one coming up),
//!   each within [`STREAK_GAP`] of the last, and at most once per [`MIN_SWITCH`]. Typing on two
//!   boards at once doesn't flip it back and forth.
//! - **A pick wins:** a keyboard picked in the switcher stays the focus while the user keeps
//!   typing on the keyboard they were typing on; typing on another one moves it again.
//! - **Injectable source** ([`Source`]): tests feed synthetic device events and never register
//!   Raw Input; the app's source is [`raw::RawInput`].

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::keyboard_focus;

/// Events in a row from another keyboard before the focus follows it: a key sends two (down and
/// up), so about three keys.
pub const FOLLOW_AFTER: u32 = 6;
/// The longest pause inside a run of [`FOLLOW_AFTER`] events; a longer one starts the count again.
pub const STREAK_GAP: Duration = Duration::from_secs(2);
/// The focus follows the typing at most once per this.
pub const MIN_SWITCH: Duration = Duration::from_secs(1);

/// The card on the App page.
pub const TITLE: &str = "Keyboard shown";
/// The switch.
pub const LABEL: &str = "Follow the keyboard I type on";
/// What the switch does: its hover and ⓘ, after [`LABEL`].
pub const TIP: &str = "Shows the keyboard you last typed on. A few keys on another keyboard move \
                       it there. Foxglove reads only which keyboard typed, never what you type.";
/// The switch off, in its hover.
pub const OFF: &str = "Off: your pick stays until you change it.";

/// The switch's hover: the whole label, what it does, and what off means while it is off.
pub fn hover(on: bool) -> String {
    if on {
        format!("{LABEL}: {TIP}")
    } else {
        format!("{LABEL}: {TIP}\n{OFF}")
    }
}

/// Which keyboard is being typed on, debounced. Pure: keyboards (by focus key) and times go in.
#[derive(Clone, Debug, Default)]
pub struct Follower {
    /// The keyboard the user is typing on, once it has typed [`FOLLOW_AFTER`] events.
    typer: Option<String>,
    /// Another keyboard's run: its key, its events so far, and when the last one came.
    streak: Option<(String, u32, Instant)>,
    /// When the typer last changed.
    switched: Option<Instant>,
}

impl Follower {
    /// The keyboard `key` typed at `at`. Whether it is now the keyboard being typed on (it
    /// wasn't before): the focus should follow it.
    pub fn typed(&mut self, key: &str, at: Instant) -> bool {
        if self.typer.as_deref() == Some(key) {
            self.streak = None;
            return false;
        }
        let count = match &self.streak {
            Some((k, n, last)) if k == key && at.saturating_duration_since(*last) <= STREAK_GAP => {
                n + 1
            }
            _ => 1,
        };
        self.streak = Some((key.to_string(), count, at));
        if count < FOLLOW_AFTER
            || self
                .switched
                .is_some_and(|s| at.saturating_duration_since(s) < MIN_SWITCH)
        {
            return false;
        }
        self.typer = Some(key.to_string());
        self.streak = None;
        self.switched = Some(at);
        true
    }
}

/// Where typing events come from. Each event is one typing device's path (`\\?\HID#VID_…`), and
/// nothing about the key.
pub trait Source {
    /// Starts calling `sink` with each typing device's path; the returned [`Stop`] ends it.
    fn start(self: Box<Self>, sink: Sink) -> Result<Stop, String>;
}

/// Takes one typing device's path.
pub type Sink = Box<dyn FnMut(&str) + Send>;
/// Stops a [`Source`]; when it returns, the source sends nothing more.
pub type Stop = Box<dyn FnOnce() + Send>;

/// The running source's stop, while the setting is on.
static RUNNING: Mutex<Option<Stop>> = Mutex::new(None);
/// Repaints the settings window when the focus follows the typing.
static WAKE: OnceLock<crate::ui_wake::UiWake> = OnceLock::new();

/// The app's source: Raw Input.
#[cfg(all(windows, not(test)))]
fn default_source() -> Box<dyn Source> {
    Box::new(raw::RawInput)
}

/// Tests (and other systems) get one that registers nothing.
#[cfg(any(not(windows), test))]
fn default_source() -> Box<dyn Source> {
    Box::new(Nothing)
}

/// A source that never types: tests and non-Windows builds.
#[cfg(any(not(windows), test))]
struct Nothing;

#[cfg(any(not(windows), test))]
impl Source for Nothing {
    fn start(self: Box<Self>, _: Sink) -> Result<Stop, String> {
        Ok(Box::new(|| {}))
    }
}

/// At app start: listens when the setting is on.
pub fn start() {
    if keyboard_focus::follow_typing() {
        listen(default_source());
    }
}

/// The switch: saves the setting and starts or stops the listener.
pub fn set(on: bool) {
    keyboard_focus::set_follow_typing(on);
    if on {
        listen(default_source());
    } else {
        quiet();
    }
}

/// Starts `source` unless one runs.
pub fn listen(source: Box<dyn Source>) {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.is_some() {
        return;
    }
    match source.start(Box::new(|path| {
        typed(path, Instant::now());
    })) {
        Ok(stop) => *running = Some(stop),
        Err(e) => eprintln!("Foxglove: can't follow the keyboard you type on: {e}"),
    }
}

/// Stops the running source, if any.
pub fn quiet() {
    let stop = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(stop) = stop {
        stop();
    }
}

/// Whether a source runs.
#[cfg(test)]
pub fn listening() -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// The device at `path` typed at `at`: the focus follows it as `keyboard_focus::typed` says.
/// Whether the focus moved.
pub fn typed(path: &str, at: Instant) -> bool {
    let Some((vid, pid)) = ids(path) else {
        return false;
    };
    let moved = keyboard_focus::typed(vid, pid, at);
    if moved {
        if let Some(wake) = WAKE.get() {
            wake.request_repaint();
        }
    }
    moved
}

/// A HID device path's VID and PID: `VID_1D50&PID_615E` over USB, `VID&0002046D_PID&B342`
/// (Bluetooth) or `VID&02046D_PID&B342` (Bluetooth LE), where the digits before the last four
/// say who issued the VID.
pub fn ids(path: &str) -> Option<(u16, u16)> {
    let up = path.to_ascii_uppercase();
    let hex = |tag: &str| -> Option<u16> {
        let rest = &up[up.find(tag)? + tag.len()..];
        let digits = rest
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(rest.len());
        let run = &rest[..digits];
        // `VID_` is followed by its four digits; `VID&` by the issuer's, then the four.
        let four = if tag.ends_with('_') {
            run.get(..4)?
        } else {
            run.get(run.len().checked_sub(4)?..)?
        };
        u16::from_str_radix(four, 16).ok()
    };
    let vid = hex("VID_").or_else(|| hex("VID&"))?;
    let pid = hex("PID_").or_else(|| hex("PID&"))?;
    Some((vid, pid))
}

/// The App page's "Keyboard shown" card: the switch and its ⓘ.
pub fn group(ui: &mut egui::Ui) {
    let _ = WAKE.set(crate::ui_wake::UiWake::from_ctx(ui.ctx()));
    super::settings_ui::section(ui, TITLE, |ui| {
        let was = keyboard_focus::follow_typing();
        let mut on = was;
        let changed = ui
            .horizontal(|ui| {
                let c = ui.checkbox(&mut on, LABEL);
                super::theme::with_info(ui, c, &hover(was)).changed()
            })
            .inner;
        if changed {
            set(on);
        }
    });
}

/// Raw Input on a message-only window: each keyboard event's header, never its data.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
mod raw {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::time::Duration;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::{
        GetRawInputData, GetRawInputDeviceInfoW, RegisterRawInputDevices, HRAWINPUT,
        RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDEV_REMOVE,
        RIDI_DEVICENAME, RID_HEADER,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostThreadMessageW, RegisterClassW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE,
        WM_INPUT, WM_INPUT_DEVICE_CHANGE, WM_QUIT, WNDCLASSW,
    };

    use super::{Sink, Source, Stop};

    /// HID usage page 1 (generic desktop), usage 6 (keyboard).
    const DESKTOP: u16 = 0x01;
    const KEYBOARD: u16 = 0x06;

    thread_local! {
        /// The listener thread's sink and each device handle's path (`None`: no path).
        static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
        static PATHS: RefCell<HashMap<isize, Option<String>>> = RefCell::new(HashMap::new());
    }

    pub struct RawInput;

    impl Source for RawInput {
        fn start(self: Box<Self>, sink: Sink) -> Result<Stop, String> {
            let (ready, started) = mpsc::channel();
            let thread = super::super::services::spawn_named("follow-typing", move || {
                SINK.set(Some(sink));
                // SAFETY: the window and its registration live on this thread, in its loop.
                let result = unsafe { listen() };
                match result {
                    Ok(hwnd) => {
                        // SAFETY: plain thread id.
                        let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
                        // SAFETY: this thread's own message loop and window.
                        unsafe { run(hwnd) };
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                    }
                }
            });
            // Bounded: the thread answers as soon as its window is up.
            let id = match started.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(id)) => id,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err("the listener didn't start".into()),
            };
            Ok(Box::new(move || {
                // SAFETY: WM_QUIT to the listener's thread ends its loop.
                let _ = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
                // It unregisters before it ends, so a new listener's registration stays.
                let _ = thread.join();
            }))
        }
    }

    /// The message-only window, registered for every keyboard's Raw Input in the background.
    ///
    /// The process has one Raw Input registration per device class: this one takes the
    /// keyboard's from winit's, whose raw keyboard `DeviceEvent`s the app never reads.
    unsafe fn listen() -> Result<HWND, String> {
        let class: Vec<u16> = "FoxgloveFollowTyping\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        // A second start finds the class already there.
        let _ = unsafe { RegisterClassW(&wc) };
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|e| format!("no window: {e}"))?;
        let device = RAWINPUTDEVICE {
            usUsagePage: DESKTOP,
            usUsage: KEYBOARD,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: hwnd,
        };
        if let Err(e) =
            unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) }
        {
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err(format!("no Raw Input: {e}"));
        }
        Ok(hwnd)
    }

    /// The loop until WM_QUIT, then the registration and the window go.
    unsafe fn run(hwnd: HWND) {
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            let _ = unsafe { DispatchMessageW(&msg) };
        }
        let device = RAWINPUTDEVICE {
            usUsagePage: DESKTOP,
            usUsage: KEYBOARD,
            dwFlags: RIDEV_REMOVE,
            hwndTarget: HWND::default(),
        };
        let _ = unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) };
        let _ = unsafe { DestroyWindow(hwnd) };
        SINK.set(None);
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_INPUT => {
                if let Some(path) = device_path(device_of(HRAWINPUT(lparam.0 as _))) {
                    SINK.with_borrow_mut(|s| {
                        if let Some(sink) = s.as_mut() {
                            sink(&path);
                        }
                    });
                }
            }
            // A handle may be reused for another device: forget its path.
            WM_INPUT_DEVICE_CHANGE => {
                PATHS.with_borrow_mut(|p| p.remove(&lparam.0));
            }
            _ => {}
        }
        // SAFETY: the default handling, which also frees WM_INPUT's buffer.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// The device that sent an event: its header alone (RID_HEADER), never the key.
    fn device_of(input: HRAWINPUT) -> HANDLE {
        let mut header = RAWINPUTHEADER::default();
        let mut size = size_of::<RAWINPUTHEADER>() as u32;
        // SAFETY: `header` holds `size` bytes, and RID_HEADER fills the header only.
        let got = unsafe {
            GetRawInputData(
                input,
                RID_HEADER,
                Some(std::ptr::from_mut(&mut header).cast()),
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if got == u32::MAX {
            return HANDLE::default();
        }
        header.hDevice
    }

    /// A device handle's path, asked once per handle. Input sent by software has no device.
    fn device_path(device: HANDLE) -> Option<String> {
        if device.is_invalid() {
            return None;
        }
        PATHS.with_borrow_mut(|paths| {
            paths
                .entry(device.0 as isize)
                .or_insert_with(|| name_of(device))
                .clone()
        })
    }

    fn name_of(device: HANDLE) -> Option<String> {
        let mut chars = 0u32;
        // SAFETY: with no buffer, Windows writes the name's length in characters.
        unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, None, &mut chars) };
        if chars == 0 {
            return None;
        }
        let mut name = vec![0u16; chars as usize];
        // SAFETY: `name` holds `chars` characters.
        let got = unsafe {
            GetRawInputDeviceInfoW(
                Some(device),
                RIDI_DEVICENAME,
                Some(name.as_mut_ptr().cast()),
                &mut chars,
            )
        };
        if got == u32::MAX || got == 0 {
            return None;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..end]))
    }
}

#[cfg(test)]
#[path = "follow_typing_tests.rs"]
mod tests;
