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
}

impl Installed {
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
fn provide_overlap(a: &str, b: &str) -> bool {
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
        let new = asked
            .iter()
            .map(|c| c.to_string())
            .filter(|c| !m.granted.contains(c));
        m.pending_permissions = canonical(m.pending_permissions.drain(..).chain(new));
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
