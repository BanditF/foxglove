//! Verifying Foxglove plugins (#159, from Scout's #158).
//!
//! The chain of trust:
//! 1. An **offline root key**, pinned in the app, signs `root.json`. It names the keys allowed
//!    to sign the catalog and has its own version and expiry, so catalog keys can be rotated
//!    without an app release.
//! 2. One of those keys signs `catalog.json`: a **sequence** number (never lower than the last
//!    one seen, so an old catalog can't be replayed), an expiry, and for each plugin version the
//!    SHA-256 of its archive and of every file in it.
//! 3. A Plugin installs only if its archive's hash is in the catalog, and runs only if its folder
//!    holds exactly the listed files with the listed hashes, checked before every launch.
//!
//! Signatures are minisign (Ed25519), verified with `minisign-verify`. Times are RFC 3339 UTC
//! (`2026-10-24T00:00:00Z`), which compare correctly as text.
//!
//! Catalog v2 (#192, from Scout's #190), every new field optional so v1 files still read:
//! - **Epoch:** set by the root, which only the offline key signs. A catalog must carry its
//!   root's epoch, so a stolen catalog key can't start a new epoch; a new epoch restarts the
//!   sequence.
//! - **Rollback:** what this install accepted before ([`Seen`]: root version, epoch, catalog
//!   sequence) never goes down; the caller persists it and refuses to go on if it can't.
//! - **Entries** gain a `channel` (stable or beta, in one catalog), and the archive's `url` and
//!   `size` for downloads.
//! - **Revocations:** `yank` stops new installs of a version; `revoke` also stops it running.

use std::collections::BTreeMap;
use std::path::Path;

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Root {
    pub format: String,
    pub version: u64,
    /// The catalog epoch this root allows (v2).
    #[serde(default)]
    pub epoch: u64,
    pub expires: String,
    /// Minisign public keys (base64) that may sign the catalog.
    pub catalog_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub format: String,
    /// Must equal the root's epoch (v2).
    #[serde(default)]
    pub epoch: u64,
    pub sequence: u64,
    pub expires: String,
    #[serde(alias = "kits")]
    pub plugins: Vec<Entry>,
    #[serde(default)]
    pub revocations: Vec<Revocation>,
}

/// One signed plugin version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub version: String,
    /// `stable` or `beta` (v2).
    #[serde(default = "stable")]
    pub channel: String,
    /// SHA-256 of the archive (hex).
    pub sha256: String,
    /// Where to download the archive (v2): an immutable release asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The archive's size in bytes (v2), the download limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Every file in the plugin, by path with `/`, to its SHA-256.
    pub files: BTreeMap<String, String>,
}

/// The format a v2 catalog is published with. Older apps don't know it and refuse the file,
/// instead of reading it and silently ignoring its revocations.
pub const FORMAT_V2: &str = "foxglove-plugin-catalog-v2";

fn stable() -> String {
    "stable".into()
}

/// A withdrawn plugin version (v2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Revocation {
    pub id: String,
    /// The versions it covers; empty means every version.
    #[serde(default)]
    pub versions: Vec<String>,
    pub action: Action,
    #[serde(default)]
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// No new installs; installed copies keep running.
    Yank,
    /// No installs, and installed copies stop.
    Revoke,
}

/// What this install has accepted before, against rollback. Persist it after every accepted
/// catalog ([`Seen::after`]); it never goes down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    pub root_version: u64,
    pub epoch: u64,
    pub sequence: u64,
}

impl Seen {
    /// The state after accepting `root` and `catalog`.
    pub fn after(&self, root: &Root, catalog: &Catalog) -> Seen {
        let sequence = if catalog.epoch > self.epoch { catalog.sequence } else { self.sequence.max(catalog.sequence) };
        Seen { root_version: self.root_version.max(root.version), epoch: self.epoch.max(catalog.epoch), sequence }
    }
}

impl Catalog {
    /// The revocation covering `id` `version`, if any (a revoke wins over a yank).
    pub fn revocation(&self, id: &str, version: &str) -> Option<&Revocation> {
        let mut found = self
            .revocations
            .iter()
            .filter(|r| r.id == id && (r.versions.is_empty() || r.versions.iter().any(|v| v == version)));
        let first = found.next()?;
        Some(found.find(|r| r.action == Action::Revoke).unwrap_or(first))
    }

    /// The entry for an archive about to be installed, by its SHA-256, unless that version
    /// was yanked or revoked.
    pub fn installable(&self, sha256: &str) -> Result<&Entry, String> {
        let entry = self
            .plugins
            .iter()
            .find(|e| e.sha256.eq_ignore_ascii_case(sha256))
            .ok_or("this archive isn't a signed Foxglove plugin")?;
        match self.revocation(&entry.id, &entry.version) {
            Some(r) => Err(format!("{} {} was withdrawn{}", entry.id, entry.version, reason(r))),
            None => Ok(entry),
        }
    }

