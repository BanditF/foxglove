// SPDX-License-Identifier: MIT
//! `inventory.json` (spec §7.3): what is installed, the grants (§6.4) and the `seen`
//! anti-rollback state. Written atomically under `inventory.lock`; a file in an unknown format is
//! read but never written.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use hideout_contract::capability::{check_grantable, Capability, Holder, Source};
use hideout_contract::module::{Family, Kind, Visibility};
use hideout_contract::Error;
use serde::{Deserialize, Serialize};

use crate::trust::{SeenAnchor, ROOT_ANCHOR};

pub const FORMAT: &str = "hideout-inventory-v1";
/// The source id reserved for developer-mode installs (§12.6).
pub const SIDELOAD: &str = "sideload";
/// The source of a lab module (`<config>\lab-modules\`, docs/hideout-lab.md): held in memory
/// only, never written to `inventory.json`.
pub const LAB: &str = "lab";
/// The source of an `adopted` module (§3): an install the hub registered but did not make. It
/// has no files of its own to hash; the hub keeps only a copy of its `module.toml`.
pub const ADOPTED: &str = "adopted";

/// One installed module.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub version: String,
    pub kind: Kind,
    pub family: Family,
    pub visibility: Visibility,
    pub source: String,
    /// The trust anchor its catalog verified against; none for a sideload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(default)]
    pub channel: String,
    /// The canonical folder a sideload was installed from (trust model F2). None for every other
    /// source, and for a sideload recorded before this field existed: that one fails closed, so
    /// the next install of it from any folder asks again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sideload_folder: Option<String>,
    pub installed: String,
    /// Every file outside `data\` and its SHA-256.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub footprint: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// Canonical, sorted, unique (§6.4).
    #[serde(default)]
    pub granted: Vec<String>,
    #[serde(default)]
    pub pending_permissions: Vec<String>,
    /// Asked for and not allowed: declined by the user, or refused (never-list, overlap). Not
    /// pending, so they never block a start (#1178).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declined_permissions: Vec<String>,
    /// What the user had allowed before a void, the dialog's default (#1178).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_choice: Vec<String>,
    /// What the grant is bound to (#1178); none for a grant made before the binding existed. For
    /// an adopted record that now counts as void: its grants go back to pending and are asked
    /// again. The same holds for a binding with an empty program hash that still holds grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<Bound>,
    /// This run's OK of an app whose program runs whatever it is handed (#1452,
    /// [`crate::one_run`]). Such an app never has a `bound`: nothing is remembered for it, and
    /// this record is used up by the next start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_run: Option<RunOk>,
    /// The user's grant of the reserved areas to the adopted legacy module (#1155-B): path and
    /// hash of its program at Allow. Only [`Registry::allow_keyboard`] writes `on`.
    ///
    /// [`Registry::allow_keyboard`]: crate::Registry::allow_keyboard
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::keyboard::lenient"
    )]
    pub keyboard: Option<crate::keyboard::KeyboardGrant>,
    /// Who this module is, for the trust model (S1): dormant. Nothing reads it yet; none = a
    /// record from before it existed, which behaves exactly as it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

/// The identity a module's grants are bound to (trust model §3-4). Dormant in S1: it is stored
/// and read back, and no check uses it. The module id is the map key and is not repeated.
///
/// The class is the `class` tag; an unknown future class reads as [`IdentityClass::Unknown`]
/// (kept, never matching any known one) and fields this version does not know are ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "kebab-case")]
pub enum IdentityClass {
    /// Adopted by path (today's path + SHA-256), not signed by anyone.
    Adopted,
    /// A developer folder (dev mode, S5): reserved, nothing creates it yet.
    DevFolder {
        #[serde(default)]
        folder: Option<String>,
    },
    /// A dev-mode install copied from `folder` (canonical source folder).
    Sideload {
        #[serde(default)]
        folder: Option<String>,
    },
    /// Hub-installed from a catalog source: the anchor id, the SHA-256 of the anchor key, and
    /// the family.
    Catalog {
        #[serde(default)]
        anchor: Option<String>,
        #[serde(default)]
        key_fpr: Option<String>,
        #[serde(default)]
        family: Option<String>,
    },
    /// A publisher-signed install (Foxglove MSI + updater): dormant until the update key exists.
    InstalledSigned {
        #[serde(default)]
        key_fpr: Option<String>,
        #[serde(default)]
        folder: Option<String>,
    },
    /// A class written by a newer version.
    #[serde(other)]
    Unknown,
}

