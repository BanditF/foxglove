// SPDX-License-Identifier: MIT
//! H5.4 (docs: review/design-h5-rebrand.md §2): finds an installed app and offers to adopt it.
//! Wired through `discover` (the Found card and Add an app…). The fallback list of apps is data
//! (`products.json`), read at run time; apps that register themselves are found by `discover`.
//!
//! - [`find`] reads the product's installer record (`HKCU\Software\<product>\Installer`, value
//!   `App` = the install folder) and, failing that, its known install folders. Only a hit from
//!   the installer record is a trusted install path ([`InstalledPeers::installed_exe`]); a hit
//!   from a known folder can be adopted but gives no peer trust.
//! - [`InstalledPeers`] supplies the `PeerPaths` records `owner::judge` consumes (H5.3).
//! - [`adopt`] writes the product's manifest with this PC's exe path into a staging folder and
//!   registers it through the existing `--adopt` code ([`crate::adopt::run`]). It never writes a
//!   grant: the keyboard grant stays "not asked" until the user's own Allow.
//! - [`row`] is the state the Modules page shows for a product.
//!
//! The machine (registry, file system, file versions) is the [`Machine`] trait, so every test
//! runs on a fake. The real one, `RealMachine`, is Windows only and read-only.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use foxglove_plugin::owner::PeerPaths;

/// How a product's manifest is made. `@EXE@` and `@VERSION@` are filled in.
#[derive(Clone, Debug)]
pub enum ManifestSource {
    /// A template shipped inside this build (the text).
    Builtin(String),
    /// The template the product ships in its install folder, at this relative path.
    InstallFolder(String),
}

/// One product Hideout can adopt, read from the fallback data file (`products.json`). Nothing
/// else in the hub names a product: a rename or a new entry changes that file.
#[derive(Clone, Debug)]
pub struct Product {
    pub id: String,
    /// What the Modules page calls it.
    pub name: String,
    /// Image paths relative to the install folder, first match wins.
    pub exes: Vec<String>,
    /// The installer record: `HKCU\<key>`, value `App` = install folder.
    pub record_key: String,
    /// Known folders, as environment variable plus relative folder.
    pub known: Vec<(String, String)>,
    pub manifest: ManifestSource,
}

/// The fallback list, as shipped in this build.
const PRODUCTS_JSON: &str = include_str!("products.json");

/// Manifest templates bundled in this build, by the name the data file uses.
fn template(name: &str) -> Option<&'static str> {
    match name {
        "foxglove-hub-link" => Some(include_str!(
            "../../../app/src/foxglove/hub_link/module.toml"
        )),
        _ => None,
    }
}

/// Reads a product list. An entry that is incomplete or names an unknown template is skipped
/// (and logged); the rest still load.
pub fn parse_products(text: &str) -> Vec<Product> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        log::warn!("product list: not valid JSON; no fallback entries");
        return Vec::new();
    };
    let strings = |x: Option<&serde_json::Value>| -> Vec<String> {
        x.and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for e in v
        .get("products")
        .and_then(|p| p.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        let s = |k: &str| e.get(k).and_then(|x| x.as_str()).map(String::from);
        let manifest = e.get("manifest").and_then(|m| {
            if let Some(t) = m.get("template").and_then(|t| t.as_str()) {
                template(t).map(|t| ManifestSource::Builtin(t.to_string()))
            } else {
                m.get("install_folder")
                    .and_then(|t| t.as_str())
                    .map(|t| ManifestSource::InstallFolder(t.to_string()))
            }
        });
        let known = e
            .get("known")
            .and_then(|k| k.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let p = p.as_array()?;
                        Some((
                            p.first()?.as_str()?.to_string(),
                            p.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        match (s("id"), s("name"), s("record_key"), manifest) {
            (Some(id), Some(name), Some(record_key), Some(manifest)) => out.push(Product {
                id,
                name,
                exes: strings(e.get("exes")),
                record_key,
                known,
                manifest,
            }),
            _ => log::warn!(
                "product list: an entry is incomplete or has an unknown template; skipped"
            ),
        }
    }
    out
}

/// The fallback products, parsed once.
pub fn products() -> &'static [Product] {
    static LIST: std::sync::OnceLock<Vec<Product>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| parse_products(PRODUCTS_JSON))
}

pub fn product(id: &str) -> Option<&'static Product> {
    products().iter().find(|p| p.id == id)
}

/// What the finder needs from the PC. Read-only.
pub trait Machine {
    /// A string value under `HKCU`.
    fn registry_string(&self, key: &str, value: &str) -> Option<String>;
    fn env(&self, name: &str) -> Option<String>;
    fn is_file(&self, p: &Path) -> bool;
    /// `major.minor.patch` of an exe's file version.
    fn exe_version(&self, p: &Path) -> Option<String>;
    fn read_text(&self, p: &Path) -> Option<String>;
    /// The names of the subkeys under an `HKCU` key (empty when the key is absent).
    fn registry_subkeys(&self, key: &str) -> Vec<String>;
    /// The canonical form of an existing path (`..`, junctions and short names resolved, no
    /// `\?\` prefix); `None` when it can't be resolved.
    fn canonical(&self, p: &Path) -> Option<PathBuf>;
    /// Whether `p` is on one of this PC's fixed drives: not UNC, not a mapped network drive,
    /// not removable. Reads no files, so a dead network drive can't block on it.
    fn is_local_fixed(&self, p: &Path) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The installer's own record: a trusted install path.
    Record,
    /// A known folder: adoptable, not trusted as a peer path.
    KnownFolder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub id: String,
    pub exe: PathBuf,
    pub version: String,
    pub source: Source,
    /// The install folder, when known.
    pub folder: Option<PathBuf>,
    /// The record's own `Manifest` value (a rendered manifest), when it gave one.
    pub manifest: Option<PathBuf>,
}

