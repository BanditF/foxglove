// SPDX-License-Identifier: MIT
//! Accepted catalogs saved per source and loaded at open; a source fetched once through a
//! stand-in (#1049, contract §2.3, §12.4, §13.4). No network: the fetcher is a closure.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use foxglove_plugin_trust::sign;
use hideout_registry::catalogs::{read_stored, stored_path, Fetch};
use hideout_registry::trust::ROOT_ANCHOR;
use hideout_registry::{Failure, Registry};
use serde_json::json;

const NOW: &str = "2026-11-10T12:00:00Z";
const URL: &str = "https://catalog.example.invalid/c/catalog.json";
const ID: &str = "example.test.art";

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-registry-catalogs")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A TEST-ONLY key pair made for this run: (public base64, unencrypted secret file).
fn keys() -> (String, String) {
    let k = sign::keygen(Some(String::new())).unwrap();
    (k.public_base64, k.secret)
}

fn signed(secret: &str, bytes: &[u8]) -> String {
    sign::sign(
        &sign::load_secret(secret, Some(String::new())).unwrap(),
        bytes,
    )
    .unwrap()
}

fn user_config(home: &Path, key: &str) {
    std::fs::write(
        home.join("hideout.toml"),
        format!(
            "[[trust.anchors]]\nid = \"test-user\"\nkind = \"user\"\nkey = \"{key}\"\n\
             scope = [\"example.test.*\"]\n\n[[sources]]\nid = \"mine\"\nkind = \"catalog\"\n\
             visibility = \"private\"\nurl = \"{URL}\"\nanchor = \"test-user\"\n"
        ),
    )
    .unwrap();
}

/// A user-signed catalog with `seq`, revoking `example.test.art@0.5.0` when `revoke`.
fn catalog(secret: &str, seq: u64, revoke: bool) -> (Vec<u8>, String) {
    catalog_at(secret, 0, seq, revoke)
}

/// [`catalog`] with an `epoch` of the publisher's choosing.
fn catalog_at(secret: &str, epoch: u64, seq: u64, revoke: bool) -> (Vec<u8>, String) {
    let revocations = if revoke {
        json!([{ "id": ID, "versions": ["0.5.0"], "action": "revoke", "reason": "broken" }])
    } else {
        json!([])
    };
    let doc = json!({
        "format": "hideout-catalog-v1", "epoch": epoch, "sequence": seq,
        "expires": "2099-01-01T00:00:00Z", "modules": [], "revocations": revocations,
    });
    let bytes = serde_json::to_vec_pretty(&doc).unwrap();
    let sig = signed(secret, &bytes);
    (bytes, sig)
}

type Files = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// Serves `files` by URL; anything else (or nothing set) is offline.
fn fetcher(files: &Files) -> impl Fetch {
    let f = files.clone();
    move |url: &str, max: usize| -> Result<Vec<u8>, String> {
        let got = f
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| format!("offline: {url}"))?;
        if got.len() > max {
            return Err("too big".into());
        }
        Ok(got)
    }
}

fn serve(files: &Files, (bytes, sig): (Vec<u8>, String)) {
    *files.lock().unwrap() = vec![
        (URL.to_string(), bytes),
        (format!("{URL}.minisig"), sig.into_bytes()),
    ];
}

fn id(r: Result<impl std::fmt::Debug, Failure>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => f.id().map_or_else(|| format!("source: {f}"), String::from),
    }
}

