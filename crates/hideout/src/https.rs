// SPDX-License-Identifier: MIT
//! One GET through WinHTTP (#1049), for the catalog fetch. Windows' own HTTP stack, so TLS,
//! certificate checks and the system proxy are the OS's. No new crate: `windows-sys` is already
//! in the tree (MIT/Apache-2.0).
//!
//! - One request per call, no retry. Every phase has a timeout ([`TIMEOUT_MS`]) and the whole
//!   request a deadline ([`DEADLINE_MS`]): when it passes, a watchdog closes the request handle,
//!   which is how WinHTTP cancels a synchronous call in progress, so a server that drips bytes
//!   can't hold the fetch.
//! - Redirects: set explicitly to never go from HTTPS to plain HTTP.
//! - The https-only rule for sources is the registry's (`hideout_registry::catalogs`); this
//!   client also speaks plain HTTP for the window-free tests against a loopback server.

use hideout_registry::catalogs::Fetch;

/// Each phase (name lookup, connect, send, each receive) gives up after this long.
pub const TIMEOUT_MS: i32 = 15_000;
/// A whole request (all phases and the body) gives up after this long.
pub const DEADLINE_MS: u64 = 60_000;

/// The hub's catalog client.
#[derive(Clone, Copy, Debug)]
pub struct WinHttp {
    pub timeout_ms: i32,
    pub deadline_ms: u64,
}

impl Default for WinHttp {
    fn default() -> Self {
        WinHttp {
            timeout_ms: TIMEOUT_MS,
            deadline_ms: DEADLINE_MS,
        }
    }
}

