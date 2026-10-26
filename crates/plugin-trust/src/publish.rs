//! Editing the catalog when publishing (#190): adding a released version, withdrawing one,
//! and bumping the sequence and expiry before it's signed. Signing itself stays with the
//! maintainer (`minisign -S`); nothing here touches a private key.

use std::path::Path;

use crate::{Action, Catalog, Entry, Revocation, FORMAT_V2};

/// Versions of a plugin kept per channel: the new one and one before it, for rollback.
pub const KEEP: usize = 2;

/// An empty v2 catalog in `epoch` (sequence 0, so the first [`bump`] makes it 1).
pub fn new_catalog(epoch: u64) -> Catalog {
    Catalog {
        format: FORMAT_V2.into(),
        epoch,
        sequence: 0,
        expires: crate::now_utc(),
        plugins: vec![],
        revocations: vec![],
    }
}

/// `1.10.0` after `1.9.0`: numeric where both parts are numbers.
fn version_key(v: &str) -> Vec<(u64, String)> {
    v.split(['.', '-', '+'])
        .map(|p| (p.parse().unwrap_or(0), p.to_string()))
        .collect()
}

/// Adds (or replaces) an entry, then keeps only the newest [`KEEP`] versions of that plugin in
/// its channel.
pub fn add(catalog: &mut Catalog, entry: Entry) {
    catalog.format = FORMAT_V2.into();
    catalog
        .plugins
        .retain(|e| !(e.id == entry.id && e.version == entry.version));
    let (id, channel) = (entry.id.clone(), entry.channel.clone());
    catalog.plugins.push(entry);
    let mut same: Vec<String> = catalog
        .plugins
        .iter()
        .filter(|e| e.id == id && e.channel == channel)
        .map(|e| e.version.clone())
        .collect();
    same.sort_by_key(|v| std::cmp::Reverse(version_key(v)));
    let keep: Vec<String> = same.into_iter().take(KEEP).collect();
    catalog
        .plugins
        .retain(|e| e.id != id || e.channel != channel || keep.contains(&e.version));
    catalog.plugins.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| version_key(&b.version).cmp(&version_key(&a.version)))
    });
}

/// Withdraws versions of a plugin (all of them when `versions` is empty).
pub fn withdraw(
    catalog: &mut Catalog,
    id: &str,
    versions: Vec<String>,
    action: Action,
    reason: &str,
) {
    catalog.format = FORMAT_V2.into();
    catalog
        .revocations
        .retain(|r| !(r.id == id && r.versions == versions));
    catalog.revocations.push(Revocation {
        id: id.into(),
        versions,
        action,
        reason: reason.into(),
    });
}

/// Ready to sign: the next sequence and a fresh expiry, `days` from now (30 by default; the
/// catalog is re-signed weekly so it never lapses).
pub fn bump(catalog: &mut Catalog, days: u64) {
    catalog.format = FORMAT_V2.into();
    catalog.sequence += 1;
    catalog.expires = crate::utc_in(days * 86_400);
}

/// The entry for a packed plugin: `dir` is the unpacked folder (with `plugin.toml`), `zip` the
/// archive as uploaded, `url` where it's downloaded from.
pub fn entry_from(dir: &Path, zip: &Path, url: &str, channel: &str) -> Result<Entry, String> {
    let manifest = std::fs::read_to_string(dir.join("plugin.toml"))
        .map_err(|e| format!("plugin.toml: {e}"))?;
    let field = |name: &str| {
        manifest
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once('=')?;
                (k.trim() == name).then(|| v.trim().trim_matches('"').to_string())
            })
            .ok_or_else(|| format!("plugin.toml has no {name}"))
    };
    let bytes = std::fs::read(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    if !url.starts_with("https://") {
        return Err(format!("the download URL must use HTTPS: {url}"));
    }
    Ok(Entry {
        id: field("id")?,
        version: field("version")?,
        channel: channel.into(),
        sha256: crate::sha256_hex(&bytes),
        url: Some(url.into()),
        size: Some(bytes.len() as u64),
        files: crate::hash_dir(dir, &[]).map_err(|e| e.to_string())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn entry(id: &str, version: &str, channel: &str) -> Entry {
        Entry {
            id: id.into(),
            version: version.into(),
            channel: channel.into(),
            sha256: format!("{id}{version}"),
            url: None,
            size: None,
            files: BTreeMap::new(),
        }
    }

    fn versions(c: &Catalog, id: &str) -> Vec<String> {
        c.plugins
            .iter()
            .filter(|e| e.id == id)
            .map(|e| format!("{}:{}", e.version, e.channel))
            .collect()
    }

    #[test]
    fn keeps_the_new_version_and_one_before_per_channel() {
        let mut c = new_catalog(1);
        for v in ["1.9.0", "1.10.0", "1.2.0"] {
            add(&mut c, entry("a.b", v, "stable"));
        }
        assert_eq!(versions(&c, "a.b"), ["1.10.0:stable", "1.9.0:stable"]);
        add(&mut c, entry("a.b", "2.0.0-beta1", "beta"));
        add(&mut c, entry("c.d", "1.0.0", "stable"));
        assert_eq!(
            versions(&c, "a.b"),
            ["2.0.0-beta1:beta", "1.10.0:stable", "1.9.0:stable"]
        );
        // Re-adding a version replaces it.
        let mut again = entry("a.b", "1.10.0", "stable");
        again.sha256 = "new".into();
        add(&mut c, again);
        assert_eq!(
            c.plugins.iter().filter(|e| e.version == "1.10.0").count(),
            1
        );
        assert_eq!(
            c.plugins
                .iter()
                .find(|e| e.version == "1.10.0")
                .unwrap()
                .sha256,
            "new"
        );
    }

    #[test]
    fn bump_and_withdraw() {
        let mut c = new_catalog(3);
        bump(&mut c, 30);
        assert_eq!((c.format.as_str(), c.epoch, c.sequence), (FORMAT_V2, 3, 1));
        assert!(c.expires.as_str() > crate::utc_in(29 * 86_400).as_str());
        withdraw(
            &mut c,
            "a.b",
            vec!["1.0.0".into()],
            Action::Yank,
            "crash on start",
        );
        withdraw(&mut c, "a.b", vec!["1.0.0".into()], Action::Revoke, "worse");
        assert_eq!(
            c.revocations.len(),
            1,
            "the same versions are replaced, not stacked"
        );
        assert_eq!(c.revocation("a.b", "1.0.0").unwrap().action, Action::Revoke);
    }

    #[test]
    fn entry_from_a_packed_plugin() {
        let dir = std::env::temp_dir().join(format!("plugin-publish-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "id = \"org.foxglove.x\"\nname = \"X\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("x.exe"), "MZ").unwrap();
        let zip = dir.with_extension("zip");
        std::fs::write(&zip, "PK").unwrap();
        let e = entry_from(
            &dir,
            &zip,
            "https://github.com/BanditF/foxglove/releases/download/plugin-x-v1.2.3/x.zip",
            "stable",
        )
        .unwrap();
        assert_eq!(
            (e.id.as_str(), e.version.as_str(), e.size),
            ("org.foxglove.x", "1.2.3", Some(2))
        );
        assert_eq!(e.sha256, crate::sha256_hex(b"PK"));
        assert_eq!(e.files.keys().collect::<Vec<_>>(), ["plugin.toml", "x.exe"]);
        assert!(entry_from(&dir, &zip, "http://example.com/x.zip", "stable").is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(zip);
    }
}
