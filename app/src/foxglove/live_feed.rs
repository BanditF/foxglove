//! The live keyboard feed (#499, epic #498): a paired wallpaper on this PC gets key positions,
//! the layer, the right screen's art, battery, connection and the lights' colour, as `foxglove-live` v1
//! (`live_wire`) on a WebSocket at `127.0.0.1:<port>` (8768 by default).
//!
//! - **Opt-in:** off by default. While it's off nothing listens and nothing subscribes to keys.
//! - **Who:** a `file://` wallpaper (Origin `null`; #320) or a local program, and only with this
//!   install's token. Web pages are refused by their origin. A local client with a wrong token
//!   is told `refused` and closed with 4001; the log says the first in full and then sums them at
//!   most every 10 min, and the health view warns while they keep coming (`wire::Refusals`).
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
//! - **Notifications:** Foxglove's own notifications (`notice`) go to the wallpapers that said
//!   `caps: ["notify"]`, while the feed isn't quiet and "Show notifications on the wallpaper" is
//!   on. A wallpaper answers `notified`; without an answer in [`wire::ACK_LIMIT`] the hub shows
//!   the Windows notification instead.
//!
//! - **Which keyboard (#801):** the focused keyboard, or the one the feed is pinned to (Settings →
//!   Live wallpaper, `focus_pins`). `state.keyboard` names it. While it is the connected
//!   host-link keyboard the feed carries its facts, keys and the overlay's geometry; for any other
//!   keyboard only its name and shape (its `KeyLayout`), and no keys. A focus change (C1's
//!   `focus.*` facts) sends the new board and state.
//!
//! Later topics (PC stats) are new `t` values in `live_wire`; the gates, the pairing and the hub
//! stay as they are.

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
use super::notice::Note;

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
    /// Foxglove's notifications go to a wallpaper that can show them, else to Windows.
    pub notify: bool,
}

impl Default for LiveSettings {
    fn default() -> Self {
        LiveSettings {
            enabled: false,
            port: DEFAULT_PORT,
            token: String::new(),
            notify: true,
        }
    }
}

static SETTINGS: Mutex<Option<LiveSettings>> = Mutex::new(None);

/// The settings, read from the store once and then kept in step by [`save`]. A read that
/// fails isn't kept (#741): the next call reads again, so the defaults' empty token never
/// stands in for the saved one.
pub fn settings() -> LiveSettings {
    cached(
        &mut SETTINGS.lock().unwrap_or_else(|e| e.into_inner()),
        || super::store::section(STORE_OWNER, SECTION).load(),
    )
}

/// The cache's settings, reading them with `load` when there are none yet; a failed read gives
/// the defaults without keeping them.
fn cached(
    cache: &mut Option<LiveSettings>,
    load: impl FnOnce() -> Result<Option<LiveSettings>, String>,
) -> LiveSettings {
    if let Some(s) = cache {
        return s.clone();
    }
    match load() {
        Ok(saved) => cache.insert(saved.unwrap_or_default()).clone(),
        Err(_) => LiveSettings::default(),
    }
}

