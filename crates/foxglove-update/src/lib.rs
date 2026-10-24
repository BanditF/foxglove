//! Signed update metadata and transport-independent verification. MIT licensed.
use base64::{engine::general_purpose::STANDARD, Engine};
pub type PublicKey = [u8; 32];
type VerifyingKey = PublicKey;
mod version;
use ring::{digest, signature};
use serde::{Deserialize, Serialize};
use std::io::Read;
use version::Version;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Msi {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub tag: String,
    pub msi: Msi,
    pub notes: String,
    pub published: String,
}
#[derive(Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}
#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub html_url: String,
    pub assets: Vec<Asset>,
}
pub fn tag_version(tag: &str) -> Option<Version> {
    let v = Version::parse(tag.strip_prefix("app-v")?).ok()?;
    v.pre.is_empty().then_some(v)
}
pub fn newest(releases: &[Release], current: &str) -> Result<Option<usize>> {
    let current = Version::parse(current)?;
    Ok(releases
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.draft && !r.prerelease)
        .filter_map(|(i, r)| {
            tag_version(&r.tag_name)
                .filter(|v| v > &current)
                .map(|v| (i, v))
        })
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|(i, _)| i))
}
pub fn public_key(text: &str) -> Result<VerifyingKey> {
    Ok(STANDARD
        .decode(text.trim())?
        .try_into()
        .map_err(|_| "Expected a 32-byte public key")?)
}
pub fn verify(bytes: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<Manifest> {
    let signature = STANDARD.decode(std::str::from_utf8(signature)?.trim())?;
    signature::UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(bytes, &signature)
        .map_err(|_| "Invalid update signature")?;
    Ok(serde_json::from_slice(bytes)?)
}
pub fn hash(mut reader: impl Read) -> Result<(String, u64)> {
    let mut hasher = digest::Context::new(&digest::SHA256);
    let mut size = 0;
    let mut buf = [0; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((
        hasher
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        size,
    ))
}
pub fn verify_file(path: &std::path::Path, msi: &Msi) -> Result<()> {
    let result = hash(std::fs::File::open(path)?);
    match result {
        Ok((sha, size)) if sha.eq_ignore_ascii_case(&msi.sha256) && size == msi.size => Ok(()),
        _ => {
            let _ = std::fs::remove_file(path);
            Err("Installer size or SHA-256 mismatch".into())
        }
    }
}
/// Implementations must enforce HTTPS, timeouts and response size limits.
pub trait Transport {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>>;
}
pub fn check(
    http: &impl Transport,
    releases: &[Release],
    current: &str,
    key: &VerifyingKey,
) -> Result<Option<(Manifest, String)>> {
    let Some(i) = newest(releases, current)? else {
        return Ok(None);
    };
    let r = &releases[i];
    let asset = |name: &str| {
        r.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.as_str())
            .ok_or("Missing signed update metadata")
    };
    let bytes = http.get(asset("latest.json")?, 1024 * 1024)?;
    let sig = http.get(asset("latest.json.sig")?, 1024)?;
    let m = verify(&bytes, &sig, key)?;
    if m.tag != r.tag_name
        || tag_version(&m.tag).as_ref() != Some(&Version::parse(&m.version)?)
        || Version::parse(&m.version)? <= Version::parse(current)?
    {
        return Err("Manifest version does not match release or is not newer".into());
    }
    if m.msi.name != "foxglove-x64.msi" || m.msi.size == 0 {
        return Err("Invalid installer metadata".into());
    }
    Ok(Some((m, r.html_url.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::collections::HashMap;

    struct Fake(HashMap<String, Vec<u8>>);

    impl Transport for Fake {
        fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
            let body = self.0.get(url).ok_or("not found")?.clone();
            if body.len() as u64 > limit {
                return Err("too large".into());
            }
            Ok(body)
        }
    }

    fn keypair() -> (Ed25519KeyPair, PublicKey) {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let public = pair.public_key().as_ref().try_into().unwrap();
        (pair, public)
    }

    fn release(tag: &str, draft: bool, prerelease: bool) -> Release {
        let url = |name: &str| Asset {
            name: name.into(),
            browser_download_url: format!("https://x/{tag}/{name}"),
        };
        Release {
            tag_name: tag.into(),
            draft,
            prerelease,
            html_url: format!("https://x/{tag}"),
            assets: vec![url("latest.json"), url("latest.json.sig")],
        }
    }

    fn manifest(version: &str) -> Vec<u8> {
        serde_json::to_vec(&Manifest {
            version: version.into(),
            tag: format!("app-v{version}"),
            msi: Msi {
                name: "foxglove-x64.msi".into(),
                url: "https://x/foxglove-x64.msi".into(),
                sha256: "00".into(),
                size: 3,
            },
            notes: "notes".into(),
            published: "2026-10-24T00:00:00Z".into(),
        })
        .unwrap()
    }

    fn served(tag: &str, bytes: Vec<u8>, pair: &Ed25519KeyPair) -> Fake {
        let sig = STANDARD.encode(pair.sign(&bytes).as_ref()).into_bytes();
        Fake(HashMap::from([
            (format!("https://x/{tag}/latest.json"), bytes),
            (format!("https://x/{tag}/latest.json.sig"), sig),
        ]))
    }

    #[test]
    fn versions_order_and_tags_filter() {
        assert!(Version::parse("1.2.10").unwrap() > Version::parse("1.2.9").unwrap());
        assert!(Version::parse("1.0.0").unwrap() > Version::parse("1.0.0-rc.1").unwrap());
        assert!(Version::parse("01.0.0").is_err());
        assert!(tag_version("fw-v9.0.0").is_none());
        assert!(tag_version("app-v2.0.0-beta").is_none());
        let releases = [
            release("fw-v9.0.0", false, false),
            release("app-v0.3.0", false, true),
            release("app-v0.4.0", true, false),
            release("app-v0.2.0", false, false),
            release("app-v0.1.5", false, false),
        ];
        assert_eq!(newest(&releases, "0.1.0").unwrap(), Some(3));
        assert_eq!(newest(&releases, "0.2.0").unwrap(), None);
        assert_eq!(
            newest(&releases, "0.5.0").unwrap(),
            None,
            "never a downgrade"
        );
    }

    #[test]
    fn signature_must_match_exact_bytes_and_key() {
        let (pair, public) = keypair();
        let bytes = manifest("0.2.0");
        let sig = STANDARD.encode(pair.sign(&bytes).as_ref());
        assert_eq!(
            verify(&bytes, sig.as_bytes(), &public).unwrap().version,
            "0.2.0"
        );
        let mut tampered = bytes.clone();
        tampered[5] ^= 1;
        assert!(verify(&tampered, sig.as_bytes(), &public).is_err());
        let other = Ed25519KeyPair::from_seed_unchecked(&[8; 32]).unwrap();
        let wrong = other.public_key().as_ref().try_into().unwrap();
        assert!(verify(&bytes, sig.as_bytes(), &wrong).is_err());
        assert!(public_key("PLACEHOLDER: replace").is_err());
    }

    #[test]
    fn check_accepts_signed_newer_and_rejects_mismatches() {
        let (pair, public) = keypair();
        let releases = [release("app-v0.2.0", false, false)];
        let fake = served("app-v0.2.0", manifest("0.2.0"), &pair);
        let (m, page) = check(&fake, &releases, "0.1.0", &public).unwrap().unwrap();
        assert_eq!(
            (m.version.as_str(), page.as_str()),
            ("0.2.0", "https://x/app-v0.2.0")
        );
        assert!(check(&fake, &releases, "0.2.0", &public).unwrap().is_none());
        // A validly signed manifest for another version can't be replayed on this release.
        let replay = served("app-v0.2.0", manifest("0.1.1"), &pair);
        assert!(check(&replay, &releases, "0.1.0", &public).is_err());
    }

    #[test]
    fn installer_hash_and_size_are_checked() {
        let dir = std::env::temp_dir().join(format!("foxglove-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.msi");
        std::fs::write(&path, b"abc").unwrap();
        let (sha, size) = hash(&b"abc"[..]).unwrap();
        let good = Msi {
            name: "foxglove-x64.msi".into(),
            url: String::new(),
            sha256: sha.clone(),
            size,
        };
        verify_file(&path, &good).unwrap();
        let bad = Msi {
            sha256: "00".repeat(32),
            ..good.clone()
        };
        assert!(verify_file(&path, &bad).is_err());
        assert!(!path.exists(), "a mismatched download is deleted");
        std::fs::write(&path, b"abcd").unwrap();
        assert!(verify_file(&path, &good).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
