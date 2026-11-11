// SPDX-License-Identifier: MIT
//! Drives a [`Router`] over real connections: the router pipe and its alias (§8.2, §8.6), the
//! legacy stdio bridge, and in-process streams for tests. One reader and one writer thread per
//! connection; a timer thread that sleeps until the next call deadline (no polling).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use serde_json::Value;

use crate::router::{ConnId, Origin, Out, Router, ServiceCall, ServiceError, MAX_MESSAGE};

/// `<user>` for the pipe names (§8.2): `%USERNAME%` keeping ASCII letters, digits, `-`, `_`.
pub fn user() -> String {
    std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// `\\.\pipe\hideout-<user>`: the default hub folder's router pipe (§8.2).
pub fn router_pipe_name() -> String {
    router_pipe_name_for("")
}

/// `\\.\pipe\foxglove-<user>`, served as an alias once `foxglove.exe` no longer owns it.
pub fn alias_pipe_name() -> String {
    alias_pipe_name_for("")
}

/// `\\.\pipe\hideout-<user><suffix>`, `suffix` from [`folder_suffix`]; empty for the default
/// hub folder, whose name stays `\\.\pipe\hideout-<user>`.
pub fn router_pipe_name_for(suffix: &str) -> String {
    format!(r"\\.\pipe\hideout-{}{suffix}", user())
}

/// `\\.\pipe\foxglove-<user><suffix>`: the alias of the hub on that folder.
pub fn alias_pipe_name_for(suffix: &str) -> String {
    format!(r"\\.\pipe\foxglove-{}{suffix}", user())
}

/// The folder part of a hub's names (§8.2, #1010) for any hub folder but the default: `.` and
/// 16 lower-case hex digits, FNV-1a 64 of [`folder_key`]. The default folder has none.
pub fn folder_suffix(root: &Path) -> String {
    format!(".{:016x}", fnv1a(&folder_key(root)))
}

/// One spelling per folder: its canonical path when it exists (else the absolute path), without
/// the `\\?\` prefix, lower case, `/` separators, no trailing `/`.
pub fn folder_key(root: &Path) -> String {
    let full = std::fs::canonicalize(root)
        .or_else(|_| std::path::absolute(root))
        .unwrap_or_else(|_| root.to_path_buf());
    let s = full.to_string_lossy().replace('\\', "/");
    let s = if let Some(unc) = s.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else if let Some(local) = s.strip_prefix("//?/") {
        local.to_string()
    } else {
        s
    };
    s.to_lowercase().trim_end_matches('/').to_string()
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One connection's writer: its queue and how many bytes wait in it (the backlog cap of the
/// network broker, contract §8.10).
#[derive(Clone)]
struct Writer {
    tx: Sender<Option<String>>,
    queued: Arc<AtomicUsize>,
}

impl Writer {
    fn send(&self, line: String) -> bool {
        let n = line.len() + 1;
        self.queued.fetch_add(n, Ordering::Relaxed);
        if self.tx.send(Some(line)).is_err() {
            self.queued.fetch_sub(n, Ordering::Relaxed);
            return false;
        }
        true
    }
}

/// The hub's host services (contract §8.3, §8.10, §8.11, v1.12): what answers
/// [`Out::Service`]. Set with [`Hub::set_services`].
pub trait Services: Send + Sync {
    /// The host-service methods it serves (the router answers the others -32601).
    fn methods(&self) -> Vec<&'static str>;
    /// A call the router already checked. Answer it, now or later, with [`Sink::answer`]. It
    /// runs on the caller's reader thread: anything slow (a connect) goes to a thread of its own.
    fn call(&self, call: ServiceCall, sink: &Sink);
    /// Connection `conn` ended: drop everything it held.
    fn gone(&self, conn: ConnId);
}

/// What a host service sends back through: answers, events to one connection, and hub facts.
/// It holds the hub weakly, so a service never keeps a hub alive.
#[derive(Clone)]
pub struct Sink(Weak<Shared>);

/// What became of an event a host service sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Queued for the connection.
    Sent,
    /// The connection (or the hub) is gone.
    Gone,
    /// More than the backlog cap already waits for the connection: not queued (§8.10).
    Backlogged,
}

impl Sink {
    /// Answers [`Out::Service`] call `call`. False when nobody waited for it any more (the
    /// call timed out, or its connection ended): whatever it opened should be undone.
    pub fn answer(&self, call: u64, result: Result<Value, ServiceError>) -> bool {
        let Some(shared) = self.0.upgrade() else {
            return false;
        };
        let mut waited = false;
        Hub(shared).act(|r| {
            waited = r.waits_for(call);
            r.service_answer(call, result)
        });
        waited
    }

    /// Sends notification `method` to connection `conn` alone, unless more than `backlog`
    /// bytes already wait for it.
    pub fn event(&self, conn: ConnId, method: &str, params: Value, backlog: usize) -> Delivery {
        let Some(shared) = self.0.upgrade() else {
            return Delivery::Gone;
        };
        let Some(w) = shared.writers.lock().unwrap().get(&conn).cloned() else {
            return Delivery::Gone;
        };
        if w.queued.load(Ordering::Relaxed) > backlog {
            return Delivery::Backlogged;
        }
        let line = serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params});
        if w.send(line.to_string()) {
            Delivery::Sent
        } else {
            Delivery::Gone
        }
    }

    /// Sets (or with `null` removes) hub fact `key`, as any hub fact (§8.3).
    pub fn publish(&self, key: &str, value: Value) {
        if let Some(shared) = self.0.upgrade() {
            Hub(shared).act(|r| r.publish(key, value));
        }
    }
}

