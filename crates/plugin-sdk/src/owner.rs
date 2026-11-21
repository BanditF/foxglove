//! Who serves a pipe name (contract §8.6, one-owner rule, #1143 P4).
//!
//! `foxglove.exe` and `hideout.exe` can both want `\\.\pipe\foxglove-<user>`. Before either
//! claims it, [`probe`] opens the name as a client for a moment and asks the kernel who the
//! server is (`GetNamedPipeServerProcessId`). Nothing is sent and no reply is read: a server
//! can say anything, but it cannot choose its own process id. The process is then checked:
//!
//! - it runs as the same user (token SID equal to ours),
//! - its image file name is the one expected (`foxglove.exe`, `hideout.exe`),
//! - it runs at Medium integrity or above and is no AppContainer (`hideout_acl::peer`: a
//!   Low-integrity or contained process of the same user can be named `foxglove.exe` too), and
//! - its image is a place this product trusts (H5.3), compared on canonical paths (junctions,
//!   symlinks and 8.3 names resolved; a network (UNC) path never qualifies): (a) the folder of
//!   the probing exe (one `target` folder in a dev tree, kept), (b) the other product's
//!   installed exe path from an installed-path record ([`PeerPaths::installed_exe`], the
//!   Windows Installer's own answer, never a path read from a file the peer controls), or
//!   (c) an adopted `run[0]` ([`PeerPaths::run0`]). (b) and (c) match the exact file, not its
//!   folder. Chosen over a signer check: a compare needs no certificate chain and no special
//!   case for unsigned dev builds; a user-writable install folder is the same trust root as
//!   the probing exe itself. A signature check can be added where the product is signed.
//!   With no records ([`NoRecords`], the default) only (a) applies, as before.
//!
//! Every check runs on one process handle opened once, so the pid cannot be reused between
//! them; the only window left is between the kernel's answer and that `OpenProcess` (a server
//! that exits and whose pid is taken by a same-user, same-name, same-folder, Medium process in
//! those microseconds), and the process is re-checked as still running at the end. A server
//! that vanishes during the probe is "nobody there" ([`Probe::Free`]), not busy.
//! Log lines carry the image file name only, never the full path or a pid.
//!
//! A server that fails either check is [`Probe::Foreign`]: ignored, never trusted, never
//! treated as "the other program". The probe never blocks longer than [`TIMEOUT`] (plus the
//! kernel's own immediate open). Off Windows there are no pipes: every probe is [`Probe::Free`].

use std::path::PathBuf;
use std::time::Duration;

/// The longest a probe may wait (plan §5 P4).
pub const TIMEOUT: Duration = Duration::from_millis(500);

/// What a probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Nobody serves the name: the caller may claim it.
    Free,
    /// A server whose process passed both identity checks.
    Verified { pid: u32, image: PathBuf },
    /// A server is there but is not who it must be (or its identity could not be read).
    Foreign { pid: Option<u32>, why: String },
}

/// What a verified server's image file name must be (compared without case).
#[derive(Debug, Clone, Copy)]
pub struct Expect {
    pub exe: &'static str,
}

impl Expect {
    pub const FOXGLOVE: Expect = Expect {
        exe: "foxglove.exe",
    };
    pub const HIDEOUT: Expect = Expect { exe: "hideout.exe" };
}

/// Where a peer's image may legitimately live besides the probing exe's own folder (H5.3).
/// Injected so tests are mock-only; the real records come with H5.4 (Windows Installer
/// lookup, adopted `run[0]`). Returning `None`/empty narrows trust, never widens it.
pub trait PeerPaths {
    /// The installed exe path of the product whose image file name is `exe` (for example
    /// `...\Programs\Foxglove\foxglove.exe`), from an installer record.
    fn installed_exe(&self, exe: &str) -> Option<PathBuf>;
    /// The adopted `run[0]` paths for the module whose image file name is `exe`.
    fn run0(&self, exe: &str) -> Vec<PathBuf>;
    /// The canonical path of `p` (links and 8.3 names resolved); `None` when it cannot be
    /// resolved (a missing file fails closed).
    fn canonical(&self, p: &std::path::Path) -> Option<PathBuf> {
        std::fs::canonicalize(p).ok()
    }
}

/// No records: only the probing exe's own folder is trusted.
pub struct NoRecords;