/// An absolute, local path: never relative, never UNC (`\server\share`, `//server/share`).
fn is_local(p: &Path) -> bool {
    let s = p.to_string_lossy();
    p.is_absolute() && !s.starts_with(r"\\") && !s.starts_with("//")
}

/// `p` as a local path on a fixed drive of this PC, canonical, or `None`. The text is checked
/// first, then the drive type (neither reads a file), and only then is the path resolved, so a
/// disconnected network drive is refused before any file access.
pub fn local_path(m: &dyn Machine, p: &Path) -> Option<PathBuf> {
    if !is_local(p) || !m.is_local_fixed(p) {
        return None;
    }
    let c = m.canonical(p)?;
    (is_local(&c) && m.is_local_fixed(&c)).then_some(c)
}

/// The installed copy of `p`, if any. A record that names a missing exe falls through to the
/// known folders; a UNC folder is never accepted.
pub fn find(m: &dyn Machine, p: &Product) -> Option<Found> {
    let mut folders: Vec<(PathBuf, Source)> = Vec::new();
    let record_dir = m.registry_string(&p.record_key, "App").map(PathBuf::from);
    let record_exe = m.registry_string(&p.record_key, "Exe").map(PathBuf::from);
    let record_manifest = m
        .registry_string(&p.record_key, "Manifest")
        .map(PathBuf::from)
        .and_then(|x| local_path(m, &x));
    let record_version = m
        .registry_string(&p.record_key, "Version")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    // The record's own Exe first: it is what the installer wrote. It must be local, a real
    // file, and one of this product's image names.
    if let Some(exe) = record_exe {
        let known_name = p.exes.iter().any(|rel| {
            let leaf = rel.rsplit('\\').next().unwrap_or(rel);
            file_name_is(&exe, leaf)
        });
        // Text and drive type first; the file is only touched after both.
        let exe = if known_name {
            local_path(m, &exe)
        } else {
            None
        };
        if let Some(exe) = exe.filter(|e| m.is_file(e)) {
            if let Some(version) = m.exe_version(&exe).or_else(|| record_version.clone()) {
                return Some(Found {
                    id: p.id.clone(),
                    exe,
                    version,
                    source: Source::Record,
                    folder: record_dir.as_deref().and_then(|d| local_path(m, d)),
                    manifest: record_manifest,
                });
            }
        }
    }
    if let Some(dir) = record_dir {
        folders.push((dir, Source::Record));
    }
    for (var, rel) in &p.known {
        if let Some(base) = m.env(var.as_str()) {
            folders.push((Path::new(&base).join(rel), Source::KnownFolder));
        }
    }
    for (dir, source) in folders {
        let Some(dir) = local_path(m, &dir) else {
            continue;
        };
        for name in &p.exes {
            let exe = dir.join(name);
            if m.is_file(&exe) {
                // A pip launcher has no file-version resource: the record's Version stands in,
                // for record hits only.
                let version = m.exe_version(&exe).or_else(|| {
                    (source == Source::Record)
                        .then(|| record_version.clone())
                        .flatten()
                });
                if let Some(version) = version {
                    let manifest = (source == Source::Record)
                        .then(|| record_manifest.clone())
                        .flatten();
                    return Some(Found {
                        id: p.id.clone(),
                        exe,
                        version,
                        source,
                        folder: Some(dir),
                        manifest,
                    });
                }
            }
        }
    }
    None
}

pub fn find_all(m: &dyn Machine) -> Vec<Found> {
    products().iter().filter_map(|p| find(m, p)).collect()
}

/// The `PeerPaths` records for `owner::judge`: installed exe paths from installer records only,
/// and the adopted `run[0]` paths the caller read from the inventory.
pub struct InstalledPeers {
    pub found: Vec<Found>,
    pub adopted: Vec<PathBuf>,
}

fn file_name_is(p: &Path, exe: &str) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(exe))
}

impl PeerPaths for InstalledPeers {
    fn installed_exe(&self, exe: &str) -> Option<PathBuf> {
        self.found
            .iter()
            .find(|f| f.source == Source::Record && file_name_is(&f.exe, exe))
            .map(|f| f.exe.clone())
    }
    fn run0(&self, exe: &str) -> Vec<PathBuf> {
        self.adopted
            .iter()
            .filter(|p| file_name_is(p, exe))
            .cloned()
            .collect()
    }
}

/// The manifest text for `f`, with this PC's exe and version filled in.
fn manifest_text(m: &dyn Machine, p: &Product, f: &Found) -> Result<String, String> {
    let template = match &p.manifest {
        ManifestSource::Builtin(t) => t.clone(),
        ManifestSource::InstallFolder(rel) => {
            // From the record's Manifest value, else from the install folder (never exe.parent():
            // the exe sits in venv\Scripts).
            let at = match (&f.manifest, &f.folder) {
                (Some(x), _) => x.clone(),
                (None, Some(dir)) => dir.join(rel),
                (None, None) => return Err(format!("{} has no manifest for Hideout yet", p.name)),
            };
            m.read_text(&at)
                .ok_or_else(|| format!("{} has no manifest for Hideout yet", p.name))?
        }
    };
    let exe = f.exe.display().to_string().replace('\\', "/");
    Ok(template
        .replace("@EXE@", &exe)
        .replace("@VERSION@", &f.version))
}

