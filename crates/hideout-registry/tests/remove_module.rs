// SPDX-License-Identifier: MIT
//! "Remove from Hideout" (product slice 6): the registry side. The module's permissions and
//! keyboard grant go with its inventory entry, its own files and the hub's settings folder stay,
//! nothing outside the hub's own `modules\<id>` is deleted, and adopting it again asks again.
//! Window-free; the "install" is a fake exe in a temp folder.

use std::os::windows::process::CommandExt as _;
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
        std::fs::write(dir.join("a.exe"), format!("{id}: not a program")).unwrap();
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

    // The app's own data and settings under its version folder (module_dir\data, \settings).
    let ver = home.join("modules").join("example.rm.one").join("0.1.0");
    for sub in ["data", "settings"] {
        std::fs::create_dir_all(ver.join(sub)).unwrap();
        std::fs::write(ver.join(sub).join("keep.txt"), sub).unwrap();
    }

    reg.remove("example.rm.one").unwrap();

    // The payload is gone; data and settings survive.
    assert!(!ver.join("a.exe").exists() && !ver.join("module.toml").exists());
    assert!(ver.join("data").join("keep.txt").is_file());
    assert!(ver.join("settings").join("keep.txt").is_file());
    assert!(!reg.inventory.modules.contains_key("example.rm.one"));
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
        .creation_flags(0x0800_0000)
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
    std::fs::write(dir.join("a.exe"), format!("{id}: not a program")).unwrap();
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

