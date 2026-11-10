// SPDX-License-Identifier: MIT
//! Module provenance (spec §12, §13.6): the Hideout root chain, user anchors, catalogs, and what
//! an accepted catalog lets install and run. Signatures go through `foxglove-plugin-trust`.
//! Nothing here grants a permission (§6.5).

use std::collections::BTreeMap;

use foxglove_plugin_trust::{parse_utc, verify_signature, Action, Revocation};
use hideout_contract::module::{Kind, Module, Visibility};
use hideout_contract::{scope_covers, scopes_overlap, Error, ErrorId, LEGACY_ROOT_SCOPE};
use serde::{Deserialize, Serialize};

pub use hideout_contract::config::ROOT_ANCHOR;

/// The Hideout root's public key, pinned in the hub (§12.2). `None` until the root key is made
/// (#90): until then a root-anchored source is not fetched or loaded, and says so in the log.
/// Never a test key (tests/hideout-vectors/trust/keys/README.txt).
pub const PINNED_ROOT_KEY: Option<&str> = None;

/// The family a catalog signed by a user anchor reads as (§12.4, v1.1).
pub const USER_FAMILY: &str = hideout_contract::DEFAULT_USER_CATALOG_FAMILY;

fn err<T>(id: ErrorId, message: impl Into<String>) -> Result<T, Error> {
    Err(Error::new(id, message))
}

/// Anti-rollback state for one trust anchor (`inventory.json` `seen`, §7.3). It never goes down.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeenAnchor {
    pub root_version: u64,
    pub epoch: u64,
    /// The last accepted catalog sequence per source id.
    pub catalogs: BTreeMap<String, u64>,
}

impl SeenAnchor {
    /// Raises the state to an accepted root. A new epoch restarts the catalog sequences.
    pub fn record_root(&mut self, root: &Root) {
        if root.epoch > self.epoch {
            self.catalogs.clear();
        }
        self.root_version = self.root_version.max(root.version);
        self.epoch = self.epoch.max(root.epoch);
    }

    /// Raises the state to an accepted catalog from `source`.
    pub fn record_catalog(&mut self, source: &str, catalog: &Catalog) {
        let seq = self.catalogs.entry(source.to_string()).or_default();
        *seq = (*seq).max(catalog.sequence);
    }
}

/// One family in a root: the keys that sign its catalog and the ids they may sign.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyKeys {
    pub keys: Vec<String>,
    pub scope: Vec<String>,
}

/// A verified `root.json` (`hideout-root-v1`, or Foxglove's legacy root read as one family).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub version: u64,
    pub epoch: u64,
    pub expires: String,
    pub families: BTreeMap<String, FamilyKeys>,
    /// Read from `foxglove-plugin-root`: catalogs without `family` are `foxglove`'s.
    pub legacy: bool,
}

#[derive(Deserialize)]
struct RootFile {
    format: String,
    version: u64,
    #[serde(default)]
    epoch: u64,
    expires: String,
    #[serde(default)]
    families: BTreeMap<String, FamilyKeys>,
    #[serde(default)]
    catalog_keys: Vec<String>,
}

fn time_ok(s: &str) -> bool {
    parse_utc(s).is_some()
}

impl Root {
    /// Each family's scope patterns, as `Config::validate` takes them.
    pub fn scopes(&self) -> BTreeMap<String, Vec<String>> {
        self.families
            .iter()
            .map(|(f, k)| (f.clone(), k.scope.clone()))
            .collect()
    }

    /// Whether any family's scope covers `id` (a first-party scope, §12.6).
    pub fn covers(&self, id: &str) -> bool {
        self.families
            .values()
            .any(|f| f.scope.iter().any(|p| scope_covers(p, id)))
    }
}

