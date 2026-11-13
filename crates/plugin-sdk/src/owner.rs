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
//! - its image is in the same folder as the probing exe. Chosen over a signer check: both
//!   programs ship in one install folder (and one `target` folder in a dev tree), and a
//!   folder compare needs no certificate chain and no special case for unsigned dev builds;
//!   a user-writable install folder is the same trust root as the probing exe itself. A
//!   signature check can be added where the product is signed.
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

/// Opens `pipe` as a client for at most `timeout` and verifies the server's process.
pub fn probe(pipe: &str, expect: Expect, timeout: Duration) -> Probe {
    #[cfg(windows)]
    {
        win::probe(pipe, expect, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (pipe, expect, timeout);
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

    pub fn probe(pipe: &str, expect: Expect, timeout: Duration) -> Probe {
        let file = match open(pipe, timeout) {
            Ok(f) => f,
            Err(e) => match e.raw_os_error().map(|c| c as u32) {
                Some(ERROR_FILE_NOT_FOUND) | Some(ERROR_PATH_NOT_FOUND) => return Probe::Free,
                // Busy beyond the timeout: something serves it and we could not look.
                Some(ERROR_PIPE_BUSY) => {
                    return Probe::Foreign {
                        pid: None,
                        why: "every instance busy past the probe timeout".into(),
                    }
                }
                // Access denied etc.: a server we may not even open is not ours to trust.
                _ => {
                    return Probe::Foreign {
                        pid: None,
                        why: format!("could not be opened: {e}"),
                    }
                }
            },
        };
        let mut pid = 0u32;
        // SAFETY: an open pipe handle owned by `file`.
        if unsafe { GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut pid) } == 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error().map(|c| c as u32) {
                // The server hung up under us: nobody is there any more.
                Some(ERROR_PIPE_NOT_CONNECTED) | Some(ERROR_BROKEN_PIPE) => Probe::Free,
                _ => Probe::Foreign {
                    pid: None,
                    why: format!("server process unknown: {e}"),
                },
            };
        }
        // `file` closes at the end; nothing was sent.
        match verify(pid, expect) {
            Ok(image) => Probe::Verified { pid, image },
            Err(Refusal::Gone) => Probe::Free,
            Err(Refusal::Foreign(why)) => Probe::Foreign {
                pid: Some(pid),
                why,
            },
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

    pub fn image_of(pid: u32) -> Option<PathBuf> {
        // SAFETY: plain Win32 calls; the handle is closed by `Owned`.
        let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if proc.is_null() {
            return None;
        }
        let proc = Owned(proc);
        let mut code = 0u32;
        // A process that exited but whose handle is still open elsewhere is not "alive".
        if unsafe { windows_sys::Win32::System::Threading::GetExitCodeProcess(proc.0, &mut code) }
            == 0
            || code != 259
        {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        if unsafe { QueryFullProcessImageNameW(proc.0, 0, buf.as_mut_ptr(), &mut len) } == 0 {
            return None;
        }
        Some(PathBuf::from(std::ffi::OsString::from_wide(
            &buf[..len as usize],
        )))
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
    /// test them): file name, same user, no AppContainer, Medium integrity or above, same
    /// folder as `our_dir`. Messages carry the file name only.
    pub(super) fn judge(
        image: &std::path::Path,
        expect: Expect,
        our_dir: Option<&std::path::Path>,
        same_user: bool,
        peer: &hideout_acl::Peer,
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
        let same_folder = match (image.parent(), our_dir) {
            (Some(a), Some(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
            _ => false,
        };
        if !same_folder {
            return Err(format!(
                "{} is not in this program's install folder",
                expect.exe
            ));
        }
        Ok(())
    }

    fn verify(pid: u32, expect: Expect) -> Result<PathBuf, Refusal> {
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
        judge(&image, expect, our_dir.as_deref(), same_user, &peer).map_err(Refusal::Foreign)?;
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

    fn judged(peer: &hideout_acl::Peer, user: bool, dir: &str) -> Result<(), String> {
        win::judge(
            std::path::Path::new(r"C:\Apps\Fox\foxglove.exe"),
            Expect::FOXGLOVE,
            Some(std::path::Path::new(dir)),
            user,
            peer,
        )
    }

    #[test]
    fn low_integrity_appcontainer_other_user_and_other_folder_are_refused() {
        assert_eq!(judged(&medium(), true, r"c:\apps\fox"), Ok(()));
        let low = hideout_acl::Peer {
            integrity: 0x1000,
            ..medium()
        };
        assert!(judged(&low, true, r"C:\Apps\Fox")
            .unwrap_err()
            .contains("Medium"));
        let high = hideout_acl::Peer {
            integrity: 0x3000,
            ..medium()
        };
        assert_eq!(judged(&high, true, r"C:\Apps\Fox"), Ok(()));
        let contained = hideout_acl::Peer {
            app_container: Some("S-1-15-2-1-2".into()),
            ..medium()
        };
        assert!(judged(&contained, true, r"C:\Apps\Fox")
            .unwrap_err()
            .contains("AppContainer"));
        assert!(judged(&medium(), false, r"C:\Apps\Fox")
            .unwrap_err()
            .contains("another user"));
        let e = judged(&medium(), true, r"C:\Elsewhere").unwrap_err();
        assert!(e.contains("install folder") && !e.contains("Apps"), "{e}");
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