    /// The entry an installed `id` `version` runs under, unless it was revoked (a yanked
    /// version keeps running).
    pub fn runnable(&self, id: &str, version: &str) -> Result<&Entry, String> {
        if let Some(r) = self.revocation(id, version).filter(|r| r.action == Action::Revoke) {
            return Err(format!("{id} {version} was revoked{}", reason(r)));
        }
        self.plugins
            .iter()
            .find(|e| e.id == id && e.version == version)
            .ok_or_else(|| format!("{id} {version} isn't in the signed catalog"))
    }
}

fn reason(r: &Revocation) -> String {
    if r.reason.is_empty() {
        String::new()
    } else {
        format!(": {}", r.reason)
    }
}

fn time_ok(s: &str) -> bool {
    s.len() == 20 && s.ends_with('Z') && s.as_bytes()[10] == b'T'
}

/// Checks a minisign signature over `data` with any of `keys`.
fn verify_with(data: &[u8], signature: &str, keys: &[PublicKey]) -> Result<(), String> {
    let sig = Signature::decode(signature).map_err(|e| format!("bad signature: {e}"))?;
    if keys.iter().any(|k| k.verify(data, &sig, false).is_ok()) {
        Ok(())
    } else {
        Err("signature doesn't match a trusted key".into())
    }
}

/// `root.json`, signed by the pinned root key, not expired at `now`, and not older than what
/// `seen` accepted (version and epoch).
pub fn verify_root(json: &[u8], signature: &str, root_key: &str, now: &str, seen: &Seen) -> Result<Root, String> {
    let key = PublicKey::from_base64(root_key.trim()).map_err(|e| format!("bad root key: {e}"))?;
    verify_with(json, signature, &[key])?;
    let root: Root = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    if !matches!(root.format.as_str(), "foxglove-plugin-root" | "foxglove-kit-root") {
        return Err("not a Foxglove root".into());
    }
    if !time_ok(&root.expires) || root.expires.as_str() <= now {
        return Err(format!("the root expired ({})", root.expires));
    }
    if root.version < seen.root_version {
        return Err(format!("an older root (version {} < {})", root.version, seen.root_version));
    }
    if root.epoch < seen.epoch {
        return Err(format!("an older root (epoch {} < {})", root.epoch, seen.epoch));
    }
    Ok(root)
}

