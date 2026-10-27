//! Mic mute end to end: the runner starts it sandboxed as Foxglove does, against a mock keyboard
//! and mock audio. `mic.state` facts drive it; a key asks to unmute.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_devices::facts::{Facts, Value as DeviceValue};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::Error;
use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Devices, Limits};
use serde_json::{json, Value};

/// Records what the plugin asks for.
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
                "backend": "mock", "connected": true, "caps": ["lighting", "screens"]}]})
            }
            "lighting.lease" => json!({"lease": self.calls.lock().unwrap().len()}),
            // #560: an info screen of another size than the Corne's, with its id.
            "screens.list" => json!({"screens": [
                {"id": 4, "role": "info", "name": "Info screen", "width": 128, "height": 32,
                 "format": "1bit", "rotation": 0},
                {"id": 5, "role": "art", "name": "Art screen", "width": 160, "height": 80,
                 "format": "1bit", "rotation": 0}]}),
            _ => json!({}),
        })
    }
}

impl Mock {
    fn wait(&self, what: &str, f: impl Fn(&str, &Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some((_, p)) = self.calls.lock().unwrap().iter().find(|(m, p)| f(m, p)) {
                return p.clone();
            }
            assert!(
                Instant::now() < until,
                "never saw {what}: {:?}",
                self.calls.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
}

#[test]
fn muted_is_red_and_held_and_a_key_unmutes() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_mic-mute"));
    let dir = std::env::temp_dir().join(format!("mic-mute-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let id = "test.mic-mute";
    let spec = PluginSpec {
        id: id.into(),
        dir: dir.clone(),
        run: vec![exe.file_name().unwrap().to_string_lossy().into()],
        sandbox: cfg!(windows),
    };
    let mock = Arc::new(Mock::default());
    let facts = Facts::default();
    facts.set("mic.state", DeviceValue::Text("muted".into()));
    let (m, f) = (mock.clone(), facts.clone());
    let perms = [
        "audio.inputs.read",
        "audio.inputs.mute",
        "devices.read",
        "lighting.write",
        "screens.write",
    ]
    .map(|p| Permission::try_from(p.to_string()).unwrap())
    .to_vec();
    let context = move || Context {
        facts: f.clone(),
        devices: m.clone(),
        client: Client::Plugin(perms.clone()),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});

    // Muted at start: a red, high-priority lease and the muted screen, held until replaced.
    let lease = mock.wait("a red lease", |m, p| {
        m == "lighting.lease" && p["color"]["hue"] == 0 && p["color"]["saturation"] == 100
    });
    assert_eq!(lease["priority"], "high");
    let panel = mock.wait("the muted screen", |m, _| m == "screens.panel_override");
    assert_eq!(
        (
            panel["width"].clone(),
            panel["height"].clone(),
            panel["screen"].clone(),
            panel["ttl_ms"].clone()
        ),
        (json!(128), json!(32), json!(4), json!(0)),
        "held, not timed, at the info screen's size (#560)"
    );

    // The bound key toggles: the plugin asks Foxglove to unmute.
    let until = Instant::now() + Duration::from_secs(30);
    while !foxglove_plugin_host::notify(id, "action.invoked", json!({"action": "toggle"})) {
        assert!(Instant::now() < until, "the plugin never registered");
        std::thread::sleep(Duration::from_millis(20));
    }
    let asked = mock.wait("an unmute", |m, _| m == "audio.inputs.set_mute");
    assert_eq!(asked, json!({"muted": false}));

    // Foxglove reports it unmuted: the red goes, a brief white flash, LIVE for 3 s.
    mock.clear();
    facts.set("mic.state", DeviceValue::Text("unmuted".into()));
    mock.wait("the red given back", |m, _| m == "lighting.release");
    mock.wait("a white flash", |m, p| {
        m == "lighting.lease" && p["color"]["saturation"] == 0
    });
    let live = mock.wait("the LIVE screen", |m, _| m == "screens.panel_override");
    assert_eq!(live["ttl_ms"], json!(3000));
    // Both leases are given back: the red, then the flash.
    let until = Instant::now() + Duration::from_secs(30);
    while mock
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == "lighting.release")
        .count()
        < 2
    {
        assert!(
            Instant::now() < until,
            "the flash wasn't given back: {:?}",
            mock.calls.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Muted again, then the microphone goes away: the held screen is given back.
    mock.clear();
    facts.set("mic.state", DeviceValue::Text("muted".into()));
    mock.wait("the muted screen again", |m, p| {
        m == "screens.panel_override" && p["ttl_ms"] == 0
    });
    facts.set("mic.state", DeviceValue::Text("unavailable".into()));
    mock.wait("the panel given back", |m, _| m == "screens.panel_release");

    drop(runner);
    #[cfg(windows)]
    foxglove_plugin_host::delete_container(id);
    let _ = std::fs::remove_dir_all(dir);
}
