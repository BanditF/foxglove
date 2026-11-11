// SPDX-License-Identifier: MIT
//! AppContainer isolation for module processes (contract §7.7), with the public Win32 API:
//! `CreateAppContainerProfile` per module, its capability SIDs from the manifest
//! ([`crate::contain::plan`]), and `SECURITY_CAPABILITIES` in the process attributes. The
//! pattern is Foxglove's plugin host's (`crates/plugin-host/src/sandbox.rs`, #215), for a
//! module that talks over the router pipe instead of stdio.
//!
//! - The container may read and run its version folder, and write its `settings\` and
//!   `data\` (granted to its SID, inherited inside). Nothing else of the user's.
//! - The router pipe admits its SID ([`hideout_router::server::Hub::allow_pipe_client`]).
//! - The process inherits only NUL for stdin and stdout and its log for stderr; it starts
//!   suspended, goes into its Job Object, and only then runs ([`Spawned::resume`]).

use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    GetLastError, LocalFree, SetHandleInformation, ERROR_SUCCESS, HANDLE, HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
    EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID,
    TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    DeriveCapabilitySidsFromName, FreeSid, GetTokenInformation, TokenAppContainerSid,
    TokenCapabilities, TokenIsAppContainer, ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    TOKEN_APPCONTAINER_INFORMATION, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_QUERY,
};
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    OpenProcessToken, ResumeThread, TerminateProcess, UpdateProcThreadAttribute, CREATE_NO_WINDOW,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use crate::win::{Handle, Job, Process};

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_EXECUTE: u32 = 0x2000_0000;
const DELETE: u32 = 0x0001_0000;
/// `SE_GROUP_ENABLED`: a capability the token holds.
const SE_GROUP_ENABLED: u32 = 0x4;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The longest profile name the API takes.
const PROFILE_MAX: usize = 64;

/// The profile name for module `id`: `hideout.<id>` when the id keeps the id grammar (§4.3)
/// and that fits in 64 characters; else `hideout.<first 38>._<16 hex of the id's SHA-256>`. No
/// id label starts with `_`, so a plain name never has `._` and never equals a hashed one: two
/// ids never share a profile (and its SID), a crafted one included (#1089 review).
pub fn profile_name(id: &str) -> String {
    let plain = format!("hideout.{id}");
    if hideout_contract::module::id_grammar_ok(id) && plain.len() <= PROFILE_MAX {
        return plain;
    }
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(id.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let head: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || ".-_".contains(*c))
        .take(38)
        .collect();
    format!("hideout.{head}._{hash}")
}

/// The SID of module `id`'s profile, if Windows derives one (it does for any name, made or
/// not), for taking it off the router pipe when the module is removed.
pub fn profile_sid(id: &str) -> Option<String> {
    let name = wide(&profile_name(id));
    let mut sid: PSID = std::ptr::null_mut();
    // SAFETY: a NUL-terminated name; the SID is freed once below.
    unsafe {
        if DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) < 0 {
            return None;
        }
        let s = sid_string(sid).ok();
        FreeSid(sid);
        s
    }
}

/// Removes module `id`'s profile (and its `%LOCALAPPDATA%\Packages` folder), for remove.
pub fn delete_profile(id: &str) {
    let name = wide(&profile_name(id));
    // SAFETY: a NUL-terminated name.
    unsafe { DeleteAppContainerProfile(name.as_ptr()) };
}

