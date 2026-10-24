//! The Den (#69): which Kits are installed, the curated catalog, and installing and removing
//! them. Installed Kits live in `kits\<id>\` next to the settings store; what the user approved
//! for each is in the store's `[kits]` section.
//!
//! The catalog is pinned in the app (`resources/kit-catalog.json`). An entry can carry a
//! download URL and its SHA-256; nothing else is fetched. Running Kits comes with the Kit runner.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use foxglove_kit_api::manifest::Manifest;
use foxglove_kit_api::permission::Permission;
use foxglove_kit_api::API_VERSION;
use serde::{Deserialize, Serialize};

/// The most a Kit archive may unpack to.
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 2000;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    /// The vendor whose software the Kit works with, if any.
    #[serde(default)]
    pub vendor: Option<String>,
    /// Where to download it, with its SHA-256; absent while the Kit isn't released.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Deserialize)]
struct Catalog {
    format: String,
    version: u32,
    kits: Vec<CatalogEntry>,
}

fn parse_catalog(text: &str) -> Result<Vec<CatalogEntry>, String> {
    let c: Catalog = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if c.format != "foxglove-kit-catalog" || c.version != 1 {
        return Err("unknown catalog format".into());
    }
    Ok(c.kits)
}

/// The curated catalog shipped with this version of Foxglove.
pub fn catalog() -> Vec<CatalogEntry> {
    parse_catalog(include_str!("../../resources/kit-catalog.json")).unwrap_or_default()
}

/// What the user approved for one Kit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KitRecord {
    pub enabled: bool,
    pub approved: Vec<Permission>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Kits {
    installed: BTreeMap<String, KitRecord>,
}

pub fn kits_dir() -> Option<PathBuf> {
    Some(super::store::path()?.parent()?.join("kits"))
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub record: KitRecord,
}

/// The Kits in the kits folder whose manifest this app accepts, with their records.
pub fn installed() -> Vec<Installed> {
    let Some(dir) = kits_dir() else { return vec![] };
    let records = super::store::try_load::<Kits>("kits").unwrap_or_default().installed;
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut kits: Vec<Installed> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join("kit.toml")).ok()?;
            let manifest = Manifest::parse(&text, API_VERSION).ok()?;
            (e.file_name().to_string_lossy() == manifest.id).then(|| Installed {
                record: records.get(&manifest.id).cloned().unwrap_or_default(),
                manifest,
            })
        })
        .collect();
    kits.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    kits
}

/// A Kit unpacked next to the kits folder, waiting for the user's approval.
#[derive(Clone, Debug)]
pub struct Staged {
    pub manifest: Manifest,
    pub dir: PathBuf,
    /// Permissions that an installed version didn't have (all of them for a new Kit).
    pub new_permissions: Vec<Permission>,
}

/// Unpacks a Kit archive into a staging folder and checks its manifest. Nothing is installed
/// until [`commit`].
pub fn stage(archive: &Path) -> Result<Staged, String> {
    let kits = kits_dir().ok_or("no settings folder")?;
    let staging = kits.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    unpack(archive, &staging)?;
    let text = std::fs::read_to_string(staging.join("kit.toml")).map_err(|_| "the archive has no kit.toml at its top".to_string())?;
    let manifest = Manifest::parse(&text, API_VERSION)?;
    let previous = std::fs::read_to_string(kits.join(&manifest.id).join("kit.toml"))
        .ok()
        .and_then(|t| Manifest::parse(&t, API_VERSION).ok());
    let new_permissions = match &previous {
        Some(old) => manifest.added_permissions(old),
        None => manifest.permissions.clone(),
    };
    Ok(Staged { manifest, dir: staging, new_permissions })
}

