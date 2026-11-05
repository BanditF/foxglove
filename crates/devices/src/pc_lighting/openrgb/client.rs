//! A client for a running OpenRGB server (its SDK server, `127.0.0.1:6742` by default).
//!
//! One connection, two halves: requests and writes go out on the caller's thread; one reader
//! thread blocks on the socket and hands each answer to the request waiting for it. The
//! server's DEVICE_LIST_UPDATED push and the connection closing reach the host as [`Event`]s
//! the moment they arrive: nothing polls. Every wait for an answer is bounded by the client's
//! timeout; a server that never answers REQUEST_PROTOCOL_VERSION is spoken to at version 0, as
//! the document says.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use super::wire::{self, id, Controller, Header, Packet, HEADER_LEN};
use crate::{DeviceError, Result, Rgb};

/// What the server tells the host without being asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// DEVICE_LIST_UPDATED: controllers came or went. Indices may now name other devices, so
    /// the host lists them again ([`super::list`]); drivers opened before refuse to write.
    DeviceListUpdated,
    /// The server closed the connection (OpenRGB quit) or it broke.
    Disconnected,
}

/// A connection to an OpenRGB server.
pub struct Client {
    out: Mutex<TcpStream>,
    replies: Mutex<mpsc::Receiver<Packet>>,
    version: u32,
    timeout: Duration,
    /// Counts DEVICE_LIST_UPDATED pushes: a driver opened at one generation writes only while
    /// it still holds.
    generation: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
}

impl Client {
    /// Connects, negotiates the protocol version (at most [`wire::CLIENT_PROTOCOL`]) and names
    /// this client `name` in OpenRGB's list. `timeout` bounds the connect and every answer.
    /// `events` is called on the reader thread for each [`Event`].
    pub fn connect(
        addr: SocketAddr,
        name: &str,
        timeout: Duration,
        events: impl FnMut(Event) + Send + 'static,
    ) -> Result<Client> {
        let stream = TcpStream::connect_timeout(&addr, timeout).map_err(io_error)?;
        stream.set_nodelay(true).map_err(io_error)?;
        stream.set_write_timeout(Some(timeout)).map_err(io_error)?;
        let read = stream.try_clone().map_err(io_error)?;
        let (tx, rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let closing = Arc::new(AtomicBool::new(false));
        // Detached: it ends when the socket does, and never holds up a drop.
        std::thread::Builder::new()
            .name("openrgb-reader".into())
            .spawn({
                let generation = Arc::clone(&generation);
                let closing = Arc::clone(&closing);
                move || read_loop(read, tx, generation, closing, events)
            })
            .map_err(io_error)?;
        let mut client = Client {
            out: Mutex::new(stream),
            replies: Mutex::new(rx),
            version: 0,
            timeout,
            generation,
            closing,
        };
        client.version = match client.request(
            &wire::protocol_version_request(wire::CLIENT_PROTOCOL),
            id::REQUEST_PROTOCOL_VERSION,
            0,
        ) {
            Ok(p) => word(&p.payload)?.min(wire::CLIENT_PROTOCOL),
            // An old server ignores the request: it speaks version 0.
            Err(DeviceError::Timeout) => 0,
            Err(e) => return Err(e),
        };
        client.send(&wire::client_name(name))?;
        Ok(client)
    }

    /// The negotiated protocol version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// How many DEVICE_LIST_UPDATED pushes have arrived.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn controller_count(&self) -> Result<u32> {
        let p = self.request(
            &wire::controller_count_request(),
            id::REQUEST_CONTROLLER_COUNT,
            0,
        )?;
        word(&p.payload)
    }

    /// One controller's data, by index.
    pub fn controller(&self, index: u32) -> Result<Controller> {
        let p = self.request(
            &wire::controller_data_request(index, self.version),
            id::REQUEST_CONTROLLER_DATA,
            index,
        )?;
        wire::parse_controller(&p.payload, self.version)
    }

    /// Every controller, in index order.
    pub fn controllers(&self) -> Result<Vec<Controller>> {
        (0..self.controller_count()?)
            .map(|i| self.controller(i))
            .collect()
    }

