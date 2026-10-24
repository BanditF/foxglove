//! Foxglove's Palm host: one event bus and facts store for the whole app, and the keyboard with
//! the host link as a Palm device. Everything that talks to that keyboard goes through
//! [`device`], so the same code will serve other backends later.

use std::sync::{Arc, Mutex, OnceLock};

use digitalis::palm::DigitalisDevice;
use palm::{BackendId, BehaviorInfo, DeviceId, DeviceInfo, DeviceKind, EventBus};

static BUS: OnceLock<EventBus> = OnceLock::new();
static DEVICE: Mutex<Option<Arc<DigitalisDevice>>> = Mutex::new(None);
/// The actions learned from the keymap and its `&rgb_ug` id, kept for the next device.
static ACTIONS: Mutex<(Vec<BehaviorInfo>, Option<u16>)> = Mutex::new((Vec::new(), None));

/// The app's event bus; its facts store holds every device's state under `device.<id>.*`.
pub fn bus() -> &'static EventBus {
    BUS.get_or_init(EventBus::default)
}

/// The keyboard with Foxglove's host link, found once and kept while it stays connected. May
/// block on discovery: call it off the UI thread.
pub fn device() -> Option<Arc<DigitalisDevice>> {
    let mut cached = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = cached.as_ref().filter(|d| d.link().state().connected.is_some()) {
        return Some(d.clone());
    }
    *cached = super::link::host_keyboard().map(|((vid, pid), link, hello)| {
        let backend = BackendId("zmk".into());
        let product = link.state().connected.map(|c| c.product).unwrap_or_default();
        let id = DeviceId::new(backend.clone(), vid, pid, None, &format!("{vid:04x}:{pid:04x}"));
        let info = DeviceInfo {
            id: id.clone(),
            backend,
            name: product,
            kind: DeviceKind::Keyboard,
            firmware: None,
        };
        let device = Arc::new(DigitalisDevice::new(info, link, hello, bus().sink(id)));
        let (actions, underglow) = ACTIONS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        device.set_behaviors(actions);
        device.set_underglow(underglow);
        device
    });
    cached.clone()
}

/// The current device's connection generation, without blocking or discovery (for the UI).
pub fn generation() -> Option<u64> {
    let cached = DEVICE.try_lock().ok()?;
    let state = cached.as_ref()?.link().state();
    state.connected.is_some().then_some(state.generation)
}

/// What the keymap lets the app invoke (the allowlist) and its `&rgb_ug` behavior.
pub fn set_actions(actions: Vec<BehaviorInfo>, underglow: Option<u16>) {
    if let Some(d) = DEVICE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        d.set_behaviors(actions.clone());
        d.set_underglow(underglow);
    }
    *ACTIONS.lock().unwrap_or_else(|e| e.into_inner()) = (actions, underglow);
}

/// A Palm error in the words the Keyboard panel uses.
pub fn describe(e: &palm::PalmError) -> String {
    match e {
        palm::PalmError::Unsupported => "not supported by this keyboard or keymap".into(),
        palm::PalmError::Disconnected => "the keyboard is not connected".into(),
        palm::PalmError::Timeout => "no answer; newer keyboard firmware needed?".into(),
        palm::PalmError::Busy(why) | palm::PalmError::Denied(why) | palm::PalmError::Backend(why) => {
            why.clone()
        }
    }
}
