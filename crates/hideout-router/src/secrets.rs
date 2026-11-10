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
}

/// The credential name for a module's secret.
pub fn target(module: &str, name: &str) -> String {
    format!("Hideout/{module}/{name}")
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
