// SPDX-License-Identifier: MIT
//! The network broker (contract §8.10, v1.12): the hub opens the connections a caller's grant
//! names (the router already checked it) and carries the bytes over the caller's own
//! connection as base64 (`net.send` in, `net.data` out). A contained module has no network of
//! its own; this is how it reaches exactly what it was granted.
//!
//! Every stream belongs to one router connection, with ids from 1 counted per connection. Each
//! open stream has one thread blocked on the OS (a read, an accept or a receive): data or a
//! close wakes it, nothing runs on a timer. Closing wakes a blocked reader by shutting the
//! socket down, a listener by connecting to it once, and a UDP receiver with one empty
//! datagram. When a connection ends, everything it held ends with it.
//!
//! The shape follows `foxglove-plugin-host`'s broker (MIT, this repository), re-keyed per
//! connection and with the hub-wide caps.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hideout_contract::services::{
    error_id, NetAccepted, NetClosed, NetData, NetKind, NetOpen, NetSend, NetStream,
    NET_BACKLOG_BYTES, NET_DATAGRAM, NET_READ_CHUNK, NET_STREAMS_PER_CONNECTION,
    NET_STREAMS_PER_HUB, NET_TIMEOUT_S,
};
use hideout_router::server::{Delivery, Sink};
use hideout_router::{ConnId, ServiceCall, ServiceError};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(NET_TIMEOUT_S);

/// The broker's caps (§8.10); tests lower them.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub per_connection: usize,
    pub per_hub: usize,
    pub backlog_bytes: usize,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps {
            per_connection: NET_STREAMS_PER_CONNECTION,
            per_hub: NET_STREAMS_PER_HUB,
            backlog_bytes: NET_BACKLOG_BYTES,
        }
    }
}

/// What an open stream is, for writing and for closing.
enum Handle {
    Tcp(Arc<TcpStream>),
    /// The receiving socket's address: closing sends it one empty datagram.
    Udp(SocketAddr),
    /// The listener's address: closing connects to it once to wake the blocked accept.
    Listener(SocketAddr),
}

enum Slot {
    /// `net.open` is still connecting or binding (it counts toward the caps).
    Opening,
    Open {
        handle: Handle,
        stop: Arc<AtomicBool>,
    },
}

#[derive(Default)]
struct Streams {
    next: u64,
    slots: HashMap<u64, Slot>,
}

#[derive(Default)]
struct State {
    conns: HashMap<ConnId, Streams>,
    /// Connections that ended: an open finishing after its connection is gone is dropped.
    gone: HashSet<ConnId>,
    total: usize,
}

impl State {
    fn remove(&mut self, conn: ConnId, id: u64) -> Option<Slot> {
        let slot = self.conns.get_mut(&conn)?.slots.remove(&id)?;
        self.total -= 1;
        Some(slot)
    }

    /// Takes a stream id for `conn`, within the caps.
    fn reserve(&mut self, conn: ConnId, caps: &Caps) -> Result<u64, ServiceError> {
        if self.gone.contains(&conn) {
            return Err(ServiceError::new(
                2,
                error_id::NET_IO,
                "the connection ended",
            ));
        }
        let streams = self.conns.entry(conn).or_default();
        if streams.slots.len() >= caps.per_connection {
            return Err(ServiceError::new(
                8,
                error_id::NET_LIMIT,
                format!(
                    "at most {} network streams per connection",
                    caps.per_connection
                ),
            ));
        }
        if self.total >= caps.per_hub {
            return Err(ServiceError::new(
                8,
                error_id::NET_LIMIT,
                format!("at most {} network streams in the hub", caps.per_hub),
            ));
        }
        streams.next += 1;
        let id = streams.next;
        streams.slots.insert(id, Slot::Opening);
        self.total += 1;
        Ok(id)
    }
}

/// Wakes whatever blocks on a stream, once its `stop` flag is set.
fn wake(handle: Handle) {
    match handle {
        Handle::Tcp(s) => {
            let _ = s.shutdown(Shutdown::Both);
        }
        Handle::Udp(local) => {
            if let Ok(waker) = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)) {
                let _ = waker.send_to(&[], local);
            }
        }
        Handle::Listener(local) => {
            let _ = TcpStream::connect_timeout(&local, TIMEOUT);
        }
    }
}

fn close_slot(slot: Slot) {
    if let Slot::Open { handle, stop } = slot {
        stop.store(true, Ordering::SeqCst);
        wake(handle);
    }
}

/// The network broker. Cheap to clone; clones share everything.
#[derive(Clone, Default)]
pub struct Broker {
    state: Arc<Mutex<State>>,
    caps: Caps,
}

