//! Foxglove's device-layer host: one event bus and facts store for the whole app, and the keyboard with
//! the host link as a device-layer device. Everything that talks to that keyboard goes through
//! [`device`], so the same code will serve other backends later.

use std::sync::{Arc, Mutex, OnceLock};

use digitalis::devices::DigitalisDevice;
use foxglove_devices::{BackendId, BehaviorInfo, DeviceId, DeviceInfo, DeviceKind, EventBus};

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
    if let Some(d) = cached
        .as_ref()
        .filter(|d| d.link().state().connected.is_some())
    {
        return Some(d.clone());
    }
    let mut found = None;
    let mut name = String::new();
    *cached = super::link::host_keyboard().map(|((vid, pid), link, hello)| {
        found = Some((vid, pid));
        let backend = BackendId("zmk".into());
        let product = link
            .state()
            .connected
            .map(|c| c.product)
            .unwrap_or_default();
        name.clone_from(&product);
        // Its board profile, read once now it's connected (#75).
        super::boards::connected(vid, pid, &product);
        let id = DeviceId::new(
            backend.clone(),
            vid,
            pid,
            None,
            &format!("{vid:04x}:{pid:04x}"),
        );
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
    let out = cached.clone();
    drop(cached);
    // Nothing learned from the keymap yet (the overlay hasn't opened it): use the cached one,
    // so plugins can change the lights without the UI. Outside the lock: learning sets it.
    if let Some((vid, pid)) = found {
        // Foxglove: the log said only "Waiting for a keyboard"; now it says when one arrives.
        eprintln!(
            "{}",
            connected_line(super::link::transport(vid, pid), &name)
        );
        super::keyboard_actions::learn_cached(vid, pid);
    }
    out
}

/// The log line for a keyboard that just connected: `Keyboard connected (USB): Eyelash Corne`.
fn connected_line(transport: &str, product: &str) -> String {
    if product.is_empty() {
        format!("Foxglove: Keyboard connected ({transport})")
    } else {
        format!("Foxglove: Keyboard connected ({transport}): {product}")
    }
}

/// Finds the keyboard with the host link each time a shared link connects, so the status strip,
/// the Keyboard page and plugins see a keyboard that connects after startup (or answers only
/// after a reconnect). Before, [`device`] ran only when something asked, and a "no keyboard"
/// from before the connection stayed until the next ask. Start: a link's State event with a
/// connection. Stop: none needed; the worker blocks on its channel, so an idle keyboard costs
/// no wake-ups.
pub fn start() {
    enum Msg {
        Link(Arc<digitalis::link::Link>),
        Connected,
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let links_tx = tx.clone();
    super::services::spawn_named("keyboard-discovery", move || {
        // Listeners only enqueue: the worker subscribes and discovers.
        let _links = super::link::subscribe_links(move |link| {
            let _ = links_tx.send(Msg::Link(link));
        });
        let mut subscriptions = Vec::new();
        for msg in rx {
            match msg {
                Msg::Link(link) => {
                    let tx = tx.clone();
                    subscriptions.push(link.subscribe_events(move |event| {
                        if let digitalis::link::Event::State(s) = event {
                            if s.connected.is_some() {
                                let _ = tx.send(Msg::Connected);
                            }
                        }
                    }));
                }
                Msg::Connected => {
                    let _ = device();
                }
            }
        }
    });
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

/// A device-layer error in the words the Keyboard panel uses.
pub fn describe(e: &foxglove_devices::DeviceError) -> String {
    match e {
        foxglove_devices::DeviceError::Unsupported => {
            "not supported by this keyboard or keymap".into()
        }
        foxglove_devices::DeviceError::Disconnected => "the keyboard is not connected".into(),
        foxglove_devices::DeviceError::Timeout => {
            "no answer; newer keyboard firmware needed?".into()
        }
        foxglove_devices::DeviceError::Busy(why)
        | foxglove_devices::DeviceError::Denied(why)
        | foxglove_devices::DeviceError::Backend(why) => why.clone(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_connect_is_logged_with_its_transport() {
        assert_eq!(
            super::connected_line("USB", "Eyelash Corne"),
            "Foxglove: Keyboard connected (USB): Eyelash Corne"
        );
        assert_eq!(
            super::connected_line("BLE", ""),
            "Foxglove: Keyboard connected (BLE)"
        );
    }
}
