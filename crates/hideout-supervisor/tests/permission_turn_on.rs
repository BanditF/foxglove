// SPDX-License-Identifier: MIT
//! `turn_on` and the permission grant (#1178), with a real windowless process (a copy of
//! `ping.exe` registered as an adopted module): refused while permissions wait, proceeds after a
//! valid grant for the list that was shown, refused again once the program file changes.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::win::Process;

const NOW: &str = "2026-11-13T00:00:00Z";

/// Ends the module the test started, by handle.
struct Reaper(Vec<Process>);
impl Drop for Reaper {
    fn drop(&mut self) {
        for p in &self.0 {
            p.kill(1);
            let _ = p.wait();
        }
    }
}

fn adopted_home(name: &str) -> (PathBuf, PathBuf, String) {
    // One id per test: the job name is per user, module and instance.
    let id = format!("example.lab.perm{name}");
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
    let sup = Supervisor::start(&home, hub.clone());
    let mut reaper = Reaper(Vec::new());

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
    reaper.0.push(Process::open(pid).unwrap());
    assert!(sup.in_module_job(pid), "the module is not in its job");

    drop(sup);
    drop(reaper);
    let _ = exe;
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn declining_everything_still_lets_the_module_start_and_a_swapped_program_asks_again() {
    let (home, exe, id) = adopted_home("swap");
    let id = id.as_str();
    let sup = Supervisor::start(&home, hub());
    let mut reaper = Reaper(Vec::new());
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
    if let Some(pid) = sup.pid(id) {
        reaper.0.push(Process::open(pid).unwrap());
    }
    sup.turn_off(id);
    drop(reaper);

    // The program file is replaced: the grant is void and turn_on asks again.
    let mut bytes = std::fs::read(&exe).unwrap();
    bytes.extend_from_slice(b"changed");
    let _ = std::fs::write(&exe, bytes);
    let e = sup.turn_on(id).unwrap_err();
    assert!(e.contains("needs your OK"), "{e}");
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn an_allow_widens_the_router_only_by_what_it_granted() {
    let (home, _exe, id) = adopted_home("widen");
    let id = id.as_str();
    let hub = hub();
    let sup = Supervisor::start(&home, hub.clone());
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
    // ping.exe is the program; the leftover is a second ping started through cmd's `start`.
    let cmd = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
    let cmd = cmd.replace(std::path::MAIN_SEPARATOR, "/");
    // A literal TOML string keeps the backslashes `start` needs in the helper's path.
    let helper = exe.display().to_string();
    let run = format!(
        "run = [\"{cmd}\", \"/c\", \"start\", \"/b\", '{helper}', \"-n\", \"25\", \"127.0.0.1\"]"
    );
    std::fs::write(folder.join("module.toml"), text.replace(old, &run)).unwrap();
    Registry::open(&home, true)
        .unwrap()
        .register_adopted(&folder, NOW)
        .unwrap();
    let sup = Supervisor::start(&home, hub());
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