    /// One colour per LED of a controller (UPDATELEDS). No answer comes back.
    pub fn update_leds(&self, index: u32, colors: &[Rgb]) -> Result<()> {
        self.send(&wire::update_leds(index, colors)?)
    }

    /// One colour per LED of one of a controller's zones (UPDATEZONELEDS).
    pub fn update_zone_leds(&self, index: u32, zone: u32, colors: &[Rgb]) -> Result<()> {
        self.send(&wire::update_zone_leds(index, zone, colors)?)
    }

    /// Puts a controller in the mode that shows sent colours (SETCUSTOMMODE).
    pub fn set_custom_mode(&self, index: u32) -> Result<()> {
        self.send(&wire::set_custom_mode(index))
    }

    fn send(&self, bytes: &[u8]) -> Result<()> {
        let mut out = self.out.lock().map_err(|_| DeviceError::Disconnected)?;
        out.write_all(bytes).map_err(io_error)
    }

    /// Sends a request and waits (at most the timeout) for the answer with this packet id and
    /// device index. Requests run one at a time; a late answer to an earlier, timed-out
    /// request is dropped, never taken for this one's.
    fn request(&self, bytes: &[u8], want: u32, device: u32) -> Result<Packet> {
        let replies = self.replies.lock().map_err(|_| DeviceError::Disconnected)?;
        while replies.try_recv().is_ok() {}
        self.send(bytes)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match replies.recv_timeout(left) {
                Ok(p) if p.id == want && p.device == device => return Ok(p),
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(DeviceError::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(DeviceError::Disconnected),
            }
        }
    }
}

impl Drop for Client {
    /// Closes the socket. The reader thread ends as the connection does, and from now on
    /// reports nothing: no [`Event`] follows our own close.
    fn drop(&mut self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Ok(out) = self.out.lock() {
            let _ = out.shutdown(Shutdown::Both);
        }
    }
}

/// The reader thread: blocks on the socket (an event wait, not a poll) until it closes.
fn read_loop(
    mut read: TcpStream,
    replies: mpsc::Sender<Packet>,
    generation: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    mut events: impl FnMut(Event),
) {
    loop {
        match read_packet(&mut read) {
            Ok(p) if p.id == id::DEVICE_LIST_UPDATED => {
                generation.fetch_add(1, Ordering::SeqCst);
                if !closing.load(Ordering::SeqCst) {
                    events(Event::DeviceListUpdated);
                }
            }
            Ok(p) => {
                // Nobody waiting any more (the client is going): stop.
                if replies.send(p).is_err() {
                    return;
                }
            }
            Err(_) => {
                if !closing.load(Ordering::SeqCst) {
                    events(Event::Disconnected);
                }
                return;
            }
        }
    }
}

/// Reads one whole packet.
pub fn read_packet(r: &mut impl Read) -> Result<Packet> {
    let mut head = [0; HEADER_LEN];
    r.read_exact(&mut head).map_err(io_error)?;
    let h = Header::decode(&head)?;
    let mut payload = vec![0; h.size as usize];
    r.read_exact(&mut payload).map_err(io_error)?;
    Ok(Packet {
        device: h.device,
        id: h.id,
        payload,
    })
}

fn word(payload: &[u8]) -> Result<u32> {
    payload
        .get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| DeviceError::Backend("OpenRGB: short answer".into()))
}

fn io_error(e: std::io::Error) -> DeviceError {
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => DeviceError::Timeout,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe
        | ErrorKind::UnexpectedEof
        | ErrorKind::NotConnected => DeviceError::Disconnected,
        _ => DeviceError::Backend(format!("OpenRGB: {e}")),
    }
}

/// An in-process fake OpenRGB server on 127.0.0.1, for tests here and in the driver's: it
/// answers from controller blocks built with [`wire::fixture`] and records what it is sent.
/// An in-process fake OpenRGB server, for tests here and in the app (the `mock` feature).
#[cfg(any(test, feature = "mock"))]
#[doc(hidden)]
pub mod fake {
    use super::*;
    use std::net::TcpListener;