#[test]
fn an_accepted_catalog_is_saved_and_loads_at_the_next_open() {
    let home = home("saved");
    let (key, secret) = keys();
    user_config(&home, &key);
    let files = Files::default();
    serve(&files, catalog(&secret, 4, true));

    let mut reg = Registry::open(&home, false).unwrap();
    assert!(reg.trust.catalogs.is_empty());
    let st = reg.refresh_source("mine", &fetcher(&files), NOW).unwrap();
    assert_eq!((st.sequence, st.accepted.as_str()), (4, NOW));
    assert_eq!(reg.inventory.seen["test-user"].catalogs["mine"], 4);
    assert!(reg.trust.revocation(ID, "0.5.0").is_some());
    assert!(stored_path(&home, "mine").exists());

    // A new hub: the stored catalog is verified again and used.
    let reg = Registry::open(&home, false).unwrap();
    assert_eq!(reg.trust.catalogs.len(), 1, "{:?}", reg.catalog_report);
    assert!(reg.trust.revocation(ID, "0.5.0").is_some());
    assert!(
        reg.catalog_report[0].contains("sequence 4 (accepted 2026-11-10T12:00:00Z) loaded"),
        "{:?}",
        reg.catalog_report
    );
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn a_rollback_a_bad_signature_and_offline_are_refused_and_keep_the_stored_one() {
    let home = home("refused");
    let (key, secret) = keys();
    let (_, rogue) = keys();
    user_config(&home, &key);
    let files = Files::default();
    let mut reg = Registry::open(&home, false).unwrap();
    serve(&files, catalog(&secret, 5, true));
    reg.refresh_source("mine", &fetcher(&files), NOW).unwrap();

    serve(&files, catalog(&secret, 4, false));
    assert_eq!(
        id(reg.refresh_source("mine", &fetcher(&files), NOW)),
        "trust.rollback"
    );
    serve(&files, catalog(&rogue, 6, false));
    assert_eq!(
        id(reg.refresh_source("mine", &fetcher(&files), NOW)),
        "trust.signature"
    );
    files.lock().unwrap().clear();
    let offline = id(reg.refresh_source("mine", &fetcher(&files), NOW));
    assert!(
        offline.starts_with("source: ") && offline.contains("offline"),
        "{offline}"
    );

    // The same sequence again is fine (a second fetch of an unchanged catalog).
    serve(&files, catalog(&secret, 5, true));
    assert!(reg.refresh_source("mine", &fetcher(&files), NOW).is_ok());
    let st = read_stored(&home, "mine").unwrap().unwrap();
    assert_eq!(st.sequence, 5);
    assert!(reg.trust.revocation(ID, "0.5.0").is_some());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn an_older_stored_catalog_is_a_rollback_at_open() {
    let home = home("old-file");
    let (key, secret) = keys();
    user_config(&home, &key);
    let files = Files::default();
    let mut reg = Registry::open(&home, false).unwrap();
    serve(&files, catalog(&secret, 2, true));
    reg.refresh_source("mine", &fetcher(&files), NOW).unwrap();
    let old = std::fs::read(stored_path(&home, "mine")).unwrap();
    serve(&files, catalog(&secret, 3, true));
    reg.refresh_source("mine", &fetcher(&files), NOW).unwrap();

    // Someone puts the sequence-2 file back: seen says 3, so it isn't used.
    std::fs::write(stored_path(&home, "mine"), old).unwrap();
    let reg = Registry::open(&home, false).unwrap();
    assert!(reg.trust.catalogs.is_empty());
    assert!(
        reg.catalog_report[0].contains("not used") && reg.catalog_report[0].contains("below seen"),
        "{:?}",
        reg.catalog_report
    );
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn no_source_or_no_url_fetches_nothing() {
    let home = home("none");
    let mut reg = Registry::open(&home, false).unwrap();
    let asked = Arc::new(Mutex::new(0));
    let a = asked.clone();
    let f = move |_: &str, _: usize| -> Result<Vec<u8>, String> {
        *a.lock().unwrap() += 1;
        Err("unused".into())
    };
    assert!(id(reg.refresh_source("mine", &f, NOW)).starts_with("source: no source mine"));
    // A plain-http url is not fetched (§13.4: catalog sources are https).
    let (key, _) = keys();
    user_config(&home, &key);
    let text = std::fs::read_to_string(home.join("hideout.toml"))
        .unwrap()
        .replace("https://", "http://");
    std::fs::write(home.join("hideout.toml"), text).unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    assert!(id(reg.refresh_source("mine", &f, NOW)).contains("not https"));
    assert_eq!(*asked.lock().unwrap(), 0);
    assert!(!home.join("catalogs").exists());
    let _ = std::fs::remove_dir_all(home);
}

fn vectors() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors/trust")
}

#[test]
fn a_root_anchored_source_fetches_the_root_beside_it_and_needs_a_pinned_key() {
    let home = home("rooted");
    std::fs::write(
        home.join("hideout.toml"),
        format!(
            "[[sources]]\nid = \"public\"\nkind = \"catalog\"\nvisibility = \"public\"\n\
             url = \"{URL}\"\nanchor = \"{ROOT_ANCHOR}\"\n"
        ),
    )
    .unwrap();
    let v = |rel: &str| std::fs::read(vectors().join(rel)).unwrap();
    let cat = "catalogs/foxglove.with-revocations.catalog.json";
    let root = "root/root.valid.json";
    let files: Files = Arc::new(Mutex::new(vec![
        (URL.to_string(), v(cat)),
        (format!("{URL}.minisig"), v(&format!("{cat}.minisig"))),
        (
            "https://catalog.example.invalid/c/root.json".into(),
            v(root),
        ),
        (
            "https://catalog.example.invalid/c/root.json.minisig".into(),
            v(&format!("{root}.minisig")),
        ),
    ]));

    // This build pins no root key yet (#90): nothing is fetched.
    let mut reg = Registry::open(&home, false).unwrap();
    assert!(id(reg.refresh_source("public", &fetcher(&files), NOW)).contains("no pinned"));

    // With the TEST-ONLY vector root key: accepted, stored with its root, and loaded again.
    let key = String::from_utf8(v("keys/root.pub")).unwrap();
    let key = key.lines().nth(1).unwrap().trim().to_string();
    reg.root_key = Some(key.clone());
    reg.refresh_source("public", &fetcher(&files), NOW).unwrap();
    let st = read_stored(&home, "public").unwrap().unwrap();
    assert!(st.root.is_some() && st.root_signature.is_some());
    assert!(reg
        .trust
        .revocation("foxglove.example-art", "0.2.0")
        .is_some());

    let mut reg = Registry::open(&home, false).unwrap();
    assert!(reg.trust.catalogs.is_empty(), "loaded without a pinned key");
    reg.root_key = Some(key);
    reg.load_catalogs(NOW);
    assert_eq!(reg.trust.catalogs.len(), 1, "{:?}", reg.catalog_report);
    assert!(reg.trust.root.is_some());
    assert!(reg
        .trust
        .revocation("foxglove.example-art", "0.2.0")
        .is_some());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn a_user_catalog_epoch_never_excuses_a_lower_sequence() {
    let home = home("epoch");
    let (key, secret) = keys();
    user_config(&home, &key);
    let files = Files::default();
    let mut reg = Registry::open(&home, false).unwrap();
    serve(&files, catalog_at(&secret, 1, 1, false));
    reg.refresh_source("mine", &fetcher(&files), NOW).unwrap();
    let seq1 = std::fs::read(stored_path(&home, "mine")).unwrap();
    serve(&files, catalog_at(&secret, 1, 5, true));
    reg.refresh_source("mine", &fetcher(&files), NOW).unwrap();

    // Fetch: the old sequence 1 (epoch 1, no revocation) is refused, so the withdrawal stays.
    serve(&files, catalog_at(&secret, 1, 1, false));
    assert_eq!(
        id(reg.refresh_source("mine", &fetcher(&files), NOW)),
        "trust.rollback"
    );
    // Another epoch doesn't excuse it either.
    serve(&files, catalog_at(&secret, 2, 1, false));
    assert_eq!(
        id(reg.refresh_source("mine", &fetcher(&files), NOW)),
        "trust.rollback"
    );
    assert!(reg.trust.revocation(ID, "0.5.0").is_some());

    // Load: the stored sequence-1 file put back is refused at open.
    std::fs::write(stored_path(&home, "mine"), seq1).unwrap();
    let reg = Registry::open(&home, false).unwrap();
    assert!(reg.trust.catalogs.is_empty(), "{:?}", reg.catalog_report);
    assert!(reg.stored_catalog("mine").is_none());
    assert!(!reg.catalog_loaded("mine"));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn a_change_saved_while_downloading_survives_the_accept() {
    let home = home("concurrent");
    let (key, secret) = keys();
    user_config(&home, &key);
    let (bytes, sig) = catalog(&secret, 3, false);
    let h = home.clone();
    // During the download another writer raises another anchor's `seen` on disk.
    let f = move |url: &str, _: usize| -> Result<Vec<u8>, String> {
        if url == URL {
            hideout_registry::inventory::Inventory::update(&h.join("inventory.json"), |inv| {
                inv.seen.entry("other".into()).or_default().root_version = 7;
            })
            .map_err(|e| e.to_string())?;
            Ok(bytes.clone())
        } else {
            Ok(sig.clone().into_bytes())
        }
    };
    // Opened before the download: its copy of the inventory is older than the file.
    let mut reg = Registry::open(&home, false).unwrap();
    reg.refresh_source("mine", &f, NOW).unwrap();
    let disk = Registry::open(&home, false).unwrap().inventory;
    assert_eq!(disk.seen["other"].root_version, 7);
    assert_eq!(disk.seen["test-user"].catalogs["mine"], 3);
    let _ = std::fs::remove_dir_all(home);
}

/// A TEST-ONLY `root.json` (version `version`) naming one family `foxglove` with `family_key`.
fn root_doc(root_secret: &str, family_key: &str, version: u64) -> (Vec<u8>, String) {
    let doc = json!({
        "format": "hideout-root-v1", "version": version, "epoch": 0,
        "expires": "2099-01-01T00:00:00Z",
        "families": { "foxglove": { "keys": [family_key], "scope": ["foxglove.*"] } },
    });
    let bytes = serde_json::to_vec_pretty(&doc).unwrap();
    let sig = signed(root_secret, &bytes);
    (bytes, sig)
}

/// A `foxglove` family catalog with `seq`.
fn family_catalog(secret: &str, seq: u64) -> (Vec<u8>, String) {
    let doc = json!({
        "format": "hideout-catalog-v1", "family": "foxglove", "epoch": 0, "sequence": seq,
        "expires": "2099-01-01T00:00:00Z", "modules": [], "revocations": [],
    });
    let bytes = serde_json::to_vec_pretty(&doc).unwrap();
    let sig = signed(secret, &bytes);
    (bytes, sig)
}

#[test]
fn every_saved_catalog_keeps_the_root_it_was_verified_against() {
    let home = home("root-moves");
    let (root_key, root_secret) = keys();
    let (fam_key, fam_secret) = keys();
    let second = "https://catalog.example.invalid/d/catalog.json";
    std::fs::write(
        home.join("hideout.toml"),
        format!(
            "[[sources]]\nid = \"public\"\nkind = \"catalog\"\nvisibility = \"public\"\n\
             url = \"{URL}\"\nanchor = \"{ROOT_ANCHOR}\"\n\n\
             [[sources]]\nid = \"second\"\nkind = \"catalog\"\nvisibility = \"public\"\n\
             url = \"{second}\"\nanchor = \"{ROOT_ANCHOR}\"\n"
        ),
    )
    .unwrap();
    let root_url = |cat: &str| cat.replace("catalog.json", "root.json");
    let serve_at = |files: &Files, cat: &str, c: (Vec<u8>, String), r: (Vec<u8>, String)| {
        let mut f = files.lock().unwrap();
        f.retain(|(u, _)| !u.starts_with(&cat.replace("catalog.json", "")));
        f.push((cat.to_string(), c.0));
        f.push((format!("{cat}.minisig"), c.1.into_bytes()));
        f.push((root_url(cat), r.0));
        f.push((format!("{}.minisig", root_url(cat)), r.1.into_bytes()));
    };
    let files = Files::default();
    let v1 = root_doc(&root_secret, &fam_key, 1);
    serve_at(&files, URL, family_catalog(&fam_secret, 1), v1.clone());
    serve_at(&files, second, family_catalog(&fam_secret, 1), v1.clone());

    let mut reg = Registry::open(&home, false).unwrap();
    reg.root_key = Some(root_key.clone());
    reg.refresh_source("public", &fetcher(&files), NOW).unwrap();
    reg.refresh_source("second", &fetcher(&files), NOW).unwrap();
    let v1_text = String::from_utf8(v1.0.clone()).unwrap();
    assert_eq!(
        read_stored(&home, "second").unwrap().unwrap().root,
        Some(v1_text)
    );

    // A new root (version 2) comes with public's catalog: second's saved catalog moves to it.
    let v2 = root_doc(&root_secret, &fam_key, 2);
    serve_at(&files, URL, family_catalog(&fam_secret, 2), v2.clone());
    reg.refresh_source("public", &fetcher(&files), NOW).unwrap();
    let v2_text = String::from_utf8(v2.0.clone()).unwrap();
    for s in ["public", "second"] {
        let st = read_stored(&home, s).unwrap().unwrap();
        assert_eq!(st.root.as_deref(), Some(v2_text.as_str()), "{s}");
        assert_eq!(st.root_signature.as_deref(), Some(v2.1.as_str()), "{s}");
    }
    assert_eq!(reg.trust.catalogs.len(), 2);
    assert!(reg.stored_catalog("second").is_some());

    // The old root beside second's catalog is now a rollback, so it isn't accepted.
    assert_eq!(
        id(reg.refresh_source("second", &fetcher(&files), NOW)),
        "trust.root-rollback"
    );
    // The same root again keeps the stored file the catalog was checked against.
    serve_at(&files, second, family_catalog(&fam_secret, 2), v2.clone());
    reg.refresh_source("second", &fetcher(&files), NOW).unwrap();
    let st = read_stored(&home, "second").unwrap().unwrap();
    assert_eq!(
        (st.sequence, st.root.as_deref()),
        (2, Some(v2_text.as_str()))
    );

    // The next open: both load against the raised `seen` (root version 2).
    let mut reg = Registry::open(&home, false).unwrap();
    reg.root_key = Some(root_key);
    reg.load_catalogs(NOW);
    assert_eq!(reg.trust.catalogs.len(), 2, "{:?}", reg.catalog_report);
    assert_eq!(reg.trust.root.as_ref().map(|r| r.version), Some(2));
    let _ = std::fs::remove_dir_all(home);
}
