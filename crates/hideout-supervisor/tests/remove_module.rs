// SPDX-License-Identifier: MIT
//! "Remove from Hideout" through the real driver (product slice 6): a module that runs is
//! stopped first and then removed from the registry (its folder under `modules\` goes, nothing
//! else); one that doesn't run is removed at once. Windowless: a copy of `ping.exe` installed as
//! a developer-mode module; every process the test starts is ended by its handle.
#![cfg(windows)]
// It turns on lab mode, developer mode or unsigned sideload from the environment, which only a
// build with `lab-switches` reads: the installed build (no feature) has no such test.
#![cfg(feature = "lab-switches")]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hideout_registry::Registry;
use hideout_router::secrets::{target, MemorySecrets, SecretStore};
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::win::Process;

const NOW: &str = "2026-11-19T00:00:00Z";

fn home_with(id: &str, enabled: bool) -> PathBuf {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-rm-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("src");
    std::fs::create_dir_all(&folder).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::fs::copy(
        Path::new(&windir).join("System32").join("PING.EXE"),
        folder.join("ping.exe"),
    )
    .unwrap();
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"Gone\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"ping.exe\", \"-n\", \"60\", \"127.0.0.1\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&folder, NOW).unwrap();
    reg.set_enabled(id, enabled).unwrap();
    home
}

/// Secrets in memory the test can look at after the router took the store. Never the user's
/// own credential store.
#[derive(Clone, Default)]
struct Saved(Arc<Mutex<MemorySecrets>>);

impl SecretStore for Saved {
    fn get(&self, target: &str) -> Option<String> {
        self.0.lock().unwrap().get(target)
    }
    fn set(&mut self, target: &str, value: Option<&str>) -> Result<(), String> {
        self.0.lock().unwrap().set(target, value)
    }
    fn remove_all(&mut self, module: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove_all(module)
    }
}

impl Saved {
    fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.0.lock().unwrap().0.keys().cloned().collect();
        v.sort();
        v
    }
}

fn hub_with(saved: &Saved) -> Hub {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.set_secret_store(Box::new(saved.clone()));
    let (events, _rx) = mpsc::channel();
    Hub::new(router, events)
}

fn hub() -> Hub {
    hub_with(&Saved::default())
}

fn installed(home: &Path, id: &str) -> bool {
    Registry::open(home, true)
        .unwrap()
        .inventory
        .modules
        .contains_key(id)
}

#[test]
fn a_running_module_is_stopped_first_and_then_removed() {
    let id = format!("example.lab.rm-run-{}", std::process::id());
    let home = home_with(&id, true);
    let sup = Supervisor::start(&home, hub());
    let pid = sup.pid(&id).expect("the module was not started");
    let proc = Process::open(pid).unwrap();
    assert!(installed(&home, &id));
    let (tx, done) = mpsc::channel();
    sup.set_remove_done(std::sync::Arc::new(move |id: &str, r| {
        let _ = tx.send((id.to_string(), r));
    }));

    sup.remove(&id).unwrap();
    // Stopped (the handle's wait is the event), then removed when its instance ends.
    let _ = proc.wait();
    assert!(!proc.running(), "the module was not stopped");
    // The driver says when it is gone (an event, no polling).
    let (who, result) = done.recv_timeout(Duration::from_secs(30)).expect("no end");
    assert_eq!((who.as_str(), result), (id.as_str(), Ok(())));
    assert!(!installed(&home, &id), "still in the inventory");
    assert!(!home.join("modules").join(&id).exists());
    assert!(!sup.knows(&id));
    // It never comes back: nothing restarts it.
    assert_eq!(sup.pid(&id), None);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// What must hold: a removed app's stored secrets never reach a later app with the same id.
/// Remove deletes every secret stored for it, also under a name it no longer declares, and
/// leaves another app's alone.
#[test]
fn removing_a_module_deletes_the_secrets_stored_for_it_and_no_others() {
    let id = format!("example.lab.rm-secrets-{}", std::process::id());
    let other = format!("{id}x");
    let home = home_with(&id, false);
    let mut saved = Saved::default();
    for t in [
        target(&id, "token"),
        target(&id, "a name from an older version"),
        target(&other, "token"),
    ] {
        saved.set(&t, Some("v")).unwrap();
    }
    let sup = Supervisor::start(&home, hub_with(&saved));
    sup.remove(&id).unwrap();
    assert!(!installed(&home, &id));
    assert_eq!(saved.names(), [target(&other, "token")]);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// What must hold: once the app's record is gone its stored secrets are deleted, also when the
/// rest of Remove failed (here a file of its copy is in use).
#[test]
fn a_remove_that_fails_after_the_record_went_still_deletes_the_secrets() {
    use std::os::windows::fs::OpenOptionsExt;
    let id = format!("example.lab.rm-late-{}", std::process::id());
    let home = home_with(&id, false);
    let mut saved = Saved::default();
    saved.set(&target(&id, "token"), Some("v")).unwrap();
    let copy = home
        .join("modules")
        .join(&id)
        .join("0.1.0")
        .join("ping.exe");
    // Held with no sharing, so the copy can't be deleted while this is open.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&copy)
        .expect("the installed copy");
    let sup = Supervisor::start(&home, hub_with(&saved));
    assert!(sup.remove(&id).is_err(), "the copy was in use");
    assert!(!installed(&home, &id));
    assert!(saved.names().is_empty());
    drop(held);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_module_that_is_off_is_removed_at_once_and_a_second_remove_finds_nothing() {
    let id = format!("example.lab.rm-off-{}", std::process::id());
    let home = home_with(&id, false);
    let sup = Supervisor::start(&home, hub());
    assert_eq!(sup.pid(&id), None);
    sup.remove(&id).unwrap();
    assert!(!installed(&home, &id));
    assert!(!home.join("modules").join(&id).exists());
    assert!(sup.remove(&id).is_err());
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
