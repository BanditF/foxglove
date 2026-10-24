//! The plugin end to end: the plugin host's runner starts it as Foxglove does, a sender's E1.31 frame on
//! localhost colours a mock keyboard, and the lights go back when the sender ends its stream.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Devices, Limits};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::Error;
use serde_json::{json, Value};

/// One keyboard with lights: it records every `lighting.set`, keeps its state, and can lose
/// writes (`drop`), as a busy keyboard sometimes does.
struct Keyboard {
    sets: Mutex<Vec<Value>>,
    state: Mutex<Value>,
    drop: std::sync::atomic::AtomicUsize,
    /// Writes whose colour part is lost (the rest applies).
    drop_color: std::sync::atomic::AtomicUsize,
    /// Read-backs that report the other half as not caught up yet.
    unsettled: std::sync::atomic::AtomicUsize,
    reads: std::sync::atomic::AtomicUsize,
    /// When the restore (effect 3) arrives, the next read-backs report the other half lagging.
    lag_on_restore: std::sync::atomic::AtomicUsize,
}

impl Default for Keyboard {
    fn default() -> Self {
        Keyboard {
            sets: Mutex::default(),
            state: Mutex::new(json!({"on": false, "hue": 200, "saturation": 50, "brightness": 40, "effect": "3", "speed": 1})),
            drop: Default::default(),
            drop_color: Default::default(),
            unsettled: Default::default(),
            reads: Default::default(),
            lag_on_restore: Default::default(),
        }
    }
}

impl Devices for Keyboard {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        match method {
            "devices.list" => Ok(json!({"devices": [{"id": "kb", "name": "Test board", "kind": "keyboard",
                "backend": "mock", "connected": true, "caps": ["lighting"]}]})),
            "lighting.state" => {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lagging = self.unsettled.try_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |n| n.checked_sub(1)).is_ok();
                let mut state = self.state.lock().unwrap().clone();
                state["settled"] = json!(!lagging);
                Ok(state)
            }
            "lighting.set" => {
                self.sets.lock().unwrap().push(params.clone());
                if params.get("effect") == Some(&json!("3")) {
                    let lag = self.lag_on_restore.swap(0, std::sync::atomic::Ordering::Relaxed);
                    if lag > 0 {
                        self.unsettled.store(lag, std::sync::atomic::Ordering::Relaxed);
                        self.reads.store(0, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                let lost = self.drop.try_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |n| n.checked_sub(1)).is_ok();
                if !lost {
                    let mut state = self.state.lock().unwrap();
                    if let Some(on) = params.get("on").filter(|v| !v.is_null()) {
                        state["on"] = on.clone();
                    }
                    let color_lost = self.drop_color.try_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |n| n.checked_sub(1)).is_ok();
                    if let Some(c) = params.get("color").filter(|v| !v.is_null() && !color_lost) {
                        state["hue"] = c["hue"].clone();
                        state["saturation"] = c["saturation"].clone();
                        state["brightness"] = c["brightness"].clone();
                    }
                    if let Some(e) = params.get("effect").filter(|v| !v.is_null()) {
                        state["effect"] = e.clone();
                    }
                }
                Ok(json!({}))
            }
            _ => Err(Error { code: -32601, message: method.into(), data: None }),
        }
    }
}

/// A free UDP port for this run, so tests and other runs on the machine don't share E1.31's.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

static PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

fn port() -> u16 {
    *PORT.get_or_init(free_port)
}

fn send(seq: u8, r: u8, g: u8, b: u8, stop: bool) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_openrgb-e131"));
    cmd.args(["--send-test", &r.to_string(), &g.to_string(), &b.to_string(), "--seq", &seq.to_string()]);
    cmd.args(["--port", &port().to_string()]);
    if stop {
        cmd.arg("--stop");
    }
    assert!(cmd.status().unwrap().success());
}