/// Unzips `archive` into `into`, refusing paths that leave it and archives that are too big.
fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("not a Kit archive: {e}"))?;
    if zip.len() > MAX_FILES {
        return Err("the archive has too many files".into());
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name() else {
            return Err(format!("the archive has an unsafe path: {}", entry.name()));
        };
        let out = into.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut bytes = Vec::new();
        entry.by_ref().take(MAX_UNPACKED - total + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        if total > MAX_UNPACKED {
            return Err("the archive unpacks to more than 64 MiB".into());
        }
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Installs a staged Kit (replacing an older version) with the permissions the user approved.
pub fn commit(staged: Staged, enabled: bool) -> Result<(), String> {
    let kits = kits_dir().ok_or("no settings folder")?;
    let target = kits.join(&staged.manifest.id);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("removing the old version: {e}"))?;
    }
    std::fs::rename(&staged.dir, &target).map_err(|e| e.to_string())?;
    let mut all = super::store::try_load::<Kits>("kits").unwrap_or_default();
    all.installed.insert(staged.manifest.id.clone(), KitRecord { enabled, approved: staged.manifest.permissions.clone() });
    super::store::save("kits", &all).map_err(|e| e.to_string())
}

/// Drops a staged Kit the user didn't approve.
pub fn discard(staged: Staged) {
    let _ = std::fs::remove_dir_all(staged.dir);
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let mut all = super::store::try_load::<Kits>("kits").unwrap_or_default();
    all.installed.entry(id.to_string()).or_default().enabled = enabled;
    super::store::save("kits", &all).map_err(|e| e.to_string())
}

/// Uninstalls a Kit: its folder and its record.
pub fn remove(id: &str) -> Result<(), String> {
    let kits = kits_dir().ok_or("no settings folder")?;
    let dir = kits.join(id);
    if dir.parent() != Some(kits.as_path()) {
        return Err("bad Kit id".into());
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    let mut all = super::store::try_load::<Kits>("kits").unwrap_or_default();
    all.installed.remove(id);
    super::store::save("kits", &all).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn shipped_catalog_parses_with_valid_permissions() {
        let kits = parse_catalog(include_str!("../../resources/kit-catalog.json")).unwrap();
        assert!(kits.iter().all(|k| k.id.contains('.') && !k.permissions.is_empty()));
        assert!(kits.iter().all(|k| k.url.is_none() || k.sha256.is_some()), "downloads are pinned");
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("foxglove-kit-{}-{n}.zip", std::process::id()));
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, bytes) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
        path
    }

    #[test]
    fn unpacks_safely() {
        let dir = std::env::temp_dir().join(format!("foxglove-kit-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let good = zip_with(&[("kit.toml", b"id = 1"), ("bin/run.exe", b"x")]);
        unpack(&good, &dir).unwrap();
        assert!(dir.join("bin/run.exe").exists());
        let evil = zip_with(&[("../escape.txt", b"x"), ("a", b""), ("b", b"")]);
        assert!(unpack(&evil, &dir.join("evil")).unwrap_err().contains("unsafe path"));
        assert!(!dir.join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(good);
        let _ = std::fs::remove_file(evil);
    }

    /// The whole install flow in a temp settings folder (FOXGLOVE_CONFIG_DIR), never the real one.
    #[test]
    fn stage_commit_list_and_remove() {
        let root = std::env::temp_dir().join(format!("foxglove-den-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("FOXGLOVE_CONFIG_DIR", root.join("config"));
        let manifest = "id = \"com.example.weather\"
name = \"Weather\"
version = \"1.0.0\"
api = \"1.0\"
run = [\"weather.exe\"]
permissions = [\"facts.write:weather.*\", \"key.events\"]
";
        let archive = zip_with(&[("kit.toml", manifest.as_bytes()), ("weather.exe", b"MZ")]);
        let staged = stage(&archive).unwrap();
        assert_eq!(staged.new_permissions.len(), 2);
        assert!(staged.new_permissions.iter().any(Permission::sensitive));
        commit(staged, true).unwrap();
        let kits = installed();
        assert_eq!(kits.len(), 1);
        assert!(kits[0].record.enabled && kits[0].record.approved.len() == 2);
        assert!(root.join("kits/com.example.weather/weather.exe").exists());
        let again = stage(&archive).unwrap();
        assert!(again.new_permissions.is_empty(), "same version asks for nothing new");
        discard(again);
        remove("com.example.weather").unwrap();
        assert!(installed().is_empty());
        assert!(remove("../config").is_err(), "ids can't leave the kits folder");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(archive);
    }
}
