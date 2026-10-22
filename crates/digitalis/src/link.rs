//! One owner for the keyboard's Raw HID interface, shared by everything that talks to it.
//!
//! A single thread reads with a short timeout, hands each input report to whoever wants it, and
//! writes queued reports between reads, so reads and writes never race on the handle:
//! - replies to our protocols (0xD5/0xD6 with byte 1 = command | 0x80) complete the request
//!   waiting for that (proto, command), and never reach subscribers;
//! - everything else goes to the subscribers whose filter matches (the overlay's 0xFF/0xF1, a
//!   debug tap, later integrations).
//!
//! The firmware's replies carry no sequence number, so only one request per (proto, command) is
//! on the wire at a time; later ones wait their turn, and a request whose caller has given up is
//! never sent. The link reconnects by itself; whatever was queued for a lost connection is
//! dropped, never sent to the next one.

use std::collections::{HashMap, VecDeque};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::protocol::{self, Hello, REPORT};
use crate::Transport;

/// How long one read waits; a queued write waits at most this long.
const READ_TIMEOUT_MS: i32 = 10;
/// Consecutive read errors, or consecutive write errors, that count as the keyboard being gone.
const MAX_ERRORS: u32 = 5;
const RETRY: Duration = Duration::from_secs(3);
/// Reports a subscriber may fall behind by; past that, new ones are dropped for it.
const SUBSCRIBER_BUFFER: usize = 256;

pub type Report = [u8; REPORT];

/// An open Raw HID interface.
pub trait RawHid: Send {
    /// Reads one input report; 0 = nothing within `timeout_ms`.
    fn read(&mut self, buf: &mut Report, timeout_ms: i32) -> Result<usize>;
    /// Writes one output report (at most [`REPORT`] bytes; the rest is zero).
    fn write(&mut self, report: &[u8]) -> Result<()>;
}

/// The keyboard the link is talking to.
#[derive(Clone, Debug, PartialEq)]
pub struct Connected {
    pub product: String,
    /// `None` when the keyboard has no host link (only with [`crate::hid::Match::without_host_link`]).
    pub hello: Option<Hello>,
}

/// Finds and opens the keyboard; called again after every disconnect.
pub type Opener = Box<dyn FnMut() -> Result<(Box<dyn RawHid>, Connected)> + Send>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Counts connections, so a reconnect is visible even to the same keyboard.
    pub generation: u64,
    pub connected: Option<Connected>,
    /// Why the link is waiting, while it is.
    pub error: Option<String>,
}

struct Request {
    key: (u8, u8),
    tx: Sender<Report>,
    /// When the caller stops waiting.
    deadline: Instant,
}

struct Command {
    report: Vec<u8>,
    reply: Option<Request>,
}

type Filter = Box<dyn Fn(&[u8]) -> bool + Send>;

struct Subscriber {
    filter: Filter,
    tx: SyncSender<Report>,
    alive: Arc<AtomicBool>,
}

/// Reports from [`Link::subscribe`]; use it like a `Receiver`. Dropping it unsubscribes.
pub struct Subscription {
    rx: Receiver<Report>,
    alive: Arc<AtomicBool>,
}

