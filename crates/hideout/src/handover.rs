// SPDX-License-Identifier: MIT
//! The hand-over from a standalone copy of an app (first-run plan FR5, decision 5).
//!
//! When the user allows an app that is also open on its own, Hideout may ask that copy, over its
//! verified pipe, to quit cleanly and then start the app inside Hideout (`--module`). It fails
//! safe: the copy is asked only when it was seen as a verified copy of the app, the request goes
//! over the very connection whose server was checked (`owner::open_verified`), and every doubt
//! (an unverified server, no answer, a refusal, a copy that does not leave in time) leaves the
//! copy open and the app unstarted, with a note and **Start again** on the page. Nothing here
//! reads or writes the Startup entry or any other setting; the app's own settings are untouched.
//!
//! Pure decision (`hand_over`), the wire conversation (`converse`, testable on in-memory ends)
//! and the real pipe (`PipeOpen`).
//!
//! Known limits, accepted on purpose:
//! - **A name-free race.** Between the last look (`seen() == No`) and the app's start, another
//!   standalone copy could start (for example from its own Startup entry at sign-in) and take
//!   the name, so two copies run. The window is milliseconds and the outcome is the same as
//!   starting the app beside a copy without a hand-over; it is not guarded further.
//! - **The caller's process id.** The app reads the kernel's client pid when the connection is
//!   made and verifies that process a moment later. A client that exits in between, whose pid is
//!   reused by a verified hideout.exe, would pass. The only effect is a clean quit of the app,
//!   which any process of the same user can already cause (`taskkill`), so the id is read once
//!   per connection. If the request ever guards more than a quit, read it once per call.
//! - **One app's wiring.** `main.rs` wires only Foxglove's pipe and exe (`foxglove`). The copy
//!   shown to the user is generic; later the pipe name and image should come from the manifest.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

/// What is on the app's own pipe name right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Nobody: no standalone copy (or none this app has).
    No,
    /// A server that passed the identity checks as this app.
    Standalone,
    /// Something is there that did not verify, or could not be looked at: never asked, never
    /// said to be a copy.
    Unsure,
}

/// What the hub knows about standalone copies, apart from the OS (tests use a fake).
pub trait Standalone: Send + Sync {
    /// Whether a standalone copy of app `id` is open (no byte is sent).
    fn seen(&self, id: &str) -> Seen;
    /// Asks the copy to quit and waits (bounded) until it has gone. True only when it said yes
    /// and then went.
    fn ask_to_quit(&self, id: &str) -> bool;
    /// The copy has gone and the name is free: the hub takes the app's alias name for its own
    /// clients (CLI and SDK tools on `foxglove-<user>`), as it does when it starts with the name
    /// free. Nothing for a fake.
    fn freed(&self, _id: &str) {}
}

/// Closes the standalone copy of `id` if there is one. True: the app may be started now. False:
/// the copy stays (in doubt, refused, or did not leave) and the app is not started.
pub fn hand_over(s: &dyn Standalone, id: &str) -> bool {
    match s.seen(id) {
        // It closed on its own since the dialog: nothing to hand over.
        Seen::No => true,
        Seen::Unsure => false,
        Seen::Standalone => {
            let gone = s.ask_to_quit(id) && s.seen(id) == Seen::No;
            if gone {
                s.freed(id);
            }
            gone
        }
    }
}

/// The plugin API version this hub speaks to a standalone app, as `major.minor` text (the app's
/// `initialize` takes a string; the `{}` map belongs to the hub's own router protocol).
const API: &str = "1.17";

/// How long the hub waits for the copy to answer and leave: its own hand-back budget is 4 s.
pub const LIMIT: Duration = Duration::from_secs(8);
/// Lines read while waiting for one answer (events may come first).
const MAX_LINES: usize = 16;
/// The longest line read.
const MAX_LINE: u64 = 64 * 1024;

/// Ends a read blocked on one connection from another thread (the wait is over): that read
/// returns at once and the worker holding the connection can finish and let it go.
pub type Cancel = Box<dyn Fn() + Send + Sync>;

