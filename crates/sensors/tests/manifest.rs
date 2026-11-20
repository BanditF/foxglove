//! The shipped manifests: what the module may do, and that it can't write outside its namespace.

use foxglove_sensors::{write_permission, GuardedSink, Sink, ValueOut, NAMESPACE};
use hideout_contract::legacy::{self, Provides};
use hideout_contract::module::{Context, Module};
use hideout_contract::Version;

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A13: provides facts, asks for `facts.write:sensor.*` and nothing else.
#[test]
fn permission_is_facts_write_sensor_only() {
    let m = Module::parse(&read("module.toml"), &Context::default()).unwrap();
    let perms: Vec<String> = m.permissions.iter().map(|p| p.to_string()).collect();
    assert_eq!(perms, [write_permission()]);
    assert_eq!(m.provides, [format!("facts:{NAMESPACE}.*")]);
    for p in &perms {
        assert!(
            !p.starts_with("devices") && !p.contains("keys") && !p.starts_with("device."),
            "{p}"
        );
    }

    let legacy =
        Module::parse_legacy(&read("plugin.toml"), Version::parse("1.2").unwrap()).unwrap();
    let legacy_perms: Vec<String> = legacy.permissions.iter().map(|p| p.to_string()).collect();
    assert_eq!(legacy_perms, perms, "plugin.toml and module.toml agree");
    let plugin =
        legacy::Manifest::parse(&read("plugin.toml"), Version::parse("1.2").unwrap()).unwrap();
    assert_eq!(plugin.provides, vec![Provides::Facts]);
    assert_eq!(legacy.id, m.id);
}

/// A28: both manifests pass the contract's checker, and the sandbox-friendly shape holds
/// (no network, files, devices or UI).
#[test]
fn manifests_are_valid() {
    let m = Module::parse(&read("module.toml"), &Context::default()).unwrap();
    assert_eq!(m.id, "org.foxglove.sensors");
    assert!(m.panels.is_empty() && m.actions.is_empty() && m.secrets.is_empty());
    assert_eq!(m.run.as_deref(), Some(&["sensors.exe".to_string()][..]));
    assert!(Module::parse_legacy(&read("plugin.toml"), Version::parse("1.2").unwrap()).is_ok());
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
