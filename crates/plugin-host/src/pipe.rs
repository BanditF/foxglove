//! The local API's named pipe on Windows: `\\.\pipe\foxglove-<user>`.
//!
//! - Only the current user (and SYSTEM) can open it: the pipe's DACL grants the owner, nobody
//!   else, and remote clients are rejected.
//! - The first instance is created with FILE_FLAG_FIRST_PIPE_INSTANCE, so if another process
//!   already squats on the name, [`listen`] fails instead of serving next to it.
//! - Overlapped I/O, so one connection reads and writes at the same time (events go out while
//!   a read is pending). Nothing polls: a waiting listener or reader sleeps in the kernel.

use std::io::{self, BufReader, Read, Write};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateEventW;

/// The pipe's name for the current user.
pub fn name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    format!(r"\\.\pipe\foxglove-{user}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// An owner-only security descriptor: Owner Rights and SYSTEM get full access, nobody else.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn owner_only() -> io::Result<Descriptor> {
        let sddl = wide("D:P(A;;GA;;;OW)(A;;GA;;;SY)");
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut sd, std::ptr::null_mut()) };
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

fn create(name: &[u16], first: bool, sd: &Descriptor) -> io::Result<Handle> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    let flags = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            flags,
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
    Ok(Handle(h))
}

/// One overlapped operation, waited for on its own event.
fn overlapped(h: &Handle, op: impl FnOnce(*mut OVERLAPPED) -> i32) -> io::Result<u32> {
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    let event = Handle(event);
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = event.0;
    if op(&mut ov) == 0 {
        let err = unsafe { GetLastError() };
        if err != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(err as i32));
        }
    }
    let mut n = 0u32;
    if unsafe { GetOverlappedResult(h.0, &ov, &mut n, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n)
}

#[derive(Clone)]
struct Conn(Arc<Handle>);

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        match overlapped(&self.0, |ov| unsafe { ReadFile(self.0 .0, buf.as_mut_ptr(), len, std::ptr::null_mut(), ov) }) {
            Ok(n) => Ok(n as usize),
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
            Err(e) => Err(e),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        overlapped(&self.0, |ov| unsafe { WriteFile(self.0 .0, buf.as_ptr(), len, std::ptr::null_mut(), ov) })
            .map(|n| n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serves the pipe forever: each client gets its own thread running [`crate::serve`] with a
/// fresh [`crate::Context`] from `context`. Returns only on a setup error (for example, another
/// process already owns the name).
pub fn listen(context: impl Fn() -> crate::Context + Send + Sync + 'static) -> io::Result<()> {
    listen_on(&name(), context)
}

fn listen_on(name: &str, context: impl Fn() -> crate::Context + Send + Sync + 'static) -> io::Result<()> {
    let name = wide(name);
    let sd = Descriptor::owner_only()?;
    let context = Arc::new(context);
    let mut first = true;
    loop {
        let pipe = create(&name, first, &sd)?;
        first = false;
        let connected = overlapped(&pipe, |ov| unsafe { ConnectNamedPipe(pipe.0, ov) });
        match connected {
            Ok(_) => {}
            // A client that connected between create and ConnectNamedPipe.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) => {}
            Err(_) => continue,
        }
        let conn = Conn(Arc::new(pipe));
        let ctx = context();
        let _ = std::thread::Builder::new().name("plugin-host-client".into()).spawn(move || {
            let _ = crate::serve(BufReader::new(conn.clone()), conn, ctx);
        });
    }
}

/// Opens the pipe as a client (foxglove-cli, tests). Plain synchronous I/O is enough for
/// request-then-response use.
pub fn connect() -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new().read(true).write(true).open(name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn serves_a_client_over_the_pipe() {
        let name = format!(r"\\.\pipe\foxglove-test-{}", std::process::id());
        let listen_name = name.clone();
        std::thread::spawn(move || {
            let r = listen_on(&listen_name, || crate::Context {
                facts: foxglove_devices::facts::Facts::default(),
                devices: Arc::new(crate::NoDevices),
                client: crate::Client::Plugin(vec![]),
                app_version: "test".into(),
                limits: crate::Limits::default(),
            });
            panic!("listen ended: {r:?}");
        });
        let mut client = None;
        for _ in 0..200 {
            if let Ok(c) = std::fs::OpenOptions::new().read(true).write(true).open(&name) {
                client = Some(c);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut client = client.expect("the pipe comes up");
        writeln!(client, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"api":"1.0","client":{{"name":"t","version":"1"}}}}}}"#).unwrap();
        writeln!(client, r#"{{"jsonrpc":"2.0","id":2,"method":"ping"}}"#).unwrap();
        let mut lines = io::BufReader::new(client).lines();
        let first: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(first["result"]["app"], "test");
        let second: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(second["id"], 2);
        assert!(listen_on(&name, || unreachable!()).is_err(), "a squatted name is refused");
    }
}
