//! Talking to the keyboard's settings and actions off the UI thread, through its device-layer device:
//! the RGB sync switch, both halves' lights, and invoking any action from the keymap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use digitalis::protocol::RgbState;
use foxglove_devices::{Device, Value};

use super::devices::{describe, device};

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
    // Paused (#404): read it, change nothing.
    let set = set.filter(|_| !super::pause::paused());
    *RGB_SYNC.lock().unwrap_or_else(|e| e.into_inner()) = RgbSync::Asking;
    let _ = super::services::spawn_supervised("keyboard-settings", move || {
        let state = match device() {
            None => RgbSync::NoKeyboard,
            Some(d) => {
                let reply = d
                    .settings()
                    .ok_or(foxglove_devices::DeviceError::Unsupported)
                    .and_then(|s| {
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
        let behaviors = d
            .behaviors()
            .ok_or(foxglove_devices::DeviceError::Unsupported)?;
        behaviors.invoke(id.into(), p1, p2, true)
    });
}

/// Runs one keyboard command in the background and reports it as the last action.
fn run(
    label: String,
    ctx: egui::Context,
    command: impl FnOnce(&digitalis::devices::DigitalisDevice) -> foxglove_devices::Result<()>
        + Send
        + 'static,
) {
    if super::pause::paused() {
        *LAST_ACTION.lock().unwrap() = Some((label, super::pause::REFUSED.into()));
        ctx.request_repaint();
        return;
    }
    if INVOKING.swap(true, Ordering::Relaxed) {
        return;
    }
    *LAST_ACTION.lock().unwrap() = Some((label.clone(), "sending…".into()));
    super::services::spawn_named("keyboard-command", move || {
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
/// When [`RGB_STATE`] was read, so the right half's age keeps counting while it is shown
/// (it stayed at "8 s ago" however long the page was open).
static RGB_READ_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static RGB_REFRESH: AtomicBool = AtomicBool::new(true);
static RGB_POLLING: AtomicBool = AtomicBool::new(false);

/// Both halves' lights as last read, without asking the keyboard.
pub fn cached_rgb_state() -> Option<RgbState> {
    *RGB_STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn refresh_rgb_state() {
    RGB_REFRESH.store(true, Ordering::Relaxed);
}

/// The keyboard reported its lights switching (an RGB_STATE report): read them again on the
/// next frame, and draw one so whatever shows them (the tray panel, the Lights page) follows.
pub fn lights_changed() {
    refresh_rgb_state();
    super::tray_panel::wake();
}

/// Returns the cached state and reads it in the background when requested.
pub fn rgb_state(ctx: &egui::Context) -> Option<RgbState> {
    if RGB_REFRESH.load(Ordering::Relaxed) && !RGB_POLLING.swap(true, Ordering::Relaxed) {
        RGB_REFRESH.store(false, Ordering::Relaxed);
        let ctx = ctx.clone();
        super::services::spawn_named("rgb-state", move || {
            // Read from the link: the device layer's lighting state has no per-half age yet.
            // In the palette's colours: what Foxglove asked for, not its calibrated form.
            let read = || {
                device().and_then(|d| {
                    let mut s = digitalis::settings::rgb_state(d.link().as_ref()).ok()?;
                    d.asked_state(&mut s);
                    Some(s)
                })
            };
            let mut state = read();
            // The right's half is the relay's last read, and each RGB_STATE starts a fresh one
            // that this reply can't carry (firmware art_relay_right_rgb). A stale or missing
            // right half ("off · 8 s ago" while it was lit, 2026-10-26) is asked once more,
            // after the relay read has had time to land. Bounded: two reads per refresh.
            if state.as_ref().is_some_and(right_is_stale) {
                std::thread::sleep(RIGHT_READ_SETTLE);
                state = read().or(state);
            }
            *RGB_STATE.lock().unwrap() = state;
            *RGB_READ_AT.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(std::time::Instant::now());
            RGB_POLLING.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
    let elapsed = RGB_READ_AT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map_or(0, |at| at.elapsed().as_secs());
    RGB_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map(|s| aged(s, elapsed))
}

/// `s` as read `elapsed_s` seconds ago: the right half's age grows by that much (the UI adds
/// no timer for it; the age is right whenever the page is drawn).
fn aged(mut s: RgbState, elapsed_s: u64) -> RgbState {
    if let Some((_, age)) = &mut s.right {
        *age = u8::try_from(u64::from(*age) + elapsed_s)
            .unwrap_or(u8::MAX)
            .min(254);
    }
    s
}

/// How long a relay read of the right half takes to land after RGB_STATE asked for it: one
/// BLE status read between the halves, well under this.
const RIGHT_READ_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// The right half's data is older than the UI calls current ("· N s ago" past 3 s), or there
/// is none yet, while the left answered: one more read gets the relay's fresh one.
fn right_is_stale(s: &RgbState) -> bool {
    s.right.is_none_or(|(_, age)| age > STALE_AFTER_S)
}

/// The age (seconds) past which the Lights page shows "· N s ago".
pub const STALE_AFTER_S: u8 = 3;

fn lighting(
    label: &str,
    ctx: egui::Context,
    change: impl FnOnce(&dyn foxglove_devices::Lighting) -> foxglove_devices::Result<()>
        + Send
        + 'static,
) {
    run(label.into(), ctx, move |d| {
        change(
            d.lighting()
                .ok_or(foxglove_devices::DeviceError::Unsupported)?,
        )
    });
}

/// Lights on or off on both halves, through the keymap's `&rgb_ug`.
pub fn set_lights(on: bool, ctx: egui::Context) {
    let label = if on { "Lights on" } else { "Lights off" };
    lighting(label, ctx, move |l| l.set_on(on));
}

/// Colour: hue 0-359, saturation and brightness 0-100.
pub fn set_color(hue: u16, sat: u8, brightness: u8, ctx: egui::Context) {
    lighting("Set colour", ctx, move |l| {
        l.set_color(hue, sat, brightness)?;
        // The case's "Match keyboard" follows (#423).
        super::pc_lights::keyboard_color(hue, sat, brightness);
        Ok(())
    });
}

/// Effect: 0 solid, 1 breathe, 2 spectrum, 3 swirl.
pub fn set_effect(effect: u8, ctx: egui::Context) {
    lighting("Set effect", ctx, move |l| {
        l.set_effect(foxglove_devices::Effect {
            id: effect.to_string(),
            params: vec![],
        })
    });
}

/// Effect speed 1-5 (#387). ZMK only steps it, so the device steps toward it and reads back.
pub fn set_speed(speed: u8, ctx: egui::Context) {
    lighting("Set speed", ctx, move |l| l.set_speed(speed));
}

/// Both halves' firmware builds (#132), read once per connection.
#[derive(Clone, Debug, PartialEq)]
pub enum Firmware {
    Unknown,
    Reading,
    /// No keyboard with the host link; read again once one connects.
    NoKeyboard,
    /// The firmware doesn't report its build.
    NotReported,
    Known {
        left: Result<digitalis::protocol::BuildInfo, String>,
        right: Result<digitalis::protocol::BuildInfo, String>,
    },
}

static FIRMWARE: Mutex<(Option<u64>, Firmware)> = Mutex::new((None, Firmware::Unknown));
/// Set when the settings window opens: a half that didn't answer is asked again (#353).
static RETRY_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The settings window opened. A half whose build read failed (the right one is often still
/// joining after a connect) is read again on the next look, not on a timer.
pub fn settings_opened() {
    RETRY_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// "Ask the keyboard again" (#386): forget the builds, so the next look reads them afresh.
/// A read already under way is left to finish.
pub fn read_firmware_again() {
    let mut cached = FIRMWARE.lock().unwrap_or_else(|e| e.into_inner());
    if cached.1 != Firmware::Reading {
        cached.1 = Firmware::Unknown;
    }
}

impl Firmware {
    fn failed(&self) -> bool {
        matches!(self, Firmware::Known { left, right } if left.is_err() || right.is_err())
    }
}

/// The cached builds; reads them in the background when there's a new connection.
pub fn firmware(ctx: &egui::Context) -> Firmware {
    let generation = super::devices::generation();
    let mut cached = FIRMWARE.lock().unwrap_or_else(|e| e.into_inner());
    let retry = cached.1.failed() && RETRY_FAILED.swap(false, std::sync::atomic::Ordering::Relaxed);
    let stale = cached.0 != generation || cached.1 == Firmware::Unknown || retry;
    if stale && cached.1 != Firmware::Reading {
        cached.1 = Firmware::Reading;
        // Only an opening after this read retries what it leaves failed.
        RETRY_FAILED.store(false, std::sync::atomic::Ordering::Relaxed);
        let ctx = ctx.clone();
        super::services::spawn_named("firmware-info", move || {
            let read = match device() {
                None => (None, Firmware::NoKeyboard),
                Some(d) => {
                    let link = d.link();
                    let state = link.state();
                    let caps = state
                        .connected
                        .and_then(|c| c.hello)
                        .map_or(0, |h| h.capabilities);
                    let firmware = if caps & digitalis::protocol::capabilities::BUILD_INFO == 0 {
                        Firmware::NotReported
                    } else {
                        let half = |n| {
                            digitalis::settings::build_info(link.as_ref(), n)
                                .map_err(|e| e.to_string())
                        };
                        Firmware::Known {
                            left: half(0),
                            right: half(1),
                        }
                    };
                    (Some(state.generation), firmware)
                }
            };
            *FIRMWARE.lock().unwrap_or_else(|e| e.into_inner()) = read;
            ctx.request_repaint();
        });
    }
    cached.1.clone()
}

#[cfg(test)]
mod rgb_state_tests {
    use digitalis::protocol::{RgbHalf, RgbState};

    fn with_right(right: Option<(bool, u8)>) -> RgbState {
        RgbState {
            left: RgbHalf {
                on: true,
                ..RgbHalf::default()
            },
            right: right.map(|(on, age)| {
                (
                    RgbHalf {
                        on,
                        ..RgbHalf::default()
                    },
                    age,
                )
            }),
        }
    }

    /// The live case: the right read as off 8 s ago while it was lit. That reply is stale, so
    /// the refresh asks once more; a fresh right half is taken as it is.
    #[test]
    fn the_right_half_keeps_ageing_while_shown() {
        let s = with_right(Some((false, 8)));
        assert_eq!(super::aged(s, 0).right.map(|(_, a)| a), Some(8));
        assert_eq!(super::aged(s, 30).right.map(|(_, a)| a), Some(38));
        assert_eq!(super::aged(s, 10_000).right.map(|(_, a)| a), Some(254));
        assert_eq!(super::aged(with_right(None), 30).right, None);
    }

    #[test]
    fn a_stale_or_missing_right_half_is_read_once_more() {
        assert!(super::right_is_stale(&with_right(Some((false, 8)))));
        assert!(super::right_is_stale(&with_right(None)));
        assert!(!super::right_is_stale(&with_right(Some((true, 0)))));
        assert!(!super::right_is_stale(&with_right(Some((
            false,
            super::STALE_AFTER_S
        )))));
    }
}

#[cfg(test)]
mod firmware_tests {
    use super::Firmware;

    #[test]
    fn only_a_failed_half_is_read_again() {
        let failed = Firmware::Known {
            left: Err("x".into()),
            right: Err("no reply from the keyboard".into()),
        };
        assert!(failed.failed());
        assert!(!Firmware::Unknown.failed());
        assert!(!Firmware::NotReported.failed());
        assert!(!Firmware::NoKeyboard.failed());
    }
}
