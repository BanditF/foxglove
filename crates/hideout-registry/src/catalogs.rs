// SPDX-License-Identifier: MIT
//! Accepted catalogs on disk, and fetching a source's catalog (spec §2.3 `catalogs\<source>\`,
//! §12.4, §13.4, §13.6; #1049).
//!
//! - Each source keeps one file, `catalogs\<source>\accepted.json` ([`Stored`]): the exact bytes
//!   and signature of the last catalog accepted from it (and, for a root-anchored source, the
//!   root's), its sequence, and when it was accepted. One file, so it is replaced in one atomic
//!   rename (temp file first) and the catalog and its signature never come apart.
//! - It is written before `seen` is raised, and the catalog is used only after both (§12.4): a
//!   crash in between leaves a stored sequence at or above `seen`, which still loads.
//! - [`Registry::open`] loads every configured source's file and verifies it again: signature,
//!   scope, epoch, expiry, and `seen` (a stored sequence below `seen` is a rollback and is
//!   refused).
//! - [`Registry::refresh_source`] fetches one source once (no retry) through a [`Fetch`] the
//!   caller gives: the hub's HTTPS client, or a stand-in in tests. Sources are fetched only on
//!   the user's action or at the hub's start, never on a timer (§13.4).

use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The UTC time now, `YYYY-MM-DDTHH:MM:SSZ`: what the hub passes as `now`.
pub use foxglove_plugin_trust::now_utc;
use hideout_contract::config::{Source, SourceKind};
use hideout_contract::{Error, ErrorId};
use serde::{Deserialize, Serialize};

use crate::trust::{verify_catalog, verify_root, Accepted, Root, Signer, ROOT_ANCHOR};
use crate::{Failure, Registry};

/// The folder under the hub's home that holds the accepted catalogs, one folder per source.
pub const CATALOGS_DIR: &str = "catalogs";
/// The one file in `catalogs\<source>\`.
pub const ACCEPTED_FILE: &str = "accepted.json";
/// [`Stored::format`].
pub const STORED_FORMAT: &str = "hideout-accepted-catalog-v1";
/// The largest `catalog.json` or `root.json` read, in bytes.
pub const MAX_CATALOG_BYTES: usize = 4 << 20;
/// The largest `.minisig` read, in bytes.
pub const MAX_SIGNATURE_BYTES: usize = 16 << 10;
/// The root beside a root-anchored source's `catalog.json` (same folder or URL path).
pub const ROOT_FILE: &str = "root.json";
/// A folder source's catalog file.
pub const CATALOG_FILE: &str = "catalog.json";

/// What `catalogs\<source>\accepted.json` holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub format: String,
    pub source: String,
    pub anchor: String,
    pub family: String,
    pub epoch: u64,
    pub sequence: u64,
    pub expires: String,
    /// When this hub accepted it, `YYYY-MM-DDTHH:MM:SSZ` (UTC).
    pub accepted: String,
    /// `catalog.json`, byte for byte (an accepted catalog is UTF-8 JSON).
    pub catalog: String,
    /// `catalog.json.minisig`.
    pub signature: String,
    /// A root-anchored source: the `root.json` it was checked against, and its signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_signature: Option<String>,
}

/// Gets the bytes at an `https://` URL, at most `max` of them; `Err` says why not (offline, a
/// status other than 200, too large). One request, no retry.
pub trait Fetch {
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String>;
}

impl<F: Fn(&str, usize) -> Result<Vec<u8>, String>> Fetch for F {
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        self(url, max)
    }
}

/// A source id is a folder name only when it is `a-z`, `0-9` and `-` (§13.4).
fn source_id_ok(id: &str) -> bool {
    let mut c = id.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase() || f.is_ascii_digit())
        && c.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

/// `catalogs\<source>\accepted.json` under `home`.
pub fn stored_path(home: &Path, source: &str) -> PathBuf {
    home.join(CATALOGS_DIR).join(source).join(ACCEPTED_FILE)
}

