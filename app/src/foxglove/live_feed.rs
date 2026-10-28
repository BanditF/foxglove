//! The live keyboard feed (#499, epic #498): a paired wallpaper on this PC gets key positions,
//! the layer, the right screen's art, battery and connection, as `foxglove-live` v1
//! (`live_wire`) on a WebSocket at `127.0.0.1:<port>` (8768 by default).
//!
//! - **Opt-in:** off by default. While it's off nothing listens and nothing subscribes to keys.
//! - **Who:** a `file://` wallpaper (Origin `null`; #320) or a local program, and only with this
//!   install's token. Web pages are refused by their origin.
//! - **When nothing is sent:** Foxglove paused, the session locked or the display off, a
//!   full-screen program in front, or the wallpaper itself paused. The wallpaper is told once
//!   (`quiet`), then hears nothing until `live`.
//! - **Events only:** one thread blocks in `accept`, one per wallpaper blocks reading it, and the
//!   hub blocks on its channel. Key events are subscribed only while an unpaused wallpaper is
//!   connected and the gates are open; a burst is held at most 16 ms ([`wire::BATCH_MS`]) so a
//!   wallpaper gets at most 60 frames a second. The full-screen check runs when the program in
//!   front changes and at the start of a burst of typing, never on a clock.
//! - **Telling the wallpaper:** the feed's port goes out as the `foxglove_live` property through
//!   Wallpaper Engine's command line (`wallpaper_sync`) to each wallpaper whose `foxglove.json`
//!   supports `foxglove_live` (#562), so it connects when it comes up instead of retrying on a
//!   timer. The token never goes on a command line. Any wallpaper or program may be a client
//!   (docs/live-feed.md).
//!
//! Later topics (PC stats, notifications) are new `t` values in `live_wire`; the gates, the
//! pairing and the hub stay as they are.

use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_devices::facts::{Facts, Value};
use foxglove_devices::DeviceEvent;
use foxglove_plugin::ws;
use serde::{Deserialize, Serialize};

use super::live_wire::{self as wire, Message, Why};

pub const DEFAULT_PORT: u16 = 8768;
const SECTION: &str = "live_wallpaper";
/// How long a connection may take to send its upgrade request.
const HANDSHAKE_LIMIT: Duration = Duration::from_secs(5);
/// How long one write to a wallpaper may block before it's dropped.
const WRITE_LIMIT: Duration = Duration::from_secs(1);
/// Connections still in their handshake at once; more are closed at once.
const MAX_PENDING: usize = 8;
/// Key events waiting for the hub at most; more are dropped (a stalled wallpaper write).
const KEYS_IN_FLIGHT: usize = 256;
static KEYS_QUEUED: AtomicUsize = AtomicUsize::new(0);
/// Typing after this much quiet is a new burst: the full-screen check runs again.
const BURST_GAP: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveSettings {
    pub enabled: bool,
    pub port: u16,
    /// The pairing token the wallpaper presents; made on first use.
    pub token: String,
}

impl Default for LiveSettings {
    fn default() -> Self {
        LiveSettings {
            enabled: false,
            port: DEFAULT_PORT,
            token: String::new(),
        }
    }
}

static SETTINGS: Mutex<Option<LiveSettings>> = Mutex::new(None);

/// The settings, read from the store once and then kept in step by [`save`].
pub fn settings() -> LiveSettings {
    SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(|| super::store::try_load(SECTION).unwrap_or_default())
        .clone()
}

fn save(s: &LiveSettings) -> Result<(), String> {
    super::store::save(SECTION, s).map_err(|e| e.to_string())?;
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(s.clone());
    Ok(())
}

// ------------------------------------------------------------------------------ the hub

/// Board geometry and the matrix position behind each of its keys.
type Geometry = (wire::Board, Vec<(usize, usize)>);

enum Msg {
    Join(u64, TcpStream),
    Leave(u64),
    Paused(u64, bool),
    Key {
        row: u16,
        col: u16,
        pressed: bool,
        generation: u64,
    },
    /// A `keyboard.*` fact or the art changed.
    State,
    /// The program in front changed.
    Foreground,
    Board,
    /// Foxglove paused, or the session went away, or either came back.
    Gates,
    Stop,
}

struct Client {
    id: u64,
    stream: TcpStream,
    paused: bool,
}

/// Where hub writes go: a wallpaper's stream, or a list in tests.
trait Out {
    fn send(&mut self, id: u64, text: &str) -> bool;
}

struct Streams<'a>(&'a mut [Client]);

impl Out for Streams<'_> {
    fn send(&mut self, id: u64, text: &str) -> bool {
        self.0
            .iter_mut()
            .find(|c| c.id == id)
            .is_some_and(|c| ws::write_text(&mut c.stream, text).is_ok())
    }
}

/// The hub's decisions, apart from its sockets and threads.
#[derive(Default)]
struct Hub {
    /// Connected wallpapers and whether each is paused.
    clients: Vec<(u64, bool)>,
    state: wire::State,
    board: Option<Geometry>,
    quiet: Option<Why>,
    batch: wire::Batch,
}