unsafe fn sid_string(sid: PSID) -> io::Result<String> {
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

/// The SID of the AppContainer capability `name` (`humaninterfacedevice`, `internetClient`).
fn capability_sid(name: &str) -> io::Result<PSID> {
    let w = wide(name);
    let (mut groups, mut ngroups): (*mut PSID, u32) = (std::ptr::null_mut(), 0);
    let (mut sids, mut nsids): (*mut PSID, u32) = (std::ptr::null_mut(), 0);
    // SAFETY: out-pointers valid; the arrays and each SID are LocalAlloc'd and freed below.
    unsafe {
        if DeriveCapabilitySidsFromName(
            w.as_ptr(),
            &mut groups,
            &mut ngroups,
            &mut sids,
            &mut nsids,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        for i in 0..ngroups as usize {
            LocalFree(*groups.add(i));
        }
        LocalFree(groups.cast());
        let first = (nsids > 0).then(|| *sids);
        for i in 1..nsids as usize {
            LocalFree(*sids.add(i));
        }
        LocalFree(sids.cast());
        first.ok_or_else(|| io::Error::other(format!("no SID for the capability {name}")))
    }
}

/// The SID string of the capability `name`, as a token lists it.
pub fn capability_sid_string(name: &str) -> io::Result<String> {
    let sid = capability_sid(name)?;
    // SAFETY: a SID from DeriveCapabilitySidsFromName, freed once.
    unsafe {
        let s = sid_string(sid);
        LocalFree(sid);
        s
    }
}

/// A module's AppContainer: its SID and its capabilities' SIDs.
pub struct Container {
    sid: PSID,
    caps: Vec<PSID>,
    /// `S-1-15-2-…`, for the router pipe's ACL and the log.
    pub sid_string: String,
}

// SAFETY: the SIDs are owned, read-only buffers.
unsafe impl Send for Container {}
unsafe impl Sync for Container {}

impl Drop for Container {
    fn drop(&mut self) {
        // SAFETY: each SID freed once, with the call that matches its allocation.
        unsafe {
            FreeSid(self.sid);
            for c in &self.caps {
                LocalFree(*c);
            }
        }
    }
}

impl Container {
    /// Module `id`'s profile (made on first use, looked up after) with `capabilities`.
    pub fn for_module(id: &str, capabilities: &[&str]) -> io::Result<Container> {
        let name = wide(&profile_name(id));
        let display = wide("Hideout module");
        let mut sid: PSID = std::ptr::null_mut();
        // SAFETY: NUL-terminated strings; no capabilities on the profile itself.
        let hr = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                display.as_ptr(),
                display.as_ptr(),
                std::ptr::null(),
                0,
                &mut sid,
            )
        };
        // HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS): made by an earlier start, so look it up.
        // Any other failure is a real one (access denied, no profile service) and is said.
        const ALREADY_EXISTS: i32 = 0x8007_00B7_u32 as i32;
        if hr == ALREADY_EXISTS {
            // SAFETY: as above.
            let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            if hr < 0 {
                return Err(io::Error::other(format!(
                    "AppContainer profile lookup: HRESULT {hr:#x}"
                )));
            }
        } else if hr < 0 {
            return Err(io::Error::other(format!(
                "AppContainer profile: HRESULT {hr:#x}"
            )));
        }
        // SAFETY: a valid SID; freed here when it can't be read, else by `Container`'s drop.
        let sid_string = match unsafe { sid_string(sid) } {
            Ok(s) => s,
            Err(e) => {
                unsafe { FreeSid(sid) };
                return Err(e);
            }
        };
        let mut c = Container {
            sid,
            caps: Vec::new(),
            sid_string,
        };
        for name in capabilities {
            c.caps.push(capability_sid(name)?);
        }
        Ok(c)
    }

    /// Lets the container read and run what is in `dir` (`write`: also change and delete it),
    /// inherited by everything inside.
    pub fn grant(&self, dir: &Path, write: bool) -> io::Result<()> {
        let rights = if write {
            GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | DELETE
        } else {
            GENERIC_READ | GENERIC_EXECUTE
        };
        let path = wide_path(dir);
        // SAFETY: the DACL read is merged with one ACE and written back; buffers freed once.
        unsafe {
            let mut old: *mut ACL = std::ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let e = GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut old,
                std::ptr::null_mut(),
                &mut sd,
            );
            if e != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(e as i32));
            }
            let access = EXPLICIT_ACCESS_W {
                grfAccessPermissions: rights,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                    ptstrName: self.sid as *mut u16,
                },
            };
            let mut new: *mut ACL = std::ptr::null_mut();
            let e = SetEntriesInAclW(1, &access, old, &mut new);
            LocalFree(sd);
            if e != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(e as i32));
            }
            let e = SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new,
                std::ptr::null(),
            );
            LocalFree(new as _);
            if e != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(e as i32));
            }
        }
        Ok(())
    }

    /// Starts `exe args` from `cwd` in this container and in `job`, suspended. `env` is added
    /// to (and overrides) the hub's environment. `stderr` is the module's log (NUL without).
    pub fn spawn(
        &self,
        job: &Job,
        exe: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(&str, String)],
        stderr: Option<File>,
    ) -> io::Result<Spawned> {
        // Declared first, so it is released last: no other spawn of the supervisor (plain or
        // stdio, `Job::spawn_with`) runs while these three handles are inheritable, and they are
        // closed before it is released (#1089 review). The handle list gives this child exactly
        // these three.
        let _spawning = crate::win::SPAWN.lock().unwrap_or_else(|e| e.into_inner());
        let null_in = File::open("NUL")?;
        let null_out = std::fs::OpenOptions::new().write(true).open("NUL")?;
        let err = match stderr {
            Some(f) => f,
            None => std::fs::OpenOptions::new().write(true).open("NUL")?,
        };
        let inherit: [HANDLE; 3] = [
            null_in.as_raw_handle() as HANDLE,
            null_out.as_raw_handle() as HANDLE,
            err.as_raw_handle() as HANDLE,
        ];
        for h in inherit {
            // SAFETY: open handles owned by the Files above.
            if unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let mut cmdline = wide(&command_line(exe, args));
        let mut block = env_block(env);
        let cwd = wide_path(cwd);
        let mut caps: Vec<SID_AND_ATTRIBUTES> = self
            .caps
            .iter()
            .map(|c| SID_AND_ATTRIBUTES {
                Sid: *c,
                Attributes: SE_GROUP_ENABLED,
            })
            .collect();
        let security = SECURITY_CAPABILITIES {
            AppContainerSid: self.sid,
            Capabilities: if caps.is_empty() {
                std::ptr::null_mut()
            } else {
                caps.as_mut_ptr()
            },
            CapabilityCount: caps.len() as u32,
            Reserved: 0,
        };
        // SAFETY: the attribute list lives in `buf` until it is deleted below; every pointer
        // it holds (`security`, `inherit`) outlives CreateProcessW.
        unsafe {
            let mut size = 0usize;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut size);
            let mut buf = vec![0u64; size.div_ceil(8)];
            let attrs = buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            if InitializeProcThreadAttributeList(attrs, 2, 0, &mut size) == 0 {
                return Err(io::Error::last_os_error());
            }
            let ok = UpdateProcThreadAttribute(
                attrs,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &security as *const _ as *const _,
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) != 0
                && UpdateProcThreadAttribute(
                    attrs,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    inherit.as_ptr() as *const _,
                    std::mem::size_of_val(&inherit),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                ) != 0;
            if !ok {
                let e = io::Error::last_os_error();
                DeleteProcThreadAttributeList(attrs);
                return Err(e);
            }
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = inherit[0];
            si.StartupInfo.hStdOutput = inherit[1];
            si.StartupInfo.hStdError = inherit[2];
            si.lpAttributeList = attrs;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let created = CreateProcessW(
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | CREATE_UNICODE_ENVIRONMENT,
                block.as_mut_ptr().cast(),
                cwd.as_ptr(),
                &si.StartupInfo,
                &mut pi,
            );
            let err = GetLastError();
            DeleteProcThreadAttributeList(attrs);
            if created == 0 {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            let thread = Handle(pi.hThread);
            let process = Process::from_raw(pi.hProcess);
            // Into the job before its first instruction (no breakaway, its caps).
            if AssignProcessToJobObject(job.raw(), pi.hProcess) == 0 {
                let e = io::Error::last_os_error();
                TerminateProcess(pi.hProcess, 1);
                return Err(e);
            }
            Ok(Spawned {
                process,
                pid: pi.dwProcessId,
                thread,
            })
        }
    }
}

