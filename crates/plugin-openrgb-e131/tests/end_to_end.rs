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

/// One keyboard with lights; records every `lighting.set`.
#[derive(Default)]
struct Keyboard {
    sets: Mutex<Vec<Value>>,
}

impl Devices for Keyboard {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        match method {
            "devices.list" => Ok(json!({"devices": [{"id": "kb", "name": "Test board", "kind": "keyboard",
                "backend": "mock", "connected": true, "caps": ["lighting"]}]})),
            "lighting.state" => {
                Ok(json!({"on": true, "hue": 200, "saturation": 50, "brightness": 40, "effect": "3", "speed": 1}))
            }
            "lighting.set" => {
                self.sets.lock().unwrap().push(params);
                Ok(json!({}))
            }
            _ => Err(Error { code: -32601, message: method.into(), data: None }),
        }
    }
}

fn send(seq: u8, r: u8, g: u8, b: u8, stop: bool) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_openrgb-e131"));
    cmd.args(["--send-test", &r.to_string(), &g.to_string(), &b.to_string(), "--seq", &seq.to_string()]);
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
    // In the sandbox (on Windows), as Foxglove runs it: the plugin has no network of its own,
    // and the frames reach it through the host. Its own folder, so the build folder's
    // permissions stay as they are.
    let dir = std::env::temp_dir().join(format!("openrgb-e131-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let spec = PluginSpec {
        id: "test.openrgb-e131".into(),
        dir: dir.clone(),
        run: vec![exe.file_name().unwrap().to_string_lossy().into()],
        sandbox: cfg!(windows),
    };
    let context = move || Context {
        facts: foxglove_devices::facts::Facts::default(),
        devices: devices.clone(),
        client: Client::Plugin(vec![Permission::DevicesRead, Permission::LightingWrite, Permission::NetListenUdp(5568)]),
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
        Some(&json!({"device": "kb", "on": true, "color": {"hue": 200, "saturation": 50, "brightness": 40}, "effect": "3"})),
        "{sets:?}"
    );
    drop(runner);
    #[cfg(windows)]
    foxglove_plugin_host::delete_container("test.openrgb-e131");
    let _ = std::fs::remove_dir_all(dir);
}
