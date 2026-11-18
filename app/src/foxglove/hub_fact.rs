//! H5.5: the app asks the hub two things over the VERIFIED hub pipe (contract 1.19, section 8.8):
//! the fact `module.foxglove.state` (is the hub running Foxglove as its module?) and `hub.show`
//! (bring Foxglove's page to the front).
//!
//! The connection comes from `owner::open_verified`, so the server's process was checked (same
//! user, image `hideout.exe`, trusted path, no AppContainer) on the very handle the request goes
//! over; a server that fails the checks gets nothing at all. Nothing here carries a secret: the
//! `initialize` names the app and asks for the one read grant. Every failure, refusal, timeout or
//! odd answer is "don't know", which start-up treats as "start normally".
//!
//! The wire is newline-delimited JSON-RPC (section 8.2). All I/O runs on a worker thread so a
//! hub that accepts and then says nothing costs the limit, not a hung start-up.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::signin_handoff::Hosting;

/// The hub fact this module reads.
pub const FACT: &str = "module.foxglove.state";
/// The module id `hub.show` brings forward.
pub const MODULE: &str = "foxglove";
/// The most start-up waits for the hub for one conversation.
pub const LIMIT: Duration = Duration::from_millis(1500);
/// Lines read while waiting for one answer (events may come first).
const MAX_LINES: usize = 16;
/// The longest line read: a hub that never ends a line costs this much memory, not more.
const MAX_LINE: u64 = 64 * 1024;

/// Why no verified connection exists.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Nobody serves the name.
    Free,
    /// A server that did not verify (or is busy): nothing was sent.
    Foreign(String),
}

pub type Ends = (Box<dyn Read + Send>, Box<dyn Write + Send>);

/// Opens a connection whose server was verified before this returns.
pub trait Connect: Send + Sync + 'static {
    fn open(&self) -> Result<Ends, Refused>;
}

/// The real hub pipe (`foxglove-<user>`, served by `hideout.exe`).
#[cfg(windows)]
pub struct HubPipe;

#[cfg(windows)]
impl Connect for HubPipe {
    fn open(&self) -> Result<Ends, Refused> {
        use foxglove_plugin::owner::{open_verified, Expect, Probe, TIMEOUT};
        let name = foxglove_plugin_host::pipe::name();
        match open_verified(&name, Expect::HIDEOUT, TIMEOUT) {
            Ok(file) => {
                let w = file
                    .try_clone()
                    .map_err(|e| Refused::Foreign(e.to_string()))?;
                Ok((Box::new(file), Box::new(w)))
            }
            Err(Probe::Foreign { why, .. }) => Err(Refused::Foreign(why)),
            Err(_) => Err(Refused::Free),
        }
    }
}

/// What a conversation asks after `initialize`.
#[derive(Clone, Copy)]
enum Ask {
    Fact,
    Show,
}

fn request(id: i64, method: &str, params: Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    )
}

/// Reads lines until the answer to `id`; its `result`, or `None` for an error, EOF or noise.
fn answer<R: BufRead>(r: &mut R, id: i64) -> Option<Value> {
    for _ in 0..MAX_LINES {
        let mut line = String::new();
        let n = r.by_ref().take(MAX_LINE + 1).read_line(&mut line).ok()?;
        if n == 0 || (n as u64 > MAX_LINE && !line.ends_with('\n')) {
            return None;
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("id").and_then(Value::as_i64) == Some(id) {
            return v.get("result").cloned();
        }
    }
    None
}

fn converse<R: Read, W: Write>(r: R, mut w: W, ask: Ask) -> Option<Value> {
    let mut r = BufReader::new(r);
    let permissions: Vec<&str> = match ask {
        Ask::Fact => vec!["facts.read:module.foxglove.state"],
        Ask::Show => vec![],
    };
    let init = request(
        0,
        "initialize",
        json!({"client": {"name": "foxglove-app", "version": env!("CARGO_PKG_VERSION")},
               "api": {}, "permissions": permissions}),
    );
    w.write_all(init.as_bytes()).ok()?;
    answer(&mut r, 0)?;
    let line = match ask {
        Ask::Fact => request(1, "facts.get", json!({"key": FACT})),
        Ask::Show => request(1, "hub.show", json!({"id": MODULE})),
    };
    w.write_all(line.as_bytes()).ok()?;
    answer(&mut r, 1)
}

/// Runs one conversation on a worker thread, within `limit`.
fn within<C: Connect>(c: Arc<C>, ask: Ask, limit: Duration) -> Option<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("hub-fact".into())
        .spawn(move || {
            let got = c.open().ok().and_then(|(r, w)| converse(r, w, ask));
            let _ = tx.send(got);
        })
        .ok()?;
    rx.recv_timeout(limit).ok().flatten()
}

/// Does the hub report Foxglove's row as running? Only the word "running" is `Hosted`; any
/// other value is `OnItsOwn`; no answer is `Unknown`.
pub fn hosting<C: Connect>(c: Arc<C>, limit: Duration) -> Hosting {
    match within(c, Ask::Fact, limit) {
        Some(v) => match v.get("value").and_then(Value::as_str) {
            Some("running") => Hosting::Hosted,
            _ => Hosting::OnItsOwn,
        },
        None => Hosting::Unknown,
    }
}

