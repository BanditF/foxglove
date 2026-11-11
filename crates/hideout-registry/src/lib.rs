// SPDX-License-Identifier: MIT
//! Hideout's module registry (H2.2, #933), written from `docs/hideout-contract.md` v1.1.
//!
//! - [`trust`]: the Hideout root chain, user anchors and catalogs (§12, §13.6)
//! - [`inventory`]: `inventory.json`, grants and `seen` (§7.3, §6.4)
//! - [`hub_config`]: `hideout.toml`, enabled state, sources and anchors (§7.3, §13.4)
//! - [`Registry`]: installs into versioned folders (§7.2), enables, and checks the catalog and
//!   every file before each launch (§12.5, §12.6); tracks what runs.
//! - [`catalogs`]: accepted catalogs saved per source and loaded at open; fetching a source
//!   once (§2.3, §12.4, §13.4, #1049).
//! - [`lab`]: lab mode, the first-party modules of `<config>\lab-modules\` in a lab hub only
//!   (#1001, docs/hideout-lab.md).
//!
//! A signature proves where files came from and never grants a permission (§6.5): installing
//! puts a module's permissions in `pending_permissions`, and only [`Registry::approve`] grants.
//! Nothing here starts a process or opens a window.

pub mod catalogs;
pub mod hub_config;
/// The installed ids for the hub's change watch (#1012.4).
pub mod installed;
pub mod inventory;
pub mod lab;
pub mod trust;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use foxglove_plugin_trust::hash_dir;
use hideout_contract::capability::Capability;
use hideout_contract::config::Anchor;
use hideout_contract::module::{read_dir, Context, Module, Visibility};
use hideout_contract::{Error, ErrorId, Version};

use hub_config::HubConfig;
use inventory::{Installed, Inventory, LAB, SIDELOAD};
use lab::LabMode;
use trust::{verify_catalog, verify_root, Accepted, Signer, TrustSet, ROOT_ANCHOR};

/// The folder under a module version that holds its own data, never hashed (§7.3).
pub const DATA_DIR: &str = "data";
/// At most this many versions of a module are kept (§7.2).
pub const MAX_VERSIONS: usize = 2;

/// Whether developer mode (unsigned sideload, §12.6) is on in this process's environment.
pub fn dev_mode_from_env() -> bool {
    ["HIDEOUT_DEV_UNSIGNED", "FOXGLOVE_DEV_UNSIGNED_PLUGINS"]
        .iter()
        .any(|v| std::env::var(v).is_ok_and(|s| s == "1"))
}

/// Why a registry step failed: a §15 check, or the file system.
#[derive(Debug)]
pub enum Failure {
    Check(Error),
    Io(io::Error),
    NotInstalled(String),
    Disabled(String),
    /// The step would overwrite or remove the version that runs (§7.2).
    Running(String),
    /// A catalog source couldn't be read: not configured, offline, no root key (#1049).
    Source(String),
}

impl Failure {
    /// The §15 id, when the failure is a contract check.
    pub fn id(&self) -> Option<&'static str> {
        match self {
            Failure::Check(e) => Some(e.id()),
            _ => None,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Failure::Check(e) => write!(f, "{e}"),
            Failure::Io(e) => write!(f, "{e}"),
            Failure::NotInstalled(id) => write!(f, "{id} is not installed"),
            Failure::Disabled(id) => write!(f, "{id} is off"),
            Failure::Running(id) => write!(f, "{id} is running"),
            Failure::Source(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Failure {}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Check(e)
    }
}

impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        Failure::Io(e)
    }
}

/// A module's state as the registry sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Off,
    Enabled,
    Running,
}

/// What a launch may start, after the checks passed.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub id: String,
    pub version: String,
    pub dir: PathBuf,
    pub run: Option<Vec<String>>,
    pub granted: Vec<String>,
    /// `[activation] on` (§4.6), default `["enable"]`.
    pub activation: Vec<String>,
    /// A legacy `transport = "stdio"` plugin, bridged by the hub (§8.2).
    pub stdio: bool,
}

