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
}

/// Closes the standalone copy of `id` if there is one. True: the app may be started now. False:
/// the copy stays (in doubt, refused, or did not leave) and the app is not started.
pub fn hand_over(s: &dyn Standalone, id: &str) -> bool {
    match s.seen(id) {
        // It closed on its own since the dialog: nothing to hand over.
        Seen::No => true,
        Seen::Unsure => false,
        Seen::Standalone => s.ask_to_quit(id) && s.seen(id) == Seen::No,
    }
}

/// How long the hub waits for the copy to answer and leave: its own hand-back budget is 4 s.
pub const LIMIT: Duration = Duration::from_secs(8);
/// Lines read while waiting for one answer (events may come first).
const MAX_LINES: usize = 16;
/// The longest line read.
const MAX_LINE: u64 = 64 * 1024;

pub type Ends = (Box<dyn Read + Send>, Box<dyn Write + Send>);

/// Where the copy's pipe is and how it is verified.
pub trait Open: Send + Sync + 'static {
    fn probe(&self) -> Seen;
    /// A connection whose server was verified before this returns.
    fn open(&self) -> Option<Ends>;
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
               "api": {}, "permissions": ["app.windows"]}),
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

/// [`converse`] on a worker thread, answered within `limit`.
fn within<O: Open>(o: Arc<O>, limit: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("hand-over".into())
        .spawn(move || {
            let went = o.open().is_some_and(|(r, w)| converse(r, w));
            let _ = tx.send(went);
        });
    spawned.is_ok() && rx.recv_timeout(limit).unwrap_or(false)
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
}

/// The real pipe of a standalone app (Windows): who is on `pipe` is checked against the app's
/// image name and the installer's records (`find_installed`), never against a reply.
#[cfg(windows)]
pub struct PipeOpen {
    pub pipe: String,
    pub exe: foxglove_plugin::owner::Expect,
    pub home: std::path::PathBuf,
    pub id: String,
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

    fn open(&self) -> Option<Ends> {
        use foxglove_plugin::owner::{open_verified_with, TIMEOUT};
        let file = open_verified_with(&self.pipe, self.exe, TIMEOUT, &self.peers()).ok()?;
        let w = file.try_clone().ok()?;
        Some((Box::new(file), Box::new(w)))
    }
}

/// The Foxglove app's standalone copy: the pipe `foxglove-<user>` and the image `foxglove.exe`.
/// The id is the app's, as its manifest names it.
#[cfg(windows)]
pub fn foxglove(home: &std::path::Path, id: &str) -> Arc<dyn Standalone> {
    Arc::new(Wire {
        id: id.to_string(),
        open: Arc::new(PipeOpen {
            pipe: hideout_router::server::alias_pipe_name(),
            exe: foxglove_plugin::owner::Expect::FOXGLOVE,
            home: home.to_path_buf(),
            id: id.to_string(),
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
    }

    impl Fake {
        fn new(seen: &[Seen], quits: bool) -> Fake {
            Fake {
                now: Mutex::new(seen.iter().rev().copied().collect()),
                quits,
                asked: Mutex::new(0),
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
}
