//! One owner for the keyboard's Raw HID interface, shared by everything that talks to it.
//!
//! A single thread reads with a short timeout, hands each input report to whoever wants it, and
//! writes queued reports between reads, so reads and writes never race on the handle:
//! - replies (byte 1 = command | 0x80) complete the request waiting for that (proto, command);
//! - everything else goes to the subscribers whose filter matches (the overlay's 0xFF/0xF1, a
//!   debug tap, later integrations).
//!
//! The firmware's replies carry no sequence number, so there is one request in flight per
//! (proto, command); a second one for the same pair replaces the first, which then times out.
//! The link reconnects by itself and fails waiting requests while the keyboard is away.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::protocol::{Hello, REPORT};
use crate::Transport;

/// How long one read waits; a queued write waits at most this long.
const READ_TIMEOUT_MS: i32 = 10;
/// Consecutive read or write errors that count as the keyboard being gone.
const MAX_ERRORS: u32 = 5;
const RETRY: Duration = Duration::from_secs(3);

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
    pub hello: Hello,
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

struct Command {
    report: Vec<u8>,
    reply: Option<((u8, u8), Sender<Report>)>,
}

type Filter = Box<dyn Fn(&[u8]) -> bool + Send>;

struct Subscriber {
    filter: Filter,
    tx: Sender<Report>,
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

