// SPDX-License-Identifier: MIT
//! A catalog entry's `min_hub_release` (contract §12.4, v1.11): a signed catalog carrying it
//! verifies, a field that can't be read leaves the catalog accepted, and the registry names the
//! release of an installed module from its own source only. Window-free; keys made per run.

use foxglove_plugin_trust::sign;
use hideout_contract::module::Visibility;
use hideout_contract::release::{update_url, RELEASES_URL};
use hideout_registry::trust::{verify_catalog, Accepted, Signer};
use hideout_registry::Registry;
use serde_json::json;

const NOW: &str = "2026-11-10T12:00:00Z";
const TAG: &str = "https://github.com/BanditF/foxglove/releases/tag/hideout-v0.4.0";

fn signed_catalog(modules: serde_json::Value) -> (Vec<u8>, String, String) {
    let k = sign::keygen(Some(String::new())).unwrap();
    let doc = json!({
        "format": "hideout-catalog-v1", "epoch": 0, "sequence": 1,
        "expires": "2099-01-01T00:00:00Z", "modules": modules, "revocations": [],
    });
    let bytes = serde_json::to_vec_pretty(&doc).unwrap();
    let sig = sign::sign(
        &sign::load_secret(&k.secret, Some(String::new())).unwrap(),
        &bytes,
    )
    .unwrap();
    (bytes, sig, k.public_base64)
}

fn entry(id: &str, version: &str, release: serde_json::Value) -> serde_json::Value {
    let mut e = json!({ "id": id, "version": version, "sha256": "00", "files": {} });
    if !release.is_null() {
        e["min_hub_release"] = release;
    }
    e
}

/// Verifies `modules` in a catalog signed by a user key for `example.test.*`.
fn accepted(source: &str, modules: serde_json::Value) -> Accepted {
    let (bytes, sig, key) = signed_catalog(modules);
    let scope = vec!["example.test.*".to_string()];
    let catalog = verify_catalog(
        &bytes,
        &sig,
        Signer::User {
            key: &key,
            scope: &scope,
        },
        source,
        NOW,
        None,
    )
    .expect("the catalog verifies");
    Accepted {
        anchor: "test-user".into(),
        source: source.into(),
        visibility: Visibility::Private,
        catalog,
    }
}

#[test]
fn a_catalog_with_the_field_verifies_and_a_bad_one_is_ignored() {
    let a = accepted(
        "mine",
        json!([
            entry(
                "example.test.art",
                "0.5.0",
                json!({"version": "0.4.0", "url": TAG})
            ),
            entry(
                "example.test.off",
                "1.0.0",
                json!({"version": "0.4.0", "url": "https://example.invalid/x"})
            ),
            entry("example.test.bad", "1.0.0", json!("0.4.0")),
            entry("example.test.none", "1.0.0", json!(null)),
        ]),
    );
    let link = |id: &str| {
        let e = a.catalog.modules.iter().find(|e| e.id == id).unwrap();
        update_url(e.min_hub_release.as_ref()).to_string()
    };
    assert_eq!(link("example.test.art"), TAG);
    assert_eq!(link("example.test.off"), RELEASES_URL);
    assert_eq!(link("example.test.bad"), RELEASES_URL);
    assert_eq!(link("example.test.none"), RELEASES_URL);
    let art = &a.catalog.modules[0];
    assert_eq!(art.min_hub_release.as_ref().unwrap().version, "0.4.0");
}

#[test]
fn the_registry_names_the_release_from_the_modules_own_source_and_version() {
    let home = std::env::temp_dir()
        .join("hideout-registry-hub-release")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    let other_tag = "https://github.com/BanditF/foxglove/releases/tag/hideout-v9.0.0";
    // Another source lists the same id and version with another release: never used.
    reg.trust.catalogs.push(accepted(
        "other",
        json!([entry(
            "example.test.art",
            "0.5.0",
            json!({"version": "9.0.0", "url": other_tag})
        )]),
    ));
    reg.trust.catalogs.push(accepted(
        "mine",
        json!([
            entry("example.test.art", "0.4.0", json!(null)),
            entry(
                "example.test.art",
                "0.5.0",
                json!({"version": "0.4.0", "url": TAG})
            ),
        ]),
    ));
    let installed = |version: &str, source: &str| {
        serde_json::from_value(json!({
            "version": version, "kind": "service", "family": "third-party",
            "visibility": "private", "source": source, "installed": NOW,
        }))
        .unwrap()
    };
    let url = |reg: &Registry| update_url(reg.hub_release("example.test.art")).to_string();
    assert_eq!(url(&reg), RELEASES_URL, "not installed: the releases page");
    reg.inventory
        .modules
        .insert("example.test.art".into(), installed("0.5.0", "mine"));
    assert_eq!(url(&reg), TAG);
    // The installed version's own entry has no field: the releases page.
    reg.inventory
        .modules
        .insert("example.test.art".into(), installed("0.4.0", "mine"));
    assert_eq!(url(&reg), RELEASES_URL);
    // A sideload has no catalog of its own.
    reg.inventory.modules.insert(
        "example.test.art".into(),
        installed("0.5.0", hideout_registry::inventory::SIDELOAD),
    );
    assert_eq!(url(&reg), RELEASES_URL);
    let _ = std::fs::remove_dir_all(&home);
}
