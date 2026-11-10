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
    let revocations = if revoke {
        json!([{ "id": ID, "versions": ["0.5.0"], "action": "revoke", "reason": "broken" }])
    } else {
        json!([])
    };
    let doc = json!({
        "format": "hideout-catalog-v1", "epoch": 0, "sequence": seq,
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
    let st = reg.refresh_source("public", &fetcher(&files), NOW).unwrap();
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
