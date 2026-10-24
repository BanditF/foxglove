//! Which plugins may install and run (#159): only versions in the signed catalog, with their
//! files unchanged. The root key is pinned in the app; `root.json` and `catalog.json` (with
//! their minisign signatures) ship with it in `resources/plugin-trust/`.
//!
//! Until the maintainer generates the keys (#90) those files are placeholders, so no plugin is
//! trusted. `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1` (or the older `FOXGLOVE_DEV_UNSIGNED_KITS=1`) lets a developer run unsigned plugins anyway; the Plugins tab
//! says so.

use std::path::Path;

use foxglove_plugin_trust::{Catalog, Entry};
use serde::{Deserialize, Serialize};

const ROOT_KEY: &str = include_str!("../../resources/plugin-trust/root.pub");
const ROOT_JSON: &[u8] = include_bytes!("../../resources/plugin-trust/root.json");
const ROOT_SIG: &str = include_str!("../../resources/plugin-trust/root.json.minisig");
const CATALOG_JSON: &[u8] = include_bytes!("../../resources/plugin-trust/catalog.json");
const CATALOG_SIG: &str = include_str!("../../resources/plugin-trust/catalog.json.minisig");

/// Files the runner writes into a plugin's folder itself.
pub const RUNTIME_FILES: &[&str] = &["plugin.log", "kit.log"];

/// Whether unsigned plugins are allowed (developers only).
pub fn dev_unsigned() -> bool {
    ["FOXGLOVE_DEV_UNSIGNED_PLUGINS", "FOXGLOVE_DEV_UNSIGNED_KITS"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| v == "1"))
}

/// The highest catalog sequence this install has accepted, against rollback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Seen {
    sequence: u64,
}

/// The signed catalog, verified through the pinned root key.
pub fn catalog() -> Result<Catalog, String> {
    if ROOT_KEY.starts_with("PLACEHOLDER") || ROOT_JSON.is_empty() || CATALOG_JSON.is_empty() {
        return Err("this build has no signed plugin catalog yet".into());
    }
    let now = foxglove_plugin_trust::now_utc();
    let root = foxglove_plugin_trust::verify_root(ROOT_JSON, ROOT_SIG, ROOT_KEY, &now)?;
    let seen = super::store::try_load::<Seen>("plugins.trust").unwrap_or_default();
    let catalog = foxglove_plugin_trust::verify_catalog(CATALOG_JSON, CATALOG_SIG, &root, &now, seen.sequence)?;
    if catalog.sequence > seen.sequence {
        let _ = super::store::save("plugins.trust", &Seen { sequence: catalog.sequence });
    }
    Ok(catalog)
}

/// The signed entry for an archive about to be installed, by its hash.
pub fn entry_for_archive(archive: &[u8]) -> Result<Entry, String> {
    let hash = foxglove_plugin_trust::sha256_hex(archive);
    catalog()?
        .plugins
        .into_iter()
        .find(|e| e.sha256.eq_ignore_ascii_case(&hash))
        .ok_or_else(|| "this archive isn't a signed Foxglove plugin".to_string())
}

/// Whether an installed plugin may run now: its version is still in the signed catalog (so a
/// revoked version stops) and its folder holds exactly the signed files.
pub fn check_installed(id: &str, version: &str, dir: &Path) -> Result<(), String> {
    let catalog = catalog()?;
    let entry = catalog
        .plugins
        .iter()
        .find(|e| e.id == id && e.version == version)
        .ok_or_else(|| format!("{id} {version} isn't in the signed catalog"))?;
    foxglove_plugin_trust::check_files(dir, entry, RUNTIME_FILES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_build_trusts_nothing() {
        assert!(catalog().unwrap_err().contains("no signed plugin catalog"));
        assert!(entry_for_archive(b"zip").is_err());
    }
}