/// A running version that an accepted catalog revoked (§12.4): the hub stops it, ends the
/// leases it holds with reason `"revoked"` and publishes `module.<id>.revoked` (§7.8, v1.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revoked {
    pub id: String,
    pub version: String,
    /// The catalog's `reason`, as the publisher wrote it (may be empty).
    pub reason: String,
}

pub struct Registry {
    home: PathBuf,
    pub inventory: Inventory,
    pub hub: HubConfig,
    pub trust: TrustSet,
    /// id to the running version.
    running: BTreeMap<String, String>,
    pub dev_mode: bool,
    /// The API a legacy `plugin.toml` is checked against (§4.10): the Foxglove plugin API
    /// (`foxglove_plugin_api::API_VERSION`, what `plugin-host` serves), not the hub's version.
    pub legacy_api: Version,
    /// Lab mode (#1001): the modules of `<home>\lab-modules\` are loaded, in memory only.
    pub lab: bool,
    /// One line per lab module folder: loaded, or why not. For the hub's log.
    pub lab_report: Vec<String>,
    /// Lab mode only: `lab-withdrawn.json`, the lab's stand-in for a catalog revocation of a
    /// lab module (#1026, [`lab::LAB_WITHDRAWN_FILE`]).
    lab_withdrawn: BTreeMap<String, lab::LabWithdrawn>,
    /// The Hideout root's public key root-anchored catalogs verify against (§12.2):
    /// [`trust::PINNED_ROOT_KEY`] unless a test sets another.
    pub root_key: Option<String>,
    /// The accepted `root.json` and its signature, stored with each root-anchored catalog.
    root_file: Option<(String, String)>,
    /// One line per source whose stored catalog was read at open: loaded, or why not. For the
    /// hub's log (#1049).
    pub catalog_report: Vec<String>,
    /// Per source: the sequence and accepted time of the catalog this registry verified and
    /// uses (loaded at open or accepted since). Copy diagnostics reads only this (#1049).
    accepted_info: BTreeMap<String, catalogs::CatalogInfo>,
}

