//! A key's action reaches the plugin (#212): the runner starts the focus timer as Foxglove does,
//! `notify` sends `action.invoked`, and the timer publishes that it paused.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_devices::facts::{Change, Facts, Value};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Limits, NoDevices};

/// Waits on the fact's changes, not a polling deadline: a loaded machine (the whole gate at
/// once) can take a while to start a sandboxed process, so the limit is generous and the
/// runner's states are in the message.
fn wait_for(facts: &Facts, changes: &Receiver<Change>, key: &str, want: &Value, states: &Mutex<Vec<String>>) {
    let until = Instant::now() + Duration::from_secs(120);
    while facts.get(key).as_ref() != Some(want) {
        let left = until.saturating_duration_since(Instant::now());
        assert!(
            changes.recv_timeout(left).is_ok(),
            "{key} never became {want:?}: {:?}; runner: {:?}",
            facts.get(key),
            states.lock().unwrap()
        );
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
    let changes = facts.subscribe("focus.paused");
    let shared = facts.clone();
    let context = move || Context {
        facts: shared.clone(),
        devices: Arc::new(NoDevices),
        client: Client::Plugin(vec![Permission::try_from("facts.write:focus.*".to_string()).unwrap()]),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let states = Arc::new(Mutex::new(Vec::new()));
    let seen = states.clone();
    let runner = Runner::start(spec, Config::default(), context, move |s| seen.lock().unwrap().push(format!("{s:?}")));
    wait_for(&facts, &changes, "focus.paused", &Value::Bool(false), &states);
    // Until the plugin's connection is registered, notify reports it isn't running.
    let until = Instant::now() + Duration::from_secs(30);
    while !foxglove_plugin_host::notify(id, "action.invoked", serde_json::json!({"action": "pause"})) {
        assert!(Instant::now() < until, "the plugin never registered");
        std::thread::sleep(Duration::from_millis(20));
    }
    wait_for(&facts, &changes, "focus.paused", &Value::Bool(true), &states);
    // The pause is kept in its data folder (writable in the sandbox), for a restart: it's
    // saved before the fact is published.
    let state = dir.join("data").join("state.json");
    let text = std::fs::read_to_string(&state).unwrap_or_default();
    assert!(text.contains("\"paused_left\":") && !text.contains("\"paused_left\":null"), "no paused state in {}: {text}", state.display());
    drop(runner);
    #[cfg(windows)]
    foxglove_plugin_host::delete_container(id);
    let _ = std::fs::remove_dir_all(dir);
}
