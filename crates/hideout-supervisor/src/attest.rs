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
