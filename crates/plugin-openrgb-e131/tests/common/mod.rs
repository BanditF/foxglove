//! What the end-to-end tests share: a mock keyboard that records the lease calls, and the plugin
//! started by the plugin host's runner as Foxglove starts it, on a UDP port of the test's own.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::Error;
use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Devices, Limits};
use serde_json::{json, Value};

/// One keyboard with lights: it records the lease calls the plugin makes.
#[derive(Default)]
pub struct Keyboard {
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
    pub fn leases(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().iter().filter(|(m, _)| m.starts_with("lighting.")).cloned().collect()
    }

    pub fn wait(&self, what: &str, f: impl Fn(&[(String, Value)]) -> bool) -> Vec<(String, Value)> {
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
pub fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// The plugin running as Foxglove runs it, listening on `port`; cleaned up on drop.
pub struct Plugin {
    runner: Option<Runner>,
    id: String,
    dir: PathBuf,
}

impl Drop for Plugin {
    fn drop(&mut self) {
        drop(self.runner.take());
        #[cfg(windows)]
        foxglove_plugin_host::delete_container(&self.id);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Starts the plugin on `port` with the keyboard. In the sandbox (on Windows), as Foxglove runs
/// it: the plugin has no network of its own, and the frames reach it through the host. Its own
/// folder, so the build folder's permissions stay as they are.
pub fn start(id: &str, port: u16, keyboard: Arc<Keyboard>) -> Plugin {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_openrgb-e131"));
    let dir = std::env::temp_dir().join(format!("{id}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let spec = PluginSpec {
        id: id.into(),
        dir: dir.clone(),
        run: vec![exe.file_name().unwrap().to_string_lossy().into(), "--port".into(), port.to_string()],
        sandbox: cfg!(windows),
    };
    let context = move || Context {
        facts: foxglove_devices::facts::Facts::default(),
        devices: keyboard.clone(),
        client: Client::Plugin(vec![Permission::DevicesRead, Permission::LightingWrite, Permission::NetListenUdp(port)]),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});
    Plugin { runner: Some(runner), id: id.into(), dir }
}
