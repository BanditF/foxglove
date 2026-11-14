// SPDX-License-Identifier: MIT
//! Registry behaviour beyond the vectors: the launch check, grants and state (#933).

use std::path::{Path, PathBuf};

use hideout_contract::capability::Capability;
use hideout_contract::module::Visibility;
use hideout_registry::trust::ROOT_ANCHOR;
use hideout_registry::{Failure, Registry, State};

const NOW: &str = "2026-11-09T12:00:00Z";

fn vectors() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors/trust")
}

fn read(rel: &str) -> Vec<u8> {
    std::fs::read(vectors().join(rel)).unwrap()
}

fn text(rel: &str) -> String {
    String::from_utf8(read(rel)).unwrap()
}

fn root_key() -> String {
    text("keys/root.pub").lines().nth(1).unwrap().trim().into()
}

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-registry-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn signed(home: &Path) -> Registry {
    let mut reg = Registry::open(home, false).unwrap();
    let root = "root/root.valid.json";
    reg.accept_root(
        &read(root),
        &text(&format!("{root}.minisig")),
        &root_key(),
        NOW,
    )
    .unwrap();
    let cat = "catalogs/foxglove.with-revocations.catalog.json";
    reg.accept_catalog(
        &read(cat),
        &text(&format!("{cat}.minisig")),
        ROOT_ANCHOR,
        "test-public",
        Visibility::Public,
        NOW,
    )
    .unwrap();
    reg
}

fn id(r: Result<impl std::fmt::Debug, Failure>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => f.id().map_or_else(|| f.to_string(), String::from),
    }
}

