//! Just enough of a WebSocket client (RFC 6455) for obs-websocket on this PC: the upgrade
//! request, masked frames out, and whole messages in. The upgrade reply is checked for its 101
//! status only, which is enough on loopback.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use base64::Engine;

/// The largest message accepted; OBS's are a few kilobytes.
const MAX: usize = 1 << 20;

/// Random bytes for the upgrade key and the frame masks. Masking guards proxies, not secrets,
/// so the standard library's randomly keyed hasher is enough.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_usize(chunk.as_ptr() as usize);
        let bytes = h.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out
}

pub fn upgrade_request(host: &str, port: u16) -> String {
    let key = base64::engine::general_purpose::STANDARD.encode(random::<16>());
    format!(
        "GET / HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: obswebsocket.json\r\n\r\n"
    )
}

pub const TEXT: u8 = 0x1;
pub const CLOSE: u8 = 0x8;
pub const PING: u8 = 0x9;
pub const PONG: u8 = 0xA;

/// A frame from the client: final, masked.
pub fn frame(op: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x80 | op];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xFFFF => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let mask = random::<4>();
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

#[derive(Debug, PartialEq)]
pub enum Msg {
    Text(String),
    Ping(Vec<u8>),
    /// The server's close code, if it gave one.
    Close(Option<u16>),
}

/// Turns the bytes from the server into messages.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    upgraded: bool,
    partial: Vec<u8>,
}

impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Msg>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = vec![];
        if !self.upgraded {
            let Some(end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                return if self.buf.len() > 16 * 1024 {
                    Err("no reply to the WebSocket upgrade".into())
                } else {
                    Ok(out)
                };
            };
            let head = String::from_utf8_lossy(&self.buf[..end]).into_owned();
            let status = head.lines().next().unwrap_or_default();
            if !status.starts_with("HTTP/1.1 101") {
                return Err(format!("not a WebSocket server: {status}"));
            }
            self.buf.drain(..end + 4);
            self.upgraded = true;
        }
        loop {
            let b = &self.buf;
            if b.len() < 2 {
                break;
            }
            let (fin, op, masked) = (b[0] & 0x80 != 0, b[0] & 0x0F, b[1] & 0x80 != 0);
            let (len, mut at) = match b[1] & 0x7F {
                126 if b.len() >= 4 => (u64::from(u16::from_be_bytes([b[2], b[3]])), 4),
                127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()), 10),
                126 | 127 => break,
                n => (u64::from(n), 2),
            };
            if len > MAX as u64 || self.partial.len() + len as usize > MAX {
                return Err("a WebSocket message too large".into());
            }
            let mask = if masked {
                if b.len() < at + 4 {
                    break;
                }
                at += 4;
                Some([b[at - 4], b[at - 3], b[at - 2], b[at - 1]])
            } else {
                None
            };
            let len = len as usize;
            if b.len() < at + len {
                break;
            }
            let mut payload = b[at..at + len].to_vec();
            if let Some(m) = mask {
                payload
                    .iter_mut()
                    .enumerate()
                    .for_each(|(i, x)| *x ^= m[i % 4]);
            }
            self.buf.drain(..at + len);
            match op {
                // Text, binary, or a continuation of either.
                0x0..=0x2 => {
                    self.partial.extend_from_slice(&payload);
                    if fin {
                        out.push(Msg::Text(
                            String::from_utf8_lossy(&std::mem::take(&mut self.partial))
                                .into_owned(),
                        ));
                    }
                }
                CLOSE => out.push(Msg::Close(
                    (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]])),
                )),
                PING => out.push(Msg::Ping(payload)),
                _ => {}
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server frame: unmasked.
    fn server(op: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![if fin { 0x80 } else { 0 } | op];
        if payload.len() < 126 {
            out.push(payload.len() as u8);
        } else {
            out.push(126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn upgrade_then_messages_split_anywhere() {
        let long = "x".repeat(300);
        let mut bytes = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n".to_vec();
        bytes.extend(server(TEXT, true, b"{\"op\":0}"));
        bytes.extend(server(TEXT, false, b"he"));
        bytes.extend(server(0, true, b"llo"));
        bytes.extend(server(PING, true, b"p"));
        bytes.extend(server(TEXT, true, long.as_bytes()));
        bytes.extend(server(CLOSE, true, &4009u16.to_be_bytes()));
        // Fed a byte at a time, the same messages come out.
        let mut d = Decoder::default();
        let mut got = vec![];
        for b in &bytes {
            got.extend(d.feed(&[*b]).unwrap());
        }
        assert_eq!(
            got,
            [
                Msg::Text("{\"op\":0}".into()),
                Msg::Text("hello".into()),
                Msg::Ping(b"p".to_vec()),
                Msg::Text(long),
                Msg::Close(Some(4009))
            ]
        );
    }

    #[test]
    fn a_refused_upgrade_is_an_error() {
        let mut d = Decoder::default();
        assert!(d.feed(b"HTTP/1.1 400 Bad Request\r\n\r\n").is_err());
    }

    #[test]
    fn client_frames_are_masked() {
        let f = frame(TEXT, b"hello");
        assert_eq!((f[0], f[1]), (0x81, 0x85));
        let mask = &f[2..6];
        let plain: Vec<u8> = f[6..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        assert_eq!(plain, b"hello");
        let big = frame(TEXT, &[0; 300]);
        assert_eq!(
            (big[1], u16::from_be_bytes([big[2], big[3]])),
            (0x80 | 126, 300)
        );
    }
}