/// Asks the hub to show Foxglove's page. True when it said yes; an older hub (-32601) or any
/// failure is false, and the caller exits quietly anyway.
pub fn show<C: Connect>(c: Arc<C>, limit: Duration) -> bool {
    within(c, Ask::Show, limit).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Mutex;

    /// A canned server: the reply text, and everything the app wrote.
    struct Mock {
        reply: Result<String, Refused>,
        wrote: Arc<Mutex<Vec<u8>>>,
    }

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

    impl Connect for Mock {
        fn open(&self) -> Result<Ends, Refused> {
            match &self.reply {
                Ok(t) => Ok((
                    Box::new(Cursor::new(t.clone().into_bytes())),
                    Box::new(Sink(self.wrote.clone())),
                )),
                Err(Refused::Free) => Err(Refused::Free),
                Err(Refused::Foreign(w)) => Err(Refused::Foreign(w.clone())),
            }
        }
    }

    fn mock(reply: Result<&str, Refused>) -> (Arc<Mock>, Arc<Mutex<Vec<u8>>>) {
        let wrote = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Mock {
                reply: reply.map(String::from),
                wrote: wrote.clone(),
            }),
            wrote,
        )
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":0,"result":{"hub":"1.4"}}"#;
    const T: Duration = Duration::from_secs(2);

    fn replies(second: &str) -> String {
        format!("{INIT}\n{second}\n")
    }

    fn sent(wrote: &Arc<Mutex<Vec<u8>>>) -> Vec<Value> {
        let text = String::from_utf8(wrote.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn running_means_hosted_and_only_running() {
        for (word, want) in [
            ("running", Hosting::Hosted),
            ("restarting", Hosting::OnItsOwn),
            ("crash-stopped", Hosting::OnItsOwn),
        ] {
            let (m, _) = mock(Ok(&replies(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"value":"{word}"}}}}"#
            ))));
            assert_eq!(hosting(m, T), want, "{word}");
        }
        // The fact is not set (null): the hub does not host it.
        let (m, _) = mock(Ok(&replies(
            r#"{"jsonrpc":"2.0","id":1,"result":{"value":null}}"#,
        )));
        assert_eq!(hosting(m, T), Hosting::OnItsOwn);
    }

    #[test]
    fn it_asks_for_the_one_fact_with_the_one_grant_and_sends_no_secret() {
        let (m, wrote) = mock(Ok(&replies(
            r#"{"jsonrpc":"2.0","id":1,"result":{"value":"running"}}"#,
        )));
        hosting(m, T);
        let lines = sent(&wrote);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["method"], "initialize");
        assert_eq!(
            lines[0]["params"]["permissions"],
            json!(["facts.read:module.foxglove.state"])
        );
        assert!(lines[0]["params"].get("token").is_none());
        assert_eq!(lines[1]["method"], "facts.get");
        assert_eq!(lines[1]["params"], json!({"key": FACT}));
    }

    #[test]
    fn show_sends_hub_show_for_foxglove_and_asks_for_no_grant() {
        let (m, wrote) = mock(Ok(&replies(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)));
        assert!(show(m, T));
        let lines = sent(&wrote);
        assert_eq!(lines[0]["params"]["permissions"], json!([]));
        assert_eq!(lines[1]["method"], "hub.show");
        assert_eq!(lines[1]["params"], json!({"id": "foxglove"}));
    }

    #[test]
    fn an_unverified_or_absent_server_gets_nothing_and_means_unknown() {
        for r in [Refused::Free, Refused::Foreign("not hideout.exe".into())] {
            let (m, wrote) = mock(Err(r));
            assert_eq!(hosting(m.clone(), T), Hosting::Unknown);
            assert!(!show(m, T));
            assert!(wrote.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn errors_garbage_and_silence_are_all_unknown() {
        let denied = replies(r#"{"jsonrpc":"2.0","id":1,"error":{"code":5,"message":"denied"}}"#);
        let older =
            replies(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no method"}}"#);
        let refused_init = r#"{"jsonrpc":"2.0","id":0,"error":{"code":4,"message":"version"}}"#;
        for text in [denied.as_str(), "", "not json\n", INIT, refused_init] {
            let (m, _) = mock(Ok(text));
            assert_eq!(hosting(m, T), Hosting::Unknown, "{text}");
        }
        let (m, _) = mock(Ok(&older));
        assert!(!show(m, T));
    }

    #[test]
    fn a_hub_that_accepts_and_says_nothing_costs_the_limit_not_a_hang() {
        struct Silent(mpsc::Receiver<()>);
        impl Read for Silent {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                // Blocks until the test drops its sender.
                let _ = self.0.recv();
                Ok(0)
            }
        }
        struct Hang(Mutex<Option<mpsc::Receiver<()>>>);
        impl Connect for Hang {
            fn open(&self) -> Result<Ends, Refused> {
                let rx = self.0.lock().unwrap().take().unwrap();
                Ok((Box::new(Silent(rx)), Box::new(Sink(Arc::default()))))
            }
        }
        let (tx, rx) = mpsc::channel::<()>();
        let t = std::time::Instant::now();
        let got = hosting(
            Arc::new(Hang(Mutex::new(Some(rx)))),
            Duration::from_millis(150),
        );
        assert_eq!(got, Hosting::Unknown);
        assert!(t.elapsed() < Duration::from_secs(2));
        drop(tx);
    }

    #[test]
    fn a_line_that_never_ends_is_cut_off_not_buffered() {
        let long = "x".repeat(200 * 1024);
        let (m, _) = mock(Ok(&format!("{INIT}\n{long}")));
        assert_eq!(hosting(m, T), Hosting::Unknown);
    }

    #[test]
    fn events_before_the_answer_are_skipped() {
        let text = format!(
            "{INIT}\n{}\n{}\n",
            r#"{"jsonrpc":"2.0","method":"event","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{"value":"running"}}"#
        );
        let (m, _) = mock(Ok(&text));
        assert_eq!(hosting(m, T), Hosting::Hosted);
    }
}