/// A verified connection: its read end, its write end and how to cancel a blocked read.
pub type Ends = (Box<dyn Read + Send>, Box<dyn Write + Send>, Cancel);

/// Where the copy's pipe is and how it is verified.
pub trait Open: Send + Sync + 'static {
    fn probe(&self) -> Seen;
    /// A connection whose server was verified before this returns.
    fn open(&self) -> Option<Ends>;
    /// The copy has gone: see [`Standalone::freed`].
    fn freed(&self) {}
}

fn request(id: i64, method: &str, params: Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    )
}

/// Reads lines until the answer to `id`; true when it carries a `result` (an error, EOF or noise
/// is false).
fn answered<R: BufRead>(r: &mut R, id: i64) -> bool {
    for _ in 0..MAX_LINES {
        let mut line = String::new();
        match r.by_ref().take(MAX_LINE + 1).read_line(&mut line) {
            Ok(n) if n > 0 && (n as u64 <= MAX_LINE || line.ends_with('\n')) => {}
            _ => return false,
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("id").and_then(Value::as_i64) == Some(id) {
            return v.get("result").is_some();
        }
    }
    false
}

/// The conversation: initialize, `app.hand_over`, then wait for the pipe to close (the copy
/// exited, an event the kernel gives, no polling). True when it said yes and then closed.
fn converse<R: Read, W: Write>(r: R, mut w: W) -> bool {
    let mut r = BufReader::new(r);
    let init = request(
        0,
        "initialize",
        json!({"client": {"name": "hideout", "version": env!("CARGO_PKG_VERSION")},
               "api": API, "permissions": ["app.windows"]}),
    );
    if w.write_all(init.as_bytes()).is_err() || !answered(&mut r, 0) {
        return false;
    }
    if w.write_all(request(1, "app.hand_over", json!({})).as_bytes())
        .is_err()
        || !answered(&mut r, 1)
    {
        return false;
    }
    // Anything it still says (events) is read and dropped until the pipe closes.
    let mut sink = Vec::new();
    loop {
        sink.clear();
        match r.by_ref().take(MAX_LINE).read_until(b'\n', &mut sink) {
            Ok(0) | Err(_) => return true,
            Ok(_) => {}
        }
    }
}

/// A read end that fails once the wait has been cancelled.
struct Gated {
    r: Box<dyn Read + Send>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

impl Read for Gated {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::ErrorKind::ConnectionAborted.into());
        }
        self.r.read(b)
    }
}

/// [`converse`] on a worker thread, answered within `limit`. When the limit passes the blocked
/// read is cancelled, so the worker ends and the connection is let go (no thread is left behind
/// for each Start again).
fn within<O: Open>(o: Arc<O>, limit: Duration) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (tx, rx) = mpsc::channel();
    let (cancel_tx, cancel_rx) = mpsc::channel::<Arc<Cancel>>();
    let late = Arc::new(AtomicBool::new(false));
    let worker_late = late.clone();
    let spawned = std::thread::Builder::new()
        .name("hand-over".into())
        .spawn(move || {
            let went = match o.open() {
                Some((r, w, cancel)) => {
                    // A cancel ends the read pending now and fails every later one, so a copy
                    // that keeps sending cannot hold the worker (`CancelIoEx` alone reaches only
                    // a read that is already waiting).
                    let stopped = Arc::new(AtomicBool::new(false));
                    let flag = stopped.clone();
                    let cancel: Arc<Cancel> = Arc::new(Box::new(move || {
                        flag.store(true, Ordering::SeqCst);
                        cancel();
                    }));
                    // Hand the cancel over first, then look at the flag: whichever of the two
                    // threads comes second finds the other's mark, so one of them cancels.
                    let _ = cancel_tx.send(cancel.clone());
                    if worker_late.load(Ordering::SeqCst) {
                        cancel();
                    }
                    converse(Gated { r, stopped }, w)
                }
                None => false,
            };
            let _ = tx.send(went);
        });
    if spawned.is_err() {
        return false;
    }
    match rx.recv_timeout(limit) {
        Ok(went) => went,
        Err(_) => {
            late.store(true, Ordering::SeqCst);
            if let Ok(cancel) = cancel_rx.try_recv() {
                cancel();
            }
            false
        }
    }
}

