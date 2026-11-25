// SPDX-License-Identifier: MIT
//! Module secrets (`[[secrets]]`, §4.7; Foxglove API `secrets.get`/`secrets.set`): kept by the
//! hub in Windows Credential Manager for the current user, one generic credential per module
//! and name (`Hideout/<module>/<name>`). The hub's form only ever writes them and asks whether
//! one is saved; the value goes to its own module only.

use std::collections::HashMap;

/// At most this many bytes per secret (Foxglove API 1.2).
pub const MAX_SECRET: usize = 2048;

/// Where secrets live. The router holds one; tests use [`MemorySecrets`].
pub trait SecretStore: Send {
    fn get(&self, target: &str) -> Option<String>;
    /// `None` removes it.
    fn set(&mut self, target: &str, value: Option<&str>) -> Result<(), String>;
    /// Removes every secret saved for `module`, whatever its name (also names an older
    /// version declared). Other modules' secrets are never touched.
    fn remove_all(&mut self, module: &str) -> Result<(), String>;
}

/// The credential name for a module's secret.
pub fn target(module: &str, name: &str) -> String {
    format!("{}{name}", prefix(module))
}

/// What every credential name of `module` starts with, and no other module's does (an id
/// holds no `/`).
fn prefix(module: &str) -> String {
    format!("Hideout/{module}/")
}

/// Whether `module` can be asked for as a whole: a non-empty id with none of the characters
/// that would widen the match (`*`) or reach another module's names (`/`, `\`).
fn whole_module(module: &str) -> bool {
    !module.is_empty() && !module.contains(['*', '/', '\\', '\0'])
}

/// In memory, for tests and non-Windows builds.
#[derive(Default)]
pub struct MemorySecrets(pub HashMap<String, String>);

impl SecretStore for MemorySecrets {
    fn get(&self, target: &str) -> Option<String> {
        self.0.get(target).cloned()
    }

    fn set(&mut self, target: &str, value: Option<&str>) -> Result<(), String> {
        match value {
            Some(v) => self.0.insert(target.into(), v.into()),
            None => self.0.remove(target),
        };
        Ok(())
    }

    fn remove_all(&mut self, module: &str) -> Result<(), String> {
        if !whole_module(module) {
            return Err("not a module id".into());
        }
        let p = prefix(module);
        self.0.retain(|k, _| !k.starts_with(&p));
        Ok(())
    }
}

/// Windows Credential Manager, current user (generic credentials, persisted per user).
#[cfg(windows)]
pub struct CredentialSecrets;

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
impl SecretStore for CredentialSecrets {
    fn get(&self, target: &str) -> Option<String> {
        use windows_sys::Win32::Security::Credentials::{
            CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
        };
        let name = wide(target);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated and outlives the call; `cred` is freed below.
        let ok = unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
        if ok == 0 || cred.is_null() {
            return None;
        }
        // SAFETY: CredReadW succeeded, so `cred` points at a credential with a blob of
        // `CredentialBlobSize` bytes until CredFree.
        let value = unsafe {
            let c = &*cred;
            let blob = if c.CredentialBlob.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
            };
            let v = String::from_utf8(blob.to_vec()).ok();
            CredFree(cred as *const _);
            v
        };
        value
    }

    fn set(&mut self, target: &str, value: Option<&str>) -> Result<(), String> {
        use windows_sys::Win32::Security::Credentials::{
            CredDeleteW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
        };
        let mut name = wide(target);
        let Some(v) = value else {
            // SAFETY: `name` is NUL-terminated and outlives the call. A missing one is fine.
            unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
            return Ok(());
        };
        let mut blob = v.as_bytes().to_vec();
        let mut user = wide("Hideout");
        // SAFETY: all-zero is a valid CREDENTIALW (null pointers, zero sizes).
        let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
        cred.Type = CRED_TYPE_GENERIC;
        cred.TargetName = name.as_mut_ptr();
        cred.CredentialBlobSize = blob.len() as u32;
        cred.CredentialBlob = blob.as_mut_ptr();
        // Per user, on this PC only (not roamed).
        cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
        cred.UserName = user.as_mut_ptr();
        // SAFETY: every pointer in `cred` points into a buffer alive for the call.
        let ok = unsafe { CredWriteW(&cred, 0) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    fn remove_all(&mut self, module: &str) -> Result<(), String> {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
        use windows_sys::Win32::Security::Credentials::{
            CredDeleteW, CredEnumerateW, CredFree, CREDENTIALW, CRED_TYPE_GENERIC,
        };
        if !whole_module(module) {
            return Err("not a module id".into());
        }
        let p = prefix(module);
        let filter = wide(&format!("{p}*"));
        let mut count = 0u32;
        let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `filter` is NUL-terminated and outlives the call; `list` is freed below.
        if unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut list) } == 0 {
            // SAFETY: read right after the failed call on this thread.
            return if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().to_string())
            };
        }
        let mut failed = None;
        // SAFETY: CredEnumerateW succeeded, so `list` holds `count` valid credential pointers
        // until CredFree; each TargetName is a NUL-terminated string inside that block.
        unsafe {
            for c in std::slice::from_raw_parts(list, count as usize) {
                let c = &**c;
                if c.Type != CRED_TYPE_GENERIC || c.TargetName.is_null() {
                    continue;
                }
                let len = (0..).take_while(|&i| *c.TargetName.add(i) != 0).count();
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(c.TargetName, len));
                // Only this module's own names, checked again here rather than trusting the
                // filter alone.
                if !name.starts_with(&p) {
                    continue;
                }
                if CredDeleteW(c.TargetName, CRED_TYPE_GENERIC, 0) == 0
                    && GetLastError() != ERROR_NOT_FOUND
                {
                    failed = Some(std::io::Error::last_os_error().to_string());
                }
            }
            CredFree(list as *const _);
        }
        failed.map_or(Ok(()), Err)
    }
}

/// The store a new router uses: Credential Manager on Windows.
pub fn default_store() -> Box<dyn SecretStore> {
    #[cfg(windows)]
    {
        Box::new(CredentialSecrets)
    }
    #[cfg(not(windows))]
    {
        Box::new(MemorySecrets::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(names: &[&str]) -> MemorySecrets {
        MemorySecrets(
            names
                .iter()
                .map(|n| (n.to_string(), "v".to_string()))
                .collect(),
        )
    }

    fn left(s: &MemorySecrets) -> Vec<String> {
        let mut v: Vec<String> = s.0.keys().cloned().collect();
        v.sort();
        v
    }

    #[test]
    fn remove_all_takes_every_name_of_one_module_and_no_other() {
        let mut s = store(&[
            "Hideout/a.b/token",
            "Hideout/a.b/old name",
            "Hideout/a.bc/token",
            "Hideout/a/token",
            "Other/a.b/token",
        ]);
        s.remove_all("a.b").unwrap();
        assert_eq!(
            left(&s),
            ["Hideout/a.bc/token", "Hideout/a/token", "Other/a.b/token"]
        );
    }

    #[test]
    fn remove_all_refuses_an_id_that_would_match_more_than_one_module() {
        for bad in ["", "*", "a*", "a/b", "a\\b", "a\0"] {
            let mut s = store(&["Hideout/a/token", "Hideout/ab/token"]);
            assert!(s.remove_all(bad).is_err(), "{bad:?}");
            assert_eq!(s.0.len(), 2, "{bad:?}");
            assert!(!whole_module(bad));
        }
    }
}
