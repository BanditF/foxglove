// SPDX-License-Identifier: MIT
//! "Remove from Hideout" (product slice 6): the registry side. The module's permissions and
//! keyboard grant go with its inventory entry, its own files and the hub's settings folder stay,
//! nothing outside the hub's own `modules\<id>` is deleted, and adopting it again asks again.
//! Window-free; the "install" is a fake exe in a temp folder.

use std::path::{Path, PathBuf};

use hideout_registry::keyboard::canonical_path;
use hideout_registry::{Failure, Registry};

const NOW: &str = "2026-11-19T12:00:00Z";
const ID: &str = "foxglove";
const HASH: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-remove-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(id: &str, kind: &str, exe: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"Example\"\nversion = \"1.0.0\"\nkind = \"{kind}\"\n\
         family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{exe}']\napi = {{ foxglove = \"1.0\" }}\n\
         permissions = [\"hub.notify\"]\n"
    )
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

struct Fx {
    home: PathBuf,
    install: PathBuf,
    folder: PathBuf,
    reg: Registry,
}

fn adopted(name: &str) -> Fx {
    let install = temp(&format!("{name}-install"));
    std::fs::write(install.join("hub.exe"), b"program").unwrap();
    std::fs::write(install.join("settings.json"), b"{\"a\":1}").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    let exe = install
        .join("hub.exe")
        .display()
        .to_string()
        .replace('\\', "/");
    std::fs::write(folder.join("module.toml"), manifest(ID, "adopted", &exe)).unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    Fx {
        home,
        install,
        folder,
        reg,
    }
}

fn allow_everything(reg: &mut Registry, exe: &Path) {
    let list = reg.permission_list(ID).unwrap();
    reg.approve_listed(
        ID,
        list.code(),
        &["hub.notify".to_string()],
        &[],
        NOW,
        "cli",
        false,
    )
    .unwrap();
    let path = canonical_path(exe).unwrap();
    reg.allow_keyboard(ID, &path, HASH, NOW).unwrap();
    let m = &reg.inventory.modules[ID];
    assert!(!m.granted.is_empty() && m.bound.is_some() && m.keyboard.is_some());
}

#[test]
fn remove_forgets_grants_and_keyboard_and_keeps_the_install_and_the_hub_side_folder() {
    let mut f = adopted("grants");
    allow_everything(&mut f.reg, &f.install.join("hub.exe"));
    f.reg.set_enabled(ID, true).unwrap();
    let kept = f.reg.module_dir(ID, "1.0.0").join("data");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::write(kept.join("note.txt"), b"hub-side settings").unwrap();

    f.reg.remove(ID).unwrap();

    // Gone from memory and from disk (a reopened registry): grants, binding, keyboard grant.
    let on_disk = std::fs::read_to_string(f.home.join("inventory.json")).unwrap();
    assert!(
        !on_disk.contains(ID) && !on_disk.contains(HASH),
        "{on_disk}"
    );
    let again = Registry::open(&f.home, true).unwrap();
    assert!(!again.inventory.modules.contains_key(ID));
    assert!(!again.hub.enabled(ID));
    // The module's own install and the hub-side settings stay.
    assert_eq!(names(&f.install), ["hub.exe", "settings.json"]);
    assert!(kept.join("note.txt").is_file());
    assert!(f.folder.join("module.toml").is_file());
}

#[test]
fn adopting_it_again_asks_again() {
    let mut f = adopted("readopt");
    allow_everything(&mut f.reg, &f.install.join("hub.exe"));
    f.reg.remove(ID).unwrap();

    let mut reg = Registry::open(&f.home, true).unwrap();
    reg.register_adopted(&f.folder, NOW).unwrap();
    let m = &reg.inventory.modules[ID];
    assert!(m.granted.is_empty(), "a grant came back: {:?}", m.granted);
    assert!(m.last_choice.is_empty() && m.bound.is_none() && m.keyboard.is_none());
    // The list is shown afresh: nothing allowed yet.
    let list = reg.permission_list(ID).unwrap();
    assert!(reg.inventory.modules[ID].granted.is_empty());
    assert!(!list.code().is_empty());
}

#[test]
fn a_running_module_is_not_removed_by_the_registry() {
    let mut f = adopted("running");
    let launch = f.reg.pre_launch(ID);
    // Off until enabled; enable, then mark it running as the supervisor does.
    assert!(launch.is_err());
    f.reg.set_enabled(ID, true).unwrap();
    allow_everything(&mut f.reg, &f.install.join("hub.exe"));
    let launch = f.reg.pre_launch(ID).unwrap();
    f.reg.mark_running(&launch);
    assert!(matches!(f.reg.remove(ID), Err(Failure::Running(_))));
    assert!(f.reg.inventory.modules.contains_key(ID));
    f.reg.mark_stopped(ID);
    f.reg.remove(ID).unwrap();
}

