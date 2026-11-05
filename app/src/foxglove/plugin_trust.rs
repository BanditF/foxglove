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
//! plugin is trusted.
//!
//! **Sideloads (#381):** a plugin installed from a file that isn't in the signed catalog is
//! unsigned. Unsigned plugins install and run only in **developer mode**, a setting that is off
//! by default (`[plugins.dev]`), and each unsigned build still needs the user's one-time OK,
//! kept by its package hash (the SHA-256 of its files, the ones the catalog would sign). An
//! unsigned plugin that is already installed stays on disk but doesn't run, and its card says
//! why. Signed plugins are unaffected. `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1` (or the older
//! `FOXGLOVE_DEV_UNSIGNED_KITS=1`) turns developer mode on for one run, for the Test lab.
//!
//! **Grandfathered (#381):** while this build's root key is still the placeholder, the plugins
//! that were installed when this policy first ran (`[plugins.grandfather]`, by id and version)
//! keep running as before, and a health row says so. New installs from a file get the unsigned
//! flow at once. Once a real key ships the normal policy applies, and the Plugins page says so
//! once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use foxglove_plugin_trust::{Action, Catalog, Entry, Root, Seen};
use serde::{Deserialize, Serialize};

const ROOT_KEY: &str = include_str!("../../resources/plugin-trust/root.pub");

/// Files the runner writes into a plugin's folder itself.
pub const RUNTIME_FILES: &[&str] = &[
    "plugin.log",
    "kit.log",
    foxglove_plugin_host::runner::DATA_DIR,
];

/// Developer mode's warning, under its switch on the Plugins page (#381).
pub const DEV_MODE_WARNING: &str =
    "Lets Foxglove load plugins nobody has signed. Only turn this on for plugins you build yourself.";

/// Why an unsigned plugin doesn't run, on its card.
pub const OFF_UNSIGNED: &str =
    "Off: nobody has signed it, and unsigned plugins only run in developer mode.";
pub const OFF_NOT_ALLOWED: &str = "Off: you haven't allowed this unsigned build yet.";

/// Whether the environment turns developer mode on (the Test lab's switch).
pub fn dev_env() -> bool {
    [
        "FOXGLOVE_DEV_UNSIGNED_PLUGINS",
        "FOXGLOVE_DEV_UNSIGNED_KITS",
    ]
    .iter()
    .any(|k| std::env::var_os(k).is_some_and(|v| v == "1"))
}

/// The store's `[plugins.dev]`: developer mode, and the unsigned builds the user allowed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevMode {
    pub on: bool,
    /// Package hash to the plugin it was allowed for.
    pub allowed: BTreeMap<String, Allowed>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Allowed {
    pub id: String,
    pub version: String,
}

const DEV_SECTION: &str = "plugins.dev";

/// Developer mode as it stands: the setting, or the environment's override.
pub fn dev_mode() -> DevMode {
    let mut dev: DevMode = super::store::try_load(DEV_SECTION).unwrap_or_default();
    dev.on |= dev_env();
    dev
}

fn saved_dev() -> DevMode {
    super::store::try_load(DEV_SECTION).unwrap_or_default()
}

fn save_dev(dev: &DevMode) -> Result<(), String> {
    super::store::save(DEV_SECTION, dev).map_err(|e| e.to_string())
}

/// Turns developer mode on or off. The allowed builds are kept either way.
pub fn set_dev_mode(on: bool) -> Result<(), String> {
    save_dev(&DevMode { on, ..saved_dev() })
}

/// The user's one-time OK for one unsigned build.
pub fn allow(hash: &str, id: &str, version: &str) -> Result<(), String> {
    let mut dev = saved_dev();
    let entry = Allowed {
        id: id.into(),
        version: version.into(),
    };
    if dev.allowed.get(hash) == Some(&entry) {
        return Ok(());
    }
    dev.allowed.insert(hash.into(), entry);
    save_dev(&dev)
}

/// Forgets the builds allowed for a plugin that was removed.
pub fn forget(id: &str) -> Result<(), String> {
    let mut dev = saved_dev();
    let before = dev.allowed.len();
    dev.allowed.retain(|_, a| a.id != id);
    if dev.allowed.len() == before {
        return Ok(());
    }
    save_dev(&dev)
}

/// The SHA-256 of a plugin's files: each file's path and SHA-256, one per line, sorted, the way
/// the signed catalog lists them. Files the runner writes are left out.
pub fn package_hash(dir: &Path) -> Result<String, String> {
    let files = foxglove_plugin_trust::hash_dir(dir, RUNTIME_FILES).map_err(|e| e.to_string())?;
    let lines: String = files
        .iter()
        .map(|(path, sha)| format!("{path}\t{sha}\n"))
        .collect();
    Ok(foxglove_plugin_trust::sha256_hex(lines.as_bytes()))
}