fn put(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Two version folders, `settings.bad-*` and other-case names all keep their app folders; the
/// payload goes; a junction named `data` is kept and never followed.
#[test]
fn remove_keeps_every_versions_app_folders_in_any_case_and_follows_no_link() {
    let home = temp("multi-home");
    let src = temp("multi-src");
    let decoy = temp("multi-decoy");
    std::fs::write(decoy.join("precious.txt"), b"keep").unwrap();
    let mut reg = hub_copy(&home, &src, "example.rm.multi");
    let base = home.join("modules").join("example.rm.multi");
    let old = base.join("0.0.9");
    put(&old, "a.exe", "old payload");
    put(&old, "Settings/s.json", "old s");
    put(&old, "settings.bad-123/s.json", "bad");
    let cur = base.join("0.1.0");
    put(&cur, "Data/d.txt", "d");
    put(&cur, "settings.BAD-9/x.txt", "bad9");
    put(&cur, "sub/payload.bin", "p");

    reg.remove("example.rm.multi").unwrap();

    assert!(!old.join("a.exe").exists() && !cur.join("a.exe").exists());
    assert!(!cur.join("sub").exists() && !cur.join("module.toml").exists());
    assert!(old.join("Settings").join("s.json").is_file());
    assert!(old.join("settings.bad-123").join("s.json").is_file());
    assert!(cur.join("Data").join("d.txt").is_file());
    assert!(cur.join("settings.BAD-9").join("x.txt").is_file());

    // A junction named `data`: kept as a link, the decoy untouched.
    let mut reg = hub_copy(&home, &src, "example.rm.multi");
    let cur = base.join("0.1.0");
    std::fs::remove_dir_all(cur.join("data")).ok();
    junction(&cur.join("data"), &decoy);
    reg.remove("example.rm.multi").unwrap();
    assert!(decoy.join("precious.txt").is_file());
    assert!(cur.join("data").join("precious.txt").is_file());
}

/// Remove, then install again: the same version keeps its data and settings (not wiped by the
/// swap), and another version's leftovers move into the new copy instead of being pruned.
#[test]
fn install_after_remove_keeps_data_and_settings() {
    let home = temp("readd-home");
    let src = temp("readd-src");
    let mut reg = hub_copy(&home, &src, "example.rm.readd");
    let base = home.join("modules").join("example.rm.readd");
    put(&base.join("0.1.0"), "data/d.txt", "d1");
    put(&base.join("0.1.0"), "settings/s.json", "s1");
    put(&base.join("0.1.0"), "settings.bad-5/s.json", "b1");
    reg.remove("example.rm.readd").unwrap();

    // Same version again.
    reg.install(&src.join("example.rm.readd"), NOW).unwrap();
    let cur = base.join("0.1.0");
    assert!(cur.join("a.exe").is_file());
    assert_eq!(
        std::fs::read(cur.join("data").join("d.txt")).unwrap(),
        b"d1"
    );
    assert!(cur.join("settings").join("s.json").is_file());
    assert!(cur.join("settings.bad-5").join("s.json").is_file());

    // Remove, then a newer version: the old folder's data moves over.
    reg.remove("example.rm.readd").unwrap();
    let manifest = src.join("example.rm.readd").join("module.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("0.1.0", "0.2.0")).unwrap();
    reg.install(&src.join("example.rm.readd"), NOW).unwrap();
    let new = base.join("0.2.0");
    assert_eq!(
        std::fs::read(new.join("data").join("d.txt")).unwrap(),
        b"d1"
    );
    assert!(new.join("settings").join("s.json").is_file());
    assert!(!base.join("0.1.0").exists());
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

/// An archive that ships its own `data` never replaces the app's data, and two older versions
/// that both hold settings are carried, the newest under the plain name, the other beside it.
#[test]
fn collisions_keep_every_copy_and_the_newest_takes_the_name() {
    let home = temp("coll-home");
    let src = temp("coll-src");
    let mut reg = hub_copy(&home, &src, "example.rm.coll");
    // The archive itself ships a data folder.
    put(&src.join("example.rm.coll"), "data/shipped.txt", "shipped");
    let base = home.join("modules").join("example.rm.coll");
    put(&base.join("0.0.8"), "settings/s.json", "v8");
    put(&base.join("0.0.9"), "settings/s.json", "v9");
    put(&base.join("0.1.0"), "data/d.txt", "mine");
    reg.remove("example.rm.coll").unwrap();

    reg.install(&src.join("example.rm.coll"), NOW).unwrap();
    let cur = base.join("0.1.0");
    assert_eq!(read(&cur.join("data").join("d.txt")), "mine");
    assert!(!cur.join("data").join("shipped.txt").exists());
    assert_eq!(read(&cur.join("settings").join("s.json")), "v9");
    assert_eq!(read(&cur.join("settings.from-0.0.8").join("s.json")), "v8");
    assert!(!base.join("0.0.8").exists() && !base.join("0.0.9").exists());
}

/// A swap that died half way: the original comes back, and neither `.old` nor `.partial` is
/// wiped before what they hold is in the live folder.
#[test]
fn an_interrupted_install_loses_no_data() {
    let home = temp("crash-home");
    let src = temp("crash-src");
    let mut reg = hub_copy(&home, &src, "example.rm.crash");
    let base = home.join("modules").join("example.rm.crash");
    reg.remove("example.rm.crash").unwrap();
    // dir gone; the original is `.old`, a copy-in-progress is `.partial`.
    put(&base.join("0.1.0.old"), "data/d.txt", "orig");
    put(&base.join("0.1.0.old"), "a.exe", "x");
    put(&base.join("0.1.0.partial"), "data/d.txt", "partial copy");

    reg.install(&src.join("example.rm.crash"), NOW).unwrap();
    let cur = base.join("0.1.0");
    assert_eq!(read(&cur.join("data").join("d.txt")), "orig");
    assert_eq!(
        read(&cur.join("data.from-0.1.0.partial").join("d.txt")),
        "partial copy"
    );
    assert!(!base.join("0.1.0.old").exists() && !base.join("0.1.0.partial").exists());

    // A leftover that holds data with no live folder to carry it to is never deleted.
    reg.remove("example.rm.crash").unwrap();
    std::fs::remove_dir_all(base.join("0.1.0").join("a.exe")).ok();
    std::fs::remove_dir_all(&cur).unwrap();
    put(&base.join("0.1.0.partial"), "data/keep.txt", "k");
    // No live folder and an orphan `.partial` holding data: set aside, then carried after the
    // swap (no stuck install).
    reg.install(&src.join("example.rm.crash"), NOW).unwrap();
    assert_eq!(read(&cur.join("data").join("keep.txt")), "k");
    assert!(!base.join("0.1.0.partial").exists() && !base.join("0.1.0.orphan").exists());
}

/// Carried settings do not fail the launch file check (data and settings* in any case).
#[test]
fn carried_settings_pass_the_launch_check() {
    let home = temp("launch-home");
    let src = temp("launch-src");
    let mut reg = hub_copy(&home, &src, "example.rm.launch");
    let base = home.join("modules").join("example.rm.launch");
    put(&base.join("0.1.0"), "Settings/s.json", "s");
    put(&base.join("0.1.0"), "settings.bad-1/s.json", "b");
    put(&base.join("0.1.0"), "data/d.txt", "d");
    reg.remove("example.rm.launch").unwrap();
    reg.install(&src.join("example.rm.launch"), NOW).unwrap();
    reg.set_enabled("example.rm.launch", true).unwrap();
    reg.pre_launch("example.rm.launch").unwrap();
}

/// `modules\<id>\<ver>` a junction at install time: only the link goes; nothing behind it is
/// read, moved or deleted.
#[test]
fn a_version_level_junction_is_not_followed_by_install() {
    let home = temp("vjunc-home");
    let src = temp("vjunc-src");
    let decoy = temp("vjunc-decoy");
    put(&decoy, "data/precious.txt", "keep");
    let mut reg = hub_copy(&home, &src, "example.rm.vjunc");
    let ver = home.join("modules").join("example.rm.vjunc").join("0.1.0");
    std::fs::remove_dir_all(&ver).unwrap();
    junction(&ver, &decoy);

    reg.install(&src.join("example.rm.vjunc"), NOW).unwrap();

    assert_eq!(read(&decoy.join("data").join("precious.txt")), "keep");
    assert!(!ver.join("data").exists());
    assert!(ver.join("a.exe").is_file());
}

fn bump(src: &Path, id: &str, from: &str, to: &str) {
    let m = src.join(id).join("module.toml");
    let t = std::fs::read_to_string(&m).unwrap();
    std::fs::write(&m, t.replace(from, to)).unwrap();
}

/// A catalog/sideload archive cannot smuggle `settings*`, `data.from-*` folders in; only a file
/// of that name is ordinary payload (hashed).
#[test]
fn an_archive_cannot_ship_the_apps_own_folders() {
    let home = temp("ship-home");
    let src = temp("ship-src");
    let mut reg = hub_copy(&home, &src, "example.rm.ship");
    let pkg = src.join("example.rm.ship");
    put(&pkg, "settings/evil.json", "evil");
    put(&pkg, "Settings.bad-1/x", "x");
    put(&pkg, "data.from-0.0.1/x", "x");
    reg.remove("example.rm.ship").unwrap();
    reg.install(&pkg, NOW).unwrap();
    let cur = home.join("modules").join("example.rm.ship").join("0.1.0");
    assert!(cur.join("a.exe").is_file());
    for n in ["settings", "Settings.bad-1", "data.from-0.0.1"] {
        assert!(!cur.join(n).exists(), "{n} came from the archive");
    }
}

/// A junction inside the app's data, and one named `data`, never wedge a re-add or a prune, and
/// the folders behind them are untouched.
#[test]
fn links_in_kept_data_do_not_wedge_install() {
    let home = temp("kl-home");
    let src = temp("kl-src");
    let decoy = temp("kl-decoy");
    put(&decoy, "precious.txt", "keep");
    let mut reg = hub_copy(&home, &src, "example.rm.kl");
    let base = home.join("modules").join("example.rm.kl");
    let cur = base.join("0.1.0");
    put(&cur, "data/d.txt", "d");
    junction(&cur.join("data").join("link"), &decoy);
    junction(&cur.join("settings"), &decoy);
    // An older version with the same shape, to be pruned.
    put(&base.join("0.0.5"), "data/o.txt", "o");
    junction(&base.join("0.0.5").join("data").join("l2"), &decoy);

    reg.install(&src.join("example.rm.kl"), NOW).unwrap();
    reg.remove("example.rm.kl").unwrap();
    reg.install(&src.join("example.rm.kl"), NOW).unwrap();

    assert_eq!(read(&decoy.join("precious.txt")), "keep");
    assert_eq!(read(&cur.join("data").join("d.txt")), "d");
    assert!(!base.join("0.0.5").exists());
}

/// An update does not start on settings two versions old: with a previous version, a leftover's
/// settings only land beside, never as the plain `settings`.
#[test]
fn an_update_does_not_revive_old_settings() {
    let home = temp("upd-home");
    let src = temp("upd-src");
    let mut reg = hub_copy(&home, &src, "example.rm.update");
    let pkg = src.join("example.rm.update");
    let base = home.join("modules").join("example.rm.update");
    bump(&src, "example.rm.update", "0.1.0", "0.2.0");
    reg.install(&pkg, NOW).unwrap();
    put(&base.join("0.1.0"), "settings/s.json", "v1");
    put(&base.join("0.0.9"), "settings/s.json", "v09");
    bump(&src, "example.rm.update", "0.2.0", "0.3.0");
    reg.install(&pkg, NOW).unwrap();
    let cur = base.join("0.3.0");
    assert!(!cur.join("settings").exists());
    assert_eq!(read(&cur.join("settings.from-0.1.0").join("s.json")), "v1");
    assert_eq!(read(&cur.join("settings.from-0.0.9").join("s.json")), "v09");
    assert!(base.join("0.2.0").is_dir());
}

/// A prune that cannot carry (a different copy already sits at the `.from-` name) is logged,
/// not a failed install, and the leftover keeps its data.
#[test]
fn a_stuck_prune_is_not_a_failed_install() {
    let home = temp("stuck-home");
    let src = temp("stuck-src");
    let mut reg = hub_copy(&home, &src, "example.rm.stuck");
    let pkg = src.join("example.rm.stuck");
    let base = home.join("modules").join("example.rm.stuck");
    bump(&src, "example.rm.stuck", "0.1.0", "0.2.0");
    put(&base.join("0.0.9"), "settings/s.json", "old");
    put(&base.join("0.2.0"), "settings.from-0.0.9/s.json", "other");
    reg.install(&pkg, NOW).unwrap();
    assert_eq!(
        read(&base.join("0.0.9").join("settings").join("s.json")),
        "old"
    );
    assert_eq!(
        read(
            &base
                .join("0.2.0")
                .join("settings.from-0.0.9")
                .join("s.json")
        ),
        "other"
    );
    // A temp copy left by a crash is cleaned up by the next install.
    put(&base.join("0.2.0"), "data.carry-tmp/x", "x");
    reg.install(&pkg, NOW).unwrap();
    assert!(!base.join("0.2.0").join("data.carry-tmp").exists());
}
