// SPDX-License-Identifier: MIT
//! #1465: when "Keep my OK after updates in this folder" meets new bytes at a launch, the
//! router's in-memory grants follow the file: a held-back Sensitive item is no longer live.
//! A windowless `ping.exe` copy stands in for the app; every process is ended with the test.
#![cfg(windows)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_contract::capability::{Holder, Source};
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{ModuleEntry, Router, Settings};

const NOW: &str = "2026-11-17T12:00:00Z";
const ID: &str = "foxglove";

fn hub(home: &Path) -> Hub {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    // Loaded with the old grants, as at hub start; no inventory watch runs in this test.
    router.add_module(ModuleEntry {
        id: ID.into(),
        name: ID.into(),
        holder: Holder::ThirdParty(Source::Sideload),
        granted: vec!["hub.notify".into(), "net".into()],
        catalog_provide: None,
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: Some("0.1.0".into()),
    });
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

fn home_with(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("hideout-kfr-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let install = base.join("install");
    std::fs::create_dir_all(&install).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let exe = install.join("foxglove.exe");
    std::fs::copy(Path::new(&windir).join("System32").join("PING.EXE"), &exe).unwrap();
    let folder = base.join("manifest");
    std::fs::create_dir_all(&folder).unwrap();
    let run = exe.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"0.1.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}', '-n', '60', '127.0.0.1']\n\
             api = {{ foxglove = \"1.0\" }}\npermissions = [\"net\", \"hub.notify\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    let list = reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    reg.approve_listed(ID, list.code(), &all, &[], NOW, "dialog", true)
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    (home, exe)
}

#[test]
fn a_launch_on_new_bytes_narrows_the_router_to_the_file() {
    let (home, exe) = home_with("narrow");
    let hub = hub(&home);
    assert!(hub
        .read(|r| r.module_grants(ID))
        .contains(&"net".to_string()));
    // The app file changes; the launch runs the keep step, which holds the Sensitive item back.
    let mut bytes = std::fs::read(&exe).unwrap();
    bytes.extend_from_slice(b"\0build 2");
    std::fs::write(&exe, bytes).unwrap();
    let sup = common::start(&home, hub.clone());
    let _ = sup.pid(ID);
    let reg = Registry::open(&home, false).unwrap();
    // Only meaningful where the folder is a local, unsynced drive (the keep step carried it).
    let held = reg.inventory.modules[ID]
        .pending_permissions
        .iter()
        .any(|p| p == "net");
    assert!(held, "the Sensitive item waits for the user's OK");
    assert_eq!(hub.read(|r| r.module_grants(ID)), ["hub.notify"]);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
