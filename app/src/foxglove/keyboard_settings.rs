//! Talking to the keyboard's settings and actions (0xD7) off the UI thread: the RGB sync
//! switch, both halves' lights, and invoking any action from the keymap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use digitalis::link::Link;
use digitalis::protocol::RgbState;

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
static LINK: Mutex<Option<Arc<Link>>> = Mutex::new(None);

/// The keyboard with the host link, found once and kept while it stays connected.
fn link() -> Option<Arc<Link>> {
    let mut cached = LINK.lock().unwrap();
    if let Some(l) = cached.as_ref().filter(|l| l.state().connected.is_some()) {
        return Some(l.clone());
    }
    *cached = super::link::host_link().map(|(l, _)| l);
    cached.clone()
}

pub fn rgb_sync() -> RgbSync {
    *RGB_SYNC.lock().unwrap()
}

/// Reads the setting (`set` = `None`) or changes it, in the background; `ctx` is asked to
/// repaint when the answer is in.
pub fn ask_rgb_sync(set: Option<bool>, ctx: egui::Context) {
    *RGB_SYNC.lock().unwrap() = RgbSync::Asking;
    std::thread::spawn(move || {
        let state = match link() {
            None => RgbSync::NoKeyboard,
            Some(link) => match digitalis::settings::rgb_sync(link.as_ref(), set) {
                Ok(reply) => match reply.sync {
                    Some(on) => RgbSync::Known { on },
                    None => RgbSync::NotSupported,
                },
                Err(_) => RgbSync::NotSupported,
            },
        };
        *RGB_SYNC.lock().unwrap() = state;
        ctx.request_repaint();
    });
}

/// The last action invoked from the app and how it went.
static LAST_ACTION: Mutex<Option<(String, String)>> = Mutex::new(None);
static INVOKING: AtomicBool = AtomicBool::new(false);

pub fn last_action() -> Option<(String, String)> {
    LAST_ACTION.lock().unwrap().clone()
}

pub fn invoking() -> bool {
    INVOKING.load(Ordering::Relaxed)
}

/// Presses `binding` (behavior local id, param1, param2) on the keyboard as a key would.
pub fn invoke(label: String, binding: (u16, u32, u32), hold_ms: u16, ctx: egui::Context) {
    if INVOKING.swap(true, Ordering::Relaxed) {
        return;
    }
    *LAST_ACTION.lock().unwrap() = Some((label.clone(), "sending…".into()));
    std::thread::spawn(move || {
        let (id, p1, p2) = binding;
        let result = match link() {
            None => "no keyboard with Foxglove's firmware found".to_string(),
            Some(link) => match digitalis::settings::invoke(link.as_ref(), id, p1, p2, hold_ms) {
                Ok(status) => status.to_string(),
                Err(e) => format!("no answer ({e}); newer keyboard firmware needed?"),
            },
        };
        *LAST_ACTION.lock().unwrap() = Some((label, result));
        INVOKING.store(false, Ordering::Relaxed);
        RGB_POLLED_AT.lock().unwrap().take(); // show the effect right away
        ctx.request_repaint();
    });
}

/// Both halves' lights, polled about once a second while someone looks.
static RGB_STATE: Mutex<Option<RgbState>> = Mutex::new(None);
static RGB_POLLED_AT: Mutex<Option<Instant>> = Mutex::new(None);
static RGB_POLLING: AtomicBool = AtomicBool::new(false);
const RGB_POLL_EVERY: Duration = Duration::from_secs(1);

/// The latest known state; asks for a fresh one when the last is a second old.
pub fn rgb_state(ctx: &egui::Context) -> Option<RgbState> {
    let due = RGB_POLLED_AT
        .lock()
        .unwrap()
        .is_none_or(|at| at.elapsed() >= RGB_POLL_EVERY);
    if due && !RGB_POLLING.swap(true, Ordering::Relaxed) {
        *RGB_POLLED_AT.lock().unwrap() = Some(Instant::now());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let state = link().and_then(|l| digitalis::settings::rgb_state(l.as_ref()).ok());
            *RGB_STATE.lock().unwrap() = state;
            RGB_POLLING.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
    ctx.request_repaint_after(RGB_POLL_EVERY);
    *RGB_STATE.lock().unwrap()
}

// `&rgb_ug` commands (ZMK's dt-bindings/zmk/rgb.h).
const RGB_ON: u32 = 1;
const RGB_OFF: u32 = 2;
const RGB_EFFECT_SET: u32 = 13;
const RGB_COLOR_HSB: u32 = 14;

/// Lights on or off on both halves, through the keymap's `&rgb_ug`.
pub fn set_lights(on: bool, ctx: egui::Context) {
    if let Some(id) = super::keyboard_actions::underglow_behavior() {
        let (label, cmd) = if on {
            ("Lights on", RGB_ON)
        } else {
            ("Lights off", RGB_OFF)
        };
        invoke(label.into(), (id, cmd, 0), 0, ctx);
    }
}

/// Colour: hue 0-359, saturation and brightness 0-100.
pub fn set_color(hue: u16, sat: u8, brightness: u8, ctx: egui::Context) {
    if let Some(id) = super::keyboard_actions::underglow_behavior() {
        let hsb = (u32::from(hue.min(359)) << 16)
            | (u32::from(sat.min(100)) << 8)
            | u32::from(brightness.min(100));
        invoke("Set colour".into(), (id, RGB_COLOR_HSB, hsb), 0, ctx);
    }
}

/// Effect: 0 solid, 1 breathe, 2 spectrum, 3 swirl.
pub fn set_effect(effect: u8, ctx: egui::Context) {
    if let Some(id) = super::keyboard_actions::underglow_behavior() {
        invoke(
            "Set effect".into(),
            (id, RGB_EFFECT_SET, u32::from(effect)),
            0,
            ctx,
        );
    }
}