struct Shared {
    router: Mutex<Router>,
    writers: Mutex<HashMap<ConnId, Writer>>,
    /// The host services (§8.3, v1.12), once set.
    services: Mutex<Option<Arc<dyn Services>>>,
    events: Mutex<Sender<Out>>,
    nudge: Mutex<Sender<()>>,
    /// The hub folder's [`folder_suffix`]; empty for the default folder.
    suffix: Mutex<String>,
    /// Who besides the current user may open the router pipe (contract §7.7).
    pipe_acl: Mutex<PipeAcl>,
}

/// The router pipe's AppContainer guests, and its keeper: the first instance, the only one
/// Windows lets the hub change the pipe's descriptor through. The hub holds the keeper's
/// client end itself, so no module ever lands on it. The pipe's instances share one
/// descriptor: a guest set through the keeper is let in on every instance, the one waiting
/// now included.
#[derive(Default)]
struct PipeAcl {
    guests: Vec<String>,
    /// One per served pipe (the router pipe and its alias).
    #[cfg(windows)]
    keepers: Vec<(foxglove_plugin::channel::ServerPipe, std::fs::File)>,
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
            services: Mutex::new(None),
            events: Mutex::new(events),
            nudge: Mutex::new(nudge),
            suffix: Mutex::new(String::new()),
            pipe_acl: Mutex::new(PipeAcl::default()),
        }));
        let weak = Arc::downgrade(&hub.0);
        std::thread::Builder::new()
            .name("hideout-router-timer".into())
            .spawn(move || timer(weak, wake))
            .expect("router timer thread");
        hub
    }

    /// Names this hub after its folder (#1010): `suffix` from [`folder_suffix`], empty (the
    /// default) for the default folder. Set before serving the pipes and starting modules.
    pub fn set_folder_suffix(&self, suffix: &str) {
        *self.0.suffix.lock().unwrap() = suffix.to_string();
    }

    /// The suffix [`Hub::set_folder_suffix`] set.
    pub fn folder_suffix(&self) -> String {
        self.0.suffix.lock().unwrap().clone()
    }

    /// This hub's router pipe, the one its modules get in `HIDEOUT_PIPE` (§7.5).
    pub fn router_pipe(&self) -> String {
        router_pipe_name_for(&self.folder_suffix())
    }

    /// This hub's `foxglove-<user>` alias.
    pub fn alias_pipe(&self) -> String {
        alias_pipe_name_for(&self.folder_suffix())
    }

    /// The hub's host services (§8.3, v1.12): the router serves the methods they name.
    pub fn set_services(&self, services: Arc<dyn Services>) {
        let methods = services.methods();
        self.0.router.lock().unwrap().serve(&methods);
        *self.0.services.lock().unwrap() = Some(services);
    }

    /// What a host service answers and sends through.
    pub fn sink(&self) -> Sink {
        Sink(Arc::downgrade(&self.0))
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
                    let w = self.0.writers.lock().unwrap().get(&c).cloned();
                    if let Some(w) = w {
                        w.send(v.to_string());
                    }
                }
                Out::Close(c) => {
                    if let Some(w) = self.0.writers.lock().unwrap().remove(&c) {
                        let _ = w.tx.send(None);
                    }
                    let more = self.0.router.lock().unwrap().disconnect(c);
                    self.dispatch(more);
                }
                Out::Service(call) => {
                    let services = self.0.services.lock().unwrap().clone();
                    match services {
                        Some(s) => s.call(call, &self.sink()),
                        None => self.act(|r| {
                            r.service_answer(
                                call.call,
                                Err(ServiceError::new(-32601, "", "method not found")),
                            )
                        }),
                    }
                }
                Out::ServiceGone(conn) => {
                    let services = self.0.services.lock().unwrap().clone();
                    if let Some(s) = services {
                        s.gone(conn);
                    }
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
        writer: impl Write + Send + 'static,
    ) -> ConnId {
        self.attach_peer(origin, None, reader, writer)
    }

    /// [`Hub::attach`] for a peer whose process id the transport knows (the router pipe's
    /// client): the supervisor's launch check at `initialize` compares it (§7.5).
    pub fn attach_peer(
        &self,
        origin: Origin,
        pid: Option<u32>,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
    ) -> ConnId {
        self.attach_in(origin, pid, None, reader, writer)
    }

    /// [`Hub::attach_peer`] for a peer that may run in an AppContainer (`container`, its SID
    /// as its token says, §7.7): recorded before the connection's first line is read.
    pub fn attach_in(
        &self,
        origin: Origin,
        pid: Option<u32>,
        container: Option<&str>,
        reader: impl Read + Send + 'static,
        mut writer: impl Write + Send + 'static,
    ) -> ConnId {
        let conn = {
            let mut r = self.0.router.lock().unwrap();
            let conn = r.connect(origin);
            if let Some(pid) = pid {
                r.set_peer_pid(conn, pid);
            }
            if let Some(sid) = container {
                r.set_peer_container(conn, sid);
            }
            conn
        };
        let (tx, rx) = mpsc::channel::<Option<String>>();
        let queued = Arc::new(AtomicUsize::new(0));
        self.0.writers.lock().unwrap().insert(
            conn,
            Writer {
                tx,
                queued: queued.clone(),
            },
        );
        std::thread::Builder::new()
            .name(format!("hideout-router-w{conn}"))
            .spawn(move || {
                while let Ok(Some(line)) = rx.recv() {
                    let written = writer
                        .write_all(format!("{line}\n").as_bytes())
                        .and_then(|_| writer.flush());
                    queued.fetch_sub(line.len() + 1, Ordering::Relaxed);
                    if written.is_err() {
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
    /// stdout on the router as module `id`. The caller (the supervisor) owns the child and sets
    /// its stderr (the module's log, §7.7); it is left as the caller set it.
    pub fn spawn_stdio(&self, id: &str, mut command: Command) -> io::Result<Child> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        self.attach_peer(Origin::Bridged(id.into()), Some(child.id()), stdout, stdin);
        Ok(child)
    }

    /// Serves the router pipe `name` (§8.2): current-user ACL, remote clients rejected. The
    /// first instance is made here, so a name another process owns fails now.
    #[cfg(windows)]
    pub fn serve_pipe(&self, name: &str) -> io::Result<std::thread::JoinHandle<()>> {
        use foxglove_plugin::channel::ServerPipe;
        {
            // The keeper takes the name first (a squatter makes this fail), then the hub
            // connects to it itself.
            let mut acl = self.0.pipe_acl.lock().unwrap();
            let keeper = ServerPipe::create_keeper(name, &acl.guests)?;
            let client = foxglove_plugin::hideout::open_pipe(name)?;
            acl.keepers.push((keeper, client));
        }
        let first = ServerPipe::create(name, false)?;
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
                    let handle = pipe.raw_handle();
                    match pipe.accept() {
                        Ok(conn) => {
                            // The handle stays open while `conn` lives.
                            let pid = client_pid(handle);
                            let peer = match hub.admit(pid) {
                                Ok(p) => p,
                                Err(why) => {
                                    // Dropping the connection closes it.
                                    log::warn!("router: {name}: client refused: {why}");
                                    continue;
                                }
                            };
                            let sid = peer.app_container.as_deref();
                            hub.attach_in(Origin::Pipe, pid, sid, conn.clone(), conn);
                        }
                        Err(e) => log::warn!("router: {name}: accept: {e}"),
                    }
                }
            })
    }
}

