// SPDX-License-Identifier: MIT
//! S7 (F4): the program that starts is the program that was hashed. Real windowless processes
//! (copies of `ping.exe`; never the real Foxglove). The launch seam swaps files between the
//! checks, which no by-name check can see.
#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;

use hideout_registry::keyboard::{canonical_path, same_path};
use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::set_launch_hook;
use hideout_supervisor::win::{image_of_pid, Process};

const NOW: &str = "2026-11-18T10:00:00Z";
const ID: &str = "foxglove";

/// The seam is process-wide: one launch test at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn hub(home: &Path) -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(home));
    hub
}

fn ping() -> PathBuf {
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&windir).join("System32").join("PING.EXE")
}

/// An adopted `foxglove` whose `run[0]` is `run0`, allowed (bound to the bytes there now).
fn home_for(base: &Path, run0: &Path) -> PathBuf {
    let folder = base.join("manifest");
    std::fs::create_dir_all(&folder).unwrap();
    let run = run0.display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"0.1.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}', '-n', '60', '127.0.0.1']\n\
             api = {{ foxglove = \"1.0\" }}\npermissions = [\"hub.notify\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    let list = reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    reg.approve_listed(ID, list.code(), &all, &[], NOW, "dialog", false)
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    assert!(reg.binding_void(ID).is_none());
    home
}

fn base(tag: &str) -> PathBuf {
    let b = std::env::temp_dir().join(format!("hideout-pin-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&b);
    std::fs::create_dir_all(&b).unwrap();
    b
}

/// Starts the hub (which launches the enabled module) and returns the module's pid, if any;
/// the process is ended before it returns.
fn start_and_end(home: &Path) -> (Option<u32>, Option<PathBuf>) {
    let sup = common::start(home, hub(home));
    let pid = sup.pid(ID);
    let image = pid.and_then(|p| image_of_pid(p).ok());
    if let Some(p) = pid.and_then(|p| Process::open(p).ok()) {
        p.kill(1);
        let _ = p.wait();
    }
    (pid, image)
}

#[test]
fn a_file_swapped_after_the_by_name_check_is_not_started() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let b = base("swap");
    let exe = b.join("install").join("foxglove.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::copy(ping(), &exe).unwrap();
    let home = home_for(&b, &exe);
    // Between `pre_launch` (by name, passes) and the pin: other bytes in the same file.
    let swap = exe.clone();
    set_launch_hook(Some(Box::new(move |stage| {
        if stage == "before-pin" {
            let mut bytes = std::fs::read(&swap).unwrap();
            bytes.extend_from_slice(b"swapped after the check");
            std::fs::write(&swap, bytes).unwrap();
        }
    })));
    let (pid, _) = start_and_end(&home);
    set_launch_hook(None);
    assert_eq!(pid, None, "the swapped file must not start");
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn a_junction_retargeted_after_the_pin_still_starts_the_pinned_file() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let b = base("junction");
    let (a_dir, b_dir, link) = (b.join("a"), b.join("b"), b.join("link"));
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    std::fs::copy(ping(), a_dir.join("foxglove.exe")).unwrap();
    let mut other = std::fs::read(ping()).unwrap();
    other.extend_from_slice(b"the other build");
    std::fs::write(b_dir.join("foxglove.exe"), other).unwrap();
    let mklink = |target: &Path| {
        std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !mklink(&a_dir) {
        return; // the volume makes no junctions
    }
    let home = home_for(&b, &link.join("foxglove.exe"));
    let (l, bd) = (link.clone(), b_dir.clone());
    set_launch_hook(Some(Box::new(move |stage| {
        if stage == "pinned" {
            let _ = std::fs::remove_dir(&l);
            let _ = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&l)
                .arg(&bd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    })));
    let (pid, image) = start_and_end(&home);
    set_launch_hook(None);
    assert!(pid.is_some(), "the pinned file starts");
    let want = canonical_path(&a_dir.join("foxglove.exe")).unwrap();
    let got = image.expect("image of the started process");
    assert!(
        same_path(&got.display().to_string(), &want),
        "started {} not {want}",
        got.display()
    );
    let _ = std::fs::remove_dir_all(&b);
}
