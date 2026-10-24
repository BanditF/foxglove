//! One owner for the keyboard's Raw HID interface, shared by everything that talks to it.
//!
//! A blocking reader hands reports to a writer waiting on a channel. Only outstanding requests
//! and reconnect retries use timed waits; an idle Windows connection has no timer.
//! HidDevice is Send, not Sync. The Windows transport uses a separate overlapped input handle
//! and a mutex-protected hidapi output handle (Windows permits multiple opens). hidapi exposes
//! no read cancellation, so input uses the same Win32 overlapped read with an extra stop event;
//! Drop/reconnect signal it, cancel and finish the pending I/O, then join before closing handles.
//! Other platforms retain bounded hidapi reads until they have a cancellable input backend.
//! Reports are dispatched on the writer thread:
//! - replies to our protocols (0xD5-0xD7 with byte 1 = command | 0x80) complete the request
//!   waiting for that (proto, command); unsolicited art STATUS also reaches subscribers;
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

/// Consecutive read errors, or consecutive write errors, that count as the keyboard being gone.
const MAX_ERRORS: u32 = 5;
const RETRY: Duration = Duration::from_secs(3);
/// Reports a subscriber may fall behind by; past that, new ones are dropped for it.
const SUBSCRIBER_BUFFER: usize = 1024;
/// After a request times out, its (proto, command) is held this long: a late reply in that
/// window is dropped instead of answering the next request of the same kind.
const LATE_REPLY_GRACE: Duration = Duration::from_millis(500);

pub type Report = [u8; REPORT];

/// An open Raw HID interface.
pub trait RawHid: Send + Sync {
    /// Blocks until input arrives or `interrupt` is called. Only the reader calls this.
    fn read(&self, buf: &mut Report) -> Result<usize>;
    /// Permanently wakes this connection's reader; safe before a read starts, too.
    fn interrupt(&self);
    /// Writes one output report (at most [`REPORT`] bytes; the rest is zero).
    fn write(&self, report: &[u8]) -> Result<()>;
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

/// Connection changes and unsolicited art STATUS reports. Solicited replies stay private.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    State(State),
    ArtStatus { generation: u64, report: Report },
}

struct EventSubscriber {
    notify: Box<dyn Fn(Event) + Send>,
    alive: Arc<AtomicBool>,
}

/// Keeps an event listener registered until dropped.
pub struct EventSubscription(Arc<AtomicBool>);

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

struct Request {
    key: (u8, u8),
    tx: Sender<Report>,
    /// When the caller stops waiting.
    deadline: Instant,
}

enum Message {
    Command(Command),
    Input(Report),
    ReadError(String),
    Wake,
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

type ReportListener = (Box<dyn Fn(u64, &Report) + Send>, Arc<AtomicBool>);

struct Shared {
    state: Mutex<State>,
    subscribers: Mutex<Vec<Subscriber>>,
    events: Mutex<Vec<EventSubscriber>>,
    reports: Mutex<Vec<ReportListener>>,
    stop: AtomicBool,
    reconnect: AtomicBool,
}

pub struct Link {
    commands: Mutex<Sender<Message>>,
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
            events: Mutex::new(Vec::new()),
            reports: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            reconnect: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("digitalis-link".into())
            .spawn({
                let tx = tx.clone();
                move || run(open, rx, tx, &thread_shared)
            })
            .expect("link thread");
        Link { commands: Mutex::new(tx), shared, thread: Some(thread) }
    }