impl Hub {
    fn listeners(&self) -> impl Iterator<Item = u64> + '_ {
        self.clients.iter().filter(|(_, p)| !p).map(|(id, _)| *id)
    }

    /// Whether key events are wanted at all: someone listens and nothing but full screen holds
    /// the feed (full screen is checked at a burst's start, so it needs the keys).
    fn wants_keys(&self) -> bool {
        self.listeners().next().is_some() && matches!(self.quiet, None | Some(Why::Fullscreen))
    }

    fn hello(&self) -> Message {
        Message::Hello {
            v: wire::VERSION,
            topics: wire::TOPICS,
            board: self.board.as_ref().map(|(b, _)| b.clone()),
            state: self.state.clone(),
            quiet: self.quiet,
        }
    }

    fn broadcast(&self, out: &mut impl Out, m: &Message) -> Vec<u64> {
        let text = m.json();
        self.listeners()
            .filter(|&id| !out.send(id, &text))
            .collect()
    }

    fn join(&mut self, id: u64, out: &mut impl Out) -> bool {
        if self.clients.len() >= wire::MAX_CLIENTS {
            return false;
        }
        self.clients.push((id, false));
        out.send(id, &self.hello().json())
    }

    fn leave(&mut self, id: u64) {
        self.clients.retain(|(c, _)| *c != id);
    }

    /// A wallpaper paused or played again; coming back it gets everything afresh.
    fn paused(&mut self, id: u64, paused: bool, out: &mut impl Out) -> bool {
        let Some(c) = self.clients.iter_mut().find(|(c, _)| *c == id) else {
            return true;
        };
        let was = std::mem::replace(&mut c.1, paused);
        was == paused || paused || out.send(id, &self.hello().json())
    }

    /// The gates now; the wallpapers hear only a change.
    fn gates(&mut self, quiet: Option<Why>, out: &mut impl Out) -> Vec<u64> {
        if quiet == self.quiet {
            return Vec::new();
        }
        self.quiet = quiet;
        self.batch = wire::Batch::default();
        match quiet {
            Some(why) => self.broadcast(out, &Message::Quiet { why }),
            None => {
                let mut dead = self.broadcast(out, &Message::Live);
                dead.extend(self.broadcast(out, &Message::State(self.state.clone())));
                dead
            }
        }
    }

    fn set_state(&mut self, state: wire::State, out: &mut impl Out) -> Vec<u64> {
        if state == self.state {
            return Vec::new();
        }
        self.state = state;
        if self.quiet.is_some() {
            return Vec::new();
        }
        self.broadcast(out, &Message::State(self.state.clone()))
    }

    fn set_board(&mut self, board: Option<Geometry>, out: &mut impl Out) -> Vec<u64> {
        if board == self.board {
            return Vec::new();
        }
        self.board = board;
        if self.quiet.is_some() {
            return Vec::new();
        }
        let board = self.board.as_ref().map(|(b, _)| b.clone());
        self.broadcast(out, &Message::Board { board })
    }

    /// A key event; true when it started a batch.
    fn key(&mut self, row: u16, col: u16, pressed: bool, now_ms: u64) -> bool {
        if self.quiet.is_some() {
            return false;
        }
        let positions = self.board.as_ref().map_or(&[][..], |(_, p)| p.as_slice());
        match wire::key_index(positions, row, col) {
            Some(i) => self.batch.push(i, pressed, now_ms),
            None => false,
        }
    }

    fn flush(&mut self, out: &mut impl Out) -> Vec<u64> {
        match self.batch.take() {
            Some(m) if self.quiet.is_none() => self.broadcast(out, &m),
            _ => Vec::new(),
        }
    }
}

struct Running {
    tx: Sender<Msg>,
    stop: Arc<AtomicBool>,
    port: u16,
    /// The accept thread's supervisor, joined on stop so the port is free before it's bound
    /// again.
    accept: Option<std::thread::JoinHandle<()>>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// The board as the layer overlay last loaded it, kept while the feed is off.
static GEOMETRY: Mutex<Option<Geometry>> = Mutex::new(None);
/// What the settings show: listening, connected, or why it can't.
static STATUS: Mutex<String> = Mutex::new(String::new());
static CONNECTED: AtomicUsize = AtomicUsize::new(0);

fn send(msg: Msg) {
    if let Some(r) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = r.tx.send(msg);
    }
}

fn set_status(text: impl Into<String>) {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = text.into();
}

/// The layer overlay loaded a board's layout (`connection_flow`, `settings_sync`).
pub fn layout(layout: &crate::protocols::KeyboardLayout) {
    let name = super::boards::current().map_or_else(|| layout.name.clone(), |b| b.name);
    let keys = layout
        .keys
        .iter()
        .map(|k| wire::key_rect(k.x, k.y, k.w, k.h, k.r))
        .collect();
    let positions = layout.keys.iter().map(|k| (k.row, k.col)).collect();
    *GEOMETRY.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((wire::Board { name, keys }, positions));
    send(Msg::Board);
}

/// The right screen's art may have changed (`art_service`).
pub fn art_changed() {
    send(Msg::State);
}