#[cfg(windows)]
impl Hub {
    /// Lets the AppContainer `sid` open the router pipe (contract §7.7), on every instance.
    /// Call before starting a contained module. Before [`Hub::serve_pipe`] it is kept for the
    /// pipe's first descriptor.
    pub fn allow_pipe_client(&self, sid: &str) -> io::Result<()> {
        let mut acl = self.0.pipe_acl.lock().unwrap();
        if acl.guests.iter().any(|g| g == sid) {
            return Ok(());
        }
        let mut guests = acl.guests.clone();
        guests.push(sid.to_string());
        let sd = hideout_acl::Descriptor::current_user_and(&guests)?;
        for (keeper, _) in &acl.keepers {
            hideout_acl::apply_to_handle(keeper.raw_handle(), &sd)?;
        }
        acl.guests = guests;
        Ok(())
    }

    /// Takes the AppContainer `sid` off the router pipe (the module was removed, §7.7).
    pub fn disallow_pipe_client(&self, sid: &str) -> io::Result<()> {
        let mut acl = self.0.pipe_acl.lock().unwrap();
        if !acl.guests.iter().any(|g| g == sid) {
            return Ok(());
        }
        let guests: Vec<String> = acl.guests.iter().filter(|g| *g != sid).cloned().collect();
        let sd = hideout_acl::Descriptor::current_user_and(&guests)?;
        for (keeper, _) in &acl.keepers {
            hideout_acl::apply_to_handle(keeper.raw_handle(), &sd)?;
        }
        acl.guests = guests;
        Ok(())
    }

