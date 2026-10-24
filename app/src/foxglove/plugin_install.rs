//! The Plugins tab (#69): which plugins are installed, the curated catalog, and installing and removing
//! them. Installed plugins live in `plugins\<id>\` next to the settings store; what the user approved
//! for each is in the store's `[plugins]` section.
//!
//! The catalog is pinned in the app (`resources/plugin-catalog.json`). An entry can carry a
//! download URL and its SHA-256; nothing else is fetched. Running plugins comes with the plugin runner.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use foxglove_plugin_api::manifest::{self, Manifest};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::API_VERSION;
use serde::{Deserialize, Serialize};

/// The most a plugin archive may unpack to.
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 2000;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    /// The vendor whose software the plugin works with, if any.
    #[serde(default)]
    pub vendor: Option<String>,
    /// Where to download it, with its SHA-256; absent while the plugin isn't released.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Deserialize)]
struct Catalog {
    format: String,
    version: u32,
    #[serde(alias = "kits")]
    plugins: Vec<CatalogEntry>,
}

fn parse_catalog(text: &str) -> Result<Vec<CatalogEntry>, String> {
    let c: Catalog = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !matches!(c.format.as_str(), "foxglove-plugin-catalog" | "foxglove-kit-catalog") || c.version != 1 {
        return Err("unknown catalog format".into());
    }
    Ok(c.plugins)
}

/// The curated catalog shipped with this version of Foxglove.
pub fn catalog() -> Vec<CatalogEntry> {
    parse_catalog(include_str!("../../resources/plugin-catalog.json")).unwrap_or_default()
}

/// What the user approved for one plugin.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginRecord {
    pub enabled: bool,
    pub approved: Vec<Permission>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Plugins {
    installed: BTreeMap<String, PluginRecord>,
}

/// `plugins\` next to the settings store; a `kits\` folder from before #87 is renamed to it.
pub fn plugins_dir() -> Option<PathBuf> {
    let base = super::store::path()?.parent()?.to_path_buf();
    let dir = base.join("plugins");
    let old = base.join("kits");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    Some(dir)
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub record: PluginRecord,
}

/// The plugins in the plugins folder whose manifest this app accepts, with their records.
pub fn installed() -> Vec<Installed> {
    let Some(dir) = plugins_dir() else { return vec![] };
    let records = super::store::try_load::<Plugins>("plugins").unwrap_or_default().installed;
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut plugins: Vec<Installed> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let text = manifest::read_in(&e.path()).ok()?;
            let manifest = Manifest::parse(&text, API_VERSION).ok()?;
            (e.file_name().to_string_lossy() == manifest.id).then(|| Installed {
                record: records.get(&manifest.id).cloned().unwrap_or_default(),
                manifest,
            })
        })
        .collect();
    plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    plugins
}

/// A Plugin unpacked next to the plugins folder, waiting for the user's approval.
#[derive(Clone, Debug)]
pub struct Staged {
    pub manifest: Manifest,
    pub dir: PathBuf,
    /// Permissions that an installed version didn't have (all of them for a new plugin).
    pub new_permissions: Vec<Permission>,
}

/// Unpacks a plugin archive into a staging folder and checks its manifest. Nothing is installed
/// until [`commit`].
pub fn stage(archive: &Path) -> Result<Staged, String> {
    let plugins = plugins_dir().ok_or("no settings folder")?;
    let staging = plugins.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    // Only signed plugins: the archive's hash must be in the signed catalog (#159).
    let bytes = std::fs::read(archive).map_err(|e| e.to_string())?;
    let signed = match super::plugin_trust::entry_for_archive(&bytes) {
        Ok(entry) => Some(entry),
        Err(_) if super::plugin_trust::dev_unsigned() => None,
        Err(e) => return Err(e),
    };
    unpack(archive, &staging)?;
    let text = manifest::read_in(&staging).map_err(|_| "the archive has no plugin.toml at its top".to_string())?;
    let manifest = Manifest::parse(&text, API_VERSION)?;
    if let Some(entry) = &signed {
        let checked = if (entry.id.as_str(), entry.version.as_str()) != (manifest.id.as_str(), manifest.version.as_str()) {
            Err(format!("the archive says {} {}, the catalog {} {}", manifest.id, manifest.version, entry.id, entry.version))
        } else {
            foxglove_plugin_trust::check_files(&staging, entry, super::plugin_trust::RUNTIME_FILES)
        };
        if let Err(e) = checked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    let previous = manifest::read_in(&plugins.join(&manifest.id))
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
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("not a plugin archive: {e}"))?;
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

/// Installs a staged plugin (replacing an older version) with the permissions the user approved.
pub fn commit(staged: Staged, enabled: bool) -> Result<(), String> {
    let plugins = plugins_dir().ok_or("no settings folder")?;
    let target = plugins.join(&staged.manifest.id);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("removing the old version: {e}"))?;
    }
    std::fs::rename(&staged.dir, &target).map_err(|e| e.to_string())?;
    let mut all = super::store::try_load::<Plugins>("plugins").unwrap_or_default();
    all.installed.insert(staged.manifest.id.clone(), PluginRecord { enabled, approved: staged.manifest.permissions.clone() });
    super::store::save("plugins", &all).map_err(|e| e.to_string())
}