/// The standalone copy of one app over `O`.
pub struct Wire<O: Open> {
    pub id: String,
    pub open: Arc<O>,
    pub limit: Duration,
}

impl<O: Open> Standalone for Wire<O> {
    fn seen(&self, id: &str) -> Seen {
        if id == self.id {
            self.open.probe()
        } else {
            Seen::No
        }
    }

    fn ask_to_quit(&self, id: &str) -> bool {
        id == self.id && within(self.open.clone(), self.limit)
    }

    fn freed(&self, id: &str) {
        if id == self.id {
            self.open.freed();
        }
    }
}

/// The real pipe of a standalone app (Windows): who is on `pipe` is checked against the app's
/// image name and the installer's records (`find_installed`), never against a reply.
#[cfg(windows)]
pub struct PipeOpen {
    pub pipe: String,
    pub exe: foxglove_plugin::owner::Expect,
    pub home: std::path::PathBuf,
    pub id: String,
    /// The hub, which serves the alias once the copy has left the name.
    pub hub: hideout_router::server::Hub,
}

#[cfg(windows)]
impl PipeOpen {
    fn peers(&self) -> crate::find_installed::InstalledPeers {
        let adopted = hideout_registry::Registry::open_from_env(&self.home)
            .ok()
            .and_then(|r| r.permission_view(&self.id).ok())
            .map(|v| vec![std::path::PathBuf::from(v.listing.exe_path)])
            .unwrap_or_default();
        crate::find_installed::InstalledPeers {
            found: crate::find_installed::find_all(&crate::find_installed::RealMachine),
            adopted,
        }
    }
}

#[cfg(windows)]
impl Open for PipeOpen {
    fn probe(&self) -> Seen {
        use foxglove_plugin::owner::{probe_with, Probe, TIMEOUT};
        match probe_with(&self.pipe, self.exe, TIMEOUT, &self.peers()) {
            Probe::Free => Seen::No,
            Probe::Verified { .. } => Seen::Standalone,
            Probe::Foreign { .. } => Seen::Unsure,
        }
    }

    fn freed(&self) {
        // The same step the hub takes at its start: a probe first, then the name only if nobody
        // is on it (a copy that started again keeps it).
        let alias = self.hub.alias_pipe();
        if self.pipe != alias {
            // A hub folder of its own has its own alias name, which the copy never held.
            log::info!(
                "hand-over: the copy held {}, not {alias}; no alias claimed",
                self.pipe
            );
            return;
        }
        match self.hub.serve_alias(&alias) {
            hideout_router::AliasOutcome::Served => {
                log::info!("hand-over: serving the alias {alias}")
            }
            hideout_router::AliasOutcome::Left(why) => log::warn!("hand-over: alias: {why}"),
            hideout_router::AliasOutcome::Failed(e) => {
                log::info!("hand-over: alias {alias} not served: {e}")
            }
        }
    }

    fn open(&self) -> Option<Ends> {
        use foxglove_plugin::owner::{open_verified_with, TIMEOUT};
        let file = open_verified_with(&self.pipe, self.exe, TIMEOUT, &self.peers()).ok()?;
        let w = file.try_clone().ok()?;
        // A third handle for the cancel: it lives as long as the cancel closure, so the raw
        // handle it uses can never have been closed and reused.
        let c = file.try_clone().ok()?;
        let cancel: Cancel = Box::new(move || {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: `c` owns a valid handle for the whole call; a null overlapped cancels
            // every read pending on the connection.
            unsafe {
                windows_sys::Win32::System::IO::CancelIoEx(
                    c.as_raw_handle().cast(),
                    std::ptr::null(),
                )
            };
        });
        Some((Box::new(file), Box::new(w), cancel))
    }
}