/// What an installed plugin is.
#[derive(Clone, Debug, PartialEq)]
pub enum Trust {
    /// In the signed catalog, its files unchanged.
    Signed,
    /// Not signed (or changed since): its package hash.
    Unsigned(String),
    /// Not signed, but installed before this policy while the build has no signing key yet:
    /// runs as before. Its package hash.
    Grandfathered(String),
    /// Never runs: revoked, or its files can't be read.
    Refused(String),
}

/// Whether a root key is the placeholder a build carries until the real one exists (#90).
fn is_placeholder(key: &str) -> bool {
    key.trim_start().starts_with("PLACEHOLDER")
}

/// Whether this build's root key is still the placeholder.
pub fn signing_key_missing() -> bool {
    is_placeholder(ROOT_KEY)
}

/// The store's `[plugins.grandfather]`: the plugins installed when this policy first ran.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grandfather {
    /// The list was taken (once, the first time this policy ran).
    pub taken: bool,
    /// Id to the version that was installed then.
    pub plugins: BTreeMap<String, String>,
    /// The one-time line after a real signing key shipped was shown.
    pub notice_shown: bool,
}

const GRANDFATHER: &str = "plugins.grandfather";

/// The grandfathered plugins; the first time, the `installed` ones (id, version) are taken.
pub fn grandfather(installed: &[(String, String)]) -> Grandfather {
    let g: Grandfather = super::store::try_load(GRANDFATHER).unwrap_or_default();
    if g.taken {
        return g;
    }
    let g = Grandfather {
        taken: true,
        plugins: installed.iter().cloned().collect(),
        notice_shown: false,
    };
    if let Err(e) = super::store::save(GRANDFATHER, &g) {
        eprintln!("Foxglove: can't keep the plugins installed before #381: {e}");
    }
    g
}

/// A plugin installed again from a file, or removed, isn't grandfathered any more.
pub fn ungrandfather(id: &str) -> Result<(), String> {
    let mut g: Grandfather = super::store::try_load(GRANDFATHER).unwrap_or_default();
    if g.plugins.remove(id).is_none() {
        return Ok(());
    }
    super::store::save(GRANDFATHER, &g).map_err(|e| e.to_string())
}

/// `trust`, grandfathered when the key is still the placeholder and this exact version was
/// installed before the policy.
pub fn with_grandfather(
    trust: Trust,
    id: &str,
    version: &str,
    g: &Grandfather,
    key_missing: bool,
) -> Trust {
    match trust {
        Trust::Unsigned(hash) if key_missing && g.plugins.get(id).is_some_and(|v| v == version) => {
            Trust::Grandfathered(hash)
        }
        t => t,
    }
}

/// The health row while the key is the placeholder (#381).
pub const KEY_MISSING_HEALTH: &str = "plugin signing key not set; installed plugins allowed";

/// The one line on the Plugins page, once, after a real signing key shipped.
pub const SIGNING_ON_NOTICE: &str =
    "Plugin signing is on now: plugins nobody has signed only run in developer mode.";

/// The plugin-signing health rows for a build with or without its key.
pub fn health_rows_for(key_missing: bool) -> Vec<super::health::HealthRow> {
    use super::health::{HealthRow, Level};
    if key_missing {
        vec![HealthRow::new(
            "plugins",
            "Plugin signing",
            Level::Warn,
            KEY_MISSING_HEALTH,
        )]
    } else {
        vec![]
    }
}

/// The health view's provider.
pub fn health_rows() -> Vec<super::health::HealthRow> {
    health_rows_for(signing_key_missing())
}

/// The one-time line, if it's due: a real key, plugins that were grandfathered, not shown yet.
pub fn signing_notice(g: &Grandfather, key_missing: bool) -> Option<&'static str> {
    (!key_missing && g.taken && !g.plugins.is_empty() && !g.notice_shown)
        .then_some(SIGNING_ON_NOTICE)
}

/// The line was shown: never again.
pub fn notice_shown() -> Result<(), String> {
    let g: Grandfather = super::store::try_load(GRANDFATHER).unwrap_or_default();
    super::store::save(
        GRANDFATHER,
        &Grandfather {
            notice_shown: true,
            ..g
        },
    )
    .map_err(|e| e.to_string())
}

