//! WebSocket server side (RFC 6455) over an accepted connection ([`crate::TcpListener`], #298):
//! read the upgrade request, check who's asking, answer it, then read and write messages.
//!
//! The checks are the plugin's to choose, as a page's `Origin` depends on where it's loaded
//! from (a `file://` page sends `null`): [`Handshake::check`] takes the origins to accept and a
//! per-install token the page must present (`?token=` or the `Sec-WebSocket-Protocol`).

use std::io::{self, Read, Write};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// The largest message accepted from a client.
const MAX_MESSAGE: usize = 1 << 20;

/// The client's upgrade request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Handshake {
    /// The request path, with its query.
    pub path: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
}

impl Handshake {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `Origin`, as the browser sent it (`null` from a `file://` page).
    pub fn origin(&self) -> Option<&str> {
        self.header("origin")
    }

    /// A query parameter of the path (`/?token=abc` → `token` = `abc`), undecoded.
    pub fn query(&self, name: &str) -> Option<&str> {
        let (_, q) = self.path.split_once('?')?;
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }

    /// Accepts only `origins` (when any are given) and, with `token`, a client that presents it
    /// as `?token=` or as its `Sec-WebSocket-Protocol`.
    pub fn check(&self, origins: &[&str], token: Option<&str>) -> Result<(), String> {
        if !origins.is_empty() && !self.origin().is_some_and(|o| origins.contains(&o)) {
            return Err(format!("origin {:?} isn't allowed", self.origin()));
        }
        if let Some(token) = token {
            let given = self
                .query("token")
                .or(self.header("sec-websocket-protocol").map(str::trim));
            if !given.is_some_and(|g| same(g.as_bytes(), token.as_bytes())) {
                return Err("wrong or missing token".into());
            }
        }
        Ok(())
    }
}

/// Compares without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Reads the upgrade request (at most 16 KiB).
pub fn read_handshake(r: &mut impl Read) -> io::Result<Handshake> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 || r.read(&mut byte)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "no complete upgrade request",
            ));
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or_default();
    let mut parts = request.split(' ');
    if parts.next() != Some("GET") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("not a WebSocket upgrade: {request}"),
        ));
    }
    let path = parts.next().unwrap_or("/").to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Ok(Handshake { path, headers })
}

/// The `Sec-WebSocket-Accept` for a client's key.
pub fn accept_key(key: &str) -> String {
    STANDARD.encode(sha1(format!("{key}{GUID}").as_bytes()))
}

/// Answers the upgrade (101); `protocol` echoes the sub-protocol the client offered, which a
/// browser requires when it sent one (as a token, say).
pub fn accept(w: &mut impl Write, h: &Handshake, protocol: Option<&str>) -> io::Result<()> {
    let key = h
        .header("sec-websocket-key")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Sec-WebSocket-Key"))?;
    let protocol = protocol
        .map(|p| format!("Sec-WebSocket-Protocol: {p}\r\n"))
        .unwrap_or_default();
    write!(w, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n{protocol}\r\n", accept_key(key))?;
    w.flush()
}

/// Refuses the upgrade with an HTTP status.
pub fn reject(w: &mut impl Write, status: u16, reason: &str) -> io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    w.flush()
}

pub const TEXT: u8 = 0x1;
pub const BINARY: u8 = 0x2;
pub const CLOSE: u8 = 0x8;
pub const PING: u8 = 0x9;
pub const PONG: u8 = 0xA;