fn wait_for(keyboard: &Keyboard, n: usize, within: Duration) -> Vec<Value> {
    let until = Instant::now() + within;
    loop {
        let sets = keyboard.sets.lock().unwrap().clone();
        if sets.len() >= n || Instant::now() > until {
            return sets;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_frame_colours_the_keyboard_and_the_sender_stopping_restores_it() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_openrgb-e131"));
    let keyboard = Arc::new(Keyboard::default());
    let devices = keyboard.clone();
    let spec = PluginSpec {
        id: "test.openrgb-e131".into(),
        dir: exe.parent().unwrap().to_path_buf(),
        run: vec![exe.file_name().unwrap().to_string_lossy().into(), "--port".into(), port().to_string()],
        sandbox: false,
    };
    let context = move || Context {
        facts: foxglove_devices::facts::Facts::default(),
        devices: devices.clone(),
        client: Client::Plugin(vec![Permission::DevicesRead, Permission::LightingWrite, Permission::Net]),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});
    // Waits are for outcomes, with generous limits; nothing depends on how fast the machine is.
    // A red frame, sent until it lands (the plugin may still be starting and binding the port).
    let until = Instant::now() + Duration::from_secs(30);
    let mut seq = 0u8;
    let mut sets = vec![];
    while sets.is_empty() && Instant::now() < until {
        seq += 1;
        send(seq, 255, 0, 0, false);
        sets = wait_for(&keyboard, 1, Duration::from_millis(500));
    }
    assert_eq!(
        sets.first(),
        Some(&json!({"device": "kb", "on": true, "color": {"hue": 0, "saturation": 100, "brightness": 100}, "effect": "0"})),
        "{sets:?}"
    );
    // The same colour again, then the sender ends its stream: the next (and only) change is the
    // restore, so the repeated red sent nothing.
    send(seq + 1, 255, 0, 0, false);
    send(seq + 2, 0, 0, 0, true);
    let sets = wait_for(&keyboard, 2, Duration::from_secs(30));
    assert_eq!(
        sets.get(1),
        Some(&json!({"device": "kb", "on": false, "color": {"hue": 200, "saturation": 50, "brightness": 40}, "effect": "3"})),
        "{sets:?}"
    );
    // A new takeover whose first write the keyboard loses: the plugin reads the lights back and
    // sends the colour again, so the keyboard ends up green.
    keyboard.drop.store(1, std::sync::atomic::Ordering::Relaxed);
    let until = Instant::now() + Duration::from_secs(30);
    let mut green = seq + 3;
    while keyboard.state.lock().unwrap()["hue"] != json!(120) {
        assert!(Instant::now() < until, "never green: {:?}", keyboard.sets.lock().unwrap());
        send(green, 0, 255, 0, false);
        green = green.wrapping_add(1);
        std::thread::sleep(Duration::from_millis(300));
    }
    let sets = keyboard.sets.lock().unwrap().clone();
    let full_green = json!({"device": "kb", "on": true, "color": {"hue": 120, "saturation": 100, "brightness": 100}, "effect": "0"});
    assert!(sets.iter().filter(|s| **s == full_green).count() >= 2, "the lost write was sent again: {sets:?}");
    // Another sender (a new process, so a new CID) numbering from 1 again within the same
    // session: not stale, since sequences count per sender (E1.31).
    send(1, 0, 0, 255, false);
    let until = Instant::now() + Duration::from_secs(30);
    while keyboard.state.lock().unwrap()["hue"] != json!(240) {
        assert!(Instant::now() < until, "a new sender's frame was dropped: {:?}", keyboard.sets.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
    // The sender stops, and the restore's colour write is lost while the lights go off: the
    // keyboard would keep the plugin's colour for the next time they come on. The colour is
    // checked even when off, so it's sent again.
    keyboard.drop_color.store(1, std::sync::atomic::Ordering::Relaxed);
    send(green.wrapping_add(1), 0, 0, 0, true);
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let state = keyboard.state.lock().unwrap().clone();
        if state["hue"] == json!(200) {
            break;
        }
        assert!(Instant::now() < until, "the restore's lost colour wasn't sent again: {state}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // A split keyboard whose other half lags: after the restore the plugin keeps reading until
    // both halves agree (here 4 read-backs), instead of trusting the left alone.
    let until = Instant::now() + Duration::from_secs(30);
    let mut seq2 = green.wrapping_add(10);
    while keyboard.state.lock().unwrap()["hue"] != json!(60) {
        assert!(Instant::now() < until, "never yellow");
        send(seq2, 255, 255, 0, false);
        seq2 = seq2.wrapping_add(1);
        std::thread::sleep(Duration::from_millis(200));
    }
    keyboard.lag_on_restore.store(4, std::sync::atomic::Ordering::Relaxed);
    send(seq2, 0, 0, 0, true);
    let until = Instant::now() + Duration::from_secs(30);
    // Four unsettled read-backs, then the settled one: five at least.
    while keyboard.lag_on_restore.load(std::sync::atomic::Ordering::Relaxed) > 0
        || keyboard.reads.load(std::sync::atomic::Ordering::Relaxed) < 5
        || keyboard.state.lock().unwrap()["hue"] != json!(200)
    {
        assert!(Instant::now() < until, "the restore didn't wait for the other half");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(keyboard.unsettled.load(std::sync::atomic::Ordering::Relaxed), 0);
    drop(runner);
}