impl PeerPaths for NoRecords {
    fn installed_exe(&self, _exe: &str) -> Option<PathBuf> {
        None
    }
    fn run0(&self, _exe: &str) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// A canonical path as a comparable key: verbatim prefix removed, lower case, local drive-letter
/// form only. `None` for a UNC path (`\\server\share`, `\\?\UNC\...`), a device path or any
/// other shape: those never qualify.
fn path_key(p: &std::path::Path) -> Option<String> {
    let s = p.to_str()?;
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    let b = s.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        Some(s.to_ascii_lowercase())
    } else {
        None
    }
}

/// Canonical key of `p` through `paths`; `None` when unresolvable or not a local drive path.
fn canon_key(paths: &dyn PeerPaths, p: &std::path::Path) -> Option<String> {
    path_key(&paths.canonical(p)?)
}

/// Opens `pipe` as a client for at most `timeout` and verifies the server's process.
pub fn probe(pipe: &str, expect: Expect, timeout: Duration) -> Probe {
    probe_with(pipe, expect, timeout, &NoRecords)
}

/// [`probe`] with installed-path records for the cross-folder rules.
pub fn probe_with(pipe: &str, expect: Expect, timeout: Duration, paths: &dyn PeerPaths) -> Probe {
    #[cfg(windows)]
    {
        win::probe(pipe, expect, timeout, paths)
    }
    #[cfg(not(windows))]
    {
        let _ = (pipe, expect, timeout, paths);
        Probe::Free
    }
}

/// Opens `pipe` as a client and returns that same open handle once its server passed the
/// identity checks (#1184). Use it for a client that is about to SEND something: the checks
/// ran on the very connection the request goes over, so there is no gap in which another
/// server could take the name between "who is it" and "send". `Err(Probe::Free)`: nobody
/// serves the name (nothing was sent); `Err(Probe::Foreign)`: a server that is not who it must
/// be (nothing was sent, the handle is closed). `Err(Probe::Verified)` never happens. Off
/// Windows there are no pipes: always `Err(Probe::Free)`.
pub fn open_verified(
    pipe: &str,
    expect: Expect,
    timeout: Duration,
) -> Result<std::fs::File, Probe> {
    open_verified_with(pipe, expect, timeout, &NoRecords)
}

/// [`open_verified`] with installed-path records for the cross-folder rules.
pub fn open_verified_with(
    pipe: &str,
    expect: Expect,
    timeout: Duration,
    paths: &dyn PeerPaths,
) -> Result<std::fs::File, Probe> {
    #[cfg(windows)]
    {
        win::connect(pipe, expect, timeout, paths).map(|(file, _, _)| file)
    }
    #[cfg(not(windows))]
    {
        let _ = (pipe, expect, timeout, paths);
        Err(Probe::Free)
    }
}

/// Whether process `pid` (a pipe's CLIENT, as the server sees it) passes the same identity checks
/// a server does: image name, same user, Medium integrity or above, no AppContainer, and a
/// trusted path. `Free`: the process is gone. Nothing is sent. Off Windows: `Free`.
pub fn verify_pid(pid: u32, expect: Expect, paths: &dyn PeerPaths) -> Probe {
    #[cfg(windows)]
    {
        match win::verify_pid(pid, expect, paths) {
            Ok(image) => Probe::Verified { pid, image },
            Err(p) => p,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, expect, paths);
        Probe::Free
    }
}

