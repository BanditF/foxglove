// SPDX-License-Identifier: MIT
//! The `adopted` kind (§3, §7.4, #1115): register, shallow verify, launch check and remove
//! (unregister only). The "install" is a fake exe in a temp folder outside the hub's home.

use std::path::{Path, PathBuf};

use hideout_registry::inventory::ADOPTED;
use hideout_registry::{Failure, Registry, State};

const NOW: &str = "2026-11-11T12:00:00Z";
const ID: &str = "vulpes";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-adopted-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(version: &str, exe: &Path) -> String {
    let exe = exe.display().to_string().replace('\\', "/");
    format!(
        "id = \"{ID}\"\nname = \"Example AI\"\nversion = \"{version}\"\nkind = \"adopted\"\n\
         family = \"arete\"\nhub = \"1.0\"\nrun = ['{exe}']\napi = {{ arete = \"1.0\" }}\n"
    )
}

/// An external install (an exe, a data file and a secret) and a folder holding its
/// `module.toml` at `version`. Returns (install folder, manifest folder).
fn external(name: &str, version: &str) -> (PathBuf, PathBuf) {
    let install = temp(&format!("{name}-install"));
    std::fs::write(install.join("hub.exe"), b"not a real exe").unwrap();
    std::fs::write(install.join("data.db"), b"user data").unwrap();
    std::fs::write(install.join("secret.key"), b"secret").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(
        folder.join("module.toml"),
        manifest(version, &install.join("hub.exe")),
    )
    .unwrap();
    (install, folder)
}