/// Test hook for the Test lab: `FOXGLOVE_PLUGINS_AUTOINSTALL=<zip or folder>` installs and
/// enables that plugin at start with every permission its manifest declares, without asking.
/// Honoured only with developer mode for unsigned plugins and a `FOXGLOVE_CONFIG_DIR` that
/// isn't the real settings folder, so it can never reach a user's install. What happened goes
/// to stderr and `autoinstall.log` in the config dir.
pub fn autoinstall() {
    let Some(source) = std::env::var_os("FOXGLOVE_PLUGINS_AUTOINSTALL").filter(|v| !v.is_empty()) else {
        return;
    };
    let config_dir = std::env::var_os("FOXGLOVE_CONFIG_DIR").map(PathBuf::from);
    let real = directories::ProjectDirs::from("", "", "Foxglove").map(|d| d.config_dir().to_path_buf());
    let result = autoinstall_allowed(super::plugin_trust::dev_unsigned(), config_dir.as_deref(), real.as_deref())
        .and_then(|()| autoinstall_from(Path::new(&source)));
    let line = match &result {
        Ok(id) => format!(
            "WARNING: FOXGLOVE_PLUGINS_AUTOINSTALL installed {id} with all its declared permissions, unsigned and unasked (test hook)"
        ),
        Err(e) => format!("FOXGLOVE_PLUGINS_AUTOINSTALL ignored: {e}"),
    };
    eprintln!("foxglove: {line}");
    if let Some(dir) = config_dir {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("autoinstall.log")) {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// The hook's guard: developer mode, and a config dir that's set and not the real one.
fn autoinstall_allowed(dev_unsigned: bool, config_dir: Option<&Path>, real: Option<&Path>) -> Result<(), String> {
    if !dev_unsigned {
        return Err("needs FOXGLOVE_DEV_UNSIGNED_PLUGINS=1".into());
    }
    let dir = config_dir.ok_or("needs FOXGLOVE_CONFIG_DIR")?;
    let same = |a: &Path, b: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(a) == canon(b) || a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    };
    if real.is_some_and(|real| same(dir, real) || real.parent().is_some_and(|p| same(dir, p))) {
        return Err("FOXGLOVE_CONFIG_DIR is the real settings folder".into());
    }
    Ok(())
}

fn autoinstall_from(source: &Path) -> Result<String, String> {
    let zipped;
    let archive = if source.is_dir() {
        zipped = zip_dir(source)?;
        zipped.as_path()
    } else {
        source
    };
    let staged = stage(archive);
    if source.is_dir() {
        let _ = std::fs::remove_file(archive);
    }
    let staged = staged?;
    let id = staged.manifest.id.clone();
    commit(staged, true)?;
    Ok(id)
}

/// A folder as a temporary plugin archive (its files at the archive's top).
fn zip_dir(dir: &Path) -> Result<PathBuf, String> {
    fn add(z: &mut zip::ZipWriter<std::fs::File>, root: &Path, dir: &Path) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                add(z, root, &path)?;
            } else {
                let name = path.strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
                z.start_file(name, zip::write::SimpleFileOptions::default()).map_err(|e| e.to_string())?;
                std::io::copy(&mut std::fs::File::open(&path).map_err(|e| e.to_string())?, z).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    let out = std::env::temp_dir().join(format!("foxglove-autoinstall-{}.zip", std::process::id()));
    let mut z = zip::ZipWriter::new(std::fs::File::create(&out).map_err(|e| e.to_string())?);
    add(&mut z, dir, dir)?;
    z.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Drops a staged plugin the user didn't approve.
pub fn discard(staged: Staged) {
    let _ = std::fs::remove_dir_all(staged.dir);
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let mut all = super::store::try_load::<Plugins>("plugins").unwrap_or_default();
    all.installed.entry(id.to_string()).or_default().enabled = enabled;
    super::store::save("plugins", &all).map_err(|e| e.to_string())
}

/// Uninstalls a plugin: its folder and its record.
pub fn remove(id: &str) -> Result<(), String> {
    let plugins = plugins_dir().ok_or("no settings folder")?;
    let dir = plugins.join(id);
    if dir.parent() != Some(plugins.as_path()) {
        return Err("bad plugin id".into());
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    ::foxglove_plugin_host::delete_container(id);
    let mut all = super::store::try_load::<Plugins>("plugins").unwrap_or_default();
    all.installed.remove(id);
    super::store::save("plugins", &all).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn catalog_from_before_the_rename_still_parses() {
        let old = r#"{"format":"foxglove-kit-catalog","version":1,"kits":[{"id":"com.example.a","name":"A","description":"","permissions":[]}]}"#;
        assert_eq!(parse_catalog(old).map(|c| c.len()), Ok(1));
    }

    #[test]
    fn shipped_catalog_parses_with_valid_permissions() {
        let plugins = parse_catalog(include_str!("../../resources/plugin-catalog.json")).unwrap();
        assert!(plugins.iter().all(|k| k.id.contains('.') && !k.permissions.is_empty()));
        assert!(plugins.iter().all(|k| k.url.is_none() || k.sha256.is_some()), "downloads are pinned");
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("foxglove-plugin-{}-{n}.zip", std::process::id()));
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, bytes) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
        path
    }

    #[test]
    fn autoinstall_only_in_dev_mode_and_a_test_config_dir() {
        let real = Path::new(r"C:\Users\u\AppData\Roaming\Foxglove\config");
        let test = std::env::temp_dir().join("foxglove-lab-1");
        assert!(autoinstall_allowed(true, Some(&test), Some(real)).is_ok());
        assert!(autoinstall_allowed(false, Some(&test), Some(real)).is_err(), "needs developer mode");
        assert!(autoinstall_allowed(true, None, Some(real)).is_err(), "needs a config dir");
        assert!(autoinstall_allowed(true, Some(real), Some(real)).is_err(), "never the real folder");
        let upper = Path::new(r"c:\users\u\appdata\roaming\foxglove");
        assert!(autoinstall_allowed(true, Some(upper), Some(real)).is_err(), "nor the folder above it");
    }

    #[test]
    fn a_folder_zips_with_its_files_at_the_top() {
        let dir = std::env::temp_dir().join(format!("foxglove-zipdir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("plugin.toml"), "id").unwrap();
        std::fs::write(dir.join("bin/run.exe"), "MZ").unwrap();
        let archive = zip_dir(&dir).unwrap();
        let out = dir.with_extension("out");
        unpack(&archive, &out).unwrap();
        assert!(out.join("plugin.toml").is_file() && out.join("bin/run.exe").is_file());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_file(archive);
    }

    #[test]
    fn unpacks_safely() {
        let dir = std::env::temp_dir().join(format!("foxglove-plugin-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let good = zip_with(&[("plugin.toml", b"id = 1"), ("bin/run.exe", b"x")]);
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
        let root = std::env::temp_dir().join(format!("foxglove-plugin-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("FOXGLOVE_CONFIG_DIR", root.join("config"));
        let manifest = "id = \"com.example.weather\"
name = \"Weather\"
version = \"1.0.0\"
api = \"1.0\"
run = [\"weather.exe\"]
permissions = [\"facts.write:weather.*\", \"key.events\"]
";
        let archive = zip_with(&[("plugin.toml", manifest.as_bytes()), ("weather.exe", b"MZ")]);
        // Unsigned: refused, then allowed in developer mode for the rest of the flow.
        assert!(stage(&archive).unwrap_err().contains("signed"));
        std::env::set_var("FOXGLOVE_DEV_UNSIGNED_PLUGINS", "1");
        let staged = stage(&archive).unwrap();
        assert_eq!(staged.new_permissions.len(), 2);
        assert!(staged.new_permissions.iter().any(Permission::sensitive));
        commit(staged, true).unwrap();
        let plugins = installed();
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].record.enabled && plugins[0].record.approved.len() == 2);
        assert!(root.join("plugins/com.example.weather/weather.exe").exists());
        let again = stage(&archive).unwrap();
        assert!(again.new_permissions.is_empty(), "same version asks for nothing new");
        discard(again);
        remove("com.example.weather").unwrap();
        assert!(installed().is_empty());
        assert!(remove("../config").is_err(), "ids can't leave the plugins folder");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(archive);
    }
}
