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
    if !matches!(
        c.format.as_str(),
        "foxglove-plugin-catalog" | "foxglove-kit-catalog"
    ) || c.version != 1
    {
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

/// The store's `[plugins.installed]`. Saved as its own section, so saving it leaves
/// `[plugins.trust]`, `[plugins.refresh]` and `[plugins.dev]` alone (saving all of `[plugins]`
/// used to drop them).
const RECORDS: &str = "plugins.installed";

fn records() -> BTreeMap<String, PluginRecord> {
    super::store::try_load(RECORDS).unwrap_or_default()
}

fn save_records(records: &BTreeMap<String, PluginRecord>) -> Result<(), String> {
    super::store::save(RECORDS, records).map_err(|e| e.to_string())
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
    let Some(dir) = plugins_dir() else {
        return vec![];
    };
    let records = records();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
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

/// Each installed plugin's trust by id (#381), against the signed catalog, read once.
pub fn trust_all(
    plugins: &[Installed],
) -> std::collections::HashMap<String, super::plugin_trust::Trust> {
    let Some(dir) = plugins_dir() else {
        return Default::default();
    };
    use super::plugin_trust as trust;
    let catalog = trust::catalog().ok();
    let g = grandfather(plugins);
    let key_missing = trust::signing_key_missing();
    plugins
        .iter()
        .map(|p| {
            let id = &p.manifest.id;
            let version = &p.manifest.version;
            let t = trust::trust_in(catalog.as_ref(), id, version, &dir.join(id));
            let t = trust::with_grandfather(t, id, version, &g, key_missing);
            (id.clone(), t)
        })
        .collect()
}

/// The plugins installed before the sideload policy (#381); the first call takes them from
/// `plugins`, which must be all the installed ones.
pub fn grandfather(plugins: &[Installed]) -> super::plugin_trust::Grandfather {
    let ids: Vec<(String, String)> = plugins
        .iter()
        .map(|p| (p.manifest.id.clone(), p.manifest.version.clone()))
        .collect();
    super::plugin_trust::grandfather(&ids)
}

/// A Plugin unpacked next to the plugins folder, waiting for the user's approval.
#[derive(Clone, Debug)]
pub struct Staged {
    pub manifest: Manifest,
    pub dir: PathBuf,
    /// Permissions that an installed version didn't have (all of them for a new plugin).
    pub new_permissions: Vec<Permission>,
    /// Not signed (#381): its package hash, which installing allows. `None` when signed.
    pub unsigned: Option<String>,
    /// The user allowed this exact unsigned build before, so it isn't asked again.
    pub allowed_before: bool,
    /// A warning when it takes a catalog plugin's id, or a signed plugin replaces an unsigned one.
    pub clash: Option<String>,
}

/// Why an unsigned archive doesn't install outside developer mode.
pub const UNSIGNED_REFUSED: &str =
    "Foxglove only installs signed plugins unless developer mode is on";

/// Unpacks a plugin archive into a staging folder and checks its manifest. Nothing is installed
/// until [`commit`].
pub fn stage(archive: &Path) -> Result<Staged, String> {
    stage_with(
        archive,
        &super::plugin_trust::catalog(),
        &super::plugin_trust::dev_mode(),
    )
}

/// [`stage`] against a given signed catalog and developer mode.
fn stage_with(
    archive: &Path,
    catalog: &Result<foxglove_plugin_trust::Catalog, String>,
    dev: &super::plugin_trust::DevMode,
) -> Result<Staged, String> {
    use super::plugin_trust::{self as trust, Trust};
    let plugins = plugins_dir().ok_or("no settings folder")?;
    let staging = plugins.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    // Signed plugins: the archive's hash is in the signed catalog (#159). Others only in
    // developer mode (#381); a withdrawn version never.
    let bytes = std::fs::read(archive).map_err(|e| e.to_string())?;
    let signed = match catalog {
        Ok(c) => trust::entry_in(c, &bytes)?,
        Err(_) => None,
    };
    if signed.is_none() && !dev.on {
        let why = match catalog {
            Ok(_) => "this archive isn't a signed Foxglove plugin",
            Err(e) => e.as_str(),
        };
        return Err(format!("{why}. {UNSIGNED_REFUSED}"));
    }
    unpack(archive, &staging)?;
    let text = manifest::read_in(&staging)
        .map_err(|_| "the archive has no plugin.toml at its top".to_string())?;
    let manifest = match Manifest::parse(&text, API_VERSION) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    if let Some(entry) = &signed {
        let checked = if (entry.id.as_str(), entry.version.as_str())
            != (manifest.id.as_str(), manifest.version.as_str())
        {
            Err(format!(
                "the archive says {} {}, the catalog {} {}",
                manifest.id, manifest.version, entry.id, entry.version
            ))
        } else {
            foxglove_plugin_trust::check_files(&staging, entry, super::plugin_trust::RUNTIME_FILES)
        };
        if let Err(e) = checked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    let unsigned = match signed {
        Some(_) => None,
        None => match trust::package_hash(&staging) {
            Ok(hash) => Some(hash),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        },
    };
    let previous = manifest::read_in(&plugins.join(&manifest.id))
        .ok()
        .and_then(|t| Manifest::parse(&t, API_VERSION).ok());
    let new_permissions = match &previous {
        Some(old) => manifest.added_permissions(old),
        None => manifest.permissions.clone(),
    };
    let clash = match (&unsigned, &previous) {
        // An unsigned file under a catalog plugin's id.
        (Some(_), _) => catalog_name(&manifest.id, catalog.as_ref().ok()).map(|name| {
            format!("Same id as {name} from the Foxglove catalog. This file takes its place, and the catalog won't update it.")
        }),
        // A signed plugin over an unsigned one installed from a file.
        (None, Some(old)) => {
            let dir = plugins.join(&old.id);
            let was = trust::trust_in(catalog.as_ref().ok(), &old.id, &old.version, &dir);
            matches!(was, Trust::Unsigned(_)).then(|| {
                format!(
                    "This replaces {} {}, the unsigned copy you installed from a file.",
                    old.name, old.version
                )
            })
        }
        (None, None) => None,
    };
    let allowed_before = unsigned
        .as_ref()
        .is_some_and(|h| dev.allowed.contains_key(h));
    Ok(Staged {
        manifest,
        dir: staging,
        new_permissions,
        unsigned,
        allowed_before,
        clash,
    })
}

/// A catalog plugin's name by id: the curated catalog's, or the id if only the signed one has it.
fn catalog_name(id: &str, signed: Option<&foxglove_plugin_trust::Catalog>) -> Option<String> {
    catalog()
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| e.name)
        .or_else(|| {
            signed
                .is_some_and(|c| c.plugins.iter().any(|e| e.id == id))
                .then(|| id.to_string())
        })
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
            let name = entry
                .name()
                .map(|n| n.into_owned())
                .unwrap_or_else(|e| format!("({e})"));
            return Err(format!("the archive has an unsafe path: {name}"));
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
        entry
            .by_ref()
            .take(MAX_UNPACKED - total + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        if total > MAX_UNPACKED {
            return Err("the archive unpacks to more than 64 MiB".into());
        }
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Installs a staged plugin (replacing an older version) with the permissions the user approved.
/// An unsigned one's build is allowed with it: installing it is the user's one-time OK.
pub fn commit(staged: Staged, enabled: bool) -> Result<(), String> {
    let plugins = plugins_dir().ok_or("no settings folder")?;
    // What was installed before the policy is taken first; this install isn't among it (#381).
    grandfather(&installed());
    super::plugin_trust::ungrandfather(&staged.manifest.id)?;
    let target = plugins.join(&staged.manifest.id);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("removing the old version: {e}"))?;
    }
    std::fs::rename(&staged.dir, &target).map_err(|e| e.to_string())?;
    if let Some(hash) = &staged.unsigned {
        super::plugin_trust::allow(hash, &staged.manifest.id, &staged.manifest.version)?;
    }
    let mut all = records();
    all.insert(
        staged.manifest.id.clone(),
        PluginRecord {
            enabled,
            approved: staged.manifest.permissions.clone(),
        },
    );
    save_records(&all)
}

/// Test hook for the Test lab: `FOXGLOVE_PLUGINS_AUTOINSTALL=<zip or folder>` installs and
/// enables that plugin at start with every permission its manifest declares, without asking.
/// Honoured only with developer mode for unsigned plugins and a `FOXGLOVE_CONFIG_DIR` that
/// isn't the real settings folder, so it can never reach a user's install. What happened goes
/// to stderr and `autoinstall.log` in the config dir.
pub fn autoinstall() {
    let Some(source) = std::env::var_os("FOXGLOVE_PLUGINS_AUTOINSTALL").filter(|v| !v.is_empty())
    else {
        return;
    };
    // A test build always uses its own folder (store::test_sandbox).
    let config_dir = match super::store::test_sandbox() {
        Some(dir) => Some(dir.to_path_buf()),
        None => std::env::var_os("FOXGLOVE_CONFIG_DIR").map(PathBuf::from),
    };
    let real =
        directories::ProjectDirs::from("", "", "Foxglove").map(|d| d.config_dir().to_path_buf());
    let result = autoinstall_allowed(
        super::plugin_trust::dev_env(),
        config_dir.as_deref(),
        real.as_deref(),
    )
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
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("autoinstall.log"))
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// The hook's guard: developer mode, and a config dir that's set and not the real one.
fn autoinstall_allowed(
    dev_unsigned: bool,
    config_dir: Option<&Path>,
    real: Option<&Path>,
) -> Result<(), String> {
    if !dev_unsigned {
        return Err("needs FOXGLOVE_DEV_UNSIGNED_PLUGINS=1".into());
    }
    let dir = config_dir.ok_or("needs FOXGLOVE_CONFIG_DIR")?;
    let same = |a: &Path, b: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(a) == canon(b)
            || a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
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
                let name = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                z.start_file(name, zip::write::SimpleFileOptions::default())
                    .map_err(|e| e.to_string())?;
                std::io::copy(
                    &mut std::fs::File::open(&path).map_err(|e| e.to_string())?,
                    z,
                )
                .map_err(|e| e.to_string())?;
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
    let mut all = records();
    all.entry(id.to_string()).or_default().enabled = enabled;
    save_records(&all)
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
    // Its sandbox profile, passwords and tokens go with it (#213). Not from tests: those live
    // in Windows, outside the test settings folder, and a test id can be a real plugin's.
    if !cfg!(test) {
        #[cfg(windows)]
        ::foxglove_plugin_host::delete_container(id);
        if let Err(e) = ::foxglove_plugin_host::secrets::delete_all(id) {
            eprintln!("Foxglove: removing {id}'s secrets: {e}");
        }
    }
    // So are the unsigned builds the user allowed for it (#381).
    if let Err(e) = super::plugin_trust::forget(id) {
        eprintln!("Foxglove: forgetting {id}'s allowed builds: {e}");
    }
    if let Err(e) = super::plugin_trust::ungrandfather(id) {
        eprintln!("Foxglove: forgetting {id} was installed before #381: {e}");
    }
    let mut all = records();
    all.remove(id);
    save_records(&all)
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
        assert!(plugins
            .iter()
            .all(|k| k.id.contains('.') && !k.permissions.is_empty()));
        assert!(
            plugins
                .iter()
                .all(|k| k.url.is_none() || k.sha256.is_some()),
            "downloads are pinned"
        );
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("foxglove-plugin-{}-{n}.zip", std::process::id()));
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, bytes) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
        path
    }

    #[test]
    fn autoinstall_only_in_dev_mode_and_a_test_config_dir() {
        // Built with joins, so the paths are real ones on Linux too (CI).
        let roaming = PathBuf::from(if cfg!(windows) {
            r"C:\Users\u\AppData\Roaming"
        } else {
            "/home/u/.config"
        });
        let real = roaming.join("Foxglove").join("config");
        let real = real.as_path();
        let test = std::env::temp_dir().join("foxglove-lab-1");
        assert!(autoinstall_allowed(true, Some(&test), Some(real)).is_ok());
        assert!(
            autoinstall_allowed(false, Some(&test), Some(real)).is_err(),
            "needs developer mode"
        );
        assert!(
            autoinstall_allowed(true, None, Some(real)).is_err(),
            "needs a config dir"
        );
        assert!(
            autoinstall_allowed(true, Some(real), Some(real)).is_err(),
            "never the real folder"
        );
        assert!(
            autoinstall_allowed(true, Some(&roaming.join("Foxglove")), Some(real)).is_err(),
            "nor the folder above it"
        );
        // Windows paths ignore case.
        #[cfg(windows)]
        assert!(autoinstall_allowed(
            true,
            Some(Path::new(r"c:\users\u\appdata\roaming\foxglove")),
            Some(real)
        )
        .is_err());
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
        let dir =
            std::env::temp_dir().join(format!("foxglove-plugin-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let good = zip_with(&[("plugin.toml", b"id = 1"), ("bin/run.exe", b"x")]);
        unpack(&good, &dir).unwrap();
        assert!(dir.join("bin/run.exe").exists());
        let evil = zip_with(&[("../escape.txt", b"x"), ("a", b""), ("b", b"")]);
        assert!(unpack(&evil, &dir.join("evil"))
            .unwrap_err()
            .contains("unsafe path"));
        assert!(!dir.join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(good);
        let _ = std::fs::remove_file(evil);
    }

    /// The tests that use the plugins folder and developer mode, one at a time: both are shared
    /// by every test in the process.
    static SIDELOAD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn sideload_lock() -> std::sync::MutexGuard<'static, ()> {
        let guard = SIDELOAD.lock().unwrap_or_else(|e| e.into_inner());
        super::super::store::test_config_dir();
        let _ = std::fs::remove_dir_all(plugins_dir().unwrap());
        super::super::store::save(
            "plugins.dev",
            &super::super::plugin_trust::DevMode::default(),
        )
        .unwrap();
        // Taken, and empty: these tests' plugins are all new installs.
        super::super::store::save(
            "plugins.grandfather",
            &super::super::plugin_trust::Grandfather {
                taken: true,
                ..Default::default()
            },
        )
        .unwrap();
        guard
    }

    /// The whole install flow in a temp settings folder (FOXGLOVE_CONFIG_DIR), never the real one.
    #[test]
    fn stage_commit_list_and_remove() {
        let _lock = sideload_lock();
        let root = super::super::store::test_config_dir()
            .parent()
            .unwrap()
            .to_path_buf();
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
        super::super::plugin_trust::set_dev_mode(true).unwrap();
        let staged = stage(&archive).unwrap();
        assert_eq!(staged.new_permissions.len(), 2);
        assert!(staged.new_permissions.iter().any(Permission::sensitive));
        commit(staged, true).unwrap();
        let plugins = installed();
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].record.enabled && plugins[0].record.approved.len() == 2);
        assert!(
            root.join("config/plugins/com.example.weather/weather.exe")
                .exists(),
            "inside the config dir"
        );
        let again = stage(&archive).unwrap();
        assert!(
            again.new_permissions.is_empty(),
            "same version asks for nothing new"
        );
        discard(again);
        remove("com.example.weather").unwrap();
        assert!(installed().is_empty());
        assert!(
            remove("../config").is_err(),
            "ids can't leave the plugins folder"
        );
        // The settings folder is shared with the other tests: only this test's plugins go.
        let _ = std::fs::remove_dir_all(root.join("config/plugins"));
        super::super::plugin_trust::set_dev_mode(false).unwrap();
        let _ = std::fs::remove_file(archive);
    }

    const MIC_BUILD: &str = "id = \"org.foxglove.mic-mute\"