/// Verifies `root.json` against the key pinned in the hub, the clock and `seen` (§12.2, §12.3).
pub fn verify_root(
    json: &[u8],
    signature: &str,
    pinned_key: &str,
    now: &str,
    seen: Option<&SeenAnchor>,
) -> Result<Root, Error> {
    verify_signature(json, signature, &[pinned_key])
        .map_err(|e| Error::new(ErrorId::TrustSignature, format!("root.json: {e}")))?;
    let file: RootFile = serde_json::from_slice(json)
        .map_err(|e| Error::new(ErrorId::ManifestSyntax, format!("root.json: {e}")))?;
    let (families, legacy) = match file.format.as_str() {
        "hideout-root-v1" => (file.families, false),
        "foxglove-plugin-root" | "foxglove-kit-root" => {
            let keys = FamilyKeys {
                keys: file.catalog_keys,
                scope: LEGACY_ROOT_SCOPE.iter().map(|s| s.to_string()).collect(),
            };
            (BTreeMap::from([("foxglove".to_string(), keys)]), true)
        }
        other => {
            return err(
                ErrorId::ManifestSyntax,
                format!("not a root format: {other}"),
            )
        }
    };
    if !time_ok(&file.expires) || file.expires.as_str() <= now {
        return err(
            ErrorId::TrustRootExpired,
            format!("expired {}", file.expires),
        );
    }
    if let Some(seen) = seen {
        if file.version < seen.root_version || file.epoch < seen.epoch {
            return err(
                ErrorId::TrustRootRollback,
                format!(
                    "root version {} epoch {} is below seen {} / {}",
                    file.version, file.epoch, seen.root_version, seen.epoch
                ),
            );
        }
    }
    let mut key_family: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, fam) in &families {
        for key in &fam.keys {
            if let Some(other) = key_family.insert(key.trim(), name) {
                return err(
                    ErrorId::TrustKeyReuse,
                    format!("a key is in both {other} and {name}"),
                );
            }
        }
        if let Some(p) = fam.scope.iter().find(|p| p.as_str() == "*" || p.is_empty()) {
            return err(
                ErrorId::TrustScope,
                format!("{name}: scope {p:?} is refused"),
            );
        }
    }
    let list: Vec<(&String, &String)> = families
        .iter()
        .flat_map(|(f, k)| k.scope.iter().map(move |p| (f, p)))
        .collect();
    for (i, (fa, pa)) in list.iter().enumerate() {
        for (fb, pb) in &list[i + 1..] {
            if fa != fb && scopes_overlap(pa, pb) {
                return err(
                    ErrorId::TrustScopeOverlap,
                    format!("{fa} {pa} overlaps {fb} {pb}"),
                );
            }
        }
    }
    Ok(Root {
        version: file.version,
        epoch: file.epoch,
        expires: file.expires,
        families,
        legacy,
    })
}

/// Who a catalog must verify against (§13.6).
#[derive(Clone, Copy, Debug)]
pub enum Signer<'a> {
    /// The Hideout root: the catalog's family picks the key and the scope.
    Root(&'a Root),
    /// A user anchor: one key, its own scope, no `root.json` above it.
    User { key: &'a str, scope: &'a [String] },
}

/// A channel a catalog entry offers, with what it gives in plain words (§6.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelOffer {
    pub capability: String,
    #[serde(default)]
    pub risk: String,
}

fn service() -> Kind {
    Kind::Service
}
fn public() -> Visibility {
    Visibility::Public
}
fn stable() -> String {
    "stable".into()
}

/// One module version a catalog pins (§12.4). Legacy entries read as public services.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub version: String,
    #[serde(default = "service")]
    pub kind: Kind,
    #[serde(default = "public")]
    pub visibility: Visibility,
    #[serde(default = "stable")]
    pub channel: String,
    #[serde(default)]
    pub hub: Option<String>,
    pub sha256: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub provide: Vec<String>,
    #[serde(default)]
    pub channels: BTreeMap<String, ChannelOffer>,
}

impl Entry {
    /// `trust.mismatch` unless the manifest's id, version, kind and visibility are the entry's.
    pub fn check_manifest(&self, m: &Module) -> Result<(), Error> {
        let same = m.id == self.id
            && m.version == self.version
            && m.kind == self.kind
            && m.visibility == self.visibility;
        if same {
            Ok(())
        } else {
            err(
                ErrorId::TrustMismatch,
                format!(
                    "{}@{}: the manifest says {}@{} {:?} {:?}, the catalog {:?} {:?}",
                    self.id,
                    self.version,
                    m.id,
                    m.version,
                    m.kind,
                    m.visibility,
                    self.kind,
                    self.visibility
                ),
            )
        }
    }

    /// `trust.visibility` when a public source offers a private module (§13.2, §13.4).
    pub fn check_source(&self, source: Visibility) -> Result<(), Error> {
        if source == Visibility::Public && self.visibility == Visibility::Private {
            return err(
                ErrorId::TrustVisibility,
                format!("{}: a private module in a public source", self.id),
            );
        }
        Ok(())
    }
}

