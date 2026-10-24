//! The Kit end to end: Den's runner starts it as Foxglove does, a sender's E1.31 frame on
//! localhost colours a mock keyboard, and the lights go back when the sender stops.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use den::runner::{Config, KitSpec, Runner};
use den::{Client, Context, Devices, Limits};
use foxglove_kit_api::permission::Permission;
use foxglove_kit_api::rpc::Error;
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

fn send(r: u8, g: u8, b: u8) {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_openrgb-e131"))
        .args(["--send-test", &r.to_string(), &g.to_string(), &b.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
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
fn a_frame_colours_the_keyboard_and_losing_the_sender_restores_it() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_openrgb-e131"));
    let keyboard = Arc::new(Keyboard::default());
    let devices = keyboard.clone();
    let spec = KitSpec {
        id: "test.openrgb-e131".into(),
        dir: exe.parent().unwrap().to_path_buf(),
        run: vec![exe.file_name().unwrap().to_string_lossy().into()],
        sandbox: false,
    };
    let context = move || Context {
        facts: palm::facts::Facts::default(),
        devices: devices.clone(),
        client: Client::Kit(vec![Permission::DevicesRead, Permission::LightingWrite, Permission::Net]),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});
    // Give it time to connect and bind the port, then send a red frame.
    std::thread::sleep(Duration::from_millis(800));
    send(255, 0, 0);
    let sets = wait_for(&keyboard, 1, Duration::from_secs(3));
    assert_eq!(
        sets.first(),
        Some(&json!({"device": "kb", "on": true, "color": {"hue": 0, "saturation": 100, "brightness": 100}, "effect": "0"})),
        "{sets:?}"
    );
    // The same colour again changes nothing.
    send(255, 0, 0);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(keyboard.sets.lock().unwrap().len(), 1);
    // No more frames: after E1.31's 2.5 s loss timeout the lights go back.
    let sets = wait_for(&keyboard, 2, Duration::from_secs(5));
    assert_eq!(
        sets.get(1),
        Some(&json!({"device": "kb", "on": true, "color": {"hue": 200, "saturation": 50, "brightness": 40}, "effect": "3"})),
        "{sets:?}"
    );
    drop(runner);
}