impl Fetch for WinHttp {
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        win::get(&parse(url)?, max, self.timeout_ms, self.deadline_ms)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Url {
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse(url: &str) -> Result<Url, String> {
    let lower = |n: usize| url.get(..n).map(str::to_ascii_lowercase);
    let (secure, rest) = if lower(8).as_deref() == Some("https://") {
        (true, &url[8..])
    } else if lower(7).as_deref() == Some("http://") {
        (false, &url[7..])
    } else {
        return Err("not an http(s) url".into());
    };
    let (auth, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split('#').next().unwrap_or("/");
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if auth.is_empty() || auth.contains('@') || auth.chars().any(char::is_whitespace) {
        return Err("the url has no plain host".into());
    }
    let default = if secure { 443 } else { 80 };
    let (host, port) = match auth.rsplit_once(':') {
        // A bare IPv6 literal, no port.
        Some((h, _)) if !h.ends_with(']') && h.contains(':') => (auth.to_string(), default),
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| "the url's port is not a number")?,
        ),
        None => (auth.to_string(), default),
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if host.is_empty() {
        return Err("the url has no host".into());
    }
    Ok(Url {
        secure,
        host,
        port,
        path,
    })
}

mod win {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::*;

    use super::Url;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A WinHTTP session or connection handle, closed on drop.
    struct Handle(*mut c_void);

    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a handle WinHTTP returned, not null, closed once (here only).
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    /// The request handle, shared with the deadline watchdog: whoever takes it out closes it,
    /// so it is closed exactly once. `None` once closed.
    type Slot = Arc<Mutex<Option<usize>>>;

    fn close_slot(slot: &Slot) {
        if let Some(h) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            // SAFETY: a request handle WinHTTP returned, taken out of the slot, so this is the
            // only close. Closing it is WinHTTP's documented way to end a synchronous call in
            // progress on it (the call then fails, and the caller stops using the handle).
            unsafe { WinHttpCloseHandle(h as *mut c_void) };
        }
    }

    /// The last WinHTTP error in words, for the log.
    fn last(what: &str) -> String {
        // SAFETY: reads the calling thread's last-error value; no arguments.
        let code = unsafe { GetLastError() };
        let words = match code {
            12002 => "timed out",
            12007 => "the name couldn't be resolved",
            12017 => "cancelled",
            12029 => "couldn't connect",
            12030 => "the connection ended",
            12175 | 12045 | 12044 | 12057 | 12037 | 12038 => "the certificate isn't trusted",
            _ => "",
        };
        if words.is_empty() {
            format!("{what}: WinHTTP error {code}")
        } else {
            format!("{what}: {words} (WinHTTP error {code})")
        }
    }

    fn handle(h: *mut c_void, what: &str) -> Result<Handle, String> {
        if h.is_null() {
            Err(last(what))
        } else {
            Ok(Handle(h))
        }
    }

    pub(super) fn get(
        url: &Url,
        max: usize,
        timeout_ms: i32,
        deadline_ms: u64,
    ) -> Result<Vec<u8>, String> {
        let deadline = Duration::from_millis(deadline_ms);
        let started = Instant::now();
        let over = || format!("took over {} s; gave up", deadline.as_secs_f32());
        let agent = wide(concat!("Hideout/", env!("CARGO_PKG_VERSION")));
        let session = handle(
            // SAFETY: `agent` is NUL-terminated and outlives the call; null proxy and bypass
            // lists are allowed with the automatic-proxy access type.
            unsafe {
                WinHttpOpen(
                    agent.as_ptr(),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    null(),
                    null(),
                    0,
                )
            },
            "open",
        )?;
        let t = timeout_ms.min(i32::try_from(deadline_ms).unwrap_or(i32::MAX));
        // SAFETY: `session` is a live session handle.
        if unsafe { WinHttpSetTimeouts(session.0, t, t, t, t) } == 0 {
            return Err(last("timeouts"));
        }
        let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
        // SAFETY: `session` is live; the buffer is a u32 that outlives the call, its size given.
        let set = unsafe {
            WinHttpSetOption(
                session.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&policy as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        if set == 0 {
            return Err(last("redirect policy"));
        }
        let host = wide(&url.host);
        let connect = handle(
            // SAFETY: `session` is live; `host` is NUL-terminated and outlives the call.
            unsafe { WinHttpConnect(session.0, host.as_ptr(), url.port, 0) },
            "connect",
        )?;
        let (verb, path) = (wide("GET"), wide(&url.path));
        let flags = if url.secure { WINHTTP_FLAG_SECURE } else { 0 };
        // SAFETY: `connect` is live; `verb` and `path` are NUL-terminated and outlive the call;
        // null version, referrer and accept types mean the defaults.
        let request = unsafe {
            WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                flags,
            )
        };
        if request.is_null() {
            return Err(last("request"));
        }
        let slot: Slot = Arc::new(Mutex::new(Some(request as usize)));
        // The watchdog: at the deadline it closes the request, which ends any call blocked on
        // it. Its stop event is this request ending (the sender dropped); the deadline is the
        // fallback that cancels.
        let (done, wait) = mpsc::channel::<()>();
        let dog = {
            let slot = slot.clone();
            std::thread::Builder::new()
                .name("hideout-https-deadline".into())
                .spawn(move || {
                    if wait.recv_timeout(deadline) == Err(RecvTimeoutError::Timeout) {
                        close_slot(&slot);
                    }
                })
                .map_err(|e| format!("deadline watch: {e}"))?
        };
        let result = run(&slot, max, started, deadline, &over);
        close_slot(&slot);
        drop(done);
        let _ = dog.join();
        result
    }

    /// Send, response, status and body on the request in `slot`. Every call first takes the
    /// handle out of the slot: `None` means the watchdog closed it (the deadline passed).
    fn run(
        slot: &Slot,
        max: usize,
        started: Instant,
        deadline: Duration,
        over: &dyn Fn() -> String,
    ) -> Result<Vec<u8>, String> {
        let req = || -> Result<*mut c_void, String> {
            if started.elapsed() >= deadline {
                return Err(over());
            }
            slot.lock()
                .unwrap_or_else(|e| e.into_inner())
                .map(|h| h as *mut c_void)
                .ok_or_else(over)
        };
        // A failed call after the deadline is the watchdog's cancel, said as such.
        let fail = |what: &str| {
            if started.elapsed() >= deadline {
                over()
            } else {
                last(what)
            }
        };
        // SAFETY (all calls below): `req()` gives the request handle while the slot holds it;
        // if the watchdog closes it during a call, WinHTTP fails that call (cancelled) and no
        // later call is made on it, since `req()` then returns the deadline error. Buffers and
        // out-pointers are locals that outlive each call, with their sizes given.
        if unsafe { WinHttpSendRequest(req()?, null(), 0, null(), 0, 0, 0) } == 0 {
            return Err(fail("send"));
        }
        if unsafe { WinHttpReceiveResponse(req()?, null_mut()) } == 0 {
            return Err(fail("response"));
        }
        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let ok = unsafe {
            WinHttpQueryHeaders(
                req()?,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut len,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(fail("status"));
        }
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        let mut body = Vec::new();
        loop {
            let mut avail: u32 = 0;
            if unsafe { WinHttpQueryDataAvailable(req()?, &mut avail) } == 0 {
                return Err(fail("read"));
            }
            if avail == 0 {
                break;
            }
            if body.len() + avail as usize > max {
                return Err(format!("over {max} bytes"));
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read: u32 = 0;
            let ok = unsafe {
                WinHttpReadData(req()?, body[start..].as_mut_ptr().cast(), avail, &mut read)
            };
            if ok == 0 {
                return Err(fail("read"));
            }
            body.truncate(start + read as usize);
            if read == 0 {
                break;
            }
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn urls_are_read_as_scheme_host_port_path() {
        let u = parse("https://Cat.example/a/catalog.json?x=1#f").unwrap();
        assert_eq!(
            u,
            Url {
                secure: true,
                host: "Cat.example".into(),
                port: 443,
                path: "/a/catalog.json?x=1".into()
            }
        );
        let u = parse("http://127.0.0.1:8080").unwrap();
        assert_eq!((u.secure, u.port, u.path.as_str()), (false, 8080, "/"));
        let u = parse("https://[::1]:9/c").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("::1", 9));
        for bad in [
            "ftp://x/",
            "https://",
            "https://u:p@h/",
            "https://h:x/",
            "h/c",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    /// One loopback HTTP server answering `replies` in order, one connection each. Start
    /// event: bind; stop event: the last reply sent (the thread then ends).
    fn serve(replies: Vec<(u16, Vec<u8>)>) -> u16 {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let Ok((mut s, _)) = l.accept() else { return };
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        port
    }

    fn client(timeout_ms: i32, deadline_ms: u64) -> WinHttp {
        WinHttp {
            timeout_ms,
            deadline_ms,
        }
    }

    #[test]
    fn a_get_reads_the_body_and_refuses_other_statuses_and_big_bodies() {
        let port = serve(vec![
            (200, b"{\"ok\":1}".to_vec()),
            (404, b"no".to_vec()),
            (200, vec![b'x'; 100]),
        ]);
        let c = client(5_000, 10_000);
        let url = format!("http://127.0.0.1:{port}/catalog.json");
        assert_eq!(c.get(&url, 1024).unwrap(), b"{\"ok\":1}");
        assert_eq!(c.get(&url, 1024).unwrap_err(), "HTTP 404");
        assert!(c.get(&url, 10).unwrap_err().contains("over 10 bytes"));
    }

    #[test]
    fn nothing_listening_is_an_error_not_a_hang() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let e = client(3_000, 10_000)
            .get(&format!("http://127.0.0.1:{port}/catalog.json"), 1024)
            .unwrap_err();
        assert!(e.contains("WinHTTP error"), "{e}");
    }

    /// A server that drips one header byte every 100 ms, never finishing: well inside the
    /// per-phase timeout each time, so only the overall deadline ends it.
    #[test]
    fn a_dripping_server_is_cut_off_at_the_deadline() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        // Stop event: the client closes the connection (a write fails); 30 s is the fallback.
        std::thread::spawn(move || {
            let Ok((mut s, _)) = l.accept() else { return };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = s.read(&mut [0u8; 4096]);
            let stop = Instant::now() + Duration::from_secs(30);
            for b in b"HTTP/1.1 200 OK\r\nX-Slow: "
                .iter()
                .chain(std::iter::repeat(&b'a'))
            {
                if Instant::now() > stop || s.write_all(&[*b]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let t0 = Instant::now();
        let e = client(5_000, 1_000)
            .get(&format!("http://127.0.0.1:{port}/catalog.json"), 1024)
            .unwrap_err();
        let took = t0.elapsed();
        assert!(e.contains("gave up"), "{e}");
        assert!(took < Duration::from_secs(4), "took {took:?}");
    }
}
