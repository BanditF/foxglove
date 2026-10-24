//! Settings saved on the keyboard itself (0xD7), read and changed off the UI thread.

use std::sync::Mutex;

/// What Foxglove knows about "right half's lights follow the left".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RgbSync {
    /// Not asked yet.
    Unknown,
    Asking,
    /// No keyboard with Foxglove's host link is connected.
    NoKeyboard,
    /// The keyboard's firmware doesn't have the setting (or didn't answer).
    NotSupported,
    Known {
        on: bool,
    },
}

static RGB_SYNC: Mutex<RgbSync> = Mutex::new(RgbSync::Unknown);

pub fn rgb_sync() -> RgbSync {
    *RGB_SYNC.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reads the setting (`set` = `None`) or changes it, in the background; `ctx` is asked to
/// repaint when the answer is in.
pub fn ask_rgb_sync(set: Option<bool>, ctx: egui::Context) {
    *RGB_SYNC.lock().unwrap_or_else(|e| e.into_inner()) = RgbSync::Asking;
    let _ = super::services::spawn_supervised("keyboard-settings", move || {
        let state = match super::link::host_link() {
            None => RgbSync::NoKeyboard,
            Some((link, _)) => match digitalis::settings::rgb_sync(link.as_ref(), set) {
                Ok(reply) => match reply.sync {
                    Some(on) => RgbSync::Known { on },
                    None => RgbSync::NotSupported,
                },
                Err(_) => RgbSync::NotSupported,
            },
        };
        *RGB_SYNC.lock().unwrap_or_else(|e| e.into_inner()) = state;
        ctx.request_repaint();
    });
}
