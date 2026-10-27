//! The countdown screen at the size Foxglove gives (#560): the runner starts the focus timer as
//! Foxglove does, against a mock keyboard whose info screen isn't the Corne's 64x52.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_devices::facts::Facts;
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::Error;
use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Devices, Limits};
use serde_json::{json, Value};

/// Records what the plugin asks for; one keyboard with a 200x40 info screen.
#[derive(Default)]
struct Mock {
    calls: Mutex<Vec<(String, Value)>>,
}

impl Devices for Mock {
    fn call_from(&self, _conn: u64, method: &str, params: Value) -> Result<Value, Error> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params));
        Ok(match method {
            "devices.list" => {
                json!({"devices": [{"id": "kb", "name": "Test board", "kind": "keyboard",
                "backend": "mock", "connected": true, "caps": ["screens"]}]})
            }
            "screens.list" => json!({"screens": [
                {"id": 2, "role": "info", "name": "Info screen", "width": 200, "height": 40,
                 "format": "1bit", "rotation": 0}]}),
            _ => json!({}),
        })
    }
}

#[test]
fn the_countdown_is_drawn_at_the_screens_size() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_focus-timer"));
    let dir = std::env::temp_dir().join(format!("focus-timer-screen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let spec = PluginSpec {
        id: "test.focus-screen".into(),
        dir: dir.clone(),
        run: vec![
            exe.file_name().unwrap().to_string_lossy().into(),
            "--focus-seconds".into(),
            "600".into(),
            "--no-lights".into(),
        ],
        sandbox: cfg!(windows),
    };
    let mock = Arc::new(Mock::default());
    let m = mock.clone();
    let perms = ["devices.read", "screens.write", "facts.write:focus.*"]
        .map(|p| Permission::try_from(p.to_string()).unwrap())
        .to_vec();
    let context = move || Context {
        facts: Facts::default(),
        devices: m.clone(),
        client: Client::Plugin(perms.clone()),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});
    // A loaded machine (the whole gate at once) can take a while to start a sandboxed process.
    let until = Instant::now() + Duration::from_secs(120);
    let frame = loop {
        let found = mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .find(|(m, _)| m == "screens.panel_override")
            .map(|(_, p)| p.clone());
        if let Some(p) = found {
            break p;
        }
        assert!(
            Instant::now() < until,
            "never drew: {:?}",
            mock.calls.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    runner.stop();
    assert_eq!(
        (
            frame["width"].clone(),
            frame["height"].clone(),
            frame["screen"].clone()
        ),
        (json!(200), json!(40), json!(2))
    );
    use base64::Engine;
    let bits = base64::engine::general_purpose::STANDARD
        .decode(frame["bitmap"].as_str().unwrap())
        .unwrap();
    assert_eq!(bits.len(), 25 * 40, "a whole 200x40 frame");
    assert!(bits.iter().any(|b| *b != 0), "the countdown, not a blank");
    let _ = std::fs::remove_dir_all(dir);
}
