//! The Kit runner against a real child process (the den-test-kit binary).

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use den::runner::{Config, KitSpec, KitState, Runner};
use den::{Client, Context, Limits, NoDevices};

fn spec(mode: &str) -> KitSpec {
    spec_in(mode, false)
}

fn spec_in(mode: &str, sandbox: bool) -> KitSpec {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_den-test-kit"));
    let dir = std::env::temp_dir().join(format!("den-runner-{mode}-{sandbox}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let local = dir.join(exe.file_name().unwrap());
    std::fs::copy(&exe, &local).unwrap();
    KitSpec { id: format!("test.{mode}"), dir, run: vec![local.file_name().unwrap().to_string_lossy().into(), mode.into()], sandbox }
}

fn context() -> Context {
    Context {
        facts: palm::facts::Facts::default(),
        devices: Arc::new(NoDevices),
        client: Client::Kit(vec![]),
        app_version: "test".into(),
        limits: Limits::default(),
    }
}

fn fast() -> Config {
    Config {
        ping_timeout: Duration::from_millis(800),
        backoff_start: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        max_crashes: 3,
        crash_window: Duration::from_secs(60),
        ..Config::default()
    }
}

fn states(mode: &str, run_for: Duration) -> Vec<KitState> {
    let (tx, rx) = mpsc::channel();
    let runner = Runner::start(spec(mode), fast(), context, move |s| {
        let _ = tx.send(s);
    });
    std::thread::sleep(run_for);
    drop(runner);
    std::thread::sleep(Duration::from_millis(300));
    rx.try_iter().collect()
}

#[test]
fn a_healthy_kit_keeps_running_until_stopped() {
    let s = states("ok", Duration::from_millis(2000));
    assert_eq!(s.first(), Some(&KitState::Running), "{s:?}");
    assert!(!s.iter().any(|s| matches!(s, KitState::Restarting { .. })), "pings keep it alive: {s:?}");
    assert_eq!(s.last(), Some(&KitState::Stopped), "{s:?}");
}

#[test]
fn a_crashing_kit_backs_off_then_is_disabled() {
    let s = states("crash", Duration::from_millis(2000));
    let restarts = s.iter().filter(|s| matches!(s, KitState::Restarting { .. })).count();
    assert_eq!(restarts, 2, "{s:?}");
    assert!(matches!(s.iter().rev().find(|s| matches!(s, KitState::Disabled(_))), Some(KitState::Disabled(r)) if r.contains("exited (3)")), "{s:?}");
}

#[test]
fn a_silent_kit_is_restarted() {
    let s = states("hang", Duration::from_millis(2500));
    assert!(
        s.iter().any(|s| matches!(s, KitState::Restarting { reason, .. } if reason.contains("no sign of life"))),
        "{s:?}"
    );
}

/// In the AppContainer the Kit still talks to the host, but can't read the user's files.
#[cfg(windows)]
#[test]
fn a_sandboxed_kit_runs_but_cannot_read_user_files() {
    let (tx, rx) = mpsc::channel();
    let runner = Runner::start(spec_in("ok", true), fast(), context, move |s| {
        let _ = tx.send(s);
    });
    std::thread::sleep(Duration::from_millis(2000));
    drop(runner);
    std::thread::sleep(Duration::from_millis(300));
    let s: Vec<KitState> = rx.try_iter().collect();
    assert_eq!(s.first(), Some(&KitState::Running), "{s:?}");
    assert!(!s.iter().any(|s| matches!(s, KitState::Restarting { .. })), "it keeps pinging from the sandbox: {s:?}");

    // `probe <file>` exits 7 if it can read the file, 0 if it can't.
    let (tx, rx) = mpsc::channel();
    let mut probe = spec_in("probe", true);
    let secret = std::env::temp_dir().join(format!("den-secret-{}.txt", std::process::id()));
    std::fs::write(&secret, "secret").unwrap();
    probe.run.push(secret.to_string_lossy().into());
    let runner = Runner::start(probe, fast(), context, move |s| {
        let _ = tx.send(s);
    });
    std::thread::sleep(Duration::from_millis(800));
    drop(runner);
    let s: Vec<KitState> = rx.try_iter().collect();
    assert!(
        s.iter().any(|s| matches!(s, KitState::Restarting { reason, .. } if reason.contains("exited (0)"))),
        "the sandbox denied the read: {s:?}"
    );

    // The same probe outside the sandbox can read it, so the denial is the sandbox's doing.
    let (tx, rx) = mpsc::channel();
    let mut open = spec_in("probe", false);
    open.run.push(secret.to_string_lossy().into());
    let runner = Runner::start(open, fast(), context, move |s| {
        let _ = tx.send(s);
    });
    std::thread::sleep(Duration::from_millis(800));
    drop(runner);
    let s: Vec<KitState> = rx.try_iter().collect();
    let _ = std::fs::remove_file(secret);
    assert!(
        s.iter().any(|s| matches!(s, KitState::Restarting { reason, .. } if reason.contains("exited (7)"))),
        "unsandboxed, the read works: {s:?}"
    );
    den::delete_container("test.ok");
    den::delete_container("test.probe");
}
