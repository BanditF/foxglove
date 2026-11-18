// SPDX-License-Identifier: MIT
//! H5.4 (docs: review/design-h5-rebrand.md §2): finds an installed Foxglove or Vulpes and offers
//! to adopt it. UNWIRED: nothing calls this yet, so nothing is found, registered, started or
//! written on a user's PC. The module-mode go turns it on.
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
pub enum ManifestSource {
    /// A template shipped inside this build (Foxglove's own `hub_link/module.toml`).
    Builtin(&'static str),
    /// The template the product ships in its install folder, at this relative path.
    InstallFolder(&'static str),
}

/// One product Hideout can adopt. The one place that names them: a rename changes this table.
pub struct Product {
    pub id: &'static str,
    /// What the Modules page calls it.
    pub name: &'static str,
    /// Image file names, first match wins.
    pub exes: &'static [&'static str],
    /// The installer record: `HKCU\<key>`, value `App` = install folder.
    pub record_key: &'static str,
    /// Known folders, as environment variable plus relative folder.
    pub known: &'static [(&'static str, &'static str)],
    pub manifest: ManifestSource,
}

pub const PRODUCTS: &[Product] = &[
    Product {
        id: "foxglove",
        name: "Foxglove",
        exes: &["foxglove.exe"],
        record_key: r"Software\Foxglove\Installer",
        known: &[("LOCALAPPDATA", r"Programs\Foxglove")],
        manifest: ManifestSource::Builtin(include_str!(
            "../../../app/src/foxglove/hub_link/module.toml"
        )),
    },
    Product {
        id: "vulpes",
        name: "Vulpes",
        exes: &["vulpes.exe", "arete.exe"],
        record_key: r"Software\Vulpes\Installer",
        known: &[("LOCALAPPDATA", r"Programs\Vulpes")],
        manifest: ManifestSource::InstallFolder(r"hideout\module.toml"),
    },
];

pub fn product(id: &str) -> Option<&'static Product> {
    PRODUCTS.iter().find(|p| p.id == id)
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
    pub id: &'static str,
    pub exe: PathBuf,
    pub version: String,
    pub source: Source,
}

/// The installed copy of `p`, if any. A record that names a missing exe falls through to the
/// known folders; a UNC folder is never accepted.
pub fn find(m: &dyn Machine, p: &Product) -> Option<Found> {
    let mut folders: Vec<(PathBuf, Source)> = Vec::new();
    if let Some(dir) = m.registry_string(p.record_key, "App") {
        folders.push((PathBuf::from(dir), Source::Record));
    }
    for (var, rel) in p.known {
        if let Some(base) = m.env(var) {
            folders.push((Path::new(&base).join(rel), Source::KnownFolder));
        }
    }
    for (dir, source) in folders {
        if dir.to_string_lossy().starts_with(r"\\") {
            continue;
        }
        for name in p.exes {
            let exe = dir.join(name);
            if m.is_file(&exe) {
                if let Some(version) = m.exe_version(&exe) {
                    return Some(Found {
                        id: p.id,
                        exe,
                        version,
                        source,
                    });
                }
            }
        }
    }
    None
}

pub fn find_all(m: &dyn Machine) -> Vec<Found> {
    PRODUCTS.iter().filter_map(|p| find(m, p)).collect()
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
        ManifestSource::Builtin(t) => (*t).to_string(),
        ManifestSource::InstallFolder(rel) => {
            let at = f.exe.parent().unwrap_or(Path::new("")).join(rel);
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
    let p = product(found.id).ok_or_else(|| "not an app Hideout knows".to_string())?;
    let text = manifest_text(m, p, found)?;
    let dir = staging.join(p.id);
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
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

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
            std::fs::read_to_string(p).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default)]
    struct Fake {
        reg: BTreeMap<(String, String), String>,
        env: BTreeMap<String, String>,
        /// path -> (version, text)
        files: BTreeMap<PathBuf, (String, Option<String>)>,
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
    }

    fn fox() -> &'static Product {
        product("foxglove").unwrap()
    }

    fn with_record(dir: &str, exe_present: bool) -> Fake {
        let mut f = Fake::default();
        f.reg
            .insert((fox().record_key.into(), "App".into()), dir.to_string());
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
            .insert((fox().record_key.into(), "App".into()), r"D:\gone".into());
        assert_eq!(find(&f, fox()).unwrap().source, Source::KnownFolder);
    }

    #[test]
    fn a_unc_record_is_never_a_find() {
        let f = with_record(r"\\server\share\Foxglove", true);
        assert!(find(&f, fox()).is_none());
    }

    #[test]
    fn only_the_installer_record_gives_peer_trust() {
        let rec = Found {
            id: "foxglove",
            exe: r"C:\R\foxglove.exe".into(),
            version: "0.3.0".into(),
            source: Source::Record,
        };
        let known = Found {
            id: "vulpes",
            exe: r"C:\K\vulpes.exe".into(),
            version: "0.1.0".into(),
            source: Source::KnownFolder,
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
            id: "foxglove",
            exe,
            version: "0.3.0".into(),
            source: Source::Record,
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
            id: "vulpes",
            exe,
            version: "0.1.0".into(),
            source: Source::Record,
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
            id: "foxglove",
            exe: exe.clone(),
            version: "0.3.0".into(),
            source: Source::Record,
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
