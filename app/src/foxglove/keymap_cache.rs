//! The keymap as ZMK Studio last sent it, so the overlay can start when Studio can't be asked
//! (keyboard on Bluetooth only, Studio locked or busy).
//!
//! Saved as the conversation itself: while Foxglove reads the keymap over USB, each request it
//! sends and the keyboard's reply are recorded. Later a Studio client is answered from that:
//! each request gets the reply its identical recorded request got, renumbered (the library
//! starts every session at a random request id, and asks for behaviour details in no fixed
//! order). No format of our own to keep in step with the library: a recording it can't use
//! just fails, and Foxglove reports the original error.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prost::Message;
use zmk_studio_api::proto::zmk::studio::{response, Request, Response};
use zmk_studio_api::transport::serial::{SerialTransport, SerialTransportError};
use zmk_studio_api::StudioClient;

// ZMK Studio's framing (zmk-studio-api's `framing`, which isn't public).
const SOF: u8 = 0xAB;
const ESC: u8 = 0xAC;
const EOF: u8 = 0xAD;

/// Splits a byte stream into Studio frames.
#[derive(Default)]
struct Frames {
    data: Vec<u8>,
    in_frame: bool,
    escaped: bool,
}

impl Frames {
    fn push(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        for &b in chunk {
            if self.escaped {
                self.data.push(b);
                self.escaped = false;
            } else if !self.in_frame {
                self.in_frame = b == SOF;
            } else {
                match b {
                    SOF => self.data.clear(),
                    ESC => self.escaped = true,
                    EOF => {
                        frames.push(std::mem::take(&mut self.data));
                        self.in_frame = false;
                    }
                    _ => self.data.push(b),
                }
            }
        }
        frames
    }
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![SOF];
    for &b in payload {
        if matches!(b, SOF | ESC | EOF) {
            out.push(ESC);
        }
        out.push(b);
    }
    out.push(EOF);
    out
}

fn request_id(r: &Response) -> Option<u32> {
    match &r.r#type {
        Some(response::Type::RequestResponse(rr)) => Some(rr.request_id),
        _ => None,
    }
}

/// A request and the keyboard's reply to it.
#[derive(Clone, Debug, PartialEq)]
struct Exchange {
    request: Request,
    reply: Response,
}

/// A transport that keeps a copy of what's written and read once recording is switched on.
pub struct Recorder<T> {
    inner: T,
    recording: Recording,
}

/// What a [`Recorder`] has captured so far.
#[derive(Clone, Default)]
pub struct Recording {
    sent: Arc<Mutex<Vec<u8>>>,
    received: Arc<Mutex<Vec<u8>>>,
    on: Arc<AtomicBool>,
}

