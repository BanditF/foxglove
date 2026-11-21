// SPDX-License-Identifier: MIT
//! `turn_on` and the permission grant (#1178), with a real windowless process (a copy of
//! `ping.exe` registered as an adopted module): refused while permissions wait, proceeds after a
//! valid grant for the list that was shown, refused again once the program file changes.
#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::win::Process;

const NOW: &str = "2026-11-13T00:00:00Z";

fn adopted_home(name: &str) -> (PathBuf, PathBuf, String) {
    // One id per test: the job name is per user, module and instance.
    let id = format!("example.lab.perm{name}-{}", std::process::id());
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-perm-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let install = base.join("install");
    let folder = base.join("manifest");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let exe = install.join("ping.exe");
    std::fs::copy(Path::new(&windir).join("System32").join("PING.EXE"), &exe).unwrap();
    let run = exe.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"Perm\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"arete\"\nhub = \"1.0\"\nrun = [\"{run}\", \"-n\", \"60\", \"127.0.0.1\"]\n\
             api = {{ arete = \"1.0\" }}\npermissions = [\"hub.notify\", \"ui.window\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    (home, exe, id)
}

fn hub() -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    Hub::new(router, events)
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn turn_on_waits_for_the_users_ok_and_proceeds_after_a_valid_grant() {
    let (home, exe, id) = adopted_home("ok");
    let id = id.as_str();
    let hub = hub();
    let sup = common::start(&home, hub.clone());

    // Asked, not granted: refused, nothing saved as on, nothing started.
    let e = sup.turn_on(id).unwrap_err();
    assert!(e.contains("needs your OK"), "{e}");
    assert!(!Registry::open_from_env(&home).unwrap().hub.enabled(id));
    assert!(sup.pid(id).is_none());

    // A grant for a list that is not the one shown changes nothing.
    let list = Registry::open_from_env(&home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    let wrong = "0".repeat(16);
    let e = sup
        .grant_listed(id, &wrong, &strs(&["hub.notify"]), &[], "dialog")
        .unwrap_err();
    assert_eq!(e, "permission.list_changed");
    assert!(sup.turn_on(id).is_err());

    // The user's Allow for the shown list: turn_on now proceeds and starts the program.
    let out = sup
        .grant_listed(
            id,
            list.code(),
            &strs(&["hub.notify", "ui.window"]),
            &[],
            "dialog",
        )
        .unwrap();
    assert_eq!(out.granted.len(), 2);
    sup.turn_on(id).expect("turn_on after a valid grant");
    assert!(Registry::open_from_env(&home).unwrap().hub.enabled(id));
    let state = hub.read(|r| r.fact(&format!("module.{id}.state")).cloned());
    let pid = sup
        .pid(id)
        .unwrap_or_else(|| panic!("the module was not started after the grant: {state:?}"));
    assert!(sup.in_module_job(pid), "the module is not in its job");

    drop(sup);
    let _ = exe;
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn declining_everything_still_lets_the_module_start_and_a_swapped_program_asks_again() {
    let (home, exe, id) = adopted_home("swap");
    let id = id.as_str();
    let sup = common::start(&home, hub());
    let list = Registry::open_from_env(&home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    sup.grant_listed(
        id,
        list.code(),
        &[],
        &strs(&["hub.notify", "ui.window"]),
        "dialog",
    )
    .unwrap();
    sup.turn_on(id).expect("declined items never block a start");
    // Ended by handle and waited for, so the program file is free to be replaced below.
    let running = sup.pid(id).map(|pid| Process::open(pid).unwrap());
    sup.turn_off(id);
    if let Some(p) = running {
        p.kill(1);
        let _ = p.wait();
    }

    // The program file is replaced: the grant is void and turn_on asks again.
    let mut bytes = std::fs::read(&exe).unwrap();
    bytes.extend_from_slice(b"changed");
    // The killed process may still hold its image for a moment on a busy PC (#1207): the write
    // must land, or the test would check an unchanged program.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::fs::write(&exe, &bytes).is_err() {
        assert!(
            std::time::Instant::now() < deadline,
            "the program file stayed locked"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let e = sup.turn_on(id).unwrap_err();
    assert!(e.contains("needs your OK"), "{e}");
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// UX cut 2: a rebuilt app is refused at launch because the OK is missing, which reads "Waits
/// for your OK" (not the crash wording); the user's Allow then starts it with no Start again.
/// While the OK is missing nothing runs, and a refusal for another reason is not relabelled.
#[test]
fn an_allow_starts_a_module_that_waited_for_the_ok_and_the_row_never_blames_a_crash() {
    let (home, exe, id) = adopted_home("waits");
    let id = id.as_str();
    let hub = hub();
    let sup = common::start(&home, hub.clone());
    let code = || {
        Registry::open_from_env(&home)
            .unwrap()
            .permission_list(id)
            .unwrap()
            .code()
            .to_string()
    };
    let all = strs(&["hub.notify", "ui.window"]);
    sup.grant_listed(id, &code(), &all, &[], "dialog").unwrap();
    sup.turn_on(id).unwrap();
    let running = sup.pid(id).map(|pid| Process::open(pid).unwrap());
    sup.turn_off(id);
    if let Some(p) = running {
        p.kill(1);
        let _ = p.wait();
    }
    let mut bytes = std::fs::read(&exe).unwrap();
    bytes.extend_from_slice(b"rebuilt");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::fs::write(&exe, &bytes).is_err() {
        assert!(
            std::time::Instant::now() < deadline,
            "the file stayed locked"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // The hub tries to start it (it is on): refused, not started, worded as waiting.
    {
        let mut reg = Registry::open_from_env(&home).unwrap();
        reg.set_enabled(id, true).unwrap();
    }
    sup.feed(hideout_supervisor::policy::Ev::Enable { id: id.into() });
    assert!(sup.pid(id).is_none(), "started without the OK");
    let fact = |k: &str| hub.read(|r| r.fact(&format!("module.{id}.{k}")).cloned());
    assert_eq!(fact("state"), Some(serde_json::json!("install-failed")));
    assert_eq!(fact("failed"), Some(serde_json::json!("ok")));
    assert_eq!(
        fact("reason"),
        Some(serde_json::json!(hideout_supervisor::words::WAITS_FOR_OK))
    );
    // Start again with the OK still missing: still refused, still not started.
    sup.start_again(id);
    assert!(sup.pid(id).is_none());
    assert_eq!(fact("failed"), Some(serde_json::json!("ok")));
    // The user's Allow for the new file, then Turn on: it runs, no Start again. An Allow on a
    // list shown before the old grants went back to waiting is refused once (the list changed,
    // so the dialog reopens with everything it binds); the Allow on the new list goes through.
    let list = Registry::open_from_env(&home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    let shown: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    assert_eq!(
        sup.grant_listed(id, list.code(), &shown, &[], "dialog")
            .unwrap_err(),
        "permission.list_changed"
    );
    let list = Registry::open_from_env(&home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    let shown: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    assert_eq!(shown, all);
    sup.grant_listed(id, list.code(), &shown, &[], "dialog")
        .unwrap();
    sup.turn_on(id).unwrap();
    let state = fact("state");
    let pid = sup
        .pid(id)
        .unwrap_or_else(|| panic!("not started after the Allow: {state:?}"));
    assert!(sup.in_module_job(pid));
    assert_ne!(fact("failed"), Some(serde_json::json!("ok")));
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn an_allow_widens_the_router_only_by_what_it_granted() {
    let (home, _exe, id) = adopted_home("widen");
    let id = id.as_str();
    let hub = hub();
    let sup = common::start(&home, hub.clone());
    let code = || {
        Registry::open_from_env(&home)
            .unwrap()
            .permission_list(id)
            .unwrap()
            .code()
            .to_string()
    };
    sup.grant_listed(id, &code(), &strs(&["hub.notify"]), &[], "dialog")
        .unwrap();
    assert_eq!(hub.read(|r| r.module_grants(id)), ["hub.notify"]);

    // Something running as the user writes an extra grant into inventory.json.
    hideout_registry::inventory::Inventory::update(&home.join("inventory.json"), |inv| {
        inv.modules
            .get_mut(id)
            .unwrap()
            .granted
            .push("net".to_string());
    })
    .unwrap();

    // The user allows another item: the router gets it, not the injected one.
    sup.grant_listed(id, &code(), &strs(&["ui.window"]), &[], "dialog")
        .unwrap();
    let held = hub.read(|r| r.module_grants(id));
    assert!(held.contains(&"hub.notify".to_string()), "{held:?}");
    assert!(held.contains(&"ui.window".to_string()), "{held:?}");
    assert!(!held.contains(&"net".to_string()), "widened: {held:?}");
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_job_with_a_leftover_process_stays_checkable_after_its_instance_ended() {
    let (home, exe, id) = adopted_home("retired");
    let id = id.as_str();
    // The module's program starts a helper and exits at once; the helper outlives it.
    let folder = home.parent().unwrap().join("manifest");
    let text = std::fs::read_to_string(folder.join("module.toml")).unwrap();
    let old = text.lines().find(|l| l.starts_with("run")).unwrap();
    // The program is powershell, which starts the leftover (a copy of ping) in a console of its
    // own and exits at once. `cmd /c start /b` shared cmd's console, which Windows tears down when
    // cmd exits, so the helper sometimes died before the supervisor looked (#1187).
    let ps = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let ps = format!("{ps}/System32/WindowsPowerShell/v1.0/powershell.exe").replace('\\', "/");
    let helper = exe.display().to_string();
    let script = format!("Start-Process -WindowStyle Hidden -FilePath '{helper}' -ArgumentList '-n','25','127.0.0.1'");
    let run = format!(
        "run = [\"{ps}\", \"-NoProfile\", \"-NonInteractive\", \"-Command\", \"{}\"]",
        script.replace('\\', "\\\\")
    );
    std::fs::write(folder.join("module.toml"), text.replace(old, &run)).unwrap();
    Registry::open(&home, true)
        .unwrap()
        .register_adopted(&folder, NOW)
        .unwrap();
    let sup = common::start(&home, hub());
    let list = Registry::open_from_env(&home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    sup.grant_listed(
        id,
        list.code(),
        &strs(&["hub.notify", "ui.window"]),
        &[],
        "dialog",
    )
    .unwrap();
    sup.turn_on(id).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while sup.retired_jobs() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    sup.turn_off(id);
    assert!(
        sup.retired_jobs() >= 1,
        "the ended instance's job, with its helper still in it, was dropped"
    );
    // Once the helper is gone the job is pruned on the next check.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    while std::time::Instant::now() < deadline {
        let _ = sup.in_module_job(std::process::id());
        if sup.retired_jobs() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert_eq!(sup.retired_jobs(), 0, "an empty retired job was kept");
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// A module that is not adopted (a sideload or catalog one whose files changed) is never worded
/// as waiting for the OK: the wording is for adopted apps only (the driver also checks the
/// source). This guards the outcome; the void flag is only set on the adopted path.
#[test]
fn a_changed_module_that_is_not_adopted_is_never_worded_as_waiting_for_the_ok() {
    let (home, exe, id) = adopted_home("notadopted");
    let id = id.as_str();
    let hub = hub();
    let sup = common::start(&home, hub.clone());
    let list = Registry::open_from_env(&home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    let shown: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    sup.grant_listed(id, list.code(), &shown, &[], "dialog")
        .unwrap();
    let mut bytes = std::fs::read(&exe).unwrap();
    bytes.extend_from_slice(b"tampered");
    std::fs::write(&exe, &bytes).unwrap();
    let inv = home.join("inventory.json");
    let text = std::fs::read_to_string(&inv).unwrap();
    let re: Vec<&str> = text.split("\"source\"").collect();
    assert_eq!(re.len(), 2, "one module");
    let tail = re[1].replacen("\"adopted\"", "\"sideload\"", 1);
    assert_ne!(tail, re[1]);
    std::fs::write(&inv, format!("{}\"source\"{tail}", re[0])).unwrap();
    {
        let mut reg = Registry::open_from_env(&home).unwrap();
        reg.set_enabled(id, true).unwrap();
    }
    sup.feed(hideout_supervisor::policy::Ev::Enable { id: id.into() });
    assert!(sup.pid(id).is_none(), "started a changed module");
    let fact = |k: &str| hub.read(|r| r.fact(&format!("module.{id}.{k}")).cloned());
    assert_ne!(fact("failed"), Some(serde_json::json!("ok")));
    assert_ne!(
        fact("reason"),
        Some(serde_json::json!(hideout_supervisor::words::WAITS_FOR_OK))
    );
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// An app file that is there but can't be read (another program holds it) is refused with its
/// own line, nothing starts, and the user's Allow is kept: once the file can be read, Turn on
/// starts it with no new OK.
#[test]
fn an_app_file_that_cant_be_read_is_refused_and_the_ok_is_kept() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let (home, exe, id) = adopted_home("unreadable");
    let id = id.as_str();
    let hub = hub();
    let sup = common::start(&home, hub.clone());
    let list = Registry::open_from_env(&home)
        .unwrap()
        .permission_list(id)
        .unwrap();
    let all = strs(&["hub.notify", "ui.window"]);
    sup.grant_listed(id, list.code(), &all, &[], "dialog")
        .unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&exe)
        .unwrap();
    let words = hideout_supervisor::words::CANT_READ_APP_FILE;

    // The user's Turn on: refused with its own mark (the page words it), not saved as on,
    // nothing started.
    let refused = sup.turn_on(id).unwrap_err();
    assert!(refused.starts_with("app-file-unreadable"), "{refused}");
    assert!(!Registry::open_from_env(&home).unwrap().hub.enabled(id));
    assert!(sup.pid(id).is_none());

    // The hub's own start (the app is on): refused, worded as a failed start with that line,
    // never as waiting for the OK.
    Registry::open_from_env(&home)
        .unwrap()
        .set_enabled(id, true)
        .unwrap();
    sup.feed(hideout_supervisor::policy::Ev::Enable { id: id.into() });
    assert!(
        sup.pid(id).is_none(),
        "started from a file it could not read"
    );
    let fact = |k: &str| hub.read(|r| r.fact(&format!("module.{id}.{k}")).cloned());
    assert_eq!(fact("failed"), Some(serde_json::json!("launch")));
    assert_eq!(fact("reason"), Some(serde_json::json!(words)));

    // Nothing was withdrawn.
    let reg = Registry::open_from_env(&home).unwrap();
    let m = &reg.inventory.modules[id];
    assert_eq!(m.granted, all);
    assert!(m.pending_permissions.is_empty());
    assert!(m.bound.as_ref().is_some_and(|b| !b.void));

    drop(held);
    sup.turn_on(id).expect("turn_on once the file can be read");
    assert!(sup.pid(id).is_some());

    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