fn save(s: &LiveSettings) -> Result<(), String> {
    super::store::section(STORE_OWNER, SECTION)
        .save(s)
        .map_err(|e| e.to_string())?;
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
    /// A wallpaper's `hi`: whether it takes notifications.
    Caps(u64, bool),
    /// A wallpaper drew notification `id`.
    Notified(u64),
    /// A notification to route (`notice::show`).
    Notify(Note),
    Key {
        row: u16,
        col: u16,
        pressed: bool,
        generation: u64,
    },
    /// A `keyboard.*` fact or the art changed.
    State,
    /// The feed's keyboard may have changed (`focus.*`, #801).
    Focus,
    /// Foxglove sent the left screen's panel a new frame, or handed it back (`screens`).
    Panel,
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

/// A notification sent to the wallpapers, waiting for one to say it drew it.
struct Pending {
    id: u64,
    note: Note,
    due: Instant,
}

/// The hub's decisions, apart from its sockets and threads.
#[derive(Default)]
struct Hub {
    /// Connected wallpapers and whether each is paused.
    clients: Vec<(u64, bool)>,
    /// The wallpapers whose `hi` declared [`wire::CAP_NOTIFY`].
    notifiers: Vec<u64>,
    state: wire::State,
    board: Option<Geometry>,
    quiet: Option<Why>,
    batch: wire::Batch,
    /// The right screen's piece with its frames (`live_art`), sent once per change.
    art: Option<wire::Art>,
    /// The piece changed while the feed was quiet: it goes out when the feed is live again.
    art_unsent: bool,
    /// The left screen's PC panel as last sent to the keyboard.
    panel: Option<wire::Panel>,
    panel_unsent: bool,
    pending: Vec<Pending>,
    last_note: u64,
    /// The feed shows a keyboard other than the connected host-link one (#801): no keys.
    foreign: bool,
}

impl Hub {
    fn listeners(&self) -> impl Iterator<Item = u64> + '_ {
        self.clients.iter().filter(|(_, p)| !p).map(|(id, _)| *id)
    }

    /// A wallpaper's `hi`: it takes notifications or not.
    fn caps(&mut self, id: u64, notify: bool) {
        self.notifiers.retain(|&c| c != id);
        if notify && self.clients.iter().any(|(c, _)| *c == id) {
            self.notifiers.push(id);
        }
    }

    /// Routes a notification: to every unpaused wallpaper that takes them, while the feed isn't
    /// quiet, to be acknowledged within [`wire::ACK_LIMIT`]. The note comes back when it has to
    /// be a Windows notification now; the ids are wallpapers whose write failed.
    fn notify(&mut self, note: Note, now: Instant, out: &mut impl Out) -> (Option<Note>, Vec<u64>) {
        let to: Vec<u64> = self
            .listeners()
            .filter(|id| self.notifiers.contains(id))
            .collect();
        if self.quiet.is_some() || to.is_empty() {
            return (Some(note), Vec::new());
        }
        self.last_note += 1;
        let id = self.last_note;
        let text = Message::Notify {
            id,
            title: note.title.clone(),
            body: note.body.clone(),
            level: note.level,
        }
        .json();
        let sent_to = to.len();
        let dead: Vec<u64> = to.into_iter().filter(|&c| !out.send(c, &text)).collect();
        if dead.len() == sent_to {
            return (Some(note), dead);
        }
        self.pending.push(Pending {
            id,
            note,
            due: now + wire::ACK_LIMIT,
        });
        (None, dead)
    }

    /// A wallpaper drew notification `id`: no Windows notification for it.
    fn notified(&mut self, id: u64) {
        self.pending.retain(|p| p.id != id);
    }

    /// The notifications nobody acknowledged in time, taken out to show as Windows ones.
    fn expired(&mut self, now: Instant) -> Vec<Note> {
        let (late, waiting): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.due <= now);
        self.pending = waiting;
        late.into_iter().map(|p| p.note).collect()
    }

    /// When the next unacknowledged notification falls back.
    fn next_due(&self) -> Option<Instant> {
        self.pending.iter().map(|p| p.due).min()
    }

    /// Whether key events are wanted at all: someone listens and nothing but full screen holds
    /// the feed (full screen is checked at a burst's start, so it needs the keys).
    fn wants_keys(&self) -> bool {
        !self.foreign
            && self.listeners().next().is_some()
            && matches!(self.quiet, None | Some(Why::Fullscreen))
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

    /// The hello, then the piece's frames and the Info screen when there are any.
    fn greet(&self, id: u64, out: &mut impl Out) -> bool {
        out.send(id, &self.hello().json())
            && self.art.as_ref().is_none_or(|_| {
                out.send(
                    id,
                    &Message::Art {
                        art: self.art.clone(),
                    }
                    .json(),
                )
            })
            && self.panel.as_ref().is_none_or(|_| {
                out.send(
                    id,
                    &Message::Panel {
                        panel: self.panel.clone(),
                    }
                    .json(),
                )
            })
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
        self.greet(id, out)
    }

    fn leave(&mut self, id: u64) {
        self.clients.retain(|(c, _)| *c != id);
        self.notifiers.retain(|&c| c != id);
    }

    /// A wallpaper paused or played again; coming back it gets everything afresh.
    fn paused(&mut self, id: u64, paused: bool, out: &mut impl Out) -> bool {
        let Some(c) = self.clients.iter_mut().find(|(c, _)| *c == id) else {
            return true;
        };
        let was = std::mem::replace(&mut c.1, paused);
        was == paused || paused || self.greet(id, out)
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
                if std::mem::take(&mut self.art_unsent) {
                    let art = self.art.clone();
                    dead.extend(self.broadcast(out, &Message::Art { art }));
                }
                if std::mem::take(&mut self.panel_unsent) {
                    let panel = self.panel.clone();
                    dead.extend(self.broadcast(out, &Message::Panel { panel }));
                }
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

    /// The piece on the right screen changed (its frames, or that there's none now).
    fn set_art(&mut self, art: Option<wire::Art>, out: &mut impl Out) -> Vec<u64> {
        if art == self.art {
            return Vec::new();
        }
        self.art = art;
        if self.quiet.is_some() {
            self.art_unsent = true;
            return Vec::new();
        }
        let art = self.art.clone();
        self.broadcast(out, &Message::Art { art })
    }

    /// The left screen's panel changed (the caller holds it to [`wire::PANEL_MS`]).
    fn set_panel(&mut self, panel: Option<wire::Panel>, out: &mut impl Out) -> Vec<u64> {
        if panel == self.panel {
            return Vec::new();
        }
        self.panel = panel;
        if self.quiet.is_some() {
            self.panel_unsent = true;
            return Vec::new();
        }
        let panel = self.panel.clone();
        self.broadcast(out, &Message::Panel { panel })
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
        if self.quiet.is_some() || self.foreign {
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
/// Token refusals: the log's summaries and the health warning (`wire::Refusals`).
static REFUSALS: Mutex<wire::Refusals> = Mutex::new(wire::Refusals::new());

/// The health view's rows (registered with `health::register` as "live_feed"): a warning while
/// a wallpaper keeps presenting an old pairing token; and while the feed runs and the keyboard
/// shows a piece, the art the feed has for it (`live_parity`, #1053).
pub fn health_rows() -> Vec<super::health::HealthRow> {
    let warns = REFUSALS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .warns(Instant::now());
    let mut rows = refusal_rows(warns);
    rows.extend(super::live_parity::rows_now());
    rows
}

fn refusal_rows(warns: bool) -> Vec<super::health::HealthRow> {
    if !warns {
        return Vec::new();
    }
    vec![super::health::HealthRow::new(
        "live_feed",
        "Live wallpaper pairing",
        super::health::Level::Warn,
        "a wallpaper is using an old pairing token",
    )
    .with_notice(
        "Wallpaper needs the new pairing token",
        "Copy it from App › Live wallpaper and paste it into the wallpaper's Foxglove pairing token.",
    )]
}

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

/// The left screen's panel as Foxglove last sent it, kept while the feed is off.
static PANEL: Mutex<Option<wire::Panel>> = Mutex::new(None);

/// Foxglove sent the left screen's panel a frame (`Some`), or handed the panel back to the
/// firmware (`None`), from the screens service.
pub fn panel(frame: Option<&super::screens::Frame>) {
    let p = frame.and_then(|f| wire::Panel::of(f.width, f.height, f.stride, &f.bits));
    let mut cur = PANEL.lock().unwrap_or_else(|e| e.into_inner());
    if *cur != p {
        *cur = p;
        drop(cur);
        send(Msg::Panel);
    }
}

/// The right screen's art may have changed (`art_service`).
pub fn art_changed() {
    send(Msg::State);
}

/// Routes one of Foxglove's notifications (`notice::show`) to the wallpapers. The note comes
/// back when it can't go that way: the setting is off or the feed isn't running. Whether a
/// wallpaper can show it now is the hub's to decide; it falls back to Windows itself.
pub fn notify(note: Note) -> Result<(), Note> {
    notify_with(&settings(), note)
}

fn notify_with(s: &LiveSettings, note: Note) -> Result<(), Note> {
    if !s.notify {
        return Err(note);
    }
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(r) = running.as_ref() else {
        return Err(note);
    };
    r.tx.send(Msg::Notify(note)).map_err(|e| match e.0 {
        Msg::Notify(n) => n,
        _ => unreachable!("sent a Notify"),
    })
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
    if let Ok(saved) = super::store::section(STORE_OWNER, SECTION).load::<LiveSettings>() {
        return start_with(saved.unwrap_or_default());
    }
    // The settings can't be read yet (#741): no feed and no token until they can. Tried again
    // a few times on a thread of its own (about 30 s), then left for the Settings card.
    let _ = std::thread::Builder::new()
        .name("live-settings".into())
        .spawn(|| {
            let mut last = String::new();
            for wait in super::app_log::BACKOFF {
                std::thread::sleep(wait);
                match super::store::section(STORE_OWNER, SECTION).load::<LiveSettings>() {
                    Ok(saved) => return start_with(saved.unwrap_or_default()),
                    Err(e) => last = e,
                }
            }
            eprintln!("Foxglove: live feed: settings unreadable ({last}); not started");
            set_status(format!("settings unreadable: {last}"));
        });
}

/// Starts the feed with settings just read from the store, if they say it's on.
fn start_with(s: LiveSettings) {
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(s.clone());
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
    // A `listen` right after this can then find the port still held ("Can't listen"), only
    // after an accept panic; the next attempt frees it.
    let backing_off = super::services::status()
        .get("live-accept")
        .is_some_and(|s| s.restarting);
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
    forward_facts(facts, "focus.*", &tx, || Msg::Focus);
    forward_facts(facts, "app.foreground", &tx, || Msg::Foreground);
    let rx = Mutex::new(rx);
    // Key generations count across hub restarts, so a restarted hub never takes keys still
    // queued by the forwarder of the one that panicked.
    let key_generation = Arc::new(AtomicU64::new(0));
    let _ = super::services::spawn_supervised("live-hub", move || {
        let rx = rx.lock().unwrap_or_else(|e| e.into_inner());
        run(&rx, &tx, &key_generation)
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
    match admitted {
        Ok(()) => REFUSALS.lock().unwrap_or_else(|e| e.into_inner()).paired(),
        Err(NotAdmitted::Token) => {
            let lines = REFUSALS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .refused(Instant::now());
            for line in lines {
                eprintln!("Foxglove: {line}");
            }
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        Err(NotAdmitted::Other(e)) => {
            eprintln!("Foxglove: live feed refused a connection: {e}");
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
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
                let _ = match wire::parse(&t) {
                    Some(wire::FromWallpaper::Paused { paused }) => {
                        tx.send(Msg::Paused(id, paused))
                    }
                    Some(wire::FromWallpaper::Hi { caps, .. }) => {
                        tx.send(Msg::Caps(id, wire::FromWallpaper::wants_notify(&caps)))
                    }
                    Some(wire::FromWallpaper::Notified { id: note }) => {
                        tx.send(Msg::Notified(note))
                    }
                    None => Ok(()),
                };
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

/// Why a connection didn't get the feed.
enum NotAdmitted {
    /// A wrong or missing token: the client was told `refused` (the log sums these).
    Token,
    /// Anything else (a web origin, a wrong path, a broken upgrade), logged as it is.
    Other(String),
}

fn handshake(stream: &mut TcpStream, token: &str) -> Result<(), NotAdmitted> {
    let other = |e: std::io::Error| NotAdmitted::Other(e.to_string());
    let h = ws::read_handshake(&mut Deadline(stream, Instant::now() + HANDSHAKE_LIMIT))
        .map_err(other)?;
    if !(h.path == "/live" || h.path.starts_with("/live?")) {
        let _ = ws::reject(stream, 404, "Not Found");
        // Without the query: it may hold a token.
        return Err(NotAdmitted::Other(format!(
            "no such path {}",
            h.path.split('?').next().unwrap_or("")
        )));
    }
    let offered = h.header("sec-websocket-protocol");
    let protocol = offered
        .is_some_and(|p| p.split(',').any(|p| p.trim() == wire::PROTOCOL))
        .then_some(wire::PROTOCOL);
    match wire::admit(h.origin(), wire::token_from(&h.path, offered), token) {
        Ok(()) => {}
        // A web page gets a plain 403 and no WebSocket at all.
        Err(e @ wire::Denied::Origin(_)) => {
            let _ = ws::reject(stream, 403, "Forbidden");
            return Err(NotAdmitted::Other(e.to_string()));
        }
        // A local client with a wrong token: a browser can't see a refused upgrade's status
        // (only close 1006), so the upgrade completes and it is told `refused` and closed with
        // CLOSE_REFUSED. That is all it learns.
        Err(wire::Denied::Token) => {
            refuse(stream, &h, protocol);
            return Err(NotAdmitted::Token);
        }
    }
    ws::accept(stream, &h, protocol).map_err(other)?;
    stream.set_read_timeout(None).map_err(other)
}

/// Tells a client with a wrong token so, then closes: `{"t":"refused","why":"token"}` and a
/// close [`wire::CLOSE_REFUSED`]. Waits at most [`WRITE_LIMIT`] for the client's close.
fn refuse(stream: &mut TcpStream, h: &ws::Handshake, protocol: Option<&str>) {
    let _ = stream.set_write_timeout(Some(WRITE_LIMIT));
    if ws::accept(stream, h, protocol).is_err() {
        return;
    }
    let refused = Message::Refused {
        why: wire::Refused::Token,
    };
    let _ = ws::write_text(stream, &refused.json());
    let _ = ws::write_frame(
        stream,
        ws::CLOSE,
        &close_payload(wire::CLOSE_REFUSED, "token"),
    );
    if stream.set_read_timeout(Some(WRITE_LIMIT)).is_ok() {
        let _ = ws::Reader::default().read(stream);
    }
}

/// A close frame's body: the code, then the reason.
fn close_payload(code: u16, reason: &str) -> Vec<u8> {
    let mut p = code.to_be_bytes().to_vec();
    p.extend_from_slice(reason.as_bytes());
    p
}

/// The hub: owns the wallpapers' streams, the state and the gates.
fn run(rx: &Receiver<Msg>, tx: &Sender<Msg>, key_generation: &Arc<AtomicU64>) {
    let facts = super::devices::bus().facts();
    let _away = super::away::subscribe({
        let tx = tx.clone();
        move |_| {
            let _ = tx.send(Msg::Gates);
        }
    });

    let epoch = Instant::now();
    let mut clients = Clients(Vec::new());
    let fed = feed_keyboard();
    let mut hub = Hub {
        state: read_state(facts),
        board: board_for(fed.as_ref()),
        foreign: !super::focus_pins::host_link(fed.as_ref()),
        ..Hub::default()
    };
    let mut fullscreen = fullscreen_now();
    hub.quiet = wire::quiet(super::pause::paused(), super::away::away(), fullscreen);
    let mut art = ArtWatch::default();
    hub.art = art.read().unwrap_or_default();
    // The art published against the keyboard's (#1053): one log line per step into a gap.
    let mut gap = super::live_parity::Gap::default();
    gap.note(hub.art.as_ref(), art.keyboard.as_deref());
    hub.panel = PANEL.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut panel = PanelPace::default();
    let mut batch_started: Option<Instant> = None;
    let mut last_key: Option<Instant> = None;
    let mut keys = KeyTap::new(key_generation.clone());
    loop {
        // Wakes for the batch's frame, the panel's pace or the next notification's fallback, else
        // only on a message.
        let wake = [
            batch_started.map(|t| t + Duration::from_millis(wire::BATCH_MS)),
            panel.due,
            hub.next_due(),
        ]
        .into_iter()
        .flatten()
        .min();
        let msg = match wake {
            Some(at) => {
                let left = at.saturating_duration_since(Instant::now());
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
                if hub.join(id, &mut Streams(&mut clients)) {
                    // A wallpaper connects when it has just loaded, from its saved properties:
                    // wallpaper sync sends them again (the theme too).
                    super::wallpaper_sync::reconnected();
                } else {
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
            Some(Msg::Caps(id, notify)) => hub.caps(id, notify),
            Some(Msg::Notified(note)) => hub.notified(note),
            Some(Msg::Notify(note)) => {
                // Full screen is looked at now: a game may have come up since the last check.
                fullscreen = fullscreen_now();
                dead = hub.gates(gates(fullscreen), &mut Streams(&mut clients));
                let (fallback, failed) =
                    hub.notify(note, Instant::now(), &mut Streams(&mut clients));
                dead.extend(failed);
                if let Some(n) = fallback {
                    super::notice::toast(&n);
                }
            }
            Some(Msg::State) => {
                dead = hub.set_state(read_state(facts), &mut Streams(&mut clients));
                if let Some(piece) = art.read() {
                    dead.extend(hub.set_art(piece, &mut Streams(&mut clients)));
                }
            }
            Some(Msg::Panel) => panel.changed(Instant::now()),
            Some(Msg::Board) => {
                let board = board_for(feed_keyboard().as_ref());
                dead = hub.set_board(board, &mut Streams(&mut clients));
            }
            Some(Msg::Focus) => {
                let fed = feed_keyboard();
                hub.foreign = !super::focus_pins::host_link(fed.as_ref());
                if hub.foreign {
                    // Keys of a burst already batched were the host link's: dropped.
                    hub.batch = wire::Batch::default();
                }
                dead = hub.set_board(board_for(fed.as_ref()), &mut Streams(&mut clients));
                dead.extend(hub.set_state(read_state(facts), &mut Streams(&mut clients)));
                if let Some(piece) = art.read() {
                    dead.extend(hub.set_art(piece, &mut Streams(&mut clients)));
                }
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
        for n in hub.expired(Instant::now()) {
            super::notice::toast(&n);
        }
        if panel.take(Instant::now()) {
            let latest = PANEL.lock().unwrap_or_else(|e| e.into_inner()).clone();
            dead.extend(hub.set_panel(latest, &mut Streams(&mut clients)));
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
        gap.note(hub.art.as_ref(), art.keyboard.as_deref());
        // A kept piece this PC has no file for, on screen while a wallpaper watches: its bytes
        // are read back once (#878); the feed hears when they land (`art_changed`).
        let watched = clients.iter().any(|c| !c.paused) && !super::pause::paused();
        if let Some(crc) = art.take_read(watched) {
            super::art_service::read_kept(crc);
        }
        ANY_PAUSED.store(clients.iter().any(|c| c.paused), Ordering::Relaxed);
        let n = clients.len();
        if CONNECTED.swap(n, Ordering::Relaxed) != n {
            super::tray_panel::wake();
        }
    }
    // Stopped: the wallpapers are let go, those still queued to join too, and notifications
    // nobody acknowledged yet show as Windows ones.
    keys.want(false, tx);
    for p in std::mem::take(&mut hub.pending) {
        super::notice::toast(&p.note);
    }
    while let Ok(m) = rx.try_recv() {
        match m {
            Msg::Join(_, stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            Msg::Notify(n) => super::notice::toast(&n),
            _ => {}
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
        if CONNECTED.swap(0, Ordering::Relaxed) != 0 {
            super::tray_panel::wake();
        }
    }
}

/// The key-event subscription, held only while it's wanted: each start is a new generation,
/// and a forwarder of an old one ends at its next event (dropping the bus's queue with it).
/// Dropped (the hub stopped or panicked), it ends its forwarder the same way (#587).
struct KeyTap {
    on: bool,
    generation: Arc<AtomicU64>,
}

impl Drop for KeyTap {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl KeyTap {
    fn new(generation: Arc<AtomicU64>) -> KeyTap {
        KeyTap {
            on: false,
            generation,
        }
    }

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

/// Paces the Info screen to one frame a [`wire::PANEL_MS`]: the first change goes out at once, a
/// change within the window waits for its end (the latest frame then), so an animated panel
/// costs a wallpaper at most four frames a second.
#[derive(Default)]
struct PanelPace {
    sent: Option<Instant>,
    /// When the held change goes out.
    due: Option<Instant>,
    ready: bool,
}

impl PanelPace {
    fn changed(&mut self, now: Instant) {
        let gap = Duration::from_millis(wire::PANEL_MS);
        match self.sent {
            Some(t) if now.duration_since(t) < gap => {
                self.due.get_or_insert(t + gap);
            }
            _ => self.ready = true,
        }
    }

    /// True when the latest panel should go out now.
    fn take(&mut self, now: Instant) -> bool {
        if self.due.is_some_and(|d| now >= d) {
            self.due = None;
            self.ready = true;
        }
        if std::mem::take(&mut self.ready) {
            self.sent = Some(now);
            return true;
        }
        false
    }
}

/// Where the right screen's piece comes from, so its frames are read only when it changes (a
/// battery or layer change re-reads the state, not the art file).
#[derive(Default)]
struct ArtWatch {
    seen: Option<ArtKey>,
    /// The CRC of the kept place on screen that this PC has no piece for (#878), if one is.
    not_on_pc: Option<u32>,
    /// The CRCs whose bytes were asked of the keyboard this run: once each.
    asked: std::collections::HashSet<u32>,
    /// What the right screen showed at the last read, by the keyboard's report
    /// (`live_parity::keyboard_shows`); `None` with no piece on it.
    keyboard: Option<String>,
}

/// What the frames were read for: the source, the name the state gives it (the wallpaper
/// draws frames only for the piece `state.art` names, so the two come from one snapshot), and
/// the library's scan (a kept place's piece may arrive with a rescan).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtKey {
    source: Option<super::live_art::Source>,
    name: Option<String>,
    library: u64,
    /// Kept places' bytes read back so far (#878): one may have landed for this place.
    kept: u64,
}

impl ArtWatch {
    /// The piece's `art` body when what plays changed since the last call, else `None`.
    fn read(&mut self) -> Option<Option<wire::Art>> {
        let snap = super::art_service::handle().map(|h| h.snapshot());
        let source = snap.as_ref().and_then(super::live_art::source);
        let name = snap.as_ref().and_then(showing_name);
        // The keyboard's side for the art parity (#1053), at the same read.
        self.keyboard = snap
            .as_ref()
            .and_then(|s| super::live_parity::keyboard_shows(s.connected, s.status));
        // Only a kept place reads the library; the others don't wait on its scans.
        let (library, kept) = match source {
            Some(super::live_art::Source::Slot { .. }) => (
                super::art_library::generation(),
                super::live_art::kept_generation(),
            ),
            _ => (0, 0),
        };
        self.changed(ArtKey {
            source,
            name,
            library,
            kept,
        })
    }

    fn changed(&mut self, key: ArtKey) -> Option<Option<wire::Art>> {
        if self.seen.as_ref() == Some(&key) {
            return None;
        }
        self.seen = Some(key.clone());
        let art = art_body(key.source.as_ref(), key.name);
        self.not_on_pc = match (&key.source, &art) {
            (Some(super::live_art::Source::Slot { crc: Some(crc), .. }), Some(a))
                if a.missing == Some(wire::MISSING_NOT_ON_PC) =>
            {
                Some(*crc)
            }
            _ => None,
        };
        Some(art)
    }

    /// The kept place on screen has no frames because this PC has no piece for it (#878): while
    /// `watched` (a paired wallpaper that isn't paused, Foxglove not paused), the CRC to ask the
    /// keyboard for, once per CRC per run.
    fn take_read(&mut self, watched: bool) -> Option<u32> {
        let crc = self.not_on_pc.filter(|_| watched)?;
        self.asked.insert(crc).then_some(crc)
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

/// The keyboard the feed is about: its pin, else the focus (#801).
fn feed_keyboard() -> Option<super::keyboard_focus::Keyboard> {
    super::keyboard_focus::for_view(super::keyboard_focus::View::Feed)
}

/// The board for `fed`: the overlay's geometry while it is the host-link keyboard (its keys map
/// onto it), else the keyboard's own layout, drawn only.
fn board_for(fed: Option<&super::keyboard_focus::Keyboard>) -> Option<Geometry> {
    match fed {
        Some(k) if !super::focus_pins::host_link(Some(k)) => Some((
            board_of(&k.name, &super::keyboard_focus::layout(k)),
            Vec::new(),
        )),
        _ => GEOMETRY.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    }
}

/// A keyboard's layout as the feed's board: its parts side by side, a key's width apart.
fn board_of(name: &str, layout: &super::key_layout::KeyLayout) -> wire::Board {
    let mut keys = Vec::new();
    let mut x0 = 0.0;
    for part in &layout.parts {
        let mut right: f32 = 0.0;
        for k in part {
            let r = k.rect;
            keys.push(wire::key_rect(
                r.min.x + x0,
                r.min.y,
                r.width(),
                r.height(),
                k.r,
            ));
            right = right.max(k.bounds().max.x);
        }
        if !part.is_empty() {
            x0 += right + 1.0;
        }
    }
    wire::Board {
        name: name.to_string(),
        keys,
    }
}

/// The state for the feed's keyboard `fed` from the host link's `host` state: all of it for the
/// host-link keyboard, only its name and whether it's there for any other.
fn state_for(host: wire::State, fed: Option<&super::keyboard_focus::Keyboard>) -> wire::State {
    let keyboard = fed.map(|k| wire::Keyboard {
        name: k.name.clone(),
        source: k.source.as_str().into(),
        connected: k.connected,
    });
    if super::focus_pins::host_link(fed) {
        return wire::State { keyboard, ..host };
    }
    wire::State {
        link: wire::Link {
            connected: fed.is_some_and(|k| k.connected),
            ..wire::Link::default()
        },
        keyboard,
        ..wire::State::default()
    }
}

fn read_state(facts: &Facts) -> wire::State {
    state_for(read_host_state(facts), feed_keyboard().as_ref())
}

/// The host-link keyboard's facts (`keyboard.*`) and the right screen's art.
fn read_host_state(facts: &Facts) -> wire::State {
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
            profile_connected: match facts.get("keyboard.profile_connected") {
                Some(Value::Bool(b)) => Some(b),
                _ => None,
            },
        },
        lights: lights(facts),
        keyboard: None,
    }
}

/// The keyboard's lights from `keyboard.lighting.*` (`keyboard_facts`), once it reported them.
fn lights(facts: &Facts) -> Option<wire::Lights> {
    let text = |k: &str| match facts.get(k) {
        Some(Value::Text(t)) => Some(t),
        _ => None,
    };
    let flag = |k: &str| matches!(facts.get(k), Some(Value::Bool(true)));
    Some(wire::Lights {
        on: flag("keyboard.lighting.on"),
        color: text("keyboard.lighting.color")?,
        brightness: match facts.get("keyboard.lighting.brightness") {
            Some(Value::Int(n)) => u8::try_from(n.clamp(0, 100)).unwrap_or(0),
            _ => 0,
        },
        effect: text("keyboard.lighting.effect").unwrap_or_default(),
        rainbow: flag("keyboard.lighting.rainbow"),
    })
}

/// The `art` message's body for what plays (`live_art::source`) under the name `state.art`
/// gives it: `None` (`art: null`) with nothing Foxglove can name. A pushed set Foxglove has no
/// record of has no `state.art` name; its art goes out as "A piece from this PC" (#1051's
/// promise, the tray panel's words), not `""` (#1075).
fn art_body(source: Option<&super::live_art::Source>, name: Option<String>) -> Option<wire::Art> {
    use super::live_art::Source;
    source.map(|s| {
        let name = match (s, name) {
            (_, Some(n)) => n,
            (Source::Pushed, None) => super::art_pieces::UNKNOWN_PUSHED.into(),
            (_, None) => String::new(),
        };
        super::live_art::art(s, name)
    })
}

/// The feed's two art facts for one snapshot, as the hub sends them (#1075): the `state`
/// message (`state.art`, as `read_host_state` names it; the rest left default) and the `art`
/// message (as `ArtWatch` makes it), both as wire JSON. Tests read the published fields from
/// these, the same ones the lab's `feed_art_facts` reads, not from the art service's snapshot.
#[cfg(test)]
pub(super) fn art_messages(s: &super::art_service::Snapshot) -> (String, String) {
    let name = showing_name(s);
    let art = art_body(super::live_art::source(s).as_ref(), name.clone());
    let state = wire::State {
        art: name,
        ..wire::State::default()
    };
    (Message::State(state).json(), Message::Art { art }.json())
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

/// A connected wallpaper reports itself paused: Wallpaper Engine is paused, by the user or
/// otherwise (#1120: a lease's `play` never goes over it).
pub(super) fn wallpaper_paused() -> bool {
    ANY_PAUSED.load(Ordering::Relaxed)
}

static ANY_PAUSED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) fn fullscreen_now() -> bool {
    FAKE_FULLSCREEN.load(Ordering::SeqCst)
}

/// A full-screen program is in front: a game, a video or a presentation.
#[cfg(all(windows, not(test)))]
pub(super) fn fullscreen_now() -> bool {
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
pub(super) fn fullscreen_now() -> bool {
    false
}

// ------------------------------------------------------------------------------ settings

/// Why the pairing token changed, for the log line (never the token itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenReason {
    /// Turned on for the first time: no saved section yet.
    FirstEnable,
    /// "New token" was clicked.
    NewToken,
    /// Turned on with a saved section but no token in it.
    EmptyOnEnable,
}

impl TokenReason {
    fn words(self) -> &'static str {
        match self {
            TokenReason::FirstEnable => "first enable",
            TokenReason::NewToken => "New token",
            TokenReason::EmptyOnEnable => "empty on enable",
        }
    }
}

/// The settings turned on or off. Turning on makes a token when there is none, and says why:
/// `saved` is whether the store has the section.
fn turned(
    mut s: LiveSettings,
    on: bool,
    saved: bool,
) -> Result<(LiveSettings, Option<TokenReason>), String> {
    s.enabled = on;
    if !(on && s.token.is_empty()) {
        return Ok((s, None));
    }
    s.token = wire::new_token().ok_or("no random source for the pairing token")?;
    let why = if saved {
        TokenReason::EmptyOnEnable
    } else {
        TokenReason::FirstEnable
    };
    Ok((s, Some(why)))
}

fn token_line(why: TokenReason) -> String {
    format!("Foxglove: live feed: new pairing token ({})", why.words())
}

/// A change from the Settings card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Turn(bool),
    NewToken,
}

/// Applies a change to the settings as the store has them now: `load` reads them first, and
/// when it fails nothing happens (no token made, nothing saved or started; #741). `store`
/// saves the result before it's answered, so a token handed out is always the saved one.
fn change(
    what: Change,
    load: impl FnOnce() -> Result<Option<LiveSettings>, String>,
    store: impl FnOnce(&LiveSettings) -> Result<(), String>,
) -> Result<(LiveSettings, Option<TokenReason>), String> {
    let saved = load().map_err(|e| format!("settings unreadable: {e}; try again in a moment"))?;
    let present = saved.is_some();
    let current = saved.unwrap_or_default();
    let (s, why) = match what {
        Change::Turn(on) => turned(current, on, present)?,
        Change::NewToken => {
            let mut s = current;
            s.token = wire::new_token().ok_or("no random source for the pairing token")?;
            (s, Some(TokenReason::NewToken))
        }
    };
    store(&s)?;
    Ok((s, why))
}

fn apply(what: Change) -> Result<(), String> {
    let (s, why) = change(
        what,
        || super::store::section(STORE_OWNER, SECTION).load(),
        save,
    )?;
    if let Some(why) = why {
        eprintln!("{}", token_line(why));
    }
    match (what, s.enabled) {
        (Change::Turn(false), _) => {
            stop();
            Ok(())
        }
        (_, true) => listen(&s),
        (_, false) => Ok(()),
    }
}

/// Turns the feed on or off; turning it on makes the token the first time.
fn set_enabled(on: bool) -> Result<(), String> {
    apply(Change::Turn(on))
}

/// A new token: wallpapers paired with the old one are let go.
fn new_token() -> Result<(), String> {
    apply(Change::NewToken)
}

/// What the settings card says the feed sends.
pub const EXPLAINER: &str = "A live wallpaper paired with Foxglove shows your typing as ripples, with the layer, the art screen's art, battery, connection and the keyboard's light colour. It gets which key position was pressed and when, never the letter or keycode, and only with the pairing token. Someone who knows your keymap could still tell the keys apart. Nothing is sent while Foxglove is paused, the PC is locked, the screen is off or a full-screen game runs.";

/// The notification switch's label and hover.
pub const NOTIFY_LABEL: &str = "Show notifications on the wallpaper when it's on screen";
pub const NOTIFY_HOVER: &str = "Foxglove's own notifications, such as a low battery, show on a paired wallpaper that can draw them. When none is on screen, or it doesn't answer within 2 seconds, or a full-screen game, a locked PC or a pause holds the feed, they show as Windows notifications instead.";

/// "Show notifications on the wallpaper" changed.
fn set_notify(on: bool) -> Result<(), String> {
    let mut s = settings();
    s.notify = on;
    save(&s)
}

/// The notification switch, apart so the review render draws the same row.
pub fn notify_row(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    ui.checkbox(on, NOTIFY_LABEL).on_hover_text(NOTIFY_HOVER)
}

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
        // What the store holds: a refused change shows no token (#741).
        let s = settings();
        if s.enabled {
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
            // #801: which keyboard the wallpaper shows, the focus or a pin.
            super::focus_pins::feed_row(ui);
            let mut notify = s.notify;
            if notify_row(ui, &mut notify).changed() {
                error = set_notify(notify).err();
            }
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

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "wallpaper-engine";

#[cfg(test)]
mod tests {
    use super::*;

    /// One upgrade request through `handshake` on a loopback socket: what came back, and what
    /// the feed made of it.
    fn try_handshake(origin: &str, path: &str, token: &str) -> (Vec<u8>, Result<(), String>) {
        use std::io::Read;
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: foxglove-live.v1\r\nOrigin: {origin}\r\n\r\n"
        );
        let client = std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            c.write_all(request.as_bytes()).unwrap();
            let mut got = Vec::new();
            let _ = c.read_to_end(&mut got);
            got
        });
        let (mut stream, _) = listener.accept().unwrap();
        let r = handshake(&mut stream, token).map_err(|e| match e {
            NotAdmitted::Token => "token".to_string(),
            NotAdmitted::Other(e) => e,
        });
        let _ = stream.shutdown(Shutdown::Both);
        drop(stream);
        (client.join().unwrap(), r)
    }

    /// A wallpaper with an old token: the upgrade completes so a browser can see why, it hears
    /// `refused` and a close 4001, and nothing else (no hello, nothing about the token).
    #[test]
    fn a_wrong_token_is_told_refused_and_closed_4001() {
        let token = "s3cretS3cretS3cretS3cret";
        let (got, r) = try_handshake("null", "/live?token=old", token);
        assert_eq!(r, Err("token".into()));
        let text = String::from_utf8_lossy(&got);
        let (head, frames) = got.split_at(text.find("\r\n\r\n").unwrap() + 4);
        let head = String::from_utf8_lossy(head);
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        assert!(head.contains("Sec-WebSocket-Protocol: foxglove-live.v1"));
        let body = br#"{"t":"refused","why":"token"}"#;
        let mut want = vec![0x81, body.len() as u8];
        want.extend_from_slice(body);
        want.extend_from_slice(&[0x88, 7, 0x0f, 0xa1]);
        want.extend_from_slice(b"token");
        assert_eq!(
            frames,
            want.as_slice(),
            "refused, then close 4001, and no more"
        );
        assert!(!text.contains(token));
        assert_eq!(wire::CLOSE_REFUSED, 4001);

        // No token at all: the same.
        let (got, r) = try_handshake("null", "/live", token);
        assert_eq!(r, Err("token".into()));
        assert!(String::from_utf8_lossy(&got).contains(r#"{"t":"refused","why":"token"}"#));
    }

    /// A web page is refused by its origin with a plain 403: no WebSocket, no `refused`.
    #[test]
    fn a_web_page_gets_a_plain_403() {
        let token = "s3cretS3cretS3cretS3cret";
        let (got, r) = try_handshake("https://example.com", "/live?token=old", token);
        assert!(r.unwrap_err().contains("isn't a local wallpaper"));
        let text = String::from_utf8_lossy(&got);
        assert!(text.starts_with("HTTP/1.1 403"), "{text}");
        assert!(!text.contains("refused"));
        // The right token: admitted.
        let (got, r) = try_handshake("null", &format!("/live?token={token}"), token);
        assert_eq!(r, Ok(()));
        assert!(String::from_utf8_lossy(&got).starts_with("HTTP/1.1 101"));
    }

    /// The health row: a warning only while a wallpaper keeps presenting an old token.
    #[test]
    fn the_health_row_names_an_old_pairing_token() {
        assert!(refusal_rows(false).is_empty());
        let rows = refusal_rows(true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].level, super::super::health::Level::Warn);
        assert_eq!(rows[0].detail, "a wallpaper is using an old pairing token");
        assert_eq!(
            super::super::health_notice::text(&rows[0]),
            (
                "Wallpaper needs the new pairing token".to_string(),
                "Copy it from App › Live wallpaper and paste it into the wallpaper's Foxglove pairing token."
                    .to_string()
            )
        );
    }

    /// #587: a hub that panics drops its tap, which moves the shared generation on, so the old
    /// forwarder ends and the restarted hub's first tap can't match its queued keys.
    #[test]
    fn a_dropped_key_tap_retires_its_generation() {
        let generation = Arc::new(AtomicU64::new(0));
        let tap = KeyTap::new(generation.clone());
        assert_eq!(tap.generation(), 0);
        drop(tap);
        let restarted = KeyTap::new(generation.clone());
        assert_eq!(restarted.generation(), 1);
        assert_eq!(generation.load(Ordering::SeqCst), 1);
    }
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

    /// #741 (pid 34688): with the store unreadable, turning the feed on makes no token and
    /// starts nothing; once it reads, the token made is saved and is the one the feed admits.
    #[test]
    fn no_token_is_handed_out_while_the_settings_are_unreadable() {
        use crate::foxglove::store;
        let dir = store::test_config_dir().join("live-unreadable");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("foxglove.toml");
        let old = LiveSettings {
            enabled: false,
            token: "the-saved-one".into(),
            ..LiveSettings::default()
        };
        store::save_at(&path, SECTION, &old, false).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let load = || store::try_load_at::<LiveSettings>(&path, SECTION);
        let save =
            |s: &LiveSettings| store::save_at(&path, SECTION, s, false).map_err(|e| e.to_string());

        store::FAIL_READS.lock().unwrap().insert(path.clone());
        for what in [Change::Turn(true), Change::NewToken] {
            let mut saved = false;
            let err = change(what, load, |_| {
                saved = true;
                Ok(())
            })
            .unwrap_err();
            assert!(err.starts_with("settings unreadable: "), "{err}");
            assert!(!saved, "nothing to save, no token made");
        }
        // The store's health row says so meanwhile.
        let rows = store::rows_for_test(&path);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].detail.starts_with("settings unreadable: ")
                && rows[0]
                    .detail
                    .ends_with("(not saved until read: [live_wallpaper])"),
            "{}",
            rows[0].detail
        );
        store::FAIL_READS.lock().unwrap().remove(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // The read recovers: enabling keeps the saved token, saves it, and the feed admits it.
        let (s, why) = change(Change::Turn(true), load, save).unwrap();
        assert_eq!((s.token.as_str(), why), ("the-saved-one", None));
        assert!(store::rows_for_test(&path).is_empty());
        // A new token: saved before it's shown, and the one admitted.
        let (s, why) = change(Change::NewToken, load, save).unwrap();
        assert_eq!(why, Some(TokenReason::NewToken));
        let on_disk = load().unwrap().unwrap();
        assert_eq!(on_disk.token, s.token);
        assert!(on_disk.enabled);
        assert!(wire::admit(None, Some(&on_disk.token), &s.token).is_ok());
        assert!(wire::admit(None, Some("the-saved-one"), &s.token).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #741: the token survives unrelated saves, a failed read at start followed by an enable,
    /// and a restart. A temp store file, never the user's.
    #[test]
    fn the_pairing_token_survives_a_failed_read_and_a_restart() {
        use crate::foxglove::store;
        let dir = store::test_config_dir().join("live-token");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("foxglove.toml");
        let load = || store::try_load_at::<LiveSettings>(&path, SECTION);

        // First enable: no section yet, so a token, said as the first enable.
        let mut cache = None;
        let (s, why) = turned(cached(&mut cache, load), true, false).unwrap();
        assert_eq!(why, Some(TokenReason::FirstEnable));
        assert_eq!(
            token_line(why.unwrap()),
            "Foxglove: live feed: new pairing token (first enable)"
        );
        store::save_at(&path, SECTION, &s, false).unwrap();
        let token = s.token.clone();
        assert!(!token.is_empty());

        // Unrelated sections saved after it leave it alone.
        store::save_at(&path, "screens", &toml::Table::new(), false).unwrap();
        store::save_at(&path, "art", &toml::Table::new(), false).unwrap();
        assert_eq!(load().unwrap().unwrap().token, token);

        // A start whose read fails: defaults, not kept.
        store::FAIL_READS.lock().unwrap().insert(path.clone());
        let mut cache = None;
        let at_start = cached(&mut cache, load);
        assert!(at_start.token.is_empty() && cache.is_none());
        // Turning it on then would make a token, but the store refuses to save it.
        let (s, why) = turned(at_start, true, true).unwrap();
        assert_eq!(why, Some(TokenReason::EmptyOnEnable));
        assert!(store::save_at(&path, SECTION, &s, false).is_err());
        store::FAIL_READS.lock().unwrap().remove(&path);
        assert!(
            store::save_at(&path, SECTION, &s, false).is_err(),
            "still refused until the section is read"
        );
        // The next look reads the store again and gets the saved token; turning on keeps it.
        let s = cached(&mut cache, load);
        assert_eq!(s.token, token);
        let (s, why) = turned(s, true, true).unwrap();
        assert_eq!((s.token.as_str(), why), (token.as_str(), None));
        store::save_at(&path, SECTION, &s, false).unwrap();

        // A restart: a fresh cache reads the same token.
        let mut restarted = None;
        assert_eq!(cached(&mut restarted, load).token, token);
        assert_eq!(
            token_line(TokenReason::NewToken),
            "Foxglove: live feed: new pairing token (New token)"
        );
        let _ = std::fs::remove_dir_all(&dir);
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

    /// #801: the feed names its keyboard; for one that isn't the connected host-link keyboard
    /// (a VIA keyboard in focus, or a pin to one) it sends no keys and none of the host link's
    /// facts, only the name and its own shape.
    #[test]
    fn the_feed_names_its_keyboard_and_sends_another_keyboards_shape_only() {
        use crate::foxglove::keyboard_focus::{Keyboard, Source};
        let kb = |name: &str, source, connected| Keyboard {
            key: name.into(),
            name: name.into(),
            source,
            vid: 0x3434,
            pid: 0x0250,
            device: None,
            connected,
        };
        let host = wire::State {
            layer: Some(wire::Layer {
                index: 3,
                name: "NAV".into(),
            }),
            art: Some("Fox run".into()),
            link: wire::Link {
                connected: true,
                ..wire::Link::default()
            },
            ..wire::State::default()
        };
        let host_kb = kb("Eyelash Corne", Source::HostLink, true);
        let s = state_for(host.clone(), Some(&host_kb));
        assert_eq!(s.layer, host.layer, "the host link's facts");
        assert_eq!(
            s.keyboard,
            Some(wire::Keyboard {
                name: "Eyelash Corne".into(),
                source: "host_link".into(),
                connected: true,
            })
        );
        assert_eq!(
            state_for(host.clone(), None),
            host,
            "no keyboard yet: as before"
        );
        let k5 = kb("K5 Pro", Source::Via, true);
        let s = state_for(host.clone(), Some(&k5));
        assert_eq!((s.layer, s.art), (None, None), "not the K5 Pro's");
        assert!(s.link.connected);
        assert_eq!(s.keyboard.unwrap().source, "via");
        let gone = kb("Eyelash Corne", Source::HostLink, false);
        let s = state_for(host, Some(&gone));
        assert_eq!(s.layer, None, "a greyed keyboard has no facts");
        assert!(!s.link.connected);

        // Its shape: the parts side by side, a key apart.
        let layout =
            crate::foxglove::key_layout::of(Some(&crate::foxglove::fixtures::users_desk::board()));
        let board = board_of("Eyelash Corne", &layout);
        let n: usize = layout.parts.iter().map(Vec::len).sum();
        assert_eq!(board.keys.len(), n);
        let left_edge = |k: &wire::KeyRect| k[0];
        let split = layout.parts[0].len();
        assert!(
            board.keys[split..]
                .iter()
                .map(left_edge)
                .fold(f32::MAX, f32::min)
                > board.keys[..split]
                    .iter()
                    .map(left_edge)
                    .fold(f32::MIN, f32::max),
            "the right half to the right of the left"
        );

        // No keys while the feed shows another keyboard.
        let mut hub = Hub {
            board: Some(corne()),
            foreign: true,
            ..Hub::default()
        };
        let mut out = Sent::default();
        assert!(hub.join(1, &mut out));
        assert!(!hub.wants_keys());
        assert!(!hub.key(0, 2, true, 100));
        assert!(hub.batch.is_empty());
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

    /// The right screen's frames go out once per change, to a joining or returning wallpaper,
    /// and after a quiet spell only when the piece changed during it.
    #[test]
    fn art_frames_go_out_once_per_change() {
        let piece = |n: &str| wire::Art {
            name: n.into(),
            from: wire::FROM_FILE,
            nva: Some("TlZBMQ==".into()),
            missing: None,
        };
        let mut hub = Hub::default();
        let mut out = Sent::default();
        hub.join(1, &mut out);
        assert_eq!(out.count(1), 1, "no art yet: the hello only");
        hub.set_art(Some(piece("Fox run")), &mut out);
        assert!(out
            .last(1)
            .starts_with(r#"{"t":"art","art":{"name":"Fox run""#));
        let n = out.count(1);
        assert!(hub.set_art(Some(piece("Fox run")), &mut out).is_empty());
        hub.set_state(
            wire::State {
                battery: wire::Battery {
                    left: Some(50),
                    right: None,
                },
                ..wire::State::default()
            },
            &mut out,
        );
        assert_eq!(
            out.count(1),
            n + 1,
            "a state change doesn't resend the frames"
        );
        hub.join(2, &mut out);
        assert_eq!(out.count(2), 2, "a new wallpaper: hello, then the frames");
        assert!(out.last(2).contains("Fox run"));
        hub.paused(2, true, &mut out);
        hub.paused(2, false, &mut out);
        assert_eq!(out.count(2), 4, "back from a pause: both again");
        // Quiet: held, and sent on live only because it changed.
        hub.gates(Some(Why::Away), &mut out);
        hub.set_art(Some(piece("Radar")), &mut out);
        let quiet = out.count(1);
        hub.gates(None, &mut out);
        assert_eq!(out.count(1), quiet + 3, "live, state, art");
        assert!(out.last(1).contains("Radar"));
        hub.gates(Some(Why::Away), &mut out);
        let quiet = out.count(1);
        hub.gates(None, &mut out);
        assert_eq!(out.count(1), quiet + 2, "unchanged: live and state only");
        hub.set_art(None, &mut out);
        assert_eq!(out.last(1), r#"{"t":"art","art":null}"#);
    }

    /// the Info screen: sent on change, to joining wallpapers, after a quiet spell only when it
    /// changed; paced to one frame a PANEL_MS.
    #[test]
    fn the_left_panel_goes_out_paced() {
        let frame = |b: u8| wire::Panel::of(8, 1, 1, &[b]);
        let mut hub = Hub::default();
        let mut out = Sent::default();
        hub.join(1, &mut out);
        hub.set_panel(frame(1), &mut out);
        assert!(out
            .last(1)
            .starts_with(r#"{"t":"panel","panel":{"w":8,"h":1"#));
        let n = out.count(1);
        assert!(
            hub.set_panel(frame(1), &mut out).is_empty(),
            "the same frame"
        );
        assert_eq!(out.count(1), n);
        hub.join(2, &mut out);
        assert_eq!(out.count(2), 2, "hello, then the panel");
        hub.gates(Some(Why::Away), &mut out);
        hub.set_panel(None, &mut out);
        let q = out.count(1);
        hub.gates(None, &mut out);
        assert_eq!(out.count(1), q + 3, "live, state, panel");
        assert_eq!(out.last(1), r#"{"t":"panel","panel":null}"#);

        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut pace = PanelPace::default();
        pace.changed(t0);
        assert!(pace.take(t0), "the first goes at once");
        pace.changed(ms(50));
        assert!(!pace.take(ms(50)), "held");
        pace.changed(ms(120));
        assert_eq!(
            pace.due,
            Some(ms(wire::PANEL_MS)),
            "one wake for the window"
        );
        assert!(!pace.take(ms(200)));
        assert!(
            pace.take(ms(wire::PANEL_MS)),
            "the latest at the window's end"
        );
        assert!(!pace.take(ms(400)), "nothing more");
        pace.changed(ms(900));
        assert!(pace.take(ms(900)), "after a quiet window: at once");
    }

    /// The art file is read only when the source changes, not on every state change; a new name
    /// for the same source (the frames go only with the name `state.art` gives) or a library
    /// rescan reads it again.
    #[test]
    fn art_is_read_only_when_the_piece_changes() {
        use super::super::live_art::Source;
        let key = |source: Option<Source>, name: &str, library: u64| ArtKey {
            source,
            name: (!name.is_empty()).then(|| name.to_string()),
            library,
            kept: 0,
        };
        let mut w = ArtWatch::default();
        assert_eq!(
            w.changed(key(None, "", 0)),
            Some(None),
            "no keyboard: art null"
        );
        assert_eq!(w.changed(key(None, "", 0)), None, "nothing, still nothing");
        let emblem = Some(Source::BuiltIn(0, 1));
        let first = w.changed(key(emblem.clone(), "Bandit emblem", 0));
        let art = first.expect("changed").expect("a piece");
        assert_eq!(art.name, "Bandit emblem");
        assert_eq!((art.from, art.missing), (wire::FROM_BUILT_IN, None));
        assert!(art.nva.is_some(), "the firmware's own frames");
        assert_eq!(w.changed(key(emblem.clone(), "Bandit emblem", 0)), None);
        let renamed = w.changed(key(emblem, "piece 1", 0)).expect("named again");
        assert_eq!(
            renamed.unwrap().name,
            "piece 1",
            "the frames follow the state's name"
        );
        let kept = Some(Source::Slot {
            name: "Radar".into(),
            len: 900,
            crc: Some(0x1234_5678),
        });
        let a = w.changed(key(kept.clone(), "Radar", 3)).unwrap().unwrap();
        assert_eq!(a.from, wire::FROM_KEPT);
        assert_eq!(
            (a.nva, a.missing),
            (None, Some(wire::MISSING_NOT_ON_PC)),
            "no piece on this PC has its CRC: a labelled placeholder"
        );
        assert_eq!(w.changed(key(kept.clone(), "Radar", 3)), None);
        assert!(
            w.changed(key(kept, "Radar", 4)).is_some(),
            "a rescan reads it again"
        );
        // #878: its bytes are asked of the keyboard only while a wallpaper watches, and once.
        assert_eq!(w.take_read(false), None, "nobody watching");
        assert_eq!(w.take_read(true), Some(0x1234_5678));
        assert_eq!(w.take_read(true), None, "once per CRC per run");
        assert_eq!(w.changed(key(None, "", 0)), Some(None), "the keyboard went");
        assert_eq!(w.take_read(true), None, "nothing kept on screen");
        // Older firmware (no CRC): nothing to check a read against, so nothing is asked.
        let old = Some(Source::Slot {
            name: "Radar".into(),
            len: 900,
            crc: None,
        });
        w.changed(key(old, "Radar", 4));
        assert_eq!(w.take_read(true), None);
    }

    /// #40: the keyboard's lights reach the wallpaper once the keyboard reported them.
    #[test]
    fn the_state_carries_the_keyboard_lights_once_known() {
        let facts = Facts::default();
        assert_eq!(lights(&facts), None, "left out until reported");
        facts.set("keyboard.lighting.on", Value::Bool(true));
        facts.set("keyboard.lighting.color", Value::Text("#ff6a00".into()));
        facts.set("keyboard.lighting.brightness", Value::Int(60));
        facts.set("keyboard.lighting.effect", Value::Text("Solid".into()));
        facts.set("keyboard.lighting.rainbow", Value::Bool(false));
        let state = wire::State {
            lights: lights(&facts),
            ..wire::State::default()
        };
        assert!(
            Message::State(state).json().ends_with(
                r##""lights":{"on":true,"color":"#ff6a00","brightness":60,"effect":"Solid","rainbow":false}}"##
            ),
        );
        facts.remove_prefix("keyboard.", &[]);
        assert_eq!(lights(&facts), None, "gone with the keyboard");
    }

    fn note(title: &str) -> Note {
        Note {
            title: title.into(),
            body: "14% left. Charge it soon.".into(),
            level: wire::Level::Info,
        }
    }

    /// A hub with wallpaper 1 that declared `notify` and wallpaper 2 that didn't.
    fn notify_hub(out: &mut Sent) -> Hub {
        let mut hub = Hub::default();
        hub.join(1, out);
        hub.join(2, out);
        hub.caps(1, true);
        hub.caps(2, false);
        hub
    }

    /// The route to the wallpaper: only the one that declared the cap gets it, and its ack
    /// means no Windows notification.
    #[test]
    fn a_notification_goes_to_the_capable_wallpaper_and_its_ack_holds_the_toast() {
        let mut out = Sent::default();
        let mut hub = notify_hub(&mut out);
        let now = Instant::now();
        let (fallback, dead) = hub.notify(note("Right half battery low"), now, &mut out);
        assert_eq!(fallback, None, "on the wallpaper, not Windows");
        assert!(dead.is_empty());
        assert_eq!(
            out.last(1),
            r#"{"t":"notify","id":1,"title":"Right half battery low","body":"14% left. Charge it soon.","level":"info"}"#
        );
        assert_eq!(out.count(2), 1, "only the hello: no cap, no text");
        assert_eq!(hub.next_due(), Some(now + wire::ACK_LIMIT));
        hub.notified(1);
        assert_eq!(hub.next_due(), None);
        assert!(
            hub.expired(now + wire::ACK_LIMIT * 2).is_empty(),
            "no toast"
        );
    }

    #[test]
    fn no_ack_in_time_gives_the_toast() {
        let mut out = Sent::default();
        let mut hub = notify_hub(&mut out);
        let now = Instant::now();
        assert_eq!(hub.notify(note("a"), now, &mut out).0, None);
        assert!(
            hub.expired(now + wire::ACK_LIMIT / 2).is_empty(),
            "still waiting"
        );
        hub.notified(99);
        assert_eq!(hub.expired(now + wire::ACK_LIMIT), vec![note("a")]);
        assert!(hub.expired(now + wire::ACK_LIMIT * 3).is_empty(), "once");
    }

    #[test]
    fn without_a_capable_wallpaper_it_is_a_toast() {
        let now = Instant::now();
        // Nobody connected.
        let mut out = Sent::default();
        assert_eq!(
            Hub::default().notify(note("a"), now, &mut out).0,
            Some(note("a"))
        );
        // Connected without the cap, or the capable one paused or gone.
        let mut hub = notify_hub(&mut out);
        hub.paused(1, true, &mut out);
        assert_eq!(hub.notify(note("b"), now, &mut out).0, Some(note("b")));
        hub.paused(1, false, &mut out);
        hub.leave(1);
        assert_eq!(hub.notify(note("c"), now, &mut out).0, Some(note("c")));
        // A cap from a client that never joined counts for nothing.
        hub.caps(7, true);
        assert_eq!(hub.notify(note("d"), now, &mut out).0, Some(note("d")));
        // The capable one's write fails: a toast at once, and it's dropped.
        let mut hub = notify_hub(&mut out);
        out.dead.push(1);
        let (fallback, dead) = hub.notify(note("e"), now, &mut out);
        assert_eq!((fallback, dead), (Some(note("e")), vec![1]));
        assert_eq!(hub.next_due(), None);
    }

    #[test]
    fn a_quiet_feed_gives_the_toast() {
        for why in [Why::Paused, Why::Away, Why::Fullscreen] {
            let mut out = Sent::default();
            let mut hub = notify_hub(&mut out);
            hub.gates(Some(why), &mut out);
            let told = out.count(1);
            assert_eq!(
                hub.notify(note("a"), Instant::now(), &mut out).0,
                Some(note("a")),
                "{why:?}"
            );
            assert_eq!(out.count(1), told, "{why:?}: nothing sent");
        }
    }

    /// The setting off: [`notify`] hands the note back, and `notice::show` gives it to the
    /// toast (the end-to-end test sees the fake sink get the fallbacks).
    #[test]
    fn the_setting_off_gives_the_toast() {
        crate::foxglove::store::test_config_dir();
        assert!(LiveSettings::default().notify, "on by default");
        let parsed: LiveSettings = toml::from_str("enabled = true").unwrap();
        assert!(parsed.notify, "a store from before keeps it on");
        let off = LiveSettings {
            notify: false,
            ..LiveSettings::default()
        };
        assert_eq!(notify_with(&off, note("off")), Err(note("off")));
        assert!(NOTIFY_HOVER.contains("Windows notifications instead"));
        assert!(!NOTIFY_LABEL.contains("Bandit"), "any capable wallpaper");
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
        // A wrong token completes the upgrade so a browser can be told why (`refused`, 4001).
        assert!(ask("/live?token=wrong", Some("null"), None).contains("101"));
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
            ..LiveSettings::default()
        })
        .unwrap();
        let mut wrong = StandIn::connect(port, "wrong").expect("upgraded, to be told why");
        assert_eq!(
            wrong.next(Duration::from_secs(5)).as_deref(),
            Some(r#"{"t":"refused","why":"token"}"#),
            "a wrong token"
        );
        // Then the close (4001, "token"), and no hello.
        let close = wrong.next(Duration::from_secs(5)).unwrap();
        assert!(
            close.ends_with("token") && !close.contains("hello"),
            "{close}"
        );
        drop(wrong);
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

        // The stand-in declares `notify`: Foxglove's notification goes to it. Its ack holds the
        // toast; a second it ignores falls back to the (fake) toast sink.
        crate::foxglove::notice::test_toasts::ROUTE.set(true);
        w.say(r#"{"t":"hi","wallpaper":"stand-in","caps":["notify"]}"#);
        let mut sent = None;
        let mut tries = 0;
        for _ in 0..50 {
            tries += 1;
            crate::foxglove::notice::show(
                crate::foxglove::notice::Kind::Battery,
                "LiveE2E acked",
                "drawn",
                wire::Level::Warn,
            );
            while let Some(m) = w.next(Duration::from_millis(100)) {
                if m.starts_with(r#"{"t":"notify""#) {
                    sent = Some(m);
                    break;
                }
            }
            if sent.is_some() {
                break;
            }
        }
        let sent = sent.expect("a notify frame");
        assert!(sent.contains(r#""title":"LiveE2E acked","body":"drawn","level":"warn""#));
        let v: serde_json::Value = serde_json::from_str(&sent).unwrap();
        w.say(&format!(r#"{{"t":"notified","id":{}}}"#, v["id"]));
        crate::foxglove::notice::show(
            crate::foxglove::notice::Kind::Battery,
            "LiveE2E ignored",
            "not drawn",
            wire::Level::Info,
        );
        let deadline = Instant::now() + wire::ACK_LIMIT + Duration::from_secs(3);
        while crate::foxglove::notice::test_toasts::titled("LiveE2E ignored").is_empty()
            && Instant::now() < deadline
        {
            let _ = w.next(Duration::from_millis(100));
        }
        assert_eq!(
            crate::foxglove::notice::test_toasts::titled("LiveE2E ignored").len(),
            1,
            "no ack: the toast"
        );
        // Those sent before the hi arrived (no cap yet) were toasts; the acked one isn't, though
        // its limit has passed.
        assert_eq!(
            crate::foxglove::notice::test_toasts::titled("LiveE2E acked").len(),
            tries - 1
        );

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
                ..LiveSettings::default()
            })
            .expect("the port is free again");
        }
        let mut old = StandIn::connect(port, &token).unwrap();
        assert_eq!(
            old.next(Duration::from_secs(5)).as_deref(),
            Some(r#"{"t":"refused","why":"token"}"#),
            "the old token"
        );
        drop(old);
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
        s.status = Some(foxglove_link::protocol::ArtStatus {
            right_state: foxglove_link::protocol::right_state::PLAYING,
            right_set_count: 4,
            ..Default::default()
        });
        assert_eq!(showing_name(&s).as_deref(), Some("Fox run"));
    }
}
