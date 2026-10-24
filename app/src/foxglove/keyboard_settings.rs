//! Talking to the keyboard's settings and actions off the UI thread, through its Palm device:
//! the RGB sync switch, both halves' lights, and invoking any action from the keymap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use digitalis::protocol::RgbState;
use palm::{Device, Value};

use super::palm_host::{describe, device};

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
        let state = match device() {
            None => RgbSync::NoKeyboard,
            Some(d) => {
                let reply = d.settings().ok_or(palm::PalmError::Unsupported).and_then(|s| {
                    if let Some(on) = set {
                        s.set("rgb_sync", Value::Bool(on))?;
                    }
                    s.get("rgb_sync")
                });
                match reply {
                    Ok(Some(Value::Bool(on))) => RgbSync::Known { on },
                    _ => RgbSync::NotSupported,
                }
            }
        };
        *RGB_SYNC.lock().unwrap_or_else(|e| e.into_inner()) = state;
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

/// Presses `binding` (behavior local id, param1, param2) on the keyboard as a key would. The
/// hold time comes from the action learned from the keymap. Risky actions reach here only after
/// the panel's confirmation, so they go out confirmed.
pub fn invoke(label: String, binding: (u16, u32, u32), ctx: egui::Context) {
    if !super::keyboard_actions::permitted(binding) {
        *LAST_ACTION.lock().unwrap() = Some((label, "refused: unsafe or unknown behavior".into()));
        return;
    }
    let (id, p1, p2) = binding;
    run(label, ctx, move |d| {
        let behaviors = d.behaviors().ok_or(palm::PalmError::Unsupported)?;
        behaviors.invoke(id.into(), p1, p2, true)
    });
}

/// Runs one keyboard command in the background and reports it as the last action.
fn run(
    label: String,
    ctx: egui::Context,
    command: impl FnOnce(&digitalis::palm::DigitalisDevice) -> palm::Result<()> + Send + 'static,
) {
    if INVOKING.swap(true, Ordering::Relaxed) {
        return;
    }
    *LAST_ACTION.lock().unwrap() = Some((label.clone(), "sending…".into()));
    std::thread::spawn(move || {
        let result = match device() {
            None => "no keyboard with Foxglove's firmware found".to_string(),
            Some(d) => match command(&d) {
                Ok(()) => "done".to_string(),
                Err(e) => describe(&e),
            },
        };
        *LAST_ACTION.lock().unwrap() = Some((label, result));
        INVOKING.store(false, Ordering::Relaxed);
        refresh_rgb_state(); // show the effect right away
        ctx.request_repaint();
    });
}

/// Both halves' lights, refreshed on request and after actions.
static RGB_STATE: Mutex<Option<RgbState>> = Mutex::new(None);
static RGB_REFRESH: AtomicBool = AtomicBool::new(true);
static RGB_POLLING: AtomicBool = AtomicBool::new(false);

pub fn refresh_rgb_state() {
    RGB_REFRESH.store(true, Ordering::Relaxed);
}

/// Returns the cached state and reads it in the background when requested.
pub fn rgb_state(ctx: &egui::Context) -> Option<RgbState> {
    if RGB_REFRESH.load(Ordering::Relaxed) && !RGB_POLLING.swap(true, Ordering::Relaxed) {
        RGB_REFRESH.store(false, Ordering::Relaxed);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // Read from the link: Palm's lighting state has no per-half age yet.
            let state = device().and_then(|d| digitalis::settings::rgb_state(d.link().as_ref()).ok());
            *RGB_STATE.lock().unwrap() = state;
            RGB_POLLING.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
    *RGB_STATE.lock().unwrap()
}

fn lighting(
    label: &str,
    ctx: egui::Context,
    change: impl FnOnce(&dyn palm::Lighting) -> palm::Result<()> + Send + 'static,
) {
    run(label.into(), ctx, move |d| {
        change(d.lighting().ok_or(palm::PalmError::Unsupported)?)
    });
}

/// Lights on or off on both halves, through the keymap's `&rgb_ug`.
pub fn set_lights(on: bool, ctx: egui::Context) {
    let label = if on { "Lights on" } else { "Lights off" };
    lighting(label, ctx, move |l| l.set_on(on));
}

/// Colour: hue 0-359, saturation and brightness 0-100.
pub fn set_color(hue: u16, sat: u8, brightness: u8, ctx: egui::Context) {
    lighting("Set colour", ctx, move |l| l.set_color(hue, sat, brightness));
}

/// Effect: 0 solid, 1 breathe, 2 spectrum, 3 swirl.
pub fn set_effect(effect: u8, ctx: egui::Context) {
    lighting("Set effect", ctx, move |l| {
        l.set_effect(palm::Effect {
            id: effect.to_string(),
            params: vec![],
        })
    });
}
