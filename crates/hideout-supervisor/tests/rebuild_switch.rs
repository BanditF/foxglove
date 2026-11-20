// SPDX-License-Identifier: MIT
//! R5: Settings > Developer > "Restart apps after a rebuild", through the real driver and router
//! with no process ever started. With the switch off a rebuild that would otherwise pass stops
//! nothing, starts nothing and pins nothing. Windowless.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::{rebuild_preflight, Supervisor};
use hideout_supervisor::policy::Ev;
use hideout_supervisor::sha256_file;
use serde_json::Value;

const NOW: &str = "2026-11-19T12:00:00Z";
const ID: &str = "foxglove";

fn temp(name: &str) -> PathBuf {
    // Not %TEMP%: a dev folder must not be writable by other accounts.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("hideout-rebuild-switch")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An app at `<out>\foxglove.exe` in a listed dev folder, allowed in the window, rebuilt once.
/// Returns the home and the new build's hash.
fn fixture() -> (PathBuf, String) {
    let out = temp("out");
    let exe = out.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let exe_s = exe
        .display()
        .to_string()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let folder = temp("manifest");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = [\"{exe_s}\"]\n\
             api = {{ foxglove = \"1.0\" }}\npermissions = [\"hub.notify\"]\n"
        ),
    )
    .unwrap();
    let home = temp("home");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    std::env::set_var("HIDEOUT_DEV_MODE", "1");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.add_dev_folder(&out.display().to_string()).unwrap();
    let list = reg.permission_list(ID).unwrap();
    let chosen = vec!["hub.notify".to_string()];
    reg.approve_listed(ID, list.code(), &chosen, &[], NOW, "dialog", false)
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    std::fs::write(&exe, b"build 2").unwrap();
    let sha = sha256_file(&exe).unwrap();
    (home, sha)
}

fn hub(home: &Path) -> Hub {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(home).unwrap());
    let _ = router.watch_facts(vec![String::new()]);
    let (events, rx) = mpsc::channel();
    std::mem::forget(rx);
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

fn state(hub: &Hub) -> Value {
    hub.read(|r| r.fact(&format!("module.{ID}.state")).cloned())
        .unwrap_or(Value::Null)
}

#[test]
fn switch_off_a_passing_rebuild_stops_nothing_and_pins_nothing() {
    let (home, sha) = fixture();
    // Control: with the switch on (the default) this rebuild passes the preflight.
    {
        let reg = Registry::open_from_env(&home).unwrap();
        assert!(reg.restart_after_rebuild());
        let pin = rebuild_preflight(&reg, ID, Some("old"), &sha).expect("the fixture passes");
        assert_eq!(pin.sha256, sha);
    }
    // Switch off, saved the way Settings saves it.
    Registry::open_from_env(&home)
        .unwrap()
        .set_restart_after_rebuild(false)
        .unwrap();
    let h = hub(&home);
    let sup = Supervisor::start(&home, h.clone());
    sup.feed(Ev::Known {
        id: ID.into(),
        name: "Foxglove".into(),
        enabled: true,
        at_enable: true,
        previous: None,
    });
    sup.feed(Ev::Started {
        id: ID.into(),
        instance: 1,
    });
    sup.feed(Ev::Ready { id: ID.into() });
    assert_eq!(state(&h), Value::String("running".into()));
    sup.rebuilt(ID, &sha);
    // No Exit (still running, not stopping) and no Launch, and nothing pinned.
    assert_eq!(
        state(&h),
        Value::String("running".into()),
        "the old copy was stopped with the switch off"
    );
    assert!(
        !sup.prepared_held(ID),
        "a build was pinned with the switch off"
    );
}
