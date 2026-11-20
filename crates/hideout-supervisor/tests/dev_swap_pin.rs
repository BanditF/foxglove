// SPDX-License-Identifier: MIT
//! A dev-folder app's exe swapped between the pin and the spawn (the pin lets it be renamed) is
//! not started: the name lock sees another file at the name and refuses the launch. Own test
//! binary because the launch hook is process-wide. `ping.exe` stands in for the app.
#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::set_launch_hook;

const NOW: &str = "2026-11-20T12:00:00Z";
const ID: &str = "foxglove";

fn hub(home: &Path) -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

/// The app at `<base>\out\foxglove.exe`; enrolled in a dev folder when `dev`.
fn fixture(tag: &str, dev: bool) -> (PathBuf, PathBuf) {
    // Not %TEMP%: a dev folder must not be writable by other accounts.
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hideout-devren-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let out = base.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let exe = out.join("foxglove.exe");
    std::fs::copy(Path::new(&windir).join("System32").join("PING.EXE"), &exe).unwrap();
    let folder = base.join("manifest");
    std::fs::create_dir_all(&folder).unwrap();
    let run = exe.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}', '-n', '60', '127.0.0.1']\n\
             api = {{ foxglove = \"1.0\" }}\npermissions = [\"hub.notify\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    if dev {
        reg.add_dev_folder(&out.display().to_string()).unwrap();
        let list = reg.permission_list(ID).unwrap();
        let chosen = vec!["hub.notify".to_string()];
        reg.approve_listed(ID, list.code(), &chosen, &[], NOW, "dialog", false)
            .unwrap();
    }
    reg.set_enabled(ID, true).unwrap();
    (home, exe)
}

#[test]
fn a_dev_app_swapped_between_the_pin_and_the_spawn_is_not_started() {
    let (home, exe) = fixture("swap", true);
    std::env::set_var("HIDEOUT_DEV_MODE", "1");
    let swap = exe.clone();
    set_launch_hook(Some(Box::new(move |stage| {
        if stage == "pinned" {
            // The dev pin allows this: the hashed file moves aside, other bytes take its name.
            std::fs::rename(&swap, swap.with_extension("aside")).unwrap();
            let mut bytes = std::fs::read(swap.with_extension("aside")).unwrap();
            bytes.extend_from_slice(b"swapped after the pin");
            std::fs::write(&swap, bytes).unwrap();
        }
    })));
    let sup = common::start(&home, hub(&home));
    let pid = sup.pid(ID);
    set_launch_hook(None);
    assert_eq!(pid, None, "the swapped file must not start");
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
