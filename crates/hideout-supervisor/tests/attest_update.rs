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
    signed_with(version, seed, None)
}

fn signed_with(version: &str, seed: u8, exe: Option<&str>) -> (Vec<u8>, Vec<u8>, [u8; 32]) {
    let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
    let exe = exe.map_or(String::new(), |h| format!(r#","exe_sha256":"{h}""#));
    let json = format!(
        r#"{{"version":"{version}","tag":"app-v{version}","msi":{{"name":"a.msi","url":"https://x/a.msi","sha256":"00","size":1}},"notes":"","published":"2026-11-13T00:00:00Z"{exe}}}"#
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
    let run = exe_in
        .unwrap_or(&exe)
        .display()
        .to_string()
        .replace('\\', "/");
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
    let old = text
        .lines()
        .find(|l| l.starts_with("run"))
        .unwrap()
        .to_string();
    let new = format!(
        "run = [\"{}\"]",
        exe2.display().to_string().replace('\\', "/")
    );
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

fn handoff_dir(home: &Path) -> PathBuf {
    let d = home.parent().unwrap().join("updates");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_handoff(dir: &Path, version: &str, seed: u8) -> [u8; 32] {
    let (json, sig, key) = signed(version, seed);
    foxglove_update::handoff::write(dir, &foxglove_update::Signed { json, sig }).unwrap();
    key
}

#[test]
fn the_updaters_handoff_keeps_the_grant_once_and_only_when_signed_and_newer() {
    let (home, exe, id) = home("handoff", None);
    let sup = sup(&home);
    allow(&sup, &home, &id);
    let dir = handoff_dir(&home);

    // A hand-off signed by another key is dropped and consumed; nothing is attested.
    std::fs::write(&exe, b"program v2").unwrap();
    let _ = write_handoff(&dir, "1.1.0", 8);
    let key = signed("1.1.0", 7).2;
    assert!(
        hideout_supervisor::attest::attest_from_handoff(&home, &dir, &id, &key, None).is_none(),
        "a signature from another key is not accepted"
    );
    assert!(!dir.join(foxglove_update::handoff::FILE).exists());

    // The real key's release is attested, once; the file is gone afterwards.
    let _ = write_handoff(&dir, "1.1.0", 7);
    assert_eq!(
        hideout_supervisor::attest::attest_from_handoff(&home, &dir, &id, &key, None).as_deref(),
        Some("1.1.0")
    );
    assert!(
        hideout_supervisor::attest::attest_from_handoff(&home, &dir, &id, &key, None).is_none()
    );
    let reg = Registry::open_from_env(&home).unwrap();
    let m = &reg.inventory.modules[&id];
    assert_eq!(m.granted, ["hub.notify"]);
    assert_eq!(m.bound.as_ref().unwrap().attested[0].by, "updater");

    // The same release again, with the program changed once more, is a replay: refused.
    std::fs::write(&exe, b"program v3").unwrap();
    let _ = write_handoff(&dir, "1.1.0", 7);
    assert!(
        hideout_supervisor::attest::attest_from_handoff(&home, &dir, &id, &key, None).is_none()
    );
    let reg = Registry::open_from_env(&home).unwrap();
    assert_eq!(
        reg.inventory.modules[&id]
            .bound
            .as_ref()
            .unwrap()
            .attested
            .len(),
        1
    );
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_stale_handoff_is_not_used() {
    let (home, exe, id) = home("stale", None);
    let sup = sup(&home);
    allow(&sup, &home, &id);
    let dir = handoff_dir(&home);
    std::fs::write(&exe, b"program v2").unwrap();
    let key = write_handoff(&dir, "1.1.0", 7);
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
    std::fs::File::options()
        .write(true)
        .open(dir.join(foxglove_update::handoff::FILE))
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert!(
        hideout_supervisor::attest::attest_from_handoff(&home, &dir, &id, &key, None).is_none()
    );
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

fn attested(home: &Path, id: &str) -> usize {
    Registry::open_from_env(home).unwrap().inventory.modules[id]
        .bound
        .as_ref()
        .unwrap()
        .attested
        .len()
}

#[test]
fn the_version_rule_always_applies_to_the_installed_version_too() {
    let (home, exe, id) = home("ver", None);
    let sup = sup(&home);
    allow(&sup, &home, &id);
    std::fs::write(&exe, b"program v2").unwrap();
    // The module is recorded at 1.0.0: the same or an older release is a replay; a version that
    // doesn't parse is refused too.
    for v in ["1.0.0", "0.9.0", "banana"] {
        let (json, sig, key) = signed(v, 7);
        let ver = Verified::from_signed(&json, &sig, &key).unwrap();
        assert!(sup.attest_verified_update(&id, &ver).is_err(), "{v}");
    }
    assert_eq!(attested(&home, &id), 0);
    let (json, sig, key) = signed("1.0.1", 7);
    let ver = Verified::from_signed(&json, &sig, &key).unwrap();
    sup.attest_verified_update(&id, &ver).unwrap();
    // Now 1.0.1 is the floor.
    std::fs::write(&exe, b"program v3").unwrap();
    assert!(sup.attest_verified_update(&id, &ver).is_err());
    assert_eq!(attested(&home, &id), 1);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_release_that_names_the_program_hash_must_match_it() {
    let (home, exe, id) = home("hash", None);
    let sup = sup(&home);
    allow(&sup, &home, &id);
    std::fs::write(&exe, b"program v2").unwrap();
    let good = sha_hex(b"program v2").to_uppercase();
    let bad = sha_hex(b"something else");
    let (json, sig, key) = signed_with("1.1.0", 7, Some(&bad));
    let ver = Verified::from_signed(&json, &sig, &key).unwrap();
    assert!(sup.attest_verified_update(&id, &ver).is_err());
    assert_eq!(attested(&home, &id), 0);
    let (json, sig, key) = signed_with("1.1.0", 7, Some(&good));
    let ver = Verified::from_signed(&json, &sig, &key).unwrap();
    sup.attest_verified_update(&id, &ver).unwrap();
    assert_eq!(attested(&home, &id), 1);
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_handoff_older_than_the_instance_is_not_used() {
    let (home, exe, id) = home("since", None);
    let sup = sup(&home);
    allow(&sup, &home, &id);
    let dir = handoff_dir(&home);
    std::fs::write(&exe, b"program v2").unwrap();
    let key = write_handoff(&dir, "1.1.0", 7);
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    assert!(
        hideout_supervisor::attest::attest_from_handoff(&home, &dir, &id, &key, Some(later))
            .is_none()
    );
    let _ = write_handoff(&dir, "1.1.0", 7);
    let before = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    assert_eq!(
        hideout_supervisor::attest::attest_from_handoff(&home, &dir, &id, &key, Some(before))
            .as_deref(),
        Some("1.1.0")
    );
    drop(sup);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

fn sha_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(b))
}
