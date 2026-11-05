//! Making the keys and signing `root.json` and `catalog.json` (#785), for `plugin-sign`. Behind
//! the `sign` feature, so the app, which only verifies, never links a signer.
//!
//! Keys are minisign's own format, so `minisign -V` checks what this signs and the files from
//! `minisign -G` load here. Everything signed is verified again, with the same checks the app
//! makes, before it's returned.

use minisign::{KeyPair, PublicKey, SecretKey, SecretKeyBox};

use crate::{Catalog, Root, Seen, FORMAT_V2};

/// A new key pair in minisign's file format.
pub struct KeyFiles {
    /// The `.key` file: encrypted with the password, or not when it's empty.
    pub secret: String,
    /// The `.pub` file.
    pub public: String,
    /// The public key's base64 line: what `root.pub` and `catalog_keys` hold.
    pub public_base64: String,
}

/// A new key pair. `None` asks for the password on the terminal; `Some("")` leaves the key
/// unencrypted (minisign's `-W`).
pub fn keygen(password: Option<String>) -> Result<KeyFiles, String> {
    let kp = KeyPair::generate_encrypted_keypair(password).map_err(|e| e.to_string())?;
    Ok(KeyFiles {
        secret: kp.sk.to_box(None).map_err(|e| e.to_string())?.into_string(),
        public: kp.pk.to_box().map_err(|e| e.to_string())?.into_string(),
        public_base64: kp.pk.to_base64(),
    })
}

/// A secret key from its `.key` file. `None` asks for the password on the terminal; `Some("")`
/// is for an unencrypted key.
pub fn load_secret(text: &str, password: Option<String>) -> Result<SecretKey, String> {
    SecretKey::from_box(
        SecretKeyBox::from_string(text).map_err(|e| e.to_string())?,
        password,
    )
    .map_err(|e| format!("can't open the secret key: {e}"))
}

/// The base64 public key in a `.pub` file, or a file holding just that line (`root.pub`).
pub fn public_base64(text: &str) -> Result<String, String> {
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
        .ok_or("no public key in the file")?;
    if line.starts_with("PLACEHOLDER") {
        return Err("the pinned root key is still the placeholder (#90)".into());
    }
    PublicKey::from_base64(line).map_err(|e| format!("not a minisign public key: {e}"))?;
    Ok(line.to_string())
}

/// The base64 public key of a secret key.
pub fn public_of(sk: &SecretKey) -> Result<String, String> {
    Ok(PublicKey::from_secret_key(sk)
        .map_err(|e| e.to_string())?
        .to_base64())
}

/// A minisign signature (`.minisig` file) over `data`.
pub fn sign(sk: &SecretKey, data: &[u8]) -> Result<String, String> {
    let pk = PublicKey::from_secret_key(sk).map_err(|e| e.to_string())?;
    Ok(minisign::sign(Some(&pk), sk, data, None, None)
        .map_err(|e| e.to_string())?
        .into_string())
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text.into_bytes())
}

/// `root.json` and its signature, made offline with the root key. It names `catalog_keys` and
/// expires `days` from now.
pub fn sign_root(
    root_sk: &SecretKey,
    catalog_keys: Vec<String>,
    version: u64,
    epoch: u64,
    days: u64,
) -> Result<(Root, Vec<u8>, String), String> {
    let root = Root {
        format: "foxglove-plugin-root".into(),
        version,
        epoch,
        expires: crate::utc_in(days * 86_400),
        catalog_keys,
    };
    let json = to_json(&root)?;
    let sig = sign(root_sk, &json)?;
    crate::verify_root(
        &json,
        &sig,
        &public_of(root_sk)?,
        &crate::now_utc(),
        &Seen::default(),
    )?;
    Ok((root, json, sig))
}

/// The catalog to publish: `source`'s plugins and revocations, in `root`'s epoch, expiring
/// `days` from now, with a sequence above both the source's and what's published in that
/// epoch, so no install that saw the published one refuses it.
pub fn next_catalog(
    source: &Catalog,
    root: &Root,
    published: Option<&Catalog>,
    days: u64,
) -> Catalog {
    let published_seq = published
        .filter(|p| p.epoch == root.epoch)
        .map_or(0, |p| p.sequence);
    Catalog {
        format: FORMAT_V2.into(),
        epoch: root.epoch,
        sequence: source.sequence.max(published_seq) + 1,
        expires: crate::utc_in(days * 86_400),
        plugins: source.plugins.clone(),
        revocations: source.revocations.clone(),
    }
}

