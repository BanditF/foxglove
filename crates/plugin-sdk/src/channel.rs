// SPDX-License-Identifier: MIT
//! Both ends of a direct channel (docs/hideout-contract.md §8.4; the wire rules are in
//! `foxglove_plugin_api::hideout::channel`).
//!
//! On `channel.create {channel, consumer, nonce}` the provider makes a **one-time named pipe with
//! a current-user ACL** and answers `{pipe}`; the hub hands `{pipe, nonce}` to the consumer and
//! steps out. Frames never pass through the hub.
//!
//! - [`ChannelPipe::create`] makes the pipe: one instance only, the first-instance flag (a taken
//!   name fails instead of being shared), remote clients rejected, and a DACL that grants the
//!   current user's SID and nobody else. One client ever: a second connect is refused.
//! - [`ChannelPipe::accept_nonce`] waits for the consumer and checks that its first frame is the
//!   nonce, both within [`ACCEPT_TIMEOUT`]; a wrong or late nonce spends the channel.
//! - [`connect`] is the consumer's end: it connects only to a local `hideout-ch-` pipe and sends
//!   the nonce first.
//! - Then frames flow both ways ([`read_frame`], [`write_frame`]: `u32` little-endian length +
//!   payload, at most [`MAX_FRAME`]). Either side closing ends the channel. A [`ChannelConn`]
//!   clones: one thread reads while another writes.
//!
//! ```no_run
//! use foxglove_plugin::channel::{unique_pipe_name, ChannelPipe, ACCEPT_TIMEOUT};
//! let name = unique_pipe_name("example.art", "frames");
//! let pipe = ChannelPipe::create(&name).unwrap();
//! // answer channel.create with {"pipe": name}, then, on a thread of its own:
//! let mut conn = pipe.accept_nonce("the-nonce", ACCEPT_TIMEOUT).unwrap();
//! ```

use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{IntoRawHandle, RawHandle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use foxglove_plugin_api::hideout::channel as wire;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, SECURITY_IDENTIFICATION,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::PIPE_UNLIMITED_INSTANCES;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

pub use wire::{is_channel_pipe, MAX_NONCE};

/// The most a channel frame may carry. A longer length prefix is a protocol error.
pub const MAX_FRAME: usize = wire::MAX_CHANNEL_FRAME;

/// How long a provider waits for its consumer to connect and send the nonce (§8.4, v1.4).
pub const ACCEPT_TIMEOUT: Duration = Duration::from_secs(wire::ACCEPT_TIMEOUT_S);

const ERROR_BROKEN_PIPE: i32 = 109;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A pipe name for one channel instance: `\\.\pipe\hideout-ch-<pid>-<n>-<module>-<channel>`,
/// unique per process and call; characters other than ASCII letters, digits, `-` and `_` in the
/// ids become `-`, and the name is cut to the 256 characters a pipe name may have.
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
    let mut name = format!(
        "{}{}-{n}-{}-{}",
        wire::PIPE_PREFIX,
        std::process::id(),
        clean(module),
        clean(channel)
    );
    name.truncate(wire::MAX_PIPE_NAME);
    name
}

pub use hideout_acl::current_user_sid;
use hideout_acl::{wide, Descriptor};

/// A one-time channel pipe, created and waiting for its consumer.
pub struct ChannelPipe {
    handle: Owned,
    name: String,
}

