//! Per-plugin secrets (#213): passwords and tokens a plugin needs (an OBS password, an OAuth
//! refresh token), kept out of manifests, command lines, facts, logs and profile exports.
//!
//! - Stored in Windows Credential Manager under `Foxglove/plugin/<plugin id>/<name>`, so
//!   Windows protects them with the user's login.
//! - A plugin reaches only its own (the host knows which plugin a connection is); outside tools
//!   get none. It needs the `secrets` permission, shown at install.
//! - The user enters the ones a plugin declares (`[[secrets]]` in `plugin.toml`) in the
//!   Plugins tab; a plugin can also save its own (a refreshed token). Uninstalling a plugin
//!   deletes its secrets.

use std::sync::OnceLock;

/// Where secrets live: Credential Manager, or memory in tests.
pub trait Store: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, value: Option<&str>) -> Result<(), String>;
    /// Deletes every key starting with `prefix`.
    fn delete_prefix(&self, prefix: &str) -> Result<(), String>;
}

static STORE: OnceLock<Box<dyn Store>> = OnceLock::new();

fn store() -> &'static dyn Store {
    STORE
        .get_or_init(|| Box::new(imp::CredentialManager))
        .as_ref()
}

/// Uses another store (tests); only before the first use.
pub fn use_store(s: Box<dyn Store>) -> bool {
    STORE.set(s).is_ok()
}

const MAX_VALUE: usize = 2048;

fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    ok.then_some(())
        .ok_or_else(|| format!("secret names are 1-64 of a-z, 0-9, - _ . (got {name:?})"))
}

fn prefix(plugin: &str) -> String {
    format!("plugin/{plugin}/")
}

pub fn get(plugin: &str, name: &str) -> Result<Option<String>, String> {
    check_name(name)?;
    store().get(&format!("{}{name}", prefix(plugin)))
}

/// Saves (or with `None` removes) one of a plugin's secrets.
pub fn set(plugin: &str, name: &str, value: Option<&str>) -> Result<(), String> {
    check_name(name)?;
    if value.is_some_and(|v| v.len() > MAX_VALUE) {
        return Err(format!("a secret is at most {MAX_VALUE} bytes"));
    }
    store().set(&format!("{}{name}", prefix(plugin)), value)
}

/// Deletes all of a plugin's secrets (when it's uninstalled).
pub fn delete_all(plugin: &str) -> Result<(), String> {
    store().delete_prefix(&prefix(plugin))
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    pub struct CredentialManager;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn target(key: &str) -> Vec<u16> {
        wide(&format!("Foxglove/{key}"))
    }

    fn os(what: &str) -> String {
        format!("{what}: {}", std::io::Error::last_os_error())
    }

    impl super::Store for CredentialManager {
        fn get(&self, key: &str) -> Result<Option<String>, String> {
            let name = target(key);
            let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
            unsafe {
                if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 {
                    return if GetLastError() == ERROR_NOT_FOUND {
                        Ok(None)
                    } else {
                        Err(os("reading a secret"))
                    };
                }
                let c = &*cred;
                let bytes =
                    std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
                        .to_vec();
                CredFree(cred as _);
                String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| "a secret isn't text".into())
            }
        }

        fn set(&self, key: &str, value: Option<&str>) -> Result<(), String> {
            let name = target(key);
            unsafe {
                let Some(value) = value else {
                    if CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) == 0
                        && GetLastError() != ERROR_NOT_FOUND
                    {
                        return Err(os("removing a secret"));
                    }
                    return Ok(());
                };
                let mut user = wide("Foxglove");
                let mut blob = value.as_bytes().to_vec();
                let mut cred: CREDENTIALW = std::mem::zeroed();
                cred.Type = CRED_TYPE_GENERIC;
                cred.TargetName = name.as_ptr() as *mut u16;
                cred.CredentialBlobSize = blob.len() as u32;
                cred.CredentialBlob = blob.as_mut_ptr();
                cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
                cred.UserName = user.as_mut_ptr();
                if CredWriteW(&cred, 0) == 0 {
                    return Err(os("saving a secret"));
                }
            }
            Ok(())
        }

        fn delete_prefix(&self, prefix: &str) -> Result<(), String> {
            let filter = wide(&format!("Foxglove/{prefix}*"));
            let mut count = 0u32;
            let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
            unsafe {
                if CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut list) == 0 {
                    return if GetLastError() == ERROR_NOT_FOUND {
                        Ok(())
                    } else {
                        Err(os("listing secrets"))
                    };
                }
                for i in 0..count as usize {
                    let c = &**list.add(i);
                    CredDeleteW(c.TargetName, CRED_TYPE_GENERIC, 0);
                }
                CredFree(list as _);
            }
            Ok(())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct CredentialManager;

    impl super::Store for CredentialManager {
        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Err("secrets are stored only on Windows so far".into())
        }
        fn set(&self, _: &str, _: Option<&str>) -> Result<(), String> {
            Err("secrets are stored only on Windows so far".into())
        }
        fn delete_prefix(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
}

/// A store in memory, for tests.
#[derive(Default)]
pub struct Memory(std::sync::Mutex<std::collections::BTreeMap<String, String>>);

impl Store for Memory {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }
    fn set(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match value {
            Some(v) => m.insert(key.into(), v.into()),
            None => m.remove(key),
        };
        Ok(())
    }
    fn delete_prefix(&self, prefix: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k, _| !k.starts_with(prefix));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sizes_are_checked() {
        assert!(check_name("obs-password").is_ok());
        for bad in ["", "Bad", "a/b", "x y", &"a".repeat(65)] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert!(set("p", "x", Some(&"v".repeat(MAX_VALUE + 1))).is_err());
    }

    /// The real Credential Manager, under a test-only plugin id, cleaned up after. It writes to
    /// this user's vault, so it runs only when asked: `cargo test -p foxglove-plugin-host -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn credential_manager_round_trip() {
        let store = imp::CredentialManager;
        let key = format!("plugin/test.secrets-{}/token", std::process::id());
        store.set(&key, Some("s3cret")).unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("s3cret"));
        store
            .delete_prefix(&key[..key.rfind('/').unwrap() + 1])
            .unwrap();
        assert_eq!(store.get(&key).unwrap(), None);
        store.set(&key, None).unwrap();
    }
}