/// The stored catalog of `source`, if one was accepted.
pub fn read_stored(home: &Path, source: &str) -> io::Result<Option<Stored>> {
    if !source_id_ok(source) {
        return Ok(None);
    }
    let text = match std::fs::read_to_string(stored_path(home, source)) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let s: Stored = serde_json::from_str(&text).map_err(io::Error::other)?;
    if s.format != STORED_FORMAT || s.source != source {
        return Err(io::Error::other(format!(
            "{}: not a stored catalog of {source}",
            stored_path(home, source).display()
        )));
    }
    Ok(Some(s))
}

/// Writes `s` atomically: a temp file in the same folder, then one rename over the old file.
fn write_stored(home: &Path, s: &Stored) -> io::Result<()> {
    let path = stored_path(home, &s.source);
    let dir = path.parent().expect("a source folder");
    hideout_acl::create_private_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(s).map_err(io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

/// `url` with its last path segment replaced by `name` (`…/catalog.json` to `…/root.json`).
fn sibling(url: &str, name: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    match base.rfind('/') {
        Some(i) if i > "https://".len() => format!("{}{name}", &base[..=i]),
        _ => format!("{}/{name}", base.trim_end_matches('/')),
    }
}

/// A local file, at most `max` bytes.
fn read_capped(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    f.take(max as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if out.len() > max {
        return Err(format!("{} is over {max} bytes", path.display()));
    }
    Ok(out)
}

fn utf8(bytes: Vec<u8>, what: &str) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("{what} is not UTF-8"))
}

/// A catalog and its signature as fetched, with the root beside it for a root-anchored source.
struct Fetched {
    catalog: Vec<u8>,
    signature: String,
    root: Option<(Vec<u8>, String)>,
}

/// Reads one source's files: an `https://` URL through `fetch`, a folder from disk.
fn fetch_source(source: &Source, rooted: bool, fetch: &dyn Fetch) -> Result<Fetched, String> {
    let (catalog, signature, root) = match source.kind {
        SourceKind::Catalog => {
            let url = source.url.as_deref().ok_or("the source has no url")?;
            if !url
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
            {
                return Err("the source's url is not https".into());
            }
            let get = |u: &str, max| fetch.get(u, max).map_err(|e| format!("{u}: {e}"));
            let catalog = get(url, MAX_CATALOG_BYTES)?;
            let sig = utf8(
                get(&format!("{url}.minisig"), MAX_SIGNATURE_BYTES)?,
                "the signature",
            )?;
            let root = if rooted {
                let r = sibling(url, ROOT_FILE);
                let json = get(&r, MAX_CATALOG_BYTES)?;
                let sig = utf8(
                    get(&format!("{r}.minisig"), MAX_SIGNATURE_BYTES)?,
                    "the root signature",
                )?;
                Some((json, sig))
            } else {
                None
            };
            (catalog, sig, root)
        }
        SourceKind::Folder => {
            let dir = Path::new(source.path.as_deref().ok_or("the source has no path")?);
            if !dir.is_absolute() {
                return Err("the source's path is not absolute".into());
            }
            let catalog = read_capped(&dir.join(CATALOG_FILE), MAX_CATALOG_BYTES)?;
            let sig = utf8(
                read_capped(
                    &dir.join(format!("{CATALOG_FILE}.minisig")),
                    MAX_SIGNATURE_BYTES,
                )?,
                "the signature",
            )?;
            let root = if rooted {
                let json = read_capped(&dir.join(ROOT_FILE), MAX_CATALOG_BYTES)?;
                let sig = utf8(
                    read_capped(
                        &dir.join(format!("{ROOT_FILE}.minisig")),
                        MAX_SIGNATURE_BYTES,
                    )?,
                    "the root signature",
                )?;
                Some((json, sig))
            } else {
                None
            };
            (catalog, sig, root)
        }
        SourceKind::Git => return Err("git sources aren't fetched yet".into()),
    };
    Ok(Fetched {
        catalog,
        signature,
        root,
    })
}

impl Registry {
    /// The configured source `id` (`hideout.toml` `[[sources]]`).
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.hub.config.sources.iter().find(|s| s.id == id)
    }

    /// The stored catalog of `source` (sequence, accepted time), for Copy diagnostics.
    pub fn stored_catalog(&self, source: &str) -> Option<Stored> {
        read_stored(&self.home, source).ok().flatten()
    }

    /// Saves the accepted catalog of `source` (called by [`Registry::accept_catalog`] before
    /// `seen` is raised).
    pub(crate) fn store_catalog(
        &self,
        json: &[u8],
        signature: &str,
        anchor: &str,
        source: &str,
        catalog: &crate::trust::Catalog,
        now: &str,
    ) -> Result<(), Failure> {
        if !source_id_ok(source) {
            return Err(
                Error::new(ErrorId::ConfigSourceId, format!("bad source id {source:?}")).into(),
            );
        }
        let text = std::str::from_utf8(json)
            .map_err(|_| Error::new(ErrorId::ManifestSyntax, "catalog: not UTF-8"))?;
        let (root, root_signature) = match (anchor == ROOT_ANCHOR, &self.root_file) {
            (true, Some((r, s))) => (Some(r.clone()), Some(s.clone())),
            _ => (None, None),
        };
        let s = Stored {
            format: STORED_FORMAT.into(),
            source: source.into(),
            anchor: anchor.into(),
            family: catalog.family.clone(),
            epoch: catalog.epoch,
            sequence: catalog.sequence,
            expires: catalog.expires.clone(),
            accepted: now.into(),
            catalog: text.into(),
            signature: signature.into(),
            root,
            root_signature,
        };
        write_stored(&self.home, &s)?;
        Ok(())
    }

    /// Loads every configured source's stored catalog (at [`Registry::open`]), each verified
    /// again against its anchor, `seen` and `now`. A root-anchored one needs
    /// [`Registry::root_key`]. One line per source goes to `catalog_report`.
    pub fn load_catalogs(&mut self, now: &str) {
        let sources = self.hub.config.sources.clone();
        for s in &sources {
            let Some(anchor) = s.anchor.as_deref() else {
                continue;
            };
            let line = match self.load_one(s, anchor, now) {
                Ok(None) => continue,
                Ok(Some(st)) => format!(
                    "{}: catalog sequence {} (accepted {}) loaded",
                    s.id, st.sequence, st.accepted
                ),
                Err(why) => format!("{}: stored catalog not used: {why}", s.id),
            };
            self.catalog_report.push(line);
        }
    }

    fn load_one(&mut self, s: &Source, anchor: &str, now: &str) -> Result<Option<Stored>, String> {
        let Some(st) = read_stored(&self.home, &s.id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        if st.anchor != anchor {
            return Err(format!(
                "signed for anchor {}, the source names {anchor}",
                st.anchor
            ));
        }
        let seen = self.inventory.seen.get(anchor).cloned();
        let catalog = if anchor == ROOT_ANCHOR {
            let key = self
                .root_key
                .clone()
                .ok_or("this build has no pinned Hideout root key")?;
            let (Some(r), Some(rs)) = (&st.root, &st.root_signature) else {
                return Err("no root stored with it".into());
            };
            let root = verify_root(r.as_bytes(), rs, &key, now, seen.as_ref())
                .map_err(|e| e.to_string())?;
            let catalog = verify_catalog(
                st.catalog.as_bytes(),
                &st.signature,
                Signer::Root(&root),
                &s.id,
                now,
                seen.as_ref(),
            )
            .map_err(|e| e.to_string())?;
            self.keep_root(root, r, rs);
            catalog
        } else {
            let a = self.user_anchor(anchor).map_err(|e| e.to_string())?;
            let signer = Signer::User {
                key: &a.key,
                scope: &a.scope,
            };
            verify_catalog(
                st.catalog.as_bytes(),
                &st.signature,
                signer,
                &s.id,
                now,
                seen.as_ref(),
            )
            .map_err(|e| e.to_string())?
        };
        self.trust
            .catalogs
            .retain(|a| !(a.anchor == anchor && a.source == s.id));
        self.trust.catalogs.push(Accepted {
            anchor: anchor.into(),
            source: s.id.clone(),
            visibility: s.visibility,
            catalog,
        });
        Ok(Some(st))
    }

    /// Keeps the newest verified root (by version, then epoch) and its file.
    fn keep_root(&mut self, root: Root, json: &str, sig: &str) {
        let newer = self
            .trust
            .root
            .as_ref()
            .is_none_or(|r| (root.version, root.epoch) > (r.version, r.epoch));
        if newer {
            self.trust.root = Some(root);
            self.root_file = Some((json.into(), sig.into()));
        }
    }

    /// Fetches source `id`'s catalog once and accepts it (§12.4): saved, `seen` raised, then in
    /// the trust set. The caller tells the supervisor (`Supervisor::catalog_accepted`) after an
    /// `Ok`. A refused catalog (bad signature, a rollback, out of scope, expired) or one that
    /// couldn't be read leaves the stored one as it was. `now` is the UTC time.
    pub fn refresh_source(
        &mut self,
        id: &str,
        fetch: &dyn Fetch,
        now: &str,
    ) -> Result<Stored, Failure> {
        let source = self
            .source(id)
            .cloned()
            .ok_or_else(|| Failure::Source(format!("no source {id} in hideout.toml")))?;
        let anchor = source.anchor.clone().ok_or_else(|| {
            Failure::Source(format!("{id} has no anchor (a developer-mode folder)"))
        })?;
        let rooted = anchor == ROOT_ANCHOR;
        if rooted && self.root_key.is_none() {
            return Err(Failure::Source(
                "this build has no pinned Hideout root key".into(),
            ));
        }
        let got = fetch_source(&source, rooted, fetch).map_err(Failure::Source)?;
        if let Some((json, sig)) = &got.root {
            let key = self.root_key.clone().unwrap_or_default();
            let seen = self.inventory.seen.get(ROOT_ANCHOR);
            let root = verify_root(json, sig, &key, now, seen)?;
            let same = self
                .trust
                .root
                .as_ref()
                .is_some_and(|r| (r.version, r.epoch) == (root.version, root.epoch));
            if !same {
                self.accept_root(json, sig, &key, now)?;
            }
            let text = std::str::from_utf8(json)
                .map_err(|_| Error::new(ErrorId::ManifestSyntax, "root.json: not UTF-8"))?;
            self.root_file = Some((text.into(), sig.clone()));
        }
        self.accept_catalog(
            &got.catalog,
            &got.signature,
            &anchor,
            id,
            source.visibility,
            now,
        )?;
        self.stored_catalog(id)
            .ok_or_else(|| Failure::Source(format!("{id}: accepted but not read back")))
    }

    /// Check for updates for module `module` (#1026): [`Registry::refresh_source`] of the source
    /// it was installed from. A sideload or lab module has none.
    pub fn refresh_module_source(
        &mut self,
        module: &str,
        fetch: &dyn Fetch,
        now: &str,
    ) -> Result<Stored, Failure> {
        let source = self
            .inventory
            .modules
            .get(module)
            .ok_or_else(|| Failure::NotInstalled(module.into()))?
            .source
            .clone();
        if self.source(&source).is_none() {
            return Err(Failure::Source(format!(
                "{module} comes from {source}, which is no source in hideout.toml"
            )));
        }
        self.refresh_source(&source, fetch, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_json_sits_beside_the_catalog() {
        assert_eq!(
            sibling("https://h.example/a/b/catalog.json", ROOT_FILE),
            "https://h.example/a/b/root.json"
        );
        assert_eq!(
            sibling("https://h.example/catalog.json?x=1", ROOT_FILE),
            "https://h.example/root.json"
        );
        assert_eq!(
            sibling("https://h.example", ROOT_FILE),
            "https://h.example/root.json"
        );
    }

    #[test]
    fn source_ids_are_folder_safe() {
        assert!(source_id_ok("main-2"));
        for bad in ["", "..", "a/b", "A", "a\\b", "-x"] {
            assert!(!source_id_ok(bad), "{bad}");
        }
    }
}
