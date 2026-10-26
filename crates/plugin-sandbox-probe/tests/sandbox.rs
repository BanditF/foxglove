//! Which Windows APIs work for a plugin in the AppContainer (#215): each probe runs under the
//! plugin host's runner, sandboxed and not, and the exit code says what happened.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use foxglove_plugin_host::runner::{Config, PluginSpec, PluginState, Runner};
use foxglove_plugin_host::{Client, Context, Limits, NoDevices};

/// Runs `sandbox-probe <mode>` and returns its exit code.
fn probe(mode: &str, sandbox: bool) -> i32 {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_sandbox-probe"));
    let dir = std::env::temp_dir().join(format!(
        "sandbox-probe-{mode}-{sandbox}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let id = format!("test.probe-{mode}");
    let spec = PluginSpec {
        id: id.clone(),
        dir: dir.clone(),
        run: vec![
            exe.file_name().unwrap().to_string_lossy().into(),
            mode.into(),
        ],
        sandbox,
    };
    let context = || Context {
        facts: foxglove_devices::facts::Facts::default(),
        devices: Arc::new(NoDevices),
        client: Client::Plugin(vec![]),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let (tx, rx) = mpsc::channel();
    let runner = Runner::start(spec, Config::default(), context, move |s| {
        let _ = tx.send(s);
    });
    // The probe exits at once; the runner reports how.
    let until = Instant::now() + Duration::from_secs(30);
    let code = loop {
        let left = until
            .checked_duration_since(Instant::now())
            .expect("the probe never exited");
        if let Ok(PluginState::Restarting { reason, .. }) = rx.recv_timeout(left) {
            let code = reason
                .trim_start_matches("exited (")
                .split(')')
                .next()
                .unwrap_or("")
                .parse();
            break code.unwrap_or_else(|_| panic!("unexpected: {reason}"));
        }
    };
    drop(runner);
    if let Ok(log) = std::fs::read_to_string(dir.join("plugin.log")) {
        eprint!("{log}");
    }
    if sandbox {
        foxglove_plugin_host::delete_container(&id);
    }
    let _ = std::fs::remove_dir_all(dir);
    code
}

#[test]
fn media_sessions_inside_the_sandbox() {
    assert_eq!(probe("smtc", false), 0, "outside the sandbox SMTC works");
    let inside = probe("smtc", true);
    eprintln!("SMTC in the AppContainer: exit {inside}");
    assert!(inside == 0 || inside == 3, "{inside}");
}

#[test]
fn microphone_inside_the_sandbox() {
    let outside = probe("mic", false);
    if outside == 4 {
        eprintln!("no microphone here: skipped");
        return;
    }
    assert_eq!(outside, 0, "outside the sandbox Core Audio works");
    let inside = probe("mic", true);
    eprintln!("Core Audio mic in the AppContainer: exit {inside}");
    assert!(inside == 0 || inside == 3, "{inside}");
}