impl ChannelPipe {
    /// Creates the pipe `name` (use [`unique_pipe_name`]) with a current-user-only ACL. It has
    /// one instance, so it takes no second client. Fails if the name is already taken.
    pub fn create(name: &str) -> io::Result<ChannelPipe> {
        let sd = Descriptor::current_user_only()?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.as_ptr(),
            bInheritHandle: 0,
        };
        let wname = wide(name);
        let h = unsafe {
            CreateNamedPipeW(
                wname.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
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
            handle: Owned(h),
            name: name.to_string(),
        })
    }

    /// The name to answer `channel.create` with.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pipe's handle, for inspecting it (the test reads its DACL).
    pub fn raw_handle(&self) -> RawHandle {
        self.handle.0 as RawHandle
    }

    /// Blocks until the consumer connects, with no time limit. Prefer [`Self::accept_nonce`].
    pub fn accept(self) -> io::Result<ChannelConn> {
        self.accept_until(None)
    }

    /// Waits up to `timeout` for the consumer to connect and send `nonce` as its first frame.
    /// Late is `TimedOut`, a wrong nonce `PermissionDenied`; either way the pipe is closed and
    /// the channel is spent.
    pub fn accept_nonce(self, nonce: &str, timeout: Duration) -> io::Result<ChannelConn> {
        let deadline = Instant::now() + timeout;
        let mut conn = self.accept_until(Some(deadline))?;
        conn.deadline = Some(deadline);
        let checked = conn.read_nonce(nonce);
        conn.deadline = None;
        checked.map(|()| conn)
    }

    fn accept_until(self, deadline: Option<Instant>) -> io::Result<ChannelConn> {
        let h = self.handle.0;
        overlapped(h, deadline, |ov| unsafe { ConnectNamedPipe(h, ov) })?;
        Ok(ChannelConn {
            pipe: PipeConn(std::sync::Arc::new(self.handle)),
            deadline: None,
        })
    }
}

/// The provider's whole `channel.create` (§8.4) in one call: makes a one-time pipe for
/// `module`'s `channel`, and on a thread of its own waits up to [`ACCEPT_TIMEOUT`] for the
/// consumer with `nonce`, then hands the connection to `stream`. Returns the pipe name to answer
/// with (`{"pipe": name}`). A late or wrong consumer only closes the pipe (logged).
pub fn serve(
    module: &str,
    channel: &str,
    nonce: &str,
    stream: impl FnOnce(ChannelConn) + Send + 'static,
) -> io::Result<String> {
    let pipe = ChannelPipe::create(&unique_pipe_name(module, channel))?;
    let name = pipe.name().to_string();
    let nonce = nonce.to_string();
    let label = channel.to_string();
    std::thread::Builder::new()
        .name(format!("hideout-channel-{channel}"))
        .spawn(move || match pipe.accept_nonce(&nonce, ACCEPT_TIMEOUT) {
            Ok(conn) => stream(conn),
            Err(e) => eprintln!("hideout channel {label}: not opened: {e}"),
        })?;
    Ok(name)
}

/// The consumer's end (§8.4): connects to the `pipe` the hub answered `channel.open` with and
/// sends `nonce` as the first frame. Only a local `\\.\pipe\hideout-ch-` pipe is connected to
/// (`InvalidInput` otherwise), with identification-level impersonation only. The pipe takes one
/// client ever, so a second connect fails.
pub fn connect(pipe: &str, nonce: &str) -> io::Result<ChannelConn> {
    if !is_channel_pipe(pipe) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a local hideout-ch- channel pipe",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(pipe)?;
    let mut conn = ChannelConn {
        pipe: PipeConn(std::sync::Arc::new(Owned(file.into_raw_handle() as HANDLE))),
        deadline: None,
    };
    write_frame(&mut conn, nonce.as_bytes())?;
    Ok(conn)
}

/// A connected channel, either end: `Read` and `Write` on the pipe. Clones share it, so one
/// thread reads while another writes; the last one dropped closes the channel.
#[derive(Clone)]
pub struct ChannelConn {
    pipe: PipeConn,
    /// While the nonce is read: when to give up.
    deadline: Option<Instant>,
}

impl ChannelConn {
    /// Ends a read or write blocked on this channel in another thread (`CancelIoEx`): that
    /// call returns an error. For a consumer that stops reading (the hub's panel host closing a
    /// hidden panel's stream): set its own stop flag first, then cancel.
    pub fn cancel(&self) {
        unsafe { CancelIoEx((self.pipe.0).0, std::ptr::null()) };
    }

    /// Reads the consumer's first frame and checks it is `nonce` (at most [`MAX_NONCE`] bytes,
    /// compared in constant time). A wrong nonce is `PermissionDenied`; drop the connection then.
    pub fn read_nonce(&mut self, nonce: &str) -> io::Result<()> {
        let denied = |e: io::Error| match e.kind() {
            io::ErrorKind::UnexpectedEof | io::ErrorKind::PermissionDenied => io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the channel's first frame is not the nonce",
            ),
            _ => e,
        };
        let mut len = [0u8; 4];
        self.read_exact(&mut len).map_err(denied)?;
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_NONCE {
            return Err(denied(io::ErrorKind::PermissionDenied.into()));
        }
        let mut got = vec![0u8; len];
        self.read_exact(&mut got).map_err(denied)?;
        if wire::nonce_matches(&got, nonce) {
            Ok(())
        } else {
            Err(denied(io::ErrorKind::PermissionDenied.into()))
        }
    }
}

