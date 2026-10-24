//! A key's action reaches the plugin (#212): the runner starts the focus timer as Foxglove does,
//! `notify` sends `action.invoked`, and the timer publishes that it paused.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use foxglove_devices::facts::{Facts, Value};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Limits, NoDevices};

fn wait_for(facts: &Facts, key: &str, want: &Value) {
    let until = Instant::now() + Duration::from_secs(30);
    while facts.get(key).as_ref() != Some(want) {
        assert!(Instant::now() < until, "{key} never became {want:?}: {:?}", facts.get(key));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_bound_key_pauses_the_timer() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_focus-timer"));
    let dir = std::env::temp_dir().join(format!("focus-timer-actions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let id = "test.focus-actions";
    let spec = PluginSpec {
        id: id.into(),
        dir: dir.clone(),
        run: vec![exe.file_name().unwrap().to_string_lossy().into(), "--focus-seconds".into(), "600".into(), "--no-lights".into(), "--no-panel".into()],
        sandbox: cfg!(windows),
    };
    let facts = Facts::default();
    let shared = facts.clone();
    let context = move || Context {
        facts: shared.clone(),
        devices: Arc::new(NoDevices),
        client: Client::Plugin(vec![Permission::try_from("facts.write:focus.*".to_string()).unwrap()]),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});
    wait_for(&facts, "focus.paused", &Value::Bool(false));
    // Until the plugin's connection is registered, notify reports it isn't running.
    let until = Instant::now() + Duration::from_secs(30);
    while !foxglove_plugin_host::notify(id, "action.invoked", serde_json::json!({"action": "pause"})) {
        assert!(Instant::now() < until, "the plugin never registered");
        std::thread::sleep(Duration::from_millis(20));
    }
    wait_for(&facts, "focus.paused", &Value::Bool(true));
    // The pause is kept in its data folder (writable in the sandbox), for a restart.
    let state = dir.join("data").join("state.json");
    let until = Instant::now() + Duration::from_secs(30);
    while !std::fs::read_to_string(&state).is_ok_and(|t| t.contains("\"paused_left\":") && !t.contains("\"paused_left\":null")) {
        assert!(Instant::now() < until, "no paused state in {}", state.display());
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(runner);
    #[cfg(windows)]
    foxglove_plugin_host::delete_container(id);
    let _ = std::fs::remove_dir_all(dir);
}