/// The full image path of process `pid`, or `None` when it is not running (or not readable).
/// Used to tell a hub still alive from one that is gone, and to compare a new server's exact
/// path with a recorded one (#1143 P5).
pub fn image_of(pid: u32) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        win::image_of(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// A process held by the handle opened when it was first seen, so its image can still be asked
/// after its pid was given to another process (S1: the hub names an outside tool this way).
/// Off Windows nothing can be opened.
pub struct PeerProcess {
    #[cfg(windows)]
    inner: win::PeerProcess,
}

impl PeerProcess {
    /// Opens `pid` now; `None` when it is gone or cannot be read.
    pub fn open(pid: u32) -> Option<PeerProcess> {
        #[cfg(windows)]
        {
            win::PeerProcess::open(pid).map(|inner| PeerProcess { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = pid;
            None
        }
    }

    /// The full image path of the held process; `None` once it has exited.
    pub fn image(&self) -> Option<PathBuf> {
        #[cfg(windows)]
        {
            self.inner.image()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Whether the held process still runs.
    pub fn alive(&self) -> bool {
        #[cfg(windows)]
        {
            self.inner.alive()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// The process's token facts (integrity, AppContainer), read from this same handle, so a
    /// check on the token and the image name one process.
    #[cfg(windows)]
    pub fn peer(&self) -> std::io::Result<hideout_acl::Peer> {
        self.inner.peer()
    }
}

/// The file-name rule, apart from Windows so it is testable anywhere.
pub fn image_matches(image: &std::path::Path, expect: Expect) -> bool {
    image
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(expect.exe))
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::io;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
        ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED, HANDLE,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Pipes::{GetNamedPipeServerProcessId, WaitNamedPipeW};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn denied(why: impl Into<String>) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, why.into())
    }

    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    pub fn probe(pipe: &str, expect: Expect, timeout: Duration, paths: &dyn PeerPaths) -> Probe {
        // `file` closes at the end; nothing was sent.
        match connect(pipe, expect, timeout, paths) {
            Ok((_file, pid, image)) => Probe::Verified { pid, image },
            Err(p) => p,
        }
    }

    /// Opens the pipe and verifies its server, keeping the handle.
    pub fn connect(
        pipe: &str,
        expect: Expect,
        timeout: Duration,
        paths: &dyn PeerPaths,
    ) -> Result<(std::fs::File, u32, PathBuf), Probe> {
        let file = match open(pipe, timeout) {
            Ok(f) => f,
            Err(e) => match e.raw_os_error().map(|c| c as u32) {
                Some(ERROR_FILE_NOT_FOUND) | Some(ERROR_PATH_NOT_FOUND) => return Err(Probe::Free),
                // Busy beyond the timeout: something serves it and we could not look.
                Some(ERROR_PIPE_BUSY) => {
                    return Err(Probe::Foreign {
                        pid: None,
                        why: "every instance busy past the probe timeout".into(),
                    })
                }
                // Access denied etc.: a server we may not even open is not ours to trust.
                _ => {
                    return Err(Probe::Foreign {
                        pid: None,
                        why: format!("could not be opened: {e}"),
                    })
                }
            },
        };
        let mut pid = 0u32;
        // SAFETY: an open pipe handle owned by `file`.
        if unsafe { GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut pid) } == 0 {
            let e = io::Error::last_os_error();
            return Err(match e.raw_os_error().map(|c| c as u32) {
                // The server hung up under us: nobody is there any more.
                Some(ERROR_PIPE_NOT_CONNECTED) | Some(ERROR_BROKEN_PIPE) => Probe::Free,
                _ => Probe::Foreign {
                    pid: None,
                    why: format!("server process unknown: {e}"),
                },
            });
        }
        match verify(pid, expect, paths) {
            Ok(image) => Ok((file, pid, image)),
            Err(Refusal::Gone) => Err(Probe::Free),
            Err(Refusal::Foreign(why)) => Err(Probe::Foreign {
                pid: Some(pid),
                why,
            }),
        }
    }

    /// CreateFile never waits on a pipe, but fails at once with PIPE_BUSY when every instance
    /// is connected: wait (bounded) for one, then try once more.
    fn open(pipe: &str, timeout: Duration) -> io::Result<std::fs::File> {
        let try_open = || {
            use std::os::windows::fs::OpenOptionsExt;
            let mut o = std::fs::OpenOptions::new();
            o.read(true).write(true);
            // Same rights and QOS as the module client: no append, identification only.
            o.access_mode(0x0012_019B);
            o.security_qos_flags(0x0001_0000);
            o.open(pipe)
        };
        match try_open() {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                let wide: Vec<u16> = pipe.encode_utf16().chain(Some(0)).collect();
                let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
                // SAFETY: NUL-terminated name.
                if unsafe { WaitNamedPipeW(wide.as_ptr(), ms) } == 0 {
                    let e = io::Error::last_os_error();
                    // The server went away while we waited: nobody is there. Only a
                    // timeout (or anything else) is "still busy".
                    return Err(match e.raw_os_error().map(|c| c as u32) {
                        Some(ERROR_FILE_NOT_FOUND) | Some(ERROR_PATH_NOT_FOUND) => e,
                        _ => io::Error::from_raw_os_error(ERROR_PIPE_BUSY as i32),
                    });
                }
                try_open()
            }
            other => other,
        }
    }

    pub fn verify_pid(pid: u32, expect: Expect, paths: &dyn PeerPaths) -> Result<PathBuf, Probe> {
        match verify(pid, expect, paths) {
            Ok(image) => Ok(image),
            Err(Refusal::Gone) => Err(Probe::Free),
            Err(Refusal::Foreign(why)) => Err(Probe::Foreign {
                pid: Some(pid),
                why,
            }),
        }
    }

    pub fn image_of(pid: u32) -> Option<PathBuf> {
        PeerProcess::open(pid)?.image()
    }

    /// A process handle held from the moment it was opened: a later pid reuse cannot change
    /// what it names.
    pub struct PeerProcess(Owned);

    // SAFETY: a process handle may be used and closed from any thread.
    unsafe impl Send for PeerProcess {}

    impl PeerProcess {
        pub fn open(pid: u32) -> Option<PeerProcess> {
            // SAFETY: plain Win32 call; the handle is checked and closed by `Owned`.
            let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            (!proc.is_null()).then(|| PeerProcess(Owned(proc)))
        }

        /// Whether the process is still running (an exited one whose handle is held is not).
        pub fn alive(&self) -> bool {
            let mut code = 0u32;
            // SAFETY: a valid process handle and a valid out pointer.
            let ok = unsafe {
                windows_sys::Win32::System::Threading::GetExitCodeProcess(self.0 .0, &mut code)
            };
            ok != 0 && code == 259
        }

        /// The process's token facts (integrity, AppContainer), read from this same handle.
        pub fn peer(&self) -> std::io::Result<hideout_acl::Peer> {
            // SAFETY: `self.0 .0` is a valid process handle opened with query rights.
            unsafe { hideout_acl::peer_of_process(self.0 .0) }
        }

        pub fn image(&self) -> Option<PathBuf> {
            if !self.alive() {
                return None;
            }
            let mut buf = vec![0u16; 1024];
            let mut len = buf.len() as u32;
            // SAFETY: a valid handle; `buf` is `len` wide characters.
            if unsafe { QueryFullProcessImageNameW(self.0 .0, 0, buf.as_mut_ptr(), &mut len) } == 0
            {
                return None;
            }
            Some(PathBuf::from(std::ffi::OsString::from_wide(
                &buf[..len as usize],
            )))
        }
    }

    enum Refusal {
        /// The server's process no longer exists.
        Gone,
        Foreign(String),
    }

    fn foreign(why: impl Into<String>) -> Refusal {
        Refusal::Foreign(why.into())
    }

    fn file_name(p: &std::path::Path) -> String {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "?".into())
    }

    /// The identity rules on facts already read (apart from the OS, so synthetic peers can
    /// test them): file name, same user, no AppContainer, Medium integrity or above, and a
    /// trusted place (module docs: canonical compare, UNC refused, fails closed). Messages
    /// carry the file name only.
    pub(super) fn judge(
        image: &std::path::Path,
        expect: Expect,
        our_dir: Option<&std::path::Path>,
        same_user: bool,
        peer: &hideout_acl::Peer,
        paths: &dyn PeerPaths,
    ) -> Result<(), String> {
        if !image_matches(image, expect) {
            return Err(format!("runs {}, not {}", file_name(image), expect.exe));
        }
        if !same_user {
            return Err(format!("{} runs as another user", expect.exe));
        }
        if peer.app_container.is_some() {
            return Err(format!("{} runs in an AppContainer", expect.exe));
        }
        if peer.integrity < hideout_acl::MEDIUM_RID {
            return Err(format!("{} runs below Medium integrity", expect.exe));
        }
        let Some(img) = canon_key(paths, image) else {
            return Err(format!(
                "{} is not on a local path this program can resolve",
                expect.exe
            ));
        };
        // (a) the probing exe's own folder.
        let in_our_folder = our_dir
            .and_then(|d| canon_key(paths, d))
            .zip(std::path::Path::new(&img).parent().and_then(path_key))
            .is_some_and(|(ours, theirs)| ours == theirs);
        // (b) the other product's installed exe, (c) an adopted run[0]: the exact file.
        let is_record = |p: PathBuf| canon_key(paths, &p).is_some_and(|k| k == img);
        let trusted = in_our_folder
            || paths.installed_exe(expect.exe).is_some_and(is_record)
            || paths.run0(expect.exe).into_iter().any(is_record);
        if !trusted {
            return Err(format!(
                "{} is not this product's install folder",
                expect.exe
            ));
        }
        Ok(())
    }

    fn verify(pid: u32, expect: Expect, paths: &dyn PeerPaths) -> Result<PathBuf, Refusal> {
        // One handle for every check below: while it is open the pid cannot be reused.
        // SAFETY: plain Win32 calls; handles are closed by `Owned`.
        let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if proc.is_null() {
            let code = unsafe { GetLastError() };
            // ERROR_INVALID_PARAMETER: no such process (the server exited).
            return Err(if code == 87 {
                Refusal::Gone
            } else {
                foreign(format!(
                    "process not readable ({})",
                    io::Error::from_raw_os_error(code as i32)
                ))
            });
        }
        let proc = Owned(proc);
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        if unsafe { QueryFullProcessImageNameW(proc.0, 0, buf.as_mut_ptr(), &mut len) } == 0 {
            return Err(foreign("image path not readable"));
        }
        let image = PathBuf::from(std::ffi::OsString::from_wide(&buf[..len as usize]));
        let theirs = user_sid(proc.0).map_err(|e| foreign(e.to_string()))?;
        let ours = user_sid(unsafe { GetCurrentProcess() }).map_err(|e| foreign(e.to_string()))?;
        let same_user = unsafe { EqualSid(theirs.sid, ours.sid) } != 0;
        // SAFETY: `proc` is a valid open process handle.
        let peer = unsafe { hideout_acl::peer_of_process(proc.0) }
            .map_err(|e| foreign(format!("token unreadable ({e})")))?;
        let our_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        judge(&image, expect, our_dir.as_deref(), same_user, &peer, paths)
            .map_err(Refusal::Foreign)?;
        // Still running? A server that exited meanwhile is gone, not verified.
        let mut code = 0u32;
        if unsafe { windows_sys::Win32::System::Threading::GetExitCodeProcess(proc.0, &mut code) }
            == 0
            || code != 259
        {
            return Err(Refusal::Gone);
        }
        Ok(image)
    }

    /// A token user SID; `sid` points into `_buf`, which lives as long as this value.
    struct Sid {
        _buf: Vec<u64>,
        sid: *mut core::ffi::c_void,
    }

    fn user_sid(process: HANDLE) -> io::Result<Sid> {
        let mut token: HANDLE = std::ptr::null_mut();
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(denied("process token not readable"));
        }
        let token = Owned(token);
        let mut need = 0u32;
        unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut need) };
        if need == 0 {
            return Err(denied("token user size unknown"));
        }
        // u64 elements keep the buffer aligned for TOKEN_USER.
        let mut buf = vec![0u64; (need as usize).div_ceil(8)];
        if unsafe {
            GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr() as _, need, &mut need)
        } == 0
        {
            return Err(denied("token user not readable"));
        }
        let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
        Ok(Sid { _buf: buf, sid })
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::channel::ServerPipe;

    fn test_name(tag: &str) -> String {
        format!(r"\\.\pipe\foxglove-test-owner-{tag}-{}", std::process::id())
    }

    /// The test binary's own file name: what a "correct identity" server looks like here.
    fn own_exe() -> Expect {
        let name = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        Expect {
            exe: Box::leak(name.into_boxed_str()),
        }
    }

    /// Keeps a real server on `name` until it is connected once.
    fn serve(name: &str) -> std::thread::JoinHandle<()> {
        let first = ServerPipe::create(name, true).expect("first instance");
        std::thread::spawn(move || {
            let _ = first.accept();
        })
    }

    #[test]
    fn nobody_serving_is_free_and_fast() {
        let t = std::time::Instant::now();
        let p = probe(&test_name("none"), own_exe(), TIMEOUT);
        assert_eq!(p, Probe::Free);
        assert!(t.elapsed() < TIMEOUT);
    }

    #[test]
    fn a_real_server_with_the_right_identity_is_verified() {
        let name = test_name("ok");
        let _h = serve(&name);
        match probe(&name, own_exe(), TIMEOUT) {
            Probe::Verified { pid, .. } => assert_eq!(pid, std::process::id()),
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn a_server_with_the_wrong_identity_is_foreign() {
        let name = test_name("fake");
        let _h = serve(&name);
        // Same process, same user, but not "foxglove.exe": an impostor by name.
        match probe(&name, Expect::FOXGLOVE, TIMEOUT) {
            Probe::Foreign { pid, why } => {
                assert_eq!(pid, Some(std::process::id()));
                assert!(why.contains("not foxglove.exe"), "{why}");
                // File name only: no directory, no pid.
                assert!(!why.contains('\\'), "{why}");
                assert!(!why.contains(&std::process::id().to_string()), "{why}");
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn open_verified_keeps_the_handle_only_for_the_right_identity() {
        let name = test_name("openv-ok");
        let _h = serve(&name);
        assert!(open_verified(&name, own_exe(), TIMEOUT).is_ok());
        let name = test_name("openv-bad");
        let _h = serve(&name);
        match open_verified(&name, Expect::HIDEOUT, TIMEOUT) {
            Err(Probe::Foreign { .. }) => {}
            other => panic!("expected Foreign, got {other:?}"),
        }
        assert!(matches!(
            open_verified(&test_name("openv-none"), own_exe(), TIMEOUT),
            Err(Probe::Free)
        ));
    }

    #[test]
    fn a_busy_pipe_gives_up_within_the_limit() {
        let name = test_name("busy");
        // One instance, already connected: the next open sees PIPE_BUSY.
        let first = ServerPipe::create(&name, true).unwrap();
        let _client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        let _conn = first.accept();
        let t = std::time::Instant::now();
        let p = probe(&name, own_exe(), Duration::from_millis(200));
        assert!(matches!(p, Probe::Foreign { pid: None, .. }), "{p:?}");
        assert!(
            t.elapsed() < TIMEOUT + Duration::from_millis(300),
            "{:?}",
            t.elapsed()
        );
    }

    fn medium() -> hideout_acl::Peer {
        hideout_acl::Peer {
            integrity: hideout_acl::MEDIUM_RID,
            app_container: None,
        }
    }

    /// Mock records: a link table stands in for the file system (junctions, 8.3 names);
    /// a path listed in `missing` cannot be resolved.
    #[derive(Default)]
    struct Mock {
        installed: Vec<(&'static str, &'static str)>,
        run0: Vec<(&'static str, &'static str)>,
        links: Vec<(&'static str, &'static str)>,
        missing: Vec<&'static str>,
    }

    impl PeerPaths for Mock {
        fn installed_exe(&self, exe: &str) -> Option<PathBuf> {
            self.installed
                .iter()
                .find(|(e, _)| *e == exe)
                .map(|(_, p)| PathBuf::from(p))
        }
        fn run0(&self, exe: &str) -> Vec<PathBuf> {
            self.run0
                .iter()
                .filter(|(e, _)| *e == exe)
                .map(|(_, p)| PathBuf::from(p))
                .collect()
        }
        fn canonical(&self, p: &std::path::Path) -> Option<PathBuf> {
            let s = p.to_str()?;
            if self.missing.iter().any(|m| m.eq_ignore_ascii_case(s)) {
                return None;
            }
            // Longest link prefix wins, like a junction in the middle of a path.
            let hit = self
                .links
                .iter()
                .filter(|(from, _)| {
                    s.len() >= from.len() && s[..from.len()].eq_ignore_ascii_case(from)
                })
                .max_by_key(|(from, _)| from.len());
            Some(match hit {
                Some((from, to)) => PathBuf::from(format!("{to}{}", &s[from.len()..])),
                None => p.to_path_buf(),
            })
        }
    }

    const FOX: &str = r"C:\Users\u\AppData\Local\Programs\Foxglove\foxglove.exe";
    const HUB_DIR: &str = r"C:\Users\u\AppData\Local\Programs\Hideout";

    fn judge_with(
        image: &str,
        our_dir: &str,
        peer: &hideout_acl::Peer,
        user: bool,
        paths: &Mock,
    ) -> Result<(), String> {
        win::judge(
            std::path::Path::new(image),
            Expect::FOXGLOVE,
            Some(std::path::Path::new(our_dir)),
            user,
            peer,
            paths,
        )
    }

    fn installed() -> Mock {
        Mock {
            installed: vec![("foxglove.exe", FOX)],
            ..Mock::default()
        }
    }

    #[test]
    fn low_integrity_appcontainer_other_user_and_other_folder_are_refused() {
        let m = Mock::default();
        let d = r"C:\Apps\Fox";
        let img = r"C:\Apps\Fox\foxglove.exe";
        assert_eq!(judge_with(img, r"c:\apps\fox", &medium(), true, &m), Ok(()));
        let low = hideout_acl::Peer {
            integrity: 0x1000,
            ..medium()
        };
        assert!(judge_with(img, d, &low, true, &m)
            .unwrap_err()
            .contains("Medium"));
        let high = hideout_acl::Peer {
            integrity: 0x3000,
            ..medium()
        };
        assert_eq!(judge_with(img, d, &high, true, &m), Ok(()));
        let contained = hideout_acl::Peer {
            app_container: Some("S-1-15-2-1-2".into()),
            ..medium()
        };
        assert!(judge_with(img, d, &contained, true, &m)
            .unwrap_err()
            .contains("AppContainer"));
        assert!(judge_with(img, d, &medium(), false, &m)
            .unwrap_err()
            .contains("another user"));
        let e = judge_with(img, r"C:\Elsewhere", &medium(), true, &m).unwrap_err();
        assert!(
            e.contains("not this product's install folder") && !e.contains("Apps"),
            "{e}"
        );
    }

    #[test]
    fn a_hub_in_one_product_folder_verifies_the_other_products_installed_exe() {
        let m = installed();
        assert_eq!(judge_with(FOX, HUB_DIR, &medium(), true, &m), Ok(()));
        // Case differences are not a difference.
        assert_eq!(
            judge_with(&FOX.to_uppercase(), HUB_DIR, &medium(), true, &m),
            Ok(())
        );
        // The same file name anywhere else is a stranger.
        let stranger = r"C:\Users\u\Downloads\foxglove.exe";
        let e = judge_with(stranger, HUB_DIR, &medium(), true, &m).unwrap_err();
        assert!(e.contains("not this product's install folder"), "{e}");
        // A different file in the installed folder is not the record (exact file, not folder).
        let sibling = Mock {
            installed: vec![(
                "foxglove.exe",
                r"C:\Users\u\AppData\Local\Programs\Foxglove\other.exe",
            )],
            ..Mock::default()
        };
        assert!(judge_with(FOX, HUB_DIR, &medium(), true, &sibling).is_err());
        // No record at all: trust does not widen.
        assert!(judge_with(FOX, HUB_DIR, &medium(), true, &Mock::default()).is_err());
        // A record for the other exe name never vouches for this one.
        let other = Mock {
            installed: vec![("hideout.exe", FOX)],
            ..Mock::default()
        };
        assert!(judge_with(FOX, HUB_DIR, &medium(), true, &other).is_err());
    }

    #[test]
    fn an_adopted_run0_is_trusted_for_that_exact_file_only() {
        let m = Mock {
            run0: vec![("foxglove.exe", r"D:\Tools\Fox\foxglove.exe")],
            ..Mock::default()
        };
        assert_eq!(
            judge_with(r"D:\Tools\Fox\foxglove.exe", HUB_DIR, &medium(), true, &m),
            Ok(())
        );
        assert!(judge_with(r"D:\Tools\Fox2\foxglove.exe", HUB_DIR, &medium(), true, &m).is_err());
    }

    #[test]
    fn a_junction_or_short_name_is_resolved_before_the_compare() {
        // A junction pointing into the installed folder is the installed file.
        let m = Mock {
            links: vec![(r"C:\Temp\J", r"C:\Users\u\AppData\Local\Programs\Foxglove")],
            installed: vec![("foxglove.exe", FOX)],
            ..Mock::default()
        };
        assert_eq!(
            judge_with(r"C:\Temp\J\foxglove.exe", HUB_DIR, &medium(), true, &m),
            Ok(())
        );
        // An 8.3 short name resolves to the same long path as the record.
        let short = Mock {
            links: vec![(r"C:\PROGRA~1", r"C:\Program Files")],
            installed: vec![("foxglove.exe", r"C:\Program Files\Fox\foxglove.exe")],
            ..Mock::default()
        };
        assert_eq!(
            judge_with(
                r"C:\PROGRA~1\Fox\foxglove.exe",
                HUB_DIR,
                &medium(),
                true,
                &short
            ),
            Ok(())
        );
        // A junction that leads somewhere else is judged by where it leads.
        let out = Mock {
            links: vec![(r"C:\Temp\J", r"C:\Evil")],
            installed: vec![("foxglove.exe", FOX)],
            ..Mock::default()
        };
        assert!(judge_with(r"C:\Temp\J\foxglove.exe", HUB_DIR, &medium(), true, &out).is_err());
        // The dev-tree rule resolves links too: a junction to our own folder is our folder.
        let dev = Mock {
            links: vec![(r"C:\Link", r"C:\Dev\target")],
            ..Mock::default()
        };
        assert_eq!(
            judge_with(
                r"C:\Link\foxglove.exe",
                r"C:\Dev\target",
                &medium(),
                true,
                &dev
            ),
            Ok(())
        );
    }

    #[test]
    fn unc_and_unresolvable_paths_fail_closed() {
        // A network path never qualifies, even when it is the very record or our folder.
        let unc = r"\\server\share\Foxglove\foxglove.exe";
        let m = Mock {
            installed: vec![("foxglove.exe", unc)],
            run0: vec![("foxglove.exe", unc)],
            ..Mock::default()
        };
        assert!(judge_with(unc, r"\\server\share\Foxglove", &medium(), true, &m).is_err());
        let verbatim = r"\\?\UNC\server\share\Foxglove\foxglove.exe";
        assert!(judge_with(verbatim, HUB_DIR, &medium(), true, &m).is_err());
        // A junction that resolves to a network share is refused.
        let via_link = Mock {
            links: vec![(r"C:\Share", r"\\?\UNC\server\share")],
            installed: vec![("foxglove.exe", r"C:\Share\foxglove.exe")],
            ..Mock::default()
        };
        assert!(judge_with(
            r"C:\Share\foxglove.exe",
            HUB_DIR,
            &medium(),
            true,
            &via_link
        )
        .is_err());
        // Device and relative paths are refused.
        assert!(judge_with(r"\\.\C:\foxglove.exe", HUB_DIR, &medium(), true, &m).is_err());
        assert!(judge_with(r"foxglove.exe", HUB_DIR, &medium(), true, &m).is_err());
        // Cannot resolve the image (deleted, access denied): fails closed.
        let gone = Mock {
            missing: vec![FOX],
            ..installed()
        };
        assert!(judge_with(FOX, HUB_DIR, &medium(), true, &gone).is_err());
        // Cannot resolve the record: no trust from it.
        let gone_record = Mock {
            missing: vec![FOX],
            installed: vec![("foxglove.exe", FOX)],
            ..Mock::default()
        };
        assert!(judge_with(
            r"C:\Other\foxglove.exe",
            HUB_DIR,
            &medium(),
            true,
            &gone_record
        )
        .is_err());
    }

    #[test]
    fn identity_checks_still_come_before_any_path_rule() {
        let m = installed();
        let low = hideout_acl::Peer {
            integrity: 0x1000,
            ..medium()
        };
        assert!(judge_with(FOX, HUB_DIR, &low, true, &m).is_err());
        assert!(judge_with(FOX, HUB_DIR, &medium(), false, &m).is_err());
        // Wrong file name in the right folder.
        let e = win::judge(
            std::path::Path::new(r"C:\Users\u\AppData\Local\Programs\Foxglove\evil.exe"),
            Expect::FOXGLOVE,
            Some(std::path::Path::new(HUB_DIR)),
            true,
            &medium(),
            &m,
        )
        .unwrap_err();
        assert!(e.contains("not foxglove.exe"), "{e}");
    }

    #[test]
    fn path_keys_accept_only_local_drive_paths() {
        use std::path::Path;
        assert_eq!(
            path_key(Path::new(r"\\?\C:\A\b.exe")).as_deref(),
            Some(r"c:\a\b.exe")
        );
        assert_eq!(path_key(Path::new(r"\\?\UNC\s\x")), None);
        assert_eq!(path_key(Path::new(r"\\s\x")), None);
        assert_eq!(path_key(Path::new(r"C:rel")), None);
    }

    #[test]
    fn a_server_that_vanishes_during_the_wait_is_free_not_busy() {
        let name = test_name("vanish");
        let first = ServerPipe::create(&name, true).unwrap();
        let client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        let conn = first.accept();
        let n2 = name.clone();
        // Busy now; the whole server (and so the name) goes away while the probe waits.
        let t = std::thread::spawn(move || probe(&n2, own_exe(), Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(200));
        drop(conn);
        drop(client);
        let p = t.join().unwrap();
        assert_eq!(p, Probe::Free, "{p:?}");
    }

    #[test]
    fn the_name_rule_ignores_case_and_directories() {
        use std::path::Path;
        assert!(image_matches(
            Path::new(r"C:\x\FOXGLOVE.EXE"),
            Expect::FOXGLOVE
        ));
        assert!(!image_matches(
            Path::new(r"C:\x\foxglove.exe.evil"),
            Expect::FOXGLOVE
        ));
        assert!(!image_matches(
            Path::new(r"C:\x\hideout.exe"),
            Expect::FOXGLOVE
        ));
    }
}
