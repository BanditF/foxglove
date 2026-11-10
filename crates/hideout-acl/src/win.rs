// SPDX-License-Identifier: MIT
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetTokenInformation,
    TokenUser, ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const INHERITED_ACE: u8 = 0x10;
/// SYSTEM and the built-in Administrators group.
const SYSTEM_SID: &str = "S-1-5-18";
const ADMINS_SID: &str = "S-1-5-32-544";

/// UTF-16 with a trailing NUL.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The current user's SID as a string (`S-1-5-21-...`), from the process token.
pub fn current_user_sid() -> io::Result<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        // u64 backing keeps the TOKEN_USER aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
        let result = if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            sid_string(user.User.Sid)
        };
        CloseHandle(token);
        result
    }
}

unsafe fn sid_string(sid: *mut core::ffi::c_void) -> io::Result<String> {
    let mut s: *mut u16 = std::ptr::null_mut();
    if ConvertSidToStringSidW(sid, &mut s) == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
    LocalFree(s.cast());
    Ok(out)
}

/// A self-relative security descriptor, freed on drop.
pub struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn from_sddl(sddl: &str) -> io::Result<Descriptor> {
        let sddl = wide(sddl);
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Descriptor(sd))
    }

    /// Full access for the current user's SID only (protected, so nothing is inherited): pipes.
    pub fn current_user_only() -> io::Result<Descriptor> {
        Self::from_sddl(&format!("D:P(A;;GA;;;{})", current_user_sid()?))
    }

    /// Full access for the current user, SYSTEM and Administrators; protected, and inherited by
    /// files and folders created inside: the hub's config folder.
    pub fn private_tree() -> io::Result<Descriptor> {
        let me = current_user_sid()?;
        Self::from_sddl(&format!(
            "D:PAI(A;OICI;FA;;;{me})(A;OICI;FA;;;{SYSTEM_SID})(A;OICI;FA;;;{ADMINS_SID})"
        ))
    }

    /// The pointer for `SECURITY_ATTRIBUTES::lpSecurityDescriptor`.
    pub fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

/// One ACE of a DACL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ace {
    /// True for an allow ACE, false for a deny (or any other kind).
    pub allow: bool,
    /// The trustee's SID string.
    pub sid: String,
    pub inherited: bool,
}

/// A DACL as read from a file or folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dacl {
    /// True when the DACL does not inherit from the parent.
    pub protected: bool,
    pub aces: Vec<Ace>,
}

/// Reads the DACL of a file or folder. A missing (null) DACL, which grants everyone, is an error.
pub fn dacl(path: &Path) -> io::Result<Dacl> {
    let p = wide_path(path);
    unsafe {
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let rc = GetNamedSecurityInfoW(
            p.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut sd,
        );
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let out = (|| {
            if acl.is_null() {
                return Err(io::Error::other("null DACL (grants everyone)"));
            }
            let mut control = 0u16;
            let mut rev = 0u32;
            if GetSecurityDescriptorControl(sd, &mut control, &mut rev) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut aces = Vec::new();
            for i in 0..(*acl).AceCount as u32 {
                let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
                if GetAce(acl, i, &mut ace) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let ace = &*(ace as *const ACCESS_ALLOWED_ACE);
                // Allow (0) and deny (1) ACEs share this layout; others carry no plain SID.
                let kind = ace.Header.AceType;
                let sid = if kind <= 1 {
                    sid_string(std::ptr::addr_of!(ace.SidStart) as *mut _)?
                } else {
                    String::from("?")
                };
                aces.push(Ace {
                    allow: kind == 0,
                    sid,
                    inherited: ace.Header.AceFlags & INHERITED_ACE != 0,
                });
            }
            Ok(Dacl {
                protected: control & SE_DACL_PROTECTED != 0,
                aces,
            })
        })();
        LocalFree(sd.cast());
        out
    }
}