impl Recording {
    /// Each request that got a reply, with that reply, in the order they were sent.
    fn exchanges(&self) -> Vec<Exchange> {
        let mut replies: HashMap<u32, Response> = HashMap::new();
        for frame in Frames::default().push(&self.received.lock().unwrap()) {
            if let Ok(reply) = Response::decode(frame.as_slice()) {
                if let Some(id) = request_id(&reply) {
                    replies.entry(id).or_insert(reply);
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        Frames::default()
            .push(&self.sent.lock().unwrap())
            .into_iter()
            .filter_map(|frame| Request::decode(frame.as_slice()).ok())
            .filter(|request| seen.insert(request.request_id)) // re-sends
            .filter_map(|request| {
                let reply = replies.remove(&request.request_id)?;
                Some(Exchange { request, reply })
            })
            .collect()
    }
}

impl<T: Read> Read for Recorder<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if self.recording.on.load(Ordering::Relaxed) {
            self.recording
                .received
                .lock()
                .unwrap()
                .extend_from_slice(&buf[..n]);
        }
        Ok(n)
    }
}

impl<T: Write> Write for Recorder<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        if self.recording.on.load(Ordering::Relaxed) {
            self.recording
                .sent
                .lock()
                .unwrap()
                .extend_from_slice(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Like `StudioClient::open_serial`, recording the conversation after the stale-input drain.
pub fn open_serial(
    port: &str,
) -> Result<(StudioClient<Recorder<SerialTransport>>, Recording), SerialTransportError> {
    let recording = Recording::default();
    let io = Recorder {
        inner: SerialTransport::open(port)?,
        recording: recording.clone(),
    };
    let mut client = StudioClient::new(io);
    client.drain_stale_input(Duration::from_secs(1));
    recording.on.store(true, Ordering::Relaxed);
    Ok((client, recording))
}

/// Answers each request with the reply its recorded twin got, renumbered. A request with no
/// recorded twin gets no answer, and an empty read is an error, so the client fails at once.
pub struct Replay {
    exchanges: Vec<Exchange>,
    requests: Frames,
    out: VecDeque<u8>,
}

impl Replay {
    fn new(exchanges: Vec<Exchange>) -> Self {
        Replay {
            exchanges,
            requests: Frames::default(),
            out: VecDeque::new(),
        }
    }

    fn answer(&self, request: &Request) -> Option<Response> {
        let twin = self
            .exchanges
            .iter()
            .find(|e| e.request.subsystem == request.subsystem)?;
        let mut reply = twin.reply.clone();
        if let Some(response::Type::RequestResponse(rr)) = &mut reply.r#type {
            rr.request_id = request.request_id;
        }
        Some(reply)
    }
}

impl Write for Replay {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for frame in self.requests.push(buf) {
            let Ok(request) = Request::decode(frame.as_slice()) else {
                continue;
            };
            if let Some(reply) = self.answer(&request) {
                self.out.extend(encode_frame(&reply.encode_to_vec()));
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Replay {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.out.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the saved keymap has no answer to this",
            ));
        }
        let n = buf.len().min(self.out.len());
        for (dst, src) in buf.iter_mut().zip(self.out.drain(..n)) {
            *dst = src;
        }
        Ok(n)
    }
}

fn path(vid: u16, pid: u16) -> Option<PathBuf> {
    crate::settings::Settings::config_file_path()
        .map(|p| p.with_file_name(format!("keymap-{vid:04X}-{pid:04X}.studio")))
}

/// The exchanges as a file: for each, the request then the reply, each as its length (u32,
/// little endian) and its protobuf bytes.
fn to_file(exchanges: &[Exchange]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in exchanges {
        for bytes in [e.request.encode_to_vec(), e.reply.encode_to_vec()] {
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
    }
    out
}

fn from_file(mut bytes: &[u8]) -> Option<Vec<Exchange>> {
    let mut next = || -> Option<&[u8]> {
        let len = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        let item = bytes.get(4..4 + len)?;
        bytes = &bytes[4 + len..];
        Some(item)
    };
    let mut exchanges = Vec::new();
    while let Some(request) = next() {
        exchanges.push(Exchange {
            request: Request::decode(request).ok()?,
            reply: Response::decode(next()?).ok()?,
        });
    }
    Some(exchanges)
}

/// Keeps `recording` as keyboard `vid:pid`'s keymap.
pub fn save(vid: u16, pid: u16, recording: &Recording) {
    let exchanges = recording.exchanges();
    let Some(path) = path(vid, pid).filter(|_| !exchanges.is_empty()) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, to_file(&exchanges)) {
        eprintln!("Foxglove: can't save the keymap to {}: {e}", path.display());
    }
}

/// A Studio client that answers from keyboard `vid:pid`'s saved keymap, if there is one.
pub fn replay(vid: u16, pid: u16) -> Option<StudioClient<Replay>> {
    let exchanges = from_file(&std::fs::read(path(vid, pid)?).ok()?)?;
    let mut client = StudioClient::new(Replay::new(exchanges));
    client.set_call_timeout(Duration::from_millis(500));
    Some(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zmk_studio_api::proto::zmk::studio::{request, RequestResponse};
    use zmk_studio_api::proto::zmk::{core, keymap};

    fn reply(id: u32, layers: usize) -> Response {
        // Distinguishable replies: a keymap subsystem reply with `layers` empty layers.
        Response {
            r#type: Some(response::Type::RequestResponse(RequestResponse {
                request_id: id,
                subsystem: Some(
                    zmk_studio_api::proto::zmk::studio::request_response::Subsystem::Keymap(
                        keymap::Response {
                            response_type: Some(keymap::response::ResponseType::GetKeymap(
                                keymap::Keymap {
                                    layers: vec![Default::default(); layers],
                                    ..Default::default()
                                },
                            )),
                        },
                    ),
                ),
            })),
        }
    }

    fn get_keymap(id: u32) -> Request {
        Request {
            request_id: id,
            subsystem: Some(request::Subsystem::Keymap(keymap::Request {
                request_type: Some(keymap::request::RequestType::GetKeymap(true)),
            })),
        }
    }

    fn get_lock_state(id: u32) -> Request {
        Request {
            request_id: id,
            subsystem: Some(request::Subsystem::Core(core::Request {
                request_type: Some(core::request::RequestType::GetLockState(true)),
            })),
        }
    }

    fn frames(items: &[Vec<u8>]) -> Vec<u8> {
        items.iter().flat_map(|b| encode_frame(b)).collect()
    }

    #[test]
    fn frames_round_trip_with_escapes() {
        let payload = [1, SOF, 2, ESC, EOF, 3];
        let mut stream = vec![0x55]; // noise before the frame is skipped
        stream.extend(encode_frame(&payload));
        assert_eq!(Frames::default().push(&stream), vec![payload.to_vec()]);
    }

    #[test]
    fn recording_pairs_requests_with_their_replies() {
        let recording = Recording::default();
        recording.on.store(true, Ordering::Relaxed);
        // Lock state (id 7) sent twice (a re-send), keymap (id 8) once; replies out of order.
        *recording.sent.lock().unwrap() = frames(&[
            get_lock_state(7).encode_to_vec(),
            get_lock_state(7).encode_to_vec(),
            get_keymap(8).encode_to_vec(),
        ]);
        *recording.received.lock().unwrap() = frames(&[
            reply(8, 6).encode_to_vec(),
            reply(7, 1).encode_to_vec(),
            reply(7, 1).encode_to_vec(),
        ]);
        let exchanges = recording.exchanges();
        assert_eq!(exchanges.len(), 2);
        assert_eq!(exchanges[0].request, get_lock_state(7));
        assert_eq!(exchanges[1].reply, reply(8, 6));
        assert_eq!(from_file(&to_file(&exchanges)).unwrap(), exchanges);
    }

    #[test]
    fn replay_answers_by_request_and_renumbers() {
        let mut r = Replay::new(vec![
            Exchange {
                request: get_lock_state(7),
                reply: reply(7, 1),
            },
            Exchange {
                request: get_keymap(8),
                reply: reply(8, 6),
            },
        ]);
        // Asked in the other order, with new ids.
        r.write_all(&encode_frame(&get_keymap(42).encode_to_vec()))
            .unwrap();
        let mut buf = [0u8; 256];
        let n = r.read(&mut buf).unwrap();
        let got = Response::decode(Frames::default().push(&buf[..n])[0].as_slice()).unwrap();
        assert_eq!(got, reply(42, 6));
        assert_eq!(
            r.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}

/// Against a real keyboard on `FOXGLOVE_TEST_PORT` (default COM4):
/// `cargo test -- --ignored replays_a_real_keyboard`
#[cfg(test)]
mod hardware {
    use super::*;
    use crate::protocols::zmk_rpc::fetch_zmk_data_from_client;

    #[test]
    #[ignore]
    fn replays_a_real_keyboard() {
        let port = std::env::var("FOXGLOVE_TEST_PORT").unwrap_or_else(|_| "COM4".into());
        let (client, recording) = open_serial(&port).expect("open port");
        let live = fetch_zmk_data_from_client(client).expect("live keymap");
        let exchanges = from_file(&to_file(&recording.exchanges())).expect("saved");
        let replayed =
            fetch_zmk_data_from_client(StudioClient::new(Replay::new(exchanges.clone())))
                .expect("replayed keymap");
        assert_eq!(live.resolved_layers, replayed.resolved_layers);
        assert_eq!(live.physical_layouts, replayed.physical_layouts);
        assert_eq!(live.supported_behaviors, replayed.supported_behaviors);
        println!(
            "{} layers from {} saved exchanges",
            replayed.resolved_layers.len(),
            exchanges.len()
        );
    }
}