/// Foxglove paused or resumed (`pause`).
pub fn pause_changed() {
    send(Msg::Gates);
}

/// Starts the feed if the user turned it on. `FOXGLOVE_DISABLE=live` leaves it out.
pub fn start() {
    if super::repaint_trace::disabled("live") {
        return;
    }
    let s = settings();
    if s.enabled {
        if let Err(e) = listen(&s) {
            set_status(e);
        }
    }
}

/// Stops the feed: the wallpapers are let go and the port is closed.
pub fn stop() {
    let Some(r) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    r.stop.store(true, Ordering::SeqCst);
    let _ = r.tx.send(Msg::Stop);
    // Wakes the accept thread, which then sees the stop.
    let woke = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], r.port)),
        Duration::from_millis(500),
    );
    // With the wake-up delivered, the accept thread ends at once and drops the listener. Not
    // while its supervisor waits out a restart's backoff (up to a minute, #587): the next
    // attempt takes the wake-up and ends then.
    let backing_off = super::services::status()
        .get("live-accept")
        .is_some_and(|s| !s.running && !s.stopped);
    if let Some(accept) = r.accept.filter(|_| woke.is_ok() && !backing_off) {
        let _ = accept.join();
    }
    super::wallpaper_sync::live(None);
    set_status("");
}

fn listen(s: &LiveSettings) -> Result<(), String> {
    stop();
    let listener = TcpListener::bind(("127.0.0.1", s.port))
        .map_err(|e| format!("Can't listen on 127.0.0.1:{}: {e}", s.port))?;
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    set_status(format!("Listening on 127.0.0.1:{}", s.port));
    eprintln!("Foxglove: live feed on 127.0.0.1:{}", s.port);
    let token = s.token.clone();
    let (accept_tx, accept_stop) = (tx.clone(), stop.clone());
    // Both supervised (#587). The hub's queue outlives a panicking attempt, so the accept
    // thread, the clients and `send` keep a good sender.
    let accept = super::services::spawn_supervised("live-accept", move || {
        accept_loop(&listener, &accept_stop, &accept_tx, &token)
    });
    *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Running {
        tx: tx.clone(),
        stop,
        port: s.port,
        accept,
    });
    let facts = super::devices::bus().facts();
    forward_facts(facts, "keyboard.*", &tx, || Msg::State);
    forward_facts(facts, "app.foreground", &tx, || Msg::Foreground);
    let rx = Mutex::new(rx);
    let _ = super::services::spawn_supervised("live-hub", move || {
        run(&rx.lock().unwrap_or_else(|e| e.into_inner()), &tx)
    });
    super::wallpaper_sync::live(Some(s.port));
    Ok(())
}

fn accept_loop(listener: &TcpListener, stop: &AtomicBool, tx: &Sender<Msg>, token: &str) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let pending = Arc::new(AtomicUsize::new(0));
    let mut errors = 0;
    for conn in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let stream = match conn {
            Ok(s) => {
                errors = 0;
                s
            }
            Err(e) => {
                errors += 1;
                if errors >= 32 {
                    eprintln!("Foxglove: live feed stops accepting: {e}");
                    set_status(format!("Stopped: {e}"));
                    break;
                }
                continue;
            }
        };
        if pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING {
            pending.fetch_sub(1, Ordering::SeqCst);
            let _ = stream.shutdown(Shutdown::Both);
            continue;
        }
        let (tx, token, pending) = (tx.clone(), token.to_string(), pending.clone());
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        super::services::spawn_named("live-client", move || {
            serve(stream, id, &tx, &token, &pending)
        });
    }
}

