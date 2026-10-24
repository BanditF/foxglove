//! The plugin runner against a real child process (the test-plugin binary).

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use foxglove_plugin_host::runner::{Config, PluginSpec, PluginState, Runner};
use foxglove_plugin_host::{Client, Context, Limits, NoDevices};

fn spec(mode: &str) -> PluginSpec {
    spec_in(mode, false)
}

fn spec_in(mode: &str, sandbox: bool) -> PluginSpec {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_test-plugin"));
    let dir = std::env::temp_dir().join(format!("plugin-host-runner-{mode}-{sandbox}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let local = dir.join(exe.file_name().unwrap());
    std::fs::copy(&exe, &local).unwrap();
    PluginSpec { id: format!("test.{mode}"), dir, run: vec![local.file_name().unwrap().to_string_lossy().into(), mode.into()], sandbox }
}

fn context() -> Context {
    Context {
        facts: foxglove_devices::facts::Facts::default(),
        devices: Arc::new(NoDevices),
        client: Client::Plugin(vec![]),
        app_version: "test".into(),
        limits: Limits::default(),
    }
}

fn fast() -> Config {
    Config {
        // The test plugin pings every 200 ms; a wide margin, so a loaded machine isn't "silent".
        ping_timeout: Duration::from_secs(3),
        backoff_start: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        max_crashes: 3,
        crash_window: Duration::from_secs(60),
        ..Config::default()
    }
}

fn states(mode: &str, run_for: Duration) -> Vec<PluginState> {
    let (tx, rx) = mpsc::channel();
    let runner = Runner::start(spec(mode), fast(), context, move |s| {
        let _ = tx.send(s);
    });
    std::thread::sleep(run_for);
    drop(runner);
    std::thread::sleep(Duration::from_millis(300));
    rx.try_iter().collect()
}

/// The states until one matches `done` (or `within` runs out), then stopped.
fn states_until(mode: &str, within: Duration, done: impl Fn(&PluginState) -> bool) -> Vec<PluginState> {
    run_until(spec(mode), within, done)
}

/// Runs `spec` until a state matches `done` (or `within` runs out): waits on the outcome, not
/// on a time.
fn run_until(spec: PluginSpec, within: Duration, done: impl Fn(&PluginState) -> bool) -> Vec<PluginState> {
    let (tx, rx) = mpsc::channel();
    let runner = Runner::start(spec, fast(), context, move |s| {
        let _ = tx.send(s);
    });
    let until = std::time::Instant::now() + within;
    let mut seen = Vec::new();
    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(s) => {
                let stop = done(&s);
                seen.push(s);
                if stop {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    drop(runner);
    std::thread::sleep(Duration::from_millis(300));
    seen.extend(rx.try_iter());
    seen
}

#[test]
fn a_healthy_plugin_keeps_running_until_stopped() {
    let s = states("ok", Duration::from_millis(2000));
    assert_eq!(s.first(), Some(&PluginState::Running), "{s:?}");
    assert!(!s.iter().any(|s| matches!(s, PluginState::Restarting { .. })), "pings keep it alive: {s:?}");
    assert_eq!(s.last(), Some(&PluginState::Stopped), "{s:?}");
}

#[test]
fn a_crashing_plugin_backs_off_then_is_disabled() {
    // Waits for the outcome rather than a fixed time, so a slow machine doesn't cut it short.
    let s = states_until("crash", Duration::from_secs(15), |s| matches!(s, PluginState::Disabled(_)));
    let restarts = s.iter().filter(|s| matches!(s, PluginState::Restarting { .. })).count();
    assert_eq!(restarts, 2, "{s:?}");
    assert!(matches!(s.iter().rev().find(|s| matches!(s, PluginState::Disabled(_))), Some(PluginState::Disabled(r)) if r.contains("exited (3)")), "{s:?}");
}

#[test]
fn a_silent_plugin_is_restarted() {
    let silent = |s: &PluginState| matches!(s, PluginState::Restarting { reason, .. } if reason.contains("no sign of life"));
    let s = states_until("hang", Duration::from_secs(30), silent);
    assert!(
        s.iter().any(|s| matches!(s, PluginState::Restarting { reason, .. } if reason.contains("no sign of life"))),
        "{s:?}"
    );
}

/// In the AppContainer the plugin still talks to the host, but can't read the user's files.
#[cfg(windows)]
#[test]
fn a_sandboxed_plugin_runs_but_cannot_read_user_files() {
    let (tx, rx) = mpsc::channel();
    let runner = Runner::start(spec_in("ok", true), fast(), context, move |s| {
        let _ = tx.send(s);
    });
    std::thread::sleep(Duration::from_millis(2000));
    drop(runner);
    std::thread::sleep(Duration::from_millis(300));
    let s: Vec<PluginState> = rx.try_iter().collect();
    assert_eq!(s.first(), Some(&PluginState::Running), "{s:?}");
    assert!(!s.iter().any(|s| matches!(s, PluginState::Restarting { .. })), "it keeps pinging from the sandbox: {s:?}");

    // `probe <file>` exits 7 if it can read the file, 0 if it can't.
    let mut probe = spec_in("probe", true);
    let secret = std::env::temp_dir().join(format!("plugin-host-secret-{}.txt", std::process::id()));
    std::fs::write(&secret, "secret").unwrap();
    probe.run.push(secret.to_string_lossy().into());
    let exited = |s: &PluginState| matches!(s, PluginState::Restarting { reason, .. } if reason.contains("exited ("));
    let s = run_until(probe, Duration::from_secs(30), exited);
    assert!(
        s.iter().any(|s| matches!(s, PluginState::Restarting { reason, .. } if reason.contains("exited (0)"))),
        "the sandbox denied the read: {s:?}"
    );

    // The same probe outside the sandbox can read it, so the denial is the sandbox's doing.
    let mut open = spec_in("probe", false);
    open.run.push(secret.to_string_lossy().into());
    let s = run_until(open, Duration::from_secs(30), exited);
    let _ = std::fs::remove_file(secret);
    assert!(
        s.iter().any(|s| matches!(s, PluginState::Restarting { reason, .. } if reason.contains("exited (7)"))),
        "unsandboxed, the read works: {s:?}"
    );
    foxglove_plugin_host::delete_container("test.ok");
    foxglove_plugin_host::delete_container("test.probe");
}
