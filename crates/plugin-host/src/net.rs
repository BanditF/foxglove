//! The network broker (#214): plugins with scoped network permissions stay in the sandbox,
//! which has no network at all, and Foxglove opens the connections their permissions name.
//! Bytes go both ways as base64 over the plugin's own connection (`net.send`, `net.data`).
//!
//! Windows can't limit a sandboxed process to particular hosts without admin-only, machine-wide
//! settings, and the sandbox blocks loopback outright, so the host does it instead.
//!
//! A plugin can also accept connections on one loopback port (`tcp-listen`, #298): each arrives
//! as `net.accepted` with a stream of its own. Every reader blocks until something arrives;
//! nothing polls.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use foxglove_plugin_api::methods::{NetAccepted, NetClosed, NetData, NetKind, NetOpenParams};
use foxglove_plugin_api::permission::{Grant, Permission};
use foxglove_plugin_api::rpc::{code, Error, Request};
use serde_json::json;

/// Open streams per connection.
const MAX_STREAMS: usize = 16;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

enum Conn {
    Tcp(TcpStream),
    /// The receiving socket's address: closing sends it one empty datagram to wake the reader,
    /// which blocks in `recv_from` with no timeout (#282).
    Udp(SocketAddr),
    /// A listener's address: closing connects to it once to wake the blocked `accept`.
    Listener(SocketAddr),
}

struct Open {
    conn: Conn,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct Net {
    streams: Arc<Mutex<HashMap<u64, Open>>>,
    next: Arc<AtomicU64>,
}

fn error(code: i32, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
        data: None,
    }
}

/// Whether `grant` lets the plugin open this.
pub(crate) fn allowed(grant: &Grant, p: &NetOpenParams) -> bool {
    if grant.has(&Permission::Net) {
        return true;
    }
    match p.kind {
        NetKind::Tcp => grant.can_connect(&p.host, p.port),
        NetKind::Udp => p.host == "127.0.0.1" && grant.can_listen_udp(p.port),
        NetKind::TcpListen => p.host == "127.0.0.1" && grant.can_listen_tcp(p.port),
    }
}

fn event(out: &Sender<String>, method: &str, params: serde_json::Value) -> bool {
    out.send(serde_json::to_string(&Request::notification(method, params)).unwrap())
        .is_ok()
}

impl Net {
    /// Opens a stream (already permitted) and starts forwarding what arrives on it.
    pub(crate) fn open(&self, p: &NetOpenParams, out: Sender<String>) -> Result<u64, Error> {
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        if streams.len() >= MAX_STREAMS {
            return Err(error(code::RATE_LIMITED, "too many open network streams"));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let stop = Arc::new(AtomicBool::new(false));
        let failed =
            |e: std::io::Error| error(code::INVALID_PARAMS, format!("{}:{}: {e}", p.host, p.port));
        let conn = match p.kind {
            NetKind::Tcp => {
                let addrs = (p.host.as_str(), p.port)
                    .to_socket_addrs()
                    .map_err(failed)?;
                let mut last = std::io::Error::other("no address");
                let stream = addrs
                    .into_iter()
                    .find_map(|a| {
                        TcpStream::connect_timeout(&a, CONNECT_TIMEOUT)
                            .map_err(|e| last = e)
                            .ok()
                    })
                    .ok_or_else(|| failed(last))?;
                let reader = stream.try_clone().map_err(failed)?;
                forward_tcp(self.streams.clone(), id, reader, out, stop.clone());
                Conn::Tcp(stream)
            }
            NetKind::TcpListen => {
                let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, p.port)).map_err(failed)?;
                let local = listener.local_addr().map_err(failed)?;
                self.accept(id, listener, out, stop.clone());
                Conn::Listener(local)
            }
            NetKind::Udp => {
                let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, p.port)).map_err(failed)?;
                let local = socket.local_addr().map_err(failed)?;
                self.forward_udp(id, socket, out, stop.clone());
                Conn::Udp(local)
            }
        };
        streams.insert(id, Open { conn, stop });
        Ok(id)
    }

    /// Accepts connections until the listener's stream closes; each gets its own stream and
    /// counts toward the limit (a connection over it is refused).
    fn accept(
        &self,
        listener_id: u64,
        listener: TcpListener,
        out: Sender<String>,
        stop: Arc<AtomicBool>,
    ) {
        let (streams, next) = (self.streams.clone(), self.next.clone());
        let _ = std::thread::Builder::new()
            .name(format!("plugin-net-{listener_id}-accept"))
            .spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    let from = conn.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                    let mut open = streams.lock().unwrap_or_else(|e| e.into_inner());
                    let reader = match conn.try_clone() {
                        Ok(r) if open.len() < MAX_STREAMS => r,
                        _ => {
                            let _ = conn.shutdown(Shutdown::Both);
                            continue;
                        }
                    };
                    let id = next.fetch_add(1, Ordering::Relaxed) + 1;
                    let conn_stop = Arc::new(AtomicBool::new(false));
                    open.insert(
                        id,
                        Open {
                            conn: Conn::Tcp(conn),
                            stop: conn_stop.clone(),
                        },
                    );
                    drop(open);
                    // `net.accepted` goes out before any of the connection's data.
                    if !event(
                        &out,
                        "net.accepted",
                        json!(NetAccepted {
                            listener: listener_id,
                            stream: id,
                            from
                        }),
                    ) {
                        break;
                    }
                    forward_tcp(streams.clone(), id, reader, out.clone(), conn_stop);
                }
                streams
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&listener_id);
            });
    }
}

