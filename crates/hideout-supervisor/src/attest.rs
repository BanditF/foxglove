// SPDX-License-Identifier: MIT
//! The updater's vouching for a new program file (#1178, contract §6.4). The only caller of
//! `Registry::attest_update` outside tests. It takes no path: the new program is whatever
//! `run[0]` names now, and the registry insists it sits in the folder the grant is bound to.
//!
//! The proof that an update really came from the updater is the Ed25519 signature on the
//! release's `latest.json` (the same key and check the app's updater uses): a [`Verified`]
//! cannot be made any other way, and nothing a module can send over the router builds one.

use std::path::Path;

use hideout_registry::Registry;

/// A release manifest whose signature checked out against the update key.
pub struct Verified {
    version: String,
}

impl Verified {
    /// Checks `signature` (base64, as published beside `latest.json`) over `latest_json`.
    pub fn from_signed(
        latest_json: &[u8],
        signature: &[u8],
        key: &foxglove_update::PublicKey,
    ) -> Result<Verified, String> {
        let m = foxglove_update::verify(latest_json, signature, key).map_err(|e| e.to_string())?;
        Ok(Verified { version: m.version })
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Records, for module `id`, that the program now installed is the next version of the one its
/// grant is bound to. Keeps the grant (history in `bound.attested`) only for a bound, not void
/// grant whose program stayed in the same install folder; anything else is an error and the
/// change voids at the next check as before.
pub fn attest_installed(home: &Path, id: &str, update: &Verified) -> Result<(), String> {
    let mut reg = Registry::open(home, hideout_registry::dev_mode_from_env())
        .map_err(|e| format!("{e:?}"))?;
    reg.attest_update(
        id,
        &update.version,
        "updater",
        &hideout_registry::catalogs::now_utc(),
    )
    .map_err(|e| e.id().map_or_else(|| format!("{e:?}"), String::from))
}

/// The update key the app's updater checks with; the hub checks the same release signature.
/// A build without the real key (a placeholder) can't attest, and says so in the log.
pub fn update_key() -> Option<foxglove_update::PublicKey> {
    foxglove_update::public_key(include_str!("../../../app/resources/update-key.pub")).ok()
}

/// After the legacy Foxglove module's planned exit for an update install (#1179): takes the
/// updater's hand-off from `dir`, checks its signature against `key` here (the file is only a
/// carrier; a module that can write it can't sign), and attests the program now at `run[0]`.
///
/// Guards: the file is used once and only while fresh; the release must be newer than any
/// version already attested for this module (a captured old release can't be replayed); and
/// `attest_installed` still insists on a bound, not void grant and the same install folder.
/// Returns the attested version, or `None` when there was nothing usable (logged).
pub fn attest_from_handoff(
    home: &Path,
    dir: &Path,
    id: &str,
    key: &foxglove_update::PublicKey,
) -> Option<String> {
    let signed = foxglove_update::handoff::take(dir, std::time::SystemTime::now())?;
    let v = match Verified::from_signed(&signed.json, &signed.sig, key) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("supervisor: {id}: update hand-off ignored: {e}");
            return None;
        }
    };
    let newest_attested = Registry::open(home, hideout_registry::dev_mode_from_env())
        .ok()
        .and_then(|r| {
            r.inventory
                .modules
                .get(id)?
                .bound
                .as_ref()?
                .attested
                .iter()
                .filter_map(|a| semver_triple(&a.version))
                .max()
        });
    if let (Some(old), Some(new)) = (newest_attested, semver_triple(&v.version)) {
        if new <= old {
            log::warn!(
                "supervisor: {id}: update hand-off ignored: {} is not newer than an attested release",
                v.version
            );
            return None;
        }
    }
    match attest_installed(home, id, &v) {
        Ok(()) => {
            log::info!(
                "supervisor: {id}: grant kept across the update to {}",
                v.version
            );
            Some(v.version)
        }
        Err(e) => {
            log::warn!("supervisor: {id}: update not attested ({e}); the change voids as before");
            None
        }
    }
}

fn semver_triple(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}