/// True when the DACL is protected and holds exactly an own allow for each of `sids`.
fn is_exactly(d: &Dacl, sids: &[&str]) -> bool {
    d.protected
        && d.aces.len() == sids.len()
        && d.aces.iter().all(|a| a.allow && !a.inherited)
        && sids.iter().all(|s| d.aces.iter().any(|a| a.sid == *s))
}

/// Makes `dir` private: a protected DACL of the current user, SYSTEM and Administrators, inherited
/// by what it holds (existing children that inherit are updated by Windows). Returns `true` when
/// the folder had to be changed, `false` when it already was private. A folder that does not
/// exist is an error.
pub fn ensure_private_dir(dir: &Path) -> io::Result<bool> {
    let me = current_user_sid()?;
    if let Ok(d) = dacl(dir) {
        if is_exactly(&d, &[&me, SYSTEM_SID, ADMINS_SID]) {
            return Ok(false);
        }
    }
    let sd = Descriptor::private_tree()?;
    let p = wide_path(dir);
    unsafe {
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl: *mut ACL = std::ptr::null_mut();
        if GetSecurityDescriptorDacl(sd.as_ptr(), &mut present, &mut acl, &mut defaulted) == 0 {
            return Err(io::Error::last_os_error());
        }
        let rc = SetNamedSecurityInfoW(
            p.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null_mut(),
        );
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
    }
    Ok(true)
}

/// Like `create_dir_all`, but a folder that did not exist is made private on creation.
/// An existing folder is left alone (the hub fixes its root once, at start, with
/// [`ensure_private_dir`]).
pub fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    ensure_private_dir(dir).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hideout-acl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn assert_private(d: &Dacl) {
        let me = current_user_sid().unwrap();
        assert!(d.protected, "no inherited ACEs from the parent");
        assert_eq!(d.aces.len(), 3, "{:?}", d.aces);
        for sid in [me.as_str(), SYSTEM_SID, ADMINS_SID] {
            assert!(
                d.aces
                    .iter()
                    .any(|a| a.sid == sid && a.allow && !a.inherited),
                "allow for {sid}: {:?}",
                d.aces
            );
        }
        // Never Users (BU), Everyone (WD) or Authenticated Users (AU).
        for sid in ["S-1-5-32-545", "S-1-1-0", "S-1-5-11"] {
            assert!(d.aces.iter().all(|a| a.sid != sid), "{sid} present");
        }
    }

    #[test]
    fn created_dir_is_private_and_children_inherit() {
        let root = temp("new");
        let dir = root.join("a").join("b");
        create_private_dir_all(&dir).unwrap();
        assert_private(&dacl(&dir).unwrap());
        // Files and folders made inside carry the same three ACEs (inherited).
        let f = dir.join("inventory.json");
        std::fs::write(&f, "{}").unwrap();
        let sub = dir.join("logs");
        std::fs::create_dir(&sub).unwrap();
        for p in [&f, &sub] {
            let d = dacl(p).unwrap();
            assert_eq!(d.aces.len(), 3, "{p:?}: {:?}", d.aces);
            assert!(d.aces.iter().all(|a| a.allow && a.inherited));
        }
        assert!(!ensure_private_dir(&dir).unwrap(), "already private");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn existing_dir_with_inherited_aces_is_fixed_once() {
        let dir = temp("old");
        // A plain create_dir_all inherits whatever the temp folder grants.
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hideout.toml");
        std::fs::write(&f, "").unwrap();
        assert!(!dacl(&dir).unwrap().protected);

        assert!(ensure_private_dir(&dir).unwrap(), "changed");
        assert_private(&dacl(&dir).unwrap());
        // The existing file picked up the new ACEs, none left from the old parent.
        let d = dacl(&f).unwrap();
        assert_eq!(d.aces.len(), 3, "{:?}", d.aces);
        assert!(
            !ensure_private_dir(&dir).unwrap(),
            "second start: nothing to do"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_dir_is_an_error() {
        assert!(ensure_private_dir(&temp("missing")).is_err());
    }
}