/// The identity record (`Installed.identity`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    #[serde(flatten)]
    pub class: IdentityClass,
    /// The manifest's `kind`, `visibility` and `family` at the Allow, as written in the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub established_at: Option<String>,
    /// Always `dialog` once written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub established_by: Option<String>,
    /// Sensitive items the user confirmed under this identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive_confirmed: Vec<String>,
}

/// The list, manifest and program a grant was made for (#1178, contract §6.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    pub manifest_sha256: String,
    /// Canonical path of `run[0]`; empty for a module the hub installed itself.
    #[serde(default)]
    pub exe_path: String,
    /// SHA-256 of the program; cleared once void (a void binding never matches again).
    #[serde(default)]
    pub exe_sha256: String,
    pub list_hash: String,
    pub at: String,
    /// `dialog` or `cli`.
    pub by: String,
    /// `run[1..]` (the program's arguments) at the allow; empty = no arguments, and the check
    /// holds the program to that.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run_args: Vec<String>,
    /// The program changed or moved since the allow.
    #[serde(default)]
    pub void: bool,
    /// Program changes the install or update path attested, oldest first (#1178): the grant
    /// follows the install, not one file hash. Shown on the Modules page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attested: Vec<Attestation>,
    /// "Keep my OK after updates in this folder" (#1178 follow-up, design note option A): a new
    /// hash of the same file is kept, and recorded in `attested` with `by: "folder"`, while the
    /// program stays at this exact canonical path on a local fixed drive with the same
    /// `run[1..]`. Set only by an Allow from the hub's dialog ([`Registry::approve_listed`]
    /// with `follow`); none = the hash binds as before.
    ///
    /// [`Registry::approve_listed`]: crate::Registry::approve_listed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow: Option<FollowFolder>,
}

/// The OK of one run (#1452): what the user allowed and for which program and arguments, held
/// until the app's next start has used it. It is not a binding: nothing keeps it (no
/// follow-folder, no attested update, no dev folder), and after one start it allows nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOk {
    pub manifest_sha256: String,
    /// Canonical path of `run[0]` at the Allow.
    #[serde(default)]
    pub exe_path: String,
    /// SHA-256 of the program at the Allow; cleared once the OK is over.
    #[serde(default)]
    pub exe_sha256: String,
    pub list_hash: String,
    pub at: String,
    /// `dialog` or `cli`.
    pub by: String,
    /// `run[1..]` at the Allow; the start is held to exactly these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run_args: Vec<String>,
    pub state: RunState,
}

/// Where a one-run OK stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    /// Allowed; the app has not started since.
    Allowed,
    /// The start it was given for was made. The next start ends it.
    Started,
    /// Used up by a start: Hideout asks again.
    Ended,
    /// The program or its arguments changed before or during the run.
    Void,
    /// A state this hub does not know (a newer or edited file): never an OK.
    #[serde(other)]
    Unknown,
}

impl RunOk {
    /// Whether the OK still allows something: given, and not yet over.
    pub fn live(&self) -> bool {
        matches!(self.state, RunState::Allowed | RunState::Started) && !self.exe_sha256.is_empty()
    }

    /// The record in the shape of a binding, for the readers that show when and how the app was
    /// allowed. Never stored: it keeps nothing and follows nothing.
    pub fn as_bound(&self) -> Bound {
        Bound {
            manifest_sha256: self.manifest_sha256.clone(),
            exe_path: self.exe_path.clone(),
            exe_sha256: self.exe_sha256.clone(),
            list_hash: self.list_hash.clone(),
            at: self.at.clone(),
            by: self.by.clone(),
            run_args: self.run_args.clone(),
            void: !self.live(),
            attested: Vec::new(),
            follow: None,
        }
    }
}