/// Writes one unmasked frame (a server's).
pub fn write_frame(w: &mut impl Write, op: u8, payload: &[u8]) -> io::Result<()> {
    let mut f = vec![0x80 | op];
    match payload.len() {
        n if n < 126 => f.push(n as u8),
        n if n <= 0xFFFF => {
            f.push(126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    f.extend_from_slice(payload);
    w.write_all(&f)?;
    w.flush()
}

pub fn write_text(w: &mut impl Write, text: &str) -> io::Result<()> {
    write_frame(w, TEXT, text.as_bytes())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// The client's close code, if it gave one.
    Close(Option<u16>),
}

/// Reads whole messages from a client, joining fragments; a control frame (a ping) can come
/// between them without losing the message. Client frames must be masked.
#[derive(Default)]
pub struct Reader {
    data: Vec<u8>,
    first_op: Option<u8>,
}

impl Reader {
    pub fn read(&mut self, r: &mut impl Read) -> io::Result<Message> {
        read_message(r, &mut self.data, &mut self.first_op)
    }
}

fn read_message(
    r: &mut impl Read,
    data: &mut Vec<u8>,
    first_op: &mut Option<u8>,
) -> io::Result<Message> {
    loop {
        let mut head = [0u8; 2];
        r.read_exact(&mut head)?;
        let (fin, op, masked) = (head[0] & 0x80 != 0, head[0] & 0x0F, head[1] & 0x80 != 0);
        if !masked {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a client frame without a mask",
            ));
        }
        let len = match head[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                r.read_exact(&mut b)?;
                u64::from(u16::from_be_bytes(b))
            }
            127 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b)?;
                u64::from_be_bytes(b)
            }
            n => u64::from(n),
        };
        if len > MAX_MESSAGE as u64 || data.len() + len as usize > MAX_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a message too large",
            ));
        }
        let mut mask = [0u8; 4];
        r.read_exact(&mut mask)?;
        let mut payload = vec![0u8; len as usize];
        r.read_exact(&mut payload)?;
        payload
            .iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b ^= mask[i % 4]);
        match op {
            // Control frames come whole, even between fragments.
            CLOSE => {
                return Ok(Message::Close(
                    (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]])),
                ))
            }
            PING => return Ok(Message::Ping(payload)),
            PONG => return Ok(Message::Pong(payload)),
            _ => {
                first_op.get_or_insert(op);
                data.extend_from_slice(&payload);
                if fin {
                    let data = std::mem::take(data);
                    return Ok(match first_op.take() {
                        Some(BINARY) => Message::Binary(data),
                        _ => Message::Text(String::from_utf8_lossy(&data).into_owned()),
                    });
                }
            }
        }
    }
}

/// SHA-1, for the handshake's accept key only (RFC 3174).
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (hi, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *hi = hi.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_and_the_rfc_accept_key() {
        let hex: String = sha1(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1(&[b'a'; 1000]).len(), 20);
        // RFC 6455's example.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn a_handshake_is_read_checked_and_answered() {
        let req = b"GET /match?token=s3cret HTTP/1.1\r\nHost: 127.0.0.1:47911\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nOrigin: null\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
        let h = read_handshake(&mut &req[..]).unwrap();
        assert_eq!(
            (h.path.as_str(), h.origin(), h.query("token")),
            ("/match?token=s3cret", Some("null"), Some("s3cret"))
        );
        assert!(h.check(&["null"], Some("s3cret")).is_ok());
        assert!(h.check(&["file://"], None).is_err(), "another origin");
        assert!(h.check(&[], Some("wrong")).is_err(), "a wrong token");
        assert!(h.check(&[], None).is_ok(), "no checks asked for");
        let mut out = Vec::new();
        accept(&mut out, &h, None).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.starts_with("HTTP/1.1 101")
                && out.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        );
        assert!(read_handshake(&mut &b"POST / HTTP/1.1\r\n\r\n"[..]).is_err());
    }

    /// A client frame: masked.
    fn client(op: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mask = [1, 2, 3, 4];
        let mut f = vec![if fin { 0x80 } else { 0 } | op, 0x80 | payload.len() as u8];
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    #[test]
    fn messages_in_and_out() {
        let mut stream = client(TEXT, false, b"hel");
        stream.extend(client(PING, true, b"p"));
        stream.extend(client(0, true, b"lo"));
        stream.extend(client(CLOSE, true, &1000u16.to_be_bytes()));
        let (mut r, mut reader) = (&stream[..], Reader::default());
        assert_eq!(
            reader.read(&mut r).unwrap(),
            Message::Ping(b"p".to_vec()),
            "a control frame between fragments"
        );
        assert_eq!(
            reader.read(&mut r).unwrap(),
            Message::Text("hello".into()),
            "the message isn't lost"
        );
        assert_eq!(reader.read(&mut r).unwrap(), Message::Close(Some(1000)));
        // Unmasked client frames are refused.
        assert!(Reader::default()
            .read(&mut &[0x81u8, 0x01, b'x'][..])
            .is_err());
        let mut out = Vec::new();
        write_text(&mut out, "hi").unwrap();
        assert_eq!(out, [0x81, 2, b'h', b'i']);
    }
}