/// The user's OK on **Use in Hideout** (or the refresh after an update): registers `found` in
/// the hub folder `home` through the `--adopt` code, staging the manifest in `staging`. Writes
/// no grant and starts nothing. Returns the line `--adopt` prints.
pub fn adopt(
    m: &dyn Machine,
    home: &Path,
    staging: &Path,
    found: &Found,
    now: &str,
) -> Result<String, String> {
    let p = product(&found.id).ok_or_else(|| "not an app Hideout knows".to_string())?;
    let text = manifest_text(m, p, found)?;
    let dir = staging.join(&p.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join("module.toml"), text)
        .map_err(|e| format!("manifest not saved: {e}"))?;
    let out = crate::adopt::run(home, &dir, now);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// What the hub knows of an adopted product.
pub struct AdoptedFacts {
    pub run0: PathBuf,
    pub version: String,
    /// The keyboard grant is on (only the user's Allow sets it).
    pub granted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// Nothing installed, nothing adopted: **Get {name}**.
    GetIt,
    /// Installed, not adopted: **Use in Hideout**.
    UseInHideout,
    /// Adopted, module mode off.
    RunsOnItsOwn,
    /// Adopted, module mode on, no grant yet.
    WaitsForYourOk,
    /// Adopted, module mode on, granted.
    Ready,
    /// Adopted, but its exe is gone: **Remove from list**.
    NotInstalled,
    /// The installed app changed (new version or another path): **Update in Hideout**.
    NeedsRefresh,
}

pub fn row(
    found: Option<&Found>,
    adopted: Option<&AdoptedFacts>,
    module_mode: bool,
    m: &dyn Machine,
) -> Row {
    match adopted {
        None => found.map_or(Row::GetIt, |_| Row::UseInHideout),
        Some(a) if !m.is_file(&a.run0) => Row::NotInstalled,
        Some(a) => {
            if found.is_some_and(|f| f.version != a.version || f.exe != a.run0) {
                Row::NeedsRefresh
            } else if !module_mode {
                Row::RunsOnItsOwn
            } else if a.granted {
                Row::Ready
            } else {
                Row::WaitsForYourOk
            }
        }
    }
}

impl Row {
    /// The state line, for a product named `name`.
    pub fn line(self, name: &str) -> String {
        let s = match self {
            Row::GetIt => "Not installed",
            Row::UseInHideout => "Installed",
            Row::RunsOnItsOwn => "Runs on its own",
            Row::WaitsForYourOk => "Waits for your OK",
            Row::Ready => "Ready",
            Row::NotInstalled => "Not installed",
            Row::NeedsRefresh => "Changed since you added it",
        };
        format!("{name} \u{b7} {s}")
    }
    /// The one button, if any.
    pub fn button(self, name: &str) -> Option<String> {
        match self {
            Row::GetIt => Some(format!("Get {name}")),
            Row::UseInHideout => Some("Use in Hideout".into()),
            Row::NotInstalled => Some("Remove from list".into()),
            Row::NeedsRefresh => Some("Update in Hideout".into()),
            _ => None,
        }
    }
}

/// The lab's discovery source (G1): `<lab folder>\lab-discover.json`,
/// `{"registry": {"<HKCU key>": {"<value>": "<string>"}}, "env": {"<NAME>": "<value>"}}`.
/// In lab mode the Found card reads this instead of the user's real `HKCU` and environment, so
/// an agent can press Add without touching the PC's records. Files are still read from disk,
/// read-only, so a lab can point a record at a folder under its own lab folder.
pub const LAB_DISCOVER_FILE: &str = "lab-discover.json";

type Strs = std::collections::BTreeMap<String, String>;

#[derive(Default)]
struct LabDiscover {
    registry: std::collections::BTreeMap<String, Strs>,
    env: Strs,
}

fn strs(v: Option<&serde_json::Value>) -> Strs {
    let mut out = Strs::new();
    if let Some(o) = v.and_then(|v| v.as_object()) {
        for (k, v) in o {
            if let Some(s) = v.as_str() {
                out.insert(k.clone(), s.to_string());
            }
        }
    }
    out
}

fn parse_lab(text: &str) -> Result<LabDiscover, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("{LAB_DISCOVER_FILE}: {e}"))?;
    let mut d = LabDiscover {
        env: strs(v.get("env")),
        ..Default::default()
    };
    if let Some(o) = v.get("registry").and_then(|r| r.as_object()) {
        for (k, vals) in o {
            d.registry.insert(k.clone(), strs(Some(vals)));
        }
    }
    Ok(d)
}

pub struct LabMachine {
    data: LabDiscover,
    files: Box<dyn Machine>,
}

fn key_norm(k: &str) -> String {
    k.replace('/', "\\").trim_matches('\\').to_lowercase()
}

impl LabMachine {
    /// `text` is the file's contents (`None` when absent: nothing is found).
    pub fn parse(text: Option<&str>, files: Box<dyn Machine>) -> Result<LabMachine, String> {
        let data = match text {
            Some(t) => parse_lab(t)?,
            None => LabDiscover::default(),
        };
        Ok(LabMachine { data, files })
    }
}

