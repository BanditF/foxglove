//! Keys that trigger plugin actions (#212). A plugin declares actions in `plugin.toml`; the user
//! binds each to one of F13–F24 in the Plugins tab. Every keyboard has those keys and they type
//! nothing, so the user puts one on a key in ZMK Studio or VIA, and no plugin needs
//! `key.events`. Foxglove registers the bound keys as global hotkeys and sends
//! `action.invoked` to that plugin only. Bindings live in the store's `[plugins.bindings]`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The keys actions can be bound to.
pub const KEYS: [&str; 12] = ["F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub plugin: String,
    pub action: String,
}

/// Key name (`F13`) to what it triggers.
pub type Bindings = BTreeMap<String, Binding>;

pub fn bindings() -> Bindings {
    super::store::try_load("plugins.bindings").unwrap_or_default()
}

/// The key bound to a plugin's action, if any.
pub fn key_for(all: &Bindings, plugin: &str, action: &str) -> Option<String> {
    all.iter().find(|(_, b)| b.plugin == plugin && b.action == action).map(|(k, _)| k.clone())
}

/// Binds `key` (or nothing) to a plugin's action: a key triggers one action, and an action has
/// one key. Pure, for [`bind`] and the tests.
pub fn rebind(all: &mut Bindings, plugin: &str, action: &str, key: Option<&str>) {
    all.retain(|k, b| !(b.plugin == plugin && b.action == action) && Some(k.as_str()) != key);
    if let Some(key) = key.filter(|k| KEYS.contains(k)) {
        all.insert(key.into(), Binding { plugin: plugin.into(), action: action.into() });
    }
}

/// Saves a binding and re-registers the hotkeys.
pub fn bind(plugin: &str, action: &str, key: Option<&str>) -> Result<(), String> {
    let mut all = bindings();
    rebind(&mut all, plugin, action, key);
    super::store::save("plugins.bindings", &all).map_err(|e| e.to_string())?;
    imp::reload();
    Ok(())
}

/// Keys another program already holds, from the last registration.
static TAKEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn taken() -> Vec<String> {
    TAKEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn pressed(key: &str) {
    if let Some(b) = bindings().get(key) {
        foxglove_plugin_host::notify(&b.plugin, "action.invoked", serde_json::json!({ "action": b.action }));
    }
}

pub fn start() {
    if !super::repaint_trace::disabled("actions") {
        imp::start();
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT, VK_F13};
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PostThreadMessageW, MSG, WM_APP, WM_HOTKEY};

    use super::{bindings, pressed, KEYS, TAKEN};

    /// The hotkey thread, to wake when the bindings change.
    static THREAD: AtomicU32 = AtomicU32::new(0);
    const RELOAD: u32 = WM_APP + 1;

    pub fn start() {
        super::super::services::spawn_named("plugin-actions", || unsafe {
            THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);
            let mut registered: Vec<i32> = Vec::new();
            let register = |registered: &mut Vec<i32>| {
                for id in registered.drain(..) {
                    let _ = UnregisterHotKey(None, id);
                }
                let mut taken = Vec::new();
                for key in bindings().keys() {
                    let Some(i) = KEYS.iter().position(|k| k == key) else { continue };
                    let id = i as i32 + 1;
                    let vk = u32::from(VK_F13.0) + i as u32;
                    if RegisterHotKey(None, id, HOT_KEY_MODIFIERS(MOD_NOREPEAT.0), vk).is_ok() {
                        registered.push(id);
                    } else {
                        taken.push(key.clone());
                    }
                }
                *TAKEN.lock().unwrap_or_else(|e| e.into_inner()) = taken;
            };
            register(&mut registered);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_HOTKEY => {
                        if let Some(key) = KEYS.get(msg.wParam.0.wrapping_sub(1)) {
                            pressed(key);
                        }
                    }
                    RELOAD => register(&mut registered),
                    _ => {}
                }
            }
        });
    }

    pub fn reload() {
        let thread = THREAD.load(Ordering::Relaxed);
        if thread != 0 {
            unsafe {
                let _ = PostThreadMessageW(thread, RELOAD, Default::default(), Default::default());
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn start() {}
    pub fn reload() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_triggers_one_action_and_an_action_has_one_key() {
        let mut all = Bindings::new();
        rebind(&mut all, "a.timer", "pause", Some("F13"));
        rebind(&mut all, "a.timer", "reset", Some("F14"));
        assert_eq!(key_for(&all, "a.timer", "pause").as_deref(), Some("F13"));
        // Moving an action to another key frees the old one.
        rebind(&mut all, "a.timer", "pause", Some("F15"));
        assert_eq!(all.keys().collect::<Vec<_>>(), ["F14", "F15"]);
        // Binding a taken key takes it over.
        rebind(&mut all, "b.mic", "toggle", Some("F14"));
        assert_eq!(all["F14"], Binding { plugin: "b.mic".into(), action: "toggle".into() });
        assert_eq!(key_for(&all, "a.timer", "reset"), None);
        // Unbinding, and keys outside F13-F24, leave nothing.
        rebind(&mut all, "b.mic", "toggle", None);
        rebind(&mut all, "b.mic", "toggle", Some("A"));
        assert_eq!(all.keys().collect::<Vec<_>>(), ["F15"]);
    }
}
