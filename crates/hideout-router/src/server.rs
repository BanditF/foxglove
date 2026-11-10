// SPDX-License-Identifier: MIT
//! Drives a [`Router`] over real connections: the router pipe and its alias (§8.2, §8.6), the
//! legacy stdio bridge, and in-process streams for tests. One reader and one writer thread per
//! connection; a timer thread that sleeps until the next call deadline (no polling).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::router::{ConnId, Origin, Out, Router, MAX_MESSAGE};

/// `<user>` for the pipe names (§8.2): `%USERNAME%` keeping ASCII letters, digits, `-`, `_`.
pub fn user() -> String {
    std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// `\\.\pipe\hideout-<user>`.
pub fn router_pipe_name() -> String {
    format!(r"\\.\pipe\hideout-{}", user())
}

/// `\\.\pipe\foxglove-<user>`, served as an alias once `foxglove.exe` no longer owns it.
pub fn alias_pipe_name() -> String {
    format!(r"\\.\pipe\foxglove-{}", user())
}

struct Shared {
    router: Mutex<Router>,
    writers: Mutex<HashMap<ConnId, Sender<Option<String>>>>,
    events: Mutex<Sender<Out>>,
    nudge: Mutex<Sender<()>>,
}

/// A router with its connections. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Hub(Arc<Shared>);

impl Hub {
    /// Starts the timer thread. Every [`Out`] that isn't a write or a close goes to `events`
    /// (the supervisor and the panel host).
    pub fn new(router: Router, events: Sender<Out>) -> Hub {
        let (nudge, wake) = mpsc::channel();
        let hub = Hub(Arc::new(Shared {
            router: Mutex::new(router),
            writers: Mutex::new(HashMap::new()),
            events: Mutex::new(events),
            nudge: Mutex::new(nudge),
        }));
        let weak = Arc::downgrade(&hub.0);
        std::thread::Builder::new()
            .name("hideout-router-timer".into())
            .spawn(move || timer(weak, wake))
            .expect("router timer thread");
        hub
    }

    /// Runs `f` on the router and carries out what it returns.
    pub fn act(&self, f: impl FnOnce(&mut Router) -> Vec<Out>) {
        let out = {
            let mut r = self.0.router.lock().unwrap();
            f(&mut r)
        };
        self.dispatch(out);
        let _ = self.0.nudge.lock().unwrap().send(());
    }

    /// Reads the router without changing it.
    pub fn read<T>(&self, f: impl FnOnce(&Router) -> T) -> T {
        f(&self.0.router.lock().unwrap())
    }

    fn dispatch(&self, out: Vec<Out>) {
        for o in out {
            match o {
                Out::Send(c, v) => {
                    if let Some(w) = self.0.writers.lock().unwrap().get(&c) {
                        let _ = w.send(Some(v.to_string()));
                    }
                }
                Out::Close(c) => {
                    if let Some(w) = self.0.writers.lock().unwrap().remove(&c) {
                        let _ = w.send(None);
                    }
                    let more = self.0.router.lock().unwrap().disconnect(c);
                    self.dispatch(more);
                }
                other => {
                    let _ = self.0.events.lock().unwrap().send(other);
                }
            }
        }
    }

    /// Serves one connection over any reader and writer (a pipe instance, a child's stdio, an
    /// in-process stream). Returns at once; the connection lives on its own threads.
    pub fn attach(
        &self,
        origin: Origin,
        reader: impl Read + Send + 'static,
        mut writer: impl Write + Send + 'static,
    ) -> ConnId {
        let conn = self.0.router.lock().unwrap().connect(origin);
        let (tx, rx) = mpsc::channel::<Option<String>>();
        self.0.writers.lock().unwrap().insert(conn, tx);
        std::thread::Builder::new()
            .name(format!("hideout-router-w{conn}"))
            .spawn(move || {
                while let Ok(Some(line)) = rx.recv() {
                    if writer
                        .write_all(format!("{line}\n").as_bytes())
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("router writer thread");
        let hub = self.clone();
        std::thread::Builder::new()
            .name(format!("hideout-router-r{conn}"))
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                while let Ok(Some(line)) = read_line(&mut reader) {
                    let line = line.trim_end_matches(['\r', '\n']);
                    if line.is_empty() {
                        continue;
                    }
                    hub.act(|r| r.receive(conn, line, Instant::now()));
                    if !hub.0.writers.lock().unwrap().contains_key(&conn) {
                        break;
                    }
                }
                hub.0.writers.lock().unwrap().remove(&conn);
                hub.act(|r| r.disconnect(conn));
            })
            .expect("router reader thread");
        conn
    }

    /// The legacy stdio bridge (§8.2): starts a `transport = "stdio"` plugin with its stdin and
    /// stdout on the router as module `id`. The caller (the supervisor) owns the child.
    pub fn spawn_stdio(&self, id: &str, mut command: Command) -> io::Result<Child> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        self.attach(Origin::Bridged(id.into()), stdout, stdin);
        Ok(child)
    }

    /// Serves the router pipe `name` (§8.2): current-user ACL, remote clients rejected. The
    /// first instance is made here, so a name another process owns fails now.
    #[cfg(windows)]
    pub fn serve_pipe(&self, name: &str) -> io::Result<std::thread::JoinHandle<()>> {
        use foxglove_plugin::channel::ServerPipe;
        let first = ServerPipe::create(name, true)?;
        let hub = self.clone();
        let name = name.to_string();
        std::thread::Builder::new()
            .name("hideout-router-accept".into())
            .spawn(move || {
                let mut next = Some(first);
                loop {
                    let pipe = match next.take() {
                        Some(p) => p,
                        None => match ServerPipe::create(&name, false) {
                            Ok(p) => p,
                            Err(e) => {
                                log::error!("router: {name}: {e}");
                                return;
                            }
                        },
                    };
                    match pipe.accept() {
                        Ok(conn) => {
                            hub.attach(Origin::Pipe, conn.clone(), conn);
                        }
                        Err(e) => log::warn!("router: {name}: accept: {e}"),
                    }
                }
            })
    }
}

/// One line of at most [`MAX_MESSAGE`] bytes (a longer one is returned too long, for the router
/// to refuse); `None` at the end of the stream.
fn read_line(r: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r.take(MAX_MESSAGE as u64 + 2).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Sleeps until the next call deadline, or until nudged after any change; fails late calls.
fn timer(hub: std::sync::Weak<Shared>, wake: Receiver<()>) {
    loop {
        let Some(shared) = hub.upgrade() else { return };
        let deadline = shared.router.lock().unwrap().next_deadline();
        drop(shared);
        let got = match deadline {
            None => wake.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(d) => wake.recv_timeout(d.saturating_duration_since(Instant::now())),
        };
        match got {
            Ok(()) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                let Some(shared) = hub.upgrade() else { return };
                Hub(shared).act(|r| r.tick(Instant::now()));
            }
        }
    }
}