impl Machine for LabMachine {
    fn registry_string(&self, key: &str, value: &str) -> Option<String> {
        let want = key_norm(key);
        let (_, vals) = self
            .data
            .registry
            .iter()
            .find(|(k, _)| key_norm(k) == want)?;
        let (_, v) = vals.iter().find(|(n, _)| n.eq_ignore_ascii_case(value))?;
        Some(v.trim_end_matches('\\').to_string()).filter(|s| !s.is_empty())
    }
    fn env(&self, name: &str) -> Option<String> {
        self.data
            .env
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .filter(|s| !s.is_empty())
    }
    fn is_file(&self, p: &Path) -> bool {
        self.files.is_file(p)
    }
    fn exe_version(&self, p: &Path) -> Option<String> {
        self.files.exe_version(p)
    }
    fn read_text(&self, p: &Path) -> Option<String> {
        self.files.read_text(p)
    }
    fn registry_subkeys(&self, key: &str) -> Vec<String> {
        let base = format!("{}\\", key_norm(key));
        let mut out = Vec::new();
        for k in self.data.registry.keys() {
            let n = key_norm(k);
            if let Some(rest) = n.strip_prefix(&base) {
                let first = rest.split('\\').next().unwrap_or("");
                // Original casing of the child name.
                let orig = k.replace('/', "\\");
                let orig = orig.trim_matches('\\');
                let child = orig[base.len()..]
                    .split('\\')
                    .next()
                    .unwrap_or(first)
                    .to_string();
                if !child.is_empty() && !out.contains(&child) {
                    out.push(child);
                }
            }
        }
        out
    }
    fn canonical(&self, p: &Path) -> Option<PathBuf> {
        self.files.canonical(p)
    }
    fn is_local_fixed(&self, p: &Path) -> bool {
        self.files.is_local_fixed(p)
    }
}

/// The machine discovery reads: the lab's fake registry when this hub runs in lab mode (the
/// registry refuses the user's own hub folder), else `real`. A broken lab file reads as empty
/// and is logged; it never falls back to the real registry.
pub fn machine_for(home: &Path, real: Box<dyn Machine>) -> Box<dyn Machine> {
    if !hideout_registry::lab::lab_mode_from_env(home).on() {
        return real;
    }
    let path = home.join(LAB_DISCOVER_FILE);
    let text = std::fs::read_to_string(&path).ok();
    match LabMachine::parse(text.as_deref(), real) {
        Ok(m) => Box::new(m),
        Err(e) => {
            log::warn!("lab discovery: {e}; nothing is found");
            // `real` was moved into the failed parse; rebuild an empty lab machine on a blank PC.
            Box::new(LabMachine {
                data: LabDiscover::default(),
                files: Box::new(NoFiles),
            })
        }
    }
}