/// `catalog.json` and its signature. Checked against `root` first: the catalog key must be one
/// it names, as the app requires.
pub fn sign_catalog(
    catalog_sk: &SecretKey,
    root: &Root,
    catalog: &Catalog,
) -> Result<(Vec<u8>, String), String> {
    let key = public_of(catalog_sk)?;
    if !root.catalog_keys.contains(&key) {
        return Err(format!("root.json doesn't name this catalog key ({key})"));
    }
    let json = to_json(catalog)?;
    let sig = sign(catalog_sk, &json)?;
    crate::verify_catalog(&json, &sig, root, &crate::now_utc(), &Seen::default())?;
    Ok((json, sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unencrypted() -> (KeyFiles, SecretKey) {
        let k = keygen(Some(String::new())).unwrap();
        let sk = load_secret(&k.secret, Some(String::new())).unwrap();
        (k, sk)
    }

    #[test]
    fn keys_round_trip_through_their_files() {
        let (k, sk) = unencrypted();
        assert_eq!(public_of(&sk).unwrap(), k.public_base64);
        assert_eq!(public_base64(&k.public).unwrap(), k.public_base64);
        assert_eq!(
            public_base64(&format!("{}\n", k.public_base64)).unwrap(),
            k.public_base64
        );

        let locked = keygen(Some("hunter2".into())).unwrap();
        assert!(load_secret(&locked.secret, Some(String::new())).is_err());
        let sk = load_secret(&locked.secret, Some("hunter2".into())).unwrap();
        assert_eq!(public_of(&sk).unwrap(), locked.public_base64);
    }

    #[test]
    fn the_placeholder_is_not_a_key() {
        let placeholder = include_str!("../../../app/resources/plugin-trust/root.pub");
        if placeholder.starts_with("PLACEHOLDER") {
            assert!(public_base64(placeholder)
                .unwrap_err()
                .contains("placeholder"));
        }
        assert!(public_base64("").is_err());
        assert!(public_base64("not a key").is_err());
    }

    #[test]
    fn signs_a_chain_the_app_accepts() {
        let (root_k, root_sk) = unencrypted();
        let (cat_k, cat_sk) = unencrypted();
        let (root, root_json, root_sig) =
            sign_root(&root_sk, vec![cat_k.public_base64.clone()], 1, 2, 730).unwrap();

        let now = crate::now_utc();
        let verified = crate::verify_root(
            &root_json,
            &root_sig,
            &root_k.public_base64,
            &now,
            &Seen::default(),
        )
        .unwrap();
        assert_eq!((verified.version, verified.epoch), (1, 2));

        let source = crate::publish::new_catalog(0);
        let published = Catalog {
            epoch: 2,
            sequence: 7,
            ..source.clone()
        };
        let next = next_catalog(&source, &root, Some(&published), 30);
        assert_eq!((next.epoch, next.sequence), (2, 8));
        assert_eq!(next_catalog(&source, &root, None, 30).sequence, 1);
        let old_epoch = Catalog {
            epoch: 1,
            sequence: 50,
            ..source.clone()
        };
        assert_eq!(
            next_catalog(&source, &root, Some(&old_epoch), 30).sequence,
            1,
            "a new epoch restarts the sequence"
        );

        let (json, sig) = sign_catalog(&cat_sk, &root, &next).unwrap();
        let seen = Seen {
            root_version: 1,
            epoch: 2,
            sequence: 7,
        };
        let c = crate::verify_catalog(&json, &sig, &verified, &now, &seen).unwrap();
        assert_eq!(c.sequence, 8);

        // The root key isn't a catalog key, and a tampered catalog doesn't verify.
        assert!(sign_catalog(&root_sk, &root, &next)
            .unwrap_err()
            .contains("doesn't name"));
        let mut tampered = json.clone();
        tampered.extend_from_slice(b" ");
        assert!(crate::verify_catalog(&tampered, &sig, &verified, &now, &seen).is_err());
    }
}