/// A process started suspended in its container and job.
pub struct Spawned {
    pub process: Process,
    pub pid: u32,
    thread: Handle,
}

impl Spawned {
    /// Lets it run.
    pub fn resume(self) -> io::Result<(Process, u32)> {
        // SAFETY: the primary thread's handle from CreateProcessW.
        if unsafe { ResumeThread(self.thread.0) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok((self.process, self.pid))
    }
}

/// One argument quoted as `CommandLineToArgvW` reads it back.
fn quote(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
                continue;
            }
            _ => {}
        }
        if c != '\\' {
            out.extend(std::iter::repeat_n('\\', slashes));
            slashes = 0;
            out.push(c);
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
}

fn command_line(exe: &Path, args: &[String]) -> String {
    let mut s = String::new();
    quote(&exe.to_string_lossy(), &mut s);
    for a in args {
        s.push(' ');
        quote(a, &mut s);
    }
    s
}

/// The hub's environment with `add` on top, as a sorted UTF-16 block.
fn env_block(add: &[(&str, String)]) -> Vec<u16> {
    let mut vars: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .filter(|(k, _)| !add.iter().any(|(a, _)| a.eq_ignore_ascii_case(k)))
        .collect();
    vars.extend(add.iter().map(|(k, v)| (k.to_string(), v.clone())));
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

fn token_info(p: &Process, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
    // SAFETY: a process handle with query rights; the buffer is sized by the first call.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(p.raw(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);
        let mut len = 0u32;
        GetTokenInformation(token.0, class, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        if GetTokenInformation(token.0, class, buf.as_mut_ptr().cast(), len, &mut len) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buf)
    }
}