/// The folder and file name a follow-folder grant was set for, and when (always `by: "dialog"`;
/// any other `by` read from the file is no follow).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowFolder {
    /// Canonical folder of `run[0]` at the Allow.
    pub folder: String,
    /// File name of `run[0]` at the Allow.
    pub file_name: String,
    pub at: String,
    pub by: String,
}

/// One program change the update path vouched for: old file to new file, same install folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    pub from_sha256: String,
    pub to_sha256: String,
    pub from_path: String,
    pub to_path: String,
    /// The version the updater or installer named.
    pub version: String,
    pub at: String,
    /// Who vouched: `updater` or `installer`.
    pub by: String,
    /// A `folder` step only (#1465): the step sent the held items back to waiting and voided the
    /// keyboard record. Absent on a step recorded before that rule, which carried everything;
    /// the registry applies the rule to such a step once when it opens
    /// ([`Registry::reask_old_folder_steps`]) and sets this.
    ///
    /// [`Registry::reask_old_folder_steps`]: crate::Registry::reask_old_folder_steps
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held_back: bool,
}

impl Installed {
    /// Whether an install now of kind `kind` from `source`/`anchor` (and, for a sideload, the
    /// canonical `folder`) is the same origin as this entry. Only the same origin carries the
    /// user's grants (trust model F1/F2); a recorded entry with no sideload folder is never the
    /// same origin as a sideload (fails closed once).
    pub fn same_origin(
        &self,
        source: &str,
        anchor: Option<&str>,
        kind: Kind,
        folder: Option<&str>,
    ) -> bool {
        self.source == source
            && self.anchor.as_deref() == anchor
            && self.kind == kind
            && (source != SIDELOAD
                || match (self.sideload_folder.as_deref(), folder) {
                    (Some(old), Some(new)) => crate::keyboard::same_path(old, new),
                    _ => false,
                })
    }

    /// Who this module is for the never-list (§6.6).
    pub fn holder(&self) -> Holder {
        // A lab module holds permissions as a sideload does, never as a first-party module.
        // An adopted module has no catalog behind it either: it is held as a sideload is.
        let source = if self.source == SIDELOAD || self.source == LAB || self.source == ADOPTED {
            Source::Sideload
        } else {
            match self.anchor.as_deref() {
                Some(ROOT_ANCHOR) => Source::Catalog,
                Some(_) => Source::UserKey,
                None => Source::Unsigned,
            }
        };
        match (self.visibility, self.family) {
            (Visibility::Private, _) => Holder::Private(source),
            (_, f) if f.first_party() && source == Source::Catalog => Holder::FirstParty,
            _ => Holder::ThirdParty(source),
        }
    }
}

/// Whether two `provide:` method patterns (a name or a prefix ending in `.*`) overlap.
pub(crate) fn provide_overlap(a: &str, b: &str) -> bool {
    let covers = |outer: &str, inner: &str| match outer.strip_suffix('*') {
        Some(prefix) => inner.starts_with(prefix),
        None => outer == inner,
    };
    covers(a, b) || covers(b, a)
}

/// What a grant left out: never-list items (§6.6), each with its `permission.never` error.
pub type Refused = Vec<Error>;

fn canonical(list: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = list.into_iter().collect();
    v.sort();
    v.dedup();
    v
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub format: String,
    #[serde(default)]
    pub modules: BTreeMap<String, Installed>,
    #[serde(default)]
    pub seen: BTreeMap<String, SeenAnchor>,
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            format: FORMAT.into(),
            modules: BTreeMap::new(),
            seen: BTreeMap::new(),
        }
    }
}