impl Deref for Subscription {
    type Target = Receiver<Report>;
    fn deref(&self) -> &Receiver<Report> {
        &self.rx
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

struct Shared {
    state: Mutex<State>,
    subscribers: Mutex<Vec<Subscriber>>,
    stop: AtomicBool,
}

pub struct Link {
    commands: Mutex<Sender<Command>>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Link {
    /// Starts the link thread; it connects with `open` and keeps reconnecting.
    pub fn start(open: Opener) -> Link {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            subscribers: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("digitalis-link".into())
            .spawn(move || run(open, rx, &thread_shared))
            .expect("link thread");
        Link { commands: Mutex::new(tx), shared, thread: Some(thread) }
    }

    pub fn state(&self) -> State {
        self.shared.state.lock().unwrap().clone()
    }

    /// Waits until the link is connected, for one-shot tools.
    pub fn wait_connected(&self, timeout: Duration) -> Result<Connected> {
        let end = Instant::now() + timeout;
        loop {
            let s = self.state();
            if let Some(c) = s.connected {
                return Ok(c);
            }
            if Instant::now() >= end {
                return Err(anyhow!(s.error.unwrap_or_else(|| "keyboard not connected".into())));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Input reports that aren't replies and match `filter`. A subscriber more than
    /// [`SUBSCRIBER_BUFFER`] reports behind misses the newest ones.
    pub fn subscribe(&self, filter: impl Fn(&[u8]) -> bool + Send + 'static) -> Subscription {
        let (tx, rx) = mpsc::sync_channel(SUBSCRIBER_BUFFER);
        let alive = Arc::new(AtomicBool::new(true));
        self.shared.subscribers.lock().unwrap().push(Subscriber {
            filter: Box::new(filter),
            tx,
            alive: alive.clone(),
        });
        Subscription { rx, alive }
    }

    fn queue(&self, command: Command) -> Result<()> {
        if self.shared.state.lock().unwrap().connected.is_none() {
            return Err(anyhow!("keyboard not connected"));
        }
        self.commands.lock().unwrap().send(command).map_err(|_| anyhow!("link stopped"))
    }
}

impl Transport for Link {
    fn send(&self, report: &[u8]) -> Result<()> {
        self.queue(Command { report: report.to_vec(), reply: None })
    }

    fn request(&self, report: &[u8], proto: u8, cmd: u8, timeout: Duration) -> Result<Report> {
        let (tx, rx) = mpsc::channel();
        let deadline = Instant::now() + timeout;
        self.queue(Command { report: report.to_vec(), reply: Some(Request { key: (proto, cmd), tx, deadline }) })?;
        rx.recv_timeout(timeout).map_err(|e| match e {
            // The link drops a request at its deadline, which can beat our own timeout.
            RecvTimeoutError::Disconnected if Instant::now() < deadline => anyhow!("keyboard disconnected"),
            _ => anyhow!("no reply from the keyboard"),
        })
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn set_state(shared: &Shared, f: impl FnOnce(&mut State)) {
    f(&mut shared.state.lock().unwrap());
}

/// Drops whatever is queued; dropping a request's sender fails it right away.
fn discard(commands: &Receiver<Command>) {
    while commands.try_recv().is_ok() {}
}

fn run(mut open: Opener, commands: Receiver<Command>, shared: &Shared) {
    while !shared.stop.load(Ordering::Relaxed) {
        let (mut dev, connected) = match open() {
            Ok(c) => c,
            Err(e) => {
                set_state(shared, |s| s.error = Some(e.to_string()));
                let end = Instant::now() + RETRY;
                while Instant::now() < end && !shared.stop.load(Ordering::Relaxed) {
                    discard(&commands);
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
        };
        set_state(shared, |s| {
            s.generation += 1;
            s.connected = Some(connected);
            s.error = None;
        });

        let error = serve(dev.as_mut(), &commands, shared);
        set_state(shared, |s| {
            s.connected = None;
            s.error = error;
        });
        // Meant for the connection that just ended, not for the next one.
        discard(&commands);
    }
}

/// Requests on the wire, and those waiting for their (proto, command) to be free.
#[derive(Default)]
struct Requests {
    in_flight: HashMap<(u8, u8), Request>,
    waiting: VecDeque<Command>,
}

/// Talks to one connection until it fails (returns why) or the link stops (returns None).
fn serve(dev: &mut dyn RawHid, commands: &Receiver<Command>, shared: &Shared) -> Option<String> {
    let mut requests = Requests::default();
    let (mut read_errors, mut write_errors) = (0, 0);
    let mut buf = [0u8; REPORT];
    while !shared.stop.load(Ordering::Relaxed) {
        match dev.read(&mut buf, READ_TIMEOUT_MS) {
            Ok(n) => {
                read_errors = 0;
                if n > 0 {
                    dispatch(&buf, &mut requests, shared);
                }
            }
            Err(e) => {
                read_errors += 1;
                if read_errors >= MAX_ERRORS {
                    return Some(format!("lost keyboard: {e}"));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        // Requests that waited for their key go first, then whatever is new, in order.
        let now = Instant::now();
        requests.in_flight.retain(|_, r| r.deadline > now);
        let queued: Vec<Command> =
            requests.waiting.drain(..).chain(std::iter::from_fn(|| commands.try_recv().ok())).collect();
        for c in queued {
            if let Some(r) = &c.reply {
                if r.deadline <= now {
                    continue; // its caller has given up: never send it
                }
                if requests.in_flight.contains_key(&r.key) {
                    requests.waiting.push_back(c);
                    continue;
                }
            }
            match dev.write(&c.report) {
                Ok(()) => write_errors = 0,
                Err(e) => {
                    write_errors += 1;
                    if write_errors >= MAX_ERRORS {
                        return Some(format!("lost keyboard: {e}"));
                    }
                    continue;
                }
            }
            if let Some(r) = c.reply {
                requests.in_flight.insert(r.key, r);
            }
        }
    }
    None
}

fn dispatch(r: &Report, requests: &mut Requests, shared: &Shared) {
    if matches!(r[0], protocol::PANEL | protocol::ART) && r[1] & 0x80 != 0 {
        // A reply: to its request if one is waiting, otherwise (late, unasked) to nobody.
        if let Some(req) = requests.in_flight.remove(&(r[0], r[1] & 0x7f)) {
            let _ = req.tx.send(*r);
        }
        return;
    }
    shared.subscribers.lock().unwrap().retain(|s| {
        if !s.alive.load(Ordering::Relaxed) {
            return false;
        }
        if !(s.filter)(r) {
            return true;
        }
        // A subscriber that has fallen far behind misses this one but stays subscribed.
        !matches!(s.tx.try_send(*r), Err(TrySendError::Disconnected(_)))
    });
}

#[cfg(feature = "hid")]
mod hid_open {
    use super::*;
    use crate::hid::{Keyboard, Match};

    impl RawHid for Keyboard {
        fn read(&mut self, buf: &mut Report, timeout_ms: i32) -> Result<usize> {
            Ok(self.device().read_timeout(buf, timeout_ms)?)
        }

        fn write(&mut self, report: &[u8]) -> Result<()> {
            Transport::send(self, report)
        }
    }

    impl Link {
        /// A link to the first keyboard matching `m` that answers HELLO.
        pub fn hid(m: Match) -> Result<Link> {
            let mut api = hidapi::HidApi::new()?;
            Ok(Link::start(Box::new(move || {
                let kb = Keyboard::open_matching(&mut api, &m)?;
                let connected = Connected { product: kb.product.clone(), hello: kb.hello };
                Ok((Box::new(kb) as Box<dyn RawHid>, connected))
            })))
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{self, art, panel};

    const WAIT: Duration = Duration::from_secs(2);

    /// A keyboard in memory. It answers every report whose byte 2 is an id by echoing that id,
    /// right away or once released, and it can send its own reports and die.
    #[derive(Clone, Default)]
    struct Fake {
        inbox: Arc<Mutex<VecDeque<Report>>>,
        held: Arc<Mutex<VecDeque<Report>>>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        dead: Arc<AtomicBool>,
        silent: Arc<AtomicBool>,
        hold: Arc<AtomicBool>,
    }

    fn report(bytes: &[u8]) -> Report {
        let mut r = [0u8; REPORT];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    impl Fake {
        fn push(&self, bytes: &[u8]) {
            self.inbox.lock().unwrap().push_back(report(bytes));
        }

        /// Lets the oldest held reply through.
        fn release_one(&self) {
            let r = self.held.lock().unwrap().pop_front().expect("a held reply");
            self.inbox.lock().unwrap().push_back(r);
        }

        fn written(&self) -> Vec<Vec<u8>> {
            self.written.lock().unwrap().clone()
        }

        fn wait_written(&self, n: usize) {
            let end = Instant::now() + WAIT;
            while self.written.lock().unwrap().len() < n && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl RawHid for Fake {
        fn read(&mut self, buf: &mut Report, timeout_ms: i32) -> Result<usize> {
            if self.dead.load(Ordering::Relaxed) {
                return Err(anyhow!("device gone"));
            }
            match self.inbox.lock().unwrap().pop_front() {
                Some(r) => {
                    *buf = r;
                    Ok(REPORT)
                }
                None => {
                    std::thread::sleep(Duration::from_millis(timeout_ms as u64));
                    Ok(0)
                }
            }
        }

        fn write(&mut self, out: &[u8]) -> Result<()> {
            if self.dead.load(Ordering::Relaxed) {
                return Err(anyhow!("device gone"));
            }
            self.written.lock().unwrap().push(out.to_vec());
            if !self.silent.load(Ordering::Relaxed) {
                let reply = report(&[out[0], 0x80 | out[1], out.get(2).copied().unwrap_or(0)]);
                if self.hold.load(Ordering::Relaxed) {
                    self.held.lock().unwrap().push_back(reply);
                } else {
                    // Another app's traffic arrives first, then the reply.
                    self.push(&[0xFF, 1, 0, 2]);
                    self.inbox.lock().unwrap().push_back(reply);
                }
            }
            Ok(())
        }
    }

    fn link(fake: &Fake) -> Link {
        let fake = fake.clone();
        let l = Link::start(Box::new(move || {
            if fake.dead.load(Ordering::Relaxed) {
                return Err(anyhow!("no keyboard"));
            }
            let c = Connected { product: "Fake".into(), hello: None };
            Ok((Box::new(fake.clone()) as Box<dyn RawHid>, c))
        }));
        l.wait_connected(WAIT).unwrap();
        l
    }

    /// An art STATUS request tagged with `id` (the firmware ignores byte 2).
    fn status(l: &Link, id: u8, timeout: Duration) -> Result<u8> {
        l.request(&[protocol::ART, art::STATUS, id], protocol::ART, art::STATUS, timeout).map(|r| r[2])
    }

    #[test]
    fn replies_reach_the_request_and_other_reports_the_subscriber() {
        let fake = Fake::default();
        let l = link(&fake);
        let layers = l.subscribe(|r| r[0] == 0xFF);
        let everything = l.subscribe(|_| true);
        let r = l.request(&[0xD5, panel::HELLO, 42], protocol::PANEL, panel::HELLO, WAIT).unwrap();
        assert_eq!((r[0], r[1], r[2]), (0xD5, 0x81, 42));
        assert_eq!(&layers.recv_timeout(WAIT).unwrap()[..4], &[0xFF, 1, 0, 2]);
        // The catch-all saw the layer report but not the reply.
        assert_eq!(everything.recv_timeout(WAIT).unwrap()[0], 0xFF);
        assert!(everything.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn unasked_replies_go_nowhere_but_other_protocols_pass() {
        let fake = Fake::default();
        let l = link(&fake);
        let everything = l.subscribe(|_| true);
        fake.push(&[0xD6, 0x85, 9]); // a late art reply nobody waits for
        fake.push(&[0x01, 0x81, 7]); // someone else's protocol with the high bit set
        assert_eq!(&everything.recv_timeout(WAIT).unwrap()[..3], &[0x01, 0x81, 7]);
        assert!(everything.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn same_key_requests_take_turns_and_get_their_own_replies() {
        let fake = Fake::default();
        fake.hold.store(true, Ordering::Relaxed);
        let l = Arc::new(link(&fake));
        let first = std::thread::spawn({
            let l = l.clone();
            move || status(&l, 1, WAIT)
        });
        fake.wait_written(1);
        let second = std::thread::spawn({
            let l = l.clone();
            move || status(&l, 2, WAIT)
        });
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fake.written().len(), 1, "the second waits until the first is answered");

        fake.release_one();
        assert_eq!(first.join().unwrap().unwrap(), 1);
        fake.wait_written(2);
        fake.release_one();
        assert_eq!(second.join().unwrap().unwrap(), 2);
    }

    #[test]
    fn a_request_whose_caller_gave_up_is_never_sent() {
        let fake = Fake::default();
        fake.hold.store(true, Ordering::Relaxed);
        let l = Arc::new(link(&fake));
        let first = std::thread::spawn({
            let l = l.clone();
            move || status(&l, 1, WAIT)
        });
        fake.wait_written(1);
        assert_eq!(status(&l, 2, Duration::from_millis(100)).unwrap_err().to_string(), "no reply from the keyboard");
        fake.release_one();
        assert_eq!(first.join().unwrap().unwrap(), 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fake.written().len(), 1);
    }

    #[test]
    fn unanswered_request_times_out() {
        let fake = Fake::default();
        fake.silent.store(true, Ordering::Relaxed);
        let l = link(&fake);
        let start = Instant::now();
        let e = status(&l, 1, Duration::from_millis(200)).unwrap_err();
        assert_eq!(e.to_string(), "no reply from the keyboard");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn sends_go_out_in_order() {
        let fake = Fake::default();
        fake.silent.store(true, Ordering::Relaxed);
        let l = link(&fake);
        for y in 0..5u8 {
            l.send(&[0xD5, 0x03, y]).unwrap();
        }
        fake.wait_written(5);
        let ys: Vec<u8> = fake.written().iter().map(|w| w[2]).collect();
        assert_eq!(ys, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn dropped_subscriptions_are_forgotten() {
        let fake = Fake::default();
        let l = link(&fake);
        drop(l.subscribe(|_| false));
        fake.push(&[0xFF, 1, 0, 0]);
        let end = Instant::now() + WAIT;
        while !l.shared.subscribers.lock().unwrap().is_empty() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(l.shared.subscribers.lock().unwrap().is_empty());
    }

    #[test]
    fn reconnects_after_the_keyboard_goes_away() {
        let fake = Fake::default();
        let l = link(&fake);
        let first = l.state().generation;

        fake.dead.store(true, Ordering::Relaxed);
        let end = Instant::now() + WAIT;
        while l.state().connected.is_some() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        let s = l.state();
        assert!(s.connected.is_none());
        assert!(s.error.is_some());
        assert!(l.send(&protocol::present()).is_err(), "sends fail fast while away");

        fake.dead.store(false, Ordering::Relaxed);
        l.wait_connected(Duration::from_secs(5)).unwrap();
        assert_eq!(l.state().generation, first + 1);
    }
}