#[test]
fn an_unsigned_module_runs_only_in_developer_mode() {
    let home = home("sideload");
    let folder = vectors().join("folders/example.private.notes-0.1.0");
    let mut reg = Registry::open(&home, false).unwrap();
    assert_eq!(id(reg.install(&folder, NOW)), "install.dev-mode-required");

    reg.dev_mode = true;
    let m = reg.install(&folder, NOW).unwrap();
    assert_eq!((m.source.as_str(), m.anchor), ("sideload", None));
    reg.set_enabled("example.private.notes", true).unwrap();
    assert!(reg.pre_launch("example.private.notes").is_ok());

    // The same install, read again without developer mode: refused before launch.
    let reg = Registry::open(&home, false).unwrap();
    assert_eq!(
        id(reg.pre_launch("example.private.notes")),
        "install.dev-mode-required"
    );
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn every_launch_checks_the_files_again() {
    let home = home("files");
    let mut reg = signed(&home);
    let folder = vectors().join("folders/foxglove.example-art-0.4.0");
    let m = reg.install(&folder, NOW).unwrap();
    assert_eq!(m.anchor.as_deref(), Some(ROOT_ANCHOR));
    assert!(matches!(
        reg.pre_launch("foxglove.example-art"),
        Err(Failure::Disabled(_))
    ));
    reg.set_enabled("foxglove.example-art", true).unwrap();
    let launch = reg.pre_launch("foxglove.example-art").unwrap();
    assert_eq!(launch.run, Some(vec!["example-art.exe".to_string()]));
    assert!(launch.dir.ends_with("modules/foxglove.example-art/0.4.0"));

    // Data is the module's own and never hashed; a changed exe is refused.
    std::fs::create_dir_all(launch.dir.join("data")).unwrap();
    std::fs::write(launch.dir.join("data/state.json"), "{}").unwrap();
    assert!(reg.pre_launch("foxglove.example-art").is_ok());
    std::fs::write(launch.dir.join("example-art.exe"), "tampered").unwrap();
    assert_eq!(id(reg.pre_launch("foxglove.example-art")), "trust.files");

    // Without its catalog the module doesn't launch.
    let reg = Registry::open(&home, false).unwrap();
    std::fs::copy(
        folder.join("example-art.exe"),
        launch.dir.join("example-art.exe"),
    )
    .unwrap();
    assert_eq!(
        id(reg.pre_launch("foxglove.example-art")),
        "trust.unknown-archive"
    );
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn state_and_running_follow_the_hub() {
    let home = home("state");
    let mut reg = signed(&home);
    let id_ = "foxglove.example-art";
    assert_eq!(reg.state(id_), State::NotInstalled);
    reg.install(&vectors().join("folders/foxglove.example-art-0.4.0"), NOW)
        .unwrap();
    assert_eq!(reg.state(id_), State::Off);
    reg.set_enabled(id_, true).unwrap();
    assert_eq!(reg.state(id_), State::Enabled);
    let launch = reg.pre_launch(id_).unwrap();
    reg.mark_running(&launch);
    assert_eq!(reg.state(id_), State::Running);
    assert!(reg.revoked_running().is_empty());
    assert!(matches!(
        reg.install(&vectors().join("folders/foxglove.example-art-0.4.0"), NOW),
        Err(Failure::Running(_))
    ));
    assert!(matches!(reg.remove(id_), Err(Failure::Running(_))));
    reg.mark_stopped(id_);
    reg.remove(id_).unwrap();
    assert_eq!(reg.state(id_), State::NotInstalled);
    assert!(!reg.module_dir(id_, "0.4.0").exists());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn grants_wait_for_approval_and_skip_the_never_list() {
    let home = home("grants");
    let mut reg = Registry::open(&home, true).unwrap();
    let id_ = "example.private.notes";
    reg.install(&vectors().join("folders/example.private.notes-0.1.0"), NOW)
        .unwrap();
    let caps: Vec<Capability> = ["modules.manage", "hub.notify", "ui.window", "hub.notify"]
        .iter()
        .map(|c| Capability::parse(c).unwrap())
        .collect();
    assert!(reg.inventory.ask(id_, &caps));
    assert!(reg.inventory.modules[id_].granted.is_empty());
    reg.inventory.save(&home.join("inventory.json")).unwrap();

    // Nothing is granted by asking; the user's Allow names the list it saw.
    let list = reg.permission_list(id_).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    let out = reg
        .approve_listed(id_, list.code(), &all, &[], NOW, "dialog")
        .unwrap();
    assert_eq!(out.refused.len(), 1);
    assert_eq!(out.refused[0].1.id(), "permission.never");
    let m = &reg.inventory.modules[id_];
    assert_eq!(m.granted, ["hub.notify", "ui.window"]);
    assert!(m.pending_permissions.is_empty());

    // A reinstall (an update) whose manifest asks for nothing any more drops those grants (#1178:
    // a grant never outlives the ask); what it still asks for would stay.
    reg.install(&vectors().join("folders/example.private.notes-0.1.0"), NOW)
        .unwrap();
    let again = Registry::open(&home, true).unwrap();
    assert!(again.inventory.modules[id_].granted.is_empty());
    assert!(!again.inventory.holds(id_, &caps[1]));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn a_legacy_plugin_is_checked_against_the_foxglove_api() {
    // #985: the API a plugin.toml targets is the Foxglove plugin API, not the hub's version.
    let home = home("legacy-api");
    let mut reg = Registry::open(&home, true).unwrap();
    assert_eq!(reg.legacy_api, foxglove_plugin_api::API_VERSION);
    let folder = home.join("src-legacy");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("legacy.exe"), "exe").unwrap();
    let write = |api: &str| {
        std::fs::write(
            folder.join("plugin.toml"),
            format!(
                "id = \"com.example.legacy\"
name = \"Legacy\"
version = \"0.1.0\"
                 api = \"{api}\"
run = [\"legacy.exe\"]
"
            ),
        )
        .unwrap();
    };
    // Built for the API this Foxglove speaks: installs (it read as "this Foxglove speaks 1.1").
    write(&foxglove_plugin_api::API_VERSION.to_string());
    let m = reg.install(&folder, NOW).unwrap();
    assert_eq!(m.source, "sideload");
    // A newer minor than the plugin API serves is refused.
    let api = foxglove_plugin_api::API_VERSION;
    write(&format!("{}.{}", api.major, api.minor + 1));
    assert_eq!(id(reg.install(&folder, NOW)), "lifecycle.api");
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn a_revoked_running_version_carries_its_reason() {
    // #959: the hub needs the catalog's reason to show and to end the module's leases.
    let home = home("revoked");
    let mut reg = signed(&home);
    let id_ = "foxglove.example-art";
    reg.install(&vectors().join("folders/foxglove.example-art-0.4.0"), NOW)
        .unwrap();
    reg.set_enabled(id_, true).unwrap();
    let mut launch = reg.pre_launch(id_).unwrap();
    reg.mark_running(&launch);
    assert!(reg.revoked().is_empty());
    // The catalog revokes 0.2.0 (and only yanks 0.1.0, which keeps running).
    launch.version = "0.1.0".into();
    reg.mark_running(&launch);
    assert!(reg.revoked().is_empty());
    launch.version = "0.2.0".into();
    reg.mark_running(&launch);
    assert_eq!(
        reg.revoked(),
        vec![hideout_registry::Revoked {
            id: id_.into(),
            version: "0.2.0".into(),
            reason: "placeholder reason".into(),
        }]
    );
    assert_eq!(reg.revoked_running(), vec![id_.to_string()]);
    reg.mark_stopped(id_);
    assert!(reg.revoked().is_empty());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn an_installed_revoked_version_is_withdrawn_running_or_not() {
    // #1026: the supervisor asks about the installed version, not only a running one.
    let home = home("withdrawn");
    let mut reg = signed(&home);
    let id_ = "foxglove.example-art";
    reg.install(&vectors().join("folders/foxglove.example-art-0.4.0"), NOW)
        .unwrap();
    assert_eq!(reg.revocation_of(id_), None);
    assert_eq!(reg.go_back_version(id_), None);
    // The catalog revokes 0.2.0 and yanks 0.1.0.
    let dir = home.join("modules").join(id_);
    std::fs::create_dir_all(dir.join("0.1.0")).unwrap();
    std::fs::create_dir_all(dir.join("0.2.0")).unwrap();
    {
        let m = reg.inventory.modules.get_mut(id_).unwrap();
        m.version = "0.2.0".into();
        m.previous = Some("0.1.0".into());
    }
    let r = reg.revocation_of(id_).unwrap();
    assert_eq!(
        (r.version.as_str(), r.reason.as_str()),
        ("0.2.0", "placeholder reason")
    );
    // A yanked previous version still runs, so Go back offers it.
    assert_eq!(reg.go_back_version(id_).as_deref(), Some("0.1.0"));
    // A revoked previous version is never offered, and a rollback to it is refused (§7.4).
    {
        let m = reg.inventory.modules.get_mut(id_).unwrap();
        m.version = "0.4.0".into();
        m.previous = Some("0.2.0".into());
    }
    assert_eq!(reg.revocation_of(id_), None);
    assert_eq!(reg.go_back_version(id_), None);
    assert_eq!(id(reg.roll_back(id_)), "trust.revoked");
    // No previous folder: nothing to offer.
    reg.inventory.modules.get_mut(id_).unwrap().previous = Some("0.3.0".into());
    assert_eq!(reg.go_back_version(id_), None);
    let _ = std::fs::remove_dir_all(home);
}