    /// How the fake answers REQUEST_PROTOCOL_VERSION.
    #[derive(Clone, Copy)]
    pub enum Answers {
        /// Its own highest version.
        Version(u32),
        /// Not at all, as an old (version 0) server.
        Silent,
    }

    /// Builds one controller's block at the negotiated version.
    pub type Build = Box<dyn Fn(u32) -> Vec<u8> + Send>;

    pub struct Fake {
        pub addr: SocketAddr,
        /// Every packet the client sent, in order.
        pub got: Arc<Mutex<Vec<Packet>>>,
        /// Pushes DEVICE_LIST_UPDATED, or (with `None`) closes the connection.
        pub push: mpsc::Sender<Option<()>>,
    }

    impl Fake {
        /// Serves one client: `controllers` are block builders taking the negotiated version.
        pub fn start(answers: Answers, controllers: Vec<Build>) -> Fake {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let got: Arc<Mutex<Vec<Packet>>> = Arc::default();
            let (push, pushed) = mpsc::channel::<Option<()>>();
            let log = Arc::clone(&got);
            // Detached: on Windows a shutdown doesn't wake a blocked read on the same socket,
            // so it ends when the client goes (or its bounded read timeout passes).
            std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                // Bounded: a test that goes wrong ends instead of hanging.
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let shared = Arc::new(Mutex::new(stream.try_clone().unwrap()));
                let pusher = Arc::clone(&shared);
                std::thread::spawn(move || {
                    while let Ok(msg) = pushed.recv() {
                        let s = pusher.lock().unwrap();
                        match msg {
                            Some(_) => {
                                let _ =
                                    (&*s).write_all(&wire::packet(0, id::DEVICE_LIST_UPDATED, &[]));
                            }
                            None => {
                                let _ = s.shutdown(Shutdown::Both);
                                return;
                            }
                        }
                    }
                });
                let mut read = stream;
                let mut version = 0;
                while let Ok(p) = read_packet(&mut read) {
                    log.lock().unwrap().push(p.clone());
                    let answer = match p.id {
                        id::REQUEST_PROTOCOL_VERSION => match answers {
                            Answers::Version(v) => {
                                version = v.min(word(&p.payload).unwrap());
                                Some(v.to_le_bytes().to_vec())
                            }
                            Answers::Silent => None,
                        },
                        id::REQUEST_CONTROLLER_COUNT => {
                            Some((controllers.len() as u32).to_le_bytes().to_vec())
                        }
                        id::REQUEST_CONTROLLER_DATA => controllers
                            .get(p.device as usize)
                            .map(|build| build(version)),
                        _ => None,
                    };
                    if let Some(payload) = answer {
                        let s = shared.lock().unwrap();
                        (&*s)
                            .write_all(&wire::packet(p.device, p.id, &payload))
                            .unwrap();
                    }
                }
                // The client went: close, as a real server does.
                let _ = shared.lock().unwrap().shutdown(Shutdown::Both);
            });
            Fake { addr, got, push }
        }

        /// The packets of one id the client sent.
        pub fn sent(&self, id: u32) -> Vec<Packet> {
            self.got
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.id == id)
                .cloned()
                .collect()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = self.push.send(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use crate::pc_lighting::openrgb::wire::fixture::{controller, ZoneFx};

    const T: Duration = Duration::from_secs(5);

    fn strip(name: &'static str, leds: u32) -> Build {
        Box::new(move |v| {
            controller(
                v,
                name,
                "",
                &[ZoneFx {
                    name: "Strip",
                    leds,
                    matrix: None,
                    segments: vec![],
                }],
                |i| format!("LED {}", i + 1),
                [9, 9, 9],
            )
        })
    }

    #[test]
    fn it_negotiates_names_itself_and_lists_controllers() {
        let fake = Fake::start(Answers::Version(5), vec![strip("A", 2), strip("B", 3)]);
        let client = Client::connect(fake.addr, "Foxglove", T, |_| {}).unwrap();
        assert_eq!(client.version(), 4, "a newer server is spoken to at ours");
        let list = client.controllers().unwrap();
        assert_eq!(
            list.iter()
                .map(|c| (c.name.as_str(), c.leds.len()))
                .collect::<Vec<_>>(),
            vec![("A", 2), ("B", 3)]
        );
        assert_eq!(list[0].vendor, "Fixture Vendor");
        let name = fake.sent(id::SET_CLIENT_NAME);
        assert_eq!(name[0].payload, b"Foxglove\0");
        let data = fake.sent(id::REQUEST_CONTROLLER_DATA);
        assert_eq!(data[1].device, 1);
        assert_eq!(
            data[1].payload,
            4u32.to_le_bytes(),
            "asks at the negotiated version"
        );
    }

    #[test]
    fn an_older_server_is_spoken_to_at_its_version() {
        let fake = Fake::start(Answers::Version(2), vec![strip("A", 1)]);
        let client = Client::connect(fake.addr, "Foxglove", T, |_| {}).unwrap();
        assert_eq!(client.version(), 2);
        assert_eq!(client.controller(0).unwrap().name, "A");
    }

    #[test]
    fn a_server_that_never_answers_the_version_is_version_0() {
        let fake = Fake::start(Answers::Silent, vec![strip("Old", 2)]);
        let client =
            Client::connect(fake.addr, "Foxglove", Duration::from_millis(200), |_| {}).unwrap();
        assert_eq!(client.version(), 0);
        let c = client.controller(0).unwrap();
        assert_eq!((c.name.as_str(), c.vendor.as_str()), ("Old", ""));
        assert!(fake.sent(id::REQUEST_CONTROLLER_DATA)[0].payload.is_empty());
    }

    #[test]
    fn writes_go_out_as_the_document_lays_them_out() {
        let fake = Fake::start(Answers::Version(4), vec![strip("A", 2)]);
        let client = Client::connect(fake.addr, "Foxglove", T, |_| {}).unwrap();
        let red = Rgb { r: 255, g: 0, b: 0 };
        client.set_custom_mode(0).unwrap();
        client.update_leds(0, &[red, red]).unwrap();
        client.update_zone_leds(0, 0, &[red]).unwrap();
        // One round trip: the server reads in order, so it has every write by its answer.
        client.controller_count().unwrap();
        let zone = fake.sent(id::UPDATEZONELEDS);
        assert_eq!(fake.sent(id::SETCUSTOMMODE).len(), 1);
        assert_eq!(
            fake.sent(id::UPDATELEDS)[0].payload,
            vec![14, 0, 0, 0, 2, 0, 255, 0, 0, 0, 255, 0, 0, 0]
        );
        assert_eq!(
            zone[0].payload,
            vec![14, 0, 0, 0, 0, 0, 0, 0, 1, 0, 255, 0, 0, 0]
        );
    }

    #[test]
    fn list_changes_and_the_server_going_arrive_as_events() {
        let fake = Fake::start(Answers::Version(4), vec![strip("A", 1)]);
        let (tx, rx) = mpsc::channel();
        let client = Client::connect(fake.addr, "Foxglove", T, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        assert_eq!(client.generation(), 0);
        fake.push.send(Some(())).unwrap();
        assert_eq!(rx.recv_timeout(T), Ok(Event::DeviceListUpdated));
        assert_eq!(client.generation(), 1);
        // A push in the middle of a request doesn't take its place.
        assert_eq!(client.controller_count().unwrap(), 1);
        drop(fake);
        assert_eq!(rx.recv_timeout(T), Ok(Event::Disconnected));
        assert!(client.controller_count().is_err());
    }

    #[test]
    fn no_server_is_a_quick_disconnected_and_a_client_closes_quietly() {
        let addr = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let err = Client::connect(addr, "Foxglove", Duration::from_millis(500), |_| {});
        assert!(matches!(
            err,
            Err(DeviceError::Disconnected | DeviceError::Timeout)
        ));

        let fake = Fake::start(Answers::Version(4), vec![]);
        let (tx, rx) = mpsc::channel();
        let client = Client::connect(fake.addr, "Foxglove", T, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        assert_eq!(client.controllers().unwrap(), vec![]);
        drop(client);
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "no event on our own close"
        );
    }
}