/// Forwards what arrives on a TCP stream until it ends, then says so (unless the plugin closed
/// it).
fn forward_tcp(
    streams: Arc<Mutex<HashMap<u64, Open>>>,
    id: u64,
    mut reader: TcpStream,
    out: Sender<String>,
    stop: Arc<AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name(format!("plugin-net-{id}"))
        .spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            let reason = loop {
                match reader.read(&mut buf) {
                    Ok(0) => break "closed by the other side".to_string(),
                    Ok(n) => {
                        let data = json!(NetData {
                            stream: id,
                            data: STANDARD.encode(&buf[..n]),
                            from: None
                        });
                        if !event(&out, "net.data", data) {
                            break "the plugin went away".into();
                        }
                    }
                    Err(e) => break e.to_string(),
                }
            };
            streams
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            if !stop.load(Ordering::Relaxed) {
                event(&out, "net.closed", json!(NetClosed { stream: id, reason }));
            }
        });
}

impl Net {
    fn forward_udp(&self, id: u64, socket: UdpSocket, out: Sender<String>, stop: Arc<AtomicBool>) {
        let streams = self.streams.clone();
        let _ = std::thread::Builder::new()
            .name(format!("plugin-net-{id}"))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                // Blocks until a datagram arrives; `close` wakes it with an empty one after setting
                // `stop`, so an idle stream costs no wakeups.
                while !stop.load(Ordering::Relaxed) {
                    match socket.recv_from(&mut buf) {
                        Ok(_) if stop.load(Ordering::Relaxed) => break,
                        Ok((n, from)) => {
                            let data = json!(NetData {
                                stream: id,
                                data: STANDARD.encode(&buf[..n]),
                                from: Some(from.to_string())
                            });
                            if !event(&out, "net.data", data) {
                                break;
                            }
                        }
                        // Windows reports an earlier send's ICMP error on the next receive; keep going.
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                        Err(_) => break,
                    }
                }
                streams
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
            });
    }

    pub(crate) fn send(&self, id: u64, data: &str) -> Result<(), Error> {
        let bytes = STANDARD
            .decode(data)
            .map_err(|e| error(code::INVALID_PARAMS, format!("bad base64: {e}")))?;
        let streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        match streams.get(&id).map(|o| &o.conn) {
            Some(Conn::Tcp(s)) => {
                let mut s = s;
                s.write_all(&bytes)
                    .map_err(|e| error(code::INVALID_PARAMS, e.to_string()))
            }
            Some(Conn::Udp(_)) => Err(error(code::INVALID_PARAMS, "a udp stream only receives")),
            Some(Conn::Listener(_)) => Err(error(
                code::INVALID_PARAMS,
                "send to an accepted stream, not the listener",
            )),
            None => Err(error(code::INVALID_PARAMS, format!("no stream {id}"))),
        }
    }

    pub(crate) fn close(&self, id: u64) {
        if let Some(o) = self
            .streams
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
        {
            o.stop.store(true, Ordering::Relaxed);
            match o.conn {
                Conn::Tcp(s) => {
                    let _ = s.shutdown(Shutdown::Both);
                }
                Conn::Udp(local) => {
                    // The reader sees `stop` once this empty datagram wakes it.
                    if let Ok(waker) = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)) {
                        let _ = waker.send_to(&[], local);
                    }
                }
                Conn::Listener(local) => {
                    // `accept` sees `stop` once this connection wakes it.
                    let _ = TcpStream::connect_timeout(&local, CONNECT_TIMEOUT);
                }
            }
        }
    }

    /// Closes everything when the plugin's connection ends.
    pub(crate) fn close_all(&self) {
        let ids: Vec<u64> = self
            .streams
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect();
        for id in ids {
            self.close(id);
        }
    }
}