/// An installed plugin's trust against `catalog` (`None`: no signed catalog).
pub fn trust_in(catalog: Option<&Catalog>, id: &str, version: &str, dir: &Path) -> Trust {
    if let Some(c) = catalog {
        match c.runnable(id, version) {
            Ok(entry) => {
                if foxglove_plugin_trust::check_files(dir, entry, RUNTIME_FILES).is_ok() {
                    return Trust::Signed;
                }
            }
            Err(e)
                if c.revocation(id, version)
                    .is_some_and(|r| r.action == Action::Revoke) =>
            {
                return Trust::Refused(e)
            }
            Err(_) => {}
        }
    }
    match package_hash(dir) {
        Ok(hash) => Trust::Unsigned(hash),
        Err(e) => Trust::Refused(format!("can't read its files ({e})")),
    }
}

/// Whether a plugin with this trust may run now, or the line its card shows.
pub fn verdict(trust: &Trust, dev: &DevMode) -> Result<(), String> {
    match trust {
        Trust::Signed | Trust::Grandfathered(_) => Ok(()),
        Trust::Refused(e) => Err(format!("Off: {e}.")),
        Trust::Unsigned(_) if !dev.on => Err(OFF_UNSIGNED.into()),
        Trust::Unsigned(hash) if !dev.allowed.contains_key(hash) => Err(OFF_NOT_ALLOWED.into()),
        Trust::Unsigned(_) => Ok(()),
    }
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
    if is_placeholder(root_key) || set.root_json.is_empty() || set.catalog_json.is_empty() {
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

/// The signed entry for an archive about to be installed, by its hash: `Ok(None)` when the
/// archive isn't in the catalog (unsigned), an error when that version was yanked or revoked
/// (refused even in developer mode).
pub fn entry_in(catalog: &Catalog, archive: &[u8]) -> Result<Option<Entry>, String> {
    let hash = foxglove_plugin_trust::sha256_hex(archive);
    match catalog.installable(&hash) {
        Ok(entry) => Ok(Some(entry.clone())),
        Err(e)
            if catalog
                .plugins
                .iter()
                .any(|p| p.sha256.eq_ignore_ascii_case(&hash)) =>
        {
            Err(e)
        }
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn placeholder_build_trusts_nothing() {
        assert!(catalog().unwrap_err().contains("no signed plugin catalog"));
        assert!(accept_download(&Signed::default()).is_err());
    }

    /// A catalog with one plugin version, unsigned (verifying is plugin-trust's job).
    pub fn catalog_with(
        entries: Vec<Entry>,
        revocations: Vec<foxglove_plugin_trust::Revocation>,
    ) -> Catalog {
        Catalog {
            format: foxglove_plugin_trust::FORMAT_V2.into(),
            epoch: 0,
            sequence: 1,
            expires: "2099-01-01T00:00:00Z".into(),
            plugins: entries,
            revocations,
        }
    }

    pub fn entry_for(id: &str, version: &str, archive: &[u8], dir: &Path) -> Entry {
        Entry {
            id: id.into(),
            version: version.into(),
            channel: "stable".into(),
            sha256: foxglove_plugin_trust::sha256_hex(archive),
            url: None,
            size: None,
            files: foxglove_plugin_trust::hash_dir(dir, RUNTIME_FILES).unwrap(),
        }
    }

    /// #381: the three cases of the placeholder key: grandfathered plugins run and the health
    /// row warns; with a real key the normal policy and the one-time line; new installs and
    /// other versions are never grandfathered.
    #[test]
    fn grandfathering_while_the_key_is_the_placeholder() {
        assert!(
            is_placeholder(ROOT_KEY),
            "this build still has the placeholder"
        );
        assert!(signing_key_missing());
        assert!(!is_placeholder(
            "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
        ));
        let g = Grandfather {
            taken: true,
            plugins: [("org.example.old".to_string(), "1.0.0".to_string())].into(),
            notice_shown: false,
        };
        let off = DevMode::default();
        let unsigned = || Trust::Unsigned("h".into());

        // 1. Placeholder: the plugin installed before runs with developer mode off, the health
        //    row warns, and no line is due.
        let t = with_grandfather(unsigned(), "org.example.old", "1.0.0", &g, true);
        assert_eq!(t, Trust::Grandfathered("h".into()));
        assert_eq!(verdict(&t, &off), Ok(()));
        let rows = health_rows_for(true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].level, super::super::health::Level::Warn);
        assert_eq!(rows[0].detail, KEY_MISSING_HEALTH);
        assert_eq!(
            KEY_MISSING_HEALTH,
            "plugin signing key not set; installed plugins allowed"
        );
        assert_eq!(signing_notice(&g, true), None);

        // 2. A real key: the normal policy, no health row, and the line once.
        let t = with_grandfather(unsigned(), "org.example.old", "1.0.0", &g, false);
        assert_eq!(verdict(&t, &off), Err(OFF_UNSIGNED.into()));
        assert!(health_rows_for(false).is_empty());
        assert_eq!(signing_notice(&g, false), Some(SIGNING_ON_NOTICE));
        let shown = Grandfather {
            notice_shown: true,
            ..g.clone()
        };
        assert_eq!(signing_notice(&shown, false), None, "once");
        assert_eq!(
            signing_notice(
                &Grandfather {
                    taken: true,
                    ..Default::default()
                },
                false
            ),
            None,
            "nothing was grandfathered, nothing to say"
        );

        // 3. Another plugin or another version (a new install) isn't grandfathered.
        for (id, version) in [("org.example.new", "1.0.0"), ("org.example.old", "1.1.0")] {
            let t = with_grandfather(unsigned(), id, version, &g, true);
            assert_eq!(
                verdict(&t, &off),
                Err(OFF_UNSIGNED.into()),
                "{id} {version}"
            );
        }
        // Signed and refused plugins are left as they are.
        assert_eq!(
            with_grandfather(Trust::Signed, "org.example.old", "1.0.0", &g, true),
            Trust::Signed
        );
        let refused = Trust::Refused("revoked".into());
        assert_eq!(
            with_grandfather(refused.clone(), "org.example.old", "1.0.0", &g, true),
            refused
        );
    }

    #[test]
    fn trust_and_verdicts() {
        let dir = std::env::temp_dir().join(format!("foxglove-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), "x").unwrap();
        std::fs::write(dir.join("p.exe"), "MZ").unwrap();
        let entry = entry_for("org.example.p", "1.0.0", b"zip", &dir);
        let signed = catalog_with(vec![entry.clone()], vec![]);
        let off = DevMode::default();
        let on = DevMode {
            on: true,
            ..Default::default()
        };

        // Signed: runs whatever developer mode says; a log it writes changes nothing.
        std::fs::write(dir.join("plugin.log"), "hi").unwrap();
        let t = trust_in(Some(&signed), "org.example.p", "1.0.0", &dir);
        assert_eq!(t, Trust::Signed);
        assert_eq!(verdict(&t, &off), Ok(()));

        // Not in the catalog (or no catalog at all): unsigned, by its files' hash.
        let hash = package_hash(&dir).unwrap();
        let t = trust_in(None, "org.example.p", "1.0.0", &dir);
        assert_eq!(t, Trust::Unsigned(hash.clone()));
        assert_eq!(verdict(&t, &off), Err(OFF_UNSIGNED.into()));
        assert_eq!(verdict(&t, &on), Err(OFF_NOT_ALLOWED.into()));
        let mut allowed = on.clone();
        allowed.allowed.insert(hash.clone(), Allowed::default());
        assert_eq!(verdict(&t, &allowed), Ok(()));
        let mut off_allowed = allowed.clone();
        off_allowed.on = false;
        assert_eq!(
            verdict(&t, &off_allowed),
            Err(OFF_UNSIGNED.into()),
            "needs both"
        );

        // A signed plugin whose files changed is unsigned, with a new hash the user hasn't allowed.
        std::fs::write(dir.join("p.exe"), "MZ changed").unwrap();
        let t = trust_in(Some(&signed), "org.example.p", "1.0.0", &dir);
        assert!(matches!(&t, Trust::Unsigned(h) if *h != hash));
        assert_eq!(verdict(&t, &allowed), Err(OFF_NOT_ALLOWED.into()));

        // Revoked: never runs, not even in developer mode.
        let revoked = catalog_with(
            vec![entry.clone()],
            vec![foxglove_plugin_trust::Revocation {
                id: "org.example.p".into(),
                versions: vec![],
                action: Action::Revoke,
                reason: "malware".into(),
            }],
        );
        let t = trust_in(Some(&revoked), "org.example.p", "1.0.0", &dir);
        assert!(matches!(&t, Trust::Refused(e) if e.contains("revoked")));
        assert!(verdict(&t, &allowed).unwrap_err().starts_with("Off: "));

        // Installing: in the catalog, not, or withdrawn.
        assert_eq!(entry_in(&signed, b"zip"), Ok(Some(entry.clone())));
        assert_eq!(entry_in(&signed, b"other"), Ok(None));
        let yanked = catalog_with(
            vec![entry],
            vec![foxglove_plugin_trust::Revocation {
                id: "org.example.p".into(),
                versions: vec!["1.0.0".into()],
                action: Action::Yank,
                reason: String::new(),
            }],
        );
        assert!(entry_in(&yanked, b"zip").unwrap_err().contains("withdrawn"));
        let _ = std::fs::remove_dir_all(dir);
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
