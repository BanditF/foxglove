// SPDX-License-Identifier: MIT
//! #1096: a lab module whose manifest asks for a newer hub (with a key this hub doesn't know) is
//! not dropped at load. The lab hub lists it from its head, the supervisor reads it as
//! `needs-hub` at hub start (§7.8), and it is never launched. Windowless; nothing runs.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;

const ID: &str = "example.lab.kiln";

fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-lab-needs-hub-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(LAB_MODULES_DIR)).unwrap();
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

#[test]
fn a_newer_hub_lab_module_reads_needs_hub_and_never_runs() {
    let home = lab_home();
    let dir = home.join(LAB_MODULES_DIR).join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    // A stand-in exe that would end at once if anything started it.
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::fs::copy(
        PathBuf::from(&windir).join("System32").join("cmd.exe"),
        dir.join("cmd.exe"),
    )
    .unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Kiln\"\nversion = \"2.0.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.9\"\nvisibility = \"private\"\n\
             run = [\"cmd.exe\", \"/d\", \"/c\", \"exit 0\"]\nglaze_from_hub_1_9 = true\n"
        ),
    )
    .unwrap();

    let reg = Registry::open_from_env(&home).unwrap();
    let report = reg.lab_report.clone();
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&reg);
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    let sup = Supervisor::start(&home, hub.clone());

    let state = hub.read(|r| {
        r.fact(&format!("module.{ID}.state"))
            .and_then(|v| v.as_str().map(String::from))
    });
    let pid = sup.pid(ID);
    let recorded = Instances::load(&home).modules.contains_key(ID);
    let listed = reg.inventory.modules.contains_key(ID);
    let name = reg.module_name(ID);
    let lab_state = sup.lab_state(ID);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());

    assert!(
        report.iter().any(|l| l.contains("needs hub 1.9")),
        "{report:?}"
    );
    assert!(listed, "not in the inventory: {report:?}");
    assert_eq!(name, "Kiln");
    assert_eq!(state.as_deref(), Some("needs-hub"));
    assert_eq!(lab_state, "needs-hub");
    assert!(
        pid.is_none() && !recorded,
        "a module needing a newer hub was started"
    );
}