impl Inventory {
    /// Reads `path`; a missing file is an empty inventory.
    pub fn load(path: &Path) -> io::Result<Inventory> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Inventory::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes atomically (a temp file, then a rename) under `inventory.lock` beside it.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let _lock = Lock::take(&path.with_file_name("inventory.lock"))?;
        self.write(path)
    }

    /// Reads the file, changes it with `f` and writes it back, all under `inventory.lock`, so
    /// what another writer saved meanwhile is kept (#1049: `seen` raised after a fetch never
    /// writes back an older copy). Returns what was written.
    pub fn update(path: &Path, f: impl FnOnce(&mut Inventory)) -> io::Result<Inventory> {
        let _lock = Lock::take(&path.with_file_name("inventory.lock"))?;
        let mut inv = Inventory::load(path)?;
        f(&mut inv);
        inv.write(path)?;
        Ok(inv)
    }

    /// The write itself; the caller holds `inventory.lock`.
    fn write(&self, path: &Path) -> io::Result<()> {
        if self.format != FORMAT {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unknown inventory format {}: not written", self.format),
            ));
        }
        // Lab modules live in memory only (docs/hideout-lab.md).
        let mut kept = self.clone();
        kept.modules.retain(|_, m| m.source != LAB);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&kept).map_err(io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Grants `asked` to `id`: never-list items are left out and returned (§6.6); the rest join
    /// `granted` in canonical form. The caller shows the dialog first where §6.4 needs one.
    pub fn grant(&mut self, id: &str, asked: &[Capability]) -> Option<Refused> {
        // Another adopted module's `provide:` grants (#1118): an adopted module is refused a
        // `provide:` that overlaps one, so two never hold the same method area (the GPU broker).
        let others: Vec<String> = self
            .modules
            .iter()
            .filter(|(k, o)| k.as_str() != id && o.source == ADOPTED)
            .flat_map(|(_, o)| o.granted.iter())
            .filter_map(|g| g.strip_prefix("provide:").map(String::from))
            .collect();
        let m = self.modules.get_mut(id)?;
        let holder = m.holder();
        let adopted = m.source == ADOPTED;
        let mut refused = Vec::new();
        let mut keep = m.granted.clone();
        for cap in asked {
            if let (true, Capability::Provide(p)) = (adopted, cap) {
                if others.iter().any(|o| provide_overlap(o, p)) {
                    refused.push(Error::new(
                        hideout_contract::ErrorId::PermissionProvider,
                        format!(
                            "{cap}: another adopted module already holds an overlapping provide"
                        ),
                    ));
                    continue;
                }
            }
            match check_grantable(cap, holder) {
                Ok(()) => keep.push(cap.to_string()),
                Err(e) => refused.push(e),
            }
        }
        m.granted = canonical(keep);
        m.pending_permissions.retain(|p| !m.granted.contains(p));
        Some(refused)
    }

    /// An update asks for `asked`: what isn't granted yet waits in `pending_permissions` (the
    /// old version keeps running, "Update waiting for you", §6.4). Returns whether any waits.
    pub fn ask(&mut self, id: &str, asked: &[Capability]) -> bool {
        let Some(m) = self.modules.get_mut(id) else {
            return false;
        };
        let asked: Vec<String> = asked.iter().map(|c| c.to_string()).collect();
        // What the manifest no longer asks for is dropped (narrowing, #1178).
        m.granted.retain(|g| asked.contains(g));
        m.declined_permissions.retain(|d| asked.contains(d));
        let new = asked
            .iter()
            .filter(|c| !m.granted.contains(c) && !m.declined_permissions.contains(c))
            .cloned();
        m.pending_permissions = canonical(m.pending_permissions.drain(..).chain(new));
        m.pending_permissions.retain(|p| asked.contains(p));
        !m.pending_permissions.is_empty()
    }

    /// Whether `id` holds `cap` (exact canonical form).
    pub fn holds(&self, id: &str, cap: &Capability) -> bool {
        let text = cap.to_string();
        self.modules
            .get(id)
            .is_some_and(|m| m.granted.contains(&text))
    }
}

/// `inventory.lock`: created exclusively, removed on drop.
struct Lock(PathBuf);

impl Lock {
    fn take(path: &Path) -> io::Result<Lock> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    io::Error::new(io::ErrorKind::WouldBlock, "inventory.lock is held")
                } else {
                    e
                }
            })?;
        Ok(Lock(path.to_path_buf()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