    /// Input reports that aren't replies and match `filter`, for as long as the receiver lives.
    pub fn subscribe(&self, filter: impl Fn(&[u8]) -> bool + Send + 'static) -> Receiver<Report> {
        let (tx, rx) = mpsc::channel();
        self.shared.subscribers.lock().unwrap().push(Subscriber { filter: Box::new(filter), tx });
        rx
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
        self.queue(Command { report: report.to_vec(), reply: Some(((proto, cmd), tx)) })?;
        rx.recv_timeout(timeout).map_err(|e| match e {
            RecvTimeoutError::Timeout => anyhow!("no reply from the keyboard"),
            RecvTimeoutError::Disconnected => anyhow!("keyboard disconnected"),
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

fn run(mut open: Opener, commands: Receiver<Command>, shared: &Shared) {
    let mut pending: HashMap<(u8, u8), Sender<Report>> = HashMap::new();
    while !shared.stop.load(Ordering::Relaxed) {
        let (mut dev, connected) = match open() {
            Ok(c) => c,
            Err(e) => {
                set_state(shared, |s| s.error = Some(e.to_string()));
                // Queued commands can't go anywhere; dropping them fails their requests now.
                let end = Instant::now() + RETRY;
                while Instant::now() < end && !shared.stop.load(Ordering::Relaxed) {
                    while commands.try_recv().is_ok() {}
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

        let error = serve(dev.as_mut(), &commands, &mut pending, shared);
        pending.clear(); // fails the requests still waiting
        set_state(shared, |s| {
            s.connected = None;
            s.error = error;
        });
    }
}

/// Talks to one connection until it fails (returns why) or the link stops (returns None).
fn serve(
    dev: &mut dyn RawHid,
    commands: &Receiver<Command>,
    pending: &mut HashMap<(u8, u8), Sender<Report>>,
    shared: &Shared,
) -> Option<String> {
    let mut errors = 0;
    let mut buf = [0u8; REPORT];
    while !shared.stop.load(Ordering::Relaxed) {
        match dev.read(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => errors = 0,
            Ok(_) => {
                errors = 0;
                dispatch(&buf, pending, shared);
            }
            Err(e) => {
                errors += 1;
                if errors >= MAX_ERRORS {
                    return Some(format!("lost keyboard: {e}"));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        while let Ok(c) = commands.try_recv() {
            if let Some((key, tx)) = c.reply {
                pending.insert(key, tx);
            }
            if let Err(e) = dev.write(&c.report) {
                errors += 1;
                if errors >= MAX_ERRORS {
                    return Some(format!("lost keyboard: {e}"));
                }
            }
        }
    }
    None
}

fn dispatch(r: &Report, pending: &mut HashMap<(u8, u8), Sender<Report>>, shared: &Shared) {
    if r[1] & 0x80 != 0 {
        if let Some(tx) = pending.remove(&(r[0], r[1] & 0x7f)) {
            let _ = tx.send(*r);
            return;
        }
    }
    // Subscribers whose receiver is gone drop out here.
    shared.subscribers.lock().unwrap().retain(|s| !(s.filter)(r) || s.tx.send(*r).is_ok());
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
    use crate::protocol::{self, panel};
    use std::collections::VecDeque;

    /// A keyboard in memory: answers HELLO and art STATUS, can send its own reports, can die.
    #[derive(Clone, Default)]
    struct Fake {
        inbox: Arc<Mutex<VecDeque<Report>>>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        dead: Arc<AtomicBool>,
        silent: Arc<AtomicBool>,
    }

    impl Fake {
        fn push(&self, bytes: &[u8]) {
            let mut r = [0u8; REPORT];
            r[..bytes.len()].copy_from_slice(bytes);
            self.inbox.lock().unwrap().push_back(r);
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

        fn write(&mut self, report: &[u8]) -> Result<()> {
            if self.dead.load(Ordering::Relaxed) {
                return Err(anyhow!("device gone"));
            }
            self.written.lock().unwrap().push(report.to_vec());
            if !self.silent.load(Ordering::Relaxed) && report.len() >= 2 {
                // Another app's traffic arrives first, then the reply.
                self.push(&[0xFF, 1, 0, 2]);
                self.push(&[report[0], 0x80 | report[1], 42]);
            }
            Ok(())
        }
    }

    fn link(fake: &Fake) -> Link {
        let fake = fake.clone();
        let opens = Arc::new(Mutex::new(0));
        Link::start(Box::new(move || {
            if fake.dead.load(Ordering::Relaxed) {
                return Err(anyhow!("no keyboard"));
            }
            *opens.lock().unwrap() += 1;
            let c = Connected { product: "Fake".into(), hello: Hello::default() };
            Ok((Box::new(fake.clone()) as Box<dyn RawHid>, c))
        }))
    }

    #[test]
    fn replies_reach_the_request_and_other_reports_the_subscriber() {
        let fake = Fake::default();
        let l = link(&fake);
        l.wait_connected(Duration::from_secs(1)).unwrap();
        let layers = l.subscribe(|r| r[0] == 0xFF);
        let r = l.request(&protocol::hello(), protocol::PANEL, panel::HELLO, Duration::from_secs(1)).unwrap();
        assert_eq!((r[0], r[1], r[2]), (0xD5, 0x81, 42));
        let layer = layers.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(&layer[..4], &[0xFF, 1, 0, 2]);
        // Replies never leak to subscribers.
        assert!(l.subscribe(|_| true).recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn unanswered_request_times_out() {
        let fake = Fake::default();
        fake.silent.store(true, Ordering::Relaxed);
        let l = link(&fake);
        l.wait_connected(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        let e = l.request(&protocol::art_status(), protocol::ART, 0x05, Duration::from_millis(200)).unwrap_err();
        assert_eq!(e.to_string(), "no reply from the keyboard");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn sends_go_out_in_order() {
        let fake = Fake::default();
        fake.silent.store(true, Ordering::Relaxed);
        let l = link(&fake);
        l.wait_connected(Duration::from_secs(1)).unwrap();
        for y in 0..5u8 {
            l.send(&[0xD5, 0x03, y]).unwrap();
        }
        std::thread::sleep(Duration::from_millis(100));
        let ys: Vec<u8> = fake.written.lock().unwrap().iter().map(|w| w[2]).collect();
        assert_eq!(ys, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn reconnects_after_the_keyboard_goes_away() {
        let fake = Fake::default();
        let l = link(&fake);
        let first = l.wait_connected(Duration::from_secs(1)).map(|_| l.state().generation).unwrap();

        fake.dead.store(true, Ordering::Relaxed);
        let end = Instant::now() + Duration::from_secs(2);
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