/// One connection: the upgrade, the checks, then the wallpaper's own messages until it closes.
fn serve(mut stream: TcpStream, id: u64, tx: &Sender<Msg>, token: &str, pending: &AtomicUsize) {
    let admitted = handshake(&mut stream, token);
    pending.fetch_sub(1, Ordering::SeqCst);
    if let Err(e) = admitted {
        eprintln!("Foxglove: live feed refused a connection: {e}");
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let _ = writer.set_write_timeout(Some(WRITE_LIMIT));
    if tx.send(Msg::Join(id, writer)).is_err() {
        return;
    }
    let mut reader = ws::Reader::default();
    loop {
        match reader.read(&mut stream) {
            Ok(ws::Message::Text(t)) => {
                if let Some(wire::FromWallpaper::Paused { paused }) = wire::parse(&t) {
                    let _ = tx.send(Msg::Paused(id, paused));
                }
            }
            Ok(ws::Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = tx.send(Msg::Leave(id));
}

/// Reads with one deadline for the whole upgrade request, however it trickles in.
struct Deadline<'a>(&'a mut TcpStream, Instant);

impl std::io::Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.1.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.0.set_read_timeout(Some(left))?;
        std::io::Read::read(self.0, buf)
    }
}

fn handshake(stream: &mut TcpStream, token: &str) -> Result<(), String> {
    let h = ws::read_handshake(&mut Deadline(stream, Instant::now() + HANDSHAKE_LIMIT))
        .map_err(|e| e.to_string())?;
    if !(h.path == "/live" || h.path.starts_with("/live?")) {
        let _ = ws::reject(stream, 404, "Not Found");
        // Without the query: it may hold a token.
        return Err(format!(
            "no such path {}",
            h.path.split('?').next().unwrap_or("")
        ));
    }
    let offered = h.header("sec-websocket-protocol");
    if let Err(e) = wire::admit(h.origin(), wire::token_from(&h.path, offered), token) {
        let _ = ws::reject(stream, 403, "Forbidden");
        return Err(e);
    }
    let protocol = offered
        .is_some_and(|p| p.split(',').any(|p| p.trim() == wire::PROTOCOL))
        .then_some(wire::PROTOCOL);
    ws::accept(stream, &h, protocol).map_err(|e| e.to_string())?;
    stream.set_read_timeout(None).map_err(|e| e.to_string())
}

/// The hub: owns the wallpapers' streams, the state and the gates.
fn run(rx: &Receiver<Msg>, tx: &Sender<Msg>) {
    let facts = super::devices::bus().facts();
    let _away = super::away::subscribe({
        let tx = tx.clone();
        move |_| {
            let _ = tx.send(Msg::Gates);
        }
    });

    let epoch = Instant::now();
    let mut clients = Clients(Vec::new());
    let mut hub = Hub {
        state: read_state(facts),
        board: GEOMETRY.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        ..Hub::default()
    };
    let mut fullscreen = fullscreen_now();
    hub.quiet = wire::quiet(super::pause::paused(), super::away::away(), fullscreen);
    let mut batch_started: Option<Instant> = None;
    let mut last_key: Option<Instant> = None;
    let mut keys = KeyTap::default();
    loop {
        let msg = match batch_started {
            Some(started) => {
                let left = Duration::from_millis(wire::BATCH_MS).saturating_sub(started.elapsed());
                match rx.recv_timeout(left) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        let mut dead = Vec::new();
        let gates =
            |fullscreen: bool| wire::quiet(super::pause::paused(), super::away::away(), fullscreen);
        match msg {
            None => {}
            Some(Msg::Stop) => break,
            Some(Msg::Join(id, stream)) => {
                clients.push(Client {
                    id,
                    stream,
                    paused: false,
                });
                if !hub.join(id, &mut Streams(&mut clients)) {
                    dead.push(id);
                }
            }
            Some(Msg::Leave(id)) => dead.push(id),
            Some(Msg::Paused(id, p)) => {
                if let Some(c) = clients.iter_mut().find(|c| c.id == id) {
                    c.paused = p;
                }
                if !hub.paused(id, p, &mut Streams(&mut clients)) {
                    dead.push(id);
                }
            }
            Some(Msg::State) => {
                dead = hub.set_state(read_state(facts), &mut Streams(&mut clients));
            }
            Some(Msg::Board) => {
                let board = GEOMETRY.lock().unwrap_or_else(|e| e.into_inner()).clone();
                dead = hub.set_board(board, &mut Streams(&mut clients));
            }
            Some(Msg::Foreground) => {
                fullscreen = fullscreen_now();
                dead = hub.gates(gates(fullscreen), &mut Streams(&mut clients));
            }
            Some(Msg::Gates) => {
                dead = hub.gates(gates(fullscreen), &mut Streams(&mut clients));
            }
            Some(Msg::Key {
                row,
                col,
                pressed,
                generation,
            }) if generation == keys.generation() => {
                KEYS_QUEUED.fetch_sub(1, Ordering::SeqCst);
                let now = Instant::now();
                if last_key.is_none_or(|t| now.duration_since(t) >= BURST_GAP) {
                    fullscreen = fullscreen_now();
                    dead = hub.gates(gates(fullscreen), &mut Streams(&mut clients));
                }
                last_key = Some(now);
                let now_ms = now.duration_since(epoch).as_millis() as u64;
                if hub.key(row, col, pressed, now_ms) {
                    batch_started = Some(now);
                }
            }
            Some(Msg::Key { .. }) => {
                KEYS_QUEUED.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let due = batch_started.is_some_and(|t| {
            t.elapsed() >= Duration::from_millis(wire::BATCH_MS) || hub.batch.is_full()
        });
        if due || hub.batch.is_empty() {
            dead.extend(hub.flush(&mut Streams(&mut clients)));
            batch_started = None;
        }
        for id in dead {
            hub.leave(id);
            if let Some(i) = clients.iter().position(|c| c.id == id) {
                let _ = clients.remove(i).stream.shutdown(Shutdown::Both);
            }
        }
        keys.want(hub.wants_keys(), tx);
        let n = clients.len();
        if CONNECTED.swap(n, Ordering::Relaxed) != n {
            super::tray_panel::wake();
        }
    }
    // Stopped: the wallpapers are let go, those still queued to join too.
    keys.want(false, tx);
    while let Ok(m) = rx.try_recv() {
        if let Msg::Join(_, stream) = m {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
    for mut c in clients.0.drain(..) {
        let _ = ws::write_frame(&mut c.stream, ws::CLOSE, &1001u16.to_be_bytes());
        let _ = c.stream.flush();
        let _ = c.stream.shutdown(Shutdown::Both);
    }
    CONNECTED.store(0, Ordering::Relaxed);
}

/// The hub's wallpapers. A hub that panics lets them go on the way out, so each one reconnects
/// to the restarted hub instead of waiting on a connection nobody writes to (#587).
struct Clients(Vec<Client>);

impl std::ops::Deref for Clients {
    type Target = Vec<Client>;
    fn deref(&self) -> &Vec<Client> {
        &self.0
    }
}

impl std::ops::DerefMut for Clients {
    fn deref_mut(&mut self) -> &mut Vec<Client> {
        &mut self.0
    }
}

impl Drop for Clients {
    fn drop(&mut self) {
        for c in &self.0 {
            let _ = c.stream.shutdown(Shutdown::Both);
        }
        CONNECTED.store(0, Ordering::Relaxed);
    }
}

/// The key-event subscription, held only while it's wanted: each start is a new generation,
/// and a forwarder of an old one ends at its next event (dropping the bus's queue with it).
#[derive(Default)]
struct KeyTap {
    on: bool,
    generation: Arc<AtomicU64>,
}

impl KeyTap {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn want(&mut self, want: bool, tx: &Sender<Msg>) {
        if want == self.on {
            return;
        }
        self.on = want;
        let mine = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if !want {
            return;
        }
        let events = super::devices::bus().subscribe(true);
        let (tx, current) = (tx.clone(), self.generation.clone());
        super::services::spawn_named("live-keys", move || {
            for e in events {
                if current.load(Ordering::SeqCst) != mine {
                    break;
                }
                if let DeviceEvent::Key { row, col, pressed } = e.event {
                    if KEYS_QUEUED.fetch_add(1, Ordering::SeqCst) >= KEYS_IN_FLIGHT {
                        KEYS_QUEUED.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let key = Msg::Key {
                        row,
                        col,
                        pressed,
                        generation: mine,
                    };
                    if tx.send(key).is_err() {
                        break;
                    }
                }
            }
        });
    }
}

/// Forwards a fact pattern's changes to the hub until the hub has gone.
fn forward_facts(facts: &Facts, pattern: &str, tx: &Sender<Msg>, msg: fn() -> Msg) {
    let changes = facts.subscribe(pattern);
    let tx = tx.clone();
    super::services::spawn_named("live-facts", move || {
        for _ in changes {
            if tx.send(msg()).is_err() {
                break;
            }
        }
    });
}

fn read_state(facts: &Facts) -> wire::State {
    let int = |k: &str| match facts.get(k) {
        Some(Value::Int(n)) => u8::try_from(n).ok(),
        _ => None,
    };
    let connected = matches!(facts.get("keyboard.connected"), Some(Value::Bool(true)));
    let layer = int("keyboard.layer").map(|index| wire::Layer {
        index,
        name: super::wallpaper_sync::layer_name(&[index], &super::layer_names::remembered())
            .unwrap_or_default(),
    });
    wire::State {
        layer,
        art: super::art_service::handle().and_then(|h| showing_name(&h.snapshot())),
        battery: wire::Battery {
            left: int("keyboard.left.battery"),
            right: int("keyboard.right.battery"),
        },
        link: wire::Link {
            connected,
            output: match facts.get("keyboard.link") {
                Some(Value::Text(t)) => Some(t),
                _ => None,
            },
            profile: int("keyboard.profile"),
        },
    }
}

/// The right screen's piece, by name.
fn showing_name(s: &super::art_service::Snapshot) -> Option<String> {
    use super::art_service::{file_name, name_of, Choice};
    if !s.connected {
        return None;
    }
    if let Some(p) = s.showing_file() {
        return Some(file_name(p));
    }
    let set = s.showing_built_in()?;
    let count = s.status.map_or(0, |st| st.right_set_count) as i8;
    Some(if set < count || count <= 0 {
        name_of(&Choice::BuiltIn(set), s)
    } else {
        name_of(&Choice::Slot((set - count) as u8), s)
    })
}

/// Tests stand in for the program in front.
#[cfg(test)]
static FAKE_FULLSCREEN: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
fn fullscreen_now() -> bool {
    FAKE_FULLSCREEN.load(Ordering::SeqCst)
}

/// A full-screen program is in front: a game, a video or a presentation.
#[cfg(all(windows, not(test)))]
fn fullscreen_now() -> bool {
    use windows::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE,
        QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    // SAFETY: no arguments; it returns the state by value.
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| {
        matches!(
            s,
            QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
        )
    })
}

#[cfg(all(not(windows), not(test)))]
fn fullscreen_now() -> bool {
    false
}

// ------------------------------------------------------------------------------ settings

/// Turns the feed on or off; turning it on makes the token the first time.
fn set_enabled(on: bool) -> Result<(), String> {
    let mut s = settings();
    s.enabled = on;
    if on && s.token.is_empty() {
        s.token = wire::new_token().ok_or("no random source for the pairing token")?;
    }
    save(&s)?;
    if on {
        listen(&s)
    } else {
        stop();
        Ok(())
    }
}

/// A new token: wallpapers paired with the old one are let go.
fn new_token() -> Result<(), String> {
    let mut s = settings();
    s.token = wire::new_token().ok_or("no random source for the pairing token")?;
    save(&s)?;
    if s.enabled {
        listen(&s)?;
    }
    Ok(())
}

/// What the settings card says the feed sends.
pub const EXPLAINER: &str = "A live wallpaper paired with Foxglove shows your typing as ripples, with the layer, the art screen's art, battery and connection. It gets which key position was pressed and when, never the letter or keycode, and only with the pairing token. Someone who knows your keymap could still tell the keys apart. Nothing is sent while Foxglove is paused, the PC is locked, the screen is off or a full-screen game runs.";

/// The settings card.
pub fn group(ui: &mut egui::Ui) {
    super::settings_ui::section(ui, "Live wallpaper", |ui| {
        let s = settings();
        let mut on = s.enabled;
        let mut error = None;
        if ui
            .checkbox(&mut on, "Live keyboard feed for wallpapers")
            .changed()
        {
            error = set_enabled(on).err();
        }
        ui.weak(EXPLAINER);
        if on {
            let s = settings();
            ui.horizontal(|ui| {
                ui.label("Pairing token");
                let mut shown = s.token.clone();
                ui.add(
                    egui::TextEdit::singleline(&mut shown)
                        .desired_width(200.0)
                        .interactive(false),
                );
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(s.token.clone());
                }
                if ui
                    .button("New token")
                    .on_hover_text("Wallpapers paired with the old token are disconnected")
                    .clicked()
                {
                    error = new_token().err();
                }
            });
            ui.weak(
                "Paste it into the wallpaper's \"Foxglove pairing token\" in Wallpaper Engine.",
            );
            let status = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let n = CONNECTED.load(Ordering::Relaxed);
            let line = match n {
                0 => status,
                1 => format!("{status}: 1 wallpaper connected"),
                n => format!("{status}: {n} wallpapers connected"),
            };
            if !line.is_empty() {
                ui.weak(line);
            }
        }
        if let Some(e) = error {
            set_status(e.clone());
            ui.colored_label(super::theme::palette(ui).danger, e);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Records what each wallpaper was sent; `dead` ones fail.
    #[derive(Default)]
    struct Sent {
        to: HashMap<u64, Vec<String>>,
        dead: Vec<u64>,
    }

    impl Out for Sent {
        fn send(&mut self, id: u64, text: &str) -> bool {
            self.to.entry(id).or_default().push(text.to_string());
            !self.dead.contains(&id)
        }
    }

    impl Sent {
        fn last(&self, id: u64) -> &str {
            self.to[&id].last().unwrap()
        }
        fn count(&self, id: u64) -> usize {
            self.to.get(&id).map_or(0, Vec::len)
        }
    }

    fn corne() -> Geometry {
        (
            wire::Board {
                name: "Eyelash Corne".into(),
                keys: vec![wire::key_rect(0.0, 0.0, 1.0, 1.0, 0.0); 3],
            },
            vec![(0, 0), (0, 1), (0, 2)],
        )
    }

    #[test]
    fn off_by_default() {
        let s = LiveSettings::default();
        assert!(!s.enabled && s.token.is_empty());
        assert_eq!(s.port, DEFAULT_PORT);
        let parsed: LiveSettings = toml::from_str("").unwrap();
        assert!(!parsed.enabled);
        assert!(EXPLAINER.contains("never the letter or keycode"));
        assert!(!EXPLAINER.contains("Bandit"), "any paired wallpaper (#562)");
    }

    #[test]
    fn a_wallpaper_gets_hello_then_keys_as_indices() {
        let mut hub = Hub {
            board: Some(corne()),
            ..Hub::default()
        };
        let mut out = Sent::default();
        assert!(hub.join(1, &mut out));
        assert!(out.last(1).starts_with(r#"{"t":"hello","v":1"#));
        assert!(hub.wants_keys());
        assert!(hub.key(0, 2, true, 100), "starts a batch");
        assert!(!hub.key(0, 2, false, 108));
        assert!(!hub.key(9, 9, true, 109), "off the board: dropped");
        assert!(hub.flush(&mut out).is_empty());
        assert_eq!(out.last(1), r#"{"t":"keys","e":[[2,1,0],[2,0,8]]}"#);
    }

    #[test]
    fn each_gate_stops_sending() {
        for why in [Why::Paused, Why::Away, Why::Fullscreen] {
            let mut hub = Hub {
                board: Some(corne()),
                ..Hub::default()
            };
            let mut out = Sent::default();
            hub.join(1, &mut out);
            hub.gates(Some(why), &mut out);
            assert_eq!(
                out.last(1),
                Message::Quiet { why }.json(),
                "told once that it's quiet"
            );
            let told = out.count(1);
            assert!(!hub.key(0, 1, true, 5), "{why:?}: keys dropped");
            hub.flush(&mut out);
            let state = wire::State {
                art: Some("Fox run".into()),
                ..wire::State::default()
            };
            hub.set_state(state, &mut out);
            hub.set_board(None, &mut out);
            assert!(hub.gates(Some(why), &mut out).is_empty());
            assert_eq!(out.count(1), told, "{why:?}: nothing else while quiet");
            // Key events stay subscribed only for the full-screen check.
            assert_eq!(hub.wants_keys(), why == Why::Fullscreen);
            hub.gates(None, &mut out);
            assert_eq!(out.to[&1][told], r#"{"t":"live"}"#);
            assert!(out.last(1).contains(r#""art":"Fox run""#), "caught up");
        }
    }

    #[test]
    fn a_paused_wallpaper_hears_nothing_and_catches_up() {
        let mut hub = Hub {
            board: Some(corne()),
            ..Hub::default()
        };
        let mut out = Sent::default();
        hub.join(1, &mut out);
        hub.join(2, &mut out);
        hub.paused(1, true, &mut out);
        hub.key(0, 0, true, 0);
        hub.flush(&mut out);
        assert_eq!(out.count(1), 1, "only the hello");
        assert_eq!(out.count(2), 2);
        hub.paused(2, true, &mut out);
        assert!(!hub.wants_keys(), "nobody listens: no key subscription");
        hub.paused(1, false, &mut out);
        assert!(out.last(1).starts_with(r#"{"t":"hello""#));
        assert!(hub.wants_keys());
    }

    #[test]
    fn state_goes_out_on_change_only_and_dead_wallpapers_are_dropped() {
        let mut hub = Hub::default();
        let mut out = Sent::default();
        hub.join(1, &mut out);
        hub.join(2, &mut out);
        out.dead.push(2);
        let state = wire::State {
            battery: wire::Battery {
                left: Some(80),
                right: None,
            },
            ..wire::State::default()
        };
        assert_eq!(hub.set_state(state.clone(), &mut out), vec![2]);
        let n = out.count(1);
        assert!(hub.set_state(state, &mut out).is_empty());
        assert_eq!(out.count(1), n, "the same state isn't sent again");
    }

    #[test]
    fn at_most_four_wallpapers() {
        let mut hub = Hub::default();
        let mut out = Sent::default();
        for id in 0..wire::MAX_CLIENTS as u64 {
            assert!(hub.join(id, &mut out));
        }
        assert!(!hub.join(99, &mut out));
    }

    /// End to end over a real loopback socket, with a stand-in wallpaper: a wrong token and a
    /// web page are refused, the right token gets the hello.
    #[test]
    fn the_handshake_admits_only_the_paired_wallpaper() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "PairPairPairPairPairPair";
        let server = std::thread::spawn(move || {
            let mut results = Vec::new();
            for _ in 0..4 {
                let (mut s, _) = listener.accept().unwrap();
                results.push(handshake(&mut s, token).is_ok());
            }
            results
        });
        // The client ends stay open until the server is done with them.
        let mut held = Vec::new();
        let mut ask = |path: &str, origin: Option<&str>, protocols: Option<&str>| {
            use std::io::Read;
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let mut req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n");
            if let Some(o) = origin {
                req += &format!("Origin: {o}\r\n");
            }
            if let Some(p) = protocols {
                req += &format!("Sec-WebSocket-Protocol: {p}\r\n");
            }
            req += "\r\n";
            c.write_all(req.as_bytes()).unwrap();
            let mut head = [0u8; 12];
            c.read_exact(&mut head).unwrap();
            held.push(c);
            String::from_utf8_lossy(&head).to_string()
        };
        assert!(ask("/live?token=wrong", Some("null"), None).contains("403"));
        assert!(ask(
            &format!("/live?token={token}"),
            Some("https://example.com"),
            None
        )
        .contains("403"));
        assert!(ask(&format!("/other?token={token}"), Some("null"), None).contains("404"));
        assert!(ask(
            "/live",
            Some("null"),
            Some(&format!("foxglove-live.v1, {token}"))
        )
        .contains("101"));
        assert_eq!(server.join().unwrap(), [false, false, false, true]);
        drop(held);
    }

    /// A stand-in wallpaper's end of the socket: the upgrade, then the server's frames.
    struct StandIn(TcpStream);

    impl StandIn {
        fn connect(port: u16, token: &str) -> Option<StandIn> {
            use std::io::Read;
            let mut c = TcpStream::connect(("127.0.0.1", port)).ok()?;
            c.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
            let req = format!("GET /live?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nOrigin: null\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n");
            c.write_all(req.as_bytes()).ok()?;
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if c.read(&mut b).ok()? == 0 {
                    return None;
                }
                head.push(b[0]);
            }
            String::from_utf8_lossy(&head)
                .starts_with("HTTP/1.1 101")
                .then_some(StandIn(c))
        }

        /// The next text frame, or `None` when nothing comes within `wait`.
        fn next(&mut self, wait: Duration) -> Option<String> {
            use std::io::Read;
            self.0.set_read_timeout(Some(wait)).ok()?;
            let mut h = [0u8; 2];
            self.0.read_exact(&mut h).ok()?;
            let len = match h[1] & 0x7F {
                126 => {
                    let mut l = [0u8; 2];
                    self.0.read_exact(&mut l).ok()?;
                    usize::from(u16::from_be_bytes(l))
                }
                127 => return None,
                n => usize::from(n),
            };
            let mut body = vec![0u8; len];
            self.0.read_exact(&mut body).ok()?;
            Some(String::from_utf8_lossy(&body).into_owned())
        }

        fn say(&mut self, text: &str) {
            let mask = [7u8, 1, 2, 3];
            let mut f = vec![0x81, 0x80 | text.len() as u8];
            f.extend_from_slice(&mask);
            f.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
            self.0.write_all(&f).unwrap();
        }
    }

    /// End to end, sandboxed (#499's acceptance): the real listener and hub on a free loopback
    /// port with a stand-in wallpaper. A wrong token is refused, key presses arrive as indices,
    /// and the full-screen gate and the wallpaper's own pause each stop the keys.
    #[test]
    fn end_to_end_with_a_stand_in_wallpaper() {
        crate::foxglove::store::test_config_dir();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let token = wire::new_token().unwrap();
        *GEOMETRY.lock().unwrap() = Some(corne());
        listen(&LiveSettings {
            enabled: true,
            port,
            token: token.clone(),
        })
        .unwrap();
        assert!(StandIn::connect(port, "wrong").is_none(), "a wrong token");
        let mut w = StandIn::connect(port, &token).expect("the paired wallpaper");
        let hello = w.next(Duration::from_secs(5)).unwrap();
        assert!(hello.starts_with(r#"{"t":"hello","v":1"#), "{hello}");
        assert!(hello.contains(r#""name":"Eyelash Corne""#), "{hello}");

        let id = foxglove_devices::DeviceId::new(
            foxglove_devices::BackendId("live-test".into()),
            1,
            2,
            None,
            "live-test",
        );
        let sink = super::super::devices::bus().sink(id);
        let press = || {
            sink.emit(DeviceEvent::Key {
                row: 0,
                col: 2,
                pressed: true,
            })
        };
        // The key subscription starts when the wallpaper joins, just after its hello.
        let mut keys = None;
        for _ in 0..50 {
            press();
            if let Some(m) = w.next(Duration::from_millis(100)) {
                keys = Some(m);
                break;
            }
        }
        let keys = keys.expect("a keys frame");
        assert!(keys.starts_with(r#"{"t":"keys","e":[[2,1,"#), "{keys}");

        // A full-screen game comes to the front: quiet, then nothing.
        FAKE_FULLSCREEN.store(true, Ordering::SeqCst);
        super::super::devices::bus()
            .facts()
            .set("app.foreground", Value::Text("game.exe".into()));
        let mut quiet = None;
        while let Some(m) = w.next(Duration::from_secs(2)) {
            if m.starts_with(r#"{"t":"quiet""#) {
                quiet = Some(m);
                break;
            }
        }
        assert_eq!(
            quiet.as_deref(),
            Some(r#"{"t":"quiet","why":"fullscreen"}"#)
        );
        press();
        assert_eq!(
            w.next(Duration::from_millis(300)),
            None,
            "no keys in full screen"
        );

        // Back to the desktop: live again.
        FAKE_FULLSCREEN.store(false, Ordering::SeqCst);
        super::super::devices::bus()
            .facts()
            .set("app.foreground", Value::Text("explorer.exe".into()));
        assert_eq!(
            w.next(Duration::from_secs(2)).as_deref(),
            Some(r#"{"t":"live"}"#)
        );
        let state = w.next(Duration::from_secs(2)).unwrap();
        assert!(state.starts_with(r#"{"t":"state""#), "{state}");

        // The wallpaper pauses: nothing more for it.
        w.say(r#"{"t":"paused","paused":true}"#);
        let mut got = Vec::new();
        for _ in 0..5 {
            press();
            if let Some(m) = w.next(Duration::from_millis(100)) {
                got.push(m);
            }
        }
        // At most a frame already on its way before the pause arrived.
        assert!(got.len() <= 1, "{got:?}");

        // A new token: the same port at once (the old accept thread is joined first), and only
        // the new token gets in.
        let fresh = wire::new_token().unwrap();
        for _ in 0..3 {
            listen(&LiveSettings {
                enabled: true,
                port,
                token: fresh.clone(),
            })
            .expect("the port is free again");
        }
        assert!(StandIn::connect(port, &token).is_none(), "the old token");
        assert!(StandIn::connect(port, &fresh).is_some(), "the new token");

        stop();
        assert!(
            TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(300)
            )
            .is_err()
                || StandIn::connect(port, &token).is_none(),
            "stopped: nothing listens"
        );
    }

    #[test]
    fn the_art_name_follows_what_plays() {
        let mut s = super::super::art_service::Snapshot::default();
        assert_eq!(showing_name(&s), None, "no keyboard");
        s.connected = true;
        s.sent = Some((std::path::PathBuf::from("C:/art/Fox run.nva"), 1));
        s.status = Some(digitalis::protocol::ArtStatus {
            right_state: digitalis::protocol::right_state::PLAYING,
            right_set_count: 4,
            ..Default::default()
        });
        assert_eq!(showing_name(&s).as_deref(), Some("Fox run"));
    }
}