    /// Whether a router pipe client with process id `pid` is let in (contract §2.3, §7.7).
    /// While the pipe has AppContainer guests it carries a Low label, so a Low-integrity
    /// process could open it: below Medium only the hub's own containers are admitted; the
    /// user's Medium processes (modules, outside tools) are as before. A container that isn't
    /// (or no longer is) a guest is refused either way, and so is a client whose process or
    /// token can't be read (an elevated tool the hub can't look into, a process already gone).
    /// Admitted, the peer as its token says (its AppContainer SID goes on its connection).
    pub fn admit(&self, pid: Option<u32>) -> Result<hideout_acl::Peer, String> {
        let guests = self.pipe_guests();
        // A client the router can't name is refused, guests or not (#1089 re-check).
        let pid = pid.ok_or("its process is unknown")?;
        let peer = hideout_acl::peer(pid).map_err(|e| format!("pid {pid}: token not read: {e}"))?;
        if hideout_acl::admits(&peer, &guests) {
            Ok(peer)
        } else {
            Err(format!(
                "pid {pid} runs at integrity {:#x}{} and isn't one of the hub's AppContainers",
                peer.integrity,
                peer.app_container
                    .as_deref()
                    .map_or(String::new(), |s| format!(" in {s}"))
            ))
        }
    }

    /// The AppContainer SIDs that may open the router pipe.
    pub fn pipe_guests(&self) -> Vec<String> {
        self.0.pipe_acl.lock().unwrap().guests.clone()
    }
}

/// The process id of a connected router pipe's client.
#[cfg(windows)]
fn client_pid(handle: std::os::windows::io::RawHandle) -> Option<u32> {
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    let mut pid = 0u32;
    // SAFETY: `handle` is a connected pipe instance kept open by its `PipeConn`.
    let ok = unsafe { GetNamedPipeClientProcessId(handle as _, &mut pid) };
    (ok != 0 && pid != 0).then_some(pid)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;
    use std::path::PathBuf;

    fn hub() -> Hub {
        let (events, _rx) = mpsc::channel();
        Hub::new(
            Router::new(Settings::default(), Box::new(|_, a| a.to_vec())),
            events,
        )
    }

    #[test]
    fn the_default_folder_keeps_the_old_names() {
        let user = user();
        assert_eq!(router_pipe_name(), format!(r"\\.\pipe\hideout-{user}"));
        assert_eq!(alias_pipe_name(), format!(r"\\.\pipe\foxglove-{user}"));
        let h = hub();
        assert_eq!(h.folder_suffix(), "");
        assert_eq!(h.router_pipe(), router_pipe_name());
        assert_eq!(h.alias_pipe(), alias_pipe_name());
    }

    #[test]
    fn two_hubs_on_two_folders_get_two_pipe_names() {
        let (a, b) = (hub(), hub());
        a.set_folder_suffix(&folder_suffix(Path::new(r"C:\Lab\One")));
        b.set_folder_suffix(&folder_suffix(Path::new(r"C:\Lab\Two")));
        assert_ne!(a.router_pipe(), b.router_pipe());
        assert_ne!(a.alias_pipe(), b.alias_pipe());
        assert_ne!(a.router_pipe(), router_pipe_name());
        assert!(a.router_pipe().starts_with(&router_pipe_name()));
        let s = a.folder_suffix();
        assert_eq!(s.len(), 17);
        assert!(s.starts_with('.') && s[1..].bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn one_folder_one_suffix_however_it_is_spelled() {
        assert_eq!(
            folder_suffix(Path::new(r"C:\Lab\Cfg\")),
            folder_suffix(Path::new("c:/lab/cfg"))
        );
        // An existing folder hashes by its canonical path, with or without `\\?\`.
        let dir = std::env::temp_dir().join(format!("hideout-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let canon = std::fs::canonicalize(&dir).unwrap();
        let upper = PathBuf::from(dir.to_string_lossy().to_uppercase() + "\\");
        assert_eq!(folder_key(&canon), folder_key(&dir));
        assert_eq!(folder_key(&upper), folder_key(&dir));
        assert!(!folder_key(&dir).starts_with("//?/"));
        let _ = std::fs::remove_dir(&dir);
    }
}
