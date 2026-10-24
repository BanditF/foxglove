//! The plugin end to end: the plugin host's runner starts it as Foxglove does, a sender's E1.31 frame on
//! localhost takes a low-priority lease on a mock keyboard, new colours change it in place, and
//! the lease ends when the sender ends its stream or goes silent.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Devices, Limits};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::Error;
use serde_json::{json, Value};

/// One keyboard with lights: it records the lease calls the plugin makes.
#[derive(Default)]
struct Keyboard {
    calls: Mutex<Vec<(String, Value)>>,
}

impl Devices for Keyboard {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.calls.lock().unwrap().push((method.to_string(), params));
        match method {
            "devices.list" => Ok(json!({"devices": [{"id": "kb", "name": "Test board", "kind": "keyboard",
                "backend": "mock", "connected": true, "caps": ["lighting"]}]})),
            "lighting.lease" => Ok(json!({"lease": 7})),
            "lighting.update" | "lighting.release" => Ok(json!({})),
            _ => Err(Error { code: -32601, message: method.into(), data: None }),
        }
    }
}

impl Keyboard {
    /// The lease calls so far.
    fn leases(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().iter().filter(|(m, _)| m.starts_with("lighting.")).cloned().collect()
    }

    fn wait(&self, what: &str, f: impl Fn(&[(String, Value)]) -> bool) -> Vec<(String, Value)> {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let calls = self.leases();
            if f(&calls) {
                return calls;
            }
            assert!(Instant::now() < until, "never saw {what}: {calls:?}");
            std::thread::sleep(Duration::from_millis(20));
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

#[test]
fn a_stream_holds_a_low_lease_and_its_end_gives_the_lights_back() {
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
        run: vec![exe.file_name().unwrap().to_string_lossy().into(), "--port".into(), port().to_string()],
        sandbox: cfg!(windows),
    };
    let context = move || Context {
        facts: foxglove_devices::facts::Facts::default(),
        devices: devices.clone(),
        client: Client::Plugin(vec![Permission::DevicesRead, Permission::LightingWrite, Permission::NetListenUdp(port())]),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});
    // Waits are for outcomes, with generous limits; nothing depends on how fast the machine is.
    // A red frame, sent until it lands (the plugin may still be starting and binding the port).
    let until = Instant::now() + Duration::from_secs(30);
    let mut seq = 0u8;
    while keyboard.leases().is_empty() && Instant::now() < until {
        seq += 1;
        send(seq, 255, 0, 0, false);
        std::thread::sleep(Duration::from_millis(300));
    }
    let calls = keyboard.wait("a lease", |c| !c.is_empty());
    assert_eq!(calls[0], ("lighting.lease".into(), json!({"device": "kb", "color": {"hue": 0, "saturation": 100, "brightness": 100}, "effect": null, "priority": "low"})));
    // A new colour changes the same lease; the same colour again sends nothing.
    seq += 1;
    send(seq, 0, 255, 0, false);
    let calls = keyboard.wait("an update", |c| c.iter().any(|(m, _)| m == "lighting.update"));
    let update = calls.iter().find(|(m, _)| m == "lighting.update").unwrap();
    assert_eq!(update.1, json!({"lease": 7, "color": {"hue": 120, "saturation": 100, "brightness": 100}}));
    seq += 1;
    send(seq, 0, 255, 0, false);
    // The sender ends its stream: the lease is given back, once, and no new one was taken.
    send(seq + 1, 0, 0, 0, true);
    let calls = keyboard.wait("the release", |c| c.iter().any(|(m, _)| m == "lighting.release"));
    assert_eq!(calls.iter().filter(|(m, _)| m == "lighting.lease").count(), 1, "{calls:?}");
    assert_eq!(calls.iter().filter(|(m, _)| m == "lighting.update").count(), 1, "the repeat sent nothing: {calls:?}");
    assert_eq!(calls.iter().find(|(m, _)| m == "lighting.release").unwrap().1, json!({"lease": 7}));
    // A new stream that goes silent: released after E1.31's loss timeout.
    send(seq + 2, 0, 0, 255, false);
    keyboard.wait("a second lease", |c| c.iter().filter(|(m, _)| m == "lighting.lease").count() == 2);
    keyboard.wait("the release after silence", |c| c.iter().filter(|(m, _)| m == "lighting.release").count() == 2);
    drop(runner);
    #[cfg(windows)]
    foxglove_plugin_host::delete_container("test.openrgb-e131");
    let _ = std::fs::remove_dir_all(dir);
}
