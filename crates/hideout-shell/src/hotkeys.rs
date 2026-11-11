// SPDX-License-Identifier: MIT
//! The hub's bound keys as system-wide hotkeys (#991): one thread owns them with
//! `RegisterHotKey` (no window) and sleeps in `GetMessageW`. A press arrives as `WM_HOTKEY` and
//! goes to `on_press` (the router's `action.invoke {source: "key"}`), so a key works while the
//! hub's window is in the tray. A new set of keys arrives as a posted message; nothing wakes the
//! thread on a timer. A key that another app has registered is reported back, to show on its
//! Keys row ("Another app uses Ctrl+Alt+M.").

use std::sync::{Arc, Mutex};

use hideout_panels::keys::Chord;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY, WM_QUIT,
};

/// "The wanted keys changed": the thread registers them again.
const WM_REBIND: u32 = WM_APP + 7;

/// Runs on the hotkey thread for each press, with (module, action).
pub type OnPress = Box<dyn Fn(&str, &str) + Send>;

/// (module, action, key as people read it).
pub type Binding = (String, String, String);

#[derive(Default)]
struct Shared {
    wanted: Vec<Binding>,
    /// The bindings the system refused at the last registration, once per registration.
    refused: Option<Vec<Binding>>,
}

/// The hotkey thread's handle. Dropping it ends the thread and frees every key.
pub struct Hotkeys {
    thread: u32,
    shared: Arc<Mutex<Shared>>,
}

impl Hotkeys {
    /// Starts the thread. `on_press(module, action)` runs on it for each press; `wake` runs
    /// after each registration, so the shell picks up what was refused.
    pub fn start(on_press: OnPress, wake: Box<dyn Fn() + Send>) -> Option<Hotkeys> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("hideout-hotkeys".into())
            .spawn(move || run(s, on_press, wake, tx))
            .map_err(|e| log::warn!("hotkeys: no thread: {e}"))
            .ok()?;
        // The thread's id once its message queue exists (it posts no sooner).
        let thread = rx.recv().ok()?;
        Some(Hotkeys { thread, shared })
    }

    /// Registers exactly `keys` (the old ones go first).
    pub fn set(&self, keys: Vec<Binding>) {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).wanted = keys;
        if let Err(e) = unsafe { PostThreadMessageW(self.thread, WM_REBIND, WPARAM(0), LPARAM(0)) }
        {
            log::warn!("hotkeys: not posted: {e}");
        }
    }

    /// The keys the system refused at the last registration, once.
    pub fn take_refused(&self) -> Option<Vec<Binding>> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .refused
            .take()
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

fn run(
    shared: Arc<Mutex<Shared>>,
    on_press: OnPress,
    wake: Box<dyn Fn() + Send>,
    ready: std::sync::mpsc::Sender<u32>,
) {
    let mut msg = MSG::default();
    // Makes the thread's message queue before anyone posts to it.
    unsafe {
        let _ = PeekMessageW(&mut msg, None, WM_APP, WM_APP, PM_NOREMOVE);
    }
    let _ = ready.send(unsafe { GetCurrentThreadId() });
    // Registered now, by hotkey id (index + 1).
    let mut held: Vec<Binding> = Vec::new();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        match msg.message {
            WM_HOTKEY => {
                let id = msg.wParam.0;
                if let Some((m, a, k)) = id.checked_sub(1).and_then(|i| held.get(i)) {
                    log::info!("hotkeys: {k} runs {m} {a}");
                    on_press(m, a);
                }
            }
            WM_REBIND => {
                for i in 0..held.len() {
                    let _ = unsafe { UnregisterHotKey(None, i as i32 + 1) };
                }
                let wanted = shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .wanted
                    .clone();
                let mut refused = Vec::new();
                held.clear();
                for b in wanted {
                    let Some(chord) = Chord::parse(&b.2) else {
                        log::warn!("hotkeys: {} {}: {:?} is no key", b.0, b.1, b.2);
                        continue;
                    };
                    let id = held.len() as i32 + 1;
                    let mods = HOT_KEY_MODIFIERS(chord.win_modifiers()) | MOD_NOREPEAT;
                    match unsafe { RegisterHotKey(None, id, mods, chord.win_vk()) } {
                        Ok(()) => held.push(b),
                        Err(e) => {
                            log::info!("hotkeys: {} not registered: {e}", b.2);
                            refused.push(b);
                        }
                    }
                }
                log::info!("hotkeys: {} key(s) registered", held.len());
                shared.lock().unwrap_or_else(|e| e.into_inner()).refused = Some(refused);
                wake();
            }
            _ => {}
        }
    }
    for i in 0..held.len() {
        let _ = unsafe { UnregisterHotKey(None, i as i32 + 1) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Window-free: registers a key no one uses (Ctrl+Alt+Shift+F24), reports nothing refused,
    /// and frees it again when dropped. A second registration of the same key, from another
    /// thread, is refused while the first one holds it.
    #[test]
    fn keys_register_and_a_held_one_is_refused() {
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let wake = move || {
            let _ = wake_tx.send(());
        };
        let first = Hotkeys::start(Box::new(|_, _| {}), Box::new(wake.clone())).unwrap();
        let key = (
            "m.a".to_string(),
            "x".to_string(),
            "Ctrl+Alt+Shift+F24".to_string(),
        );
        first.set(vec![key.clone()]);
        wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(first.take_refused(), Some(vec![]));
        assert_eq!(first.take_refused(), None, "once");
        let second = Hotkeys::start(Box::new(|_, _| {}), Box::new(wake)).unwrap();
        second.set(vec![key.clone()]);
        wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(second.take_refused(), Some(vec![key]));
        second.set(vec![]);
        wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(second.take_refused(), Some(vec![]));
    }
}