/// A verified catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    pub family: String,
    pub epoch: u64,
    pub sequence: u64,
    pub expires: String,
    pub modules: Vec<Entry>,
    pub revocations: Vec<Revocation>,
}

#[derive(Deserialize)]
struct CatalogFile {
    format: String,
    #[serde(default)]
    epoch: u64,
    sequence: u64,
    expires: String,
    #[serde(default, alias = "plugins", alias = "kits")]
    modules: Vec<Entry>,
    #[serde(default)]
    revocations: Vec<Revocation>,
}

const CATALOG_FORMATS: &[&str] = &[
    "hideout-catalog-v1",
    "foxglove-plugin-catalog-v2",
    "foxglove-plugin-catalog",
    "foxglove-kit-catalog",
];

/// Verifies a catalog from source `source_id` (§12.4): signature by a key bound to its family,
/// every id in scope, the epoch, the expiry and `seen`. Entry visibility against the source is
/// checked per entry ([`Entry::check_source`]), after the manifest comparison.
pub fn verify_catalog(
    json: &[u8],
    signature: &str,
    signer: Signer,
    source_id: &str,
    now: &str,
    seen: Option<&SeenAnchor>,
) -> Result<Catalog, Error> {
    let sig_err = |e: String| Error::new(ErrorId::TrustSignature, format!("catalog: {e}"));
    // The family names the key, so it is read before the signature, then held to it.
    let declared: Option<String> = serde_json::from_slice::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("family")?.as_str().map(String::from));
    let (family, scope, root_epoch) = match signer {
        Signer::Root(root) => {
            let name = match (&declared, root.legacy) {
                (Some(f), _) => f.clone(),
                (None, true) => "foxglove".to_string(),
                (None, false) => return Err(sig_err("no family under the root".into())),
            };
            let Some(fam) = root.families.get(&name) else {
                return Err(sig_err(format!("the root lists no family {name}")));
            };
            let keys: Vec<&str> = fam.keys.iter().map(String::as_str).collect();
            verify_signature(json, signature, &keys).map_err(sig_err)?;
            (name, fam.scope.clone(), Some(root.epoch))
        }
        Signer::User { key, scope } => {
            verify_signature(json, signature, &[key]).map_err(sig_err)?;
            let name = declared.unwrap_or_else(|| USER_FAMILY.to_string());
            if name != USER_FAMILY {
                return Err(sig_err(format!("a user key cannot sign family {name}")));
            }
            (name, scope.to_vec(), None)
        }
    };
    let file: CatalogFile = serde_json::from_slice(json)
        .map_err(|e| Error::new(ErrorId::ManifestSyntax, format!("catalog: {e}")))?;
    if !CATALOG_FORMATS.contains(&file.format.as_str()) {
        return err(
            ErrorId::ManifestSyntax,
            format!("not a catalog format: {}", file.format),
        );
    }
    let ids = file
        .modules
        .iter()
        .map(|e| &e.id)
        .chain(file.revocations.iter().map(|r| &r.id));
    for id in ids {
        if !scope.iter().any(|p| scope_covers(p, id)) {
            return err(
                ErrorId::TrustScope,
                format!("{id} is outside the {family} scope"),
            );
        }
    }
    if let Some(e) = root_epoch {
        if file.epoch != e {
            return err(
                ErrorId::TrustEpoch,
                format!("catalog epoch {} is not the root's {e}", file.epoch),
            );
        }
    }
    if !time_ok(&file.expires) || file.expires.as_str() <= now {
        return err(ErrorId::TrustExpired, format!("expired {}", file.expires));
    }
    if let Some(seen) = seen {
        let last = seen.catalogs.get(source_id).copied().unwrap_or(0);
        if file.epoch == seen.epoch && file.sequence < last {
            return err(
                ErrorId::TrustRollback,
                format!("sequence {} is below seen {last}", file.sequence),
            );
        }
    }
    Ok(Catalog {
        family,
        epoch: file.epoch,
        sequence: file.sequence,
        expires: file.expires,
        modules: file.modules,
        revocations: file.revocations,
    })
}

