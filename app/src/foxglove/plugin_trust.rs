//! Which plugins may install and run (#159, #192): only versions in the signed catalog, not
//! withdrawn, with their files unchanged. The root key is pinned in the app.
//!
//! - **Sources:** the catalog built into the app (`resources/plugin-trust/`) and the last one
//!   downloaded ([`super::catalog_refresh`]), kept in `plugin-catalog\` next to the settings
//!   store. Whichever verifies and is newest (epoch, then sequence) wins.
//! - **Rollback:** the root version, epoch and sequence accepted before are kept in the store's
//!   `[plugins.trust]`. If they can't be saved, nothing is trusted: better no plugins than a
//!   catalog that could later be rolled back unnoticed.
//!
//! Until the maintainer generates the keys (#90) the built-in files are placeholders, so no
//! plugin is trusted. `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1` (or the older
//! `FOXGLOVE_DEV_UNSIGNED_KITS=1`) lets a developer run unsigned plugins anyway; the Plugins tab
//! says so.

use std::path::{Path, PathBuf};

use foxglove_plugin_trust::{Catalog, Entry, Root, Seen};

const ROOT_KEY: &str = include_str!("../../resources/plugin-trust/root.pub");

/// Files the runner writes into a plugin's folder itself.
pub const RUNTIME_FILES: &[&str] = &[
    "plugin.log",
    "kit.log",
    foxglove_plugin_host::runner::DATA_DIR,
];

/// Whether unsigned plugins are allowed (developers only).
pub fn dev_unsigned() -> bool {
    [
        "FOXGLOVE_DEV_UNSIGNED_PLUGINS",
        "FOXGLOVE_DEV_UNSIGNED_KITS",
    ]
    .iter()
    .any(|k| std::env::var_os(k).is_some_and(|v| v == "1"))
}

/// A root and catalog with their signatures, as published.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signed {
    pub root_json: Vec<u8>,
    pub root_sig: String,
    pub catalog_json: Vec<u8>,
    pub catalog_sig: String,
}

/// The published file names, in [`Signed`]'s order.
pub const FILES: [&str; 4] = [
    "root.json",
    "root.json.minisig",
    "catalog.json",
    "catalog.json.minisig",
];

impl Signed {
    pub fn from_files(files: [Vec<u8>; 4]) -> Signed {
        let [root_json, root_sig, catalog_json, catalog_sig] = files;
        Signed {
            root_json,
            root_sig: String::from_utf8_lossy(&root_sig).into_owned(),
            catalog_json,
            catalog_sig: String::from_utf8_lossy(&catalog_sig).into_owned(),
        }
    }

    fn files(&self) -> [&[u8]; 4] {
        [
            &self.root_json,
            self.root_sig.as_bytes(),
            &self.catalog_json,
            self.catalog_sig.as_bytes(),
        ]
    }
}

fn built_in() -> Signed {
    Signed {
        root_json: include_bytes!("../../resources/plugin-trust/root.json").to_vec(),
        root_sig: include_str!("../../resources/plugin-trust/root.json.minisig").into(),
        catalog_json: include_bytes!("../../resources/plugin-trust/catalog.json").to_vec(),
        catalog_sig: include_str!("../../resources/plugin-trust/catalog.json.minisig").into(),
    }
}

/// Where the downloaded catalog is kept.
pub fn cache_dir() -> Option<PathBuf> {
    Some(super::store::path()?.parent()?.join("plugin-catalog"))
}

fn cached() -> Option<Signed> {
    let dir = cache_dir()?;
    let read = |name: &str| std::fs::read(dir.join(name)).ok();
    Some(Signed::from_files([
        read(FILES[0])?,
        read(FILES[1])?,
        read(FILES[2])?,
        read(FILES[3])?,
    ]))
}

fn verify(set: &Signed, root_key: &str, now: &str, seen: &Seen) -> Result<(Root, Catalog), String> {
    if root_key.starts_with("PLACEHOLDER")
        || set.root_json.is_empty()
        || set.catalog_json.is_empty()
    {
        return Err("this build has no signed plugin catalog yet".into());
    }
    let root =
        foxglove_plugin_trust::verify_root(&set.root_json, &set.root_sig, root_key, now, seen)?;
    let catalog = foxglove_plugin_trust::verify_catalog(
        &set.catalog_json,
        &set.catalog_sig,
        &root,
        now,
        seen,
    )?;
    Ok((root, catalog))
}