name = \"Mic mute (my build)\"
version = \"0.0.1\"
api = \"1.0\"
run = [\"mic.exe\"]
permissions = [\"facts.write:mine.*\"]
";

    /// The done-when test (#381): a sideloaded unsigned plugin under a catalog id. Refused
    /// outside developer mode; in it, the clash warning and the one-time OK, the SIDELOADED
    /// and UNSIGNED badges, off again when developer mode is, a changed build asks again, and
    /// it gets only the permissions it was approved for.
    #[test]
    fn sideloaded_unsigned_plugin_with_warning_badge_and_permissions() {
        use super::super::plugin_trust::{self as trust, Trust};
        use super::super::plugins_ui::{cards, Tag};
        let _lock = sideload_lock();
        let archive = zip_with(&[("plugin.toml", MIC_BUILD.as_bytes()), ("mic.exe", b"MZ")]);

        // Developer mode is off by default: refused, nothing unpacked.
        let e = stage(&archive).unwrap_err();
        assert!(e.ends_with(UNSIGNED_REFUSED), "{e}");
        assert!(installed().is_empty());

        trust::set_dev_mode(true).unwrap();
        let staged = stage(&archive).unwrap();
        let hash = staged.unsigned.clone().expect("unsigned");
        assert!(!staged.allowed_before, "asks the first time");
        assert!(
            staged
                .clash
                .as_deref()
                .is_some_and(|c| c.starts_with("Same id as Mic mute from the Foxglove catalog")),
            "{:?}",
            staged.clash
        );
        commit(staged, true).unwrap();
        assert!(
            trust::dev_mode().allowed.contains_key(&hash),
            "the OK is kept by hash"
        );
        let again = stage(&archive).unwrap();
        assert!(again.allowed_before, "the same build isn't asked twice");
        discard(again);

        let plugins = installed();
        let t = trust_all(&plugins);
        assert_eq!(
            t.get("org.foxglove.mic-mute"),
            Some(&Trust::Unsigned(hash.clone()))
        );
        let card = |dev: &trust::DevMode| {
            cards(&plugins, |id| t[id].clone(), dev, &catalog())
                .into_iter()
                .find(|c| c.id == "org.foxglove.mic-mute")
                .unwrap()
        };
        let on = card(&trust::dev_mode());
        assert_eq!(
            (on.tag, on.unsigned, on.blocked.clone()),
            (Tag::Sideloaded, true, None)
        );

        // Developer mode off: disabled, not deleted, with the reason.
        trust::set_dev_mode(false).unwrap();
        let off = card(&trust::dev_mode());
        assert_eq!(off.blocked.as_deref(), Some(trust::OFF_UNSIGNED));
        assert_eq!(installed().len(), 1, "still installed");
        assert!(
            installed()[0].record.enabled,
            "its own switch is kept for later"
        );

        // A changed build is a new hash, not allowed yet.
        trust::set_dev_mode(true).unwrap();
        let dir = plugins_dir().unwrap().join("org.foxglove.mic-mute");
        std::fs::write(dir.join("mic.exe"), b"MZ patched").unwrap();
        let changed = trust_all(&installed());
        assert_eq!(
            trust::verdict(&changed["org.foxglove.mic-mute"], &trust::dev_mode()),
            Err(trust::OFF_NOT_ALLOWED.into())
        );
        std::fs::write(dir.join("mic.exe"), b"MZ").unwrap();

        // Its permissions are enforced: it gets what it was approved for and nothing else.
        let approved = installed()[0].record.approved.clone();
        let ctx = foxglove_plugin_host::Context {
            facts: foxglove_devices::facts::Facts::default(),
            devices: std::sync::Arc::new(foxglove_plugin_host::NoDevices),
            client: foxglove_plugin_host::Client::Plugin(approved),
            app_version: "test".into(),
            limits: Default::default(),
        };
        let input = [
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"api":"1.0","client":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"facts.set","params":{"key":"mine.x","value":true}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"facts.set","params":{"key":"weather.x","value":true}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"lighting.set","params":{}}"#,
        ]
        .join("\n")
            + "\n";
        let out = Out::default();
        foxglove_plugin_host::serve(std::io::Cursor::new(input), out.clone(), ctx).unwrap();
        let replies = out.replies();
        let denied = foxglove_plugin_api::rpc::code::DENIED;
        let code = |id: i64| {
            replies
                .iter()
                .find(|r| r["id"] == id)
                .map(|r| r["error"]["code"].as_i64())
        };
        assert_eq!(code(1), Some(None), "its own facts: {replies:?}");
        assert_eq!(code(2), Some(Some(denied as i64)), "another plugin's facts");
        assert_eq!(
            code(3),
            Some(Some(denied as i64)),
            "lighting it never asked for"
        );

        // Removing it forgets the OK too.
        remove("org.foxglove.mic-mute").unwrap();
        assert!(!trust::dev_mode().allowed.contains_key(&hash));
        trust::set_dev_mode(false).unwrap();
        let _ = std::fs::remove_file(archive);
    }

    /// #381: a signed plugin is unaffected by developer mode, and installing it over an unsigned
    /// copy says so (the reverse of the catalog-id warning).
    #[test]
    fn signed_plugins_are_unaffected_and_replacing_a_sideload_warns() {
        use super::super::plugin_trust::{self as trust, tests::*, Trust};
        let _lock = sideload_lock();
        // The unsigned copy, in developer mode.
        trust::set_dev_mode(true).unwrap();
        let mine = zip_with(&[("plugin.toml", MIC_BUILD.as_bytes()), ("mic.exe", b"MZ")]);
        commit(stage(&mine).unwrap(), true).unwrap();
        trust::set_dev_mode(false).unwrap();

        // The signed release: the same id, its files and archive in a (test) catalog.
        let release = MIC_BUILD
            .replace("Mic mute (my build)", "Mic mute")
            .replace("0.0.1", "1.0.0");
        let files = std::env::temp_dir().join(format!("foxglove-signed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&files);
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("plugin.toml"), &release).unwrap();
        std::fs::write(files.join("mic.exe"), b"MZ signed").unwrap();
        let archive = zip_dir(&files).unwrap();
        let bytes = std::fs::read(&archive).unwrap();
        let entry = entry_for("org.foxglove.mic-mute", "1.0.0", &bytes, &files);
        let signed = Ok(catalog_with(vec![entry], vec![]));

        let off = trust::DevMode::default();
        let staged = stage_with(&archive, &signed, &off).unwrap();
        assert!(staged.unsigned.is_none(), "signed: no OK to give");
        assert_eq!(
            staged.clash.as_deref(),
            Some("This replaces Mic mute (my build) 0.0.1, the unsigned copy you installed from a file.")
        );
        commit(staged, true).unwrap();
        let dir = plugins_dir().unwrap().join("org.foxglove.mic-mute");
        let t = trust::trust_in(signed.as_ref().ok(), "org.foxglove.mic-mute", "1.0.0", &dir);
        assert_eq!(t, Trust::Signed);
        assert_eq!(
            trust::verdict(&t, &off),
            Ok(()),
            "runs with developer mode off"
        );

        remove("org.foxglove.mic-mute").unwrap();
        let _ = std::fs::remove_dir_all(files);
        let _ = std::fs::remove_file(archive);
        let _ = std::fs::remove_file(mine);
    }

    /// #381: a sideloaded plugin never updates from the catalog. A newer signed version of its
    /// id, with a download, leaves its files, its version and its card alone; and the catalog
    /// refresh has no way to touch the plugins folder.
    #[test]
    fn sideloads_never_update_from_the_catalog() {
        use super::super::plugin_trust::{self as trust, tests::*, Trust};
        use super::super::plugins_ui::{cards, Tag};
        let _lock = sideload_lock();
        trust::set_dev_mode(true).unwrap();
        let mine = zip_with(&[("plugin.toml", MIC_BUILD.as_bytes()), ("mic.exe", b"MZ")]);
        commit(stage(&mine).unwrap(), true).unwrap();
        let dir = plugins_dir().unwrap().join("org.foxglove.mic-mute");
        let before = trust::package_hash(&dir).unwrap();

        let newer = foxglove_plugin_trust::Entry {
            url: Some("https://example.invalid/mic-mute-9.9.9.zip".into()),
            ..entry_for("org.foxglove.mic-mute", "9.9.9", b"newer", &dir)
        };
        let catalog_now = catalog_with(vec![newer], vec![]);
        let t = trust::trust_in(Some(&catalog_now), "org.foxglove.mic-mute", "0.0.1", &dir);
        assert_eq!(t, Trust::Unsigned(before.clone()), "still the user's build");
        let plugins = installed();
        let card = cards(&plugins, |_| t.clone(), &trust::dev_mode(), &catalog())
            .into_iter()
            .find(|c| c.id == "org.foxglove.mic-mute")
            .unwrap();
        assert_eq!(card.tag, Tag::Sideloaded);
        assert!(card.name.ends_with("0.0.1") && card.blocked.is_none());
        assert_eq!(
            trust::package_hash(&dir).unwrap(),
            before,
            "files untouched"
        );
        // Nothing that fetches the catalog installs or replaces plugins.
        let refresh = include_str!("catalog_refresh.rs");
        for word in ["plugin_install", "plugins_dir", "commit(", "stage("] {
            assert!(
                !refresh.contains(word),
                "catalog_refresh.rs mentions {word}"
            );
        }

        remove("org.foxglove.mic-mute").unwrap();
        trust::set_dev_mode(false).unwrap();
        let _ = std::fs::remove_file(mine);
    }

    /// #381 with the real store: a plugin installed before the policy keeps running while the
    /// key is the placeholder; a new install, or the same id installed again, gets the unsigned
    /// flow; and the one-time line, once due, is kept as shown.
    #[test]
    fn plugins_from_before_the_policy_are_grandfathered() {
        use super::super::plugin_trust::{self as trust, Grandfather, Trust};
        let _lock = sideload_lock();
        assert!(
            trust::signing_key_missing(),
            "this build has the placeholder key"
        );
        // A plugin from before: its folder and record, as an older Foxglove left them.
        let old = plugins_dir().unwrap().join("org.example.old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(
            old.join("plugin.toml"),
            "id = \"org.example.old\"\nname = \"Old\"\nversion = \"1.0.0\"\napi = \"1.0\"\nrun = [\"old.exe\"]\n",
        )
        .unwrap();
        std::fs::write(old.join("old.exe"), b"MZ").unwrap();
        set_enabled("org.example.old", true).unwrap();
        // The first run of this policy: nothing taken yet.
        super::super::store::save("plugins.grandfather", &Grandfather::default()).unwrap();

        let off = trust::DevMode::default();
        let t = trust_all(&installed());
        assert!(
            matches!(t["org.example.old"], Trust::Grandfathered(_)),
            "{t:?}"
        );
        assert_eq!(
            trust::verdict(&t["org.example.old"], &off),
            Ok(()),
            "keeps running"
        );

        // A new install from a file gets the unsigned flow, placeholder or not.
        let e = stage(&zip_with(&[
            ("plugin.toml", MIC_BUILD.as_bytes()),
            ("mic.exe", b"MZ"),
        ]))
        .unwrap_err();
        assert!(e.ends_with(UNSIGNED_REFUSED), "{e}");
        trust::set_dev_mode(true).unwrap();
        let again = MIC_BUILD
            .replace("org.foxglove.mic-mute", "org.example.old")
            .replace("0.0.1", "1.0.0");
        let archive = zip_with(&[("plugin.toml", again.as_bytes()), ("mic.exe", b"MZ")]);
        let staged = stage(&archive).unwrap();
        assert!(staged.unsigned.is_some() && !staged.allowed_before, "asks");
        commit(staged, true).unwrap();
        trust::set_dev_mode(false).unwrap();
        let t = trust_all(&installed());
        assert_eq!(
            trust::verdict(&t["org.example.old"], &off),
            Err(trust::OFF_UNSIGNED.into()),
            "installed again from a file: not grandfathered any more"
        );

        // The one-time line is kept as shown.
        let due = Grandfather {
            taken: true,
            plugins: [("org.example.old".to_string(), "1.0.0".to_string())].into(),
            notice_shown: false,
        };
        super::super::store::save("plugins.grandfather", &due).unwrap();
        assert!(trust::signing_notice(&trust::grandfather(&[]), false).is_some());
        trust::notice_shown().unwrap();
        assert_eq!(trust::signing_notice(&trust::grandfather(&[]), false), None);

        remove("org.example.old").unwrap();
        let _ = std::fs::remove_file(archive);
    }

    /// Saving the installed list leaves the rest of `[plugins]` alone (it used to drop the
    /// catalog's rollback state and developer mode).
    #[test]
    fn saving_records_keeps_the_other_plugin_sections() {
        let _lock = sideload_lock();
        super::super::plugin_trust::set_dev_mode(true).unwrap();
        set_enabled("org.example.nothing", false).unwrap();
        assert!(super::super::plugin_trust::dev_mode().on);
        super::super::plugin_trust::set_dev_mode(false).unwrap();
        let mut all = records();
        all.remove("org.example.nothing");
        save_records(&all).unwrap();
    }

    #[derive(Clone, Default)]
    struct Out(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Out {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Out {
        fn replies(&self) -> Vec<serde_json::Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }
}