impl Read for ChannelConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.pipe.read_until(buf, self.deadline)
    }
}

impl Write for ChannelConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pipe.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
    w.write_all(&wire::encode_frame(payload))?;
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

/// An owned pipe handle, closed on drop.
struct Owned(HANDLE);

// SAFETY: a pipe handle may be used from any thread; overlapped I/O keeps reads and writes apart.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// One overlapped operation on `h`, waited for in the kernel (no polling). With a `deadline`,
/// the wait ends there: the operation is cancelled and the answer is `TimedOut`.
fn overlapped(
    h: HANDLE,
    deadline: Option<Instant>,
    op: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> io::Result<u32> {
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    let event = Owned(event);
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = event.0;
    if op(&mut ov) == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_PIPE_CONNECTED {
            return Ok(0);
        }
        if code != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
    }
    let mut n = 0u32;
    if let Some(d) = deadline {
        let ms = d
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u128::from(u32::MAX - 1)) as u32;
        if unsafe { WaitForSingleObject(event.0, ms) } == WAIT_TIMEOUT {
            unsafe { CancelIoEx(h, &ov) };
            // The OVERLAPPED must outlive the I/O: wait for the cancel to land.
            if unsafe { GetOverlappedResult(h, &ov, &mut n, 1) } != 0 {
                return Ok(n); // it finished just before the cancel
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the channel's consumer did not come in time",
            ));
        }
    }
    if unsafe { GetOverlappedResult(h, &ov, &mut n, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n)
}

/// One instance of a many-client server pipe (the hub's router pipe, §8.2): a current-user-only
/// DACL, remote clients rejected, overlapped I/O so one connection reads and writes at the same
/// time. The first instance takes the name with the first-instance flag, so a squatter makes it
/// fail instead of serving beside it.
pub struct ServerPipe {
    handle: Owned,
}

impl ServerPipe {
    /// Creates an instance of `name`; `first` for the first one a server makes.
    pub fn create(name: &str, first: bool) -> io::Result<ServerPipe> {
        Self::make(name, first, &Descriptor::current_user_only()?, 0)
    }

    /// The first instance of `name`, opened with `WRITE_DAC` and `WRITE_OWNER` so the server
    /// can change the pipe's DACL and label later (its instances share one descriptor):
    /// [`hideout_acl::apply_to_handle`] on [`ServerPipe::raw_handle`] lets AppContainer
    /// clients in (contract §7.7). Windows refuses those rights on any later instance, so only
    /// the first gets them; `guests` may open it from the start.
    pub fn create_keeper(name: &str, guests: &[String]) -> io::Result<ServerPipe> {
        const WRITE_DAC: u32 = 0x0004_0000;
        const WRITE_OWNER: u32 = 0x0008_0000;
        let sd = Descriptor::current_user_and(guests)?;
        Self::make(name, true, &sd, WRITE_DAC | WRITE_OWNER)
    }

    fn make(name: &str, first: bool, sd: &Descriptor, access: u32) -> io::Result<ServerPipe> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.as_ptr(),
            bInheritHandle: 0,
        };
        let wname = wide(name);
        let first = if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
        let h = unsafe {
            CreateNamedPipeW(
                wname.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | first | access,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &sa,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(ServerPipe { handle: Owned(h) })
    }

    /// The instance's handle, for inspecting it (the test reads its DACL).
    pub fn raw_handle(&self) -> RawHandle {
        self.handle.0 as RawHandle
    }

    /// Blocks until a client connects to this instance.
    pub fn accept(self) -> io::Result<PipeConn> {
        let h = self.handle.0;
        overlapped(h, None, |ov| unsafe { ConnectNamedPipe(h, ov) })?;
        Ok(PipeConn(std::sync::Arc::new(self.handle)))
    }
}

