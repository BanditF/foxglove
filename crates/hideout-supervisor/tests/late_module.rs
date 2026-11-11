// SPDX-License-Identifier: MIT
//! A module that joins the inventory after the hub started (#1076; the inventory watch,
//! #1058): the supervisor learns it and starts it, as it would at a hub start. Here a lab
//! module copied into `lab-modules\` while the hub runs, folder first and its files after (as
//! a copy does): until its manifest is there the registry doesn't hold it and `learn` says
//! so; once it is, the router knows it and it runs. The module is a copy of `cmd.exe` waiting
//! a few seconds; its job is ended at the end. Windowless.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::win::Job;

const ID: &str = "example.lab.late";

fn lab_home() -> PathBuf {
    let home = std::env::temp_dir()
        .join(format!("hideout-late-{}", std::process::id()))
        .join("home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(LAB_MODULES_DIR)).unwrap();
    std::env::set_var("HIDEOUT_LAB", "1");
    std::env::set_var("HIDEOUT_CONFIG_DIR", &home);
    home
}

#[test]
fn a_module_added_after_the_start_is_learnt_and_started() {
    let home = lab_home();
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open_from_env(&home).unwrap());
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    let sup = Supervisor::start(&home, hub.clone());
    assert!(!sup.knows(ID));

    // The folder first: nothing to learn yet.
    let dir = home.join(LAB_MODULES_DIR).join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(
        sup.learn(ID).is_none(),
        "learnt a folder without a manifest"
    );
    assert!(!sup.knows(ID) && !hub.read(|r| r.knows_module(ID)));

    // Then its files.
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::fs::copy(
        Path::new(&windir).join("System32").join("cmd.exe"),
        dir.join("cmd.exe"),
    )
    .unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Late\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"cmd.exe\", \"/d\", \"/c\", \"ping -n 20 127.0.0.1 >nul\"]\n"
        ),
    )
    .unwrap();
    let learnt = sup
        .learn(ID)
        .expect("not learnt once its manifest was there");
    assert_eq!(learnt.map(|m| m.name).as_deref(), Some("Late"));
    assert!(sup.knows(ID));
    assert!(
        hub.read(|r| r.knows_module(ID)),
        "the router didn't learn it"
    );
    let pid = sup.pid(ID);
    let rec = Instances::load(&home).modules.get(ID).cloned();
    if let Some(job) = rec.as_ref().and_then(|r| Job::open(&r.job).ok()) {
        job.kill(1);
    }
    assert!(pid.is_some(), "the late module wasn't started");
    // Learnt once: a second ask leaves it alone.
    assert!(matches!(sup.learn(ID), Some(None)));
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
