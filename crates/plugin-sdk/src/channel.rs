//! The provider's end of a direct channel (docs/hideout-contract.md §8.4).
//!
//! On `channel.create {channel, consumer, nonce}` the provider makes a **one-time named pipe with
//! a current-user ACL** and answers `{pipe}`:
//!
//! - [`ChannelPipe::create`] makes the pipe: one instance only, the first-instance flag (a taken
//!   name fails instead of being shared), remote clients rejected, and a DACL that grants the
//!   current user's SID and nobody else.
//! - [`ChannelPipe::accept`] waits for the consumer; [`ChannelConn::read_nonce`] checks that the
//!   consumer's first frame is the nonce; then frames flow both ways ([`read_frame`],
//!   [`write_frame`]: `u32` little-endian length + payload). Either side closing ends the channel.
//!
//! ```no_run
//! use foxglove_plugin::channel::{unique_pipe_name, ChannelPipe};
//! let name = unique_pipe_name("example.art", "frames");
//! let pipe = ChannelPipe::create(&name).unwrap();
//! // answer channel.create with {"pipe": name}, then:
//! let mut conn = pipe.accept().unwrap();
//! conn.read_nonce("the-nonce").unwrap();
//! ```

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The most a channel frame may carry. A longer length prefix is a protocol error.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

const ERROR_BROKEN_PIPE: i32 = 109;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A pipe name for one channel instance: `\\.\pipe\hideout-ch-<pid>-<n>-<module>-<channel>`,
/// unique per process and call; characters other than ASCII letters, digits, `-` and `_` in the
/// ids become `-`.
pub fn unique_pipe_name(module: &str, channel: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        r"\\.\pipe\hideout-ch-{}-{n}-{}-{}",
        std::process::id(),
        clean(module),
        clean(channel)
    )
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
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
            let mut s: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
                Err(io::Error::last_os_error())
            } else {
                let mut n = 0;
                while *s.add(n) != 0 {
                    n += 1;
                }
                let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
                LocalFree(s.cast());
                Ok(out)
            }
        };
        CloseHandle(token);
        result
    }
}

/// A security descriptor granting full access to the current user's SID only (protected, so
/// nothing is inherited).
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn current_user_only() -> io::Result<Descriptor> {
        let sddl = wide(&format!("D:P(A;;GA;;;{})", current_user_sid()?));
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
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

/// A one-time channel pipe, created and waiting for its consumer.
pub struct ChannelPipe {
    file: File,
    name: String,
}

impl ChannelPipe {
    /// Creates the pipe `name` (use [`unique_pipe_name`]) with a current-user-only ACL. It has
    /// one instance, so it takes no second client. Fails if the name is already taken.
    pub fn create(name: &str) -> io::Result<ChannelPipe> {
        let sd = Descriptor::current_user_only()?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let wname = wide(name);
        let h = unsafe {
            CreateNamedPipeW(
                wname.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                &sa,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(ChannelPipe {
            file: unsafe { File::from_raw_handle(h as RawHandle) },
            name: name.to_string(),
        })
    }

    /// The name to answer `channel.create` with.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pipe's handle, for inspecting it (the test reads its DACL).
    pub fn raw_handle(&self) -> RawHandle {
        self.file.as_raw_handle()
    }

    /// Blocks until the consumer connects.
    pub fn accept(self) -> io::Result<ChannelConn> {
        let h = self.file.as_raw_handle() as HANDLE;
        if unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } == 0 {
            let err = io::Error::last_os_error();
            // A consumer that connected between create and accept is fine.
            if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                return Err(err);
            }
        }
        Ok(ChannelConn { file: self.file })
    }
}

/// The connected channel: `Read` and `Write` on the pipe. Dropping it closes the channel.
pub struct ChannelConn {
    file: File,
}

impl ChannelConn {
    /// Reads the consumer's first frame and checks it is `nonce`. A wrong nonce is
    /// `PermissionDenied`; drop the connection then.
    pub fn read_nonce(&mut self, nonce: &str) -> io::Result<()> {
        let got = read_frame(self)?;
        if got.as_deref() == Some(nonce.as_bytes()) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the channel's first frame is not the nonce",
            ))
        }
    }
}

impl Read for ChannelConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.file.read(buf) {
            // The consumer closed its end: end of stream.
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE) => Ok(0),
            other => other,
        }
    }
}

impl Write for ChannelConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Writes one frame: `u32` little-endian length, then the payload.
pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame too long",
        ));
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    w.write_all(&out)?;
    w.flush()
}

/// Reads one frame; `None` when the other side closed before a new frame began.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        let n = r.read(&mut len[got..])?;
        if n == 0 {
            return if got == 0 {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        got += n;
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too long"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSidToSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::PSID;
    use windows_sys::Win32::Security::{
        EqualSid, GetAce, ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION,
    };

    #[test]
    fn current_user_only_pipe_connects_and_closes() {
        let name = unique_pipe_name("test.module", "chan");
        let pipe = ChannelPipe::create(&name).unwrap();
        assert_eq!(pipe.name(), name);

        // The DACL: exactly one ACE, an allow for the current user's SID.
        unsafe {
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let rc = GetSecurityInfo(
                pipe.raw_handle() as HANDLE,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            );
            assert_eq!(rc, 0, "GetSecurityInfo");
            assert!(!dacl.is_null(), "a null DACL would grant everyone");
            assert_eq!((*dacl).AceCount, 1, "one ACE only");
            let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
            assert_ne!(GetAce(dacl, 0, &mut ace), 0);
            let ace = &*(ace as *const ACCESS_ALLOWED_ACE);
            assert_eq!(ace.Header.AceType, 0, "an allow ACE");
            let sid_str = wide(&current_user_sid().unwrap());
            let mut me: PSID = std::ptr::null_mut();
            assert_ne!(ConvertStringSidToSidW(sid_str.as_ptr(), &mut me), 0);
            let same = EqualSid(std::ptr::addr_of!(ace.SidStart) as PSID, me);
            LocalFree(me.cast());
            LocalFree(sd.cast());
            assert_ne!(same, 0, "the ACE is for the current user");
        }

        // A second pipe on the same name is refused.
        assert!(ChannelPipe::create(&name).is_err());

        // Connect, send the nonce, exchange a frame, close.
        let client_name = name.clone();
        let client = std::thread::spawn(move || {
            let mut c = crate::hideout::open_pipe(&client_name).unwrap();
            write_frame(&mut c, b"n-123").unwrap();
            write_frame(&mut c, b"hello").unwrap();
            let back = read_frame(&mut c).unwrap().unwrap();
            assert_eq!(back, b"world");
            // Dropping c closes the channel.
        });
        let mut conn = pipe.accept().unwrap();
        conn.read_nonce("n-123").unwrap();
        assert_eq!(read_frame(&mut conn).unwrap().unwrap(), b"hello");
        write_frame(&mut conn, b"world").unwrap();
        client.join().unwrap();
        assert_eq!(
            read_frame(&mut conn).unwrap(),
            None,
            "close ends the channel"
        );
    }

    #[test]
    fn wrong_nonce_is_denied() {
        let name = unique_pipe_name("test.module", "nonce");
        let pipe = ChannelPipe::create(&name).unwrap();
        let n = name.clone();
        let t = std::thread::spawn(move || {
            let mut c = crate::hideout::open_pipe(&n).unwrap();
            write_frame(&mut c, b"bad").unwrap();
            // Wait for the provider to hang up.
            let _ = read_frame(&mut c);
        });
        let mut conn = pipe.accept().unwrap();
        let err = conn.read_nonce("good").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        drop(conn);
        t.join().unwrap();
    }
}