fn id(r: Result<impl std::fmt::Debug, Failure>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => f.id().map_or_else(|| f.to_string(), String::from),
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn an_adopted_module_is_recorded_without_hashes_and_survives_a_reopen() {
    let home = temp("register-home");
    let (install, folder) = external("register", "0.4.0");
    let mut reg = Registry::open(&home, false).unwrap();
    let m = reg.register_adopted(&folder, NOW).unwrap();
    assert_eq!((m.source.as_str(), m.version.as_str()), (ADOPTED, "0.4.0"));
    assert!(m.files.is_empty() && m.anchor.is_none());
    // The hub keeps its own copy of the manifest and writes nothing into the install.
    assert!(reg.module_dir(ID, "0.4.0").join("module.toml").is_file());
    assert_eq!(names(&install), ["data.db", "hub.exe", "secret.key"]);
    assert_eq!(reg.module_name(ID), "Example AI");

    let reopened = Registry::open(&home, false).unwrap();
    assert_eq!(reopened.inventory.modules[ID], m);
    assert_eq!(reopened.state(ID), State::Off);
}

#[test]
fn a_folder_that_is_not_adopted_or_is_inside_the_hub_is_refused() {
    let home = temp("refuse-home");
    let mut reg = Registry::open(&home, false).unwrap();
    // A service manifest is not an adopted one.
    let svc = temp("refuse-service");
    std::fs::write(
        svc.join("module.toml"),
        "id = \"example.svc\"\nname = \"Svc\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
         family = \"third-party\"\nhub = \"1.0\"\nrun = [\"svc.exe\"]\n",
    )
    .unwrap();
    assert_eq!(id(reg.register_adopted(&svc, NOW)), "manifest.kind");
    // run[0] under the hub's own modules folder is not an external install.
    let inside = home.join("modules").join("x").join("hub.exe");
    std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
    std::fs::write(&inside, b"x").unwrap();
    let folder = temp("refuse-manifest");
    std::fs::write(folder.join("module.toml"), manifest("0.4.0", &inside)).unwrap();
    assert_eq!(id(reg.register_adopted(&folder, NOW)), "manifest.run");
    assert!(reg.inventory.modules.is_empty());
}

#[test]
fn a_missing_path_is_not_found_never_a_crash_and_never_hashed() {
    let home = temp("missing-home");
    let (install, folder) = external("missing", "0.4.0");
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.set_enabled(ID, true).unwrap();
    assert!(reg.verify_adopted(ID).is_ok());
    assert!(reg.pre_launch(ID).is_ok());

    // Other files changing are no business of the hub: path and version only.
    std::fs::write(install.join("data.db"), b"changed").unwrap();
    assert!(reg.pre_launch(ID).is_ok());

    std::fs::remove_file(install.join("hub.exe")).unwrap();
    for r in [reg.verify_adopted(ID), reg.pre_launch(ID).map(|_| ())] {
        match r {
            Err(Failure::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
            other => panic!("expected not found, got {other:?}"),
        }
    }
    // Registering a path that isn't there is refused the same way.
    let mut other = Registry::open(&temp("missing-home2"), false).unwrap();
    assert!(matches!(
        other.register_adopted(&folder, NOW),
        Err(Failure::Io(e)) if e.kind() == std::io::ErrorKind::NotFound
    ));
}

#[test]
fn registering_again_updates_the_version_and_keeps_the_choice() {
    let home = temp("again-home");
    let (_, v1) = external("again1", "0.4.0");
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&v1, NOW).unwrap();
    reg.set_enabled(ID, true).unwrap();
    let (_, v2) = external("again2", "0.5.0");
    let m = reg.register_adopted(&v2, "2026-11-12T00:00:00Z").unwrap();
    assert_eq!((m.version.as_str(), m.installed.as_str()), ("0.5.0", NOW));
    assert!(reg.hub.enabled(ID));
    assert!(!reg.module_dir(ID, "0.4.0").exists());
    assert_eq!(reg.pre_launch(ID).unwrap().version, "0.5.0");
}

#[test]
fn remove_unregisters_only_and_never_touches_the_install() {
    let home = temp("remove-home");
    let (install, folder) = external("remove", "0.4.0");
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.set_enabled(ID, true).unwrap();
    // What the hub kept for it (settings, data) stays; only the manifest record goes.
    let kept = reg.module_dir(ID, "0.4.0").join("data");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::write(kept.join("note.txt"), b"hub-side data").unwrap();

    reg.remove(ID).unwrap();
    assert!(!reg.inventory.modules.contains_key(ID));
    assert!(Registry::open(&home, false)
        .unwrap()
        .inventory
        .modules
        .is_empty());
    assert_eq!(names(&install), ["data.db", "hub.exe", "secret.key"]);
    assert_eq!(
        std::fs::read(install.join("secret.key")).unwrap(),
        b"secret"
    );
    assert!(kept.join("note.txt").is_file());
    assert!(!reg.module_dir(ID, "0.4.0").join("module.toml").exists());
    assert_eq!(id(reg.remove(ID)), "vulpes is not installed");
}

#[test]
fn an_installed_id_is_not_adopted_over() {
    let home = temp("clash-home");
    let (_, folder) = external("clash", "0.4.0");
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.inventory.modules.get_mut(ID).unwrap().source = "sideload".into();
    assert!(matches!(
        reg.register_adopted(&folder, NOW),
        Err(Failure::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists
    ));
}

#[test]
fn a_second_adopted_provide_grant_for_the_same_area_is_refused() {
    use hideout_contract::capability::Capability;
    let home = temp("two-provide-home");
    let mut reg = Registry::open(&home, false).unwrap();
    let (_, a) = external("two-a", "0.1.0");
    reg.register_adopted(&a, NOW).unwrap();
    // A second adopted module under another id.
    let install = temp("two-b-install");
    std::fs::write(install.join("hub.exe"), b"x").unwrap();
    let b = temp("two-b-manifest");
    let exe = install
        .join("hub.exe")
        .display()
        .to_string()
        .replace('\\', "/");
    std::fs::write(
        b.join("module.toml"),
        format!(
            "id = \"other.adopted\"\nname = \"B\"\nversion = \"0.1.0\"\nkind = \"adopted\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nrun = ['{exe}']\n"
        ),
    )
    .unwrap();
    reg.register_adopted(&b, NOW).unwrap();
    let gpu = [Capability::parse("provide:gpu.*").unwrap()];
    let one = reg.inventory.grant(ID, &gpu).unwrap();
    assert!(one.is_empty());
    let two = reg.inventory.grant("other.adopted", &gpu).unwrap();
    assert_eq!(two.len(), 1, "{two:?}");
    assert_eq!(two[0].id.to_string(), "permission.provider");
    assert!(reg.inventory.modules["other.adopted"].granted.is_empty());
    // A narrower name inside the held area overlaps too.
    let inner = [Capability::parse("provide:gpu.request").unwrap()];
    assert_eq!(
        reg.inventory.grant("other.adopted", &inner).unwrap().len(),
        1
    );
}
