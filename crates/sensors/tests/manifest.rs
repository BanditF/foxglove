//! The shipped manifests: what the module may do, and that it can't write outside its namespace.

use foxglove_sensors::{write_permission, GuardedSink, Sink, ValueOut, NAMESPACE};
use hideout_contract::module::{Context, Module};

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A13: provides facts, writes only `sensor.*`, and reads only the one fact that slows it in a
/// game (`keyboard.game_mode`). It is a native Hideout module: no plugin.toml.
#[test]
fn permissions_are_the_sensor_namespace_and_the_game_flag() {
    let m = Module::parse(&read("module.toml"), &Context::default()).unwrap();
    let perms: Vec<String> = m.permissions.iter().map(|p| p.to_string()).collect();
    assert_eq!(
        perms,
        [
            write_permission(),
            "facts.read:keyboard.game_mode".to_string()
        ]
    );
    assert_eq!(m.provides, [format!("facts:{NAMESPACE}.*")]);
    for p in &perms {
        assert!(
            !p.starts_with("devices") && !p.contains("keys") && !p.starts_with("device."),
            "{p}"
        );
    }
    assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("plugin.toml")
        .exists());
}

/// A28: the manifest passes the contract's checker, asks for the hub that sends `facts.demand`,
/// and the sandbox-friendly shape holds (no network, files, devices or UI).
#[test]
fn manifest_is_valid() {
    let m = Module::parse(&read("module.toml"), &Context::default()).unwrap();
    assert_eq!(m.id, "org.foxglove.sensors");
    assert!(m.panels.is_empty() && m.actions.is_empty() && m.secrets.is_empty());
    assert_eq!(m.run.as_deref(), Some(&["sensors.exe".to_string()][..]));
    assert_eq!(m.hub, "1.5");
}

#[derive(Default)]
struct Recorder(Vec<String>);

impl Sink for Recorder {
    fn set(&mut self, key: &str, _: ValueOut) {
        self.0.push(format!("set {key}"));
    }
    fn withdraw(&mut self, key: &str) {
        self.0.push(format!("withdraw {key}"));
    }
}

/// A14: a key outside the namespace never reaches the facts store.
#[test]
fn a_write_outside_the_namespace_is_refused() {
    let mut sink = GuardedSink::new(Recorder::default());
    sink.set("sensor.gpu.temp_c", ValueOut::Int(60));
    sink.set("focus.state", ValueOut::Text("focus"));
    sink.set("sensors.gpu.temp_c", ValueOut::Int(1));
    sink.set("sensor", ValueOut::Int(1));
    sink.withdraw("keyboard.game_mode");
    sink.withdraw("sensor.gpu.temp_c");
    assert_eq!(
        sink.inner().0,
        ["set sensor.gpu.temp_c", "withdraw sensor.gpu.temp_c"]
    );
    assert_eq!(sink.refused(), 4);
}