/// `catalog.json`, signed by a key `root` names, in the root's epoch, not expired, and not
/// older than what `seen` accepted in that epoch.
pub fn verify_catalog(json: &[u8], signature: &str, root: &Root, now: &str, seen: &Seen) -> Result<Catalog, String> {
    let keys: Vec<PublicKey> = root.catalog_keys.iter().filter_map(|k| PublicKey::from_base64(k).ok()).collect();
    verify_with(json, signature, &keys)?;
    let catalog: Catalog = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    if !matches!(catalog.format.as_str(), FORMAT_V2 | "foxglove-plugin-catalog" | "foxglove-kit-catalog") {
        return Err("not a Foxglove plugin catalog".into());
    }
    if !time_ok(&catalog.expires) || catalog.expires.as_str() <= now {
        return Err(format!("the catalog expired ({})", catalog.expires));
    }
    if catalog.epoch != root.epoch {
        return Err(format!("the catalog's epoch ({}) isn't the root's ({})", catalog.epoch, root.epoch));
    }
    if catalog.epoch == seen.epoch && catalog.sequence < seen.sequence {
        return Err(format!("an older catalog (sequence {} < {})", catalog.sequence, seen.sequence));
    }
    Ok(catalog)
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Every file under `dir` (paths with `/`) and its SHA-256, skipping `skip` (files the runner
/// writes there itself, like `plugin.log`).
pub fn hash_dir(dir: &Path, skip: &[&str]) -> std::io::Result<BTreeMap<String, String>> {
    fn walk(base: &Path, dir: &Path, skip: &[&str], out: &mut BTreeMap<String, String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let rel = path.strip_prefix(base).expect("under base").to_string_lossy().replace('\\', "/");
            if skip.contains(&rel.as_str()) {
                continue;
            }
            if entry.file_type()?.is_dir() {
                walk(base, &path, skip, out)?;
            } else {
                out.insert(rel, sha256_hex(&std::fs::read(&path)?));
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, skip, &mut out)?;
    Ok(out)
}

/// Whether `dir` holds exactly the entry's files, unchanged.
pub fn check_files(dir: &Path, entry: &Entry, skip: &[&str]) -> Result<(), String> {
    let found = hash_dir(dir, skip).map_err(|e| e.to_string())?;
    for (path, want) in &entry.files {
        match found.get(path) {
            None => return Err(format!("{path} is missing")),
            Some(got) if got != want => return Err(format!("{path} was changed")),
            _ => {}
        }
    }
    if let Some(extra) = found.keys().find(|p| !entry.files.contains_key(*p)) {
        return Err(format!("{extra} isn't part of the signed plugin"));
    }
    Ok(())
}

/// The UTC time now as RFC 3339 (`YYYY-MM-DDTHH:MM:SSZ`), for the expiry checks.
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::KeyPair;

    fn sign(kp: &KeyPair, data: &[u8]) -> String {
        minisign::sign(Some(&kp.pk), &kp.sk, data, None, None).unwrap().to_string()
    }

    struct Setup {
        root_key: String,
        root_json: Vec<u8>,
        root_sig: String,
        catalog_kp: KeyPair,
    }

    fn setup(root_expires: &str) -> Setup {
        setup_with(root_expires, 1, 0)
    }

    fn setup_with(root_expires: &str, version: u64, epoch: u64) -> Setup {
        let root_kp = KeyPair::generate_unencrypted_keypair().unwrap();
        let catalog_kp = KeyPair::generate_unencrypted_keypair().unwrap();
        let root = Root {
            format: "foxglove-plugin-root".into(),
            version,
            epoch,
            expires: root_expires.into(),
            catalog_keys: vec![catalog_kp.pk.to_base64()],
        };
        let root_json = serde_json::to_vec(&root).unwrap();
        let root_sig = sign(&root_kp, &root_json);
        Setup { root_key: root_kp.pk.to_base64(), root_json, root_sig, catalog_kp }
    }

    fn entry(id: &str, version: &str, sha: &str) -> Entry {
        Entry {
            id: id.into(),
            version: version.into(),
            channel: stable(),
            sha256: sha.into(),
            url: None,
            size: None,
            files: BTreeMap::new(),
        }
    }

    fn catalog_in(epoch: u64, sequence: u64, expires: &str) -> Vec<u8> {
        serde_json::to_vec(&Catalog {
            format: "foxglove-plugin-catalog".into(),
            epoch,
            sequence,
            expires: expires.into(),
            plugins: vec![entry("com.example.weather", "1.0.0", &"ab".repeat(32))],
            revocations: vec![],
        })
        .unwrap()
    }

    fn catalog(sequence: u64, expires: &str, _files: BTreeMap<String, String>) -> Vec<u8> {
        catalog_in(0, sequence, expires)
    }

    fn seen(sequence: u64) -> Seen {
        Seen { sequence, ..Seen::default() }
    }

    const NOW: &str = "2026-10-24T00:00:00Z";
    const LATER: &str = "2027-01-01T00:00:00Z";

    #[test]
    fn catalog_from_before_the_rename_still_reads() {
        let old = r#"{"format":"foxglove-kit-catalog","sequence":1,"expires":"2030-01-01T00:00:00Z","kits":[]}"#;
        let c: Catalog = serde_json::from_str(old).unwrap();
        assert!(c.plugins.is_empty());
    }

    #[test]
    fn the_chain_verifies_and_refuses_tampering_expiry_and_rollback() {
        let s = setup(LATER);
        let root = verify_root(&s.root_json, &s.root_sig, &s.root_key, NOW, &Seen::default()).unwrap();
        let json = catalog(5, LATER, BTreeMap::new());
        let sig = sign(&s.catalog_kp, &json);
        assert_eq!(verify_catalog(&json, &sig, &root, NOW, &seen(5)).unwrap().sequence, 5);
        // Tampered catalog.
        let mut bad = json.clone();
        bad[20] ^= 1;
        assert!(verify_catalog(&bad, &sig, &root, NOW, &seen(0)).is_err());
        // Rolled back.
        assert!(verify_catalog(&json, &sig, &root, NOW, &seen(6)).unwrap_err().contains("older"));
        // Expired.
        let old = catalog(7, "2026-01-01T00:00:00Z", BTreeMap::new());
        assert!(verify_catalog(&old, &sign(&s.catalog_kp, &old), &root, NOW, &seen(0)).unwrap_err().contains("expired"));
        // Signed by a key the root doesn't name.
        let stranger = KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(verify_catalog(&json, &sign(&stranger, &json), &root, NOW, &seen(0)).is_err());
        // A root signed by anything but the pinned key.
        assert!(verify_root(&s.root_json, &sign(&stranger, &s.root_json), &s.root_key, NOW, &Seen::default()).is_err());
        assert!(verify_root(&s.root_json, &s.root_sig, &s.root_key, "2027-06-01T00:00:00Z", &Seen::default()).is_err());
    }

    #[test]
    fn root_and_epoch_never_go_back() {
        let s = setup_with(LATER, 3, 2);
        let newer = Seen { root_version: 4, epoch: 0, sequence: 0 };
        assert!(verify_root(&s.root_json, &s.root_sig, &s.root_key, NOW, &newer).unwrap_err().contains("version"));
        let later_epoch = Seen { root_version: 0, epoch: 3, sequence: 0 };
        assert!(verify_root(&s.root_json, &s.root_sig, &s.root_key, NOW, &later_epoch).unwrap_err().contains("epoch"));
        let root = verify_root(&s.root_json, &s.root_sig, &s.root_key, NOW, &Seen { root_version: 3, epoch: 1, sequence: 90 }).unwrap();
        // Only the root moves the epoch: a catalog in another epoch is refused...
        let wrong = catalog_in(3, 1, LATER);
        assert!(verify_catalog(&wrong, &sign(&s.catalog_kp, &wrong), &root, NOW, &Seen::default()).unwrap_err().contains("epoch"));
        // ...and a new epoch restarts the sequence.
        let json = catalog_in(2, 1, LATER);
        let before = Seen { root_version: 3, epoch: 1, sequence: 90 };
        let c = verify_catalog(&json, &sign(&s.catalog_kp, &json), &root, NOW, &before).unwrap();
        assert_eq!(before.after(&root, &c), Seen { root_version: 3, epoch: 2, sequence: 1 });
        // Within the epoch, the sequence only rises.
        let same = Seen { root_version: 3, epoch: 2, sequence: 5 };
        assert!(verify_catalog(&json, &sign(&s.catalog_kp, &json), &root, NOW, &same).unwrap_err().contains("older"));
        assert_eq!(same.after(&root, &c), same);
    }

    #[test]
    fn yank_stops_installs_and_revoke_stops_running() {
        let mut c: Catalog = serde_json::from_slice(&catalog_in(0, 1, LATER)).unwrap();
        c.plugins.push(entry("com.example.weather", "2.0.0", &"cd".repeat(32)));
        assert_eq!(c.installable(&"AB".repeat(32)).unwrap().version, "1.0.0");
        assert!(c.installable(&"ef".repeat(32)).is_err());
        c.revocations.push(Revocation { id: "com.example.weather".into(), versions: vec!["1.0.0".into()], action: Action::Yank, reason: "bug".into() });
        assert!(c.installable(&"ab".repeat(32)).unwrap_err().contains("withdrawn: bug"));
        assert!(c.runnable("com.example.weather", "1.0.0").is_ok(), "a yanked version keeps running");
        assert!(c.installable(&"cd".repeat(32)).is_ok(), "other versions aren't touched");
        c.revocations.push(Revocation { id: "com.example.weather".into(), versions: vec![], action: Action::Revoke, reason: String::new() });
        assert!(c.runnable("com.example.weather", "1.0.0").unwrap_err().contains("revoked"));
        assert!(c.runnable("com.example.weather", "2.0.0").is_err(), "empty versions covers all");
        assert!(c.runnable("com.example.other", "1.0.0").unwrap_err().contains("isn't in"));
    }

    #[test]
    fn v1_catalogs_read_with_v2_defaults() {
        let v1 = r#"{"format":"foxglove-plugin-catalog","sequence":2,"expires":"2030-01-01T00:00:00Z","plugins":[{"id":"a.b","version":"1","sha256":"00","files":{}}]}"#;
        let c: Catalog = serde_json::from_str(v1).unwrap();
        assert_eq!((c.epoch, c.plugins[0].channel.as_str(), c.plugins[0].url.clone(), c.revocations.len()), (0, "stable", None, 0));
    }

    #[test]
    fn files_must_match_exactly() {
        let dir = std::env::temp_dir().join(format!("plugin-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("plugin.toml"), b"id").unwrap();
        std::fs::write(dir.join("bin/run.exe"), b"MZ").unwrap();
        std::fs::write(dir.join("plugin.log"), b"log").unwrap();
        let files = hash_dir(&dir, &["plugin.log"]).unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), ["bin/run.exe", "plugin.toml"]);
        let entry = Entry { files, ..entry("x", "1", "") };
        check_files(&dir, &entry, &["plugin.log"]).unwrap();
        std::fs::write(dir.join("bin/run.exe"), b"MZ evil").unwrap();
        assert!(check_files(&dir, &entry, &["plugin.log"]).unwrap_err().contains("changed"));
        std::fs::write(dir.join("bin/run.exe"), b"MZ").unwrap();
        std::fs::write(dir.join("extra.dll"), b"x").unwrap();
        assert!(check_files(&dir, &entry, &["plugin.log"]).unwrap_err().contains("isn't part"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn now_is_rfc3339_utc() {
        let n = now_utc();
        assert!(time_ok(&n), "{n}");
        assert!(n.as_str() > "2026-01-01T00:00:00Z");
    }
}
