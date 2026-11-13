// SPDX-License-Identifier: MIT
//! The updater's vouching (#1178): `attest_verified_update` takes no path, needs a release
//! signed with the update key, and keeps a bound grant only for a program in the same folder.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use base64::{engine::general_purpose::STANDARD, Engine};
use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::attest::Verified;
use hideout_supervisor::driver::Supervisor;
use ring::signature::{Ed25519KeyPair, KeyPair};

const NOW: &str = "2026-11-13T00:00:00Z";

fn signed(version: &str, seed: u8) -> (Vec<u8>, Vec<u8>, [u8; 32]) {
    let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
    let json = format!(
        r#"{{"version":"{version}","tag":"app-v{version}","msi":{{"name":"a.msi","url":"https://x/a.msi","sha256":"00","size":1}},"notes":"","published":"2026-11-13T00:00:00Z"}}"#
    );
    let sig = STANDARD.encode(pair.sign(json.as_bytes()).as_ref());
    let key: [u8; 32] = pair.public_key().as_ref().try_into().unwrap();
    (json.into_bytes(), sig.into_bytes(), key)
}

fn home(name: &str, exe_in: Option<&Path>) -> (PathBuf, PathBuf, String) {
    let id = format!("example.lab.attest{name}");
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-attest-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let install = base.join("install");
    let folder = base.join("manifest");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let exe = install.join("prog.exe");
    std::fs::write(&exe, b"program v1").unwrap();
    let run = exe_in.unwrap_or(&exe).display().to_string().replace('\\', "/");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"A\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"arete\"\nhub = \"1.0\"\nrun = [\"{run}\"]\n\
             api = {{ arete = \"1.0\" }}\npermissions = [\"hub.notify\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    Registry::open(&home, true)
        .unwrap()
        .register_adopted(&folder, NOW)
        .unwrap();
    (home, exe, id)
}

fn sup(home: &Path) -> Supervisor {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    Supervisor::start(home, Hub::new(router, events))
}

fn allow(sup: &Supervisor, home: &Path, id: &str) {
    let code = Registry::open_from_env(home)
        .unwrap()
        .permission_list(id)
        .unwrap()
        .code()
        .to_string();
    sup.grant_listed(id, &code, &["hub.notify".to_string()], &[], "dialog")
        .unwrap();
}

#[test]
fn a_signed_update_in_the_same_folder_keeps_the_grant_and_is_recorded() {
    let (home, exe, id) = home("ok", None);
    let sup = sup(&home);
    allow(&sup, &home, &id);

    std::fs::write(&exe, b"program v2").unwrap();
    let (json, sig, key) = signed("1.1.0", 7);
    let v = Verified::from_signed(&json, &sig, &key).unwrap();
    sup.attest_verified_update(&id, &v).unwrap();

    let reg = Registry::open_from_env(&home).unwrap();
    let m = &reg.inventory.modules[&id];
    assert_eq!(m.granted, ["hub.notify"]);
    let b = m.bound.as_ref().unwrap();
    assert_eq!(b.attested.len(), 1);
    assert_eq!(
        (b.attested[0].version.as_str(), b.attested[0].by.as_str()),
        ("1.1.0", "updater")
    );
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn an_unsigned_or_wrongly_signed_release_cannot_make_a_verified_update() {
    let (json, sig, key) = signed("1.1.0", 7);
    let (_, _, other_key) = signed("1.1.0", 8);
    assert!(Verified::from_signed(&json, &sig, &other_key).is_err());
    let mut tampered = json.clone();
    tampered[12] = b'9';
    assert!(Verified::from_signed(&tampered, &sig, &key).is_err());
    assert!(Verified::from_signed(&json, b"not a signature", &key).is_err());
}

#[test]
fn another_folder_or_a_void_grant_is_refused_and_changes_nothing() {
    // The program now runs from another folder than the one allowed.
    let other = std::env::temp_dir().join(format!("hideout-attest-other-{}", std::process::id()));
    std::fs::create_dir_all(&other).unwrap();
    let (home_a, _exe, id) = home("folder", None);
    let sup_a = sup(&home_a);
    allow(&sup_a, &home_a, &id);
    let exe2 = other.join("prog.exe");
    std::fs::write(&exe2, b"program v1").unwrap();
    let folder = home_a.parent().unwrap().join("manifest");
    let text = std::fs::read_to_string(folder.join("module.toml")).unwrap();
    let old = text.lines().find(|l| l.starts_with("run")).unwrap().to_string();
    let new = format!("run = [\"{}\"]", exe2.display().to_string().replace('\\', "/"));
    std::fs::write(folder.join("module.toml"), text.replace(&old, &new)).unwrap();
    Registry::open(&home_a, true)
        .unwrap()
        .register_adopted(&folder, NOW)
        .unwrap();
    let (json, sig, key) = signed("1.1.0", 7);
    let v = Verified::from_signed(&json, &sig, &key).unwrap();
    assert!(sup_a.attest_verified_update(&id, &v).is_err());
    let reg = Registry::open_from_env(&home_a).unwrap();
    assert!(reg.inventory.modules[&id]
        .bound
        .as_ref()
        .unwrap()
        .attested
        .is_empty());
    drop(sup_a);

    // Nothing bound at all: refused.
    let (home_b, _e, id_b) = home("unbound", None);
    let sup_b = sup(&home_b);
    assert!(sup_b.attest_verified_update(&id_b, &v).is_err());
    drop(sup_b);
    let _ = std::fs::remove_dir_all(home_a.parent().unwrap());
    let _ = std::fs::remove_dir_all(home_b.parent().unwrap());
    let _ = std::fs::remove_dir_all(other);
}