/// The newest of the sets that verify, or the last error.
fn newest(
    sets: &[Signed],
    root_key: &str,
    now: &str,
    seen: &Seen,
) -> Result<(Root, Catalog), String> {
    let mut best: Option<(Root, Catalog)> = None;
    let mut error = "no plugin catalog".to_string();
    for set in sets {
        match verify(set, root_key, now, seen) {
            Ok(found) => {
                let key = |c: &Catalog| (c.epoch, c.sequence);
                if best.as_ref().is_none_or(|(_, b)| key(&found.1) > key(b)) {
                    best = Some(found);
                }
            }
            Err(e) => error = e,
        }
    }
    best.ok_or(error)
}

fn load_seen() -> Seen {
    super::store::try_load("plugins.trust").unwrap_or_default()
}

/// Records what was accepted; an error means it couldn't be, and the caller must not go on.
fn record(seen: &Seen, root: &Root, catalog: &Catalog) -> Result<(), String> {
    let next = seen.after(root, catalog);
    if next == *seen {
        return Ok(());
    }
    super::store::save("plugins.trust", &next).map_err(|e| {
        format!("can't record the plugin catalog's sequence, so no plugin is trusted: {e}")
    })
}

/// The signed catalog, verified through the pinned root key.
pub fn catalog() -> Result<Catalog, String> {
    let now = foxglove_plugin_trust::now_utc();
    let seen = load_seen();
    let sets: Vec<Signed> = cached().into_iter().chain([built_in()]).collect();
    let (root, catalog) = newest(&sets, ROOT_KEY, &now, &seen)?;
    record(&seen, &root, &catalog)?;
    Ok(catalog)
}

/// Checks a downloaded set and, if it verifies and is newer than what's cached, keeps it.
/// Returns the catalog's sequence.
pub fn accept_download(set: &Signed) -> Result<u64, String> {
    let now = foxglove_plugin_trust::now_utc();
    let seen = load_seen();
    let (root, catalog) = verify(set, ROOT_KEY, &now, &seen)?;
    if let Some((_, current)) = cached().and_then(|c| verify(&c, ROOT_KEY, &now, &seen).ok()) {
        if (catalog.epoch, catalog.sequence) < (current.epoch, current.sequence) {
            return Err(format!(
                "the server sent an older catalog (sequence {} < {})",
                catalog.sequence, current.sequence
            ));
        }
    }
    let dir = cache_dir().ok_or("no settings folder")?;
    write_all(&dir, set).map_err(|e| format!("can't keep the catalog: {e}"))?;
    record(&seen, &root, &catalog)?;
    Ok(catalog.sequence)
}

/// Writes the four files so that a reader never sees a mix: into a fresh folder, then swapped in.
fn write_all(dir: &Path, set: &Signed) -> std::io::Result<()> {
    let tmp = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for (name, bytes) in FILES.iter().zip(set.files()) {
        std::fs::write(tmp.join(name), bytes)?;
    }
    let old = dir.with_extension("old");
    let _ = std::fs::remove_dir_all(&old);
    if dir.exists() {
        std::fs::rename(dir, &old)?;
    }
    std::fs::rename(&tmp, dir)?;
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// The signed entry for an archive about to be installed, by its hash; refused if that version
/// was yanked or revoked.
pub fn entry_for_archive(archive: &[u8]) -> Result<Entry, String> {
    let hash = foxglove_plugin_trust::sha256_hex(archive);
    catalog()?.installable(&hash).cloned()
}

/// Whether an installed plugin may run now: its version is in the signed catalog and not
/// revoked, and its folder holds exactly the signed files.
pub fn check_installed(id: &str, version: &str, dir: &Path) -> Result<(), String> {
    let catalog = catalog()?;
    let entry = catalog.runnable(id, version)?;
    foxglove_plugin_trust::check_files(dir, entry, RUNTIME_FILES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_build_trusts_nothing() {
        assert!(catalog().unwrap_err().contains("no signed plugin catalog"));
        assert!(entry_for_archive(b"zip").is_err());
        assert!(accept_download(&Signed::default()).is_err());
    }

    #[test]
    fn cache_is_swapped_in_whole() {
        let dir =
            std::env::temp_dir().join(format!("foxglove-catalog-cache-{}", std::process::id()));
        let set = |n: u8| Signed::from_files([vec![n], vec![n], vec![n], vec![n]]);
        write_all(&dir, &set(1)).unwrap();
        write_all(&dir, &set(2)).unwrap();
        for name in FILES {
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), vec![2]);
        }
        assert!(!dir.with_extension("new").exists() && !dir.with_extension("old").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