/// Whether `p` runs in an AppContainer (its token's `TokenIsAppContainer`).
pub fn is_app_container(p: &Process) -> io::Result<bool> {
    Ok(token_info(p, TokenIsAppContainer)?[0] as u32 != 0)
}

/// The AppContainer SID `p` runs in, `None` outside one (a process taken back after a hub
/// restart keeps its container, and the new router pipe must let it in again).
pub fn container_sid(p: &Process) -> io::Result<Option<String>> {
    if !is_app_container(p)? {
        return Ok(None);
    }
    let buf = token_info(p, TokenAppContainerSid)?;
    // SAFETY: the buffer holds a TOKEN_APPCONTAINER_INFORMATION whose SID points inside it.
    unsafe {
        let info = &*(buf.as_ptr() as *const TOKEN_APPCONTAINER_INFORMATION);
        sid_string(info.TokenAppContainer).map(Some)
    }
}

/// The capability SIDs in `p`'s token.
pub fn capabilities(p: &Process) -> io::Result<Vec<String>> {
    let buf = token_info(p, TokenCapabilities)?;
    // SAFETY: the buffer holds a TOKEN_GROUPS of GroupCount entries.
    unsafe {
        let g = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        let first = g.Groups.as_ptr();
        (0..g.GroupCount as usize)
            .map(|i| sid_string((*first.add(i)).Sid))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_command_lines() {
        assert_eq!(
            profile_name("foxglove.example-art"),
            "hideout.foxglove.example-art"
        );
        // Long ids that share their first 56 characters get two profiles (#1089 review).
        let a = format!("{}.a", "x".repeat(60));
        let b = format!("{}.b", "x".repeat(60));
        let (pa, pb) = (profile_name(&a), profile_name(&b));
        assert_ne!(pa, pb);
        assert!(pa.len() <= 64 && pb.len() <= 64, "{pa} {pb}");
        assert_eq!(pa, profile_name(&a), "the same id, the same profile");
        assert_eq!(profile_name("a.b"), "hideout.a.b");
        // A crafted 56-character id spelling a victim's hashed name in full is a plain name
        // that can't equal it: hashed names have `._`, which no id can (#1089 re-check).
        let victim = format!("v.{}", "y".repeat(70));
        let hashed = profile_name(&victim);
        assert_eq!(hashed.len(), 64);
        assert!(hashed.contains("._"), "{hashed}");
        let crafted = hashed.strip_prefix("hideout.").unwrap();
        assert!(
            !hideout_contract::module::id_grammar_ok(crafted),
            "{crafted}"
        );
        assert_ne!(profile_name(crafted), hashed);
        // And the old form's spelling (no underscore) is a valid id, plain, and differs.
        let old_form = crafted.replace("._", ".");
        assert!(hideout_contract::module::id_grammar_ok(&old_form));
        assert_eq!(profile_name(&old_form), format!("hideout.{old_form}"));
        assert_ne!(profile_name(&old_form), hashed);
        let c = command_line(
            Path::new(r"C:\a b\m.exe"),
            &["/c".into(), "say \"hi\"".into(), r"C:\x\".into(), "".into()],
        );
        assert_eq!(c, r#""C:\a b\m.exe" /c "say \"hi\"" C:\x\ """#);
    }

    #[test]
    fn the_environment_block_overrides_and_ends_twice() {
        let b = env_block(&[("HIDEOUT_PIPE", r"\\.\pipe\x".into())]);
        let s = String::from_utf16_lossy(&b);
        assert!(s.ends_with("\0\0"));
        assert_eq!(s.matches("HIDEOUT_PIPE=").count(), 1);
    }
}