    /// Re-enumerate after a device-interface change, preferring the current transport. No I/O on the caller.
    pub fn reconnect(&self) {
        self.shared.reconnect.store(true, Ordering::Relaxed);
        let _ = self.commands.lock().unwrap().send(Message::Wake);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
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

    /// Registers a nonblocking event sink and immediately sends the current state. The sink
    /// runs on the link thread: only enqueue events here, never perform I/O or call the link.
    pub fn subscribe_events(&self, notify: impl Fn(Event) + Send + 'static) -> EventSubscription {
        let state = self.shared.state.lock().unwrap();
        let mut events = self.shared.events.lock().unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        notify(Event::State(state.clone()));
        events.push(EventSubscriber { notify: Box::new(notify), alive: alive.clone() });
        EventSubscription(alive)
    }

    /// Observes incoming reports, including HELLO/STATUS replies, without consuming them.
    /// The callback runs on the reader thread and must only enqueue work.
    pub fn subscribe_reports(&self, notify: impl Fn(u64, &Report) + Send + 'static) -> EventSubscription {
        let alive = Arc::new(AtomicBool::new(true));
        self.shared.reports.lock().unwrap().push((Box::new(notify), alive.clone()));
        EventSubscription(alive)
    }

    fn queue(&self, command: Command) -> Result<()> {
        if self.shared.state.lock().unwrap().connected.is_none() {
            return Err(anyhow!("keyboard not connected"));
        }
        self.commands.lock().unwrap().send(Message::Command(command)).map_err(|_| anyhow!("link stopped"))
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
        let _ = self.commands.lock().unwrap().send(Message::Wake);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn set_state(shared: &Shared, f: impl FnOnce(&mut State)) {
    let mut state = shared.state.lock().unwrap();
    let before = state.clone();
    f(&mut state);
    if state.connected != before.connected || state.generation != before.generation {
        publish(shared, Event::State(state.clone()));
    }
}

fn publish(shared: &Shared, event: Event) {
    shared.events.lock().unwrap().retain(|s| {
        if !s.alive.load(Ordering::Relaxed) {
            return false;
        }
        (s.notify)(event.clone());
        true
    });
}

/// Drops whatever is queued; dropping a request's sender fails it right away.
fn discard(commands: &Receiver<Message>) {
    while commands.try_recv().is_ok() {}
}

fn run(mut open: Opener, commands: Receiver<Message>, tx: Sender<Message>, shared: &Shared) {
    while !shared.stop.load(Ordering::Relaxed) {
        shared.reconnect.store(false, Ordering::Relaxed);
        let (dev, connected) = match open() {
            Ok(c) => c,
            Err(e) => {
                set_state(shared, |s| s.error = Some(e.to_string()));
                let end = Instant::now() + RETRY;
                while Instant::now() < end && !shared.stop.load(Ordering::Relaxed)
                    && !shared.reconnect.load(Ordering::Relaxed) {
                    discard(&commands);
                    std::thread::park_timeout(end.saturating_duration_since(Instant::now()));
                }
                continue;
            }
        };
        set_state(shared, |s| {
            s.generation += 1;
            s.connected = Some(connected);
            s.error = None;
        });

        let error = serve(Arc::from(dev), &commands, &tx, shared);
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
    /// Keys whose request timed out, held until the time given (see [`LATE_REPLY_GRACE`]).
    held: HashMap<(u8, u8), Instant>,
}

/// Talks to one connection until it fails (returns why) or the link stops (returns None).
fn serve(dev: Arc<dyn RawHid>, commands: &Receiver<Message>, tx: &Sender<Message>, shared: &Shared) -> Option<String> {
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let finished_ref = &finished;
        let reading = dev.clone();
        let reader = scope.spawn(move || {
            let mut errors = 0;
            let mut buf = [0; REPORT];
            while !finished_ref.load(Ordering::Relaxed)
                && !shared.stop.load(Ordering::Relaxed)
                && !shared.reconnect.load(Ordering::Relaxed) {
                match reading.read(&mut buf) {
                    Ok(n) => {
                        errors = 0;
                        if n > 0 && tx.send(Message::Input(buf)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        errors += 1;
                        if errors >= MAX_ERRORS {
                            let _ = tx.send(Message::ReadError(format!("lost keyboard: {e}")));
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        });
        let error = write_loop(dev.as_ref(), commands, shared);
        finished.store(true, Ordering::Relaxed);
        dev.interrupt();
        // interrupt is latched: even a reader not yet scheduled cannot miss it.
        let _ = reader.join();
        error
    })
}

impl Requests {
    fn expire(&mut self, now: Instant) {
        self.in_flight.retain(|key, r| {
            if r.deadline > now {
                return true;
            }
            self.held.insert(*key, r.deadline + LATE_REPLY_GRACE);
            false
        });
        self.held.retain(|_, until| *until > now);
        self.waiting.retain(|c| c.reply.as_ref().is_none_or(|r| r.deadline > now));
    }

    fn next_deadline(&self) -> Option<Instant> {
        let waiting = self.waiting.iter().filter_map(|c| c.reply.as_ref());
        let held = waiting.clone().filter_map(|r| self.held.get(&r.key).copied());
        self.in_flight.values().map(|r| r.deadline)
            .chain(waiting.map(|r| r.deadline))
            .chain(held)
            .min()
    }
}

fn write_loop(dev: &dyn RawHid, commands: &Receiver<Message>, shared: &Shared) -> Option<String> {
    let mut requests = Requests::default();
    let mut write_errors = 0;
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return None;
        }
        if shared.reconnect.load(Ordering::Relaxed) {
            return Some("device interfaces changed".into());
        }
        requests.expire(Instant::now());
        let queued: Vec<_> = requests.waiting.drain(..).collect();
        for c in queued {
            if let Some(r) = &c.reply {
                if r.deadline <= Instant::now() {
                    continue; // its caller has given up: never send it
                }
                if requests.in_flight.contains_key(&r.key) || requests.held.contains_key(&r.key) {
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
        let message = match requests.next_deadline() {
            Some(deadline) => match commands.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return None,
            },
            None => match commands.recv() {
                Ok(m) => m,
                Err(_) => return None,
            },
        };
        match message {
            Message::Command(c) => requests.waiting.push_back(c),
            Message::Input(r) => dispatch(&r, &mut requests, shared),
            Message::ReadError(e) => return Some(e),
            Message::Wake => (),
        }
    }
}

fn dispatch(r: &Report, requests: &mut Requests, shared: &Shared) {
    requests.expire(Instant::now());
    let generation = shared.state.lock().unwrap().generation;
    shared.reports.lock().unwrap().retain(|(notify, alive)| {
        if !alive.load(Ordering::Relaxed) { return false; }
        notify(generation, r);
        true
    });
    if matches!(r[0], protocol::PANEL | protocol::ART | protocol::SETTINGS) && r[1] & 0x80 != 0 {
        // A reply goes to its request. Unasked art STATUS may be a firmware notification;
        // other replies stay private. A key held after a timeout drops the late reply.
        let key = (r[0], r[1] & 0x7f);
        if requests.held.remove(&key).is_some() {
            return;
        }
        if let Some(req) = requests.in_flight.remove(&key) {
            let _ = req.tx.send(*r);
            return;
        }
        if key != (protocol::ART, protocol::art::STATUS) {
            return;
        }
    }
    if r[0] == protocol::ART && r[1] & 0x7f == protocol::art::STATUS {
        let generation = shared.state.lock().unwrap().generation;
        publish(shared, Event::ArtStatus { generation, report: *r });
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

    #[cfg(windows)]
    struct Device {
        writer: Mutex<Keyboard>,
        reader: crate::hid::Input,
    }

    #[cfg(windows)]
    impl RawHid for Device {
        fn read(&self, buf: &mut Report) -> Result<usize> {
            self.reader.read(buf)
        }

        fn interrupt(&self) {
            self.reader.interrupt();
        }

        fn write(&self, report: &[u8]) -> Result<()> {
            Transport::send(&*self.writer.lock().unwrap(), report)
        }
    }

    // hidapi has no portable cancellation API. Keep the existing single-handle bounded
    // reads off Windows rather than leave a thread blocked forever at shutdown.
    #[cfg(not(windows))]
    struct Device {
        keyboard: Mutex<Keyboard>,
        interrupted: AtomicBool,
    }

    #[cfg(not(windows))]
    impl RawHid for Device {
        fn read(&self, buf: &mut Report) -> Result<usize> {
            if self.interrupted.load(Ordering::Relaxed) { return Ok(0); }
            self.keyboard.lock().unwrap().read_timeout(buf, 10)
        }

        fn interrupt(&self) {
            self.interrupted.store(true, Ordering::Relaxed);
        }

        fn write(&self, report: &[u8]) -> Result<()> {
            Transport::send(&*self.keyboard.lock().unwrap(), report)
        }
    }

    impl Link {
        /// A link to the first keyboard matching `m` that answers HELLO.
        pub fn hid(m: Match) -> Result<Link> {
            let mut api = hidapi::HidApi::new()?;
            Ok(Link::start(Box::new(move || {
                let kb = Keyboard::open_matching(&mut api, &m)?;
                let connected = Connected { product: kb.product.clone(), hello: kb.hello };
                #[cfg(windows)]
                let device = Device { reader: kb.input()?, writer: Mutex::new(kb) };
                #[cfg(not(windows))]
                let device = Device { keyboard: Mutex::new(kb), interrupted: AtomicBool::new(false) };
                Ok((Box::new(device) as Box<dyn RawHid>, connected))
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
        inbox: Arc<(Mutex<VecDeque<Report>>, std::sync::Condvar)>,
        interrupted: Arc<AtomicBool>,
        reads: Arc<std::sync::atomic::AtomicUsize>,
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
            self.inbox.0.lock().unwrap().push_back(report(bytes));
            self.inbox.1.notify_all();
        }

        /// Lets the oldest held reply through.
        fn release_one(&self) {
            let r = self.held.lock().unwrap().pop_front().expect("a held reply");
            self.inbox.0.lock().unwrap().push_back(r);
            self.inbox.1.notify_all();
        }

        fn disconnect(&self) {
            let _inbox = self.inbox.0.lock().unwrap();
            self.dead.store(true, Ordering::Relaxed);
            self.inbox.1.notify_all();
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
        fn read(&self, buf: &mut Report) -> Result<usize> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let mut inbox = self.inbox.0.lock().unwrap();
            loop {
                if self.interrupted.load(Ordering::Relaxed) { return Ok(0); }
                if self.dead.load(Ordering::Relaxed) { return Err(anyhow!("device gone")); }
                if let Some(r) = inbox.pop_front() { *buf = r; return Ok(REPORT); }
                inbox = self.inbox.1.wait(inbox).unwrap();
            }
        }

        fn interrupt(&self) {
            let _inbox = self.inbox.0.lock().unwrap();
            self.interrupted.store(true, Ordering::Relaxed);
            self.inbox.1.notify_all();
        }

        fn write(&self, out: &[u8]) -> Result<()> {
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
                    self.inbox.0.lock().unwrap().push_back(reply);
                    self.inbox.1.notify_all();
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
            let mut connection = fake.clone();
            connection.interrupted = Arc::new(AtomicBool::new(false));
            Ok((Box::new(connection) as Box<dyn RawHid>, c))
        }));
        l.wait_connected(WAIT).unwrap();
        l
    }

    /// An art STATUS request tagged with `id` (the firmware ignores byte 2).
    fn status(l: &Link, id: u8, timeout: Duration) -> Result<u8> {
        l.request(&[protocol::ART, art::STATUS, id], protocol::ART, art::STATUS, timeout).map(|r| r[2])
    }

    #[test]
    fn idle_reader_stays_blocked_and_drop_wakes_it() {
        let fake = Fake::default();
        let l = link(&fake);
        let end = Instant::now() + WAIT;
        while fake.reads.load(Ordering::Relaxed) == 0 && Instant::now() < end {
            std::thread::yield_now();
        }
        assert_eq!(fake.reads.load(Ordering::Relaxed), 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fake.reads.load(Ordering::Relaxed), 1, "idle input must not poll");
        let (tx, rx) = mpsc::channel();
        let dropping = std::thread::spawn(move || { drop(l); tx.send(()).unwrap(); });
        rx.recv_timeout(WAIT).expect("Drop must wake the blocked reader");
        dropping.join().unwrap();
    }

    #[test]
    fn reconnect_wakes_idle_input_and_fails_outstanding_requests() {
        let fake = Fake::default();
        fake.hold.store(true, Ordering::Relaxed);
        let l = Arc::new(link(&fake));
        let first = l.state().generation;
        let pending = std::thread::spawn({
            let l = l.clone();
            move || status(&l, 1, WAIT)
        });
        fake.wait_written(1);
        l.reconnect();
        assert_eq!(pending.join().unwrap().unwrap_err().to_string(), "keyboard disconnected");
        let end = Instant::now() + WAIT;
        while l.state().generation == first && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(l.state().generation, first + 1);
        assert!(l.state().connected.is_some());
    }

    #[test]
    fn held_key_expires_and_wakes_a_waiting_request_without_input() {
        let fake = Fake::default();
        fake.silent.store(true, Ordering::Relaxed);
        let l = Arc::new(link(&fake));
        assert!(status(&l, 1, Duration::from_millis(50)).is_err());
        let second = std::thread::spawn({
            let l = l.clone();
            move || status(&l, 2, WAIT)
        });
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fake.written().len(), 1, "the timed-out key stays quarantined");
        fake.wait_written(2);
        assert_eq!(fake.written().len(), 2, "grace expiry must wake the writer");
        fake.push(&[protocol::ART, 0x80 | art::STATUS, 2]);
        assert_eq!(second.join().unwrap().unwrap(), 2);
    }

    #[test]
    fn expiry_uses_the_request_deadline_and_cleans_held_keys_on_touch() {
        let now = Instant::now();
        let key = (protocol::ART, art::STATUS);
        let (tx, _rx) = mpsc::channel();
        let mut requests = Requests::default();
        requests.in_flight.insert(key, Request { key, tx, deadline: now });
        requests.expire(now + Duration::from_millis(100));
        assert!(requests.in_flight.is_empty());
        assert_eq!(requests.held[&key], now + LATE_REPLY_GRACE);
        assert_eq!(requests.next_deadline(), None, "an idle held key needs no timer");
        requests.expire(now + LATE_REPLY_GRACE);
        assert!(requests.held.is_empty());
    }

    #[test]
    fn report_observers_see_replies_without_consuming_them_and_drop_unsubscribes() {
        let fake = Fake::default();
        let l = link(&fake);
        let (tx, rx) = mpsc::channel();
        let subscription = l.subscribe_reports(move |generation, r| {
            if r[0] == protocol::ART { let _ = tx.send((generation, *r)); }
        });
        assert_eq!(status(&l, 42, WAIT).unwrap(), 42);
        let (generation, reply) = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(generation, l.state().generation);
        assert_eq!(reply[2], 42);
        drop(subscription);
        assert_eq!(status(&l, 43, WAIT).unwrap(), 43);
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn arrival_during_failed_open_skips_retry_delay() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut first = true;
        let l = Link::start(Box::new(move || {
            if first {
                first = false;
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                return Err(anyhow!("not present"));
            }
            Ok((Box::new(Fake::default()), Connected { product: "USB".into(), hello: None }))
        }));
        entered_rx.recv_timeout(WAIT).unwrap();
        l.reconnect();
        release_tx.send(()).unwrap();
        assert_eq!(l.wait_connected(WAIT).unwrap().product, "USB");
    }

    #[test]
    fn transport_change_replaces_a_handle_that_still_reads_successfully() {
        let fake = Fake::default();
        let l = link(&fake);
        let first = l.state().generation;
        let (tx, rx) = mpsc::channel();
        let _subscription = l.subscribe_events(move |event| { let _ = tx.send(event); });
        rx.recv_timeout(WAIT).unwrap();
        l.reconnect();
        let Event::State(away) = rx.recv_timeout(WAIT).unwrap() else { panic!("state") };
        assert!(away.connected.is_none());
        let Event::State(back) = rx.recv_timeout(WAIT).unwrap() else { panic!("state") };
        assert!(back.connected.is_some());
        assert_eq!(back.generation, first + 1);
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
    fn unsolicited_art_status_and_other_protocols_pass() {
        let fake = Fake::default();
        let l = link(&fake);
        let everything = l.subscribe(|_| true);
        fake.push(&[0xD5, 0x81, 9]); // an unasked HELLO stays private
        fake.push(&[0xD6, 0x85, 9]); // future firmware may announce art STATUS
        fake.push(&[0x01, 0x81, 7]); // someone else's protocol with the high bit set
        assert_eq!(&everything.recv_timeout(WAIT).unwrap()[..3], &[0xD6, 0x85, 9]);
        assert_eq!(&everything.recv_timeout(WAIT).unwrap()[..3], &[0x01, 0x81, 7]);
        assert!(everything.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn state_events_include_initial_state_disconnect_and_new_generation() {
        let fake = Fake::default();
        let l = link(&fake);
        let (tx, rx) = mpsc::channel();
        let subscription = l.subscribe_events(move |event| { let _ = tx.send(event); });
        let first = l.state();
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Event::State(first.clone()));
        set_state(&l.shared, |s| s.connected = None);
        let Event::State(away) = rx.recv_timeout(WAIT).unwrap() else { panic!("state") };
        assert!(away.connected.is_none());
        set_state(&l.shared, |s| {
            s.generation += 1;
            s.connected = first.connected;
        });
        let Event::State(back) = rx.recv_timeout(WAIT).unwrap() else { panic!("state") };
        assert_eq!(back.generation, first.generation + 1);
        assert!(back.connected.is_some());
        drop(subscription);
        set_state(&l.shared, |s| s.connected = None);
        assert!(rx.try_recv().is_err());
        assert!(l.shared.events.lock().unwrap().is_empty());
    }

    #[test]
    fn status_events_exclude_solicited_and_quarantined_replies() {
        let fake = Fake::default();
        let l = link(&fake);
        let (tx, rx) = mpsc::channel();
        let _subscription = l.subscribe_events(move |event| { let _ = tx.send(event); });
        assert!(matches!(rx.recv_timeout(WAIT).unwrap(), Event::State(_)));
        assert_eq!(status(&l, 7, WAIT).unwrap(), 7);
        assert!(rx.try_recv().is_err());
        let report = report(&[protocol::ART, 0x80 | art::STATUS, 9]);
        let mut requests = Requests::default();
        requests.held.insert((protocol::ART, art::STATUS), Instant::now() + WAIT);
        dispatch(&report, &mut requests, &l.shared);
        assert!(rx.try_recv().is_err());
        dispatch(&report, &mut requests, &l.shared);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Event::ArtStatus { generation: l.state().generation, report });
        let report = self::report(&[protocol::ART, art::STATUS, 0]);
        dispatch(&report, &mut requests, &l.shared);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Event::ArtStatus { generation: l.state().generation, report });
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
    fn a_late_reply_does_not_answer_the_next_request() {
        let fake = Fake::default();
        fake.hold.store(true, Ordering::Relaxed);
        let l = link(&fake);
        // The first request times out; its reply only arrives afterwards.
        assert!(status(&l, 1, Duration::from_millis(100)).is_err());
        fake.wait_written(1);
        let second = std::thread::spawn({
            let l = Arc::new(l);
            let fake = fake.clone();
            move || {
                std::thread::sleep(Duration::from_millis(50));
                fake.release_one(); // the late reply to request 1
                let got = status(&l, 2, WAIT);
                fake.wait_written(2);
                (got, l)
            }
        });
        std::thread::sleep(Duration::from_millis(700));
        fake.release_one(); // the real reply to request 2
        let (got, _l) = second.join().unwrap();
        assert_eq!(got.unwrap(), 2);
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
        let (tx, rx) = mpsc::channel();
        let _subscription = l.subscribe_events(move |event| { let _ = tx.send(event); });
        assert!(matches!(rx.recv_timeout(WAIT).unwrap(), Event::State(_)));

        fake.disconnect();
        let Event::State(s) = rx.recv_timeout(WAIT).unwrap() else { panic!("state") };
        assert!(s.connected.is_none());
        assert!(s.error.is_some());
        assert!(l.send(&protocol::present()).is_err(), "sends fail fast while away");

        fake.dead.store(false, Ordering::Relaxed);
        let Event::State(s) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else { panic!("state") };
        assert!(s.connected.is_some());
        assert_eq!(s.generation, first + 1);
    }
}