fn io(e: impl std::fmt::Display) -> ServiceError {
    ServiceError::new(6, error_id::NET_IO, e.to_string())
}

impl Broker {
    pub fn new(caps: Caps) -> Broker {
        Broker {
            state: Arc::default(),
            caps,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Open streams in the whole hub (opens still connecting included).
    pub fn open_streams(&self) -> usize {
        self.lock().total
    }

    /// Open streams of one connection.
    pub fn streams_of(&self, conn: ConnId) -> usize {
        self.lock().conns.get(&conn).map_or(0, |s| s.slots.len())
    }

    /// One checked call (§8.10); answered through `sink`, now or from the open's thread.
    pub fn call(&self, call: ServiceCall, sink: &Sink) {
        let answer = match call.method.as_str() {
            "net.open" => match serde_json::from_value::<NetOpen>(call.params.clone()) {
                Ok(p) => return self.open(call.conn, call.call, p, sink.clone()),
                Err(e) => Err(ServiceError::argument(
                    error_id::NET_ARGUMENT,
                    e.to_string(),
                )),
            },
            "net.send" => match serde_json::from_value::<NetSend>(call.params.clone()) {
                Ok(p) => self.send(call.conn, p, sink),
                Err(e) => Err(ServiceError::argument(
                    error_id::NET_ARGUMENT,
                    e.to_string(),
                )),
            },
            "net.close" => match serde_json::from_value::<NetStream>(call.params.clone()) {
                Ok(p) => {
                    self.close(call.conn, p.stream);
                    Ok(json!({}))
                }
                Err(e) => Err(ServiceError::argument(
                    error_id::NET_ARGUMENT,
                    e.to_string(),
                )),
            },
            m => Err(ServiceError::new(
                -32601,
                "",
                format!("{m}: not a network method"),
            )),
        };
        sink.answer(call.call, answer);
    }

    /// Connection `conn` ended: every stream it held ends, with no event (§8.10).
    pub fn gone(&self, conn: ConnId) {
        let slots: Vec<Slot> = {
            let mut st = self.lock();
            st.gone.insert(conn);
            let Some(streams) = st.conns.remove(&conn) else {
                return;
            };
            st.total -= streams.slots.len();
            streams.slots.into_values().collect()
        };
        if !slots.is_empty() {
            log::info!(
                "net: connection {conn} ended; {} stream(s) closed",
                slots.len()
            );
        }
        slots.into_iter().for_each(close_slot);
    }

    fn open(&self, conn: ConnId, call: u64, p: NetOpen, sink: Sink) {
        let id = match self.lock().reserve(conn, &self.caps) {
            Ok(id) => id,
            Err(e) => return sink.answer(call, Err(e)),
        };
        let broker = self.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("hideout-net-open-{conn}-{id}"))
            .spawn(move || {
                let opened = connect(&p);
                broker.opened(conn, id, call, opened, sink);
            });
        if let Err(e) = spawned {
            self.lock().remove(conn, id);
            log::warn!("net: no thread for an open: {e}");
        }
    }

    /// An open finished: the stream goes live (and is answered) or its slot is freed.
    fn opened(
        &self,
        conn: ConnId,
        id: u64,
        call: u64,
        opened: Result<Opened, ServiceError>,
        sink: Sink,
    ) {
        let opened = match opened {
            Ok(o) => o,
            Err(e) => {
                self.lock().remove(conn, id);
                return sink.answer(call, Err(e));
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (handle, run): (Handle, Box<dyn FnOnce() + Send>) = match opened {
            Opened::Tcp(s) => {
                let s = Arc::new(s);
                let (b, sink, stop, reader) = (self.clone(), sink.clone(), stop.clone(), s.clone());
                (
                    Handle::Tcp(s),
                    Box::new(move || b.read_tcp(conn, id, reader, sink, stop)),
                )
            }
            Opened::Udp(sock, local) => {
                let (b, sink, stop) = (self.clone(), sink.clone(), stop.clone());
                (
                    Handle::Udp(local),
                    Box::new(move || b.read_udp(conn, id, sock, sink, stop)),
                )
            }
            Opened::Listener(l, local) => {
                let (b, sink, stop) = (self.clone(), sink.clone(), stop.clone());
                (
                    Handle::Listener(local),
                    Box::new(move || b.accept(conn, id, l, sink, stop)),
                )
            }
        };
        {
            let mut st = self.lock();
            match st.conns.get_mut(&conn).and_then(|s| s.slots.get_mut(&id)) {
                Some(slot) => {
                    *slot = Slot::Open {
                        handle,
                        stop: stop.clone(),
                    }
                }
                // Closed or its connection ended while it opened: dropped, nothing sent.
                None => {
                    drop(st);
                    stop.store(true, Ordering::SeqCst);
                    wake(handle);
                    return;
                }
            }
        }
        // The answer goes first, then what arrives (a client keeps early events anyway).
        sink.answer(call, Ok(json!(NetStream { stream: id })));
        let spawned = std::thread::Builder::new()
            .name(format!("hideout-net-{conn}-{id}"))
            .spawn(run);
        if spawned.is_err() {
            self.end(conn, id, &sink, &stop, "the hub couldn't start a reader");
        }
    }

    /// Ends a stream the broker gave up on: removed, woken and, unless its holder closed it,
    /// told with `net.closed` once.
    fn end(&self, conn: ConnId, id: u64, sink: &Sink, stop: &AtomicBool, reason: &str) {
        let slot = self.lock().remove(conn, id);
        // `stop` set: the holder closed it (or its connection ended), nothing is said.
        if stop.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(slot) = slot {
            if let Slot::Open { handle, .. } = slot {
                wake(handle);
            }
            let closed = json!(NetClosed {
                stream: id,
                reason: reason.into()
            });
            sink.event(conn, "net.closed", closed, usize::MAX);
        }
    }

    /// What the stream's holder gets; false once the stream ends.
    fn deliver(
        &self,
        conn: ConnId,
        id: u64,
        sink: &Sink,
        stop: &AtomicBool,
        method: &str,
        params: Value,
    ) -> bool {
        match sink.event(conn, method, params, self.caps.backlog_bytes) {
            Delivery::Sent => true,
            Delivery::Gone => {
                self.lock().remove(conn, id);
                false
            }
            Delivery::Backlogged => {
                log::warn!("net: connection {conn} isn't reading; stream {id} closed");
                self.end(conn, id, sink, stop, "the module isn't reading");
                false
            }
        }
    }

    fn read_tcp(
        &self,
        conn: ConnId,
        id: u64,
        s: Arc<TcpStream>,
        sink: Sink,
        stop: Arc<AtomicBool>,
    ) {
        let mut buf = vec![0u8; NET_READ_CHUNK];
        let reason = loop {
            match (&*s).read(&mut buf) {
                Ok(0) => break "closed by the other side".to_string(),
                Ok(n) => {
                    let data = json!(NetData {
                        stream: id,
                        data: STANDARD.encode(&buf[..n]),
                        from: None
                    });
                    if !self.deliver(conn, id, &sink, &stop, "net.data", data) {
                        return;
                    }
                }
                Err(e) => break e.to_string(),
            }
        };
        self.end(conn, id, &sink, &stop, &reason);
    }

    fn read_udp(&self, conn: ConnId, id: u64, sock: UdpSocket, sink: Sink, stop: Arc<AtomicBool>) {
        let mut buf = vec![0u8; NET_DATAGRAM];
        // Blocks until a datagram arrives; a close wakes it with an empty one after `stop`.
        loop {
            match sock.recv_from(&mut buf) {
                _ if stop.load(Ordering::SeqCst) => return,
                Ok((n, from)) => {
                    let data = json!(NetData {
                        stream: id,
                        data: STANDARD.encode(&buf[..n]),
                        from: Some(from.to_string())
                    });
                    if !self.deliver(conn, id, &sink, &stop, "net.data", data) {
                        return;
                    }
                }
                // Windows reports an earlier send's ICMP error on the next receive.
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                Err(e) => return self.end(conn, id, &sink, &stop, &e.to_string()),
            }
        }
    }

    /// Accepts until the listener's stream closes; each connection is a stream of its own,
    /// counted toward the caps (one over them is closed at once, unannounced).
    fn accept(
        &self,
        conn: ConnId,
        listener_id: u64,
        l: TcpListener,
        sink: Sink,
        stop: Arc<AtomicBool>,
    ) {
        for accepted in l.incoming() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let s = match accepted {
                Ok(s) => s,
                // One connection that went away before it was taken: the next one may be fine.
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    continue
                }
                // Anything else would fail again at once: the listener ends (never a spin).
                Err(e) => return self.end(conn, listener_id, &sink, &stop, &e.to_string()),
            };
            let from = s.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let _ = s.set_write_timeout(Some(TIMEOUT));
            let s = Arc::new(s);
            let s_stop = Arc::new(AtomicBool::new(false));
            let id = {
                let mut st = self.lock();
                match st.reserve(conn, &self.caps) {
                    Ok(id) => {
                        if let Some(slot) =
                            st.conns.get_mut(&conn).and_then(|c| c.slots.get_mut(&id))
                        {
                            *slot = Slot::Open {
                                handle: Handle::Tcp(s.clone()),
                                stop: s_stop.clone(),
                            };
                        }
                        id
                    }
                    Err(e) => {
                        log::info!(
                            "net: connection {conn}: an accepted connection refused: {}",
                            e.reason
                        );
                        let _ = s.shutdown(Shutdown::Both);
                        continue;
                    }
                }
            };
            let accepted = json!(NetAccepted {
                listener: listener_id,
                stream: id,
                from
            });
            if !self.deliver(conn, id, &sink, &s_stop, "net.accepted", accepted) {
                let _ = s.shutdown(Shutdown::Both);
                continue;
            }
            let (b, sink2) = (self.clone(), sink.clone());
            let spawned = std::thread::Builder::new()
                .name(format!("hideout-net-{conn}-{id}"))
                .spawn(move || b.read_tcp(conn, id, s, sink2, s_stop.clone()));
            if spawned.is_err() {
                self.lock().remove(conn, id);
            }
        }
    }

    fn send(&self, conn: ConnId, p: NetSend, sink: &Sink) -> Result<Value, ServiceError> {
        let bytes = STANDARD.decode(&p.data).map_err(|e| {
            ServiceError::argument(error_id::NET_ARGUMENT, format!("bad base64: {e}"))
        })?;
        let s = {
            let st = self.lock();
            match st.conns.get(&conn).and_then(|c| c.slots.get(&p.stream)) {
                Some(Slot::Open {
                    handle: Handle::Tcp(s),
                    stop,
                }) => (s.clone(), stop.clone()),
                Some(Slot::Open {
                    handle: Handle::Udp(_),
                    ..
                }) => {
                    return Err(ServiceError::argument(
                        error_id::NET_STREAM,
                        "a udp stream only receives",
                    ))
                }
                Some(Slot::Open {
                    handle: Handle::Listener(_),
                    ..
                }) => {
                    return Err(ServiceError::argument(
                        error_id::NET_STREAM,
                        "send to an accepted stream, not the listener",
                    ))
                }
                Some(Slot::Opening) => {
                    return Err(ServiceError::argument(
                        error_id::NET_STREAM,
                        "the stream isn't open yet",
                    ))
                }
                None => {
                    return Err(ServiceError::argument(
                        error_id::NET_STREAM,
                        format!("no stream {} on this connection", p.stream),
                    ))
                }
            }
        };
        // Outside the lock: a write may wait for the other side (10 s at most).
        match (&*s.0).write_all(&bytes) {
            Ok(()) => Ok(json!({})),
            Err(e) => {
                let reason = format!("write failed: {e}");
                self.end(conn, p.stream, sink, &s.1, &reason);
                Err(io(reason))
            }
        }
    }

    /// `net.close`: ends the stream, with no `net.closed`; a stream already gone is fine.
    pub fn close(&self, conn: ConnId, id: u64) {
        let slot = self.lock().remove(conn, id);
        if let Some(slot) = slot {
            close_slot(slot);
        }
    }
}

enum Opened {
    Tcp(TcpStream),
    Udp(UdpSocket, SocketAddr),
    Listener(TcpListener, SocketAddr),
}

/// Connects or binds what `p` asks for (already permitted and checked by the router).
fn connect(p: &NetOpen) -> Result<Opened, ServiceError> {
    let at = |e: std::io::Error| io(format!("{}:{}: {e}", p.host, p.port));
    match p.kind {
        NetKind::Tcp => {
            let addrs = (p.host.as_str(), p.port).to_socket_addrs().map_err(at)?;
            let mut last = std::io::Error::other("no address");
            for a in addrs {
                match TcpStream::connect_timeout(&a, TIMEOUT) {
                    Ok(s) => {
                        let _ = s.set_write_timeout(Some(TIMEOUT));
                        let _ = s.set_nodelay(true);
                        return Ok(Opened::Tcp(s));
                    }
                    Err(e) => last = e,
                }
            }
            Err(at(last))
        }
        NetKind::Udp => {
            let s = UdpSocket::bind((Ipv4Addr::LOCALHOST, p.port)).map_err(at)?;
            let local = s.local_addr().map_err(at)?;
            Ok(Opened::Udp(s, local))
        }
        NetKind::TcpListen => {
            let l = TcpListener::bind((Ipv4Addr::LOCALHOST, p.port)).map_err(at)?;
            let local = l.local_addr().map_err(at)?;
            Ok(Opened::Listener(l, local))
        }
    }
}