struct NoFiles;
impl Machine for NoFiles {
    fn registry_string(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    fn env(&self, _: &str) -> Option<String> {
        None
    }
    fn is_file(&self, _: &Path) -> bool {
        false
    }
    fn exe_version(&self, _: &Path) -> Option<String> {
        None
    }
    fn read_text(&self, _: &Path) -> Option<String> {
        None
    }
    fn registry_subkeys(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn canonical(&self, p: &Path) -> Option<PathBuf> {
        Some(p.to_path_buf())
    }
    fn is_local_fixed(&self, _: &Path) -> bool {
        true
    }
}

#[cfg(windows)]
#[allow(unused_imports)]
pub use real::RealMachine;

/// The real machine: read-only registry, environment and file reads.
#[cfg(windows)]
mod real {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        RRF_RT_REG_SZ,
    };

    pub struct RealMachine;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    impl Machine for RealMachine {
        fn registry_string(&self, key: &str, value: &str) -> Option<String> {
            let (k, v) = (wide(key.as_ref()), wide(value.as_ref()));
            let mut bytes = 0u32;
            // SAFETY: NUL-terminated wide strings; a size query then a read into our buffer.
            unsafe {
                let e = RegGetValueW(
                    HKEY_CURRENT_USER,
                    k.as_ptr(),
                    v.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut bytes,
                );
                if e != 0 || bytes < 2 {
                    return None;
                }
                let mut buf = vec![0u16; bytes as usize / 2 + 1];
                let mut cap = (buf.len() * 2) as u32;
                let e = RegGetValueW(
                    HKEY_CURRENT_USER,
                    k.as_ptr(),
                    v.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut cap,
                );
                if e != 0 {
                    return None;
                }
                let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                let s = String::from_utf16(&buf[..n]).ok()?;
                // The installer writes "[INSTALLFOLDER]" with a trailing separator.
                Some(s.trim_end_matches('\\').to_string()).filter(|s| !s.is_empty())
            }
        }
        fn env(&self, name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|s| !s.is_empty())
        }
        fn is_file(&self, p: &Path) -> bool {
            p.is_file()
        }
        fn exe_version(&self, p: &Path) -> Option<String> {
            let f = wide(p.as_os_str());
            // SAFETY: standard version-resource read into a buffer sized from the query.
            unsafe {
                let mut h = 0u32;
                let n = GetFileVersionInfoSizeW(f.as_ptr(), &mut h);
                if n == 0 {
                    return None;
                }
                let mut buf = vec![0u8; n as usize];
                if GetFileVersionInfoW(f.as_ptr(), 0, n, buf.as_mut_ptr().cast()) == 0 {
                    return None;
                }
                let root = wide("\\".as_ref());
                let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
                let mut len = 0u32;
                if VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut out, &mut len) == 0
                    || out.is_null()
                    || (len as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
                {
                    return None;
                }
                let info = &*(out as *const VS_FIXEDFILEINFO);
                Some(format!(
                    "{}.{}.{}",
                    info.dwFileVersionMS >> 16,
                    info.dwFileVersionMS & 0xffff,
                    info.dwFileVersionLS >> 16
                ))
            }
        }
        fn read_text(&self, p: &Path) -> Option<String> {
            // A manifest is small; a huge file is not one.
            const MAX: u64 = 256 * 1024;
            if std::fs::metadata(p).ok()?.len() > MAX {
                return None;
            }
            std::fs::read_to_string(p).ok()
        }
        fn canonical(&self, p: &Path) -> Option<PathBuf> {
            let c = std::fs::canonicalize(p).ok()?;
            Some(PathBuf::from(hideout_registry::keyboard::strip_verbatim(
                &c.to_string_lossy(),
            )))
        }
        fn is_local_fixed(&self, p: &Path) -> bool {
            hideout_registry::permission::on_local_fixed_drive(&p.to_string_lossy())
        }
        fn registry_subkeys(&self, key: &str) -> Vec<String> {
            let k = wide(key.as_ref());
            let mut out = Vec::new();
            // SAFETY: read-only open, enumerate into our buffer, close.
            unsafe {
                let mut h: HKEY = std::mem::zeroed();
                if RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_READ, &mut h) != 0 {
                    return out;
                }
                let mut i = 0u32;
                loop {
                    let mut buf = [0u16; 256];
                    let mut len = buf.len() as u32;
                    let e = RegEnumKeyExW(
                        h,
                        i,
                        buf.as_mut_ptr(),
                        &mut len,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                    if e != 0 {
                        break;
                    }
                    if let Ok(s) = String::from_utf16(&buf[..len as usize]) {
                        out.push(s);
                    }
                    i += 1;
                }
                RegCloseKey(h);
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn lab_machine_reads_only_the_lab_file() {
        let json = r#"{"registry":{"Software\\Hideout\\Modules\\org.x.a":{"Path":"C:\\lab\\a\\"}},"env":{"LOCALAPPDATA":"C:\\lab\\local"}}"#;
        let m = LabMachine::parse(Some(json), Box::new(NoFiles)).unwrap();
        let k = "software\\hideout\\modules\\ORG.x.a";
        assert_eq!(m.registry_string(k, "path").as_deref(), Some("C:\\lab\\a"));
        assert_eq!(
            m.registry_string("Software\\Hideout\\Modules\\org.x.b", "Path"),
            None
        );
        assert_eq!(
            m.registry_subkeys("Software\\Hideout\\Modules"),
            vec!["org.x.a".to_string()]
        );
        assert!(m.registry_subkeys("Software\\Other").is_empty());
        assert_eq!(m.env("localappdata").as_deref(), Some("C:\\lab\\local"));
        assert_eq!(m.env("PATH"), None);
        assert!(LabMachine::parse(None, Box::new(NoFiles))
            .unwrap()
            .registry_subkeys("x")
            .is_empty());
        assert!(LabMachine::parse(Some("{"), Box::new(NoFiles)).is_err());
    }

    #[test]
    fn machine_for_is_real_outside_lab_mode() {
        // No HIDEOUT_LAB in the test binary: the real machine is returned untouched.
        let home = std::env::temp_dir().join("hideout-g1-off");
        let m = machine_for(&home, Box::new(NoFiles));
        assert!(m.registry_subkeys("anything").is_empty());
    }

    #[derive(Default)]
    struct Fake {
        reg: BTreeMap<(String, String), String>,
        env: BTreeMap<String, String>,
        /// path -> (version, text)
        files: BTreeMap<PathBuf, (String, Option<String>)>,
        /// Folders (and everything below) on a mapped or removable drive.
        remote: Vec<PathBuf>,
    }

    impl Machine for Fake {
        fn registry_string(&self, key: &str, value: &str) -> Option<String> {
            self.reg.get(&(key.into(), value.into())).cloned()
        }
        fn env(&self, n: &str) -> Option<String> {
            self.env.get(n).cloned()
        }
        fn is_file(&self, p: &Path) -> bool {
            self.files.contains_key(p)
        }
        fn exe_version(&self, p: &Path) -> Option<String> {
            self.files.get(p).map(|f| f.0.clone())
        }
        fn read_text(&self, p: &Path) -> Option<String> {
            self.files.get(p).and_then(|f| f.1.clone())
        }
        fn registry_subkeys(&self, _key: &str) -> Vec<String> {
            Vec::new()
        }
        fn canonical(&self, p: &Path) -> Option<PathBuf> {
            Some(p.to_path_buf())
        }
        fn is_local_fixed(&self, p: &Path) -> bool {
            !self.remote.iter().any(|r| p.starts_with(r))
        }
    }

    fn fox() -> &'static Product {
        product("foxglove").unwrap()
    }

    fn with_record(dir: &str, exe_present: bool) -> Fake {
        let mut f = Fake::default();
        f.reg
            .insert((fox().record_key.clone(), "App".into()), dir.to_string());
        if exe_present {
            f.files.insert(
                PathBuf::from(dir).join("foxglove.exe"),
                ("0.3.0".into(), None),
            );
        }
        f
    }

    #[test]
    fn finds_from_the_installer_record() {
        let f = with_record(r"C:\Users\t\AppData\Local\Programs\Foxglove", true);
        let found = find(&f, fox()).unwrap();
        assert_eq!(found.source, Source::Record);
        assert_eq!(found.version, "0.3.0");
        assert!(found.exe.ends_with("foxglove.exe"));
    }

    #[test]
    fn falls_back_to_the_known_folder_and_nothing_found_is_none() {
        let mut f = Fake::default();
        assert!(find(&f, fox()).is_none());
        f.env.insert("LOCALAPPDATA".into(), r"C:\L".into());
        f.files.insert(
            Path::new(r"C:\L")
                .join(r"Programs\Foxglove")
                .join("foxglove.exe"),
            ("0.3.1".into(), None),
        );
        let found = find(&f, fox()).unwrap();
        assert_eq!(found.source, Source::KnownFolder);
        // A record that points at a missing exe also falls through to the known folder.
        f.reg
            .insert((fox().record_key.clone(), "App".into()), r"D:\gone".into());
        assert_eq!(find(&f, fox()).unwrap().source, Source::KnownFolder);
    }

    #[test]
    fn a_record_on_a_mapped_or_removable_drive_is_never_a_find_and_no_file_is_asked_about() {
        let mut f = with_record("D:/Mapped/Foxglove", true);
        f.remote.push(PathBuf::from("D:/Mapped"));
        assert!(find(&f, fox()).is_none());
    }

    #[test]
    fn a_unc_record_is_never_a_find() {
        let f = with_record(r"\\server\share\Foxglove", true);
        assert!(find(&f, fox()).is_none());
    }

    #[test]
    fn only_the_installer_record_gives_peer_trust() {
        let rec = Found {
            id: "foxglove".into(),
            exe: r"C:\R\foxglove.exe".into(),
            version: "0.3.0".into(),
            source: Source::Record,
            folder: None,
            manifest: None,
        };
        let known = Found {
            id: "vulpes".into(),
            exe: r"C:\K\vulpes.exe".into(),
            version: "0.1.0".into(),
            source: Source::KnownFolder,
            folder: None,
            manifest: None,
        };
        let p = InstalledPeers {
            found: vec![rec, known],
            adopted: vec![r"C:\A\FOXGLOVE.EXE".into(), r"C:\A\other.exe".into()],
        };
        assert_eq!(
            p.installed_exe("foxglove.exe"),
            Some(PathBuf::from(r"C:\R\foxglove.exe"))
        );
        assert_eq!(p.installed_exe("vulpes.exe"), None);
        assert_eq!(
            p.run0("foxglove.exe"),
            vec![PathBuf::from(r"C:\A\FOXGLOVE.EXE")]
        );
        assert!(p.run0("hideout.exe").is_empty());
    }

    const VKEY: &str = r"Software\Vulpes\Installer";
    const VAPP: &str = r"C:\Users\t\AppData\Local\Programs\Arete";
    const VEXE: &str = r"C:\Users\t\AppData\Local\Programs\Arete\venv\Scripts\arete.exe";
    const VMAN: &str = r"C:\Users\t\AppData\Local\Programs\Arete\hideout\module.toml";

    fn vul() -> &'static Product {
        product("vulpes").unwrap()
    }

    /// A Vulpes machine as the installer leaves it: all four record values, a launcher with no
    /// file-version resource (empty version), and the rendered manifest.
    fn vulpes_machine() -> Fake {
        let mut f = Fake::default();
        for (k, v) in [
            ("App", VAPP),
            ("Exe", VEXE),
            ("Version", "0.9.2"),
            ("Manifest", VMAN),
        ] {
            f.reg.insert((VKEY.into(), k.into()), v.into());
        }
        f.files.insert(PathBuf::from(VEXE), (String::new(), None));
        f.files.insert(
            PathBuf::from(VMAN),
            (String::new(), Some("id = \"vulpes\"\n".into())),
        );
        f
    }

    /// Fake with a version resource only when `version` is non-empty.
    struct NoResource(Fake);
    impl Machine for NoResource {
        fn registry_string(&self, k: &str, v: &str) -> Option<String> {
            self.0.registry_string(k, v)
        }
        fn env(&self, n: &str) -> Option<String> {
            self.0.env(n)
        }
        fn is_file(&self, p: &Path) -> bool {
            self.0.is_file(p)
        }
        fn exe_version(&self, p: &Path) -> Option<String> {
            self.0.exe_version(p).filter(|v| !v.is_empty())
        }
        fn read_text(&self, p: &Path) -> Option<String> {
            self.0.read_text(p)
        }
        fn registry_subkeys(&self, k: &str) -> Vec<String> {
            self.0.registry_subkeys(k)
        }
        fn canonical(&self, p: &Path) -> Option<PathBuf> {
            self.0.canonical(p)
        }
        fn is_local_fixed(&self, p: &Path) -> bool {
            self.0.is_local_fixed(p)
        }
    }

    #[test]
    fn vulpes_record_gives_the_venv_exe_and_the_record_version() {
        let m = NoResource(vulpes_machine());
        let found = find(&m, vul()).unwrap();
        assert_eq!(found.source, Source::Record);
        assert_eq!(found.exe, PathBuf::from(VEXE));
        assert_eq!(
            found.version, "0.9.2",
            "no file-version resource: the record's Version"
        );
        assert_eq!(found.folder, Some(PathBuf::from(VAPP)));
        assert_eq!(found.manifest, Some(PathBuf::from(VMAN)));
    }

    #[test]
    fn a_file_version_beats_the_record_version() {
        let mut f = vulpes_machine();
        f.files.insert(PathBuf::from(VEXE), ("1.0.0".into(), None));
        assert_eq!(find(&NoResource(f), vul()).unwrap().version, "1.0.0");
    }

    #[test]
    fn without_exe_value_the_exe_comes_from_the_app_folder() {
        let mut f = vulpes_machine();
        f.reg.remove(&(VKEY.into(), "Exe".into()));
        f.reg.remove(&(VKEY.into(), "Manifest".into()));
        let found = find(&NoResource(f), vul()).unwrap();
        assert_eq!(found.source, Source::Record);
        assert_eq!(found.exe, PathBuf::from(VEXE));
        assert_eq!(found.version, "0.9.2");
        assert_eq!(found.manifest, None);
    }

    #[test]
    fn vulpes_known_folder_is_programs_arete_and_not_trusted() {
        let mut f = Fake::default();
        f.env
            .insert("LOCALAPPDATA".into(), r"C:\Users\t\AppData\Local".into());
        f.files.insert(PathBuf::from(VEXE), ("0.9.2".into(), None));
        let found = find(&f, vul()).unwrap();
        assert_eq!(found.source, Source::KnownFolder);
        assert_eq!(found.exe, PathBuf::from(VEXE));
        // A known folder never reads the record's Version, and gives no peer trust.
        let peers = InstalledPeers {
            found: vec![found],
            adopted: vec![],
        };
        assert_eq!(peers.installed_exe("arete.exe"), None);
        // No version resource and no record: nothing to report, so no find.
        let mut g = Fake::default();
        g.env
            .insert("LOCALAPPDATA".into(), r"C:\Users\t\AppData\Local".into());
        g.files.insert(PathBuf::from(VEXE), (String::new(), None));
        assert!(find(&NoResource(g), vul()).is_none());
    }

    #[test]
    fn record_values_on_a_network_share_are_refused() {
        // UNC Exe: skipped; the (local) App folder still finds the exe.
        let mut f = vulpes_machine();
        let unc = r"\\server\share\Arete\venv\Scripts\arete.exe";
        f.reg.insert((VKEY.into(), "Exe".into()), unc.into());
        f.files.insert(PathBuf::from(unc), ("9.9.9".into(), None));
        let found = find(&NoResource(f), vul()).unwrap();
        assert_eq!(found.exe, PathBuf::from(VEXE));
        // UNC Manifest: dropped, so the manifest falls back to the App folder.
        let mut g = vulpes_machine();
        g.reg.insert(
            (VKEY.into(), "Manifest".into()),
            r"\\server\share\module.toml".into(),
        );
        assert_eq!(find(&NoResource(g), vul()).unwrap().manifest, None);
        // Everything UNC: no find.
        let mut h = Fake::default();
        for (k, v) in [("App", r"\\s\a"), ("Exe", r"\\s\a\venv\Scripts\arete.exe")] {
            h.reg.insert((VKEY.into(), k.into()), v.into());
        }
        h.files.insert(
            PathBuf::from(r"\\s\a\venv\Scripts\arete.exe"),
            ("1.0.0".into(), None),
        );
        assert!(find(&h, vul()).is_none());
    }

    #[test]
    fn a_record_exe_with_another_name_is_not_taken() {
        let mut f = vulpes_machine();
        f.reg.insert(
            (VKEY.into(), "Exe".into()),
            r"C:\Windows\System32\cmd.exe".into(),
        );
        f.files.insert(
            PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            ("10.0.0".into(), None),
        );
        assert_eq!(
            find(&NoResource(f), vul()).unwrap().exe,
            PathBuf::from(VEXE)
        );
    }

    #[test]
    fn adopt_reads_the_manifest_from_the_record_or_the_app_folder_not_the_exe_folder() {
        let install = temp("vr-install");
        let exe = install.join(r"venv\Scripts\arete.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        let rendered = format!(
            "id =\"vulpes\"\nname = \"AI\"\nversion = \"0.9.2\"\nkind = \"adopted\"\n\
            family = \"vulpes\"\nhub = \"1.1\"\nrun = ['{}', 'hub']\n\
            api = {{ vulpes = \"1.0\" }}\n[activation]\non = [\"open\", \"call\"]\n",
            exe.display().to_string().replace('\\', "/")
        );
        let man = install.join(r"hideout\module.toml");
        let mut m = Fake::default();
        // Only the exe's own folder holds a manifest: not used.
        m.files.insert(
            exe.parent().unwrap().join(r"hideout\module.toml"),
            (String::new(), Some(rendered.clone())),
        );
        let mut found = Found {
            id: "vulpes".into(),
            exe: exe.clone(),
            version: "0.9.2".into(),
            source: Source::Record,
            folder: Some(install.clone()),
            manifest: None,
        };
        let no = adopt(
            &m,
            &temp("vr-h0"),
            &temp("vr-s0"),
            &found,
            "2026-11-18T08:00:00Z",
        );
        assert_eq!(no.unwrap_err(), "Vulpes has no manifest for Hideout yet");
        // The App folder's hideout\module.toml.
        m.files
            .insert(man.clone(), (String::new(), Some(rendered.clone())));
        let ok = adopt(
            &m,
            &temp("vr-h1"),
            &temp("vr-s1"),
            &found,
            "2026-11-18T08:00:00Z",
        );
        assert_eq!(
            ok.unwrap(),
            "registered vulpes 0.9.2 (installed separately)"
        );
        // The record's own Manifest value wins over the App folder.
        let other = install.join("elsewhere.toml");
        m.files.insert(man, (String::new(), None));
        m.files
            .insert(other.clone(), (String::new(), Some(rendered.clone())));
        found.manifest = Some(other);
        let ok = adopt(
            &m,
            &temp("vr-h2"),
            &temp("vr-s2"),
            &found,
            "2026-11-18T08:00:00Z",
        );
        assert_eq!(
            ok.unwrap(),
            "registered vulpes 0.9.2 (installed separately)"
        );
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hideout-h54-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(dir: &Path) -> BTreeSet<std::ffi::OsString> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect()
    }

    /// Adopting registers the module with this PC's exe, writes no grant, and leaves the
    /// product's own folder untouched. A refresh after an update registers the new version.
    #[test]
    fn adopt_registers_without_a_grant_and_leaves_the_install_alone() {
        let install = temp("install");
        let exe = install.join("foxglove.exe");
        std::fs::write(&exe, b"x").unwrap();
        let before = names(&install);
        let home = temp("home");
        let staging = temp("staging");
        let found = Found {
            id: "foxglove".into(),
            exe,
            version: "0.3.0".into(),
            source: Source::Record,
            folder: None,
            manifest: None,
        };
        let m = Fake::default();
        let line = adopt(&m, &home, &staging, &found, "2026-11-18T08:00:00Z").unwrap();
        assert_eq!(line, "registered foxglove 0.3.0 (installed separately)");
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        let rec = &reg.inventory.modules["foxglove"];
        assert!(rec.keyboard.is_none());
        assert!(rec.granted.is_empty());
        assert!(!rec.pending_permissions.is_empty(), "asks, never granted");
        drop(reg);
        assert!(!staging.join("foxglove").exists(), "staging is cleaned up");
        assert_eq!(before, names(&install));
        let newer = Found {
            version: "0.3.1".into(),
            ..found
        };
        assert_eq!(
            adopt(&m, &home, &staging, &newer, "2026-11-18T09:00:00Z").unwrap(),
            "registered foxglove 0.3.1 (installed separately)"
        );
    }

    #[test]
    fn a_product_without_a_manifest_is_refused_in_words() {
        let install = temp("v-install");
        let exe = install.join("vulpes.exe");
        std::fs::write(&exe, b"x").unwrap();
        let found = Found {
            id: "vulpes".into(),
            exe,
            version: "0.1.0".into(),
            source: Source::Record,
            folder: None,
            manifest: None,
        };
        let e = adopt(
            &Fake::default(),
            &temp("v-home"),
            &temp("v-staging"),
            &found,
            "2026-11-18T08:00:00Z",
        )
        .unwrap_err();
        assert_eq!(e, "Vulpes has no manifest for Hideout yet");
    }

    #[test]
    fn rows_for_every_state() {
        let mut m = Fake::default();
        let exe = PathBuf::from(r"C:\R\foxglove.exe");
        m.files.insert(exe.clone(), ("0.3.0".into(), None));
        let found = Found {
            id: "foxglove".into(),
            exe: exe.clone(),
            version: "0.3.0".into(),
            source: Source::Record,
            folder: None,
            manifest: None,
        };
        let a = |granted, version: &str| AdoptedFacts {
            run0: exe.clone(),
            version: version.into(),
            granted,
        };
        assert_eq!(row(None, None, false, &m), Row::GetIt);
        assert_eq!(row(Some(&found), None, true, &m), Row::UseInHideout);
        assert_eq!(
            row(Some(&found), Some(&a(false, "0.3.0")), false, &m),
            Row::RunsOnItsOwn
        );
        assert_eq!(
            row(Some(&found), Some(&a(false, "0.3.0")), true, &m),
            Row::WaitsForYourOk
        );
        assert_eq!(
            row(Some(&found), Some(&a(true, "0.3.0")), true, &m),
            Row::Ready
        );
        assert_eq!(
            row(Some(&found), Some(&a(true, "0.2.0")), true, &m),
            Row::NeedsRefresh
        );
        // The exe is gone: not installed, whatever the finder says.
        m.files.clear();
        assert_eq!(
            row(None, Some(&a(true, "0.3.0")), true, &m),
            Row::NotInstalled
        );
    }

    #[test]
    fn row_words_say_app_and_name_the_product() {
        assert_eq!(
            Row::RunsOnItsOwn.line("Foxglove"),
            "Foxglove \u{b7} Runs on its own"
        );
        assert_eq!(
            Row::WaitsForYourOk.line("Vulpes"),
            "Vulpes \u{b7} Waits for your OK"
        );
        assert_eq!(
            Row::GetIt.button("Foxglove").as_deref(),
            Some("Get Foxglove")
        );
        assert_eq!(
            Row::NotInstalled.button("x").as_deref(),
            Some("Remove from list")
        );
        assert!(Row::Ready.button("x").is_none());
    }
}

#[cfg(test)]
mod data_tests {
    use super::*;

