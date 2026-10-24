//! Which Kits may install and run (#159): only versions in the signed catalog, with their
//! files unchanged. The root key is pinned in the app; `root.json` and `catalog.json` (with
//! their minisign signatures) ship with it in `resources/kit-trust/`.
//!
//! Until the maintainer generates the keys (#90) those files are placeholders, so no Kit is
//! trusted. `FOXGLOVE_DEV_UNSIGNED_KITS=1` lets a developer run unsigned Kits anyway; the Den
//! says so.

use std::path::Path;

use kit_trust::{Catalog, Entry};
use serde::{Deserialize, Serialize};

const ROOT_KEY: &str = include_str!("../../resources/kit-trust/root.pub");
const ROOT_JSON: &[u8] = include_bytes!("../../resources/kit-trust/root.json");
const ROOT_SIG: &str = include_str!("../../resources/kit-trust/root.json.minisig");
const CATALOG_JSON: &[u8] = include_bytes!("../../resources/kit-trust/catalog.json");
const CATALOG_SIG: &str = include_str!("../../resources/kit-trust/catalog.json.minisig");

/// Files the runner writes into a Kit's folder itself.
pub const RUNTIME_FILES: &[&str] = &["kit.log"];

/// Whether unsigned Kits are allowed (developers only).
pub fn dev_unsigned() -> bool {
    std::env::var_os("FOXGLOVE_DEV_UNSIGNED_KITS").is_some_and(|v| v == "1")
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
        return Err("this build has no signed Kit catalog yet".into());
    }
    let now = kit_trust::now_utc();
    let root = kit_trust::verify_root(ROOT_JSON, ROOT_SIG, ROOT_KEY, &now)?;
    let seen = super::store::try_load::<Seen>("kits.trust").unwrap_or_default();
    let catalog = kit_trust::verify_catalog(CATALOG_JSON, CATALOG_SIG, &root, &now, seen.sequence)?;
    if catalog.sequence > seen.sequence {
        let _ = super::store::save("kits.trust", &Seen { sequence: catalog.sequence });
    }
    Ok(catalog)
}

/// The signed entry for an archive about to be installed, by its hash.
pub fn entry_for_archive(archive: &[u8]) -> Result<Entry, String> {
    let hash = kit_trust::sha256_hex(archive);
    catalog()?
        .kits
        .into_iter()
        .find(|e| e.sha256.eq_ignore_ascii_case(&hash))
        .ok_or_else(|| "this archive isn't a signed Foxglove Kit".to_string())
}

/// Whether an installed Kit may run now: its version is still in the signed catalog (so a
/// revoked version stops) and its folder holds exactly the signed files.
pub fn check_installed(id: &str, version: &str, dir: &Path) -> Result<(), String> {
    let catalog = catalog()?;
    let entry = catalog
        .kits
        .iter()
        .find(|e| e.id == id && e.version == version)
        .ok_or_else(|| format!("{id} {version} isn't in the signed catalog"))?;
    kit_trust::check_files(dir, entry, RUNTIME_FILES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_build_trusts_nothing() {
        assert!(catalog().unwrap_err().contains("no signed Kit catalog"));
        assert!(entry_for_archive(b"zip").is_err());
    }
}