#[test]
fn a_hub_installed_module_loses_only_its_own_folder_under_modules() {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let home = temp("hubcopy-home");
    let src = temp("hubcopy-src");
    let mk = |id: &str| {
        let dir = src.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.exe"), id).unwrap();
        std::fs::write(
            dir.join("module.toml"),
            format!(
                "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
                 family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
                 run = [\"a.exe\"]\n"
            ),
        )
        .unwrap();
        dir
    };
    let (one, two) = (mk("example.rm.one"), mk("example.rm.two"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&one, NOW).unwrap();
    reg.install(&two, NOW).unwrap();
    // A neighbour of the hub's folders that must survive.
    std::fs::write(home.join("hideout.toml.keep"), b"x").unwrap();
    std::fs::create_dir_all(home.join("data").join("example.rm.one")).unwrap();
    std::fs::write(
        home.join("data").join("example.rm.one").join("s.json"),
        b"s",
    )
    .unwrap();

    reg.remove("example.rm.one").unwrap();

    assert!(!home.join("modules").join("example.rm.one").exists());
    assert!(home
        .join("modules")
        .join("example.rm.two")
        .join("0.1.0")
        .is_dir());
    assert!(reg.inventory.modules.contains_key("example.rm.two"));
    assert!(home.join("hideout.toml.keep").is_file());
    assert!(home
        .join("data")
        .join("example.rm.one")
        .join("s.json")
        .is_file());
    // The source folders the module came from are never touched.
    assert!(one.join("a.exe").is_file());
}

/// A junction (no admin needed), made with the shell.
fn junction(link: &Path, target: &Path) {
    let out = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

fn hub_copy(home: &Path, src: &Path, id: &str) -> Registry {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let dir = src.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.exe"), id).unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"a.exe\"]\n"
        ),
    )
    .unwrap();
    let mut reg = Registry::open(home, true).unwrap();
    reg.install(&dir, NOW).unwrap();
    reg
}

/// A junction planted inside the module's own folder is unlinked, never followed: the decoy it
/// points at keeps every file.
#[test]
fn a_junction_inside_the_copy_is_not_followed() {
    let home = temp("junction-in-home");
    let src = temp("junction-in-src");
    let decoy = temp("junction-in-decoy");
    std::fs::write(decoy.join("precious.txt"), b"keep").unwrap();
    let mut reg = hub_copy(&home, &src, "example.rm.link");
    let own = home.join("modules").join("example.rm.link");
    junction(&own.join("0.1.0").join("evil"), &decoy);

    reg.remove("example.rm.link").unwrap();

    assert!(!own.exists());
    assert!(
        decoy.join("precious.txt").is_file(),
        "the decoy was deleted through a junction"
    );
}

/// `modules\<id>` itself a junction to a folder elsewhere: only the link goes.
#[test]
fn a_junction_as_the_module_folder_only_loses_the_link() {
    let home = temp("junction-top-home");
    let src = temp("junction-top-src");
    let decoy = temp("junction-top-decoy");
    std::fs::write(decoy.join("precious.txt"), b"keep").unwrap();
    let mut reg = hub_copy(&home, &src, "example.rm.top");
    let own = home.join("modules").join("example.rm.top");
    std::fs::remove_dir_all(&own).unwrap();
    junction(&own, &decoy);

    reg.remove("example.rm.top").unwrap();

    assert!(!own.exists());
    assert!(decoy.join("precious.txt").is_file());
}

/// An id outside the inventory's charset forgets its entry but never reaches a delete: a
/// trailing dot names another module's folder on Windows, and a device name is no folder.
#[test]
fn an_id_that_is_not_plain_never_deletes_a_folder() {
    let home = temp("odd-home");
    let src = temp("odd-src");
    let mut reg = hub_copy(&home, &src, "example.rm.odd");
    let own = home.join("modules").join("example.rm.odd");
    for odd in [
        "example.rm.odd.",
        "example.rm.odd ",
        "con",
        "..",
        r"a\b",
        "Example.Rm.Odd",
    ] {
        let entry = reg.inventory.modules["example.rm.odd"].clone();
        reg.inventory.modules.insert(odd.into(), entry);
        reg.remove(odd).unwrap();
        assert!(
            own.join("0.1.0").is_dir(),
            "{odd:?} deleted another module's folder"
        );
    }
}

/// An adopted module's version comes from a manifest: a path there is never joined.
#[test]
fn an_adopted_version_that_is_a_path_deletes_nothing() {
    let mut f = adopted("oddversion");
    let outside = temp("oddversion-outside");
    std::fs::write(outside.join("module.toml"), b"x").unwrap();
    let rel = format!(
        r"..\..\..\{}",
        outside.file_name().unwrap().to_string_lossy()
    );
    f.reg.inventory.modules.get_mut(ID).unwrap().version = rel;
    f.reg.remove(ID).unwrap();
    assert!(outside.join("module.toml").is_file());
}
