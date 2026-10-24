//! Network through Foxglove (API 1.2, #214): the plugin stays sandboxed and Foxglove opens the
//! connections its permissions name (`net.connect:<host>:<port>`,
//! `net.listen:udp:127.0.0.1:<port>`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use foxglove_plugin_api::methods::{NetClosed, NetData, NetKind, NetOpenParams, NetStream as NetStreamId};
use serde_json::{json, Value};

use crate::{Plugin, PluginError, Result};

/// What arrives on a stream.
#[derive(Clone, Debug, PartialEq)]
pub enum NetEvent {
    /// Bytes, and for UDP who sent them.
    Data(Vec<u8>, Option<String>),
    /// The stream ended.
    Closed(String),
}

enum Route {
    /// Events that came before `net.open` returned the stream's id.
    Early(Vec<NetEvent>),
    Open(Sender<NetEvent>),
}

/// Routes `net.data` / `net.closed` events to their streams.
#[derive(Clone, Default)]
pub(crate) struct Routes(Arc<Mutex<HashMap<u64, Route>>>);

impl Routes {
    fn deliver(&self, stream: u64, event: NetEvent) {
        let mut routes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match routes.entry(stream).or_insert_with(|| Route::Early(vec![])) {
            Route::Early(events) => events.push(event),
            Route::Open(tx) => {
                if tx.send(event).is_err() {
                    routes.remove(&stream);
                }
            }
        }
    }

    /// Handles a notification; false if it isn't a network one.
    pub(crate) fn handle(&self, method: &str, params: Value) -> bool {
        match method {
            "net.data" => {
                if let Ok(d) = serde_json::from_value::<NetData>(params) {
                    let bytes = STANDARD.decode(d.data).unwrap_or_default();
                    self.deliver(d.stream, NetEvent::Data(bytes, d.from));
                }
                true
            }
            "net.closed" => {
                if let Ok(c) = serde_json::from_value::<NetClosed>(params) {
                    self.deliver(c.stream, NetEvent::Closed(c.reason));
                }
                true
            }
            _ => false,
        }
    }

    /// Ends every stream (Foxglove went away): each gets `Closed`.
    pub(crate) fn close_all(&self, why: &str) {
        let mut routes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for (_, route) in routes.drain() {
            if let Route::Open(tx) = route {
                let _ = tx.send(NetEvent::Closed(why.to_string()));
            }
        }
    }

    fn register(&self, stream: u64) -> Receiver<NetEvent> {
        let (tx, rx) = mpsc::channel();
        let mut routes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Route::Early(events)) = routes.remove(&stream) {
            for e in events {
                let _ = tx.send(e);
            }
        }
        routes.insert(stream, Route::Open(tx));
        rx
    }
}

impl Plugin {
    /// Opens a stream through Foxglove; what arrives comes on the receiver.
    pub fn net_open(&self, kind: NetKind, host: &str, port: u16) -> Result<(u64, Receiver<NetEvent>)> {
        let p = NetOpenParams { kind, host: host.into(), port };
        let id: NetStreamId = self.call_as("net.open", serde_json::to_value(p).expect("serializable"))?;
        Ok((id.stream, self.routes.register(id.stream)))
    }

    pub fn net_send(&self, stream: u64, bytes: &[u8]) -> Result<()> {
        self.call("net.send", json!({"stream": stream, "data": STANDARD.encode(bytes)})).map(|_| ())
    }

    pub fn net_close(&self, stream: u64) -> Result<()> {
        self.call("net.close", json!({"stream": stream})).map(|_| ())
    }

    /// Datagrams sent to 127.0.0.1:`port` (needs `net.listen:udp:127.0.0.1:<port>`).
    pub fn listen_udp(&self, port: u16) -> Result<Receiver<NetEvent>> {
        self.net_open(NetKind::Udp, "127.0.0.1", port).map(|(_, rx)| rx)
    }

    /// A TCP connection as a byte stream (needs `net.connect:<host>:<port>`): wrap it in TLS
    /// or a WebSocket like any other stream.
    pub fn connect_tcp(self: &Arc<Self>, host: &str, port: u16) -> Result<TcpStream> {
        let (id, rx) = self.net_open(NetKind::Tcp, host, port)?;
        Ok(TcpStream { plugin: self.clone(), id, rx, buf: vec![], closed: false })
    }
}

/// A TCP connection opened by Foxglove.
pub struct TcpStream {
    plugin: Arc<Plugin>,
    id: u64,
    rx: Receiver<NetEvent>,
    buf: Vec<u8>,
    closed: bool,
}

impl Read for TcpStream {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.buf.is_empty() && !self.closed {
            match self.rx.recv() {
                Ok(NetEvent::Data(bytes, _)) => self.buf = bytes,
                Ok(NetEvent::Closed(_)) | Err(_) => self.closed = true,
            }
        }
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf.drain(..n);
        Ok(n)
    }
}

impl Write for TcpStream {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.plugin.net_send(self.id, data).map_err(|e| match e {
            PluginError::Closed => std::io::Error::from(std::io::ErrorKind::BrokenPipe),
            e => std::io::Error::other(e.to_string()),
        })?;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let _ = self.plugin.net_close(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_before_the_id_is_known_are_kept() {
        let routes = Routes::default();
        assert!(routes.handle("net.data", json!({"stream": 4, "data": STANDARD.encode(b"early")})));
        let rx = routes.register(4);
        routes.handle("net.data", json!({"stream": 4, "data": STANDARD.encode(b"late"), "from": "127.0.0.1:9"}));
        routes.handle("net.closed", json!({"stream": 4, "reason": "bye"}));
        let got: Vec<NetEvent> = rx.try_iter().collect();
        assert_eq!(
            got,
            [
                NetEvent::Data(b"early".to_vec(), None),
                NetEvent::Data(b"late".to_vec(), Some("127.0.0.1:9".into())),
                NetEvent::Closed("bye".into())
            ]
        );
        assert!(!routes.handle("facts.changed", json!({})));
    }
}