/// A connected server pipe. Clones share the connection: one thread reads while another writes.
/// The last clone dropped closes it.
#[derive(Clone)]
pub struct PipeConn(std::sync::Arc<Owned>);

impl PipeConn {
    fn read_until(&self, buf: &mut [u8], deadline: Option<Instant>) -> io::Result<usize> {
        let h = self.0 .0;
        let len = buf.len().min(u32::MAX as usize) as u32;
        let ptr = buf.as_mut_ptr();
        match overlapped(h, deadline, |ov| unsafe {
            ReadFile(h, ptr, len, std::ptr::null_mut(), ov)
        }) {
            Ok(n) => Ok(n as usize),
            // The client closed its end: end of stream.
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE) => Ok(0),
            Err(e) => Err(e),
        }
    }
}

impl Read for PipeConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_until(buf, None)
    }
}

impl Write for PipeConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let h = self.0 .0;
        let len = buf.len().min(u32::MAX as usize) as u32;
        let ptr = buf.as_ptr();
        overlapped(h, None, |ov| unsafe {
            WriteFile(h, ptr, len, std::ptr::null_mut(), ov)
        })
        .map(|n| n as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSidToSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetAce, ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, PSID};

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

    #[test]
    fn one_time_pipe_takes_one_consumer() {
        let name = unique_pipe_name("test.module", "once");
        let pipe = ChannelPipe::create(&name).unwrap();
        let n = name.clone();
        let provider = std::thread::spawn(move || {
            let mut conn = pipe
                .accept_nonce("n-once", Duration::from_secs(10))
                .unwrap();
            for i in 0u8..5 {
                write_frame(&mut conn, &[i; 3]).unwrap();
            }
            // Wait for the consumer to hang up.
            assert_eq!(read_frame(&mut conn).unwrap(), None);
        });
        let mut c = connect(&name, "n-once").unwrap();
        for i in 0u8..5 {
            assert_eq!(read_frame(&mut c).unwrap().unwrap(), vec![i; 3]);
        }
        // The single instance is taken: a second consumer is refused while the first holds it.
        assert!(connect(&n, "n-once").is_err(), "second connect refused");
        drop(c);
        provider.join().unwrap();
        // And after the channel ended, the name is gone.
        assert!(
            connect(&n, "n-once").is_err(),
            "connect after close refused"
        );
    }

    #[test]
    fn late_consumer_times_out() {
        let name = unique_pipe_name("test.module", "late");
        let pipe = ChannelPipe::create(&name).unwrap();
        let err = pipe
            .accept_nonce("n", Duration::from_millis(50))
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(connect(&name, "n").is_err(), "the timed-out pipe is closed");
    }

    #[test]
    fn silent_consumer_times_out() {
        let name = unique_pipe_name("test.module", "silent");
        let pipe = ChannelPipe::create(&name).unwrap();
        let n = name.clone();
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            // Connect but never send the nonce; hold the pipe until the provider gave up.
            let _c = crate::hideout::open_pipe(&n).unwrap();
            let _ = wait.recv();
        });
        let err = pipe
            .accept_nonce("n", Duration::from_millis(200))
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        done.send(()).unwrap();
        t.join().unwrap();
    }

    #[test]
    fn consumer_connects_only_to_channel_pipes() {
        for bad in [
            r"\\host\pipe\hideout-ch-x",
            r"\\.\pipe\hideout-someone",
            "C:/x",
        ] {
            let err = connect(bad, "n").err().unwrap();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
        let long = unique_pipe_name(&"m".repeat(300), "c");
        assert!(is_channel_pipe(&long), "{long}");
    }

    #[test]
    fn oversized_nonce_frame_is_denied_unread() {
        let name = unique_pipe_name("test.module", "bignonce");
        let pipe = ChannelPipe::create(&name).unwrap();
        let n = name.clone();
        let t = std::thread::spawn(move || {
            let mut c = crate::hideout::open_pipe(&n).unwrap();
            c.write_all(&(MAX_NONCE as u32 + 1).to_le_bytes()).unwrap();
            let _ = read_frame(&mut c);
        });
        let err = pipe
            .accept_nonce("n", Duration::from_secs(10))
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        t.join().unwrap();
    }
}