    #[test]
    fn the_bundled_list_still_has_the_two_known_apps() {
        let ids: Vec<&str> = products().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["foxglove", "vulpes"]);
        assert!(matches!(
            product("foxglove").unwrap().manifest,
            ManifestSource::Builtin(_)
        ));
        assert!(matches!(
            product("vulpes").unwrap().manifest,
            ManifestSource::InstallFolder(_)
        ));
    }

    #[test]
    fn an_added_entry_is_an_app_with_no_code_change_and_bad_entries_are_skipped() {
        let list = parse_products(
            r#"{"products":[
              {"id":"gizmo","name":"Gizmo","exes":["gizmo.exe"],"record_key":"Software\\Gizmo\\Installer",
               "known":[["LOCALAPPDATA","Programs\\Gizmo"]],"manifest":{"install_folder":"hideout\\module.toml"}},
              {"id":"nokey","name":"No key","manifest":{"install_folder":"x"}},
              {"id":"odd","name":"Odd","record_key":"k","manifest":{"template":"no-such-template"}}
            ]}"#,
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "gizmo");
        assert_eq!(
            list[0].known,
            [("LOCALAPPDATA".to_string(), r"Programs\Gizmo".to_string())]
        );
        assert!(parse_products("not json").is_empty());
    }
}