/// The Foxglove app's standalone copy: the pipe `foxglove-<user>` and the image `foxglove.exe`.
/// The id is the app's, as its manifest names it.
#[cfg(windows)]
pub fn foxglove(
    home: &std::path::Path,
    id: &str,
    hub: hideout_router::server::Hub,
) -> Arc<dyn Standalone> {
    Arc::new(Wire {
        id: id.to_string(),
        open: Arc::new(PipeOpen {
            pipe: hideout_router::server::alias_pipe_name(),
            exe: foxglove_plugin::owner::Expect::FOXGLOVE,
            home: home.to_path_buf(),
            id: id.to_string(),
            hub,
        }),
        limit: LIMIT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Mutex;

    /// A fake pipe: what is on it now and what the conversation does.
    struct Fake {
        now: Mutex<Vec<Seen>>,
        quits: bool,
        asked: Mutex<u32>,
        freed: Mutex<u32>,
    }

    impl Fake {
        fn new(seen: &[Seen], quits: bool) -> Fake {
            Fake {
                now: Mutex::new(seen.iter().rev().copied().collect()),
                quits,
                asked: Mutex::new(0),
                freed: Mutex::new(0),
            }
        }
    }

    impl Standalone for Fake {
        fn seen(&self, _: &str) -> Seen {
            let mut n = self.now.lock().unwrap();
            if n.len() > 1 {
                n.pop().unwrap()
            } else {
                *n.last().unwrap()
            }
        }
        fn ask_to_quit(&self, _: &str) -> bool {
            *self.asked.lock().unwrap() += 1;
            self.quits
        }
        fn freed(&self, _: &str) {
            *self.freed.lock().unwrap() += 1;
        }
    }

    /// The alias is claimed only after a copy was closed and is gone: not when nothing was open,
    /// not on a refusal, not when the copy stayed.
    #[test]
    fn the_name_is_taken_only_after_a_copy_has_gone() {
        let f = Fake::new(&[Seen::Standalone, Seen::No], true);
        assert!(hand_over(&f, "app"));
        assert_eq!(*f.freed.lock().unwrap(), 1);
        for (seen, quits) in [
            (vec![Seen::No], true),
            (vec![Seen::Unsure], true),
            (vec![Seen::Standalone], false),
            (vec![Seen::Standalone, Seen::Standalone], true),
        ] {
            let f = Fake::new(&seen, quits);
            hand_over(&f, "app");
            assert_eq!(*f.freed.lock().unwrap(), 0, "{seen:?} {quits}");
        }
    }

    #[test]
    fn nothing_open_means_nothing_to_hand_over() {
        let f = Fake::new(&[Seen::No], true);
        assert!(hand_over(&f, "app"));
        assert_eq!(*f.asked.lock().unwrap(), 0);
    }

    #[test]
    fn a_verified_copy_is_asked_and_must_be_gone_after() {
        let f = Fake::new(&[Seen::Standalone, Seen::No], true);
        assert!(hand_over(&f, "app"));
        assert_eq!(*f.asked.lock().unwrap(), 1);
        // It said yes but is still there: the app is not started.
        let f = Fake::new(&[Seen::Standalone, Seen::Standalone], true);
        assert!(!hand_over(&f, "app"));
    }

    #[test]
    fn every_doubt_leaves_the_copy_and_the_app_stopped() {
        let f = Fake::new(&[Seen::Unsure], true);
        assert!(!hand_over(&f, "app"));
        assert_eq!(
            *f.asked.lock().unwrap(),
            0,
            "an unverified server is never asked"
        );
        let f = Fake::new(&[Seen::Standalone], false);
        assert!(!hand_over(&f, "app"));
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":0,"result":{"api":"1.17"}}"#;
    const YES: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
    const NO: &str =
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":5,"message":"only Hideout may ask this"}}"#;

    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn talk(reply: &str) -> (bool, Vec<Value>) {
        let wrote = Arc::new(Mutex::new(Vec::new()));
        let ok = converse(
            Cursor::new(reply.to_string().into_bytes()),
            Sink(wrote.clone()),
        );
        let text = String::from_utf8(wrote.lock().unwrap().clone()).unwrap();
        let lines = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (ok, lines)
    }

    #[test]
    fn it_asks_with_one_grant_and_no_secret_and_waits_for_the_pipe_to_close() {
        // The canned reply ends after the answer: the pipe closed, the copy is gone.
        let (ok, sent) = talk(&format!("{INIT}\n{YES}\n"));
        assert!(ok);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["params"]["permissions"], json!(["app.windows"]));
        assert!(sent[0]["params"].get("token").is_none());
        assert_eq!(sent[1]["method"], "app.hand_over");
        assert_eq!(sent[1]["params"], json!({}));
    }

    #[test]
    fn a_refusal_an_error_or_silence_is_not_a_yes() {
        assert!(!talk(&format!("{INIT}\n{NO}\n")).0);
        assert!(!talk(&format!("{INIT}\n")).0);
        assert!(!talk("").0);
        assert!(!talk("not json\n").0);
        // The initialize itself refused: the request is never sent.
        let (ok, sent) = talk(r#"{"jsonrpc":"2.0","id":0,"error":{"code":7,"message":"api"}}"#);
        assert!(!ok);
        assert_eq!(sent.len(), 1);
    }

    /// A pipe that answers and then stays open (a copy that does not leave): the wait ends at
    /// the limit and says no.
    struct Stays;
    impl Open for Stays {
        fn probe(&self) -> Seen {
            Seen::Standalone
        }
        fn open(&self) -> Option<Ends> {
            struct Hold(mpsc::Receiver<()>, std::io::Cursor<Vec<u8>>);
            impl Read for Hold {
                fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                    let n = self.1.read(b)?;
                    if n > 0 {
                        return Ok(n);
                    }
                    // Open until the test ends.
                    let _ = self.0.recv_timeout(Duration::from_secs(5));
                    Ok(0)
                }
            }
            let (_tx, rx) = mpsc::channel();
            std::mem::forget(_tx);
            let text = format!("{INIT}\n{YES}\n").into_bytes();
            Some((
                Box::new(Hold(rx, Cursor::new(text))),
                Box::new(Sink(Default::default())),
                Box::new(|| {}),
            ))
        }
    }

    #[test]
    fn a_copy_that_does_not_leave_in_time_is_not_gone() {
        let w = Wire {
            id: "app".into(),
            open: Arc::new(Stays),
            limit: Duration::from_millis(200),
        };
        let t = std::time::Instant::now();
        assert!(!hand_over(&w, "app"));
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn another_apps_id_never_reaches_the_pipe() {
        let w = Wire {
            id: "app".into(),
            open: Arc::new(Stays),
            limit: Duration::from_millis(50),
        };
        assert_eq!(w.seen("other"), Seen::No);
        assert!(!w.ask_to_quit("other"));
        assert_eq!(w.seen("app"), Seen::Standalone);
    }

    /// A pipe end made of channels: the hub writes, the "app" reads, and the other way.
    struct ChanR {
        rx: mpsc::Receiver<Vec<u8>>,
        left: Vec<u8>,
    }
    impl Read for ChanR {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            if self.left.is_empty() {
                match self.rx.recv() {
                    Ok(v) => self.left = v,
                    Err(_) => return Ok(0),
                }
            }
            let n = self.left.len().min(b.len());
            b[..n].copy_from_slice(&self.left[..n]);
            self.left.drain(..n);
            Ok(n)
        }
    }
    struct ChanW(Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>);
    impl Write for ChanW {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            match self.0.lock().unwrap().as_ref() {
                Some(tx) => tx
                    .send(b.to_vec())
                    .map(|_| b.len())
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed")),
                None => Err(std::io::ErrorKind::BrokenPipe.into()),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The app's own side: `Devices` answers `app.hand_over` and then the pipe closes, as the
    /// app's exit closes it.
    struct App {
        asked: Arc<Mutex<u32>>,
        close: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
    }
    impl foxglove_plugin_host::Devices for App {
        fn call(
            &self,
            method: &str,
            _params: Value,
        ) -> Result<Value, foxglove_plugin_api::rpc::Error> {
            assert_eq!(method, "app.hand_over");
            *self.asked.lock().unwrap() += 1;
            let close = self.close.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                close.lock().unwrap().take();
            });
            Ok(json!({}))
        }
    }

    /// The hub's real request against the app's real connection handler (`serve`): the
    /// initialize it sends is accepted, `app.hand_over` is reached, and the wait ends when the
    /// app closes the pipe.
    #[test]
    fn the_real_request_is_accepted_by_the_apps_connection_handler() {
        let (to_app, from_hub) = mpsc::channel();
        let (to_hub, from_app) = mpsc::channel();
        let close = Arc::new(Mutex::new(Some(to_hub)));
        let asked = Arc::new(Mutex::new(0));
        let ctx = foxglove_plugin_host::Context {
            facts: Default::default(),
            devices: Arc::new(App {
                asked: asked.clone(),
                close: close.clone(),
            }),
            client: foxglove_plugin_host::Client::Tool(Arc::new(
                |_: &foxglove_plugin_api::methods::ClientInfo,
                 asked: &[foxglove_plugin_api::permission::Permission]| {
                    asked.to_vec()
                },
            )),
            app_version: "0.3.0".into(),
            limits: Default::default(),
        };
        let reader = BufReader::new(ChanR {
            rx: from_hub,
            left: Vec::new(),
        });
        let server =
            std::thread::spawn(move || foxglove_plugin_host::serve(reader, ChanW(close), ctx));
        let hub_end = ChanR {
            rx: from_app,
            left: Vec::new(),
        };
        let went = converse(hub_end, ChanW(Arc::new(Mutex::new(Some(to_app)))));
        assert!(
            went,
            "the app accepted initialize and app.hand_over, then closed"
        );
        assert_eq!(*asked.lock().unwrap(), 1);
        let _ = server.join();
    }

    #[test]
    fn the_initialize_line_is_the_plugin_apis_own_shape() {
        let (_, sent) = talk(&format!(
            "{INIT}
{YES}
"
        ));
        let p: foxglove_plugin_api::methods::InitializeParams =
            serde_json::from_value(sent[0]["params"].clone()).expect("a valid initialize");
        assert_eq!(p.api, foxglove_plugin_api::API_VERSION.to_string());
    }

    /// A pipe whose read blocks until its cancel is called, as `CancelIoEx` ends a pipe read.
    struct Cancellable {
        ended: Mutex<mpsc::Sender<()>>,
    }
    impl Open for Cancellable {
        fn probe(&self) -> Seen {
            Seen::Standalone
        }
        fn open(&self) -> Option<Ends> {
            struct Block(mpsc::Receiver<()>, Cursor<Vec<u8>>, mpsc::Sender<()>);
            impl Read for Block {
                fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                    let n = self.1.read(b)?;
                    if n > 0 {
                        return Ok(n);
                    }
                    // Blocks until cancelled (no timeout of its own), then fails as a cancel does.
                    let _ = self.0.recv();
                    let _ = self.2.send(());
                    // As the real cancel fails a read: ERROR_OPERATION_ABORTED, which
                    // `read_until` does not retry (it does retry `Interrupted`).
                    Err(std::io::Error::from_raw_os_error(995))
                }
            }
            let (tx, rx) = mpsc::channel::<()>();
            let tx = Mutex::new(Some(tx));
            let text = format!("{INIT}\n{YES}\n").into_bytes();
            let ended = self.ended.lock().unwrap().clone();
            Some((
                Box::new(Block(rx, Cursor::new(text), ended)),
                Box::new(Sink(Default::default())),
                Box::new(move || {
                    tx.lock().unwrap().take();
                }),
            ))
        }
    }

    /// The copy answers but does not leave: at the limit the blocked read is cancelled, so the
    /// worker ends and lets the connection go instead of waiting forever.
    #[test]
    fn a_wait_that_runs_out_cancels_the_blocked_read() {
        let (ended, worker_ended) = mpsc::channel();
        let w = Wire {
            id: "app".into(),
            open: Arc::new(Cancellable {
                ended: Mutex::new(ended),
            }),
            limit: Duration::from_millis(200),
        };
        assert!(!hand_over(&w, "app"));
        // The worker's read returned because of the cancel: its end is the event waited for.
        assert!(
            worker_ended.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the blocked read was never cancelled"
        );
    }
}