/// A catalog this hub accepted, with where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Accepted {
    pub anchor: String,
    pub source: String,
    pub visibility: Visibility,
    pub catalog: Catalog,
}

/// Everything the hub currently trusts: the root (if loaded) and the accepted catalogs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrustSet {
    pub root: Option<Root>,
    pub catalogs: Vec<Accepted>,
}

impl TrustSet {
    fn withdrawn(&self, id: &str, version: &str) -> Option<&Revocation> {
        let hit = |r: &&Revocation| {
            r.id == id && (r.versions.is_empty() || r.versions.iter().any(|v| v == version))
        };
        let all: Vec<&Revocation> = self
            .catalogs
            .iter()
            .flat_map(|a| a.catalog.revocations.iter())
            .filter(hit)
            .collect();
        all.iter()
            .find(|r| r.action == Action::Revoke)
            .or(all.first())
            .copied()
    }

    /// The revocation (not a yank) that covers `id@version`, if any: a running one is stopped.
    pub fn revocation(&self, id: &str, version: &str) -> Option<&Revocation> {
        self.withdrawn(id, version)
            .filter(|r| r.action == Action::Revoke)
    }

    fn withdrawn_err(r: &Revocation, version: &str) -> Error {
        let (id, word) = match r.action {
            Action::Revoke => (ErrorId::TrustRevoked, "revoked"),
            Action::Yank => (ErrorId::TrustYanked, "yanked"),
        };
        Error::new(id, format!("{}@{version} is {word}: {}", r.id, r.reason))
    }

    /// The entry a new install of `id@version` may use: listed, and neither yanked nor revoked.
    pub fn installable(&self, id: &str, version: &str) -> Result<(&Accepted, &Entry), Error> {
        if let Some(r) = self.withdrawn(id, version) {
            return Err(Self::withdrawn_err(r, version));
        }
        self.find(|e| e.id == id && e.version == version)
            .ok_or_else(|| {
                Error::new(
                    ErrorId::TrustUnknownArchive,
                    format!("{id}@{version} is in no accepted catalog"),
                )
            })
    }

    /// The entry for an archive by its SHA-256 (§12.5), not withdrawn.
    pub fn archive(&self, sha256: &str) -> Result<(&Accepted, &Entry), Error> {
        let (a, e) = self
            .find(|e| e.sha256.eq_ignore_ascii_case(sha256))
            .ok_or_else(|| {
                Error::new(
                    ErrorId::TrustUnknownArchive,
                    format!("archive {sha256} is in no accepted catalog"),
                )
            })?;
        if let Some(r) = self.withdrawn(&e.id, &e.version) {
            return Err(Self::withdrawn_err(r, &e.version));
        }
        Ok((a, e))
    }

    /// The entry an installed `id@version` runs under: listed and not revoked (a yanked
    /// version keeps running).
    pub fn runnable(&self, id: &str, version: &str) -> Result<(&Accepted, &Entry), Error> {
        if let Some(r) = self.withdrawn(id, version) {
            if r.action == Action::Revoke {
                return Err(Self::withdrawn_err(r, version));
            }
        }
        self.find(|e| e.id == id && e.version == version)
            .ok_or_else(|| {
                Error::new(
                    ErrorId::TrustUnknownArchive,
                    format!("{id}@{version} is in no accepted catalog"),
                )
            })
    }

    fn find(&self, pred: impl Fn(&Entry) -> bool) -> Option<(&Accepted, &Entry)> {
        self.catalogs
            .iter()
            .find_map(|a| a.catalog.modules.iter().find(|e| pred(e)).map(|e| (a, e)))
    }
}

/// `trust.files` unless `found` (path to SHA-256, files outside `data\`) is exactly `want`.
pub fn check_files(
    found: &BTreeMap<String, String>,
    want: &BTreeMap<String, String>,
) -> Result<(), Error> {
    for (path, hash) in want {
        match found.get(path) {
            None => return err(ErrorId::TrustFiles, format!("{path} is missing")),
            Some(got) if !got.eq_ignore_ascii_case(hash) => {
                return err(ErrorId::TrustFiles, format!("{path} was changed"))
            }
            _ => {}
        }
    }
    if let Some(extra) = found.keys().find(|p| !want.contains_key(*p)) {
        return err(ErrorId::TrustFiles, format!("{extra} is not a signed file"));
    }
    Ok(())
}