fn hash_files(dir: &Path) -> io::Result<BTreeMap<String, String>> {
    hash_dir(dir, &[DATA_DIR])
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

impl Registry {
    /// Opens the registry in `home` (it holds `inventory.json`, `hideout.toml` and `modules\`)
    /// and loads each source's stored catalog ([`Registry::load_catalogs`], #1049).
    pub fn open(home: &Path, dev_mode: bool) -> Result<Registry, Failure> {
        let mut reg = Registry::open_bare(home, dev_mode)?;
        reg.load_catalogs(&foxglove_plugin_trust::now_utc());
        Ok(reg)
    }

    /// [`Registry::open`] without the stored catalogs (a lab hub reads no trust).
    fn open_bare(home: &Path, dev_mode: bool) -> Result<Registry, Failure> {
        let mut inventory = Inventory::load(&home.join("inventory.json"))?;
        // A lab entry is never on disk; one written there by hand is not a lab module.
        inventory.modules.retain(|_, m| m.source != LAB);
        let hub = HubConfig::load(&home.join("hideout.toml"))??;
        Ok(Registry {
            home: home.to_path_buf(),
            inventory,
            hub,
            trust: TrustSet::default(),
            running: BTreeMap::new(),
            dev_mode,
            legacy_api: foxglove_plugin_api::API_VERSION,
            lab: false,
            lab_report: Vec::new(),
            lab_withdrawn: BTreeMap::new(),
            root_key: trust::PINNED_ROOT_KEY.map(String::from),
            root_file: None,
            catalog_report: Vec::new(),
            accepted_info: BTreeMap::new(),
        })
    }

    /// [`Registry::open`] with developer mode and lab mode from this process's environment: what
    /// the hub and the supervisor use. In lab mode the lab modules are loaded
    /// ([`Registry::load_lab`]); a refused lab mode loads none.
    pub fn open_from_env(home: &Path) -> Result<Registry, Failure> {
        let lab = lab::lab_mode_from_env(home) == LabMode::On;
        // A lab hub reads no trust (docs/hideout-lab.md), so no stored catalog either.
        let mut reg = if lab {
            Registry::open_bare(home, dev_mode_from_env())?
        } else {
            Registry::open(home, dev_mode_from_env())?
        };
        if lab {
            reg.load_lab();
        }
        Ok(reg)
    }

    /// Lab mode (#1001, docs/hideout-lab.md): every `<home>\lab-modules\<id>\` whose manifest's
    /// id is its folder name joins the inventory in memory only, as source `lab`: its files
    /// hashed now and checked again before its launch, its permissions granted as for a
    /// sideload (never-list items left out, §6.6), enabled unless `hideout.toml` says
    /// otherwise. An id the inventory already holds keeps its installed version. Trust (the
    /// root, catalogs, anchors, `seen`) is not read or written.
    pub fn load_lab(&mut self) {
        self.lab = true;
        match lab::load_withdrawn(&self.home) {
            Ok(w) => {
                for (id, e) in &w {
                    self.lab_report.push(format!(
                        "{id}: {} reads as withdrawn (lab-withdrawn.json)",
                        e.version
                    ));
                }
                self.lab_withdrawn = w;
            }
            Err(e) => self
                .lab_report
                .push(format!("lab-withdrawn.json not read: {e}")),
        }
        let base = LabMode::modules_dir(&self.home);
        let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&base) {
            Ok(rd) => rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect(),
            Err(e) => {
                self.lab_report.push(format!(
                    "{}: not read ({e}); no lab modules",
                    base.display()
                ));
                return;
            }
        };
        dirs.sort();
        for dir in dirs {
            let line = match self.load_lab_one(&dir) {
                Ok(id) => format!("{id}: loaded from {}", dir.display()),
                Err(why) => format!("{}: not loaded: {why}", dir.display()),
            };
            self.lab_report.push(line);
        }
    }

    fn load_lab_one(&mut self, dir: &Path) -> Result<String, String> {
        let m = self.read_manifest(dir).map_err(|e| e.to_string())?;
        let folder = dir.file_name().unwrap_or_default().to_string_lossy();
        if folder != m.id.as_str() {
            return Err(format!("the folder must be named {}", m.id));
        }
        if self
            .inventory
            .modules
            .get(&m.id)
            .is_some_and(|i| i.source != LAB)
        {
            return Err(format!("{} is installed; the installed one runs", m.id));
        }
        let files = hash_files(dir).map_err(|e| e.to_string())?;
        self.inventory.modules.insert(
            m.id.clone(),
            Installed {
                version: m.version.clone(),
                kind: m.kind,
                family: m.family,
                visibility: m.visibility,
                source: LAB.into(),
                anchor: None,
                channel: "lab".into(),
                installed: String::new(),
                files,
                footprint: Vec::new(),
                previous: None,
                granted: Vec::new(),
                pending_permissions: Vec::new(),
            },
        );
        self.inventory.grant(&m.id, &m.permissions);
        if self.hub.enabled_set(&m.id).is_none() {
            // In memory: a lab module runs unless the lab's hideout.toml turns it off.
            self.hub.set_enabled(&m.id, true);
        }
        Ok(m.id)
    }

    /// Whether `id` is a lab module (source `lab`).
    pub fn is_lab(&self, id: &str) -> bool {
        self.inventory
            .modules
            .get(id)
            .is_some_and(|m| m.source == LAB)
    }

    pub fn module_dir(&self, id: &str, version: &str) -> PathBuf {
        if self.is_lab(id) {
            return LabMode::modules_dir(&self.home).join(id);
        }
        self.home.join("modules").join(id).join(version)
    }

    fn save_inventory(&self) -> io::Result<()> {
        hideout_acl::create_private_dir_all(&self.home)?;
        self.inventory.save(&self.home.join("inventory.json"))
    }

    /// Applies `f` to the hub config held here and to `hideout.toml` as it is on disk now, and
    /// saves that: only the change is written. A lab module's "on unless the file says
    /// otherwise" lives in memory only ([`Registry::load_lab`]); writing the whole held table
    /// would save it as a choice (#1076), and would write back over what another writer saved.
    fn change_hub(&mut self, f: impl Fn(&mut HubConfig)) -> Result<(), Failure> {
        f(&mut self.hub);
        let path = self.home.join("hideout.toml");
        let mut disk = HubConfig::load(&path)??;
        f(&mut disk);
        hideout_acl::create_private_dir_all(&self.home)?;
        disk.save(&path)?;
        Ok(())
    }

    /// Verifies `root.json` against the pinned key, stores the raised `seen`, and only then uses
    /// it (§12.4: if `seen` can't be stored the root is not used). Also checks `hideout.toml`'s
    /// sources and anchors against the root's scopes.
    pub fn accept_root(
        &mut self,
        json: &[u8],
        signature: &str,
        pinned_key: &str,
        now: &str,
    ) -> Result<(), Failure> {
        self.reload_inventory()?;
        let root = verify_root(
            json,
            signature,
            pinned_key,
            now,
            self.inventory.seen.get(ROOT_ANCHOR),
        )?;
        self.hub.validate(&root.scopes())?;
        // Stored beside every root-anchored catalog, so it must be text before it is used.
        let text = std::str::from_utf8(json)
            .map_err(|_| Error::new(ErrorId::ManifestSyntax, "root.json: not UTF-8"))?;
        let mut next = self
            .inventory
            .seen
            .get(ROOT_ANCHOR)
            .cloned()
            .unwrap_or_default();
        next.record_root(&root);
        // The other root-anchored sources' saved catalogs move to this root (checked against
        // it, saved with it) before `seen` is raised, so no saved file keeps a root that the
        // raised `seen` refuses at the next open. One that doesn't verify under it is dropped.
        let kept = self.rebase_root_catalogs(&root, text, signature, &next, now);
        self.raise_seen(|seen| {
            seen.entry(ROOT_ANCHOR.into())
                .or_default()
                .record_root(&root)
        })?;
        let dropped: Vec<String> = self
            .trust
            .catalogs
            .iter()
            .filter(|a| a.anchor == ROOT_ANCHOR && !kept.iter().any(|k| k.source == a.source))
            .map(|a| a.source.clone())
            .collect();
        for s in &dropped {
            self.accepted_info.remove(s);
        }
        self.trust.catalogs.retain(|a| a.anchor != ROOT_ANCHOR);
        self.trust.catalogs.extend(kept);
        self.trust.root = Some(root);
        self.root_file = Some((text.to_string(), signature.to_string()));
        Ok(())
    }

    fn user_anchor(&self, id: &str) -> Result<Anchor, Error> {
        self.hub
            .config
            .trust
            .anchors
            .iter()
            .find(|a| a.id == id && a.kind == "user")
            .cloned()
            .ok_or_else(|| Error::new(ErrorId::ConfigUnknownAnchor, format!("no anchor {id}")))
    }

    /// Verifies a catalog from `source` against its anchor (§12.4, §13.6), saves it under
    /// `catalogs\<source>\` (with its sequence and `now` as the accepted time, #1049), stores
    /// `seen`, then adds it to the trust set. If either can't be written it is not used.
    pub fn accept_catalog(
        &mut self,
        json: &[u8],
        signature: &str,
        anchor: &str,
        source: &str,
        visibility: Visibility,
        now: &str,
    ) -> Result<(), Failure> {
        // `seen` as saved now, not as read at open: another writer may have raised it.
        self.reload_inventory()?;
        let seen = self.inventory.seen.get(anchor).cloned();
        let catalog = if anchor == ROOT_ANCHOR {
            let root = self.trust.root.as_ref().ok_or_else(|| {
                Error::new(ErrorId::TrustSignature, "no verified root to check against")
            })?;
            verify_catalog(
                json,
                signature,
                Signer::Root(root),
                source,
                now,
                seen.as_ref(),
            )?
        } else {
            let a = self.user_anchor(anchor)?;
            let signer = Signer::User {
                key: &a.key,
                scope: &a.scope,
            };
            verify_catalog(json, signature, signer, source, now, seen.as_ref())?
        };
        self.store_catalog(json, signature, anchor, source, &catalog, now)?;
        self.raise_seen(|seen| {
            seen.entry(anchor.into())
                .or_default()
                .record_catalog(source, &catalog)
        })?;
        self.accepted_info.insert(
            source.into(),
            catalogs::CatalogInfo {
                sequence: catalog.sequence,
                accepted: now.into(),
            },
        );
        self.trust
            .catalogs
            .retain(|a| !(a.anchor == anchor && a.source == source));
        self.trust.catalogs.push(Accepted {
            anchor: anchor.into(),
            source: source.into(),
            visibility,
            catalog,
        });
        Ok(())
    }

    /// The lab entries this registry holds in memory only (never on disk).
    fn lab_entries(&self) -> Vec<(String, Installed)> {
        self.inventory
            .modules
            .iter()
            .filter(|(_, m)| m.source == LAB)
            .map(|(k, m)| (k.clone(), m.clone()))
            .collect()
    }

    /// `seen` held here joined into `into` (each anchor at the higher of the two).
    fn join_seen(&self, into: &mut BTreeMap<String, trust::SeenAnchor>) {
        for (anchor, s) in &self.inventory.seen {
            into.entry(anchor.clone()).or_default().merge(s);
        }
    }

    /// Reads `inventory.json` again (lab entries kept): what another writer saved since open,
    /// with `seen` at the higher of the file's and this registry's.
    fn reload_inventory(&mut self) -> io::Result<()> {
        let lab = self.lab_entries();
        let mut disk = Inventory::load(&self.home.join("inventory.json"))?;
        disk.modules.retain(|_, m| m.source != LAB);
        disk.modules.extend(lab);
        self.join_seen(&mut disk.seen);
        self.inventory = disk;
        Ok(())
    }

    /// Raises `seen` in the file under `inventory.lock` (read, change, write), then holds the
    /// file's inventory: nothing another writer saved is written over, and `seen` never drops.
    fn raise_seen(
        &mut self,
        f: impl FnOnce(&mut BTreeMap<String, trust::SeenAnchor>),
    ) -> io::Result<()> {
        hideout_acl::create_private_dir_all(&self.home)?;
        let lab = self.lab_entries();
        let mut held = BTreeMap::new();
        self.join_seen(&mut held);
        let mut disk = Inventory::update(&self.home.join("inventory.json"), |inv| {
            for (anchor, s) in &held {
                inv.seen.entry(anchor.clone()).or_default().merge(s);
            }
            f(&mut inv.seen)
        })?;
        disk.modules.extend(lab);
        self.inventory = disk;
        Ok(())
    }

    fn read_manifest(&self, dir: &Path) -> Result<Module, Failure> {
        let (_, m) = read_dir(dir, &Context::default(), self.legacy_api)?;
        Ok(m?)
    }

    /// Where an install of `m` comes from: an accepted catalog entry whose files `found` must
    /// match exactly, or a developer-mode sideload (§12.5, §12.6).
    fn provenance(
        &self,
        m: &Module,
        found: &BTreeMap<String, String>,
    ) -> Result<(String, Option<String>, String), Failure> {
        match self.trust.installable(&m.id, &m.version) {
            Ok((accepted, entry)) => {
                entry.check_manifest(m)?;
                entry.check_source(accepted.visibility)?;
                trust::check_files(found, &entry.files)?;
                Ok((
                    accepted.source.clone(),
                    Some(accepted.anchor.clone()),
                    entry.channel.clone(),
                ))
            }
            Err(e) if e.id == ErrorId::TrustUnknownArchive => {
                self.sideload_ok(m)?;
                Ok((SIDELOAD.into(), None, "dev".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// A module outside every catalog: developer mode on, and an id outside every first-party
    /// scope (§12.6).
    fn sideload_ok(&self, m: &Module) -> Result<(), Error> {
        if !self.dev_mode {
            return Err(Error::new(
                ErrorId::InstallDevModeRequired,
                format!("{} is in no catalog and developer mode is off", m.id),
            ));
        }
        let in_root = self.trust.root.as_ref().is_some_and(|r| r.covers(&m.id));
        if in_root || m.family.first_party() {
            return Err(Error::new(
                ErrorId::TrustSideloadScope,
                format!("{} is a first-party id", m.id),
            ));
        }
        Ok(())
    }

    /// Installs the module folder `folder` into `modules\<id>\<version>\` (§7.2). The running
    /// version is never overwritten; the previous one is kept for rollback, older ones removed.
    /// Grants carry over; permissions not yet granted wait in `pending_permissions`.
    pub fn install(&mut self, folder: &Path, now: &str) -> Result<Installed, Failure> {
        let m = self.read_manifest(folder)?;
        let found = hash_files(folder)?;
        let (source, anchor, channel) = self.provenance(&m, &found)?;
        if self.running.get(&m.id) == Some(&m.version) {
            return Err(Failure::Running(m.id));
        }
        let dir = self.module_dir(&m.id, &m.version);
        let partial = dir.with_extension("partial");
        let _ = std::fs::remove_dir_all(&partial);
        copy_dir(folder, &partial)?;
        let copied = hash_files(&partial)?;
        if copied != found {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(Error::new(ErrorId::TrustFiles, "the folder changed while copying").into());
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::rename(&partial, &dir)?;

        // A lab entry's grants never carry over into a real install (docs/hideout-lab.md).
        let old = self
            .inventory
            .modules
            .get(&m.id)
            .filter(|o| o.source != LAB)
            .cloned();
        let previous = old
            .as_ref()
            .map(|o| o.version.clone())
            .filter(|v| *v != m.version)
            .or_else(|| old.as_ref().and_then(|o| o.previous.clone()));
        let entry = Installed {
            version: m.version.clone(),
            kind: m.kind,
            family: m.family,
            visibility: m.visibility,
            source,
            anchor,
            channel,
            installed: now.into(),
            files: found,
            footprint: old
                .as_ref()
                .map(|o| o.footprint.clone())
                .unwrap_or_default(),
            previous: previous.clone(),
            granted: old.as_ref().map(|o| o.granted.clone()).unwrap_or_default(),
            pending_permissions: Vec::new(),
        };
        self.inventory.modules.insert(m.id.clone(), entry);
        self.inventory.ask(&m.id, &m.permissions);
        self.prune(&m.id, &m.version, previous.as_deref())?;
        self.save_inventory()?;
        Ok(self.inventory.modules[&m.id].clone())
    }

    /// Removes version folders of `id` other than `keep` and `previous` (and the running one).
    fn prune(&self, id: &str, keep: &str, previous: Option<&str>) -> io::Result<()> {
        let base = self.home.join("modules").join(id);
        let running = self.running.get(id).map(String::as_str);
        for entry in std::fs::read_dir(&base)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name != keep && Some(name.as_str()) != previous && Some(name.as_str()) != running {
                std::fs::remove_dir_all(base.join(&name))?;
            }
        }
        Ok(())
    }

    /// The installed manifest of `id` (its current version), without the file check.
    pub fn manifest(&self, id: &str) -> Option<Module> {
        let m = self.inventory.modules.get(id)?;
        self.read_manifest(&self.module_dir(id, &m.version)).ok()
    }

    /// Automatic rollback (§7.8): the previous version becomes the current one and the one that
    /// crashed becomes `previous`. Only a catalog module whose previous folder is still there;
    /// the launch check runs again before it starts. Returns the version it went back to.
    pub fn roll_back(&mut self, id: &str) -> Result<String, Failure> {
        let home = self.home.clone();
        let m = self
            .inventory
            .modules
            .get_mut(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        let Some(prev) = m.previous.clone() else {
            return Err(Failure::NotInstalled(format!(
                "{id} has no previous version"
            )));
        };
        if m.source == SIDELOAD || !home.join("modules").join(id).join(&prev).is_dir() {
            return Err(Failure::NotInstalled(format!("{id} {prev}")));
        }
        // §7.4: a revoked version is never gone back to.
        if self.trust.revocation(id, &prev).is_some() {
            return Err(Error::new(
                ErrorId::TrustRevoked,
                format!("{id}@{prev} is revoked; not gone back to"),
            )
            .into());
        }
        let m = self
            .inventory
            .modules
            .get_mut(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        m.previous = Some(std::mem::replace(&mut m.version, prev.clone()));
        self.save_inventory()?;
        Ok(prev)
    }

    /// Drops the previous version once the new one ran cleanly for a day (§7.2).
    pub fn drop_previous(&mut self, id: &str) -> Result<(), Failure> {
        let m = self
            .inventory
            .modules
            .get_mut(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        if let Some(prev) = m.previous.take() {
            if self.running.get(id) == Some(&prev) {
                m.previous = Some(prev);
                return Err(Failure::Running(id.into()));
            }
            let dir = self.module_dir(id, &prev);
            if dir.exists() {
                std::fs::remove_dir_all(dir)?;
            }
        }
        self.save_inventory()?;
        Ok(())
    }

    /// The user approved the permission dialog (§6.4): the pending permissions are granted,
    /// except never-list items, which are returned (`permission.never`).
    pub fn approve(&mut self, id: &str) -> Result<Vec<Error>, Failure> {
        let m = self
            .inventory
            .modules
            .get(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        let asked: Vec<Capability> = m
            .pending_permissions
            .iter()
            .map(|p| Capability::parse(p))
            .collect::<Result<_, _>>()?;
        let refused = self.inventory.grant(id, &asked).unwrap_or_default();
        if let Some(m) = self.inventory.modules.get_mut(id) {
            m.pending_permissions.clear();
        }
        self.save_inventory()?;
        Ok(refused)
    }

    /// Sets or clears `hideout.toml`'s `safe_mode` flag (§7.10) and saves the file.
    pub fn set_safe_mode(&mut self, on: bool) -> Result<(), Failure> {
        self.change_hub(|h| h.set_safe_mode(on))
    }

    /// Saves `[modules.<id>] enabled` in `hideout.toml`, a lab module's too (in a registry
    /// opened in lab mode, [`Registry::open_from_env`]).
    pub fn set_enabled(&mut self, id: &str, on: bool) -> Result<(), Failure> {
        if !self.inventory.modules.contains_key(id) {
            return Err(Failure::NotInstalled(id.into()));
        }
        self.change_hub(|h| h.set_enabled(id, on))
    }

    /// Removes a module that isn't running: its folders, inventory entry and hub table.
    pub fn remove(&mut self, id: &str) -> Result<(), Failure> {
        if self.running.contains_key(id) {
            return Err(Failure::Running(id.into()));
        }
        if self.inventory.modules.remove(id).is_none() {
            return Err(Failure::NotInstalled(id.into()));
        }
        let dir = self.home.join("modules").join(id);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        self.save_inventory()?;
        self.change_hub(|h| h.forget(id))
    }

    /// The check before every launch (§12.5, §12.6): installed and enabled; a catalog module's
    /// entry still accepted and not revoked, and its folder exactly the signed files; a sideload
    /// only in developer mode, against the inventory's hashes. Starts nothing.
    pub fn pre_launch(&self, id: &str) -> Result<Launch, Failure> {
        let m = self
            .inventory
            .modules
            .get(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        if !self.hub.enabled(id) {
            return Err(Failure::Disabled(id.into()));
        }
        let dir = self.module_dir(id, &m.version);
        let found = hash_files(&dir)?;
        if m.source == LAB {
            // Only a lab hub loads one; its files must be what was loaded (docs/hideout-lab.md).
            if !self.lab {
                return Err(Error::new(
                    ErrorId::InstallDevModeRequired,
                    format!("{id} is a lab module and lab mode is off"),
                )
                .into());
            }
            trust::check_files(&found, &m.files)?;
        } else if m.source == SIDELOAD {
            if !self.dev_mode {
                return Err(Error::new(
                    ErrorId::InstallDevModeRequired,
                    format!("{id} is a sideload and developer mode is off"),
                )
                .into());
            }
            trust::check_files(&found, &m.files)?;
        } else {
            let (accepted, entry) = self.trust.runnable(id, &m.version)?;
            if Some(&accepted.anchor) != m.anchor.as_ref() {
                return Err(Error::new(
                    ErrorId::TrustSignature,
                    format!("{id} was installed from another anchor"),
                )
                .into());
            }
            trust::check_files(&found, &entry.files)?;
        }
        let manifest = self.read_manifest(&dir)?;
        Ok(Launch {
            id: id.into(),
            version: m.version.clone(),
            dir,
            run: manifest.run,
            granted: m.granted.clone(),
            activation: if manifest.activation_on.is_empty() {
                vec!["enable".into()]
            } else {
                manifest.activation_on
            },
            stdio: manifest.transport == hideout_contract::module::Transport::Stdio,
        })
    }

    /// The hub started `launch` (after [`Registry::pre_launch`]).
    pub fn mark_running(&mut self, launch: &Launch) {
        self.running
            .insert(launch.id.clone(), launch.version.clone());
    }

    pub fn mark_stopped(&mut self, id: &str) {
        self.running.remove(id);
    }

    pub fn state(&self, id: &str) -> State {
        if !self.inventory.modules.contains_key(id) {
            State::NotInstalled
        } else if self.running.contains_key(id) {
            State::Running
        } else if self.hub.enabled(id) {
            State::Enabled
        } else {
            State::Off
        }
    }

    /// The revocation of `id@version` (§12.4), if an accepted catalog of its source revoked it
    /// (a yank is not one). Sideloads have no catalog and are never revoked; a lab module only
    /// through the lab's `lab-withdrawn.json` (lab mode, #1026).
    fn revocation_at(&self, id: &str, version: &str) -> Option<Revoked> {
        let m = self.inventory.modules.get(id)?;
        if m.source == LAB {
            return self
                .lab_withdrawn
                .get(id)
                .filter(|w| self.lab && w.version == version)
                .map(|w| Revoked {
                    id: id.into(),
                    version: version.into(),
                    reason: w.reason.clone(),
                });
        }
        if m.source == SIDELOAD {
            return None;
        }
        self.trust.revocation(id, version).map(|r| Revoked {
            id: id.into(),
            version: version.into(),
            reason: r.reason.clone(),
        })
    }

    /// Whether what withdraws `id` can be read now: its source's verified catalog is loaded (a
    /// lab module's `lab-withdrawn.json` and a sideload always can). When it can't (offline
    /// with the stored catalog expired, corrupt or unread), a withdrawal shown stays (#1049).
    pub fn withdrawals_known(&self, id: &str) -> bool {
        match self.inventory.modules.get(id) {
            None => true,
            Some(m) if m.source == LAB || m.source == SIDELOAD => true,
            Some(m) => self.catalog_loaded(&m.source),
        }
    }

    /// The revocation of `id`'s installed version, running or not (#1026): the supervisor
    /// stops it and won't start it (state `revoked`, §7.8 v1.9).
    pub fn revocation_of(&self, id: &str) -> Option<Revoked> {
        let v = &self.inventory.modules.get(id)?.version;
        self.revocation_at(id, v)
    }

    /// The version a withdrawn module's card offers in "Go back to <version>" (#1026): the kept
    /// previous version, when its folder is still there and it isn't revoked itself (a yanked
    /// one still runs, §12.4). `None` for sideloads, which never roll back.
    pub fn go_back_version(&self, id: &str) -> Option<String> {
        let m = self.inventory.modules.get(id)?;
        let prev = m.previous.clone()?;
        if m.source == SIDELOAD || m.source == LAB {
            return None;
        }
        if !self.home.join("modules").join(id).join(&prev).is_dir() {
            return None;
        }
        self.revocation_at(id, &prev).is_none().then_some(prev)
    }

    /// Running modules whose version a newly accepted catalog revoked: the hub stops them.
    pub fn revoked_running(&self) -> Vec<String> {
        self.revoked().into_iter().map(|r| r.id).collect()
    }

    /// [`Registry::revoked_running`] with the version and the catalog's reason, for the hub's
    /// `lease.closed {reason: "revoked"}` and the `module.<id>.revoked` fact (v1.6). Sideloads
    /// and lab modules have no catalog and are never revoked.
    pub fn revoked(&self) -> Vec<Revoked> {
        self.running
            .iter()
            .filter_map(|(id, v)| self.revocation_at(id, v))
            .collect()
    }
}
